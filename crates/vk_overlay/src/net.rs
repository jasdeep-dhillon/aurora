//! Loopback TCP client for Aurora's in-game overlay protocol.
//! Connects to `127.0.0.1:<AURORA_OVERLAY_PORT>` with `AURORA_OVERLAY_TOKEN`
//! once the first game swapchain presents.

use crate::wire::{FromGame, Input, MOD_SHIFT, Mesh, TexSet, ToGame};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::Duration;

const DEFAULT_OVERLAY_MODS: u8 = MOD_SHIFT;
const DEFAULT_OVERLAY_VK: u32 = 0x09; // VK_TAB
const DEFAULT_SHOT_MODS: u8 = 0;
const DEFAULT_SHOT_VK: u32 = 0x7B; // VK_F12

/// Parse `"<mods>:<vk>"` hotkey env format, falling back cleanly.
pub fn parse_hotkey_env(s: Option<&str>, default: (u8, u32)) -> (u8, u32) {
    let Some(raw) = s else { return default };
    let Some((m, k)) = raw.split_once(':') else { return default };
    match (m.trim().parse::<u8>(), k.trim().parse::<u32>()) {
        (Ok(mods), Ok(vk)) if vk != 0 => (mods & 0x07, vk),
        _ => default,
    }
}

/// Texture operation queued for the Vulkan render thread.
pub enum TexOp {
    Set(TexSet),
    Free(u64),
}

/// Latest overlay frame received from Aurora.
pub struct Frame {
    pub pixels_per_point: f32,
    pub meshes: Vec<Mesh>,
}

