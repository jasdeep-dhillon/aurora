//! Loopback TCP client for Aurora's in-game overlay protocol.
//! Connects to `127.0.0.1:<AURORA_OVERLAY_PORT>` with `AURORA_OVERLAY_TOKEN`
//! once the first game swapchain presents.

use crate::wire::{FromGame, Input, Mesh, TexSet, ToGame};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::Duration;

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
    pub frame: Mutex<Option<Arc<Frame>>>,
    pub size: Mutex<(u32, u32)>,
    out_tx: Mutex<Option<mpsc::Sender<FromGame>>>,
    started: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Self {
            open: AtomicBool::new(false),
            open_cv: (Mutex::new(false), Condvar::new()),
            tex_ops: Mutex::new(Vec::new()),
            frame: Mutex::new(None),
            size: Mutex::new((0, 0)),
            out_tx: Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
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

    /// Notify Aurora that the user opened or closed the overlay (Shift+Tab / Esc).
    pub fn notify_opened(&self, open: bool) {
        self.set_open_local(open);
        self.send(FromGame::Opened(open));
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
        if !self.started.swap(true, Ordering::SeqCst) {
            start_client(width, height);
        } else if changed {
            self.send(FromGame::Resize { width, height });
        }
    }

    pub fn drain_tex_ops(&self) -> Vec<TexOp> {
        std::mem::take(&mut *self.tex_ops.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn current_frame(&self) -> Option<Arc<Frame>> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).clone()
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

fn start_client(width: u32, height: u32) {
    let Some(port) = std::env::var("AURORA_OVERLAY_PORT").ok().and_then(|s| s.parse::<u16>().ok()) else {
        return;
    };
    let Some(token) = std::env::var("AURORA_OVERLAY_TOKEN").ok().filter(|t| !t.is_empty()) else {
        return;
    };

    let _ = std::thread::Builder::new().name("aurora-vk-overlay-net".into()).spawn(move || {
        let addr = format!("127.0.0.1:{port}");
        let stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(_) => return,
        };
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

        let mut reader = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut writer = stream;

        let (tx, rx) = mpsc::channel::<FromGame>();
        let hello = FromGame::Hello { token, pid: std::process::id(), width, height };
        if crate::wire::write_message(&mut writer, &hello.encode()).is_err() {
            return;
        }
        *shared().out_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);

        // Writer thread
        let _ = std::thread::Builder::new().name("aurora-vk-overlay-tx".into()).spawn(move || {
            while let Ok(msg) = rx.recv() {
                if crate::wire::write_message(&mut writer, &msg.encode()).is_err() {
                    break;
                }
            }
        });

        // Reader loop
        while let Ok(body) = crate::wire::read_message(&mut reader) {
            let Ok(msg) = ToGame::decode(&body) else {
                break;
            };
            match msg {
                ToGame::SetOpen(open) => {
                    shared().set_open_local(open);
                }
                ToGame::Frame { pixels_per_point, cursor: _, set, free, meshes } => {
                    if !set.is_empty() || !free.is_empty() {
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

        // Disconnected: close overlay and release grabs.
        shared().set_open_local(false);
        *shared().frame.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *shared().out_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    });
}
