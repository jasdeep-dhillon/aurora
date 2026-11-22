//! Controllers inside the game: the real XInput pads, plus the pads XInput
//! can't see (PlayStation, Switch, …) that the app reads with SDL3 and shares
//! (pads_shm.rs). Those fill the XInput slots real pads leave free. Steam
//! Input (input.rs) reads pads through here, and [`install`] points the
//! game's XInput imports here too, so XInput-only games see them as Xbox
//! pads, like Steam Input does.
//!
//! SDL's own modules are left alone: SDL reads PlayStation and Switch pads
//! itself and would see them twice.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::legacy::{self, Sample};
use vdf::controller::LegacyMap;
use crate::pads_shm::{self, GUIDE, Shared, State};
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// XINPUT_STATE.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pad {
    pub packet: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl From<State> for Pad {
    fn from(s: State) -> Pad {
        Pad { packet: s.packet, buttons: s.buttons, lt: s.lt, rt: s.rt, lx: s.lx, ly: s.ly, rx: s.rx, ry: s.ry }
    }
}

type GetState = unsafe extern "system" fn(u32, *mut Pad) -> u32;
type SetState = unsafe extern "system" fn(u32, *mut [u16; 2]) -> u32;
type GetCaps = unsafe extern "system" fn(u32, u32, *mut [u8; 20]) -> u32;
type GetBattery = unsafe extern "system" fn(u32, u8, *mut [u8; 2]) -> u32;
type GetProc = unsafe extern "system" fn(*mut c_void, *const c_char) -> *mut c_void;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, len: u32) -> u32;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}
#[cfg(not(windows))]
mod non_windows {
    #![allow(non_snake_case)]
    use std::ffi::c_void;

    pub unsafe fn LoadLibraryW(_: *const u16) -> *mut c_void { std::ptr::null_mut() }
    pub unsafe fn GetModuleHandleW(_: *const u16) -> *mut c_void { std::ptr::null_mut() }
    pub unsafe fn GetModuleFileNameW(_: *mut c_void, _: *mut u16, _: u32) -> u32 { 0 }
    pub unsafe fn GetProcAddress(_: *mut c_void, _: *const u8) -> *mut c_void { std::ptr::null_mut() }
}
#[cfg(not(windows))]
use non_windows::*;

const NOT_CONNECTED: u32 = 1167;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// XInput's DLLs, newest first.
const XINPUT_DLLS: [&str; 5] = ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll", "xinput1_2.dll", "xinput1_1.dll"];

pub struct XInput {
    pub get: GetState,
    pub set: SetState,
    /// XInputGetStateEx (ordinal 100): also the guide button.
    get_ex: Option<GetState>,
    caps: Option<GetCaps>,
    battery: Option<GetBattery>,
}

/// The real XInput (never our hooks: our own imports aren't patched).
pub static XINPUT: LazyLock<Option<XInput>> = LazyLock::new(|| {
    for dll in XINPUT_DLLS {
        let m = unsafe { LoadLibraryW(wide(dll).as_ptr()) };
        if m.is_null() {
            continue;
        }
        let f = |name: *const c_char| unsafe { GetProcAddress(m, name.cast()) };
        let (g, s) = (f(c"XInputGetState".as_ptr()), f(c"XInputSetState".as_ptr()));
        if g.is_null() || s.is_null() {
            continue;
        }
        let opt = |p: *mut c_void| (!p.is_null()).then_some(p);
        unsafe {
            return Some(XInput {
                get: std::mem::transmute::<*mut c_void, GetState>(g),
                set: std::mem::transmute::<*mut c_void, SetState>(s),
                get_ex: opt(f(100 as *const c_char)).map(|p| std::mem::transmute::<*mut c_void, GetState>(p)),
                caps: opt(f(c"XInputGetCapabilities".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, GetCaps>(p)),
                battery: opt(f(c"XInputGetBatteryInformation".as_ptr())).map(|p| std::mem::transmute::<*mut c_void, GetBattery>(p)),
            });
        }
    }
    None
});

// ------------------------------------------------------------ shared pads

fn open_shm(name: &str) -> Option<&'static Shared> {
    let out = pads_shm::open_shm(name);
    if out.is_none() {
        crate::log!("pads: no shared pads at {name}");
    }
    out
}

