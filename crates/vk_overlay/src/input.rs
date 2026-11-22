//! X11 / XWayland input capture for the Linux Vulkan overlay layer.
//!
//! Dynamically loads `libX11.so.6` via `dlopen` (zero link-time dependencies).
//! When Aurora opens the overlay (`ToGame::SetOpen(true)`), grabs pointer and
//! keyboard on the game's X11 window via a dedicated `XOpenDisplay(NULL)`
//! connection and streams `wire::Input` events back to Aurora.
//! If the game itself holds a pointer grab (`AlreadyGrabbed`), automatically
//! falls back to polling `XQueryPointer` so mouse movement and clicks still work.

use crate::net::shared;
use crate::wire::{Input, MOD_ALT, MOD_CTRL, MOD_SHIFT};
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

static ACTIVE_WINDOW: AtomicU64 = AtomicU64::new(0);
static SURFACE_IS_WAYLAND: AtomicBool = AtomicBool::new(false);
static OUR_DISPLAY: AtomicUsize = AtomicUsize::new(0);
static PREV_ERR_HANDLER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SurfaceKind {
    X11(u64),
    Wayland,
}

/// Record the game's active window/surface target.
pub fn set_surface(kind: SurfaceKind) {
    match kind {
        SurfaceKind::X11(win) => {
            SURFACE_IS_WAYLAND.store(false, Ordering::Relaxed);
            if win != 0 {
                ACTIVE_WINDOW.store(win, Ordering::Relaxed);
            }
        }
        SurfaceKind::Wayland => {
            SURFACE_IS_WAYLAND.store(true, Ordering::Relaxed);
            ACTIVE_WINDOW.store(0, Ordering::Relaxed);
        }
    }
}

/// Start the background X11 input capture thread once.
pub fn ensure_started() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("aurora-vk-overlay-input".into()).spawn(input_loop);
    });
}

// ---- Minimal libdl + X11 ABI -----------------------------------------------

const RTLD_LAZY: c_int = 1;
const RTLD_LOCAL: c_int = 0;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type Display = c_void;
type Window = c_ulong;
type Cursor = c_ulong;
type Time = c_ulong;
type KeySym = c_ulong;
type Bool = c_int;

const GRAB_SUCCESS: c_int = 0;
const GRAB_MODE_ASYNC: c_int = 1;
const CURRENT_TIME: Time = 0;

const BUTTON_PRESS_MASK: c_uint = 1 << 2;
const BUTTON_RELEASE_MASK: c_uint = 1 << 3;
const POINTER_MOTION_MASK: c_uint = 1 << 6;
const KEY_PRESS_MASK: c_uint = 1 << 0;
const KEY_RELEASE_MASK: c_uint = 1 << 1;

const INPUT_ONLY: c_uint = 2;
const CW_OVERRIDE_REDIRECT: c_ulong = 1 << 9;
const CW_EVENT_MASK: c_ulong = 1 << 11;
const CW_CURSOR: c_ulong = 1 << 14;
const REVERT_TO_PARENT: c_int = 2;
const XC_LEFT_PTR: c_uint = 68;

#[repr(C)]
#[derive(Default)]
struct XSetWindowAttributes {
    background_pixmap: c_ulong,
    background_pixel: c_ulong,
    border_pixmap: c_ulong,
    border_pixel: c_ulong,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: Bool,
    event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: Bool,
    colormap: c_ulong,
    cursor: Cursor,
}

const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const BUTTON_RELEASE: c_int = 5;
const MOTION_NOTIFY: c_int = 6;

const SHIFT_MASK: c_uint = 1 << 0;
const CONTROL_MASK: c_uint = 1 << 2;
const MOD1_MASK: c_uint = 1 << 3;
const BUTTON1_MASK: c_uint = 1 << 8;
const BUTTON2_MASK: c_uint = 1 << 9;
const BUTTON3_MASK: c_uint = 1 << 10;

#[repr(C)]
#[derive(Copy, Clone)]
struct XAnyEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XKeyEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XButtonEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    button: c_uint,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct XMotionEvent {
    ty: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: Time,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    is_hint: c_char,
    same_screen: Bool,
}

