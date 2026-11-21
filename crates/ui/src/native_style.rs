//! GTK/libadwaita control backgrounds, rendered by the isolated native helper.

use egui::{Color32, Rect, TextureHandle, Ui, pos2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeControlFrame {
    pub width: u16,
    pub height: u16,
    /// Straight-alpha RGBA pixels; egui performs its own premultiplication.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeControlArtwork {
    /// Normal, hovered, and pressed GTK button backgrounds, without text.
    pub light: [NativeControlFrame; 3],
    pub dark: [NativeControlFrame; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeControlMetrics {
    pub min_height: f32,
    pub padding: [f32; 4],
    pub view: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub danger: Color32,
    pub danger_fill: Color32,
}

impl NativeControlArtwork {
    pub fn is_valid(&self) -> bool {
        self.light.iter().chain(&self.dark).all(|frame| {
            (16..=256).contains(&frame.width)
                && (16..=256).contains(&frame.height)
                && frame.pixels.len() == usize::from(frame.width) * usize::from(frame.height) * 4
        })
    }
}

#[derive(Clone)]
struct InstalledArtwork {
    light: [TextureHandle; 3],
    dark: [TextureHandle; 3],
}

fn cache_id() -> egui::Id {
    egui::Id::unique("adwaita-native-control-artwork")
}

pub fn install(ctx: &egui::Context, artwork: &NativeControlArtwork) -> bool {
    if !artwork.is_valid() {
        return false;
    }
    let load = |frames: &[NativeControlFrame; 3], scheme: &str| {
        std::array::from_fn(|i| {
            let frame = &frames[i];
            ctx.load_texture(
                format!("adwaita-native-{scheme}-{i}"),
                egui::ColorImage::from_rgba_unmultiplied(
                    [usize::from(frame.width), usize::from(frame.height)],
                    &frame.pixels,
                ),
                egui::TextureOptions::LINEAR,
            )
        })
    };
    let value = InstalledArtwork {
        light: load(&artwork.light, "light"),
        dark: load(&artwork.dark, "dark"),
    };
    ctx.data_mut(|data| data.insert_temp(cache_id(), value));
    true
}

/// Paint only the GTK-rendered background. Text and interaction stay in egui.
/// Nine patches preserve native corners instead of stretching the entire bitmap.
pub fn paint_button(
    ui: &Ui,
    dark: bool,
    rect: Rect,
    hovered: bool,
    pressed: bool,
    enabled: bool,
) -> bool {
    let Some(artwork) = ui
        .ctx()
        .data(|data| data.get_temp::<InstalledArtwork>(cache_id()))
    else {
        return false;
    };
    if !rect.is_positive() || !rect.is_finite() {
        return false;
    }
    let state = if pressed && enabled {
        2
    } else if hovered && enabled {
        1
    } else {
        0
    };
    let texture = if dark {
        &artwork.dark[state]
    } else {
        &artwork.light[state]
    };
    // The helper paints an 80×34 logical-pixel GTK background at 2× resolution.
    let size = texture.size_vec2();
    let source_inset = (size.y * 8.0 / 34.0).min(size.x * 0.5).min(size.y * 0.5);
    let x_inset = 8.0_f32.min(rect.width() * 0.5);
    let y_inset = 8.0_f32.min(rect.height() * 0.5);
    let xs = [
        rect.left(),
        rect.left() + x_inset,
        rect.right() - x_inset,
        rect.right(),
    ];
    let ys = [
        rect.top(),
        rect.top() + y_inset,
        rect.bottom() - y_inset,
        rect.bottom(),
    ];
    let us = [0.0, source_inset / size.x, 1.0 - source_inset / size.x, 1.0];
    let vs = [0.0, source_inset / size.y, 1.0 - source_inset / size.y, 1.0];
    let tint = Color32::WHITE.gamma_multiply(if enabled { 1.0 } else { 0.45 });
    for y in 0..3 {
        for x in 0..3 {
            let patch = Rect::from_min_max(pos2(xs[x], ys[y]), pos2(xs[x + 1], ys[y + 1]));
            if patch.is_positive() {
                ui.painter().image(
                    texture.id(),
                    patch,
                    Rect::from_min_max(pos2(us[x], vs[y]), pos2(us[x + 1], vs[y + 1])),
                    tint,
                );
            }
        }
    }
    true
}

pub fn clear(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<InstalledArtwork>(cache_id()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artwork() -> NativeControlArtwork {
        let frame = |r, g, b| NativeControlFrame {
            width: 80,
            height: 34,
            pixels: [r, g, b, 255].repeat(80 * 34),
        };
        NativeControlArtwork {
            light: [frame(255, 0, 0), frame(0, 255, 0), frame(0, 0, 255)],
            dark: [frame(20, 0, 0), frame(0, 20, 0), frame(0, 0, 20)],
        }
    }

    #[test]
    fn native_artwork_is_validated_and_painted_with_preserved_corners() {
        let ctx = egui::Context::default();
        assert!(install(&ctx, &artwork()));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let rect = Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 34.0));
            assert!(paint_button(ui, false, rect, true, false, true));
        });
        output.textures_delta.clear();
        let meshes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .collect();
        assert_eq!(
            meshes.len(),
            9,
            "native corners and center should use separate patches"
        );
        assert!(
            meshes
                .iter()
                .flat_map(|m| &m.vertices)
                .all(|v| v.pos.x >= 10.0 && v.pos.x <= 130.0 && v.pos.y >= 10.0 && v.pos.y <= 44.0)
        );
        assert!(
            meshes
                .iter()
                .all(|m| m.texture_id != egui::TextureId::default())
        );
        let mut invalid = artwork();
        invalid.light[0].pixels.pop();
        assert!(
            !install(&ctx, &invalid),
            "invalid dimensions must not replace cached native artwork"
        );
        clear(&ctx);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            assert!(!paint_button(
                ui,
                true,
                Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 8.0)),
                false,
                false,
                true
            ));
        });
        output.textures_delta.clear();
    }
}
