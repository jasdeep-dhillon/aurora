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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static ACTIVE_WINDOW: AtomicU64 = AtomicU64::new(0);

/// Record the game's active X11 window ID (from `vkCreateXlibSurfaceKHR` or
/// `vkCreateXcbSurfaceKHR`).
pub fn set_active_window(win: u64) {
    if win != 0 {
        ACTIVE_WINDOW.store(win, Ordering::Relaxed);
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
        0xffbe..=0xffc9 => Some(0x70 + (sym - 0xffbe) as u32), // F1..F12
        0xffe1 | 0xffe2 => Some(0x10), // Shift
        0xffe3 | 0xffe4 => Some(0x11), // Control
        0xffe9 | 0xffea => Some(0x12), // Alt
        _ => None,
    }
}

fn scale_point(x: i32, y: i32, win_w: u32, win_h: u32) -> (f32, f32) {
    let (fb_w, fb_h) = *shared().size.lock().unwrap_or_else(|e| e.into_inner());
    if win_w > 0 && win_h > 0 && fb_w > 0 && fb_h > 0 {
        (x as f32 * (fb_w as f32 / win_w as f32), y as f32 * (fb_h as f32 / win_h as f32))
    } else {
        (x as f32, y as f32)
    }
}

fn input_loop() {
    let Some(x11) = (unsafe { X11::load() }) else {
        return;
    };

    let dpy = unsafe { (x11.XOpenDisplay)(std::ptr::null()) };
    if dpy.is_null() {
        return;
    }
    let kc_shift_l = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe1) };
    let kc_shift_r = unsafe { (x11.XKeysymToKeycode)(dpy, 0xffe2) };
    let kc_tab = unsafe { (x11.XKeysymToKeycode)(dpy, 0xff09) };
    let kc_esc = unsafe { (x11.XKeysymToKeycode)(dpy, 0xff1b) };
    let mut was_combo = false;

    loop {
        // While closed, poll XQueryKeymap at ~60Hz for Shift+Tab (never conflicts
        // with game keyboard grabs and works even after Aurora releases its global hotkey).
        while !shared().is_open() {
            let mut keymap = [0u8; 32];
            unsafe { (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast()) };
            let shift = key_down(&keymap, kc_shift_l) || key_down(&keymap, kc_shift_r);
            let tab = key_down(&keymap, kc_tab);
            let combo = shift && tab;
            if combo && !was_combo {
                was_combo = true;
                shared().notify_opened(true);
                break;
            }
            if !combo {
                was_combo = false;
            }
            std::thread::sleep(Duration::from_millis(16));
        }

        let win = ACTIVE_WINDOW.load(Ordering::Relaxed) as Window;
        if win == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let mut win_w = 0u32;
        let mut win_h = 0u32;
        unsafe {
            let mut root = 0;
            let (mut gx, mut gy) = (0, 0);
            let (mut bw, mut depth) = (0, 0);
            if (x11.XGetGeometry)(dpy, win, &mut root, &mut gx, &mut gy, &mut win_w, &mut win_h, &mut bw, &mut depth) == 0 {
                win_w = 0;
                win_h = 0;
            }
        }

        let ptr_mask = POINTER_MOTION_MASK | BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK;
        let ptr_grabbed = unsafe {
            (x11.XGrabPointer)(dpy, win, 0, ptr_mask, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, 0, 0, CURRENT_TIME) == GRAB_SUCCESS
        };
        let kbd_grabbed = unsafe {
            (x11.XGrabKeyboard)(dpy, win, 0, GRAB_MODE_ASYNC, GRAB_MODE_ASYNC, CURRENT_TIME) == GRAB_SUCCESS
        };
        unsafe { (x11.XFlush)(dpy) };

        let mut last_poll_pos: Option<(i32, i32)> = None;
        let mut last_poll_mask: c_uint = 0;

        while shared().is_open() {
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
                            events.push(Input::Move { x, y });
                        }
                        BUTTON_PRESS | BUTTON_RELEASE => {
                            let b = ev.button;
                            let down = ev.ty == BUTTON_PRESS;
                            mods = x11_mods(b.state);
                            let (x, y) = scale_point(b.x, b.y, win_w, win_h);
                            match b.button {
                                1 => events.push(Input::Button { button: 0, down, x, y }),
                                2 => events.push(Input::Button { button: 2, down, x, y }),
                                3 => events.push(Input::Button { button: 1, down, x, y }),
                                4 if down => events.push(Input::Wheel { x: 0.0, y: 1.0 }),
                                5 if down => events.push(Input::Wheel { x: 0.0, y: -1.0 }),
                                6 if down => events.push(Input::Wheel { x: -1.0, y: 0.0 }),
                                7 if down => events.push(Input::Wheel { x: 1.0, y: 0.0 }),
                                8 => events.push(Input::Button { button: 3, down, x, y }),
                                9 => events.push(Input::Button { button: 4, down, x, y }),
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
                            if down && (sym == 0xff1b || ((sym == 0xff09 || sym == 0xfe20) && (mods & MOD_SHIFT != 0))) {
                                was_combo = true;
                                close_requested = true;
                                break;
                            }
                            if let Some(vk) = keysym_to_vk(sym as u64) {
                                events.push(Input::Key { vk, down, repeat: false });
                            }
                            if down && n > 0 && (mods & (MOD_CTRL | MOD_ALT) == 0) {
                                if let Ok(s) = std::str::from_utf8(&buf[..n as usize]) {
                                    let printable: String = s.chars().filter(|c| !c.is_control()).collect();
                                    if !printable.is_empty() {
                                        events.push(Input::Text(printable));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Fallback when the game holds an active XGrabKeyboard: poll Shift+Tab / Esc.
                if !kbd_grabbed && !close_requested {
                    let mut keymap = [0u8; 32];
                    (x11.XQueryKeymap)(dpy, keymap.as_mut_ptr().cast());
                    let shift = key_down(&keymap, kc_shift_l) || key_down(&keymap, kc_shift_r);
                    let combo = (shift && key_down(&keymap, kc_tab)) || key_down(&keymap, kc_esc);
                    if combo && !was_combo {
                        was_combo = true;
                        close_requested = true;
                    } else if !combo {
                        was_combo = false;
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
                            events.push(Input::Move { x, y });
                        }
                        for (bit, btn) in [(BUTTON1_MASK, 0u8), (BUTTON3_MASK, 1), (BUTTON2_MASK, 2)] {
                            let was = last_poll_mask & bit != 0;
                            let now = mask & bit != 0;
                            if was != now {
                                events.push(Input::Button { button: btn, down: now, x, y });
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
            (x11.XFlush)(dpy);
        }
    }
}