/// The app's shared memory, when it gave us one (Windows only).
fn mapped() -> Option<&'static Shared> {
    static MAP: LazyLock<usize> = LazyLock::new(|| {
        let Ok(name) = std::env::var(pads_shm::ENV_PADS) else { return 0 };
        open_shm(&name).map(|s| s as *const Shared as usize).unwrap_or(0)
    });
    unsafe { (*MAP as *const Shared).as_ref() }
}

/// The shared pads, while the app keeps them up to date and Steam Input is enabled.
fn shared() -> Option<&'static Shared> {
    if !steam_input_enabled() {
        return None;
    }
    mapped().filter(|s| s.live())
}

pub fn steam_input_enabled() -> bool {
    pads_shm::steam_input_env_enabled(std::env::var(pads_shm::ENV_STEAM_INPUT).ok().as_deref())
}

pub fn family_mask() -> u32 {
    pads_shm::parse_family_mask(std::env::var(pads_shm::ENV_INPUT_FAMILIES).ok().as_deref())
}

pub fn calibration() -> pads_shm::PadCalibration {
    std::env::var(pads_shm::ENV_INPUT_CALIBRATION).ok().map(|s| pads_shm::PadCalibration::decode(&s)).unwrap_or_default()
}

/// Shared slots with a pad in them whose controller family is enabled.
fn shared_pads(s: &Shared) -> Vec<usize> {
    let mask = family_mask();
    (0..pads_shm::SLOTS)
        .filter(|k| s.slots[*k].read().is_some_and(|st| pads_shm::family_allowed(mask, st.family)))
        .collect()
}

// ------------------------------------------------------------- real pads

/// Which slots hold a real pad, as last asked; empty slots are asked again
/// at most once a second (asking an empty slot is slow).
struct Real {
    connected: [bool; 4],
    asked: [Option<Instant>; 4],
}

static REAL: Mutex<Real> = Mutex::new(Real { connected: [false; 4], asked: [None; 4] });

fn note(i: usize, connected: bool) {
    if i < 4 {
        let mut r = REAL.lock().unwrap_or_else(|e| e.into_inner());
        r.connected[i] = connected;
        r.asked[i] = Some(Instant::now());
    }
}

fn real_state(x: &XInput, i: usize) -> Option<Pad> {
    {
        let r = REAL.lock().unwrap_or_else(|e| e.into_inner());
        if !r.connected[i] && r.asked[i].is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return None;
        }
    }
    let mut p = Pad::default();
    let ok = unsafe { (x.get)(i as u32, &mut p) } == 0;
    note(i, ok);
    ok.then_some(p)
}

fn real_connected() -> [bool; 4] {
    REAL.lock().unwrap_or_else(|e| e.into_inner()).connected
}

/// The shared pad in XInput slot `i` (real pads as last seen).
fn shared_at(i: usize) -> Option<(&'static Shared, usize)> {
    let s = shared()?;
    let k = (*pads_shm::seats(real_connected(), &shared_pads(s)).get(i)?)?;
    Some((s, k))
}

// ---------------------------------------------------- what Steam Input sees

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SteamInputPad {
    pub pad: Pad,
    pub calibrated: bool,
    pub family: u8,
    pub paddles: u8,
}

/// The four pads as XInput / games see them.
pub fn all() -> [Option<Pad>; 4] {
    let mut pads: [Option<Pad>; 4] = std::array::from_fn(|i| XINPUT.as_ref().and_then(|x| real_state(x, i)));
    if let Some(s) = shared() {
        let seats = pads_shm::seats(pads.map(|p| p.is_some()), &shared_pads(s));
        for (i, k) in seats.iter().enumerate() {
            if let Some(k) = k {
                pads[i] = s.slots[*k].read().map(Pad::from);
            }
        }
    }
    pads
}

/// The four pads as Steam Input sees them, respecting per-game disable and
/// global controller family toggles.
pub fn steam_input_pads() -> [Option<SteamInputPad>; 4] {
    steam_input_pads_with(
        steam_input_enabled(),
        family_mask(),
        std::array::from_fn(|i| XINPUT.as_ref().and_then(|x| real_state(x, i))),
        shared(),
    )
}

