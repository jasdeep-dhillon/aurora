//! Windows taskbar download progress (`ITaskbarList3`).
//!
//! Shows active/paused download progress directly on Aurora's Windows taskbar
//! button and clears it as soon as downloads finish or go idle. Non-Windows
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

#[cfg(not(windows))]
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
}
