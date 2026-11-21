//! Optional sandboxed `theme.rhai` visual hooks.
//!
//! Coordinates in [`DrawCmd`] are in **logical pixels relative to `rect.min`**.
//! Widths, heights, radii, and stroke widths are in logical pixels.

use std::path::Path;

use egui::{
    Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
    Vec2, epaint::Vertex,
};
use rhai::{AST, Array, Dynamic, Engine, Map, Scope};

use crate::color::{color_to_hex, parse_hex_color};

/// Maximum Rhai operations allowed per hook invocation.
pub const MAX_OPERATIONS: u64 = 50_000;
/// Maximum Rhai call stack depth.
pub const MAX_CALL_LEVELS: usize = 16;
/// Maximum string length in Rhai.
pub const MAX_STRING_SIZE: usize = 8192;
/// Maximum array length in Rhai.
pub const MAX_ARRAY_SIZE: usize = 512;
/// Maximum map entries in Rhai.
pub const MAX_MAP_SIZE: usize = 256;

/// Drawing command emitted by a `theme.rhai` visual hook.
///
/// All positions (`x`, `y`, `cx`, `cy`, `x1`, `y1`, `x2`, `y2`) are in logical
/// pixels relative to the target `rect.min` passed to [`paint_draw_cmds`].
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCmd {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: Color32,
    },
    RectStroke {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        width: f32,
        color: Color32,
    },
    Gradient {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        from: Color32,
        to: Color32,
        horizontal: bool,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        color: Color32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Color32,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: Color32,
    },
}

/// Paint a slice of [`DrawCmd`] into `painter` clipped to `rect`.
pub fn paint_draw_cmds(painter: &Painter, rect: Rect, cmds: &[DrawCmd]) {
    if cmds.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let clip = painter.clip_rect().intersect(rect);
    let p = painter.with_clip_rect(clip);
    let origin = rect.min;

    for cmd in cmds {
        match cmd {
            DrawCmd::Rect { x, y, w, h, radius, color } => {
                let r = Rect::from_min_size(origin + Vec2::new(*x, *y), Vec2::new(*w, *h));
                let cr = CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8);
                p.rect_filled(r, cr, *color);
            }
            DrawCmd::RectStroke { x, y, w, h, radius, width, color } => {
                let r = Rect::from_min_size(origin + Vec2::new(*x, *y), Vec2::new(*w, *h));
                let cr = CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8);
                p.rect_stroke(r, cr, Stroke::new(width.max(0.0), *color), StrokeKind::Inside);
            }
            DrawCmd::Gradient { x, y, w, h, from, to, horizontal } => {
                let r = Rect::from_min_size(origin + Vec2::new(*x, *y), Vec2::new(*w, *h));
                let (tl, tr, br, bl) = if *horizontal {
                    (*from, *to, *to, *from)
                } else {
                    (*from, *from, *to, *to)
                };
                let mut mesh = Mesh::default();
                let uv = Pos2::ZERO;
                mesh.vertices = vec![
                    Vertex { pos: r.left_top(), uv, color: tl },
                    Vertex { pos: r.right_top(), uv, color: tr },
                    Vertex { pos: r.right_bottom(), uv, color: br },
                    Vertex { pos: r.left_bottom(), uv, color: bl },
                ];
                mesh.indices = vec![0, 1, 2, 0, 2, 3];
                p.add(Shape::mesh(mesh));
            }
            DrawCmd::Circle { cx, cy, r, color } => {
                p.circle_filled(origin + Vec2::new(*cx, *cy), r.max(0.0), *color);
            }
            DrawCmd::Line { x1, y1, x2, y2, width, color } => {
                p.line_segment(
                    [origin + Vec2::new(*x1, *y1), origin + Vec2::new(*x2, *y2)],
                    Stroke::new(width.max(0.0), *color),
                );
            }
            DrawCmd::Text { x, y, text, size, color } => {
                p.text(
                    origin + Vec2::new(*x, *y),
                    Align2::LEFT_TOP,
                    text,
                    FontId::proportional(size.clamp(6.0, 128.0)),
                    *color,
                );
            }
        }
    }
}

/// A compiled, sandboxed `theme.rhai` script.
pub struct ThemeScript {
    engine: Engine,
    ast: AST,
    has_background: bool,
    has_chrome: bool,
    has_card_overlay: bool,
    error: Option<String>,
}

impl std::fmt::Debug for ThemeScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeScript")
            .field("has_background", &self.has_background)
            .field("has_chrome", &self.has_chrome)
            .field("has_card_overlay", &self.has_card_overlay)
            .field("error", &self.error)
            .finish()
    }
}

