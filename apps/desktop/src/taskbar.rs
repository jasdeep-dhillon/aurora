//! Taskbar/dock download progress (`ITaskbarList3` on Windows,
//! `com.canonical.Unity.LauncherEntry` over D-Bus on Linux).
//!
//! Shows active/paused download progress directly on Aurora's taskbar/dock
//! icon and clears it as soon as downloads finish or go idle. Other
//! platforms compile a zero-cost no-op stub while sharing the pure state
//! mapping logic so it stays unit-tested everywhere.

use model::{DownloadPhase, DownloadStatus};

const SCALE: u64 = 1000;

/// Taskbar progress state derived from current downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskbarState {
    #[default]
    None,
    Indeterminate,
    /// Quantized progress (`completed` in `0..=total`, `total == SCALE`).
    Normal { completed: u64, total: u64 },
    /// Quantized paused progress (`completed` in `0..=total`, `total == SCALE`).
    Paused { completed: u64, total: u64 },
}

/// Map the current set of downloads to the desired taskbar progress state.
pub fn state_for_downloads<'a>(downloads: impl IntoIterator<Item = &'a DownloadStatus>) -> TaskbarState {
    let mut active: Option<&DownloadStatus> = None;
    for d in downloads {
        if matches!(d.phase, DownloadPhase::Done | DownloadPhase::Failed) {
            continue;
        }
        // Prefer an unpaused active download over a paused one.
        match active {
            None => active = Some(d),
            Some(prev) if prev.paused && !d.paused => active = Some(d),
            _ => {}
        }
    }
    let Some(d) = active else {
        return TaskbarState::None;
    };
    let (done, total) = d.shown();
    if d.paused {
        let completed = if total > 0 { ((done.min(total) as u128 * SCALE as u128) / total as u128) as u64 } else { 0 };
        TaskbarState::Paused { completed, total: SCALE }
    } else if total > 0 {
        let completed = ((done.min(total) as u128 * SCALE as u128) / total as u128) as u64;
        TaskbarState::Normal { completed, total: SCALE }
    } else {
        TaskbarState::Indeterminate
    }
}

#[cfg(windows)]
mod imp {
    use super::{TaskbarState, state_for_downloads};
    use model::{AppId, DownloadStatus};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::{ITaskbarList3, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TaskbarList};
    use windows::core::IUnknown;

    pub struct Taskbar {
        hwnd: HWND,
        bar: Option<ITaskbarList3>,
        com_inited: bool,
        last: TaskbarState,
    }

    impl Taskbar {
        pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
            let hwnd = cc
                .window_handle()
                .ok()
                .and_then(|h| match h.as_raw() {
                    RawWindowHandle::Win32(w) => Some(HWND(w.hwnd.get() as *mut c_void)),
                    _ => None,
                });
            let Some(hwnd) = hwnd else {
                return Self { hwnd: HWND(std::ptr::null_mut()), bar: None, com_inited: false, last: TaskbarState::None };
            };
            // SAFETY: COM initialization and ITaskbarList3 creation on the UI thread.
            let com_inited = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let bar: Option<ITaskbarList3> = unsafe { CoCreateInstance(&TaskbarList, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
                .and_then(|b: ITaskbarList3| {
                    unsafe { b.HrInit() }?;
                    Ok(b)
                })
                .map_err(|e| tracing::debug!("Windows taskbar progress unavailable: {e}"))
                .ok();
            Self { hwnd, bar, com_inited, last: TaskbarState::None }
        }

        pub fn update(&mut self, downloads: &HashMap<AppId, DownloadStatus>) {
            let Some(bar) = &self.bar else { return };
            let next = state_for_downloads(downloads.values());
            if next == self.last {
                return;
            }
            self.last = next;
            // SAFETY: `hwnd` belongs to the main eframe window, which outlives `SteamApp`,
            // and `bar` was initialized via `HrInit` on this thread.
            unsafe {
                match next {
                    TaskbarState::None => {
                        let _ = bar.SetProgressState(self.hwnd, TBPF_NOPROGRESS);
                    }
                    TaskbarState::Indeterminate => {
                        let _ = bar.SetProgressState(self.hwnd, TBPF_INDETERMINATE);
                    }
                    TaskbarState::Normal { completed, total } => {
                        let _ = bar.SetProgressState(self.hwnd, TBPF_NORMAL);
                        let _ = bar.SetProgressValue(self.hwnd, completed, total);
                    }
                    TaskbarState::Paused { completed, total } => {
                        let _ = bar.SetProgressState(self.hwnd, TBPF_PAUSED);
                        let _ = bar.SetProgressValue(self.hwnd, completed, total);
                    }
                }
            }
        }
    }

