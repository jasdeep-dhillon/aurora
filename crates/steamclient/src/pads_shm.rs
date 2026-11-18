//! Pads XInput can't see (PlayStation, Switch, Steam Controller, generic
//! DirectInput pads) as the app reads them with SDL3 on Windows, shared with
//! games through a named file mapping. Shared source: the app includes this
//! file with `#[path]`, so both sides always agree on the layout.
//!
//! The app writes each slot's state (a seqlock keeps a read whole) and a
//! heartbeat; a game writes the rumble it wants on a slot.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// Environment variable naming the file mapping (set only on Windows, for
/// games started with our steamclient while the app reads pads).
pub const ENV_PADS: &str = "STEAMINRUST_PADS";
pub const SHM_PREFIX: &str = "Local\\sir-pads-";
pub const MAGIC: u32 = 0x3144_4150; // "PAD1"
pub const SLOTS: usize = 4;
/// A heartbeat older than this means the app stopped reading pads.
pub const STALE_MS: u64 = 3000;

/// XInput's guide button bit (only XInputGetStateEx reports it).
pub const GUIDE: u16 = 0x0400;

fn os_random_16() -> [u8; 16] {
    let mut random = [0u8; 16];
    #[cfg(windows)]
    {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn SystemFunction036(buf: *mut u8, len: u32) -> u8;
        }
        // SAFETY: RtlGenRandom (`SystemFunction036`) fills `random.len()` bytes with CSPRNG entropy.
        if unsafe { SystemFunction036(random.as_mut_ptr(), random.len() as u32) } != 0 {
            return random;
        }
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut random).is_ok()
        {
            return random;
        }
    }
    let _ = random;
    panic!("OS CSPRNG unavailable for pad mapping name")
}

