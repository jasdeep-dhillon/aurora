//! Linux: Valve's `linux64/steamclient.so`, loaded with `dlopen`, and its
//! Itanium RTTI class names.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;

/// Class names [`class_name`] gives for the engine and its `IClientUser`.
pub const ENGINE_CLASS: &str = "12CSteamClient";
pub const USER_CLASS: &str = "14IClientUserMap";

pub struct Lib(*mut c_void);

impl Lib {
    /// Loads `<root>/linux64/steamclient.so` with Steam's usual environment.
    /// Must run before any other thread starts (it sets env vars).
    pub fn open(root: &Path) -> Result<Lib, String> {
        if let Some(pid) = steam_running() {
            return Err(format!("Steam is running (pid {pid}); only one global instance can exist"));
        }
        let root = steam_env(root)?;
        // steamclient writes some files to the working directory.
        std::env::set_current_dir(&root).map_err(|e| format!("chdir {}: {e}", root.display()))?;
        Lib::dlopen(&root)
    }

    pub fn dlopen(root: &Path) -> Result<Lib, String> {
        let so = root.join("linux64/steamclient.so");
        let c = CString::new(so.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: plain dlopen of a path we built.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            return Err(format!("dlopen {}: {}", so.display(), dlerror()));
        }
        Ok(Lib(h))
    }

    pub fn sym<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = CString::new(name).unwrap();
        // SAFETY: handle from dlopen; caller picks the matching fn type.
        let p = unsafe { libc::dlsym(self.0, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("{name} missing: {}", dlerror()));
        }
        assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: T is a function pointer type of the same size.
        Ok(unsafe { std::mem::transmute_copy(&p) })
    }
}

/// Breakpad's dump folder. Nothing can ever be created under /dev/null
/// (ENOTDIR, even for root), so Valve's handler stops at "Failed writing
/// minidump, nothing to upload." and no crash or assert report leaves the
/// machine.
pub const NO_DUMPS: &str = "/dev/null/steamhost-no-dumps";

/// The environment Valve's own launcher gives steamclient and its children,
/// for both the host and the game side. Returns the canonical root.
/// Must run before any other thread starts.
pub fn steam_env(root: &Path) -> Result<std::path::PathBuf, String> {
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    // Same list steam.sh uses, so helpers like gldriverquery find their libs.
    let rt = root.join("ubuntu12_32/steam-runtime/run.sh");
    let rt_paths = std::process::Command::new(&rt)
        .arg("--print-steam-runtime-library-paths")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let mut ld = format!("{0}/ubuntu12_64:{0}/ubuntu12_32:{0}", root.display());
    if !rt_paths.is_empty() {
        ld = format!("{ld}:{rt_paths}");
    }
    // SAFETY: single-threaded at this point (called first thing from main).
    unsafe {
        std::env::set_var("SteamPath", &root);
        std::env::set_var("ValvePlatformMutex", root.to_string_lossy().to_lowercase());
        std::env::set_var("SteamAppId", "");
        std::env::set_var("BREAKPAD_DUMP_LOCATION", NO_DUMPS);
        std::env::set_var("STEAM_RUNTIME_LIBRARY_PATH", &rt_paths);
        std::env::set_var("LD_LIBRARY_PATH", ld);
    }
    Ok(root)
}

fn dlerror() -> String {
    // SAFETY: dlerror returns null or a NUL-terminated string.
    unsafe {
        let e = libc::dlerror();
        if e.is_null() { "unknown error".into() } else { CStr::from_ptr(e).to_string_lossy().into_owned() }
    }
}

fn steam_running() -> Option<u32> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if matches!(comm.trim(), "steam" | "steamwebhelper") {
            return Some(pid);
        }
    }
    None
}

/// True if `p` lies inside a loaded steamclient.so (so it is safe to read).
fn in_steamclient(p: *const c_void) -> bool {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr only inspects the loader's tables.
    if unsafe { libc::dladdr(p, &mut info) } == 0 || info.dli_fname.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes().ends_with(b"steamclient.so")
}

/// The Itanium RTTI class name of a polymorphic object from steamclient.so.
pub fn class_name(obj: *mut c_void) -> Option<String> {
    if obj.is_null() || !in_steamclient(obj as *const c_void) && !heap_like(obj) {
        return None;
    }
    // SAFETY: each pointer is checked to be inside steamclient.so before use.
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        if !in_steamclient(vt as *const c_void) {
            return None;
        }
        let ti = *vt.offset(-1);
        if !in_steamclient(ti) {
            return None;
        }
        let name = *(ti as *const *const c_char).add(1);
        if !in_steamclient(name as *const c_void) {
            return None;
        }
        Some(CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

/// Interface objects live on the heap; only their vtables are in the library.
fn heap_like(p: *mut c_void) -> bool {
    !p.is_null() && (p as usize) % 8 == 0
}

/// A game process's view: the same library, with Steam's environment, and
/// no global instance of its own.
pub fn for_client(root: &Path) -> Result<Lib, String> {
    Lib::dlopen(&steam_env(root)?)
}

/// Valve's own switches, read from /proc/self/cmdline: no assert minidumps
/// (which Breakpad would otherwise upload to crash.steampowered.com) and no
/// crash dialog or monitor.
pub const QUIET_FLAGS: [&str; 3] = ["-noassert", "-nocrashdialog", "-nocrashmonitor"];

/// Make sure the quiet flags really are on our command line (re-exec with
/// them if not). Returns only when they are.
pub fn ensure_quiet_flags(args: &[String]) {
    if args.iter().any(|a| a == QUIET_FLAGS[0]) {
        return;
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("/proc/self/exe").args(&args[1..]).args(QUIET_FLAGS).exec();
    eprintln!("steamhost: re-exec failed: {err}");
    std::process::exit(1);
}

/// Leave without running static destructors: Valve's engine threads are
/// still running, and normal exit segfaults under them.
pub fn quit(status: i32) -> ! {
    // SAFETY: _exit just ends the process.
    unsafe { libc::_exit(status) }
}