    impl Drop for Taskbar {
        fn drop(&mut self) {
            if let Some(bar) = self.bar.take() {
                if self.last != TaskbarState::None {
                    let _ = unsafe { bar.SetProgressState(self.hwnd, TBPF_NOPROGRESS) };
                }
                drop(bar);
            }
            if self.com_inited {
                unsafe { CoUninitialize() };
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
const LAUNCHER_ENTRY_URI: &str = "application://aurora.desktop";
#[cfg(any(target_os = "linux", test))]
const LAUNCHER_ENTRY_PATH: &str = "/com/canonical/unity/launcherentry/aurora";
#[cfg(any(target_os = "linux", test))]
const LAUNCHER_ENTRY_IFACE: &str = "com.canonical.Unity.LauncherEntry";
#[cfg(any(target_os = "linux", test))]
const LAUNCHER_ENTRY_SIGNAL: &str = "Update";

/// D-Bus `com.canonical.Unity.LauncherEntry` properties for a [`TaskbarState`].
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnityProgress {
    pub progress: f64,
    pub progress_visible: bool,
}

/// Map a [`TaskbarState`] to `com.canonical.Unity.LauncherEntry` properties.
#[cfg(any(target_os = "linux", test))]
pub fn unity_properties(state: TaskbarState) -> UnityProgress {
    let fraction = |completed: u64, total: u64| -> f64 {
        if total > 0 {
            (completed.min(total) as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    match state {
        TaskbarState::None => UnityProgress { progress: 0.0, progress_visible: false },
        TaskbarState::Indeterminate => UnityProgress { progress: 0.0, progress_visible: true },
        TaskbarState::Normal { completed, total } | TaskbarState::Paused { completed, total } => {
            UnityProgress { progress: fraction(completed, total), progress_visible: true }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{
        LAUNCHER_ENTRY_IFACE, LAUNCHER_ENTRY_PATH, LAUNCHER_ENTRY_SIGNAL, LAUNCHER_ENTRY_URI,
        TaskbarState, UnityProgress, state_for_downloads, unity_properties,
    };
    use model::{AppId, DownloadStatus};
    use std::collections::HashMap;
    use std::thread::JoinHandle;
    use std::time::Duration;
    use tokio::sync::{oneshot, watch};
    use zbus::zvariant::Value;

    pub(super) fn signal_body(p: UnityProgress) -> (&'static str, HashMap<&'static str, Value<'static>>) {
        let mut props = HashMap::with_capacity(2);
        props.insert("progress", Value::F64(p.progress));
        props.insert("progress-visible", Value::Bool(p.progress_visible));
        (LAUNCHER_ENTRY_URI, props)
    }

    async fn emit(conn: &zbus::Connection, state: TaskbarState) -> zbus::Result<()> {
        let body = signal_body(unity_properties(state));
        conn.emit_signal(None::<&str>, LAUNCHER_ENTRY_PATH, LAUNCHER_ENTRY_IFACE, LAUNCHER_ENTRY_SIGNAL, &body).await
    }

    pub struct Taskbar {
        last: TaskbarState,
        tx: Option<watch::Sender<TaskbarState>>,
        shutdown: Option<oneshot::Sender<()>>,
        worker: Option<JoinHandle<()>>,
    }

    impl Taskbar {
        pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
            Self::start()
        }

        pub(super) fn start() -> Self {
            Self::start_with_address(None)
        }

        pub(super) fn start_with_address(address: Option<String>) -> Self {
            let (tx, rx) = watch::channel(TaskbarState::None);
            let (shutdown, shutdown_rx) = oneshot::channel();
            let worker = std::thread::Builder::new()
                .name("aurora-taskbar".into())
                .spawn(move || run_worker(rx, shutdown_rx, address))
                .map_err(|e| tracing::debug!("Linux taskbar progress thread failed: {e}"))
                .ok();
            Self {
                last: TaskbarState::None,
                tx: Some(tx),
                shutdown: Some(shutdown),
                worker,
            }
        }

        pub fn update(&mut self, downloads: &HashMap<AppId, DownloadStatus>) {
            let next = state_for_downloads(downloads.values());
            if next == self.last {
                return;
            }
            self.last = next;
            if let Some(tx) = &self.tx {
                let _ = tx.send(next);
            }
        }
    }

    impl Drop for Taskbar {
        fn drop(&mut self) {
            self.shutdown.take();
            self.tx.take();
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    async fn connect(address: Option<&str>) -> zbus::Result<zbus::Connection> {
        match address {
            Some(addr) => zbus::connection::Builder::address(addr)?.build().await,
            None => zbus::Connection::session().await,
        }
    }

    fn run_worker(mut rx: watch::Receiver<TaskbarState>, shutdown: oneshot::Receiver<()>, address: Option<String>) {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let conn = tokio::select! {
                res = connect(address.as_deref()) => match res {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("Linux taskbar progress unavailable: {e}");
                        return;
                    }
                },
                _ = shutdown => return,
            };
            let mut emitted = TaskbarState::None;
            let initial = *rx.borrow_and_update();
            if initial != TaskbarState::None {
                if let Err(e) = emit(&conn, initial).await {
                    tracing::debug!("Linux taskbar progress emit failed: {e}");
                } else {
                    emitted = initial;
                }
            }
            while rx.changed().await.is_ok() {
                let next = *rx.borrow_and_update();
                if next == emitted {
                    continue;
                }
                if let Err(e) = emit(&conn, next).await {
                    tracing::debug!("Linux taskbar progress emit failed: {e}");
                } else {
                    emitted = next;
                }
            }
            if emitted != TaskbarState::None {
                let _ = tokio::time::timeout(Duration::from_millis(250), emit(&conn, TaskbarState::None)).await;
            }
        });
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use model::{AppId, DownloadStatus};
    use std::collections::HashMap;

    pub struct Taskbar;

    impl Taskbar {
        pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
            Self
        }

        pub fn update(&mut self, _downloads: &HashMap<AppId, DownloadStatus>) {}
    }
}

pub use imp::Taskbar;

#[cfg(test)]
mod tests {
    use super::*;
    use model::AppId;
    use std::collections::HashMap;

    fn status(app_id: AppId, phase: DownloadPhase, downloaded: u64, total: u64, paused: bool) -> DownloadStatus {
        DownloadStatus {
            app_id,
            phase,
            downloaded,
            download_total: total,
            disk_done: 0,
            disk_total: 0,
            speed: 0,
            paused,
            error: None,
            reused: 0,
            notes: Vec::new(),
            install_dir: None,
            disk_speed: 0,
        }
    }

    #[test]
    fn idle_and_completed_downloads_clear_progress() {
        assert_eq!(state_for_downloads(std::iter::empty()), TaskbarState::None);
        let done = status(10, DownloadPhase::Done, 100, 100, false);
        let failed = status(20, DownloadPhase::Failed, 50, 100, false);
        assert_eq!(state_for_downloads([&done, &failed]), TaskbarState::None);
    }

    #[test]
    fn active_paused_and_indeterminate_states() {
        let prep = status(10, DownloadPhase::Preparing, 0, 0, false);
        assert_eq!(state_for_downloads([&prep]), TaskbarState::Indeterminate);

        let dl = status(10, DownloadPhase::Downloading, 250, 1000, false);
        assert_eq!(state_for_downloads([&dl]), TaskbarState::Normal { completed: 250, total: SCALE });

        let paused = status(10, DownloadPhase::Downloading, 500, 1000, true);
        assert_eq!(state_for_downloads([&paused]), TaskbarState::Paused { completed: 500, total: SCALE });

        // Active unpaused download takes precedence over a paused one.
        assert_eq!(state_for_downloads([&paused, &dl]), TaskbarState::Normal { completed: 250, total: SCALE });
    }

    #[test]
    fn unity_properties_maps_all_states() {
        assert_eq!(
            unity_properties(TaskbarState::None),
            UnityProgress { progress: 0.0, progress_visible: false }
        );
        assert_eq!(
            unity_properties(TaskbarState::Indeterminate),
            UnityProgress { progress: 0.0, progress_visible: true }
        );
        assert_eq!(
            unity_properties(TaskbarState::Normal { completed: 250, total: 1000 }),
            UnityProgress { progress: 0.25, progress_visible: true }
        );
        assert_eq!(
            unity_properties(TaskbarState::Paused { completed: 500, total: 1000 }),
            UnityProgress { progress: 0.5, progress_visible: true }
        );
        // Zero total or completed > total clamps safely to 0.0..=1.0.
        assert_eq!(
            unity_properties(TaskbarState::Normal { completed: 10, total: 0 }),
            UnityProgress { progress: 0.0, progress_visible: true }
        );
        assert_eq!(
            unity_properties(TaskbarState::Normal { completed: 1500, total: 1000 }),
            UnityProgress { progress: 1.0, progress_visible: true }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_dbus_signal_payload_matches_unity_spec() {
        let body = imp::signal_body(unity_properties(TaskbarState::Normal { completed: 400, total: 1000 }));
        let msg = zbus::message::Message::signal(
            LAUNCHER_ENTRY_PATH,
            LAUNCHER_ENTRY_IFACE,
            LAUNCHER_ENTRY_SIGNAL,
        )
        .unwrap()
        .build(&body)
        .unwrap();

        let header = msg.header();
        assert_eq!(header.path().unwrap().as_str(), "/com/canonical/unity/launcherentry/aurora");
        assert_eq!(header.interface().unwrap().as_str(), "com.canonical.Unity.LauncherEntry");
        assert_eq!(header.member().unwrap().as_str(), "Update");

        let (uri, props): (String, HashMap<String, zbus::zvariant::OwnedValue>) = msg.body().deserialize().unwrap();
        assert_eq!(uri, "application://aurora.desktop");
        assert_eq!(f64::try_from(props["progress"].clone()).unwrap(), 0.4);
        assert!(bool::try_from(props["progress-visible"].clone()).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_taskbar_noops_gracefully_without_dbus() {
        let mut bar = imp::Taskbar::start_with_address(Some("unix:path=/nonexistent/aurora-no-dbus.sock".into()));
        let mut downloads = HashMap::new();
        downloads.insert(440, status(440, DownloadPhase::Downloading, 300, 1000, false));
        bar.update(&downloads);
        downloads.insert(440, status(440, DownloadPhase::Done, 1000, 1000, false));
        bar.update(&downloads);
        drop(bar);
    }
}
