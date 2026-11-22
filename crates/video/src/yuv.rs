//! NV12 (what the H.264 decoder outputs) to RGBA for egui textures.

use egui::Color32;

/// Convert an NV12 picture (limited range) to opaque RGBA pixels.
/// `stride` is the byte pitch of both planes, `uv` starts the chroma plane.
/// BT.709 for HD, BT.601 for SD, like players do when the stream says nothing.
pub fn nv12_to_rgba(y: &[u8], uv: &[u8], stride: usize, width: usize, height: usize, out: &mut Vec<Color32>) {
    let total = width * height;
    out.resize(total, Color32::BLACK);
    if total == 0 {
        return;
    }
    // Fixed-point coefficients (<< 13).
    let (rv, gu, gv, bu) = if height >= 600 { (14686, 1747, 4366, 17305) } else { (13074, 3203, 6660, 16531) };
    const YS: i32 = 9539; // 255/219 << 13
    #[inline(always)]
    fn clamp_u8(v: i32) -> u8 {
        (v >> 13).clamp(0, 255) as u8
    }
    #[inline(always)]
    fn pixel(yy: u8, r_off: i32, g_off: i32, b_off: i32) -> Color32 {
        let l = (yy as i32 - 16) * YS;
        Color32::from_rgb(clamp_u8(l + r_off), clamp_u8(l + g_off), clamp_u8(l + b_off))
    }

    let pairs = width / 2;
    let has_odd = (width & 1) != 0;
    let uv_bytes = width.div_ceil(2) * 2;

    // Process 2 luma rows per chroma row so UV loads and chroma offsets are computed once per 2x2 block.
    let mut row = 0;
    while row + 1 < height {
        let y0 = &y[row * stride..row * stride + width];
        let y1 = &y[(row + 1) * stride..(row + 1) * stride + width];
        let crow = &uv[(row / 2) * stride..(row / 2) * stride + uv_bytes];
        let (o0, o1) = out[row * width..(row + 2) * width].split_at_mut(width);
        let c_pairs = &crow[..pairs * 2];
        for (i, c) in c_pairs.chunks_exact(2).enumerate() {
            let x = i * 2;
            let u = c[0] as i32 - 128;
            let v = c[1] as i32 - 128;
            let r_off = rv * v;
            let g_off = -gu * u - gv * v;
            let b_off = bu * u;
            o0[x] = pixel(y0[x], r_off, g_off, b_off);
            o0[x + 1] = pixel(y0[x + 1], r_off, g_off, b_off);
            o1[x] = pixel(y1[x], r_off, g_off, b_off);
            o1[x + 1] = pixel(y1[x + 1], r_off, g_off, b_off);
        }
        if has_odd {
            let x = pairs * 2;
            let u = crow[x] as i32 - 128;
            let v = crow[x + 1] as i32 - 128;
            let r_off = rv * v;
            let g_off = -gu * u - gv * v;
            let b_off = bu * u;
            o0[x] = pixel(y0[x], r_off, g_off, b_off);
            o1[x] = pixel(y1[x], r_off, g_off, b_off);
        }
        row += 2;
    }
    if row < height {
        let y0 = &y[row * stride..row * stride + width];
        let crow = &uv[(row / 2) * stride..(row / 2) * stride + uv_bytes];
        let o0 = &mut out[row * width..(row + 1) * width];
        for (i, c) in crow[..pairs * 2].chunks_exact(2).enumerate() {
            let x = i * 2;
            let u = c[0] as i32 - 128;
            let v = c[1] as i32 - 128;
            let r_off = rv * v;
            let g_off = -gu * u - gv * v;
            let b_off = bu * u;
            o0[x] = pixel(y0[x], r_off, g_off, b_off);
            o0[x + 1] = pixel(y0[x + 1], r_off, g_off, b_off);
        }
        if has_odd {
            let x = pairs * 2;
            let u = crow[x] as i32 - 128;
            let v = crow[x + 1] as i32 - 128;
            let r_off = rv * v;
            let g_off = -gu * u - gv * v;
            let b_off = bu * u;
            o0[x] = pixel(y0[x], r_off, g_off, b_off);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn grey_and_white() {
        let y = [16u8, 235, 126, 126];
        let uv = [128u8, 128];
        let mut out = Vec::new();
        super::nv12_to_rgba(&y, &uv, 2, 2, 2, &mut out);
        assert_eq!(out[0], egui::Color32::BLACK);
        assert_eq!(out[1], egui::Color32::WHITE);
        assert!((out[2].r() as i32 - 128).abs() <= 1);
    }
}
