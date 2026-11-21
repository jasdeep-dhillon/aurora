//! `theme.toml` manifest schema, token overrides, background/font/logo config,
//! and user-configurable theme options.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use egui::Color32;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::Error;
use crate::color::{ThemeColor, parse_hex_color};
use crate::native::{NativeAppearance, NativeThemeConfig, NativeThemeOverrides};
use crate::tree::LayoutTree;

/// Validate a theme package id (`[a-z0-9_-]+`, 1..=64 chars).
pub fn validate_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(Error::InvalidId(id.to_string()));
    }
    Ok(())
}

/// Resolve a relative asset path inside `base_dir`, rejecting absolute paths
/// and `..` traversal segments.
pub fn safe_rel_path(base_dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => clean.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        return None;
    }
    Some(base_dir.join(clean))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeMeta {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub min_aurora_version: Option<String>,
}

fn default_version() -> String {
    "1.0.0".into()
}

/// How a custom background image fits the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundFit {
    #[default]
    Cover,
    Contain,
    Tile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: Option<String>,
    pub fit: BackgroundFit,
    pub opacity: f32,
    pub tint: Option<ThemeColor>,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            image: None,
            fit: BackgroundFit::Cover,
            opacity: 0.25,
            tint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundOverrides {
    pub image: Option<String>,
    pub fit: Option<BackgroundFit>,
    pub opacity: Option<f32>,
    pub tint: Option<ThemeColor>,
}