#[repr(C)]
union XEvent {
    ty: c_int,
    any: XAnyEvent,
    key: XKeyEvent,
    button: XButtonEvent,
    motion: XMotionEvent,
    pad: [c_long; 24],
}

#[allow(non_snake_case, dead_code)]
struct X11 {
    XOpenDisplay: unsafe extern "C" fn(*const c_char) -> *mut Display,
    XCloseDisplay: unsafe extern "C" fn(*mut Display) -> c_int,
    XFlush: unsafe extern "C" fn(*mut Display) -> c_int,
    XPending: unsafe extern "C" fn(*mut Display) -> c_int,
    XNextEvent: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
    XGrabPointer: unsafe extern "C" fn(*mut Display, Window, Bool, c_uint, c_int, c_int, Window, Cursor, Time) -> c_int,
    XUngrabPointer: unsafe extern "C" fn(*mut Display, Time) -> c_int,
    XGrabKeyboard: unsafe extern "C" fn(*mut Display, Window, Bool, c_int, c_int, Time) -> c_int,
    XUngrabKeyboard: unsafe extern "C" fn(*mut Display, Time) -> c_int,
    XQueryPointer: unsafe extern "C" fn(
        *mut Display,
        Window,
        *mut Window,
        *mut Window,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_uint,
    ) -> Bool,
    XGetGeometry: unsafe extern "C" fn(
        *mut Display,
        Window,
        *mut Window,
        *mut c_int,
        *mut c_int,
        *mut c_uint,
        *mut c_uint,
        *mut c_uint,
        *mut c_uint,
    ) -> c_int,
    XLookupString: unsafe extern "C" fn(*mut XKeyEvent, *mut c_char, c_int, *mut KeySym, *mut c_void) -> c_int,
    XLookupKeysym: unsafe extern "C" fn(*mut XKeyEvent, c_int) -> KeySym,
    XKeysymToKeycode: unsafe extern "C" fn(*mut Display, KeySym) -> u8,
    XQueryKeymap: unsafe extern "C" fn(*mut Display, *mut c_char) -> c_int,
    XSetErrorHandler: Option<unsafe extern "C" fn(Option<unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int>) -> *mut c_void>,
    XDefaultRootWindow: Option<unsafe extern "C" fn(*mut Display) -> Window>,
    XCreateWindow: Option<
        unsafe extern "C" fn(
            *mut Display,
            Window,
            c_int,
            c_int,
            c_uint,
            c_uint,
            c_uint,
            c_int,
            c_uint,
            *mut c_void,
            c_ulong,
            *mut XSetWindowAttributes,
        ) -> Window,
    >,
    XMapRaised: Option<unsafe extern "C" fn(*mut Display, Window) -> c_int>,
    XDestroyWindow: Option<unsafe extern "C" fn(*mut Display, Window) -> c_int>,
    XResizeWindow: Option<unsafe extern "C" fn(*mut Display, Window, c_uint, c_uint) -> c_int>,
    XSetInputFocus: Option<unsafe extern "C" fn(*mut Display, Window, c_int, Time) -> c_int>,
    XGetInputFocus: Option<unsafe extern "C" fn(*mut Display, *mut Window, *mut c_int) -> c_int>,
    XQueryTree: Option<unsafe extern "C" fn(*mut Display, Window, *mut Window, *mut Window, *mut *mut Window, *mut c_uint) -> c_int>,
    XFree: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    XTranslateCoordinates: Option<
        unsafe extern "C" fn(*mut Display, Window, Window, c_int, c_int, *mut c_int, *mut c_int, *mut Window) -> Bool,
    >,
    XCreateFontCursor: Option<unsafe extern "C" fn(*mut Display, c_uint) -> Cursor>,
    XFreeCursor: Option<unsafe extern "C" fn(*mut Display, Cursor) -> c_int>,
}

#[repr(C)]
struct XErrorEvent {
    ty: c_int,
    display: *mut Display,
}