impl ThemeScript {
    /// Compile a `theme.rhai` source string in a strictly sandboxed Rhai engine.
    pub fn compile(source: &str) -> Result<Self, String> {
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(MAX_CALL_LEVELS);
        engine.set_max_string_size(MAX_STRING_SIZE);
        engine.set_max_array_size(MAX_ARRAY_SIZE);
        engine.set_max_map_size(MAX_MAP_SIZE);
        engine.on_print(|_| {});
        engine.on_debug(|_, _, _| {});

        let ast = engine.compile(source).map_err(|e| format!("compile error: {e}"))?;

        let has_background = has_fn(&ast, "paint_background", 4);
        let has_chrome = has_fn(&ast, "paint_chrome", 4);
        let has_card_overlay = has_fn(&ast, "paint_card_overlay", 5);

        Ok(Self {
            engine,
            ast,
            has_background,
            has_chrome,
            has_card_overlay,
            error: None,
        })
    }

    /// Load and compile a `theme.rhai` file from disk.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
        Self::compile(&src)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_disabled(&self) -> bool {
        self.error.is_some()
    }

    pub fn has_background(&self) -> bool {
        self.has_background && self.error.is_none()
    }

    pub fn has_chrome(&self) -> bool {
        self.has_chrome && self.error.is_none()
    }

    pub fn has_card_overlay(&self) -> bool {
        self.has_card_overlay && self.error.is_none()
    }

    pub fn paint_background(
        &mut self,
        w: f32,
        h: f32,
        time: f64,
        theme: &ui::Theme,
    ) -> Vec<DrawCmd> {
        if !self.has_background() {
            return Vec::new();
        }
        let theme_map = theme_to_rhai_map(theme);
        let mut scope = Scope::new();
        match self.engine.call_fn::<Dynamic>(
            &mut scope,
            &self.ast,
            "paint_background",
            (w as f64, h as f64, time, theme_map),
        ) {
            Ok(val) => parse_draw_cmds(val),
            Err(e) => {
                self.error = Some(format!("paint_background: {e}"));
                Vec::new()
            }
        }
    }

    pub fn paint_chrome(&mut self, w: f32, h: f32, time: f64, theme: &ui::Theme) -> Vec<DrawCmd> {
        if !self.has_chrome() {
            return Vec::new();
        }
        let theme_map = theme_to_rhai_map(theme);
        let mut scope = Scope::new();
        match self.engine.call_fn::<Dynamic>(
            &mut scope,
            &self.ast,
            "paint_chrome",
            (w as f64, h as f64, time, theme_map),
        ) {
            Ok(val) => parse_draw_cmds(val),
            Err(e) => {
                self.error = Some(format!("paint_chrome: {e}"));
                Vec::new()
            }
        }
    }

    pub fn paint_card_overlay(
        &mut self,
        w: f32,
        h: f32,
        hovered: bool,
        installed: bool,
        theme: &ui::Theme,
    ) -> Vec<DrawCmd> {
        if !self.has_card_overlay() {
            return Vec::new();
        }
        let theme_map = theme_to_rhai_map(theme);
        let mut scope = Scope::new();
        match self.engine.call_fn::<Dynamic>(
            &mut scope,
            &self.ast,
            "paint_card_overlay",
            (w as f64, h as f64, hovered, installed, theme_map),
        ) {
            Ok(val) => parse_draw_cmds(val),
            Err(e) => {
                self.error = Some(format!("paint_card_overlay: {e}"));
                Vec::new()
            }
        }
    }
}

fn has_fn(ast: &AST, name: &str, arity: usize) -> bool {
    ast.iter_functions().any(|f| f.name == name && f.params.len() == arity)
}

/// Convert `ui::Theme` into a read-only Rhai map for visual hook scripts.
pub fn theme_to_rhai_map(theme: &ui::Theme) -> Map {
    let mut m = Map::new();
    m.insert("name".into(), Dynamic::from(theme.name.clone()));
    m.insert("dark".into(), Dynamic::from(theme.dark));
    m.insert("radius".into(), Dynamic::from(theme.radius as i64));
    m.insert("font_scale".into(), Dynamic::from(theme.font_scale as f64));
    m.insert("bg".into(), Dynamic::from(color_to_hex(theme.bg)));
    m.insert("chrome".into(), Dynamic::from(color_to_hex(theme.chrome)));
    m.insert("surface".into(), Dynamic::from(color_to_hex(theme.surface)));
    m.insert("surface_hover".into(), Dynamic::from(color_to_hex(theme.surface_hover)));
    m.insert("raised".into(), Dynamic::from(color_to_hex(theme.raised)));
    m.insert("border".into(), Dynamic::from(color_to_hex(theme.border)));
    m.insert("text".into(), Dynamic::from(color_to_hex(theme.text)));
    m.insert("text_dim".into(), Dynamic::from(color_to_hex(theme.text_dim)));
    m.insert("text_faint".into(), Dynamic::from(color_to_hex(theme.text_faint)));
    m.insert("accent".into(), Dynamic::from(color_to_hex(theme.accent)));
    m.insert("accent_2".into(), Dynamic::from(color_to_hex(theme.accent_2)));
    m.insert("on_accent".into(), Dynamic::from(color_to_hex(theme.on_accent)));
    m.insert("play".into(), Dynamic::from(color_to_hex(theme.play)));
    m.insert("online".into(), Dynamic::from(color_to_hex(theme.online)));
    m.insert("in_game".into(), Dynamic::from(color_to_hex(theme.in_game)));
    m.insert("away".into(), Dynamic::from(color_to_hex(theme.away)));
    m.insert("danger".into(), Dynamic::from(color_to_hex(theme.danger)));
    m.insert("discount".into(), Dynamic::from(color_to_hex(theme.discount)));
    m
}