impl BackgroundOverrides {
    pub fn apply_to(&self, target: &mut BackgroundConfig) {
        if let Some(v) = &self.image {
            target.image = Some(v.clone());
        }
        if let Some(v) = self.fit {
            target.fit = v;
        }
        if let Some(v) = self.opacity {
            target.opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.tint {
            target.tint = Some(v);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeOverrides {
    pub base: Option<String>,
    pub dark: Option<bool>,
    pub bg: Option<ThemeColor>,
    pub chrome: Option<ThemeColor>,
    pub surface: Option<ThemeColor>,
    pub surface_hover: Option<ThemeColor>,
    pub raised: Option<ThemeColor>,
    pub border: Option<ThemeColor>,
    pub text: Option<ThemeColor>,
    pub text_dim: Option<ThemeColor>,
    pub text_faint: Option<ThemeColor>,
    pub accent: Option<ThemeColor>,
    pub accent_2: Option<ThemeColor>,
    pub on_accent: Option<ThemeColor>,
    pub play: Option<ThemeColor>,
    pub online: Option<ThemeColor>,
    pub in_game: Option<ThemeColor>,
    pub away: Option<ThemeColor>,
    pub danger: Option<ThemeColor>,
    pub discount: Option<ThemeColor>,
    pub radius: Option<u8>,
    pub weight_preset: Option<ui::WeightPreset>,
    pub font_scale: Option<f32>,
    pub widget_style: Option<ui::WidgetStyle>,
}

impl ThemeOverrides {
    pub fn apply_to(&self, theme: &mut ui::Theme) {
        if let Some(base_name) = &self.base {
            if let Some(preset) = ui::Theme::preset_by_name(base_name) {
                *theme = preset;
            }
        }
        if let Some(v) = self.dark {
            theme.dark = v;
        }
        if let Some(v) = self.bg {
            theme.bg = v.0;
        }
        if let Some(v) = self.chrome {
            theme.chrome = v.0;
        }
        if let Some(v) = self.surface {
            theme.surface = v.0;
        }
        if let Some(v) = self.surface_hover {
            theme.surface_hover = v.0;
        }
        if let Some(v) = self.raised {
            theme.raised = v.0;
        }
        if let Some(v) = self.border {
            theme.border = v.0;
        }
        if let Some(v) = self.text {
            theme.text = v.0;
        }
        if let Some(v) = self.text_dim {
            theme.text_dim = v.0;
        }
        if let Some(v) = self.text_faint {
            theme.text_faint = v.0;
        }
        if let Some(v) = self.accent {
            theme.accent = v.0;
        }
        if let Some(v) = self.accent_2 {
            theme.accent_2 = v.0;
        }
        if let Some(v) = self.on_accent {
            theme.on_accent = v.0;
        }
        if let Some(v) = self.play {
            theme.play = v.0;
        }
        if let Some(v) = self.online {
            theme.online = v.0;
        }
        if let Some(v) = self.in_game {
            theme.in_game = v.0;
        }
        if let Some(v) = self.away {
            theme.away = v.0;
        }
        if let Some(v) = self.danger {
            theme.danger = v.0;
        }
        if let Some(v) = self.discount {
            theme.discount = v.0;
        }
        if let Some(v) = self.radius {
            theme.radius = v;
        }
        if let Some(v) = self.weight_preset {
            theme.weight_preset = v;
        }
        if let Some(v) = self.font_scale {
            theme.font_scale = v.clamp(0.75, 1.5);
        }
        if let Some(v) = self.widget_style {
            theme.widget_style = v;
        }
    }
}

/// Partial overrides applied by a theme option (can tweak theme tokens,
/// background settings, or switch to an alternate `layout_file`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OverrideBundle {
    pub native: Option<NativeThemeOverrides>,
    pub theme: Option<ThemeOverrides>,
    pub background: Option<BackgroundOverrides>,
    pub layout_file: Option<String>,
}

impl OverrideBundle {
    pub fn apply_to(
        &self,
        theme: &mut ui::Theme,
        background: &mut BackgroundConfig,
        layout_file: &mut Option<String>,
    ) {
        if let Some(t) = &self.theme {
            t.apply_to(theme);
        }
        if let Some(b) = &self.background {
            b.apply_to(background);
        }
        if let Some(lf) = &self.layout_file {
            *layout_file = Some(lf.clone());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FontsConfig {
    pub regular: Option<String>,
    pub bold: Option<String>,
    pub mono: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogoConfig {
    pub image: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Bool,
    Select,
    Color,
    Slider,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Number(f64),
    Text(String),
}

impl OptionValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Text(s) => s.parse::<bool>().ok(),
            Self::Number(_) => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Number(n) => Some(*n as f32),
            Self::Text(s) => s.parse::<f32>().ok(),
            Self::Bool(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn to_string_value(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => {
                if n.fract().abs() < f64::EPSILON {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                }
            }
            Self::Text(s) => s.clone(),
        }
    }

    pub fn parse_for_kind(kind: OptionKind, raw: &str) -> Result<Self, Error> {
        match kind {
            OptionKind::Bool => match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Self::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Self::Bool(false)),
                _ => Err(Error::InvalidOptionValue {
                    expected: "bool",
                    value: raw.to_string(),
                }),
            },
            OptionKind::Slider => {
                raw.trim()
                    .parse::<f64>()
                    .map(Self::Number)
                    .map_err(|_| Error::InvalidOptionValue {
                        expected: "number",
                        value: raw.to_string(),
                    })
            }
            OptionKind::Color => {
                let _ = parse_hex_color(raw)?;
                Ok(Self::Text(raw.trim().to_string()))
            }
            OptionKind::Select => Ok(Self::Text(raw.to_string())),
        }
    }
}

impl Serialize for OptionValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => serializer.serialize_f64(*n),
            Self::Text(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for OptionValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OptionValueVisitor;

        impl<'de> de::Visitor<'de> for OptionValueVisitor {
            type Value = OptionValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a boolean, number, or string")
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(OptionValue::Bool(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(OptionValue::Number(v as f64))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(OptionValue::Number(v as f64))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(OptionValue::Number(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OptionValue::Text(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(OptionValue::Text(v))
            }
        }

        deserializer.deserialize_any(OptionValueVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionChoice {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub overrides: Option<OverrideBundle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeOption {
    pub kind: OptionKind,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub default: OptionValue,
    #[serde(default)]
    pub choices: Vec<OptionChoice>,
    #[serde(default)]
    pub on_overrides: Option<OverrideBundle>,
    #[serde(default)]
    pub off_overrides: Option<OverrideBundle>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub min: Option<f32>,
    #[serde(default)]
    pub max: Option<f32>,
    #[serde(default)]
    pub step: Option<f32>,
}

impl ThemeOption {
    pub fn apply(
        &self,
        value: Option<&OptionValue>,
        theme: &mut ui::Theme,
        background: &mut BackgroundConfig,
        layout_file: &mut Option<String>,
    ) {
        let effective = value.unwrap_or(&self.default);
        match self.kind {
            OptionKind::Bool => {
                if let Some(enabled) = effective.as_bool() {
                    let bundle = if enabled {
                        self.on_overrides.as_ref()
                    } else {
                        self.off_overrides.as_ref()
                    };
                    if let Some(b) = bundle {
                        b.apply_to(theme, background, layout_file);
                    }
                }
            }
            OptionKind::Select => {
                if let Some(selected) = effective.as_str() {
                    if let Some(choice) = self.choices.iter().find(|c| c.value == selected) {
                        if let Some(b) = &choice.overrides {
                            b.apply_to(theme, background, layout_file);
                        }
                    }
                }
            }
            OptionKind::Color => {
                if let (Some(target), Some(hex)) = (self.target.as_deref(), effective.as_str()) {
                    if let Ok(color) = parse_hex_color(hex) {
                        apply_color_target(target, color, theme, background);
                    }
                }
            }
            OptionKind::Slider => {
                if let (Some(target), Some(mut v)) = (self.target.as_deref(), effective.as_f32()) {
                    if let Some(min) = self.min {
                        v = v.max(min);
                    }
                    if let Some(max) = self.max {
                        v = v.min(max);
                    }
                    apply_slider_target(target, v, theme, background);
                }
            }
        }
    }
}

pub fn apply_color_target(
    target: &str,
    color: Color32,
    theme: &mut ui::Theme,
    background: &mut BackgroundConfig,
) {
    let key = target.strip_prefix("theme.").unwrap_or(target);
    match key {
        "bg" => theme.bg = color,
        "chrome" => theme.chrome = color,
        "surface" => theme.surface = color,
        "surface_hover" => theme.surface_hover = color,
        "raised" => theme.raised = color,
        "border" => theme.border = color,
        "text" => theme.text = color,
        "text_dim" => theme.text_dim = color,
        "text_faint" => theme.text_faint = color,
        "accent" => theme.accent = color,
        "accent_2" => theme.accent_2 = color,
        "on_accent" => theme.on_accent = color,
        "play" => theme.play = color,
        "online" => theme.online = color,
        "in_game" => theme.in_game = color,
        "away" => theme.away = color,
        "danger" => theme.danger = color,
        "discount" => theme.discount = color,
        "background.tint" => background.tint = Some(ThemeColor(color)),
        _ => {}
    }
}

pub fn apply_slider_target(
    target: &str,
    value: f32,
    theme: &mut ui::Theme,
    background: &mut BackgroundConfig,
) {
    let key = target.strip_prefix("theme.").unwrap_or(target);
    match key {
        "radius" => theme.radius = value.round().clamp(0.0, 255.0) as u8,
        "font_scale" => theme.font_scale = value.clamp(0.75, 1.5),
        "background.opacity" => background.opacity = value.clamp(0.0, 1.0),
        _ => {}
    }
}

/// Complete parsed `theme.toml` manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeManifest {
    pub meta: ThemeMeta,
    #[serde(default)]
    pub native: Option<NativeThemeConfig>,
    #[serde(default)]
    pub theme: ThemeOverrides,
    #[serde(default)]
    pub fonts: FontsConfig,
    #[serde(default)]
    pub background: BackgroundConfig,
    #[serde(default)]
    pub logo: LogoConfig,
    /// Optional inline declarative layout tree inside `theme.toml`.
    #[serde(default)]
    pub layout: Option<LayoutTree>,
    /// Optional path (relative to theme dir) to a layout TOML file. Defaults to
    /// `"layout.toml"` when present in the theme folder.
    #[serde(default)]
    pub layout_file: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, ThemeOption>,
}

/// Output of resolving a manifest's theme tokens, background config, and active
/// `layout_file` selection.
#[derive(Debug, Clone)]
pub struct ResolvedManifestParts {
    pub theme: ui::Theme,
    pub background: BackgroundConfig,
    pub layout_file: Option<String>,
}

impl ThemeManifest {
    pub fn parse(src: &str) -> Result<Self, Error> {
        let manifest: Self = toml::from_str(src)?;
        validate_id(&manifest.meta.id)?;
        if manifest.meta.name.trim().is_empty() {
            return Err(Error::InvalidManifest("meta.name must not be empty".into()));
        }
        Ok(manifest)
    }

    /// Resolve theme tokens, background settings, and optional `layout_file`
    /// override for the given user option values.
    pub fn resolve_tokens(
        &self,
        user_options: &BTreeMap<String, OptionValue>,
    ) -> ResolvedManifestParts {
        self.resolve_tokens_with_native(user_options, NativeAppearance::default())
    }

    /// Resolve native option selections before applying explicit token edits.
    pub fn native_config(
        &self,
        user_options: &BTreeMap<String, OptionValue>,
    ) -> Option<NativeThemeConfig> {
        let mut native = self.native?;
        for (key, option) in &self.options {
            let value = user_options.get(key).unwrap_or(&option.default);
            let bundle = match option.kind {
                OptionKind::Select => value
                    .as_str()
                    .and_then(|selected| {
                        option
                            .choices
                            .iter()
                            .find(|choice| choice.value == selected)
                    })
                    .and_then(|choice| choice.overrides.as_ref()),
                OptionKind::Bool => value.as_bool().and_then(|enabled| {
                    if enabled {
                        option.on_overrides.as_ref()
                    } else {
                        option.off_overrides.as_ref()
                    }
                }),
                _ => None,
            };
            if let Some(overrides) = bundle.and_then(|bundle| bundle.native) {
                overrides.apply_to(&mut native);
            }
        }
        Some(native)
    }

    pub fn resolve_tokens_with_native(
        &self,
        user_options: &BTreeMap<String, OptionValue>,
        appearance: NativeAppearance,
    ) -> ResolvedManifestParts {
        let mut theme = self
            .native_config(user_options)
            .map(|config| config.resolve(appearance))
            .unwrap_or_default();
        self.theme.apply_to(&mut theme);
        theme.name = self.meta.name.clone();

        let mut background = self.background.clone();
        background.opacity = background.opacity.clamp(0.0, 1.0);
        let mut layout_file = self.layout_file.clone();

        for (key, opt) in &self.options {
            opt.apply(
                user_options.get(key),
                &mut theme,
                &mut background,
                &mut layout_file,
            );
        }

        ResolvedManifestParts {
            theme,
            background,
            layout_file,
        }
    }
}
