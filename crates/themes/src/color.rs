//! Friendly hex colors and `$token[@alpha]` color references for declarative themes.

use egui::Color32;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

use crate::Error;

/// Parse a hex color string (`#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA`, with
/// or without a leading `#`) into an `egui::Color32`.
pub fn parse_hex_color(raw: &str) -> Result<Color32, Error> {
    let s = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    let parse_nibble = |b: u8| -> Result<u8, Error> {
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => 10 + (b - b'a'),
            b'A'..=b'F' => 10 + (b - b'A'),
            _ => return Err(Error::InvalidColor(raw.to_string())),
        };
        Ok(v)
    };
    let bytes = s.as_bytes();
    let (r, g, b, a) = match bytes.len() {
        3 => {
            let r = parse_nibble(bytes[0])?;
            let g = parse_nibble(bytes[1])?;
            let b = parse_nibble(bytes[2])?;
            (r * 17, g * 17, b * 17, 255)
        }
        4 => {
            let r = parse_nibble(bytes[0])?;
            let g = parse_nibble(bytes[1])?;
            let b = parse_nibble(bytes[2])?;
            let a = parse_nibble(bytes[3])?;
            (r * 17, g * 17, b * 17, a * 17)
        }
        6 => {
            let r = (parse_nibble(bytes[0])? << 4) | parse_nibble(bytes[1])?;
            let g = (parse_nibble(bytes[2])? << 4) | parse_nibble(bytes[3])?;
            let b = (parse_nibble(bytes[4])? << 4) | parse_nibble(bytes[5])?;
            (r, g, b, 255)
        }
        8 => {
            let r = (parse_nibble(bytes[0])? << 4) | parse_nibble(bytes[1])?;
            let g = (parse_nibble(bytes[2])? << 4) | parse_nibble(bytes[3])?;
            let b = (parse_nibble(bytes[4])? << 4) | parse_nibble(bytes[5])?;
            let a = (parse_nibble(bytes[6])? << 4) | parse_nibble(bytes[7])?;
            (r, g, b, a)
        }
        _ => return Err(Error::InvalidColor(raw.to_string())),
    };
    Ok(if a == 255 {
        Color32::from_rgb(r, g, b)
    } else {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    })
}

/// Format a `Color32` as `#RRGGBB` (when opaque) or `#RRGGBBAA`.
pub fn color_to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

/// A `Color32` wrapper that deserializes from either a hex string (`"#1A9FFF"`,
/// `"#1A9FFFCC"`) or an egui `[r, g, b]` / `[r, g, b, a]` array, and serializes
/// as a hex string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColor(pub Color32);

impl From<Color32> for ThemeColor {
    fn from(c: Color32) -> Self {
        Self(c)
    }
}

impl From<ThemeColor> for Color32 {
    fn from(c: ThemeColor) -> Self {
        c.0
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&color_to_hex(self.0))
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorVisitor;

        impl<'de> de::Visitor<'de> for ColorVisitor {
            type Value = ThemeColor;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a hex color string like \"#1A9FFF\" or an [r, g, b, a] array")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                parse_hex_color(v).map(ThemeColor).map_err(de::Error::custom)
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let r: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let g: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let b: u8 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let a: Option<u8> = seq.next_element()?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(5, &self));
                }
                let c = match a {
                    Some(a) if a < 255 => Color32::from_rgba_unmultiplied(r, g, b, a),
                    _ => Color32::from_rgb(r, g, b),
                };
                Ok(ThemeColor(c))
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

/// Named color token on `ui::Theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorToken {
    Bg,
    Chrome,
    Surface,
    SurfaceHover,
    Raised,
    Border,
    Text,
    TextDim,
    TextFaint,
    Accent,
    Accent2,
    OnAccent,
    Play,
    Online,
    InGame,
    Away,
    Danger,
    Discount,
}

