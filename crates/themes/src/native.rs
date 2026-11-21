//! Native desktop preferences translated into egui theme tokens.
//!
//! Adwaita palettes use libadwaita's published CSS colors. Desktop settings are
//! read in a background worker; GTK is not initialized in the egui process.

use egui::Color32;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub use crate::adwaita::{NativeLibraryColors, NativeLibraryPalette};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NativeAppearance {
    pub dark: Option<bool>,
    pub accent: Option<Color32>,
    pub high_contrast: bool,
    pub library: Option<NativeLibraryPalette>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeProvider {
    #[default]
    Adwaita,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeColorScheme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAccent {
    #[default]
    System,
    Blue,
    Teal,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Slate,
}

impl NativeAccent {
    pub fn color(self) -> Option<Color32> {
        let rgb = match self {
            Self::System => return None,
            Self::Blue => 0x3584e4,
            Self::Teal => 0x2190a4,
            Self::Green => 0x3a944a,
            Self::Yellow => 0xc88800,
            Self::Orange => 0xed5b00,
            Self::Red => 0xe62d42,
            Self::Pink => 0xd56199,
            Self::Purple => 0x9141ac,
            Self::Slate => 0x6f8396,
        };
        Some(Color32::from_rgb(
            (rgb >> 16) as u8,
            (rgb >> 8) as u8,
            rgb as u8,
        ))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeThemeConfig {
    pub provider: NativeProvider,
    pub color_scheme: NativeColorScheme,
    pub accent: NativeAccent,
}

/// Option choices can change one native preference without resetting another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeThemeOverrides {
    pub color_scheme: Option<NativeColorScheme>,
    pub accent: Option<NativeAccent>,
}

impl NativeThemeOverrides {
    pub fn apply_to(self, config: &mut NativeThemeConfig) {
        if let Some(value) = self.color_scheme {
            config.color_scheme = value;
        }
        if let Some(value) = self.accent {
            config.accent = value;
        }
    }
}

impl NativeThemeConfig {
    pub fn resolve(self, appearance: NativeAppearance) -> ui::Theme {
        let dark = match self.color_scheme {
            NativeColorScheme::System => appearance.dark.unwrap_or(false),
            NativeColorScheme::Dark => true,
            NativeColorScheme::Light => false,
        };
        let mut theme = ui::Theme::adwaita(dark);
        if let Some(library) = appearance.library {
            let colors = if dark { library.dark } else { library.light };
            colors.apply_to(&mut theme);
        }
        let native_index = match self.accent {
            NativeAccent::System => None,
            NativeAccent::Blue => Some(0),
            NativeAccent::Teal => Some(1),
            NativeAccent::Green => Some(2),
            NativeAccent::Yellow => Some(3),
            NativeAccent::Orange => Some(4),
            NativeAccent::Red => Some(5),
            NativeAccent::Pink => Some(6),
            NativeAccent::Purple => Some(7),
            NativeAccent::Slate => Some(8),
        };
        let accent = appearance
            .library
            .and_then(|library| native_index.map(|index| library.accents[index][0]))
            .or_else(|| self.accent.color())
            .or_else(|| {
                (self.accent == NativeAccent::System)
                    .then_some(appearance.accent)
                    .flatten()
            })
            .unwrap_or_else(|| NativeAccent::Blue.color().unwrap());
        theme.accent = accent;
        theme.accent_2 = accent;
        theme.on_accent = Color32::WHITE;
        if let Some(library) = appearance.library {
            let colors = if dark { library.dark } else { library.light };
            let accent_text = library
                .accents
                .iter()
                .find(|colors| colors[0] == accent)
                .map(|colors| colors[if dark { 2 } else { 1 }])
                .unwrap_or(colors.accent_text);
            theme.native_metrics = Some(ui::native_style::NativeControlMetrics {
                min_height: library.button_min_height,
                padding: library.button_padding,
                view: colors.view,
                accent,
                accent_text,
                danger: colors.danger,
                danger_fill: colors.danger_fill,
            });
        }
        if appearance.high_contrast {
            theme.text = if dark { Color32::WHITE } else { Color32::BLACK };
            theme.text_dim = theme.text;
            theme.text_faint = theme.text;
            theme.border = theme.text;
        }
        theme
    }
}

/// Polls GNOME's read-only GSettings values away from the rendering thread.
/// Missing schemas/commands retain the previous successful values. Other
/// platforms keep the manifest's portable Adwaita fallback.
pub struct NativeAppearanceWatcher {
    latest: Arc<Mutex<Option<NativeAppearance>>>,
    stop: Arc<AtomicBool>,
    artwork: Arc<Mutex<Option<Arc<ui::native_style::NativeControlArtwork>>>>,
}

impl Default for NativeAppearanceWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeAppearanceWatcher {
    pub fn new() -> Self {
        let watcher = Self {
            latest: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            artwork: Arc::new(Mutex::new(None)),
        };
        #[cfg(target_os = "linux")]
        {
            let latest = Arc::clone(&watcher.latest);
            let stop = Arc::clone(&watcher.stop);
            let artwork = Arc::clone(&watcher.artwork);
            let _ = std::thread::Builder::new()
                .name("aurora-native-theme".into())
                .spawn(move || {
                    let mut current = NativeAppearance::default();
                    let mut desktop = NativeAppearance::default();
                    let mut previous_preferences = None;
                    while !stop.load(Ordering::Relaxed) {
                        if let Some(raw) =
                            read_setting("org.gnome.desktop.interface", "color-scheme")
                        {
                            if let Some(dark) = parse_color_scheme(&raw) {
                                desktop.dark = Some(dark);
                            }
                        }
                        if let Some(raw) =
                            read_setting("org.gnome.desktop.interface", "accent-color")
                        {
                            if let Some(accent) = parse_accent(&raw) {
                                desktop.accent = Some(accent);
                            }
                        }
                        if let Some(raw) =
                            read_setting("org.gnome.desktop.a11y.interface", "high-contrast")
                        {
                            match raw.as_str() {
                                "true" => desktop.high_contrast = true,
                                "false" => desktop.high_contrast = false,
                                _ => {}
                            }
                        }
                        let preferences = (desktop.dark, desktop.accent, desktop.high_contrast);
                        if previous_preferences != Some(preferences) {
                            previous_preferences = Some(preferences);
                            current.dark = desktop.dark;
                            current.accent = desktop.accent;
                            current.high_contrast = desktop.high_contrast;
                            match query_native_helper() {
                                Ok(native) => {
                                    tracing::info!(gtk = ?native.palette.gtk_version, libadwaita = ?native.palette.adwaita_version, "loaded native Adwaita styling");
                                    current.dark = Some(native.dark);
                                    current.high_contrast = native.high_contrast;
                                    current.accent = Some(native.accent);
                                    current.library = Some(native.palette);
                                    if let Ok(mut cached) = artwork.lock() {
                                        *cached = native.artwork.map(Arc::new);
                                    }
                                }
                                Err(error) => tracing::debug!(%error, "native Adwaita unavailable; using portable styling"),
                            }
                        }
                        if let Ok(mut value) = latest.lock() {
                            *value = Some(current);
                        }
                        // Short sleeps let dropping a watcher stop idle work promptly.
                        for _ in 0..20 {
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                });
        }
        watcher
    }

    pub fn latest(&self) -> Option<NativeAppearance> {
        self.latest.lock().ok().and_then(|value| *value)
    }

    pub fn latest_artwork(&self) -> Option<Arc<ui::native_style::NativeControlArtwork>> {
        self.artwork.lock().ok().and_then(|value| value.clone())
    }
}

#[cfg(target_os = "linux")]
fn query_native_helper() -> Result<crate::adwaita::NativeLibrarySnapshot, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    let started = std::time::Instant::now();
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(executable)
        .arg("--adwaita-helper")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let output = child.stdout.take().ok_or("missing helper output")?;
    let (sender, receiver) = mpsc::sync_channel(1);
    // Drain concurrently: the artwork payload is larger than an OS pipe buffer.
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let result = receiver.recv_timeout(std::time::Duration::from_secs(2));
    let bytes = match result {
        Ok(Ok(bytes)) => bytes,
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("native helper output failed or timed out".into());
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(2) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("native helper failed or timed out".into());
            }
        }
    };
    if !status.success() {
        return Err("native helper failed".into());
    }
    crate::adwaita::parse_helper_output(&bytes)
}

impl Drop for NativeAppearanceWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
fn read_setting(schema: &str, key: &str) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new("gsettings")
        .args(["get", schema, key])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut raw = String::new();
                child
                    .stdout
                    .take()?
                    .take(4096)
                    .read_to_string(&mut raw)
                    .ok()?;
                return Some(raw.trim().to_owned());
            }
            Ok(None) if started.elapsed() < Duration::from_millis(250) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_color_scheme(raw: &str) -> Option<bool> {
    match raw.trim().trim_matches('\'') {
        "prefer-dark" => Some(true),
        "prefer-light" | "default" => Some(false),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_accent(raw: &str) -> Option<Color32> {
    let accent: NativeAccent = serde_json::from_value(serde_json::Value::String(
        raw.trim().trim_matches('\'').into(),
    ))
    .ok()?;
    accent.color()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsettings_values_reject_unknown_values_instead_of_inventing_preferences() {
        assert_eq!(parse_color_scheme("'prefer-light'"), Some(false));
        assert_eq!(parse_color_scheme("'prefer-dark'"), Some(true));
        assert_eq!(parse_color_scheme("'default'"), Some(false));
        assert_eq!(parse_color_scheme("'future-scheme'"), None);
        assert_eq!(
            parse_accent("'purple'"),
            Some(Color32::from_rgb(0x91, 0x41, 0xac))
        );
        assert_eq!(parse_accent("'unknown'"), None);
    }
}