unsafe extern "C" fn ignore_x11_error(dpy: *mut Display, err: *mut c_void) -> c_int {
    let ours = OUR_DISPLAY.load(Ordering::Acquire) as *mut Display;
    let err_dpy = if !err.is_null() { unsafe { (*(err as *const XErrorEvent)).display } } else { dpy };
    if !ours.is_null() && (dpy == ours || err_dpy == ours) {
        return 0;
    }
    let prev = PREV_ERR_HANDLER.load(Ordering::Acquire);
    if prev != 0 {
        let f: unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int = unsafe { std::mem::transmute(prev) };
        return unsafe { f(dpy, err) };
    }
    0
}

impl X11 {
    unsafe fn load() -> Option<Self> {
        let lib = unsafe { dlopen(c"libX11.so.6".as_ptr(), RTLD_LAZY | RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        unsafe fn sym<T>(lib: *mut c_void, name: &CStr) -> Option<T> {
            let p = unsafe { dlsym(lib, name.as_ptr()) };
            if p.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&p) }) }
        }
        unsafe {
            Some(Self {
                XOpenDisplay: sym(lib, c"XOpenDisplay")?,
                XCloseDisplay: sym(lib, c"XCloseDisplay")?,
                XFlush: sym(lib, c"XFlush")?,
                XPending: sym(lib, c"XPending")?,
                XNextEvent: sym(lib, c"XNextEvent")?,
                XGrabPointer: sym(lib, c"XGrabPointer")?,
                XUngrabPointer: sym(lib, c"XUngrabPointer")?,
                XGrabKeyboard: sym(lib, c"XGrabKeyboard")?,
                XUngrabKeyboard: sym(lib, c"XUngrabKeyboard")?,
                XQueryPointer: sym(lib, c"XQueryPointer")?,
                XGetGeometry: sym(lib, c"XGetGeometry")?,
                XLookupString: sym(lib, c"XLookupString")?,
                XLookupKeysym: sym(lib, c"XLookupKeysym")?,
                XKeysymToKeycode: sym(lib, c"XKeysymToKeycode")?,
                XQueryKeymap: sym(lib, c"XQueryKeymap")?,
                XSetErrorHandler: sym(lib, c"XSetErrorHandler"),
                XDefaultRootWindow: sym(lib, c"XDefaultRootWindow"),
                XCreateWindow: sym(lib, c"XCreateWindow"),
                XMapRaised: sym(lib, c"XMapRaised"),
                XDestroyWindow: sym(lib, c"XDestroyWindow"),
                XResizeWindow: sym(lib, c"XResizeWindow"),
                XSetInputFocus: sym(lib, c"XSetInputFocus"),
                XGetInputFocus: sym(lib, c"XGetInputFocus"),
                XQueryTree: sym(lib, c"XQueryTree"),
                XFree: sym(lib, c"XFree"),
                XTranslateCoordinates: sym(lib, c"XTranslateCoordinates"),
                XCreateFontCursor: sym(lib, c"XCreateFontCursor"),
                XFreeCursor: sym(lib, c"XFreeCursor"),
            })
        }
    }
}

#[inline]
fn key_down(keymap: &[u8; 32], kc: u8) -> bool {
    kc != 0 && (keymap[(kc / 8) as usize] & (1u8 << (kc % 8))) != 0
}

fn x11_mods(state: c_uint) -> u8 {
    let mut m = 0u8;
    if state & SHIFT_MASK != 0 {
        m |= MOD_SHIFT;
    }
    if state & CONTROL_MASK != 0 {
        m |= MOD_CTRL;
    }
    if state & MOD1_MASK != 0 {
        m |= MOD_ALT;
    }
    m
}

