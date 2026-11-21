//! Generic CSS-like box-model & declarative UI component tree (`layout.toml`).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::color::ColorRef;

/// Box-model edge insets (padding / margin).
///
/// Deserializes from:
/// - a single number `v` -> all four edges = `v`
/// - a 2-element array `[vertical, horizontal]`
/// - a 4-element array `[top, right, bottom, left]`
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const ZERO: Self = Self { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };

    pub const fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }

    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self {
        Self { top: vertical, right: horizontal, bottom: vertical, left: horizontal }
    }

    pub fn to_egui_margin(&self) -> egui::Margin {
        let clamp_i8 = |v: f32| v.round().clamp(i8::MIN as f32, i8::MAX as f32) as i8;
        egui::Margin {
            left: clamp_i8(self.left),
            right: clamp_i8(self.right),
            top: clamp_i8(self.top),
            bottom: clamp_i8(self.bottom),
        }
    }
}

impl Serialize for Edges {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.top, self.right, self.bottom, self.left].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Edges {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EdgesVisitor;

        impl<'de> de::Visitor<'de> for EdgesVisitor {
            type Value = Edges;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a number, [vertical, horizontal], or [top, right, bottom, left]")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut vals: Vec<f32> = Vec::with_capacity(4);
                while let Some(v) = seq.next_element::<f32>()? {
                    vals.push(v);
                }
                match vals.as_slice() {
                    [all] => Ok(Edges::all(*all)),
                    [v, h] => Ok(Edges::symmetric(*v, *h)),
                    [top, right, bottom, left] => {
                        Ok(Edges { top: *top, right: *right, bottom: *bottom, left: *left })
                    }
                    other => Err(de::Error::invalid_length(other.len(), &self)),
                }
            }
        }

        deserializer.deserialize_any(EdgesVisitor)
    }
}

/// Dimension value: explicit pixels, `"fill"`, or `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SizeVal {
    Px(f32),
    Fill,
    #[default]
    Auto,
}

impl Serialize for SizeVal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Px(v) => serializer.serialize_f32(v),
            Self::Fill => serializer.serialize_str("fill"),
            Self::Auto => serializer.serialize_str("auto"),
        }
    }
}