fn steam_input_pads_with(
    enabled: bool,
    mask: u32,
    real: [Option<Pad>; 4],
    shm: Option<&Shared>,
) -> [Option<SteamInputPad>; 4] {
    if !enabled {
        return [None; 4];
    }
    let xbox_on = mask & pads_shm::FAMILY_XBOX != 0;
    let mut out: [Option<SteamInputPad>; 4] = real.map(|p| {
        p.filter(|_| xbox_on)
            .map(|pad| SteamInputPad { pad, calibrated: false, family: pads_shm::SLOT_FAMILY_XBOX, paddles: 0 })
    });
    if let Some(s) = shm {
        let active_shared: Vec<usize> = (0..pads_shm::SLOTS)
            .filter(|k| s.slots[*k].read().is_some_and(|st| pads_shm::family_allowed(mask, st.family)))
            .collect();
        let seats = pads_shm::seats(real.map(|p| p.is_some()), &active_shared);
        for (i, k) in seats.iter().enumerate() {
            if let Some(k) = k
                && let Some(st) = s.slots[*k].read()
            {
                out[i] = Some(SteamInputPad { pad: Pad::from(st), calibrated: st.calibrated, family: st.family, paddles: st.paddles });
            }
        }
    }
    out
}

/// Rumble on XInput slot `i`.
pub fn rumble(i: usize, low: u16, high: u16) {
    if !steam_input_enabled() || !calibration().rumble_enabled {
        return;
    }
    if let Some((s, k)) = shared_at(i) {
        s.slots[k].set_rumble(low, high);
    } else if let Some(x) = XINPUT.as_ref() {
        let mut v = [low, high];
        unsafe { (x.set)(i as u32, &mut v) };
    }
}

/// Motion sensors (accel in m/s^2, gyro in rad/s) for XInput slot `i`, when
/// backed by an SDL3 pad that exposes IMU sensors.
pub fn motion(i: usize) -> Option<([f32; 3], [f32; 3])> {
    let cal = calibration();
    if !steam_input_enabled() || !cal.gyro_enabled {
        return None;
    }
    let (s, k) = shared_at(i)?;
    let st = s.slots[k].read()?;
    if !st.has_motion {
        return None;
    }
    let mut gyro = st.gyro();
    if !st.calibrated && (cal.gyro_sensitivity - 1.0).abs() > f32::EPSILON {
        gyro = [gyro[0] * cal.gyro_sensitivity, gyro[1] * cal.gyro_sensitivity, gyro[2] * cal.gyro_sensitivity];
    }
    Some((st.accel(), gyro))
}

// ------------------------------------------------------- the game's XInput

#[derive(Default)]
struct LegacyState {
    map: LegacyMap,
    packets: [Option<Pad>; 4],
    revision: u64,
}

static LEGACY: LazyLock<Mutex<LegacyState>> = LazyLock::new(|| Mutex::new(LegacyState::default()));

pub(crate) fn set_legacy_map(map: LegacyMap) {
    let mut legacy = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    legacy.map = map;
    legacy.revision = legacy.revision.wrapping_add(1);
}

#[cfg(windows)]
pub(crate) fn synthetic_layout() -> (u64, LegacyMap) {
    let state = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    (state.revision, state.map.clone())
}

#[cfg(windows)]
pub(crate) fn synthetic_revision() -> u64 {
    LEGACY.lock().unwrap_or_else(|e| e.into_inner()).revision
}

#[cfg(windows)]
pub(crate) fn synthetic_pad() -> Option<Sample> {
    steam_input_pads().iter().enumerate().find_map(|(index, entry)| {
        let entry = (*entry)?;
        Some(Sample {
            pad: entry.pad, paddles: entry.paddles,
            gyro: motion(index).map(|(_, gyro)| gyro).unwrap_or_default(),
        })
    })
}

fn translated(i: usize, sample: Sample, family: u8) -> Pad {
    if !steam_input_enabled() || !pads_shm::family_allowed(family_mask(), family) {
        return sample.pad;
    }
    let mut state = LEGACY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(previous) = state.packets.get(i).copied() else { return sample.pad };
    if state.map.0.is_empty() && previous.is_none() {
        return sample.pad;
    }
    let mut mapped = legacy::remap(sample, &state.map);
    if let Some(previous) = previous {
        mapped.packet = previous.packet;
        if mapped != previous {
            mapped.packet = mapped.packet.wrapping_add(1);
        }
    }
    state.packets[i] = Some(mapped);
    mapped
}

