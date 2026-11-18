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

const MAX_CLASS_NAME: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MapRange {
    pub start: usize,
    pub end: usize,
    pub read: bool,
    pub exec: bool,
    pub is_steamclient: bool,
}

pub(crate) fn parse_maps(text: &str) -> Vec<MapRange> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        let (Some(range), Some(perms)) = (cols.next(), cols.next()) else { continue };
        let Some((s_hex, e_hex)) = range.split_once('-') else { continue };
        let (Ok(start), Ok(end)) = (usize::from_str_radix(s_hex, 16), usize::from_str_radix(e_hex, 16)) else {
            continue;
        };
        if start >= end {
            continue;
        }
        let bytes = perms.as_bytes();
        let read = bytes.first() == Some(&b'r');
        let exec = bytes.get(2) == Some(&b'x');
        let _ = (cols.next(), cols.next(), cols.next());
        let has_path = cols.next().is_some();
        let trimmed = line.trim_end();
        let is_steamclient = has_path && (trimmed.ends_with("/steamclient.so") || trimmed.ends_with(" steamclient.so"));
        out.push(MapRange { start, end, read, exec, is_steamclient });
    }
    out
}

fn load_maps() -> Vec<MapRange> {
    std::fs::read_to_string("/proc/self/maps").map(|s| parse_maps(&s)).unwrap_or_default()
}

pub(crate) fn range_in(maps: &[MapRange], addr: usize, len: usize, pred: impl Fn(&MapRange) -> bool) -> bool {
    if addr == 0 || len == 0 {
        return false;
    }
    let Some(end) = addr.checked_add(len) else {
        return false;
    };
    let mut cur = addr;
    while cur < end {
        let Some(m) = maps.iter().find(|m| pred(m) && m.start <= cur && cur < m.end) else {
            return false;
        };
        cur = m.end;
    }
    true
}

/// True if `p` lies inside a loaded steamclient.so.
fn in_steamclient(p: *const c_void) -> bool {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr only inspects the loader's tables.
    if unsafe { libc::dladdr(p, &mut info) } == 0 || info.dli_fname.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes().ends_with(b"steamclient.so")
}

pub(crate) fn class_name_with(obj: *mut c_void, maps: &[MapRange], check_dladdr: bool) -> Option<String> {
    let ptr_size = size_of::<usize>();
    let ptr_align = align_of::<usize>();
    let obj_addr = obj as usize;
    if obj.is_null() || obj_addr % ptr_align != 0 || !range_in(maps, obj_addr, ptr_size, |m| m.read) {
        return None;
    }
    // SAFETY: `obj` is verified non-null, aligned, and inside a readable mapping.
    let vt = unsafe { *(obj as *const *const *const c_void) };
    let vt_addr = vt as usize;
    let vt_prev = vt_addr.checked_sub(ptr_size)?;
    if vt.is_null()
        || vt_addr % ptr_align != 0
        || !range_in(maps, vt_prev, 2 * ptr_size, |m| m.read && m.is_steamclient)
        || (check_dladdr && (!in_steamclient(vt as *const c_void) || !in_steamclient(vt_prev as *const c_void)))
    {
        return None;
    }
    // SAFETY: `[vt - 8, vt + 8)` is verified readable inside steamclient.so.
    let ti = unsafe { *vt.offset(-1) };
    let ti_addr = ti as usize;
    if ti.is_null()
        || ti_addr % ptr_align != 0
        || !range_in(maps, ti_addr, 2 * ptr_size, |m| m.read && m.is_steamclient)
        || (check_dladdr && !in_steamclient(ti))
    {
        return None;
    }
    // SAFETY: `[ti, ti + 16)` (std::type_info vptr + __name pointer) is verified readable inside steamclient.so.
    let name = unsafe { *(ti as *const *const c_char).add(1) };
    let name_addr = name as usize;
    if name.is_null() || (check_dladdr && !in_steamclient(name as *const c_void)) {
        return None;
    }
    let mut bytes = Vec::new();
    for i in 0..MAX_CLASS_NAME {
        let b_addr = name_addr.checked_add(i)?;
        if !range_in(maps, b_addr, 1, |m| m.read && m.is_steamclient) {
            return None;
        }
        if check_dladdr && !in_steamclient(b_addr as *const c_void) {
            return None;
        }
        // SAFETY: `b_addr` is verified readable inside steamclient.so.
        let b = unsafe { *(b_addr as *const u8) };
        if b == 0 {
            return (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned());
        }
        bytes.push(b);
    }
    None
}