/// Build an unguessable session-local mapping name: `Local\sir-pads-<pid>-<128-bit hex>`.
pub fn new_mapping_name(pid: u32) -> String {
    let random = os_random_16();
    let mut hex = String::with_capacity(32);
    for b in random {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    format!("{SHM_PREFIX}{pid}-{hex}")
}

/// Only accept single-segment `Local\sir-pads-<alnum/hyphen>` names (no NULs, `\`, `/`, or `..`).
pub fn valid_mapping_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(SHM_PREFIX) else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 128 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[repr(C)]
pub struct Shared {
    pub magic: AtomicU32,
    pub _reserved: AtomicU32,
    /// Milliseconds since the Unix epoch, written by the app every poll.
    pub heartbeat: AtomicU64,
    pub slots: [Slot; SLOTS],
}

#[repr(C)]
pub struct Slot {
    /// Odd while the app writes the slot.
    seq: AtomicU32,
    connected: AtomicU32,
    packet: AtomicU32,
    /// XInput button bits (plus [`GUIDE`]).
    buttons: AtomicU32,
    /// Left trigger | right trigger << 8.
    triggers: AtomicU32,
    /// X | Y << 16, XInput's sign (Y up is positive).
    left: AtomicU32,
    right: AtomicU32,
    /// Low-frequency motor | high-frequency motor << 16, set by the game.
    pub rumble: AtomicU32,
}

/// One pad's state, in XINPUT_GAMEPAD terms.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct State {
    pub packet: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

fn pair(a: i16, b: i16) -> u32 {
    a as u16 as u32 | (b as u16 as u32) << 16
}

impl Slot {
    /// App side: publish a pad (or its absence).
    pub fn write(&self, s: Option<State>) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1) | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        let s0 = s.unwrap_or_default();
        self.connected.store(s.is_some() as u32, Ordering::Relaxed);
        self.packet.store(s0.packet, Ordering::Relaxed);
        self.buttons.store(s0.buttons as u32, Ordering::Relaxed);
        self.triggers.store(s0.lt as u32 | (s0.rt as u32) << 8, Ordering::Relaxed);
        self.left.store(pair(s0.lx, s0.ly), Ordering::Relaxed);
        self.right.store(pair(s0.rx, s0.ry), Ordering::Relaxed);
        self.seq.store((seq | 1).wrapping_add(1), Ordering::Release);
    }

    /// Game side: the pad in this slot, read whole.
    pub fn read(&self) -> Option<State> {
        for _ in 0..64 {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let connected = self.connected.load(Ordering::Relaxed) != 0;
            let (t, l, r) = (self.triggers.load(Ordering::Relaxed), self.left.load(Ordering::Relaxed), self.right.load(Ordering::Relaxed));
            let s = State {
                packet: self.packet.load(Ordering::Relaxed),
                buttons: self.buttons.load(Ordering::Relaxed) as u16,
                lt: t as u8,
                rt: (t >> 8) as u8,
                lx: l as u16 as i16,
                ly: (l >> 16) as u16 as i16,
                rx: r as u16 as i16,
                ry: (r >> 16) as u16 as i16,
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return connected.then_some(s);
            }
        }
        None
    }

    pub fn set_rumble(&self, low: u16, high: u16) {
        self.rumble.store(low as u32 | (high as u32) << 16, Ordering::Relaxed);
    }

    pub fn rumble(&self) -> (u16, u16) {
        let v = self.rumble.load(Ordering::Relaxed);
        (v as u16, (v >> 16) as u16)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Shared {
    /// The pads, if the app is still publishing them.
    pub fn live(&self) -> bool {
        self.magic.load(Ordering::Acquire) == MAGIC && now_ms().saturating_sub(self.heartbeat.load(Ordering::Relaxed)) < STALE_MS
    }
}

/// Which shared pad sits in each of the four XInput slots: the app's pads
/// fill the slots real XInput pads leave free, in order. `real[i]`: a real
/// pad holds slot i; `shared`: the shared slots that have a pad.
pub fn seats(real: [bool; SLOTS], shared: &[usize]) -> [Option<usize>; SLOTS] {
    let mut out = [None; SLOTS];
    let mut next = shared.iter().copied();
    for (i, seat) in out.iter_mut().enumerate() {
        if !real[i] {
            *seat = next.next();
        }
    }
    out
}

#[cfg(windows)]
mod win_shm {
    use super::{MAGIC, Shared, new_mapping_name, valid_mapping_name};
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
        fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn MapViewOfFile(map: *mut c_void, access: u32, off_hi: u32, off_lo: u32, bytes: usize) -> *mut c_void;
        fn UnmapViewOfFile(view: *const c_void) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn GetLastError() -> u32;
        fn SetLastError(err: u32);
    }

    const PAGE_READWRITE: u32 = 0x04;
    const FILE_MAP_WRITE: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;
    const FILE_MAP_ALL_ACCESS: u32 = 0xF001F;
    const ERROR_ALREADY_EXISTS: u32 = 183;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub struct Shm {
        handle: *mut c_void,
        view: *mut Shared,
        name: String,
    }

    unsafe impl Send for Shm {}
    unsafe impl Sync for Shm {}

    impl Shm {
        pub fn create() -> Option<Self> {
            let name = new_mapping_name(std::process::id());
            Self::create_named(&name)
        }

        pub fn create_named(name: &str) -> Option<Self> {
            if !valid_mapping_name(name) {
                return None;
            }
            let wide_name = wide(name);
            let size = std::mem::size_of::<Shared>();
            let (h, err) = unsafe {
                SetLastError(0);
                let h = CreateFileMappingW(-1, std::ptr::null(), PAGE_READWRITE, 0, size as u32, wide_name.as_ptr());
                let err = GetLastError();
                (h, err)
            };
            if h.is_null() {
                return None;
            }
            if err == ERROR_ALREADY_EXISTS {
                unsafe { CloseHandle(h) };
                return None;
            }
            let view = unsafe { MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, size) } as *mut Shared;
            if view.is_null() {
                unsafe { CloseHandle(h) };
                return None;
            }
            let shared = unsafe { &*view };
            shared.magic.store(MAGIC, Ordering::Release);
            Some(Self { handle: h, view, name: name.to_owned() })
        }

        pub fn shared(&self) -> &Shared {
            unsafe { &*self.view }
        }

        pub fn name(&self) -> &str {
            &self.name
        }
    }

    impl Drop for Shm {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.view.cast());
                CloseHandle(self.handle);
            }
        }
    }

    pub fn open_shm(name: &str) -> Option<&'static Shared> {
        if !valid_mapping_name(name) {
            return None;
        }
        let access = FILE_MAP_READ | FILE_MAP_WRITE;
        let wide_name = wide(name);
        let h = unsafe { OpenFileMappingW(access, 0, wide_name.as_ptr()) };
        if h.is_null() {
            return None;
        }
        let view = unsafe { MapViewOfFile(h, access, 0, 0, std::mem::size_of::<Shared>()) } as *const Shared;
        if view.is_null() {
            unsafe { CloseHandle(h) };
            return None;
        }
        unsafe { view.as_ref() }
    }
}

#[cfg(windows)]
pub use win_shm::{Shm, open_shm};