fn get_state(i: u32, out: *mut Pad, ex: bool) -> u32 {
    let result = XINPUT.as_ref().map_or(NOT_CONNECTED, |x| {
        unsafe { (if ex { x.get_ex.unwrap_or(x.get) } else { x.get })(i, out) }
    });
    note(i as usize, result == 0);
    if out.is_null() {
        return result;
    }
    let (sample, family) = if result == 0 {
        (Sample { pad: unsafe { out.read_unaligned() }, ..Default::default() }, pads_shm::SLOT_FAMILY_XBOX)
    } else if let Some(state) = shared_at(i as usize).and_then(|(s, k)| s.slots[k].read()) {
        (Sample {
            pad: Pad::from(state), paddles: state.paddles,
            gyro: motion(i as usize).map(|(_, gyro)| gyro).unwrap_or_default(),
        }, state.family)
    } else {
        return result;
    };
    let mut pad = translated(i as usize, sample, family);
    if !ex {
        pad.buttons &= !GUIDE;
    }
    unsafe { out.write_unaligned(pad) };
    0
}

unsafe extern "system" fn hook_get_state(i: u32, out: *mut Pad) -> u32 {
    get_state(i, out, false)
}

unsafe extern "system" fn hook_get_state_ex(i: u32, out: *mut Pad) -> u32 {
    get_state(i, out, true)
}

unsafe extern "system" fn hook_set_state(i: u32, v: *mut [u16; 2]) -> u32 {
    if let (Some((s, k)), false) = (shared_at(i as usize), v.is_null()) {
        let [low, high] = if calibration().rumble_enabled { unsafe { v.read_unaligned() } } else { [0, 0] };
        s.slots[k].set_rumble(low, high);
        return 0;
    }
    match XINPUT.as_ref() {
        Some(x) => unsafe { (x.set)(i, v) },
        None => NOT_CONNECTED,
    }
}

unsafe extern "system" fn hook_get_caps(i: u32, flags: u32, out: *mut [u8; 20]) -> u32 {
    let r = XINPUT.as_ref().and_then(|x| x.caps).map_or(NOT_CONNECTED, |f| unsafe { f(i, flags, out) });
    if r == 0 || out.is_null() || shared_at(i as usize).is_none() {
        return r;
    }
    // XINPUT_CAPABILITIES: a wired gamepad with every button, full-range
    // axes and both motors.
    let mut c = [0u8; 20];
    c[0] = 1; // XINPUT_DEVTYPE_GAMEPAD
    c[1] = 1; // XINPUT_DEVSUBTYPE_GAMEPAD
    c[4..6].copy_from_slice(&0xF3FFu16.to_le_bytes());
    c[6] = 0xFF;
    c[7] = 0xFF;
    for a in 0..4 {
        c[8 + a * 2..10 + a * 2].copy_from_slice(&0xFFC0u16.to_le_bytes());
    }
    c[16..20].copy_from_slice(&[0xFF; 4]);
    unsafe { out.write_unaligned(c) };
    0
}

unsafe extern "system" fn hook_get_battery(i: u32, kind: u8, out: *mut [u8; 2]) -> u32 {
    let r = XINPUT.as_ref().and_then(|x| x.battery).map_or(NOT_CONNECTED, |f| unsafe { f(i, kind, out) });
    if r == 0 || out.is_null() || shared_at(i as usize).is_none() {
        return r;
    }
    unsafe { out.write_unaligned([1, 3]) }; // wired, full
    0
}

/// Our function for an XInput export (by name, or ordinal 100).
fn ours_for(name: *const c_char) -> Option<usize> {
    if (name as usize) < 0x10000 {
        return (name as usize == 100).then_some(hook_get_state_ex as *const () as usize);
    }
    Some(match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"XInputGetState" => hook_get_state as *const () as usize,
        b"XInputSetState" => hook_set_state as *const () as usize,
        b"XInputGetCapabilities" => hook_get_caps as *const () as usize,
        b"XInputGetBatteryInformation" => hook_get_battery as *const () as usize,
        _ => return None,
    })
}

