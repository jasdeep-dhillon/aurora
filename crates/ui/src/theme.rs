//! Theme tokens, presets and typography.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    /// Window background.
    pub bg: Color32,
    /// Top bar, status bar, side panels.
    pub chrome: Color32,
    /// Cards.
    pub surface: Color32,
    pub surface_hover: Color32,
    /// Inputs, menus, raised elements.
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    /// Second stop of the accent gradient.
    pub accent_2: Color32,
    /// Text on accent fills.
    pub on_accent: Color32,
    pub play: Color32,
    pub online: Color32,
    pub in_game: Color32,
    pub away: Color32,
    pub danger: Color32,
    pub discount: Color32,
    pub radius: u8,
}

impl Default for Theme {
    fn default() -> Self {
        Self::nevko()
    }
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Theme {
    pub fn presets() -> Vec<Theme> {
        vec![Self::nevko(), Self::midnight(), Self::aurora(), Self::classic(), Self::oled(), Self::daylight()]
    }

    /// After the NEVKO-UI Steam skin (github.com/dotFelixan/NEVKO-UI): one
    /// deep slate for the chrome, flat grey-blue controls with no borders,
    /// and Steam blue used only for what is active.
    pub fn nevko() -> Self {
        Self {
            name: "NEVKO".into(),
            dark: true,
            bg: hex(0x1A2129),
            chrome: hex(0x161D25),
            surface: hex(0x20272F),
            surface_hover: hex(0x2E3540),
            raised: hex(0x272D36),
            border: hex(0x232A33),
            text: hex(0xDFE3E6),
            text_dim: hex(0x9198A1),
            text_faint: hex(0x6B727C),
            accent: hex(0x1A9FFF),
            accent_2: hex(0x1A9FFF),
            on_accent: hex(0xFFFFFF),
            play: hex(0x4CAF50),
            online: hex(0x57B8F7),
            in_game: hex(0x4CD18A),
            away: hex(0xE8B04B),
            danger: hex(0xE64A35),
            discount: hex(0x4C6B22),
            radius: 9,
        }
    }

    /// Default: inky blue-black, electric blue to violet accent.
    pub fn midnight() -> Self {
        Self {
            name: "Midnight".into(),
            dark: true,
            bg: hex(0x0A0D12),
            chrome: hex(0x0E1218),
            surface: hex(0x131922),
            surface_hover: hex(0x1A222D),
            raised: hex(0x1C2530),
            border: hex(0x202935),
            text: hex(0xEEF2F7),
            text_dim: hex(0x9AA5B4),
            text_faint: hex(0x5F6B7A),
            accent: hex(0x4F8CFF),
            accent_2: hex(0x8B5CF6),
            on_accent: hex(0xFFFFFF),
            play: hex(0x2FB36B),
            online: hex(0x4FC3F7),
            in_game: hex(0x9CCC4B),
            away: hex(0xF2B84B),
            danger: hex(0xF0544F),
            discount: hex(0x4C6B22),
            radius: 12,
        }
    }

    /// Deep teal with a mint-to-cyan accent.
    pub fn aurora() -> Self {
        Self {
            name: "Aurora".into(),
            bg: hex(0x081112),
            chrome: hex(0x0B1718),
            surface: hex(0x0F1E20),
            surface_hover: hex(0x15292B),
            raised: hex(0x172D30),
            border: hex(0x1B3336),
            text_dim: hex(0x93AFAE),
            text_faint: hex(0x58716F),
            accent: hex(0x2DD4BF),
            accent_2: hex(0x38BDF8),
            on_accent: hex(0x04201C),
            ..Self::midnight()
        }
    }

    /// Familiar Steam-like blues.
    pub fn classic() -> Self {
        Self {
            name: "Classic".into(),
            bg: hex(0x1B2838),
            chrome: hex(0x171A21),
            surface: hex(0x213045),
            surface_hover: hex(0x2A3F5A),
            raised: hex(0x2A3B4F),
            border: hex(0x2E4058),
            text: hex(0xDCDEDF),
            text_dim: hex(0x8F98A0),
            text_faint: hex(0x5F6A75),
            accent: hex(0x66C0F4),
            accent_2: hex(0x417A9B),
            on_accent: hex(0x0E1C2A),
            play: hex(0x5C7E10),
            radius: 4,
            ..Self::midnight()
        }
    }

    /// True black for OLED screens.
    pub fn oled() -> Self {
        Self {
            name: "OLED".into(),
            bg: hex(0x000000),
            chrome: hex(0x050505),
            surface: hex(0x0E0E0F),
            surface_hover: hex(0x18181A),
            raised: hex(0x161618),
            border: hex(0x1F1F22),
            accent: hex(0xF5F5F5),
            accent_2: hex(0xA3A3A3),
            on_accent: hex(0x000000),
            ..Self::midnight()
        }
    }

    pub fn daylight() -> Self {
        Self {
            name: "Daylight".into(),
            dark: false,
            bg: hex(0xF3F5F8),
            chrome: hex(0xFFFFFF),
            surface: hex(0xFFFFFF),
            surface_hover: hex(0xEEF2F7),
            raised: hex(0xF7F9FB),
            border: hex(0xDEE3EA),
            text: hex(0x121A24),
            text_dim: hex(0x546273),
            text_faint: hex(0x8A97A6),
            accent: hex(0x2563EB),
            accent_2: hex(0x7C3AED),
            on_accent: hex(0xFFFFFF),
            play: hex(0x16A34A),
            online: hex(0x0284C7),
            in_game: hex(0x4D7C0F),
            away: hex(0xB45309),
            danger: hex(0xDC2626),
            discount: hex(0x4D7C0F),
            radius: 12,
        }
    }

    /// A saved theme with its preset's current colors. Only the accent colors
    /// and corner roundness can be edited in Settings, so those are kept; the
    /// rest comes from the preset, so fixes to a preset (such as the status
    /// colors) reach themes saved before them. Unknown names stay as saved.
    pub fn refreshed(self) -> Self {
        match Self::presets().into_iter().find(|p| p.name == self.name) {
            Some(preset) => Self { accent: self.accent, accent_2: self.accent_2, radius: self.radius, ..preset },
            None => self,
        }
    }

    pub fn corner(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }

    pub fn corner_small(&self) -> CornerRadius {
        CornerRadius::same((self.radius * 2 / 3).max(2))
    }

    /// Push these tokens into egui's global style.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.override_text_color = Some(self.text);
        v.panel_fill = self.bg;
        v.window_fill = self.raised;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = self.corner();
        v.window_shadow = egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) };
        v.menu_corner_radius = self.corner_small();
        v.extreme_bg_color = self.raised;
        v.faint_bg_color = self.surface;
        v.code_bg_color = self.raised;
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent.linear_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);
        if self.dark && cfg!(target_os = "linux") {
            // Linux: egui's dark default (2c - c²) thickens every edge pixel,
            // which reads as jagged at 1x; a gentle gamma keeps text smooth.
            // Other systems keep egui's defaults.
            v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.75);
        }

        let r = self.corner_small();
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.surface;
        w.noninteractive.weak_bg_fill = self.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text_dim);
        w.inactive.bg_fill = self.raised;
        w.inactive.weak_bg_fill = self.raised;
        w.inactive.bg_stroke = Stroke::new(1.0, self.border);
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.hovered.bg_fill = self.surface_hover;
        w.hovered.weak_bg_fill = self.surface_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, self.border);
        w.hovered.fg_stroke = Stroke::new(1.5, self.text);
        w.hovered.expansion = 0.0;
        w.active.bg_fill = self.accent.linear_multiply(0.5);
        w.active.weak_bg_fill = self.accent.linear_multiply(0.5);
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        w.active.fg_stroke = Stroke::new(2.0, self.on_accent);
        w.active.expansion = 0.0;
        w.open.weak_bg_fill = self.surface_hover;
        w.open.bg_stroke = Stroke::new(1.0, self.border);
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = r;
        }

        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 8.0);
            s.spacing.button_padding = egui::vec2(14.0, 7.0);
            s.spacing.interact_size.y = 32.0;
            s.spacing.menu_margin = egui::Margin::same(10);
            s.spacing.scroll.bar_width = 6.0;
            s.spacing.scroll.floating = true;
            s.spacing.scroll.bar_inner_margin = 2.0;
            s.interaction.selectable_labels = false;
            use egui::TextStyle::*;
            s.text_styles = [
                (Heading, FontId::new(24.0, semibold())),
                (Body, FontId::new(14.0, FontFamily::Proportional)),
                (Button, FontId::new(14.0, medium())),
                (Small, FontId::new(12.0, FontFamily::Proportional)),
                (Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

// The interface is set in heavy weights, like a desktop using "Inter Bold":
// body and medium text in Geist Bold, semibold and bold in Geist ExtraBold.
const GEIST_BOLD: &[u8] = include_bytes!("../assets/fonts/Geist-Bold.ttf");
const GEIST_EXTRABOLD: &[u8] = include_bytes!("../assets/fonts/Geist-ExtraBold.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
const PHOSPHOR: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// Text rendering: light hinting (snap stems vertically only, keep the
/// designed shapes) and a softer coverage curve than egui's dark default,
/// so edges are smoothly anti-aliased instead of crunchy at 1x.
fn text_tweak() -> egui::epaint::text::FontTweak {
    use egui::epaint::text::{FontTweak, HintingTarget, SmoothHinting};
    if !cfg!(target_os = "linux") {
        // Only Linux gets the tuned text; elsewhere egui's defaults.
        return FontTweak::default();
    }
    FontTweak {
        hinting: Some(true),
        hinting_target: HintingTarget::Smooth(SmoothHinting { light: true, symmetric_rendering: true, preserve_linear_metrics: true }),
        ..Default::default()
    }
}

/// A system font file, mapped into memory instead of copied: only the parts
/// actually drawn get loaded, and they count as shared file cache rather than
/// the app's own memory (the CJK fonts are 70+ MB together).
fn map_font(path: &std::path::Path) -> Option<&'static [u8]> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: system font files don't change while the app runs; the mapping
    // lives for the whole program, like the font itself.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(&Box::leak(Box::new(map))[..])
}

/// Install Geist (Bold and ExtraBold, Inter Bold behind it for scripts Geist lacks) plus
/// the icon font, with grayscale anti-aliasing. Call once at startup.
pub fn install_fonts(ctx: &egui::Context) {
    install_fonts_with(ctx, false);
}

/// Like [`install_fonts`], but `lcd` renders text and icons with subpixel
/// (RGB) anti-aliasing, like the rest of the desktop. Only for contexts drawn
/// by this app's patched glow painter: the in-game overlay hands its meshes
/// to another renderer and must stay grayscale.
pub fn install_fonts_with(ctx: &egui::Context, lcd: bool) {
    use egui::epaint::text::FontTweak;
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("geist-bold", GEIST_BOLD),
        ("geist-extrabold", GEIST_EXTRABOLD),
        ("inter-bold", INTER_BOLD),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(bytes).tweak(text_tweak()).lcd(lcd)));
    }
    // Icons are drawn from their outlines unhinted, exactly as the SVGs
    // would be: the auto-hinter otherwise bends them onto the pixel grid.
    let icon_tweak = if cfg!(target_os = "linux") { FontTweak { hinting: Some(false), ..Default::default() } } else { FontTweak::default() };
    let icons = egui::FontData::from_static(PHOSPHOR).tweak(icon_tweak).lcd(lcd);
    fonts.font_data.insert("phosphor".into(), Arc::new(icons));
    // System fonts for scripts Geist and Inter don't cover, so game names in Chinese,
    // Japanese, Korean, Thai, Arabic, Hindi… show instead of empty boxes.
    let mut system: Vec<String> = Vec::new();
    if let Some(dir) = std::env::var_os("WINDIR").map(|w| std::path::PathBuf::from(w).join("Fonts")) {
        for file in ["msyh.ttc", "YuGothM.ttc", "malgun.ttf", "Nirmala.ttc", "tahoma.ttf", "seguisym.ttf"] {
            if let Some(bytes) = map_font(&dir.join(file)) {
                let name = format!("system-{file}");
                fonts.font_data.insert(name.clone(), Arc::new(egui::FontData::from_static(bytes)));
                system.push(name);
            }
        }
    }
    // egui's bundled fonts stay as fallbacks for emoji and rare scripts.
    let mut fallbacks: Vec<String> = system;
    fallbacks.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    // Phosphor has only private-use glyphs and Inter none, so the icon font
    // can sit right after the text fonts. (Geist's one private-use glyph,
    // U+F8FF, is outside Phosphor's range.)
    let family = |geist: &str| {
        let mut v = vec![geist.to_string(), "inter-bold".to_string(), "phosphor".to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("geist-bold"));
    fonts.families.insert(medium(), family("geist-bold"));
    fonts.families.insert(semibold(), family("geist-extrabold"));
    fonts.families.insert(bold(), family("geist-extrabold"));
    fonts.families.entry(FontFamily::Monospace).or_default().push("phosphor".into());
    ctx.set_fonts(fonts);
}
