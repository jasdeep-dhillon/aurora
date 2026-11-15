//! Aurora's logo and app icon, from one greyscale SVG (`assets/aurora.svg`):
//! the logo is that mark tinted with the theme's text color, the icon is the
//! mark on a dark rounded tile.

/// The aurora mark: greys on transparent, drawn tinted by [`crate::widgets::logo`].
pub const LOGO_SVG: &[u8] = include_bytes!("../assets/aurora.svg");

/// Image URI egui caches the logo under (`.svg`, so it's rendered sharp at
/// every size it's shown at).
pub const LOGO_URI: &str = "bytes://aurora-logo.svg";

/// RGBA pixels of the app icon at `size` × `size`: the mark in light grey on
/// a dark slate rounded tile.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    use resvg::tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};
    let n = size as f32;
    let Some(mut pixmap) = Pixmap::new(size, size) else { return vec![0; (size * size * 4) as usize] };

    // Rounded tile.
    let m = n * 0.04;
    let r = n * 0.22;
    let (x0, y0, x1, y1) = (m, m, n - m, n - m);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    if let Some(tile) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(0x1A, 0x21, 0x29, 0xFF));
        paint.anti_alias = true;
        pixmap.fill_path(&tile, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // The mark, inset inside the tile.
    if let Ok(tree) = resvg::usvg::Tree::from_data(LOGO_SVG, &resvg::usvg::Options::default()) {
        let inner = (x1 - x0) * 0.78;
        let scale = inner / tree.size().width().max(tree.size().height());
        let offset = (n - inner) / 2.0;
        let mut mark = Pixmap::new(size, size).expect("same size as the icon");
        resvg::render(&tree, Transform::from_scale(scale, scale).post_translate(offset, offset), &mut mark.as_mut());
        // Light grey, not pure white, like the rest of the interface text.
        for px in mark.pixels_mut() {
            let c = px.demultiply();
            let g = |v: u8| (u16::from(v) * 0xDF / 0xFF) as u8;
            *px = resvg::tiny_skia::ColorU8::from_rgba(g(c.red()), g(c.green()), g(c.blue()), c.alpha()).premultiply();
        }
        pixmap.draw_pixmap(0, 0, mark.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), Transform::identity(), None);
    }

    // Straight (not premultiplied) RGBA, as window and tray icons expect.
    pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}

/// The app icon for a window.
pub fn window_icon(size: u32) -> egui::IconData {
    egui::IconData { rgba: icon_rgba(size), width: size, height: size }
}