/// Shared state between the TCP threads, the X11 input thread, and `vkQueuePresentKHR`.
pub struct Shared {
    pub open: AtomicBool,
    pub open_cv: (Mutex<bool>, Condvar),
    pub tex_ops: Mutex<Vec<TexOp>>,
    /// CPU-side mirror of every live texture so a newly created swapchain
    /// renderer (e.g. fullscreen/borderless switch in Helldivers 2) can replay
    /// the egui font atlas and all existing images immediately.
    tex_shadow: Mutex<HashMap<u64, TexSet>>,
    pub frame: Mutex<Option<Arc<Frame>>>,
    pub size: Mutex<(u32, u32)>,
    shot_requested: AtomicBool,
    overlay_mods: AtomicU8,
    overlay_vk: AtomicU32,
    shot_mods: AtomicU8,
    shot_vk: AtomicU32,
    out_tx: Mutex<Option<mpsc::Sender<FromGame>>>,
    started: AtomicBool,
    shutdown: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        let ov = parse_hotkey_env(std::env::var("AURORA_OVERLAY_HOTKEY").ok().as_deref(), (DEFAULT_OVERLAY_MODS, DEFAULT_OVERLAY_VK));
        let sh = parse_hotkey_env(std::env::var("AURORA_SCREENSHOT_KEY").ok().as_deref(), (DEFAULT_SHOT_MODS, DEFAULT_SHOT_VK));
        Self {
            open: AtomicBool::new(false),
            open_cv: (Mutex::new(false), Condvar::new()),
            tex_ops: Mutex::new(Vec::new()),
            tex_shadow: Mutex::new(HashMap::new()),
            frame: Mutex::new(None),
            size: Mutex::new((0, 0)),
            shot_requested: AtomicBool::new(false),
            overlay_mods: AtomicU8::new(ov.0),
            overlay_vk: AtomicU32::new(ov.1),
            shot_mods: AtomicU8::new(sh.0),
            shot_vk: AtomicU32::new(sh.1),
            out_tx: Mutex::new(None),
            started: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub fn set_open_local(&self, open: bool) {
        let prev = self.open.swap(open, Ordering::SeqCst);
        if prev != open {
            if let Ok(mut g) = self.open_cv.0.lock() {
                *g = open;
                self.open_cv.1.notify_all();
            }
        }
    }

    /// Notify Aurora that the user opened or closed the overlay.
    pub fn notify_opened(&self, open: bool) {
        self.set_open_local(open);
        self.send(FromGame::Opened(open));
    }

    pub fn overlay_hotkey(&self) -> (u8, u32) {
        (self.overlay_mods.load(Ordering::Relaxed), self.overlay_vk.load(Ordering::Relaxed))
    }

    pub fn screenshot_hotkey(&self) -> (u8, u32) {
        (self.shot_mods.load(Ordering::Relaxed), self.shot_vk.load(Ordering::Relaxed))
    }

    pub fn set_hotkeys(&self, overlay_mods: u8, overlay_vk: u32, shot_mods: u8, shot_vk: u32) {
        if overlay_vk != 0 {
            self.overlay_mods.store(overlay_mods & 0x07, Ordering::Relaxed);
            self.overlay_vk.store(overlay_vk, Ordering::Relaxed);
        }
        if shot_vk != 0 {
            self.shot_mods.store(shot_mods & 0x07, Ordering::Relaxed);
            self.shot_vk.store(shot_vk, Ordering::Relaxed);
        }
    }

    pub fn request_screenshot(&self) {
        self.shot_requested.store(true, Ordering::Release);
    }

    pub fn take_screenshot_request(&self) -> bool {
        self.shot_requested.swap(false, Ordering::AcqRel)
    }

    pub fn send(&self, msg: FromGame) {
        if let Ok(g) = self.out_tx.lock() {
            if let Some(tx) = g.as_ref() {
                let _ = tx.send(msg);
            }
        }
    }

    pub fn send_input(&self, modifiers: u8, events: Vec<Input>) {
        if !events.is_empty() {
            self.send(FromGame::Input { modifiers, events });
        }
    }

    /// Stream an RGB screenshot frame cleanly under the 64 KiB wire limit.
    pub fn send_screenshot_rgb(&self, width: u32, height: u32, rgb: Vec<u8>) {
        self.send(FromGame::ScreenshotStart { width, height });
        for chunk in rgb.chunks(32 * 1024) {
            self.send(FromGame::ScreenshotChunk(chunk.to_vec()));
        }
        self.send(FromGame::ScreenshotEnd);
    }

    pub fn update_size(&self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let changed = {
            let mut g = self.size.lock().unwrap_or_else(|e| e.into_inner());
            if *g != (width, height) {
                *g = (width, height);
                true
            } else {
                false
            }
        };
        if !self.is_shutdown() && !self.started.swap(true, Ordering::SeqCst) {
            start_client();
        } else if changed {
            self.send(FromGame::Resize { width, height });
        }
    }

    pub fn drain_tex_ops(&self) -> Vec<TexOp> {
        std::mem::take(&mut *self.tex_ops.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Snapshot all live textures so a freshly created `OverlayRenderer`
    /// (after swapchain creation/recreation) gets the font atlas and images.
    pub fn snapshot_tex_ops(&self) -> Vec<TexOp> {
        self.tex_shadow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .map(TexOp::Set)
            .collect()
    }

    fn apply_shadow(&self, set: &[TexSet], free: &[u64]) {
        const MAX_SHADOW_TEXTURES: usize = 256;
        const MAX_SHADOW_BYTES: usize = 128 << 20;
        let mut map = self.tex_shadow.lock().unwrap_or_else(|e| e.into_inner());
        for id in free {
            map.remove(id);
        }
        let mut total_bytes: usize = map.values().map(|t| t.rgba.len()).sum();
        for ts in set {
            let Some(expected) = (ts.size[0] as usize)
                .checked_mul(ts.size[1] as usize)
                .and_then(|px| px.checked_mul(4))
            else {
                continue;
            };
            if ts.size[0] == 0 || ts.size[1] == 0 || ts.rgba.len() != expected {
                continue;
            }
            match ts.pos {
                None => {
                    let prev_bytes = map.get(&ts.id).map_or(0, |t| t.rgba.len());
                    let next_total = total_bytes.saturating_sub(prev_bytes).saturating_add(ts.rgba.len());
                    if (map.contains_key(&ts.id) || map.len() < MAX_SHADOW_TEXTURES) && next_total <= MAX_SHADOW_BYTES {
                        map.insert(ts.id, ts.clone());
                        total_bytes = next_total;
                    }
                }
                Some([px, py]) => {
                    if let Some(dst) = map.get_mut(&ts.id) {
                        let dw = dst.size[0] as usize;
                        let dh = dst.size[1] as usize;
                        let sw = ts.size[0] as usize;
                        let sh = ts.size[1] as usize;
                        let px = px as usize;
                        let py = py as usize;
                        let in_bounds = px.checked_add(sw).is_some_and(|x| x <= dw)
                            && py.checked_add(sh).is_some_and(|y| y <= dh)
                            && dw.checked_mul(dh).and_then(|n| n.checked_mul(4)) == Some(dst.rgba.len());
                        if in_bounds {
                            let row_bytes = sw * 4;
                            for row in 0..sh {
                                let src_off = row * row_bytes;
                                let dst_off = ((py + row) * dw + px) * 4;
                                dst.rgba[dst_off..dst_off + row_bytes].copy_from_slice(&ts.rgba[src_off..src_off + row_bytes]);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn current_frame(&self) -> Option<Arc<Frame>> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Cleanly release overlay state when the last Vulkan instance shuts down.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.set_open_local(false);
        *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.tex_ops.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.tex_shadow.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *self.out_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

static SHARED: OnceLock<Shared> = OnceLock::new();

pub fn shared() -> &'static Shared {
    SHARED.get_or_init(Shared::new)
}

/// Returns true if the Aurora Vulkan overlay env vars are present and enabled.
pub fn is_enabled() -> bool {
    if std::env::var("DISABLE_AURORA_OVERLAY").ok().as_deref() == Some("1") {
        return false;
    }
    std::env::var("AURORA_VK_OVERLAY").ok().as_deref() == Some("1")
        && std::env::var("AURORA_OVERLAY_PORT").ok().and_then(|s| s.parse::<u16>().ok()).is_some()
        && std::env::var("AURORA_OVERLAY_TOKEN").ok().is_some_and(|t| !t.is_empty())
}

fn start_client() {
    let Some(port) = std::env::var("AURORA_OVERLAY_PORT").ok().and_then(|s| s.parse::<u16>().ok()) else {
        return;
    };
    let Some(token) = std::env::var("AURORA_OVERLAY_TOKEN").ok().filter(|t| !t.is_empty()) else {
        return;
    };

    let _ = std::thread::Builder::new().name("aurora-vk-overlay-net".into()).spawn(move || {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        while !shared().is_shutdown() {
            let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                Ok(s) => s,
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(None);
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

            let mut reader = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            let mut writer = stream;

            let (width, height) = *shared().size.lock().unwrap_or_else(|e| e.into_inner());
            let (tx, rx) = mpsc::channel::<FromGame>();
            let hello = FromGame::Hello { token: token.clone(), pid: std::process::id(), width: width.max(128), height: height.max(128) };
            if crate::wire::write_message(&mut writer, &hello.encode()).is_err() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            *shared().out_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);

            let tx_join = std::thread::Builder::new().name("aurora-vk-overlay-tx".into()).spawn(move || {
                while let Ok(msg) = rx.recv() {
                    if crate::wire::write_message(&mut writer, &msg.encode()).is_err() {
                        break;
                    }
                }
            });

            while !shared().is_shutdown() {
                let Ok(body) = crate::wire::read_message(&mut reader) else {
                    break;
                };
                let Ok(msg) = ToGame::decode(&body) else {
                    break;
                };
                match msg {
                    ToGame::SetOpen(open) => {
                        shared().set_open_local(open);
                    }
                    ToGame::Config { overlay_mods, overlay_vk, shot_mods, shot_vk } => {
                        shared().set_hotkeys(overlay_mods, overlay_vk, shot_mods, shot_vk);
                    }
                    ToGame::TakeScreenshot => {
                        shared().request_screenshot();
                    }
                    ToGame::Frame { pixels_per_point, cursor: _, set, free, meshes } => {
                        if !set.is_empty() || !free.is_empty() {
                            shared().apply_shadow(&set, &free);
                            let mut ops = shared().tex_ops.lock().unwrap_or_else(|e| e.into_inner());
                            for t in set {
                                ops.push(TexOp::Set(t));
                            }
                            for id in free {
                                ops.push(TexOp::Free(id));
                            }
                        }
                        let frame = if meshes.is_empty() {
                            None
                        } else {
                            Some(Arc::new(Frame { pixels_per_point, meshes }))
                        };
                        *shared().frame.lock().unwrap_or_else(|e| e.into_inner()) = frame;
                    }
                }
            }

            // Disconnected: release grabs & stale frames before any reconnect attempt.
            shared().set_open_local(false);
            *shared().frame.lock().unwrap_or_else(|e| e.into_inner()) = None;
            *shared().out_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Ok(h) = tx_join {
                let _ = h.join();
            }
            if !shared().is_shutdown() {
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    });
}
