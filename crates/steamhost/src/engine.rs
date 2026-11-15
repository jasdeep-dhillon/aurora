//! Loading Valve's steamclient.so and calling its engine interface.
//!
//! Slot numbers are for the pinned build only (see `layout`): Valve reorders
//! these interfaces between builds, so nothing here may run on another one.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;

/// `IClientEngine` (CLIENTENGINE_INTERFACE_VERSION005) slots, recovered from
/// this build's `CSteamClient` vtable: the three that open the `Steam3Master`
/// IPC server, and the assert strings in slots 5 and 12.
mod layout {
    pub const CREATE_STEAM_PIPE: usize = 0;
    pub const CREATE_GLOBAL_USER: usize = 2;
    /// Getters shaped `(hUser, hPipe) -> object` (lookup under the engine lock).
    pub const USER_GETTERS: &[usize] = &[
        8, 9, 10, 13, 15, 16, 17, 18, 21, 22, 23, 24, 25, 27, 34, 35, 43, 44, 46, 47, 50, 51, 52, 54, 55, 57, 58,
        59, 60, 62, 63, 64, 65, 67, 69, 70, 71, 72, 74, 75, 76, 79,
    ];
    /// Getters shaped `(hPipe) -> object` (lookup in m_mapClientInterfaces).
    pub const PIPE_GETTERS: &[usize] = &[14, 36, 37, 38, 39, 40, 41, 53, 56, 61, 66, 77, 78];
}

type HSteamPipe = c_int;
type HSteamUser = c_int;

struct Lib(*mut c_void);

impl Lib {
    /// Loads `<root>/linux64/steamclient.so` with Steam's usual environment.
    /// Must run before any other thread starts (it sets env vars).
    fn open(root: &Path) -> Result<Lib, String> {
        if let Some(pid) = steam_running() {
            return Err(format!("Steam is running (pid {pid}); only one global instance can exist"));
        }
        let root = steam_env(root)?;
        // steamclient writes some files to the working directory.
        std::env::set_current_dir(&root).map_err(|e| format!("chdir {}: {e}", root.display()))?;
        Lib::dlopen(&root)
    }

    fn dlopen(root: &Path) -> Result<Lib, String> {
        let so = root.join("linux64/steamclient.so");
        let c = CString::new(so.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: plain dlopen of a path we built.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            return Err(format!("dlopen {}: {}", so.display(), dlerror()));
        }
        Ok(Lib(h))
    }