/// A module's full path, lowercase.
fn module_path(m: *mut c_void) -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(m, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())]).to_ascii_lowercase()
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or_default()
}

static REAL_GET_PROC: AtomicUsize = AtomicUsize::new(0);

/// GetProcAddress for the game's modules: XInput's functions come back as
/// ours (games and engines often load XInput at run time).
unsafe extern "system" fn hook_get_proc(m: *mut c_void, name: *const c_char) -> *mut c_void {
    let real: GetProc = unsafe { std::mem::transmute(REAL_GET_PROC.load(Ordering::Acquire)) };
    let p = unsafe { real(m, name) };
    if p.is_null() || name.is_null() {
        return p;
    }
    let maybe = (name as usize) < 0x10000 || unsafe { CStr::from_ptr(name) }.to_bytes().starts_with(b"XInput");
    if maybe && file_name(&module_path(m)).starts_with("xinput") {
        if let Some(ours) = ours_for(name) {
            return ours as *mut c_void;
        }
    }
    p
}

/// (real address, ours) for every loaded XInput DLL and GetProcAddress.
fn import_table() -> Vec<(usize, usize)> {
    let mut table = Vec::new();
    for dll in XINPUT_DLLS {
        let m = unsafe { GetModuleHandleW(wide(dll).as_ptr()) };
        if m.is_null() {
            continue;
        }
        for name in [c"XInputGetState".as_ptr(), c"XInputSetState".as_ptr(), c"XInputGetCapabilities".as_ptr(), c"XInputGetBatteryInformation".as_ptr(), 100 as *const c_char] {
            let p = unsafe { GetProcAddress(m, name.cast()) };
            if let (false, Some(ours)) = (p.is_null(), ours_for(name)) {
                table.push((p as usize, ours));
            }
        }
    }
    for dll in ["kernel32.dll", "kernelbase.dll"] {
        let m = unsafe { GetModuleHandleW(wide(dll).as_ptr()) };
        let p = if m.is_null() { std::ptr::null_mut() } else { unsafe { GetProcAddress(m, c"GetProcAddress".as_ptr().cast()) } };
        if !p.is_null() {
            let _ = REAL_GET_PROC.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire);
            table.push((p as usize, hook_get_proc as *const () as usize));
        }
    }
    table
}

/// Modules whose XInput stays real: Windows' own, any XInput DLL, and SDL
/// (which reads PlayStation/Switch pads itself).
///
/// .NET is the exception among Windows' modules: XNA/FNA games (Celeste)
/// read XInput through `[DllImport("xinput1_3.dll")]`, which the runtime
/// (`clr.dll`, `coreclr.dll`) resolves, and XNA itself lives in the GAC under
/// `%SystemRoot%\Microsoft.NET\assembly\`. Skipping them left those games
/// on the real XInput, which doesn't see PlayStation pads. `path` is
/// lowercase (see `module_path`).
fn leave_alone(path: &str, windir: &str) -> bool {
    let name = file_name(path);
    let managed = path.contains("\\microsoft.net\\assembly\\") || name == "clr.dll" || name == "coreclr.dll";
    let system = !windir.is_empty() && path.starts_with(windir) && !managed;
    system || name.starts_with("xinput") || name.starts_with("sdl")
}

#[cfg(not(windows))]
fn patch_all() -> usize {
    0
}

#[cfg(windows)]
fn patch_all() -> usize {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW};
    use windows::Win32::System::ProcessStatus::EnumProcessModules;
    use windows::Win32::System::Threading::GetCurrentProcess;
    let table = import_table();
    if REAL_GET_PROC.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let mut me = HMODULE::default();
    let mut modules = vec![HMODULE::default(); 1024];
    let mut needed = 0u32;
    unsafe {
        let _ = GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, windows::core::PCWSTR(patch_all as *const u16), &mut me);
        if EnumProcessModules(GetCurrentProcess(), modules.as_mut_ptr(), (modules.len() * size_of::<HMODULE>()) as u32, &mut needed).is_err() {
            return 0;
        }
    }
    modules.truncate((needed as usize / size_of::<HMODULE>()).min(modules.len()));
    let windir = std::env::var("SystemRoot").map(|w| format!("{}\\", w.to_ascii_lowercase())).unwrap_or_default();
    let mut patched = 0;
    for m in modules {
        if m == me || leave_alone(&module_path(m.0), &windir) {
            continue;
        }
        patched += unsafe { crate::overlay::patch_module(m.0 as *mut u8, &table) };
    }
    patched
}

