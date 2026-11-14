//! Steam's own logo and icon, read at runtime from the Steam client the
//! user has installed. Nothing of Valve's is bundled with the app; without
//! a Steam install the app keeps its own drawn logo and icon.

use std::path::PathBuf;

/// Steam's white monochrome logo (the one its tray icon uses).
fn mono_logo() -> Option<PathBuf> {
    let p = local::find_steam_root()?.join("public").join("steam_tray_mono.png");
    p.is_file().then_some(p)
}

/// Steam's colored app icon, largest first.
fn app_icon_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates = Vec::new();
    for size in ["256x256", "128x128", "64x64", "48x48"] {
        if let Some(h) = &home {
            candidates.push(h.join(".local/share/icons/hicolor").join(size).join("apps/steam.png"));
        }
        candidates.push(PathBuf::from("/usr/share/icons/hicolor").join(size).join("apps/steam.png"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Use Steam's logo everywhere the app shows its logo.
pub fn install_logo() {
    if let Some(p) = mono_logo() {
        ui::brand::set_logo(format!("file://{}", p.display()));
    }
}

/// Window icon: Steam's if installed, else the app's own.
pub fn window_icon(fallback_size: u32) -> egui::IconData {
    let steam = app_icon_file().and_then(|p| image::open(p).ok()).map(|img| img.into_rgba8());
    match steam {
        Some(img) => egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() },
        None => egui::IconData { rgba: ui::brand::icon_rgba(fallback_size), width: fallback_size, height: fallback_size },
    }
}