#[cfg(not(windows))]
pub fn open_shm(_name: &str) -> Option<&'static Shared> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_round_trip() {
        let slot: Slot = unsafe { std::mem::zeroed() };
        assert_eq!(slot.read(), None);
        let s = State { packet: 7, buttons: 0x1000 | GUIDE, lt: 255, rt: 1, lx: -32768, ly: 32767, rx: -1, ry: 0 };
        slot.write(Some(s));
        assert_eq!(slot.read(), Some(s));
        slot.write(None);
        assert_eq!(slot.read(), None);
        slot.set_rumble(0xFFFF, 0x1234);
        assert_eq!(slot.rumble(), (0xFFFF, 0x1234));
    }

    #[test]
    fn shared_pads_fill_free_slots() {
        assert_eq!(seats([true, false, false, false], &[2]), [None, Some(2), None, None]);
        assert_eq!(seats([false, true, false, true], &[0, 1, 3]), [Some(0), None, Some(1), None]);
        assert_eq!(seats([true; 4], &[0]), [None; 4]);
    }

    #[test]
    fn mapping_name_has_128_bit_random_suffix_and_strict_prefix_validation() {
        let n1 = new_mapping_name(4242);
        let n2 = new_mapping_name(4242);
        assert_ne!(n1, n2, "random 128-bit suffix must differ across calls");
        let suffix = n1.strip_prefix("Local\\sir-pads-4242-").expect("expected Local\\sir-pads-<pid>-<hex>");
        assert_eq!(suffix.len(), 32);
        assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert!(valid_mapping_name(&n1));
        assert!(valid_mapping_name("Local\\sir-pads-test-1234"));

        for bad in [
            "",
            "Local\\sir-pads-",
            "Global\\sir-pads-4242-0123456789abcdef0123456789abcdef",
            "sir-pads-4242-0123456789abcdef0123456789abcdef",
            "Local\\evil-pads-4242",
            "Local\\sir-pads-4242\\nested",
            "Local\\sir-pads-..\\escape",
            "Local\\sir-pads-4242\0extra",
            "Local\\sir-pads-4242 space",
        ] {
            assert!(!valid_mapping_name(bad), "expected {bad:?} to be rejected");
        }
    }

    #[cfg(windows)]
    #[test]
    fn shm_create_fails_when_pre_squatted_and_open_shm_validates_prefix() {
        use std::ffi::c_void;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileMappingW(file: isize, attrs: *const c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> *mut c_void;
            fn MapViewOfFile(map: *mut c_void, access: u32, off_hi: u32, off_lo: u32, bytes: usize) -> *mut c_void;
            fn UnmapViewOfFile(view: *const c_void) -> i32;
            fn CloseHandle(h: *mut c_void) -> i32;
        }

        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain([0]).collect() };
        let squat_name = format!("Local\\sir-pads-squat-{}", std::process::id());
        let size = std::mem::size_of::<Shared>();
        let squatter = unsafe { CreateFileMappingW(-1, std::ptr::null(), 0x04, 0, size as u32, wide(&squat_name).as_ptr()) };
        assert!(!squatter.is_null());
        let squatter_view = unsafe { MapViewOfFile(squatter, 0xF001F, 0, 0, size) } as *mut Shared;
        assert!(!squatter_view.is_null());
        unsafe { (*squatter_view).magic.store(0xDEAD_BEEF, Ordering::SeqCst) };

        // Pre-squatted mapping must fail closed and leave squatter's mapping untouched.
        assert!(Shm::create_named(&squat_name).is_none());
        assert_eq!(unsafe { (*squatter_view).magic.load(Ordering::SeqCst) }, 0xDEAD_BEEF);

        unsafe {
            UnmapViewOfFile(squatter_view.cast());
            CloseHandle(squatter);
        }

        // Once released, creating the mapping succeeds cleanly.
        let shm = Shm::create_named(&squat_name).expect("fresh mapping succeeds");
        assert_eq!(shm.shared().magic.load(Ordering::Acquire), MAGIC);

        // Random Shm::create() round-trips through open_shm.
        let rand_shm = Shm::create().expect("random Shm::create");
        assert!(valid_mapping_name(rand_shm.name()));
        let opened = open_shm(rand_shm.name()).expect("open_shm accepts valid Local\\sir-pads- name");
        assert_eq!(opened.magic.load(Ordering::Acquire), MAGIC);

        assert!(open_shm("Global\\sir-pads-1234").is_none());
        assert!(open_shm("Local\\other-1234").is_none());
    }
}