/// Map X11 KeySym to Windows Virtual-Key code (what `apps/desktop/src/ingame.rs` expects).
pub fn keysym_to_vk(sym: u64) -> Option<u32> {
    match sym {
        0xff08 => Some(0x08), // BackSpace -> VK_BACK
        0xff09 | 0xfe20 => Some(0x09), // Tab / ISO_Left_Tab -> VK_TAB
        0xff0d | 0xff8d => Some(0x0D), // Return / KP_Enter -> VK_RETURN
        0xff1b => Some(0x1B), // Escape -> VK_ESCAPE
        0x0020 => Some(0x20), // space -> VK_SPACE
        0xff55 => Some(0x21), // Prior / Page_Up -> VK_PRIOR
        0xff56 => Some(0x22), // Next / Page_Down -> VK_NEXT
        0xff57 => Some(0x23), // End -> VK_END
        0xff50 => Some(0x24), // Home -> VK_HOME
        0xff51 => Some(0x25), // Left -> VK_LEFT
        0xff52 => Some(0x26), // Up -> VK_UP
        0xff53 => Some(0x27), // Right -> VK_RIGHT
        0xff54 => Some(0x28), // Down -> VK_DOWN
        0xff63 => Some(0x2D), // Insert -> VK_INSERT
        0xffff => Some(0x2E), // Delete -> VK_DELETE
        0x0030..=0x0039 => Some(sym as u32), // '0'..'9'
        0x0041..=0x005a => Some(sym as u32), // 'A'..'Z'
        0x0061..=0x007a => Some((sym - 0x20) as u32), // 'a'..'z' -> 'A'..'Z'
        0x0060 => Some(0xC0), // grave -> VK_OEM_3
        0xffbe..=0xffc9 => Some(0x70 + (sym - 0xffbe) as u32), // F1..F12
        0xffe1 | 0xffe2 => Some(0x10), // Shift
        0xffe3 | 0xffe4 => Some(0x11), // Control
        0xffe9 | 0xffea => Some(0x12), // Alt
        _ => None,
    }
}

/// Map a Windows Virtual-Key code back to an X11 KeySym for hotkey matching.
pub fn vk_to_keysym(vk: u32) -> Option<u64> {
    match vk {
        0x08 => Some(0xff08),
        0x09 => Some(0xff09),
        0x0D => Some(0xff0d),
        0x1B => Some(0xff1b),
        0x20 => Some(0x0020),
        0x21 => Some(0xff55),
        0x22 => Some(0xff56),
        0x23 => Some(0xff57),
        0x24 => Some(0xff50),
        0x25 => Some(0xff51),
        0x26 => Some(0xff52),
        0x27 => Some(0xff53),
        0x28 => Some(0xff54),
        0x2D => Some(0xff63),
        0x2E => Some(0xffff),
        0x30..=0x39 => Some(vk as u64),
        0x41..=0x5A => Some(vk as u64),
        0xC0 => Some(0x0060),
        0x70..=0x7B => Some(0xffbe + (vk - 0x70) as u64),
        _ => None,
    }
}

fn push_event(events: &mut Vec<Input>, e: Input) {
    if let (Input::Move { .. }, Some(Input::Move { .. })) = (&e, events.last()) {
        events.pop();
    }
    events.push(e);
}

fn scale_point(x: i32, y: i32, win_w: u32, win_h: u32) -> (f32, f32) {
    let (fb_w, fb_h) = *shared().size.lock().unwrap_or_else(|e| e.into_inner());
    if win_w > 0 && win_h > 0 && fb_w > 0 && fb_h > 0 {
        (x as f32 * (fb_w as f32 / win_w as f32), y as f32 * (fb_h as f32 / win_h as f32))
    } else {
        (x as f32, y as f32)
    }
}

fn combo_matches(keymap: &[u8; 32], req_mods: u8, kc_key: u8, mods_kc: (u8, u8, u8, u8, u8, u8)) -> bool {
    if kc_key == 0 || !key_down(keymap, kc_key) {
        return false;
    }
    let shift = key_down(keymap, mods_kc.0) || key_down(keymap, mods_kc.1);
    let ctrl = key_down(keymap, mods_kc.2) || key_down(keymap, mods_kc.3);
    let alt = key_down(keymap, mods_kc.4) || key_down(keymap, mods_kc.5);
    let held = (if shift { MOD_SHIFT } else { 0 })
        | (if ctrl { MOD_CTRL } else { 0 })
        | (if alt { MOD_ALT } else { 0 });
    held == (req_mods & 0x07)
}