impl ColorToken {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "bg" => Some(Self::Bg),
            "chrome" => Some(Self::Chrome),
            "surface" => Some(Self::Surface),
            "surface_hover" => Some(Self::SurfaceHover),
            "raised" => Some(Self::Raised),
            "border" => Some(Self::Border),
            "text" => Some(Self::Text),
            "text_dim" => Some(Self::TextDim),
            "text_faint" => Some(Self::TextFaint),
            "accent" => Some(Self::Accent),
            "accent_2" => Some(Self::Accent2),
            "on_accent" => Some(Self::OnAccent),
            "play" => Some(Self::Play),
            "online" => Some(Self::Online),
            "in_game" => Some(Self::InGame),
            "away" => Some(Self::Away),
            "danger" => Some(Self::Danger),
            "discount" => Some(Self::Discount),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bg => "bg",
            Self::Chrome => "chrome",
            Self::Surface => "surface",
            Self::SurfaceHover => "surface_hover",
            Self::Raised => "raised",
            Self::Border => "border",
            Self::Text => "text",
            Self::TextDim => "text_dim",
            Self::TextFaint => "text_faint",
            Self::Accent => "accent",
            Self::Accent2 => "accent_2",
            Self::OnAccent => "on_accent",
            Self::Play => "play",
            Self::Online => "online",
            Self::InGame => "in_game",
            Self::Away => "away",
            Self::Danger => "danger",
            Self::Discount => "discount",
        }
    }

    pub fn get(self, t: &ui::Theme) -> Color32 {
        match self {
            Self::Bg => t.bg,
            Self::Chrome => t.chrome,
            Self::Surface => t.surface,
            Self::SurfaceHover => t.surface_hover,
            Self::Raised => t.raised,
            Self::Border => t.border,
            Self::Text => t.text,
            Self::TextDim => t.text_dim,
            Self::TextFaint => t.text_faint,
            Self::Accent => t.accent,
            Self::Accent2 => t.accent_2,
            Self::OnAccent => t.on_accent,
            Self::Play => t.play,
            Self::Online => t.online,
            Self::InGame => t.in_game,
            Self::Away => t.away,
            Self::Danger => t.danger,
            Self::Discount => t.discount,
        }
    }
}

/// A CSS-like color reference used in declarative layout trees.
///
/// Accepts:
/// - `"transparent"`
/// - Hex strings (`"#1A9FFF"`, `"#1A9FFFCC"`, `"#FFF"`)
/// - Theme token references (`"$chrome"`, `"$accent"`) with an optional alpha
///   multiplier suffix (`"$accent@0.2"`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorRef {
    Transparent,
    Literal(Color32),
    Token { token: ColorToken, alpha: Option<f32> },
}

impl ColorRef {
    /// Parse a color reference string.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let s = raw.trim();
        if s.eq_ignore_ascii_case("transparent") {
            return Ok(Self::Transparent);
        }
        if let Some(rest) = s.strip_prefix('$') {
            let (name, alpha) = match rest.split_once('@') {
                Some((n, a)) => {
                    let parsed: f32 = a
                        .trim()
                        .parse()
                        .map_err(|_| Error::InvalidColor(raw.to_string()))?;
                    if !parsed.is_finite() {
                        return Err(Error::InvalidColor(raw.to_string()));
                    }
                    (n.trim(), Some(parsed.clamp(0.0, 1.0)))
                }
                None => (rest.trim(), None),
            };
            let token = ColorToken::parse(name).ok_or_else(|| Error::InvalidColor(raw.to_string()))?;
            return Ok(Self::Token { token, alpha });
        }
        parse_hex_color(s).map(Self::Literal)
    }

    /// Resolve this color reference against the active [`ui::Theme`].
    pub fn resolve(&self, t: &ui::Theme) -> Color32 {
        match *self {
            Self::Transparent => Color32::TRANSPARENT,
            Self::Literal(c) => c,
            Self::Token { token, alpha } => {
                let c = token.get(t);
                match alpha {
                    Some(a) => c.gamma_multiply(a.clamp(0.0, 1.0)),
                    None => c,
                }
            }
        }
    }
}

impl Serialize for ColorRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Transparent => serializer.serialize_str("transparent"),
            Self::Literal(c) => serializer.serialize_str(&color_to_hex(c)),
            Self::Token { token, alpha: None } => {
                serializer.serialize_str(&format!("${}", token.as_str()))
            }
            Self::Token { token, alpha: Some(a) } => {
                serializer.serialize_str(&format!("${}@{a}", token.as_str()))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ColorRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorRefVisitor;

        impl<'de> de::Visitor<'de> for ColorRefVisitor {
            type Value = ColorRef;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a hex color, \"transparent\", or a \"$token[@alpha]\" reference")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                ColorRef::parse(v).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(ColorRefVisitor)
    }
}