impl<'de> Deserialize<'de> for SizeVal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SizeValVisitor;

        impl<'de> de::Visitor<'de> for SizeValVisitor {
            type Value = SizeVal;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a pixel number, \"fill\", or \"auto\"")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "fill" => Ok(SizeVal::Fill),
                    "auto" => Ok(SizeVal::Auto),
                    _ => Err(de::Error::custom(format!(
                        "invalid size '{v}' (expected number, \"fill\", or \"auto\")"
                    ))),
                }
            }
        }

        deserializer.deserialize_any(SizeValVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignMode {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontWeight {
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelSide {
    Top,
    Bottom,
    Left,
    Right,
    #[default]
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorStyle {
    None,
    Underline,
    #[default]
    Pill,
    LeftBar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WinCtrlVariant {
    #[default]
    Windows,
    Macos,
    Minimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientSpec {
    pub from: ColorRef,
    pub to: ColorRef,
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BorderEdge {
    #[serde(default = "default_one")]
    pub width: f32,
    pub color: ColorRef,
}

/// Generic CSS-like box-model and visual style attached to any [`Node`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeStyle {
    pub width: Option<SizeVal>,
    pub height: Option<SizeVal>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub padding: Option<Edges>,
    pub margin: Option<Edges>,
    pub gap: Option<f32>,
    pub bg: Option<ColorRef>,
    pub bg_gradient: Option<GradientSpec>,
    pub hover_bg: Option<ColorRef>,
    pub active_bg: Option<ColorRef>,
    pub border: Option<BorderEdge>,
    pub border_top: Option<BorderEdge>,
    pub border_bottom: Option<BorderEdge>,
    pub border_left: Option<BorderEdge>,
    pub border_right: Option<BorderEdge>,
    pub radius: Option<u8>,
    pub color: Option<ColorRef>,
    pub hover_color: Option<ColorRef>,
    pub active_color: Option<ColorRef>,
    pub font_size: Option<f32>,
    pub font_weight: Option<FontWeight>,
    pub uppercase: Option<bool>,
    pub align: Option<AlignMode>,
}

/// An entry in a declarative [`Node::Menu`] dropdown.
///
/// Accepts either the string `"separator"` or a table
/// `{ label = "...", icon = "...", action = "..." }`.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItemSpec {
    Separator,
    Item {
        label: String,
        icon: Option<String>,
        action: String,
    },
}

impl Serialize for MenuItemSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct ItemHelper<'a> {
            label: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            icon: Option<&'a str>,
            action: &'a str,
        }
        match self {
            Self::Separator => serializer.serialize_str("separator"),
            Self::Item { label, icon, action } => ItemHelper {
                label,
                icon: icon.as_deref(),
                action,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MenuItemSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct ItemHelper {
            label: String,
            #[serde(default)]
            icon: Option<String>,
            action: String,
        }

        struct MenuItemVisitor;

        impl<'de> de::Visitor<'de> for MenuItemVisitor {
            type Value = MenuItemSpec;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "\"separator\" or a table {{ label, icon?, action }}")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.eq_ignore_ascii_case("separator") {
                    Ok(MenuItemSpec::Separator)
                } else {
                    Err(de::Error::custom(format!(
                        "unexpected menu item string '{v}' (expected \"separator\")"
                    )))
                }
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let h = ItemHelper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(MenuItemSpec::Item {
                    label: h.label,
                    icon: h.icon,
                    action: h.action,
                })
            }
        }

        deserializer.deserialize_any(MenuItemVisitor)
    }
}

/// Ordering of the themed library sidebar. Existing themes keep installation groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryGrouping {
    #[default]
    Installed,
    LastPlayed,
}

fn default_true() -> bool {
    true
}
fn default_one() -> f32 {
    1.0
}
fn default_logo_size() -> f32 {
    20.0
}
fn default_indicator_height() -> f32 {
    2.5
}
fn default_tab_gap() -> f32 {
    12.0
}
fn default_icon_btn_size() -> f32 {
    28.0
}
fn default_win_ctrl_height() -> f32 {
    28.0
}
fn default_avatar_size() -> f32 {
    22.0
}
fn default_friends_drawer_width() -> f32 {
    280.0
}
fn default_filters_variant() -> String {
    "default".into()
}
fn default_list_row_height() -> f32 {
    26.0
}
fn default_list_icon_size() -> f32 {
    18.0
}
fn default_whats_new_w() -> f32 {
    240.0
}
fn default_whats_new_h() -> f32 {
    135.0
}
fn default_whats_new_max() -> usize {
    5
}
fn default_gap() -> f32 {
    12.0
}
fn default_featured_w() -> f32 {
    360.0
}
fn default_capsule_w() -> f32 {
    156.0
}
fn default_capsule_h() -> f32 {
    234.0
}
fn default_card_aspect() -> String {
    "2:3".into()
}

/// A declarative UI node in Aurora's shell or library layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    // ── Containers ───────────────────────────────────────────────────────
    Panel {
        side: PanelSide,
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        resizable: bool,
        #[serde(default)]
        drag_window: bool,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Row {
        #[serde(default)]
        justify: Justify,
        #[serde(default)]
        align: AlignMode,
        #[serde(default)]
        wrap: bool,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Col {
        #[serde(default)]
        align: AlignMode,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Box {
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Scroll {
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Spacer {
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        flex: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    Divider {
        #[serde(default)]
        orientation: Direction,
        #[serde(default)]
        color: Option<ColorRef>,
        #[serde(default = "default_one")]
        thickness: f32,
        #[serde(default)]
        style: NodeStyle,
    },

    // ── Shell Components ─────────────────────────────────────────────────
    Logo {
        #[serde(default = "default_logo_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    Text {
        value: String,
        #[serde(default)]
        style: NodeStyle,
    },
    Button {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        action: String,
        #[serde(default)]
        style: NodeStyle,
    },
    Menu {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        items: Vec<MenuItemSpec>,
        #[serde(default)]
        style: NodeStyle,
    },
    NavTabs {
        #[serde(default)]
        direction: Direction,
        #[serde(default = "default_true")]
        show_icons: bool,
        #[serde(default)]
        uppercase: bool,
        #[serde(default)]
        profile_as_username: bool,
        #[serde(default)]
        indicator: IndicatorStyle,
        #[serde(default = "default_indicator_height")]
        indicator_height: f32,
        #[serde(default = "default_tab_gap")]
        tab_gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    BackForward {
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    WindowControls {
        #[serde(default)]
        variant: WinCtrlVariant,
        #[serde(default = "default_win_ctrl_height")]
        height: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    Account {
        #[serde(default = "default_true")]
        show_avatar: bool,
        #[serde(default = "default_true")]
        show_name: bool,
        #[serde(default)]
        show_wallet: bool,
        #[serde(default = "default_true")]
        show_caret: bool,
        #[serde(default = "default_avatar_size")]
        avatar_size: f32,
        /// Optional image rounding, independent of the account button frame.
        #[serde(default)]
        avatar_radius: Option<u8>,
        /// A primary-click action; absent preserves the account/status menu.
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        style: NodeStyle,
    },
    Wallet {
        #[serde(default)]
        style: NodeStyle,
    },
    Notifications {
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        highlight_unread_bg: Option<ColorRef>,
        #[serde(default)]
        style: NodeStyle,
    },
    FriendsButton {
        #[serde(default)]
        icon: Option<String>,
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    DownloadsBar {
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        style: NodeStyle,
    },
    FriendsDrawer {
        #[serde(default = "default_friends_drawer_width")]
        width: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    PageContent {
        #[serde(default)]
        style: NodeStyle,
    },

    // ── Library Components ───────────────────────────────────────────────
    LibrarySearch {
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        show_filter_button: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryFilters {
        #[serde(default = "default_filters_variant")]
        variant: String,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryList {
        #[serde(default)]
        group_by: LibraryGrouping,
        #[serde(default = "default_list_row_height")]
        row_height: f32,
        #[serde(default = "default_list_icon_size")]
        icon_size: f32,
        #[serde(default = "default_true")]
        group_headers: bool,
        #[serde(default = "default_true")]
        show_counts: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    WhatsNewShelf {
        #[serde(default)]
        title: Option<String>,
        #[serde(default = "default_whats_new_w")]
        card_width: f32,
        #[serde(default = "default_whats_new_h")]
        card_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default)]
        date_above: bool,
        #[serde(default = "default_whats_new_max")]
        max_items: usize,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    RecentGamesShelf {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        featured_first: bool,
        #[serde(default = "default_featured_w")]
        featured_width: f32,
        #[serde(default = "default_capsule_w")]
        capsule_width: f32,
        #[serde(default = "default_capsule_h")]
        capsule_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    AllGamesGrid {
        #[serde(default)]
        title: Option<String>,
        #[serde(default = "default_true")]
        show_count: bool,
        #[serde(default = "default_true")]
        show_sort: bool,
        #[serde(default = "default_card_aspect")]
        card_aspect: String,
        #[serde(default = "default_capsule_w")]
        card_width: f32,
        #[serde(default = "default_capsule_h")]
        card_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryHomeOrDetail {
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
}

/// Top-level declarative layout tree for a theme (`layout.toml` or inline `[layout]`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutTree {
    pub shell: Vec<Node>,
    pub library: Vec<Node>,
}

impl LayoutTree {
    /// Parse a `layout.toml` document into a [`LayoutTree`].
    pub fn parse(src: &str) -> Result<Self, String> {
        toml::from_str(src).map_err(|e| e.to_string())
    }

    /// True if neither `shell` nor `library` defines any declarative nodes.
    pub fn is_empty(&self) -> bool {
        self.shell.is_empty() && self.library.is_empty()
    }
}