    fn sym<T: Copy>(&self, name: &str) -> Result<T, String> {
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
fn steam_env(root: &Path) -> Result<std::path::PathBuf, String> {
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

/// Function pointer in slot `i` of `obj`'s vtable.
///
/// SAFETY: `obj` must be a live C++ object whose vtable has more than `i`
/// slots, and F must match that method's real signature.
unsafe fn slot<F: Copy>(obj: *mut c_void, i: usize) -> F {
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        std::mem::transmute_copy(&*vt.add(i))
    }
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
fn class_name(obj: *mut c_void) -> Option<String> {
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

fn engine(lib: &Lib) -> Result<*mut c_void, String> {
    type CreateInterface = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
    let create: CreateInterface = lib.sym("CreateInterface")?;
    let mut rc: c_int = 0;
    // SAFETY: Valve's exported factory, called with a valid version string.
    let e = unsafe { create(c"CLIENTENGINE_INTERFACE_VERSION005".as_ptr(), &mut rc) };
    if e.is_null() {
        return Err(format!("CreateInterface(CLIENTENGINE_INTERFACE_VERSION005) failed ({rc})"));
    }
    match class_name(e).as_deref() {
        Some("12CSteamClient") => Ok(e),
        other => Err(format!("unexpected engine class {other:?}: not the pinned build")),
    }
}

/// Become the global Steam instance: Valve's engine opens its `Steam3Master`
/// IPC server in this process, which games then connect to.
fn global_instance(lib: &Lib) -> Result<(*mut c_void, HSteamPipe, HSteamUser), String> {
    let e = engine(lib)?;
    // SAFETY: slots and signatures from `layout`, checked against this build.
    let (pipe, user) = unsafe {
        let create_pipe: unsafe extern "C" fn(*mut c_void) -> HSteamPipe = slot(e, layout::CREATE_STEAM_PIPE);
        let create_global: unsafe extern "C" fn(*mut c_void, *mut HSteamPipe) -> HSteamUser =
            slot(e, layout::CREATE_GLOBAL_USER);
        let mut pipe = create_pipe(e);
        let user = create_global(e, &mut pipe);
        (pipe, user)
    };
    println!("global instance: pipe={pipe} user={user}");
    if user == 0 {
        return Err("CreateGlobalUser returned 0".into());
    }
    Ok((e, pipe, user))
}

fn hold() {
    let secs: u64 = std::env::var("STEAMHOST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    println!("holding the global instance for {secs}s");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

/// `IClientUser` slots in this build, named from the ("IClientUser",
/// "<method>") strings its IPC wrappers pass; signatures from how each one
/// serializes its arguments and reply.
mod user {
    pub const GETTER: usize = 8; // IClientEngine::GetIClientUser(hUser, hPipe)
    pub const LOG_ON: usize = 1; // EResult LogOn(CSteamID)
    pub const B_LOGGED_ON: usize = 4; // bool BLoggedOn()
    pub const GET_LOGON_STATE: usize = 5; // int GetLogonState()
    pub const SET_LOGIN_TOKEN: usize = 56; // void SetLoginToken(const char* token, const char* account)
}

/// Sign in with a Steam-client refresh token. The account name, token and
/// SteamID come on stdin (one per line), never argv/env, which other
/// processes can read.
pub fn login(root: &Path) -> Result<(), String> {
    let mut lines = std::io::stdin().lines();
    let mut next = |what: &str| -> Result<String, String> {
        let l = lines.next().ok_or(format!("stdin: missing {what}"))?.map_err(|e| e.to_string())?;
        Ok(l.trim().to_owned())
    };
    let (account, token, steam_id) = (next("account name")?, next("refresh token")?, next("SteamID")?);
    let steam_id: u64 = steam_id.parse().map_err(|_| "SteamID must be a 64-bit number".to_string())?;
    let (account, token) =
        (CString::new(account).map_err(|e| e.to_string())?, CString::new(token).map_err(|e| e.to_string())?);

    let lib = Lib::open(root)?;
    let (e, pipe, h_user) = global_instance(&lib)?;
    // SAFETY: getter slot shaped (this, hUser, hPipe), see `layout`.
    let cu = unsafe {
        let get: unsafe extern "C" fn(*mut c_void, HSteamUser, HSteamPipe) -> *mut c_void = slot(e, user::GETTER);
        get(e, h_user, pipe)
    };
    if class_name(cu).as_deref() != Some("14IClientUserMap") {
        return Err("GetIClientUser didn't return IClientUser: not the pinned build".into());
    }
    // SAFETY: IClientUser slots and signatures from `user`, checked against this build.
    unsafe {
        let set_token: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) = slot(cu, user::SET_LOGIN_TOKEN);
        set_token(cu, token.as_ptr(), account.as_ptr());
        let log_on: unsafe extern "C" fn(*mut c_void, u64) -> c_int = slot(cu, user::LOG_ON);
        println!("LogOn -> EResult {}", log_on(cu, steam_id));
    }
    drop(token);
    let logged_on: unsafe extern "C" fn(*mut c_void) -> bool = unsafe { slot(cu, user::B_LOGGED_ON) };
    let state: unsafe extern "C" fn(*mut c_void) -> c_int = unsafe { slot(cu, user::GET_LOGON_STATE) };
    let mut last = -1;
    for _ in 0..120 {
        // SAFETY: as above.
        let (s, ok) = unsafe { (state(cu), logged_on(cu)) };
        if s != last {
            println!("logon state {s}");
            last = s;
        }
        if ok {
            println!("logged on");
            hold();
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("not logged on after 60s".into())
}

pub fn probe(root: &Path) -> Result<(), String> {
    let lib = Lib::open(root)?;
    let (e, pipe, user) = global_instance(&lib)?;
    for &i in layout::USER_GETTERS {
        // SAFETY: lookup-only getter shaped (this, hUser, hPipe).
        let f: unsafe extern "C" fn(*mut c_void, HSteamUser, HSteamPipe) -> *mut c_void = unsafe { slot(e, i) };
        let o = unsafe { f(e, user, pipe) };
        println!("slot {i:2} (user,pipe) -> {}", class_name(o).unwrap_or_else(|| format!("{o:?}")));
    }
    for &i in layout::PIPE_GETTERS {
        // SAFETY: lookup-only getter shaped (this, hPipe).
        let f: unsafe extern "C" fn(*mut c_void, HSteamPipe) -> *mut c_void = unsafe { slot(e, i) };
        let o = unsafe { f(e, pipe) };
        println!("slot {i:2} (pipe)      -> {}", class_name(o).unwrap_or_else(|| format!("{o:?}")));
    }
    hold();
    Ok(())
}

pub fn client_test(root: &Path) -> Result<(), String> {
    // A game process loads the same library and connects over IPC.
    let lib = Lib::dlopen(&steam_env(root)?)?;
    let create_pipe: unsafe extern "C" fn() -> HSteamPipe = lib.sym("Steam_CreateSteamPipe")?;
    let connect: unsafe extern "C" fn(HSteamPipe) -> HSteamUser = lib.sym("Steam_ConnectToGlobalUser")?;
    // SAFETY: Valve's flat exports with their public Steamworks signatures.
    let pipe = unsafe { create_pipe() };
    println!("client pipe={pipe}");
    if pipe == 0 {
        return Err("no Steam3Master to connect to".into());
    }
    let user = unsafe { connect(pipe) };
    println!("client user={user}");
    if user == 0 { Err("ConnectToGlobalUser refused".into()) } else { Ok(()) }
}
