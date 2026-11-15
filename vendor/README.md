# Patched egui crates

`epaint` and `egui_glow` 0.37.0, copied from crates.io and wired in through
`[patch.crates-io]` in the root `Cargo.toml`, to give text subpixel (LCD)
anti-aliasing like the rest of the desktop. egui only does grayscale, which
looks jagged next to other apps at 1x.

## epaint

- `FontData::lcd` (+ `FontData::lcd(bool)` builder): rasterize this font per
  RGB subpixel. Passed through `FaceStore::install` → `FontFace::new`.
- `text/font_face.rs` `rasterize_lcd`: renders the glyph at 3× horizontal
  resolution, applies FreeType's default LCD filter, and stores coverage as
  `(r, g, b, 0)`. Premultiplied colors never have color without alpha, so the
  renderer can recognize these texels.
- `FontColorTransferFunction::to_atlas_color` applies the transfer function to
  each channel of such texels.

## egui_glow

- Fragment shader: on desktop OpenGL ≥ 3.3, built as GLSL 330 with a second
  output for dual-source blending (`GL_ONE_MINUS_SRC1_COLOR`). LCD texels, only
  while drawing with the font atlas (`u_font_atlas`), blend per channel; every
  other pixel blends exactly as before.
- If dual-source blending is unavailable or that shader fails to build, the
  original shader is used and LCD texels are drawn as grayscale.

## Upgrading egui

Copy the new `epaint` and `egui_glow` from `~/.cargo/registry/src/*/` and
reapply the changes above, or drop the patch and stop calling
`ui::install_fonts_with(ctx, true)`.