/// The Itanium RTTI class name of a polymorphic object from steamclient.so.
pub fn class_name(obj: *mut c_void) -> Option<String> {
    let maps = load_maps();
    class_name_with(obj, &maps, true)
}

pub(crate) fn verify_vtable_with(
    obj: *mut c_void,
    max_slot: usize,
    maps: &[MapRange],
    check_dladdr: bool,
) -> Result<(), String> {
    let ptr_size = size_of::<usize>();
    let ptr_align = align_of::<usize>();
    let obj_addr = obj as usize;
    if obj.is_null() || obj_addr % ptr_align != 0 || !range_in(maps, obj_addr, ptr_size, |m| m.read) {
        return Err("unsupported Steam build: interface object pointer is invalid or unreadable".into());
    }
    // SAFETY: `obj` is non-null, aligned, and readable.
    let vt = unsafe { *(obj as *const *const *const c_void) };
    let vt_addr = vt as usize;
    let table_bytes = max_slot
        .checked_add(1)
        .and_then(|n| n.checked_mul(ptr_size))
        .ok_or_else(|| "unsupported Steam build: vtable size overflow".to_string())?;
    if vt.is_null()
        || vt_addr % ptr_align != 0
        || !range_in(maps, vt_addr, table_bytes, |m| m.read && m.is_steamclient)
        || (check_dladdr && !in_steamclient(vt as *const c_void))
    {
        return Err("unsupported Steam build: vtable is outside readable steamclient memory".into());
    }
    for slot in 0..=max_slot {
        // SAFETY: `vt[0..=max_slot]` lies inside readable steamclient memory.
        let fp = unsafe { *vt.add(slot) };
        let fp_addr = fp as usize;
        if fp.is_null()
            || !range_in(maps, fp_addr, 1, |m| m.read && m.exec && m.is_steamclient)
            || (check_dladdr && !in_steamclient(fp))
        {
            return Err(format!(
                "unsupported Steam build: vtable slot {slot} is not an executable steamclient code pointer"
            ));
        }
    }
    Ok(())
}

/// Verify that `obj`'s vtable slots `0..=max_slot` are non-null readable code
/// pointers inside the loaded `steamclient.so` executable segment.
pub fn verify_vtable(obj: *mut c_void, max_slot: usize) -> Result<(), String> {
    let maps = load_maps();
    verify_vtable_with(obj, max_slot, &maps, true)
}

