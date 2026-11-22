//! Writes the app icon as PNG files: `cargo run -p ui --example icon_png -- <dir>`.
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let rgba = ui::brand::icon_rgba(size);
        let img = image::RgbaImage::from_raw(size, size, rgba).expect("size");
        img.save(dir.join(format!("icon-{size}.png"))).expect("save");
    }
}