fn parse_draw_cmds(val: Dynamic) -> Vec<DrawCmd> {
    let Ok(arr) = val.into_array() else {
        return Vec::new();
    };
    arr.into_iter().filter_map(parse_draw_cmd).collect()
}

fn parse_draw_cmd(item: Dynamic) -> Option<DrawCmd> {
    let map = item.try_cast::<Map>()?;
    let kind = map
        .get("type")
        .or_else(|| map.get("kind"))
        .and_then(|v| v.clone().into_string().ok())?;

    match kind.as_str() {
        "rect" => Some(DrawCmd::Rect {
            x: get_f32(&map, "x", 0.0),
            y: get_f32(&map, "y", 0.0),
            w: get_f32(&map, "w", 0.0),
            h: get_f32(&map, "h", 0.0),
            radius: get_f32(&map, "radius", 0.0),
            color: get_color(&map, "color")?,
        }),
        "rect_stroke" => Some(DrawCmd::RectStroke {
            x: get_f32(&map, "x", 0.0),
            y: get_f32(&map, "y", 0.0),
            w: get_f32(&map, "w", 0.0),
            h: get_f32(&map, "h", 0.0),
            radius: get_f32(&map, "radius", 0.0),
            width: get_f32(&map, "width", 1.0),
            color: get_color(&map, "color")?,
        }),
        "gradient" => Some(DrawCmd::Gradient {
            x: get_f32(&map, "x", 0.0),
            y: get_f32(&map, "y", 0.0),
            w: get_f32(&map, "w", 0.0),
            h: get_f32(&map, "h", 0.0),
            from: get_color(&map, "from")?,
            to: get_color(&map, "to")?,
            horizontal: get_bool(&map, "horizontal", false),
        }),
        "circle" => Some(DrawCmd::Circle {
            cx: get_f32(&map, "cx", 0.0),
            cy: get_f32(&map, "cy", 0.0),
            r: get_f32(&map, "r", 0.0),
            color: get_color(&map, "color")?,
        }),
        "line" => Some(DrawCmd::Line {
            x1: get_f32(&map, "x1", 0.0),
            y1: get_f32(&map, "y1", 0.0),
            x2: get_f32(&map, "x2", 0.0),
            y2: get_f32(&map, "y2", 0.0),
            width: get_f32(&map, "width", 1.0),
            color: get_color(&map, "color")?,
        }),
        "text" => Some(DrawCmd::Text {
            x: get_f32(&map, "x", 0.0),
            y: get_f32(&map, "y", 0.0),
            text: map.get("text")?.clone().into_string().ok()?,
            size: get_f32(&map, "size", 14.0),
            color: get_color(&map, "color")?,
        }),
        _ => None,
    }
}

fn get_f32(map: &Map, key: &str, default: f32) -> f32 {
    let Some(v) = map.get(key) else {
        return default;
    };
    if let Ok(f) = v.as_float() {
        f as f32
    } else if let Ok(i) = v.as_int() {
        i as f32
    } else {
        default
    }
}

fn get_bool(map: &Map, key: &str, default: bool) -> bool {
    map.get(key).and_then(|v| v.as_bool().ok()).unwrap_or(default)
}

fn get_color(map: &Map, key: &str) -> Option<Color32> {
    let v = map.get(key)?;
    if let Ok(s) = v.clone().into_string() {
        return parse_hex_color(&s).ok();
    }
    if let Ok(arr) = v.clone().into_array() {
        return parse_rgba_array(&arr);
    }
    None
}

fn parse_rgba_array(arr: &Array) -> Option<Color32> {
    if arr.len() < 3 || arr.len() > 4 {
        return None;
    }
    let ch = |idx: usize| -> Option<u8> {
        let v = arr.get(idx)?;
        let n = v.as_int().ok()?;
        u8::try_from(n).ok()
    };
    let r = ch(0)?;
    let g = ch(1)?;
    let b = ch(2)?;
    let a = if arr.len() == 4 { ch(3)? } else { 255 };
    Some(if a == 255 {
        Color32::from_rgb(r, g, b)
    } else {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    })
}