/// Enable the launch layout worker and the game's XInput bridge (including under Proton).
#[cfg(windows)]
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if !steam_input_enabled() || crate::ipc::target().is_none() {
            return;
        }
        crate::input::load();
        crate::synthetic_input::start();
        let _ = std::thread::Builder::new().name("pads-imports".into()).spawn(|| {
            // Again now and then: games load engine plugins and XInput late.
            loop {
                let n = std::panic::catch_unwind(patch_all).unwrap_or(0);
                if n > 0 {
                    crate::log!("pads: {n} XInput imports now see shared controllers");
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    });
}

#[cfg(not(windows))]
pub fn install() {
    crate::input::load();
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn hooks_dotnet_but_not_windows() {
        let win = "c:\\windows\\";
        // Windows' own modules and XInput itself stay real.
        assert!(leave_alone("c:\\windows\\system32\\user32.dll", win));
        assert!(leave_alone("c:\\windows\\system32\\xinput1_4.dll", win));
        assert!(leave_alone("c:\\games\\foo\\sdl3.dll", win));
        // The .NET runtime and the GAC's XNA are hooked (Celeste).
        assert!(!leave_alone("c:\\windows\\microsoft.net\\framework\\v4.0.30319\\clr.dll", win));
        assert!(!leave_alone("c:\\windows\\microsoft.net\\assembly\\gac_32\\microsoft.xna.framework\\v4.0_4.0.0.0__842cf8be1de50553\\microsoft.xna.framework.dll", win));
        assert!(!leave_alone("c:\\program files\\dotnet\\shared\\microsoft.netcore.app\\8.0.0\\coreclr.dll", win));
        // The game's own modules are hooked.
        assert!(!leave_alone("c:\\games\\celeste\\celeste.exe", win));
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
    }

    /// A pad the app shares reaches Steam Input and the game's XInput, and
    /// rumble goes back.
    #[test]
    fn shared_pad_reaches_the_game() {
        if XINPUT.is_none() {
            eprintln!("no XInput DLL on this system: skipped");
            return;
        }
        let name = format!("Local\\sir-pads-test-{}", std::process::id());
        let h = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x4, 0, std::mem::size_of::<Shared>() as u32, wide(&name).as_ptr()) };
        assert!(!h.is_null());
        unsafe { std::env::set_var(pads_shm::ENV_PADS, &name) };
        let s = mapped().expect("the shared memory opens");
        assert!(shared().is_none(), "not live before the app writes");
        s.magic.store(pads_shm::MAGIC, Ordering::Release);
        s.heartbeat.store(pads_shm::now_ms(), Ordering::Relaxed);
        s.slots[0].write(Some(State { packet: 3, buttons: 0x1000 | GUIDE, lt: 200, ly: 32767, ..Default::default() }));

        let pads = all();
        let i = pads_shm::seats(real_connected(), &[0]).iter().position(Option::is_some).expect("a free slot");
        assert_eq!(pads[i].map(|p| (p.buttons, p.lt, p.ly)), Some((0x1000 | GUIDE, 200, 32767)));

        let mut p = Pad::default();
        assert_eq!(unsafe { hook_get_state(i as u32, &mut p) }, 0);
        assert_eq!((p.packet, p.buttons, p.lt), (3, 0x1000, 200), "no guide button without the Ex call");
        assert_eq!(unsafe { hook_get_state_ex(i as u32, &mut p) }, 0);
        assert_eq!(p.buttons, 0x1000 | GUIDE);

        assert_eq!(unsafe { hook_set_state(i as u32, &mut [100, 200]) }, 0);
        assert_eq!(s.slots[0].rumble(), (100, 200));
        rumble(i, 7, 8);
        assert_eq!(s.slots[0].rumble(), (7, 8));

        let mut caps = [0u8; 20];
        assert_eq!(unsafe { hook_get_caps(i as u32, 0, &mut caps) }, 0);
        assert_eq!(caps[0], 1);
        let mut battery = [0u8; 2];
        assert_eq!(unsafe { hook_get_battery(i as u32, 0, &mut battery) }, 0);

        // GetProcAddress on XInput hands out ours; other lookups are untouched.
        assert!(XINPUT.is_some());
        assert!(!import_table().is_empty());
        let x = unsafe { GetModuleHandleW(wide("xinput1_4.dll").as_ptr()) };
        assert!(!x.is_null());
        assert_eq!(unsafe { hook_get_proc(x, c"XInputGetState".as_ptr()) } as usize, hook_get_state as *const () as usize);
        assert_eq!(unsafe { hook_get_proc(x, 100 as *const c_char) } as usize, hook_get_state_ex as *const () as usize);
        let k = unsafe { GetModuleHandleW(wide("kernel32.dll").as_ptr()) };
        assert_eq!(unsafe { hook_get_proc(k, c"GetTickCount".as_ptr()) }, unsafe { GetProcAddress(k, c"GetTickCount".as_ptr().cast()) });
        let _ = patch_all();

        // The app gone: the pad goes with it.
        s.heartbeat.store(0, Ordering::Relaxed);
        let mut p = Pad::default();
        assert_ne!(unsafe { hook_get_state(i as u32, &mut p) }, 0);
    }

    #[test]
    fn open_shm_rejects_non_sir_pads_names() {
        let rogue = format!("Local\\evil-pads-{}", std::process::id());
        let h = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x4, 0, std::mem::size_of::<Shared>() as u32, wide(&rogue).as_ptr()) };
        assert!(!h.is_null());
        assert!(open_shm(&rogue).is_none(), "must reject mapping names outside Local\\sir-pads-");
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn steam_input_pads_respects_enabled_and_family_mask() {
        let shm: Shared = unsafe { std::mem::zeroed() };
        shm.magic.store(pads_shm::MAGIC, Ordering::Release);
        shm.heartbeat.store(pads_shm::now_ms(), Ordering::Relaxed);
        shm.slots[0].write(Some(State {
            buttons: 0x1000,
            calibrated: true,
            family: pads_shm::SLOT_FAMILY_PLAYSTATION,
            ..Default::default()
        }));
        shm.slots[1].write(Some(State {
            buttons: 0x2000,
            calibrated: true,
            family: pads_shm::SLOT_FAMILY_SWITCH_PRO,
            ..Default::default()
        }));
        let real = [Some(Pad { buttons: 0x4000, ..Default::default() }), None, None, None];

        assert_eq!(steam_input_pads_with(false, pads_shm::FAMILY_ALL, real, Some(&shm)), [None; 4]);

        let all = steam_input_pads_with(true, pads_shm::FAMILY_ALL, real, Some(&shm));
        assert_eq!(all[0].map(|p| p.pad.buttons), Some(0x4000));
        assert_eq!(all[1].map(|p| (p.pad.buttons, p.family)), Some((0x1000, pads_shm::SLOT_FAMILY_PLAYSTATION)));
        assert_eq!(all[2].map(|p| (p.pad.buttons, p.family)), Some((0x2000, pads_shm::SLOT_FAMILY_SWITCH_PRO)));

        let no_xbox = steam_input_pads_with(true, pads_shm::FAMILY_PLAYSTATION | pads_shm::FAMILY_SWITCH_PRO, real, Some(&shm));
        assert!(no_xbox[0].is_none());
        assert_eq!(no_xbox[1].map(|p| p.pad.buttons), Some(0x1000));
        assert_eq!(no_xbox[2].map(|p| p.pad.buttons), Some(0x2000));

        let only_switch = steam_input_pads_with(true, pads_shm::FAMILY_SWITCH_PRO, real, Some(&shm));
        assert!(only_switch[0].is_none());
        assert_eq!(only_switch[1].map(|p| (p.pad.buttons, p.family)), Some((0x2000, pads_shm::SLOT_FAMILY_SWITCH_PRO)));
        assert!(only_switch[2].is_none());
    }
}
