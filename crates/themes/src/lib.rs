//! # Aurora Declarative Theme Engine
//!
//! Manages theme packages (`<config_dir>/themes/<id>/`), CSS-like box-model
//! declarative UI trees (`layout.toml`), per-theme user option overrides
//! (`<config_dir>/theme-data/<id>.json`), safe `.zip`/folder installation,
//! and optional sandboxed `theme.rhai` visual hooks.

pub mod color;
pub mod install;
pub mod manifest;
pub mod script;
pub mod tree;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::Color32;

pub use color::{ColorRef, ColorToken, ThemeColor, color_to_hex, parse_hex_color};
pub use install::{install_folder, install_zip, uninstall_theme};
pub use manifest::{
    BackgroundConfig, BackgroundFit, BackgroundOverrides, FontsConfig, LogoConfig, OptionChoice,
    OptionKind, OptionValue, OverrideBundle, ResolvedManifestParts, ThemeManifest, ThemeMeta,
    ThemeOption, ThemeOverrides, safe_rel_path, validate_id,
};
pub use script::{DrawCmd, ThemeScript, paint_draw_cmds, theme_to_rhai_map};
pub use tree::{
    AlignMode, BorderEdge, Direction, Edges, FontWeight, GradientSpec, IndicatorStyle, Justify,
    LayoutTree, MenuItemSpec, Node, NodeStyle, PanelSide, SizeVal, WinCtrlVariant,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid theme id '{0}' (must match [a-z0-9_-]+)")]
    InvalidId(String),
    #[error("invalid color '{0}'")]
    InvalidColor(String),
    #[error("invalid theme manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid layout tree: {0}")]
    InvalidLayout(String),
    #[error("missing theme.toml in package")]
    MissingManifest,
    #[error("unsafe path in theme package: '{0}'")]
    UnsafePath(String),
    #[error("theme package limit exceeded: {0}")]
    PackageLimit(String),
    #[error("theme '{0}' not found")]
    NotFound(String),
    #[error("option '{key}' not found on theme '{theme_id}'")]
    OptionNotFound { theme_id: String, key: String },
    #[error("invalid option value '{value}' (expected {expected})")]
    InvalidOptionValue { expected: &'static str, value: String },
}

/// A discovered, valid theme package on disk.
#[derive(Debug, Clone)]
pub struct InstalledTheme {
    pub dir: PathBuf,
    pub manifest: ThemeManifest,
    pub has_layout: bool,
    pub has_script: bool,
}

/// The active theme fully resolved with user option overrides, declarative
/// layout tree, custom font bytes, background/logo assets, and optional Rhai script.
#[derive(Debug)]
pub struct ResolvedTheme {
    pub meta: ThemeMeta,
    pub dir: PathBuf,
    pub theme: ui::Theme,
    pub layout: LayoutTree,
    pub custom_fonts: Option<ui::CustomFonts>,
    pub background_path: Option<PathBuf>,
    pub background_fit: BackgroundFit,
    pub background_opacity: f32,
    pub background_tint: Option<Color32>,
    pub logo_path: Option<PathBuf>,
    pub script: Option<ThemeScript>,
    pub script_error: Option<String>,
}

/// Discovers, activates, resolves, and hot-reloads declarative themes.
#[derive(Debug)]
pub struct ThemeManager {
    pub themes_dir: PathBuf,
    pub data_dir: PathBuf,
    pub installed: Vec<InstalledTheme>,
    pub broken: Vec<(PathBuf, String)>,
    pub active_id: Option<String>,
    pub resolved: Option<ResolvedTheme>,
    active_layout_file: Option<String>,
    last_mtime: Option<SystemTime>,
}

impl ThemeManager {
    pub fn new(themes_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            themes_dir,
            data_dir,
            installed: Vec::new(),
            broken: Vec::new(),
            active_id: None,
            resolved: None,
            active_layout_file: None,
            last_mtime: None,
        }
    }

    /// Scan `themes_dir` for installed themes, recording any broken folders in
    /// `self.broken`. If an active theme is set, re-resolves it.
    pub fn scan(&mut self) {
        self.installed.clear();
        self.broken.clear();

        let Ok(entries) = fs::read_dir(&self.themes_dir) else {
            self.resolved = None;
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            let manifest_path = path.join("theme.toml");
            if !manifest_path.is_file() {
                continue;
            }
            match fs::read_to_string(&manifest_path)
                .map_err(Error::from)
                .and_then(|s| ThemeManifest::parse(&s))
            {
                Ok(manifest) => {
                    let layout_rel = manifest.layout_file.as_deref().unwrap_or("layout.toml");
                    let has_layout_file = safe_rel_path(&path, layout_rel).is_some_and(|p| p.is_file());
                    let has_inline_layout = manifest.layout.as_ref().is_some_and(|l| !l.is_empty());
                    let has_script = path.join("theme.rhai").is_file();
                    self.installed.push(InstalledTheme {
                        dir: path,
                        manifest,
                        has_layout: has_layout_file || has_inline_layout,
                        has_script,
                    });
                }
                Err(e) => {
                    self.broken.push((path, e.to_string()));
                }
            }
        }

        self.installed.sort_by(|a, b| {
            a.manifest
                .meta
                .name
                .to_lowercase()
                .cmp(&b.manifest.meta.name.to_lowercase())
        });

        if let Some(id) = self.active_id.clone() {
            if self.resolve_active(&id).is_err() {
                self.active_id = None;
                self.resolved = None;
                self.active_layout_file = None;
                self.last_mtime = None;
            }
        }
    }

    /// Activate a theme by id, or pass `None` to return to Aurora's default UI.
    pub fn activate(&mut self, id: Option<&str>) -> Result<(), Error> {
        match id {
            None => {
                self.active_id = None;
                self.resolved = None;
                self.active_layout_file = None;
                self.last_mtime = None;
                Ok(())
            }
            Some(theme_id) => {
                self.resolve_active(theme_id)?;
                self.active_id = Some(theme_id.to_string());
                Ok(())
            }
        }
    }

    pub fn active_manifest(&self) -> Option<&ThemeManifest> {
        let id = self.active_id.as_deref()?;
        self.installed
            .iter()
            .find(|t| t.manifest.meta.id == id)
            .map(|t| &t.manifest)
    }

    pub fn get(&self, id: &str) -> Option<&InstalledTheme> {
        self.installed.iter().find(|t| t.manifest.meta.id == id)
    }

    pub fn load_option_values(&self, theme_id: &str) -> BTreeMap<String, OptionValue> {
        if validate_id(theme_id).is_err() {
            return BTreeMap::new();
        }
        let path = self.data_dir.join(format!("{theme_id}.json"));
        let Ok(raw) = fs::read_to_string(&path) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save_option_values(
        &self,
        theme_id: &str,
        values: &BTreeMap<String, OptionValue>,
    ) -> Result<(), Error> {
        validate_id(theme_id)?;
        fs::create_dir_all(&self.data_dir)?;
        let path = self.data_dir.join(format!("{theme_id}.json"));
        let content = serde_json::to_string_pretty(values)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn set_option(
        &mut self,
        theme_id: &str,
        key: &str,
        value: OptionValue,
    ) -> Result<(), Error> {
        let installed = self
            .get(theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?;
        if !installed.manifest.options.contains_key(key) {
            return Err(Error::OptionNotFound {
                theme_id: theme_id.to_string(),
                key: key.to_string(),
            });
        }
        let mut values = self.load_option_values(theme_id);
        values.insert(key.to_string(), value);
        self.save_option_values(theme_id, &values)?;
        if self.active_id.as_deref() == Some(theme_id) {
            self.resolve_active(theme_id)?;
        }
        Ok(())
    }

    pub fn set_option_str(
        &mut self,
        theme_id: &str,
        key: &str,
        raw_value: &str,
    ) -> Result<(), Error> {
        let kind = self
            .get(theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?
            .manifest
            .options
            .get(key)
            .ok_or_else(|| Error::OptionNotFound {
                theme_id: theme_id.to_string(),
                key: key.to_string(),
            })?
            .kind;
        let parsed = OptionValue::parse_for_kind(kind, raw_value)?;
        self.set_option(theme_id, key, parsed)
    }

    pub fn reset_options(&mut self, theme_id: &str) -> Result<(), Error> {
        validate_id(theme_id)?;
        let path = self.data_dir.join(format!("{theme_id}.json"));
        if path.is_file() {
            fs::remove_file(path)?;
        }
        if self.active_id.as_deref() == Some(theme_id) {
            self.resolve_active(theme_id)?;
        }
        Ok(())
    }

    /// Check whether the active theme's `theme.toml`, active layout file, or
    /// `theme.rhai` changed on disk; if so, reload and return `true`.
    pub fn check_hot_reload(&mut self) -> bool {
        if self.active_id.is_none() {
            return false;
        }
        let Some(dir) = self.resolved.as_ref().map(|r| r.dir.clone()) else {
            return false;
        };
        let current = theme_dir_mtime(&dir, self.active_layout_file.as_deref());
        if current.is_some() && current != self.last_mtime {
            self.scan();
            return true;
        }
        false
    }

    fn resolve_active(&mut self, theme_id: &str) -> Result<(), Error> {
        let installed = self
            .installed
            .iter()
            .find(|t| t.manifest.meta.id == theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?
            .clone();

        let user_options = self.load_option_values(theme_id);
        let parts = installed.manifest.resolve_tokens(&user_options);

        // Load declarative LayoutTree:
        // 1. Explicit `layout_file` (from manifest or option override), else
        // 2. `layout.toml` in theme folder if present, else
        // 3. inline `[layout]` in `theme.toml`, else empty `LayoutTree::default()`.
        let layout_rel = parts.layout_file.as_deref().unwrap_or("layout.toml");
        let layout = if let Some(path) =
            safe_rel_path(&installed.dir, layout_rel).filter(|p| p.is_file())
        {
            let src = fs::read_to_string(&path)?;
            LayoutTree::parse(&src).map_err(Error::InvalidLayout)?
        } else if let Some(inline) = installed.manifest.layout.clone() {
            inline
        } else {
            LayoutTree::default()
        };

        let custom_fonts = load_custom_fonts(&installed.dir, &installed.manifest.fonts);
        let background_path = parts
            .background
            .image
            .as_deref()
            .and_then(|rel| safe_rel_path(&installed.dir, rel))
            .filter(|p| p.is_file());
        let logo_path = installed
            .manifest
            .logo
            .image
            .as_deref()
            .and_then(|rel| safe_rel_path(&installed.dir, rel))
            .filter(|p| p.is_file());

        let script_path = installed.dir.join("theme.rhai");
        let (script, script_error) = if script_path.is_file() {
            match ThemeScript::from_file(&script_path) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };

        self.active_layout_file = Some(layout_rel.to_string());
        self.last_mtime = theme_dir_mtime(&installed.dir, Some(layout_rel));
        self.resolved = Some(ResolvedTheme {
            meta: installed.manifest.meta,
            dir: installed.dir,
            theme: parts.theme,
            layout,
            custom_fonts,
            background_path,
            background_fit: parts.background.fit,
            background_opacity: parts.background.opacity,
            background_tint: parts.background.tint.map(|c| c.0),
            logo_path,
            script,
            script_error,
        });
        Ok(())
    }
}

fn load_custom_fonts(dir: &Path, cfg: &FontsConfig) -> Option<ui::CustomFonts> {
    let load = |rel: &Option<String>| -> Option<Vec<u8>> {
        let p = safe_rel_path(dir, rel.as_deref()?)?;
        fs::read(p).ok()
    };
    let regular = load(&cfg.regular);
    let bold = load(&cfg.bold);
    let mono = load(&cfg.mono);
    if regular.is_none() && bold.is_none() && mono.is_none() {
        None
    } else {
        Some(ui::CustomFonts { regular, bold, mono })
    }
}

fn theme_dir_mtime(dir: &Path, layout_file: Option<&str>) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = None;
    let mut check = |p: PathBuf| {
        if let Ok(meta) = fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                latest = Some(match latest {
                    Some(prev) => prev.max(mtime),
                    None => mtime,
                });
            }
        }
    };
    check(dir.join("theme.toml"));
    check(dir.join("theme.rhai"));
    if let Some(lf) = layout_file.and_then(|rel| safe_rel_path(dir, rel)) {
        check(lf);
    } else {
        check(dir.join("layout.toml"));
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aurora-declarative-themes-test-{}-{}-{n}",
            std::process::id(),
            label
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn color_ref_edges_and_size_val_deserialize() {
        #[derive(serde::Deserialize)]
        struct BoxSample {
            c_hex: ColorRef,
            c_tok: ColorRef,
            c_alpha: ColorRef,
            c_trans: ColorRef,
            e_scalar: Edges,
            e_pair: Edges,
            e_quad: Edges,
            s_px: SizeVal,
            s_fill: SizeVal,
            s_auto: SizeVal,
        }

        let src = r##"
c_hex = "#1A9FFF"
c_tok = "$accent"
c_alpha = "$accent@0.2"
c_trans = "transparent"
e_scalar = 8
e_pair = [6, 12]
e_quad = [1, 2, 3, 4]
s_px = 240
s_fill = "fill"
s_auto = "auto"
"##;
        let sample: BoxSample = toml::from_str(src).expect("parse BoxSample");
        let theme = ui::Theme::nevko();

        assert_eq!(sample.c_hex.resolve(&theme), Color32::from_rgb(0x1A, 0x9F, 0xFF));
        assert_eq!(sample.c_tok.resolve(&theme), theme.accent);
        assert_eq!(sample.c_alpha.resolve(&theme), theme.accent.gamma_multiply(0.2));
        assert_eq!(sample.c_trans.resolve(&theme), Color32::TRANSPARENT);

        assert_eq!(sample.e_scalar, Edges::all(8.0));
        assert_eq!(sample.e_pair, Edges::symmetric(6.0, 12.0));
        assert_eq!(sample.e_quad, Edges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 });
        assert_eq!(sample.e_quad.to_egui_margin(), egui::Margin { left: 4, right: 2, top: 1, bottom: 3 });

        assert_eq!(sample.s_px, SizeVal::Px(240.0));
        assert_eq!(sample.s_fill, SizeVal::Fill);
        assert_eq!(sample.s_auto, SizeVal::Auto);
    }

    #[test]
    fn layout_tree_parses_two_row_steam_shell_and_library() {
        let layout_toml = r##"
[[shell]]
type = "panel"
side = "top"
size = 66
drag_window = true
style = { bg = "$chrome", border_bottom = { width = 1, color = "$border" }, padding = [0, 8] }

  [[shell.children]]
  type = "col"
  style = { gap = 0 }

    [[shell.children.children]]
    type = "row"
    justify = "space_between"
    align = "center"
    style = { height = 28 }

      [[shell.children.children.children]]
      type = "row"
      align = "center"
      style = { gap = 4 }

        [[shell.children.children.children.children]]
        type = "logo"
        size = 16

        [[shell.children.children.children.children]]
        type = "menu"
        label = "Steam"
        items = [
          { label = "Settings", action = "nav:settings" },
          "separator",
          { label = "Exit", action = "quit" },
        ]

      [[shell.children.children.children]]
      type = "row"
      align = "center"
      style = { gap = 6 }

        [[shell.children.children.children.children]]
        type = "notifications"
        size = 24
        highlight_unread_bg = "$play"

        [[shell.children.children.children.children]]
        type = "account"
        show_avatar = true
        show_name = true
        show_wallet = true
        avatar_size = 20

        [[shell.children.children.children.children]]
        type = "window_controls"
        variant = "windows"
        height = 24

    [[shell.children.children]]
    type = "row"
    align = "center"
    style = { height = 38, gap = 8 }

      [[shell.children.children.children]]
      type = "back_forward"
      size = 26

      [[shell.children.children.children]]
      type = "nav_tabs"
      direction = "horizontal"
      show_icons = false
      uppercase = true
      profile_as_username = true
      indicator = "underline"
      indicator_height = 2.5
      tab_gap = 16

[[shell]]
type = "panel"
side = "bottom"
size = 32
style = { bg = "$chrome", border_top = { width = 1, color = "$border" }, padding = [0, 12] }

  [[shell.children]]
  type = "row"
  justify = "space_between"
  align = "center"

    [[shell.children.children]]
    type = "button"
    label = "Add a Game"
    icon = "plus"
    action = "add_non_steam"

    [[shell.children.children]]
    type = "downloads_bar"
    width = 280

    [[shell.children.children]]
    type = "friends_button"
    size = 24

[[shell]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[shell.children]]
  type = "page_content"

[[library]]
type = "panel"
side = "left"
size = 272
resizable = true
style = { bg = "$chrome", border_right = { width = 1, color = "$border" }, padding = [8, 8] }

  [[library.children]]
  type = "col"
  style = { gap = 6 }

    [[library.children.children]]
    type = "library_filters"
    variant = "compact_steam"

    [[library.children.children]]
    type = "library_search"
    placeholder = "Search by name"
    show_filter_button = true

    [[library.children.children]]
    type = "library_list"
    row_height = 24
    icon_size = 18
    group_headers = true
    show_counts = true

[[library]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[library.children]]
  type = "library_home_or_detail"

    [[library.children.children]]
    type = "scroll"
    direction = "vertical"

      [[library.children.children.children]]
      type = "whats_new_shelf"
      title = "What's New"
      card_width = 240
      card_height = 135
      card_radius = 0
      date_above = true
      max_items = 5

      [[library.children.children.children]]
      type = "recent_games_shelf"
      title = "Recent Games"
      featured_first = true
      featured_width = 360
      capsule_width = 156
      capsule_height = 234
      card_radius = 0

      [[library.children.children.children]]
      type = "all_games_grid"
      title = "All Games"
      show_count = true
      show_sort = true
      card_aspect = "2:3"
      card_radius = 0
"##;

        let tree = LayoutTree::parse(layout_toml).expect("parse 2-row steam layout");
        assert!(!tree.is_empty());
        assert_eq!(tree.shell.len(), 3);
        assert_eq!(tree.library.len(), 2);
        match &tree.shell[0] {
            Node::Panel { side, size, drag_window, children, .. } => {
                assert_eq!(*side, PanelSide::Top);
                assert_eq!(*size, Some(66.0));
                assert!(*drag_window);
                assert_eq!(children.len(), 1);
            }
            other => panic!("expected top Panel, got {other:?}"),
        }
    }

    #[test]
    fn layout_tree_parses_left_sidebar_shell() {
        let layout_toml = r##"
[[shell]]
type = "panel"
side = "left"
size = 216
style = { bg = "$chrome", border_right = { width = 1, color = "$border" }, padding = [12, 10] }

  [[shell.children]]
  type = "col"
  style = { gap = 8 }

    [[shell.children.children]]
    type = "row"
    align = "center"
    style = { gap = 8 }

      [[shell.children.children.children]]
      type = "logo"
      size = 22

      [[shell.children.children.children]]
      type = "text"
      value = "Aurora"
      style = { font_size = 16, font_weight = "bold", color = "$text" }

    [[shell.children.children]]
    type = "divider"
    orientation = "horizontal"
    color = "$border"
    thickness = 1

    [[shell.children.children]]
    type = "nav_tabs"
    direction = "vertical"
    show_icons = true
    indicator = "left_bar"
    indicator_height = 3

    [[shell.children.children]]
    type = "spacer"
    flex = true

    [[shell.children.children]]
    type = "account"
    show_avatar = true
    show_name = true
    show_wallet = true

[[shell]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[shell.children]]
  type = "page_content"
"##;
        let tree = LayoutTree::parse(layout_toml).expect("parse left sidebar layout");
        assert_eq!(tree.shell.len(), 2);
        assert!(tree.library.is_empty());
        match &tree.shell[0] {
            Node::Panel { side, size, .. } => {
                assert_eq!(*side, PanelSide::Left);
                assert_eq!(*size, Some(216.0));
            }
            other => panic!("expected left Panel, got {other:?}"),
        }
    }

    #[test]
    fn install_zip_flat_nested_traversal_and_invalid_layout() {
        let root = temp_dir("zip");
        let themes_dir = root.join("themes");
        let valid_manifest = br##"
[meta]
id = "steam-decl"
name = "Steam Declarative"
version = "1.0.0"

[theme]
base = "Midnight"
accent = "#1A9FFF"
weight_preset = "balanced"
font_scale = 0.96
"##;
        let valid_layout = br#"
[[shell]]
type = "panel"
side = "center"
"#;

        // 1. Flat zip
        let flat = root.join("flat.zip");
        make_zip(&flat, &[("theme.toml", valid_manifest), ("layout.toml", valid_layout)]);
        let meta = install_zip(&flat, &themes_dir).expect("install flat zip");
        assert_eq!(meta.id, "steam-decl");
        assert!(themes_dir.join("steam-decl/layout.toml").is_file());

        // 2. GitHub-style nested zip
        let nested_manifest = br#"
[meta]
id = "nested-decl"
name = "Nested Declarative"
"#;
        let nested = root.join("nested.zip");
        make_zip(
            &nested,
            &[
                ("repo-main/theme.toml", nested_manifest),
                ("repo-main/layout.toml", valid_layout),
            ],
        );
        let meta2 = install_zip(&nested, &themes_dir).expect("install nested zip");
        assert_eq!(meta2.id, "nested-decl");

        // 3. Path traversal rejection
        let evil = root.join("evil.zip");
        make_zip(
            &evil,
            &[
                ("theme.toml", valid_manifest),
                ("../escape.txt", b"nope"),
            ],
        );
        assert!(matches!(install_zip(&evil, &themes_dir), Err(Error::UnsafePath(_))));

        // 4. Invalid layout.toml rejection (must not install)
        let bad_layout = root.join("bad-layout.zip");
        make_zip(
            &bad_layout,
            &[
                ("theme.toml", b"[meta]\nid = \"bad-layout\"\nname = \"Bad\"\n"),
                ("layout.toml", b"[[shell]]\ntype = \"nonexistent_widget\"\n"),
            ],
        );
        assert!(matches!(install_zip(&bad_layout, &themes_dir), Err(Error::InvalidLayout(_))));
        assert!(!themes_dir.join("bad-layout").exists());

        // 5. ThemeManager resolves active theme + option layout_file switching
        let data_dir = root.join("theme-data");
        let mut mgr = ThemeManager::new(themes_dir.clone(), data_dir);
        mgr.scan();
        assert!(mgr.broken.is_empty());
        mgr.activate(Some("steam-decl")).expect("activate steam-decl");
        let res = mgr.resolved.as_ref().expect("resolved");
        assert_eq!(res.theme.weight_preset, ui::WeightPreset::Balanced);
        assert!((res.theme.font_scale - 0.96).abs() < 1e-5);
        assert_eq!(res.layout.shell.len(), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rhai_script_sandbox_and_draw_cmds() {
        let src = r##"
fn paint_background(w, h, time, theme) {
    [
        #{ type: "gradient", x: 0, y: 0, w: w, h: h, from: theme.accent, to: "#00000000", horizontal: false },
        #{ type: "rect_stroke", x: 2, y: 2, w: w - 4, h: h - 4, radius: 2, width: 1, color: "#FFFFFF40" },
    ]
}
"##;
        let mut script = ThemeScript::compile(src).expect("compile");
        let theme = ui::Theme::nevko();
        let cmds = script.paint_background(800.0, 600.0, 0.0, &theme);
        assert_eq!(cmds.len(), 2);
        assert!(script.error().is_none());

        let mut loop_script =
            ThemeScript::compile("fn paint_background(w, h, t, th) { loop {} }").unwrap();
        let out = loop_script.paint_background(100.0, 100.0, 0.0, &theme);
        assert!(out.is_empty());
        assert!(loop_script.is_disabled());
    }
}