/// Verify a single vtable slot on `obj` before invoking it.
pub fn verify_slot(obj: *mut c_void, slot: usize) -> Result<(), String> {
    let maps = load_maps();
    let ptr_size = size_of::<usize>();
    let ptr_align = align_of::<usize>();
    let obj_addr = obj as usize;
    if obj.is_null() || obj_addr % ptr_align != 0 || !range_in(&maps, obj_addr, ptr_size, |m| m.read) {
        return Err("unsupported Steam build: interface object pointer is invalid or unreadable".into());
    }
    let vt = unsafe { *(obj as *const *const *const c_void) };
    let vt_addr = vt as usize;
    let slot_addr = slot
        .checked_mul(ptr_size)
        .and_then(|off| vt_addr.checked_add(off))
        .ok_or_else(|| "unsupported Steam build: vtable slot overflow".to_string())?;
    if vt.is_null()
        || vt_addr % ptr_align != 0
        || !range_in(&maps, slot_addr, ptr_size, |m| m.read && m.is_steamclient)
        || !in_steamclient(slot_addr as *const c_void)
    {
        return Err(format!("unsupported Steam build: vtable slot {slot} is outside readable steamclient memory"));
    }
    let fp = unsafe { *(slot_addr as *const *const c_void) };
    if fp.is_null()
        || !range_in(&maps, fp as usize, 1, |m| m.read && m.exec && m.is_steamclient)
        || !in_steamclient(fp)
    {
        return Err(format!(
            "unsupported Steam build: vtable slot {slot} is not an executable steamclient code pointer"
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn dummy_code() {}

    #[test]
    fn class_name_rejects_unreadable_mapped_memory_without_sigsegv() {
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(p, libc::MAP_FAILED);
        assert_eq!(class_name(p), None);
        // Unmapped / bogus aligned address also rejected without crashing.
        assert_eq!(class_name(0x1000usize as *mut c_void), None);
        unsafe {
            libc::munmap(p, 4096);
        }
    }

    #[test]
    fn parse_maps_and_rtti_and_vtable_validation() {
        let sample = "\
7f0000100000-7f0000200000 r-xp 00000000 08:01 10 /opt/Steam Path/linux64/steamclient.so
7f0000200000-7f0000210000 r--p 00100000 08:01 10 /opt/Steam Path/linux64/steamclient.so
7f0000300000-7f0000310000 ---p 00000000 00:00 0
";
        let parsed = parse_maps(sample);
        assert_eq!(parsed.len(), 3);
        assert!(parsed[0].read && parsed[0].exec && parsed[0].is_steamclient);
        assert!(parsed[1].read && !parsed[1].exec && parsed[1].is_steamclient);
        assert!(!parsed[2].read && !parsed[2].exec && !parsed[2].is_steamclient);

        // Synthetic Itanium C++ object + RTTI + 57-slot vtable validated against synthetic MapRange table.
        let rtti_name = b"14IClientUserMap\0";
        let typeinfo: [usize; 2] = [0, rtti_name.as_ptr() as usize];
        let code_ptr = dummy_code as *const () as usize;
        let mut raw_vt = vec![code_ptr; 58]; // index 0 = vtable[-1] (typeinfo), indices 1..=57 = slots 0..=56
        raw_vt[0] = typeinfo.as_ptr() as usize;
        let vtable_ptr = unsafe { raw_vt.as_ptr().add(1) };
        let obj: usize = vtable_ptr as usize;
        let obj_ptr = &obj as *const usize as *mut c_void;

        let ro_min = (rtti_name.as_ptr() as usize)
            .min(typeinfo.as_ptr() as usize)
            .min(raw_vt.as_ptr() as usize);
        let ro_max = (rtti_name.as_ptr() as usize + rtti_name.len())
            .max(typeinfo.as_ptr() as usize + size_of_val(&typeinfo))
            .max(raw_vt.as_ptr() as usize + raw_vt.len() * size_of::<usize>());

        let maps = vec![
            MapRange { start: obj_ptr as usize, end: obj_ptr as usize + 8, read: true, exec: false, is_steamclient: false },
            MapRange { start: ro_min, end: ro_max, read: true, exec: false, is_steamclient: true },
            MapRange { start: code_ptr, end: code_ptr + 16, read: true, exec: true, is_steamclient: true },
        ];

        assert_eq!(class_name_with(obj_ptr, &maps, false).as_deref(), Some("14IClientUserMap"));
        assert!(verify_vtable_with(obj_ptr, 56, &maps, false).is_ok());

        // Non-executable slot 56 must fail closed!
        raw_vt[57] = typeinfo.as_ptr() as usize; // readable inside steamclient, NOT executable
        assert!(verify_vtable_with(obj_ptr, 56, &maps, false).is_err());

        // Null slot must fail closed!
        raw_vt[57] = 0;
        assert!(verify_vtable_with(obj_ptr, 56, &maps, false).is_err());

        // Unterminated 300-byte RTTI name must return None (bounded at 256 bytes).
        let overlong = [b'A'; 300];
        let bad_ti: [usize; 2] = [0, overlong.as_ptr() as usize];
        raw_vt[0] = bad_ti.as_ptr() as usize;
        let maps_overlong = vec![
            MapRange { start: 0x1000, end: usize::MAX - 0x1000, read: true, exec: true, is_steamclient: true },
        ];
        assert_eq!(class_name_with(obj_ptr, &maps_overlong, false), None);
    }
}
