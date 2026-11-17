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
pub const MAGIC: u32 = 0x3144_4150; // "PAD1"
pub const SLOTS: usize = 4;
/// A heartbeat older than this means the app stopped reading pads.
pub const STALE_MS: u64 = 3000;

/// XInput's guide button bit (only XInputGetStateEx reports it).
pub const GUIDE: u16 = 0x0400;

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
}