fn is_ancestor_of(x11: &X11, dpy: *mut Display, maybe_anc: Window, mut child: Window) -> bool {
    let Some(query_tree) = x11.XQueryTree else { return false };
    for _ in 0..8 {
        let mut root: Window = 0;
        let mut parent: Window = 0;
        let mut children: *mut Window = std::ptr::null_mut();
        let mut nchildren: c_uint = 0;
        let ok = unsafe { query_tree(dpy, child, &mut root, &mut parent, &mut children, &mut nchildren) };
        if !children.is_null() {
            if let Some(free_fn) = x11.XFree {
                unsafe { free_fn(children.cast()) };
            }
        }
        if ok == 0 || parent == 0 || parent == root {
            return false;
        }
        if parent == maybe_anc {
            return true;
        }
        child = parent;
    }
    false
}

fn window_has_focus(x11: &X11, dpy: *mut Display, win: Window, shield: Window) -> bool {
    let Some(get_focus) = x11.XGetInputFocus else { return true };
    let mut focus: Window = 0;
    let mut revert: c_int = 0;
    unsafe { get_focus(dpy, &mut focus, &mut revert) };
    // 0 = None, 1 = PointerRoot
    if focus <= 1 {
        return false;
    }
    if focus == win || (shield != 0 && focus == shield) {
        return true;
    }
    is_ancestor_of(x11, dpy, focus, win) || is_ancestor_of(x11, dpy, win, focus)
}

