//! Windows: Valve's `steamclient64.dll` from the user's Steam folder, loaded
//! with `LoadLibraryExW`, and its MSVC RTTI class names.
//!
//! The engine's interface layouts match the Linux build's (Valve generates
//! both from the same definitions); only the loader and the RTTI format
//! differ. Each object is still checked by class name before it's used.

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Class names [`class_name`] gives for the engine and its `IClientUser`.
pub const ENGINE_CLASS: &str = ".?AVCSteamClient@@";
pub const USER_CLASS: &str = ".?AVIClientUserMap@@";

const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_GUARD: u32 = 0x100;
const READABLE: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[repr(C)]
struct MemoryInfo {
    base: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetLastError() -> u32;
    fn SetDllDirectoryW(dir: *const u16) -> i32;
    fn GetModuleHandleExW(flags: u32, addr: *const c_void, module: *mut *mut c_void) -> i32;
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryInfo, len: usize) -> usize;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
    fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// The loaded steamclient64.dll's base address, for [`in_steamclient`].
static BASE: AtomicUsize = AtomicUsize::new(0);

pub struct Lib(*mut c_void);

impl Lib {
    /// Loads `<root>\steamclient64.dll` as the global instance's engine.
    pub fn open(root: &Path) -> Result<Lib, String> {
        if let Some(pid) = steam_running() {
            return Err(format!("Steam is running (pid {pid}); only one global instance can exist"));
        }
        for_client(root)
    }

    fn load(root: &Path) -> Result<Lib, String> {
        let dll = root.join("steamclient64.dll");
        // Its own dependencies (tier0_s64.dll, vstdlib_s64.dll, …) sit beside it.
        // SAFETY: plain Win32 calls with NUL-terminated wide strings we built.
        let h = unsafe {
            SetDllDirectoryW(wide(root.as_os_str()).as_ptr());
            LoadLibraryExW(wide(dll.as_os_str()).as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH)
        };
        if h.is_null() {
            return Err(format!("LoadLibrary {}: error {}", dll.display(), unsafe { GetLastError() }));
        }
        BASE.store(h as usize, Ordering::Relaxed);
        Ok(Lib(h))
    }

    pub fn sym<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = CString::new(name).unwrap();
        // SAFETY: module from LoadLibraryExW; caller picks the matching fn type.
        let p = unsafe { GetProcAddress(self.0, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("{name} missing (error {})", unsafe { GetLastError() }));
        }
        assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: T is a function pointer type of the same size.
        Ok(unsafe { std::mem::transmute_copy(&p) })
    }
}

/// Breakpad's dump folder: `|` can't be in a Windows path, so no folder can
/// be made there and no crash or assert report is written or uploaded.
pub const NO_DUMPS: &str = "C:\\steamhost|no-dumps";

/// The environment Steam gives steamclient, then the library itself.
/// Must run before any other thread starts (it sets env vars).
pub fn for_client(root: &Path) -> Result<Lib, String> {
    let root = steam_root(root)?;
    // SAFETY: single-threaded at this point (called first thing from main).
    unsafe {
        std::env::set_var("SteamPath", &root);
        std::env::set_var("SteamAppId", "");
        std::env::set_var("BREAKPAD_DUMP_LOCATION", NO_DUMPS);
    }
    // steamclient writes some files to the working directory.
    std::env::set_current_dir(&root).map_err(|e| format!("chdir {}: {e}", root.display()))?;
    Lib::load(&root)
}

/// The Steam folder without the `\\?\` prefix `canonicalize` adds, which
/// Valve's path handling doesn't expect.
fn steam_root(root: &Path) -> Result<PathBuf, String> {
    let c = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let s = c.to_string_lossy();
    Ok(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)))
}

/// Committed, readable memory (so reading `len` bytes at `p` can't fault).
fn readable(p: *const c_void, len: usize) -> bool {
    if p.is_null() {
        return false;
    }
    let mut info: MemoryInfo = unsafe { std::mem::zeroed() };
    // SAFETY: VirtualQuery only inspects the address space.
    if unsafe { VirtualQuery(p, &mut info, size_of::<MemoryInfo>()) } == 0 {
        return false;
    }
    let end = info.base as usize + info.region_size;
    info.state == MEM_COMMIT && info.protect & READABLE != 0 && info.protect & PAGE_GUARD == 0 && p as usize + len <= end
}

/// True if `p` lies inside the loaded steamclient64.dll.
fn in_steamclient(p: *const c_void) -> bool {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 || p.is_null() {
        return false;
    }
    let mut module = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    // SAFETY: lookup only; the refcount is left unchanged.
    unsafe { GetModuleHandleExW(flags, p, &mut module) != 0 && module as usize == base && readable(p, 8) }
}

/// The MSVC RTTI class name (`.?AV<name>@@`) of a polymorphic object from
/// steamclient64.dll: vtable[-1] is its CompleteObjectLocator, whose type
/// descriptor (an image-relative offset on x64) holds the decorated name.
pub fn class_name(obj: *mut c_void) -> Option<String> {
    if !readable(obj, 8) || (obj as usize) % 8 != 0 {
        return None;
    }
    let base = BASE.load(Ordering::Relaxed);
    // SAFETY: every pointer is checked to be readable (and, past the object
    // itself, inside steamclient64.dll) before it is read.
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        if !in_steamclient(vt as *const c_void) || !in_steamclient(vt.offset(-1) as *const c_void) {
            return None;
        }
        let col = *vt.offset(-1) as *const u32;
        // signature 1 = the x64 layout with image-relative offsets.
        if !in_steamclient(col as *const c_void) || !readable(col as *const c_void, 24) || *col != 1 {
            return None;
        }
        let name = (base + *col.add(3) as usize + 16) as *const c_char;
        if !in_steamclient(name as *const c_void) {
            return None;
        }
        Some(CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

/// The official client (or its web helper) is running.
fn steam_running() -> Option<u32> {
    // SAFETY: Toolhelp snapshot walk with a correctly sized entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return None;
        }
        let mut e: ProcessEntry = std::mem::zeroed();
        e.size = size_of::<ProcessEntry>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        let mut found = None;
        while ok {
            let len = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            let name = String::from_utf16_lossy(&e.exe[..len]).to_lowercase();
            if name == "steam.exe" || name == "steamwebhelper.exe" {
                found = Some(e.pid);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Valve's own switches, read from the command line: no assert minidumps
/// and no crash dialog or monitor.
pub const QUIET_FLAGS: [&str; 3] = ["-noassert", "-nocrashdialog", "-nocrashmonitor"];

/// Make sure the quiet flags really are on our command line. Windows has no
/// exec: without them, run ourselves again with them (same pipes, no
/// window) and leave with its exit code.
pub fn ensure_quiet_flags(args: &[String]) {
    if args.iter().any(|a| a == QUIET_FLAGS[0]) {
        return;
    }
    use std::os::windows::process::CommandExt;
    let code = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).args(&args[1..]).args(QUIET_FLAGS).creation_flags(CREATE_NO_WINDOW).status())
        .map(|s| s.code().unwrap_or(1));
    match code {
        Ok(c) => quit(c),
        Err(e) => {
            eprintln!("steamhost: restart failed: {e}");
            quit(1)
        }
    }
}

/// Leave without DLL detach or static destructors: Valve's engine threads
/// are still running under them.
pub fn quit(status: i32) -> ! {
    // SAFETY: ends this process right away.
    unsafe {
        TerminateProcess(GetCurrentProcess(), status as u32);
    }
    unreachable!("TerminateProcess returned")
}