fn input_loop() {
    let Some(x11) = (unsafe { X11::load() }) else {
        return;
    };
    let dpy = unsafe { (x11.XOpenDisplay)(std::ptr::null()) };
    if dpy.is_null() {
        return;
    }
    OUR_DISPLAY.store(dpy as usize, Ordering::Release);
    if let Some(set_err) = x11.XSetErrorHandler {
        let prev = unsafe { set_err(Some(ignore_x11_error)) } as usize;
        if prev != ignore_x11_error as *const () as usize {
            PREV_ERR_HANDLER.store(prev, Ordering::Release);
        }
    }

    let kc_shift_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe1) };
    let kc_shift_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe2) };
    let kc_ctrl_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe3) };
    let kc_ctrl_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe4) };
    let kc_alt_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe9) };
    let kc_alt_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffea) };
    let mods_kc = (kc_shift_l, kc_shift_r, kc_ctrl_l, kc_ctrl_r, kc_alt_l, kc_alt_r);
    let kc_esc = unsafe { (x11.XKeysymToKeycode)(dpy, 0xff1b) };

    let resolve_kc = |vk: u32| -> u8 {
        vk_to_keysym(vk).map(|s| unsafe { (x11.XKeysymToKeycode)(dpy, s) }).unwrap_or(0)
    };
    let mut cur_ov = (0u8, 0u32, 0u8);
    let mut cur_sh = (0u8, 0u32, 0u8);
    let refresh_keys = |ov: &mut (u8, u32, u8), sh: &mut (u8, u32, u8)| {
        let (om, ovk) = shared().overlay_hotkey();
        if (om, ovk) != (ov.0, ov.1) {
            *ov = (om, ovk, resolve_kc(ovk));
        }
        let (sm, svk) = shared().screenshot_hotkey();
        if (sm, svk) != (sh.0, sh.1) {
            *sh = (sm, svk, resolve_kc(svk));
        }
    };

    let mut was_ov = false;
    let mut was_shot = false;

    while !shared().is_shutdown() {
        // While closed, poll XQueryKeymap at ~60Hz for the overlay & screenshot hotkeys.
        while !shared().is_open() && !shared().is_shutdown() {
            refresh_keys(&mut cur_ov, &mut cur_sh);
            let mut keymap = [0u8; 32];
            unsafe { (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast()) };

            let shot = combo_matches(&keymap, cur_sh.0, cur_sh.2, mods_kc);
            if shot && !was_shot {
                was_shot = true;
                shared().request_screenshot();
            } else if !shot {
                was_shot = false;
            }

            let combo = combo_matches(&keymap, cur_ov.0, cur_ov.2, mods_kc);
            if combo && !was_ov {
                was_ov = true;
                shared().notify_opened(true);
                break;
            }
            if !combo {
                was_ov = false;
            }
            std::thread::sleep(Duration::from_millis(16));
        }

        if shared().is_shutdown() {
            break;
        }

        let is_wayland = SURFACE_IS_WAYLAND.load(Ordering::Relaxed);
        let mut win = ACTIVE_WINDOW.load(Ordering::Relaxed) as Window;
        if is_wayland && win == 0 {
            eprintln!("[aurora-vk-overlay] open: surface=wayland (X11 input capture unavailable)");
            // Native Wayland surfaces don't support XGrabPointer; wait cleanly until closed.
            while shared().is_open() && !shared().is_shutdown() && SURFACE_IS_WAYLAND.load(Ordering::Relaxed) {
                refresh_keys(&mut cur_ov, &mut cur_sh);
                let mut keymap = [0u8; 32];
                unsafe { (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast()) };
                let shot = combo_matches(&keymap, cur_sh.0, cur_sh.2, mods_kc);
                if shot && !was_shot {
                    was_shot = true;
                    shared().request_screenshot();
                } else if !shot {
                    was_shot = false;
                }
                let combo = combo_matches(&keymap, cur_ov.0, cur_ov.2, mods_kc) || key_down(&keymap, kc_esc);
                if combo && !was_ov {
                    was_ov = true;
                    shared().notify_opened(false);
                    break;
                } else if !combo {
                    was_ov = false;
                }
                std::thread::sleep(Duration::from_millis(24));
            }
            continue;
        }

        if win == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let query_geom = |w: Window| -> (u32, u32) {
            let (mut ww, mut wh) = (0u32, 0u32);
            let mut root = 0;
            let (mut gx, mut gy) = (0, 0);
            let (mut bw, mut depth) = (0, 0);
            if unsafe { (x11.XGetGeometry)(dpy, w, &mut root, &mut gx, &mut gy, &mut ww, &mut wh, &mut bw, &mut depth) } != 0 {
                (ww, wh)
            } else {
                (0, 0)
            }
        };

        let (mut win_w, mut win_h) = query_geom(win);
        let arrow_cursor = x11.XCreateFontCursor.map(|f| unsafe { f(dpy, XC_LEFT_PTR) }).unwrap_or(0);
        // Create a child InputOnly shield window inside `win` (NOT a top-level
        // override-redirect window on root) so we intercept pointer events and
        // show a visible cursor without stealing top-level WM focus or blocking
        // GNOME's `_NET_WM_PING` responsiveness check.
        let mut shield: Window = 0;
        if let (Some(create_fn), Some(map_fn)) = (x11.XCreateWindow, x11.XMapRaised) {
            let sw = win_w.max(128);
            let sh = win_h.max(128);
            let mut attrs = XSetWindowAttributes {
                event_mask: (POINTER_MOTION_MASK | BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | KEY_PRESS_MASK | KEY_RELEASE_MASK) as c_long,
                cursor: arrow_cursor,
                ..Default::default()
            };
            let mask = CW_EVENT_MASK | if arrow_cursor != 0 { CW_CURSOR } else { 0 };
            let w = unsafe { create_fn(dpy, win, 0, 0, sw, sh, 0, 0, INPUT_ONLY, std::ptr::null_mut(), mask, &mut attrs) };
            if w != 0 {
                unsafe { map_fn(dpy, w) };
                shield = w;
            }
        }
        let grab_target = if shield != 0 { shield } else { win };
        let ptr_mask = POINTER_MOTION_MASK | BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK;
        let try_ptr_grab = |w: Window| -> bool {
            unsafe { (x11.XGrabPointer)(dpy, w, 0, ptr_mask, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, 0, arrow_cursor, CURRENT_TIME) == GRAB_SUCCESS }
        };
        let try_kbd_grab = |w: Window| -> bool {
            unsafe { (x11.XGrabKeyboard)(dpy, w, 0, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, CURRENT_TIME) == GRAB_SUCCESS }
        };
        let mut ptr_grabbed = try_ptr_grab(grab_target);
        let mut kbd_grabbed = try_kbd_grab(grab_target);
        unsafe { (x11.XFlush)(dpy) };
        eprintln!(
            "[aurora-vk-overlay] open: surface=x11 win=0x{win:x} shield=0x{shield:x} geom={win_w}x{win_h} ptr_grab={ptr_grabbed} kbd_grab={kbd_grabbed}"
        );

        let mut last_poll_pos: Option<(i32, i32)> = None;
        let mut last_poll_mask: c_uint = 0;
        let mut ticks = 0u32;
        let mut unfocused_checks = 0u32;

        while shared().is_open() && !shared().is_shutdown() {
            refresh_keys(&mut cur_ov, &mut cur_sh);
            ticks = ticks.wrapping_add(1);

            let cur_win = ACTIVE_WINDOW.load(Ordering::Relaxed) as Window;
            if cur_win != 0 && cur_win != win {
                if ptr_grabbed {
                    unsafe { (x11.XUngrabPointer)(dpy, CURRENT_TIME) };
                    ptr_grabbed = false;
                }
                if kbd_grabbed {
                    unsafe { (x11.XUngrabKeyboard)(dpy, CURRENT_TIME) };
                    kbd_grabbed = false;
                }
                win = cur_win;
                (win_w, win_h) = query_geom(win);
            } else if ticks % 30 == 0 {
                let geom = query_geom(win);
                if geom.0 > 0 && geom.1 > 0 && geom != (win_w, win_h) {
                    (win_w, win_h) = geom;
                    if shield != 0 {
                        if let Some(resize_fn) = x11.XResizeWindow {
                            unsafe { resize_fn(dpy, shield, win_w.max(128), win_h.max(128)) };
                        }
                    }
                }
            }
            // If focus leaves the game (Alt+Tab, Super, compositor dialog),
            // release grabs and close the overlay so we never freeze or trigger
            // Mutter's "not responding" dialog.
            if ticks > 25 && ticks % 8 == 0 {
                if window_has_focus(&x11, dpy, win, shield) {
                    unfocused_checks = 0;
                } else {
                    unfocused_checks += 1;
                    if unfocused_checks >= 2 {
                        shared().notify_opened(false);
                        break;
                    }
                }
            }
            if ticks % 12 == 0 {
                if !ptr_grabbed {
                    ptr_grabbed = try_ptr_grab(grab_target);
                }
                if !kbd_grabbed {
                    kbd_grabbed = try_kbd_grab(grab_target);
                }
            }

            let mut events = Vec::new();
            let mut mods = 0u8;
            let mut close_requested = false;

            unsafe {
                while (x11.XPending)(dpy) > 0 {
                    let mut ev: XEvent = std::mem::zeroed();
                    (x11.XNextEvent)(dpy, &mut ev);
                    match ev.ty {
                        MOTION_NOTIFY => {
                            let m = ev.motion;
                            mods = x11_mods(m.state);
                            let (x, y) = scale_point(m.x, m.y, win_w, win_h);
                            push_event(&mut events, Input::Move { x, y });
                        }
                        BUTTON_PRESS | BUTTON_RELEASE => {
                            let b = ev.button;
                            let down = ev.ty == BUTTON_PRESS;
                            mods = x11_mods(b.state);
                            let (x, y) = scale_point(b.x, b.y, win_w, win_h);
                            match b.button {
                                1 => push_event(&mut events, Input::Button { button: 0, down, x, y }),
                                2 => push_event(&mut events, Input::Button { button: 2, down, x, y }),
                                3 => push_event(&mut events, Input::Button { button: 1, down, x, y }),
                                4 if down => push_event(&mut events, Input::Wheel { x: 0.0, y: 1.0 }),
                                5 if down => push_event(&mut events, Input::Wheel { x: 0.0, y: -1.0 }),
                                6 if down => push_event(&mut events, Input::Wheel { x: -1.0, y: 0.0 }),
                                7 if down => push_event(&mut events, Input::Wheel { x: 1.0, y: 0.0 }),
                                8 => push_event(&mut events, Input::Button { button: 3, down, x, y }),
                                9 => push_event(&mut events, Input::Button { button: 4, down, x, y }),
                                _ => {}
                            }
                        }
                        KEY_PRESS | KEY_RELEASE => {
                            let mut k = ev.key;
                            let down = ev.ty == KEY_PRESS;
                            mods = x11_mods(k.state);
                            let mut buf = [0u8; 32];
                            let mut sym: KeySym = 0;
                            let n = (x11.XLookupString)(
                                &mut k,
                                buf.as_mut_ptr().cast(),
                                buf.len() as c_int,
                                &mut sym,
                                std::ptr::null_mut(),
                            );
                            if sym == 0 {
                                sym = (x11.XLookupKeysym)(&mut k, 0);
                            }
                            let vk_opt = keysym_to_vk(sym as u64);
                            if down && vk_opt == Some(cur_sh.1) && (mods & 0x07) == cur_sh.0 {
                                shared().request_screenshot();
                                continue;
                            }
                            if down && (sym == 0xff1b || (vk_opt == Some(cur_ov.1) && (mods & 0x07) == cur_ov.0)) {
                                was_ov = true;
                                close_requested = true;
                                break;
                            }
                            if let Some(vk) = vk_opt {
                                push_event(&mut events, Input::Key { vk, down, repeat: false });
                            }
                            if down && n > 0 && (mods & (MOD_CTRL | MOD_ALT) == 0) {
                                if let Ok(s) = std::str::from_utf8(&buf[..n as usize]) {
                                    let printable: String = s.chars().filter(|c| !c.is_control()).collect();
                                    if !printable.is_empty() {
                                        push_event(&mut events, Input::Text(printable));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Always poll hotkeys + Esc via XQueryKeymap (even when kbd_grabbed)
                // so Shift+Tab / Esc can NEVER get stuck if a compositor dialog appears.
                if !close_requested {
                    let mut keymap = [0u8; 32];
                    (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast());
                    let shot = combo_matches(&keymap, cur_sh.0, cur_sh.2, mods_kc);
                    if shot && !was_shot {
                        was_shot = true;
                        shared().request_screenshot();
                    } else if !shot {
                        was_shot = false;
                    }
                    let combo = combo_matches(&keymap, cur_ov.0, cur_ov.2, mods_kc) || key_down(&keymap, kc_esc);
                    if combo && !was_ov {
                        was_ov = true;
                        close_requested = true;
                    } else if !combo {
                        was_ov = false;
                    }
                }

                // Fallback when the game holds an active XGrabPointer: poll XQueryPointer.
                if !ptr_grabbed && !close_requested {
                    let (mut root, mut child) = (0, 0);
                    let (mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0);
                    let mut mask = 0u32;
                    if (x11.XQueryPointer)(dpy, win, &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask) != 0 {
                        mods = x11_mods(mask);
                        let (x, y) = scale_point(wx, wy, win_w, win_h);
                        if last_poll_pos != Some((wx, wy)) {
                            last_poll_pos = Some((wx, wy));
                            push_event(&mut events, Input::Move { x, y });
                        }
                        for (bit, btn) in [(BUTTON1_MASK, 0u8), (BUTTON3_MASK, 1), (BUTTON2_MASK, 2)] {
                            let was = last_poll_mask & bit != 0;
                            let now = mask & bit != 0;
                            if was != now {
                                push_event(&mut events, Input::Button { button: btn, down: now, x, y });
                            }
                        }
                        last_poll_mask = mask;
                    }
                }
            }

            if close_requested {
                shared().notify_opened(false);
                break;
            }
            if !events.is_empty() {
                shared().send_input(mods, events);
            }
            std::thread::sleep(Duration::from_millis(8));
        }

        unsafe {
            if ptr_grabbed {
                (x11.XUngrabPointer)(dpy, CURRENT_TIME);
            }
            if kbd_grabbed {
                (x11.XUngrabKeyboard)(dpy, CURRENT_TIME);
            }
            if shield != 0 {
                if let Some(destroy_fn) = x11.XDestroyWindow {
                    destroy_fn(dpy, shield);
                }
                if let Some(focus_fn) = x11.XSetInputFocus {
                    focus_fn(dpy, win, REVERT_TO_PARENT, CURRENT_TIME);
                }
            }
            if arrow_cursor != 0 {
                if let Some(free_fn) = x11.XFreeCursor {
                    free_fn(dpy, arrow_cursor);
                }
            }
            (x11.XFlush)(dpy);
        }
    }

    unsafe { (x11.XCloseDisplay)(dpy) };
}
