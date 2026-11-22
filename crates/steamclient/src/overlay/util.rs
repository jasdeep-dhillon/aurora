//! Shared helpers for the in-game overlay renderers (D3D11, D3D12, D3D9,
//! OpenGL, Vulkan).
//!
//! The pure math, color conversion, scissor clipping, SPIR-V validation and
//! CPU texture shadow store are platform-neutral so they can be unit-tested
//! on both Linux and Windows.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};

/// Which graphics API owns the active in-game overlay session.
///
/// Only one backend may drain `shared::take_for_present()` at a time so that
/// texture deltas (`TexOp::Set` / `TexOp::Free`) are never split across two
/// renderers (for example a launcher using D3D9 before a D3D11 game, or a
/// DXGI swap chain inspected by both D3D11 and D3D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Backend {
    None = 0,
    D3d11 = 1,
    D3d12 = 2,
    D3d9 = 3,
    OpenGl = 4,
    Vulkan = 5,
}

impl Backend {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::D3d11,
            2 => Self::D3d12,
            3 => Self::D3d9,
            4 => Self::OpenGl,
            5 => Self::Vulkan,
            _ => Self::None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::D3d11 => "D3D11",
            Self::D3d12 => "D3D12",
            Self::D3d9 => "D3D9",
            Self::OpenGl => "OpenGL",
            Self::Vulkan => "Vulkan",
        }
    }
}

static ACTIVE_BACKEND: AtomicU8 = AtomicU8::new(Backend::None as u8);

/// Claim ownership of the overlay session for `backend`.
///
/// Returns `true` if `backend` is now the active owner (either because it was
/// already the owner or because no backend had claimed ownership yet).
pub fn claim_backend(backend: Backend) -> bool {
    if backend == Backend::None {
        return false;
    }
    let want = backend as u8;
    let cur = ACTIVE_BACKEND.load(Ordering::Acquire);
    if cur == want {
        return true;
    }
    if cur != Backend::None as u8 {
        return false;
    }
    ACTIVE_BACKEND
        .compare_exchange(Backend::None as u8, want, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        || ACTIVE_BACKEND.load(Ordering::Acquire) == want
}

pub fn active_backend() -> Backend {
    Backend::from_u8(ACTIVE_BACKEND.load(Ordering::Acquire))
}

#[cfg(test)]
pub fn reset_backend_for_test() {
    ACTIVE_BACKEND.store(Backend::None as u8, Ordering::Release);
}

/// Convert an egui clip rectangle (in points, top-left origin) into pixel
/// coordinates `[left, top, right, bottom]` clamped to `[0, width]` x `[0, height]`.
/// Returns `None` if the clamped rectangle is empty.
pub fn clip_rect_pixels(clip: [f32; 4], ppp: f32, width: u32, height: u32) -> Option<[i32; 4]> {
    if width == 0 || height == 0 {
        return None;
    }
    let ppp = ppp.max(0.1);
    let (w, h) = (width as f32, height as f32);
    let left = (clip[0] * ppp).round().clamp(0.0, w) as i32;
    let top = (clip[1] * ppp).round().clamp(0.0, h) as i32;
    let right = (clip[2] * ppp).round().clamp(0.0, w) as i32;
    let bottom = (clip[3] * ppp).round().clamp(0.0, h) as i32;
    if right > left && bottom > top {
        Some([left, top, right, bottom])
    } else {
        None
    }
}

/// Convert an egui clip rectangle (top-left origin, in points) into an OpenGL
/// `glScissor(x, y, width, height)` rectangle (bottom-left origin, in pixels).
pub fn gl_scissor_rect(clip: [f32; 4], ppp: f32, width: u32, height: u32) -> Option<[i32; 4]> {
    let [left, top, right, bottom] = clip_rect_pixels(clip, ppp, width, height)?;
    let x = left;
    let y = (height as i32).saturating_sub(bottom).max(0);
    let w = right - left;
    let h = bottom - top;
    if w > 0 && h > 0 { Some([x, y, w, h]) } else { None }
}

/// Swizzle premultiplied RGBA bytes into little-endian D3D9 `D3DFMT_A8R8G8B8`
/// (`[B, G, R, A]` in memory).
pub fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

/// Pack an `[r, g, b, a]` color into a D3D9 `D3DCOLOR` (`0xAARRGGBB`).
#[inline]
pub fn rgba_to_d3dcolor(rgba: [u8; 4]) -> u32 {
    ((rgba[3] as u32) << 24) | ((rgba[0] as u32) << 16) | ((rgba[1] as u32) << 8) | (rgba[2] as u32)
}

/// Row-major 4x4 orthographic projection matrix for D3D9, mapping egui point
/// coordinates `[0, screen_w] x [0, screen_h]` to clip space `[-1, 1] x [1, -1]`
/// with D3D9's `-0.5` pixel-center offset so text renders crisply.
pub fn ortho_d3d9(screen_w: f32, screen_h: f32, fb_w: f32, fb_h: f32) -> [f32; 16] {
    let sw = screen_w.max(1.0);
    let sh = screen_h.max(1.0);
    let fw = fb_w.max(1.0);
    let fh = fb_h.max(1.0);
    // Shift by half a back-buffer pixel in NDC (-1/fw in X, +1/fh in Y).
    let tx = -1.0 - 1.0 / fw;
    let ty = 1.0 + 1.0 / fh;
    [
        2.0 / sw, 0.0,       0.0, 0.0,
        0.0,      -2.0 / sh, 0.0, 0.0,
        0.0,      0.0,       0.5, 0.0,
        tx,       ty,        0.5, 1.0,
    ]
}

/// Unpack one 20-byte egui vertex (`x, y, u, v` as `f32`, then `[r, g, b, a]`).
#[inline]
pub fn unpack_egui_vertex(bytes: &[u8]) -> Option<(f32, f32, f32, f32, [u8; 4])> {
    if bytes.len() < 20 {
        return None;
    }
    let x = f32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let y = f32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let u = f32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let v = f32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let col = [bytes[16], bytes[17], bytes[18], bytes[19]];
    Some((x, y, u, v, col))
}

/// Whether a Vulkan `VkFormat` (by numeric enum value) expects linear light
/// output (sRGB or 16-bit float swapchain) so egui's sRGB colors should be
/// converted to linear in the fragment shader.
pub fn vk_format_is_linear_out(vk_format: i32) -> bool {
    // VK_FORMAT_R8G8B8A8_SRGB = 43
    // VK_FORMAT_B8G8R8A8_SRGB = 50
    // VK_FORMAT_A8B8G8R8_SRGB_PACK32 = 57
    // VK_FORMAT_R16G16B16A16_SFLOAT = 97
    matches!(vk_format, 43 | 50 | 57 | 97)
}

/// Validate RGBA texture upload dimensions and buffer length using checked
/// arithmetic. Returns `Some((row_bytes, total_bytes))` when valid.
pub fn validate_tex_rgba(size: [u32; 2], rgba_len: usize) -> Option<(usize, usize)> {
    let [w, h] = size;
    if w == 0 || h == 0 {
        return None;
    }
    let row_bytes = (w as usize).checked_mul(4)?;
    let total_bytes = row_bytes.checked_mul(h as usize)?;
    if rgba_len < total_bytes {
        return None;
    }
    Some((row_bytes, total_bytes))
}

/// Validate a texture sub-rectangle update without `u32` addition overflow.
pub fn validate_tex_subrect(pos: [u32; 2], size: [u32; 2], target_size: [u32; 2]) -> bool {
    let ([px, py], [w, h], [tw, th]) = (pos, size, target_size);
    px.checked_add(w).is_some_and(|r| r <= tw) && py.checked_add(h).is_some_and(|b| b <= th)
}

/// Safely grow a GPU upload buffer capacity to the next power of two without
/// overflowing `usize`.
pub fn checked_grow_capacity(need: usize, min_cap: usize) -> Option<usize> {
    need.max(min_cap).checked_next_power_of_two()
}

/// Validate vertex alignment and checked byte/element totals for a list of
/// overlay meshes before copying into mapped GPU buffers.
/// Returns `Some((total_vbytes, total_ibytes))` if every mesh is well-formed.
pub fn validate_mesh_totals(meshes: &[super::wire::Mesh], vertex_size: usize) -> Option<(usize, usize)> {
    if vertex_size == 0 {
        return None;
    }
    let mut total_vbytes = 0usize;
    let mut total_ibytes = 0usize;
    let mut total_verts = 0usize;
    let mut total_indices = 0usize;
    for m in meshes {
        if m.vertices.len() % vertex_size != 0 {
            return None;
        }
        let vcount = m.vertices.len() / vertex_size;
        let vcount_u32 = u32::try_from(vcount).ok()?;
        if m.indices.iter().any(|&idx| idx >= vcount_u32) {
            return None;
        }
        let ib = m.indices.len().checked_mul(4)?;
        total_vbytes = total_vbytes.checked_add(m.vertices.len())?;
        total_ibytes = total_ibytes.checked_add(ib)?;
        total_verts = total_verts.checked_add(vcount)?;
        total_indices = total_indices.checked_add(m.indices.len())?;
    }
    u32::try_from(total_vbytes).ok()?;
    u32::try_from(total_ibytes).ok()?;
    i32::try_from(total_verts).ok()?;
    u32::try_from(total_indices).ok()?;
    Some((total_vbytes, total_ibytes))
}

/// Basic SPIR-V binary validation (magic number `0x07230203`, minimum 5-word
/// header, non-zero bound).
pub fn validate_spirv(words: &[u32]) -> bool {
    words.len() >= 5 && words[0] == 0x0723_0203 && words[3] > 0 && words[4] == 0
}

/// CPU-side shadow copy of egui textures, used by D3D9 so textures can be
/// recreated losslessly after `IDirect3DDevice9::Reset` (device lost / mode
/// switch).
#[derive(Default)]
pub struct TexShadow {
    entries: HashMap<u64, ShadowTex>,
}

pub struct ShadowTex {
    pub size: [u32; 2],
    pub linear: bool,
    pub rgba: Vec<u8>,
}

impl TexShadow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a full texture upload or sub-rectangle patch. Returns `true` if
    /// the shadow store was updated.
    pub fn set(&mut self, id: u64, pos: Option<[u32; 2]>, size: [u32; 2], linear: bool, rgba: &[u8]) -> bool {
        let [w, h] = size;
        if w == 0 || h == 0 {
            return false;
        }
        let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
        if rgba.len() != expected {
            return false;
        }
        match pos {
            None => {
                self.entries.insert(id, ShadowTex { size, linear, rgba: rgba.to_vec() });
                true
            }
            Some([px, py]) => {
                let Some(entry) = self.entries.get_mut(&id) else { return false };
                let [ew, eh] = entry.size;
                if px.checked_add(w).is_none_or(|r| r > ew) || py.checked_add(h).is_none_or(|b| b > eh) {
                    return false;
                }
                entry.linear = linear;
                let row_bytes = (w as usize) * 4;
                let dst_stride = (ew as usize) * 4;
                for row in 0..(h as usize) {
                    let src_off = row * row_bytes;
                    let dst_off = ((py as usize) + row) * dst_stride + (px as usize) * 4;
                    entry.rgba[dst_off..dst_off + row_bytes].copy_from_slice(&rgba[src_off..src_off + row_bytes]);
                }
                true
            }
        }
    }

    pub fn free(&mut self, id: u64) {
        self.entries.remove(&id);
    }

    pub fn get(&self, id: u64) -> Option<&ShadowTex> {
        self.entries.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, &ShadowTex)> {
        self.entries.iter().map(|(&id, t)| (id, t))
    }
}

/// Shared HLSL source for D3D11 (`vs_4_0`/`ps_4_0`) and D3D12 (`vs_5_0`/`ps_5_0`).
#[cfg(windows)]
pub const HLSL_OVERLAY_SHADER: &str = r#"
cbuffer C : register(b0) { float2 screen; float linear_out; float pad; };
struct VI { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
struct PI { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
PI vs(VI i) {
    PI o;
    o.pos = float4(2.0 * i.pos.x / screen.x - 1.0, 1.0 - 2.0 * i.pos.y / screen.y, 0.0, 1.0);
    o.uv = i.uv;
    o.col = i.col;
    return o;
}
Texture2D t0 : register(t0);
SamplerState s0 : register(s0);
float3 to_linear(float3 c) { return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4); }
float4 ps(PI i) : SV_Target {
    float4 c = i.col * t0.Sample(s0, i.uv);
    if (linear_out > 0.5) c.rgb = to_linear(c.rgb);
    return c;
}
"#;

/// W^X-safe vtable slot patch (writes only to the data vtable pointer using
/// temporary `PAGE_READWRITE`, then restores the original page protection).
#[cfg(windows)]
pub unsafe fn patch_vtable(vtable: *mut usize, slot: usize, ours: usize, real: &std::sync::atomic::AtomicUsize) -> bool {
    use std::ffi::c_void;
    use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
    unsafe {
        let p = vtable.add(slot);
        if *p == ours {
            return true;
        }
        let mut old = PAGE_PROTECTION_FLAGS(0);
        if VirtualProtect(p as *const c_void, size_of::<usize>(), PAGE_READWRITE, &mut old).is_err() {
            crate::log!("overlay: VirtualProtect failed for vtable slot {slot}");
            return false;
        }
        real.store(*p, Ordering::Release);
        *p = ours;
        let _ = VirtualProtect(p as *const c_void, size_of::<usize>(), old, &mut old);
        true
    }
}

/// Patch matching IAT slots across all loaded modules except our own DLL and
/// any modules in `skip`. Preserves W^X (only data IAT slots are updated).
#[cfg(windows)]
pub unsafe fn patch_all_modules(table: &[(usize, usize)], skip: &[windows::Win32::Foundation::HMODULE]) -> usize {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW,
    };
    use windows::Win32::System::ProcessStatus::EnumProcessModules;
    use windows::Win32::System::Threading::GetCurrentProcess;
    if table.is_empty() {
        return 0;
    }
    unsafe {
        let mut me = HMODULE::default();
        let _ = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(patch_all_modules as *const u16),
            &mut me,
        );
        let mut modules = vec![HMODULE::default(); 1024];
        let mut needed = 0u32;
        if EnumProcessModules(GetCurrentProcess(), modules.as_mut_ptr(), (modules.len() * size_of::<HMODULE>()) as u32, &mut needed).is_err() {
            return 0;
        }
        modules.truncate((needed as usize / size_of::<HMODULE>()).min(modules.len()));
        let mut patched = 0;
        for m in modules {
            if m == me || skip.contains(&m) {
                continue;
            }
            patched += super::input::patch_module(m.0 as *mut u8, table);
        }
        patched
    }
}

/// Compile one HLSL entry point with `d3dcompiler_47.dll` (loaded on demand so
/// games that never draw the overlay don't load it).
#[cfg(windows)]
pub unsafe fn compile_hlsl(source: &str, entry: windows::core::PCSTR, target: windows::core::PCSTR) -> windows::core::Result<Vec<u8>> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{HRESULT, Interface, PCSTR, s, w};
    type CompileFn = unsafe extern "system" fn(
        *const c_void,
        usize,
        PCSTR,
        *const c_void,
        *mut c_void,
        PCSTR,
        PCSTR,
        u32,
        u32,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> HRESULT;
    unsafe {
        let module = LoadLibraryW(w!("d3dcompiler_47.dll"))?;
        let Some(f) = GetProcAddress(module, s!("D3DCompile")) else {
            return Err(windows::core::Error::from_thread());
        };
        let f: CompileFn = std::mem::transmute(f);
        let (mut code, mut errors) = (std::ptr::null_mut(), std::ptr::null_mut());
        let hr = f(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            entry,
            target,
            0,
            0,
            &mut code,
            &mut errors,
        );
        if !errors.is_null() {
            let e = ID3DBlob::from_raw(errors);
            let text = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
            crate::log!("overlay: shader: {}", String::from_utf8_lossy(text));
        }
        hr.ok()?;
        let code = ID3DBlob::from_raw(code);
        Ok(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()).to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_claim_is_exclusive() {
        reset_backend_for_test();
        assert_eq!(active_backend(), Backend::None);
        assert_eq!(Backend::None.name(), "none");
        assert!(!claim_backend(Backend::None));
        assert!(claim_backend(Backend::D3d12));
        assert_eq!(active_backend(), Backend::D3d12);
        assert_eq!(active_backend().name(), "D3D12");
        // Re-claiming the same backend succeeds.
        assert!(claim_backend(Backend::D3d12));
        // Another backend cannot steal ownership.
        assert!(!claim_backend(Backend::D3d11));
        assert!(!claim_backend(Backend::OpenGl));
        assert!(!claim_backend(Backend::D3d9));
        assert!(!claim_backend(Backend::Vulkan));
        reset_backend_for_test();
    }

    #[test]
    fn clip_rect_and_gl_scissor_conversion() {
        // 1920x1080 framebuffer, ppp = 2.0, clip [10, 20, 110, 70] -> [20, 40, 220, 140]
        assert_eq!(clip_rect_pixels([10.0, 20.0, 110.0, 70.0], 2.0, 1920, 1080), Some([20, 40, 220, 140]));
        // OpenGL y = 1080 - 140 = 940, w = 200, h = 100
        assert_eq!(gl_scissor_rect([10.0, 20.0, 110.0, 70.0], 2.0, 1920, 1080), Some([20, 940, 200, 100]));
        // Empty or out-of-bounds clips return None
        assert_eq!(clip_rect_pixels([100.0, 100.0, 100.0, 200.0], 1.0, 800, 600), None);
        assert_eq!(gl_scissor_rect([-50.0, -50.0, -10.0, -10.0], 1.0, 800, 600), None);
        assert_eq!(clip_rect_pixels([0.0, 0.0, 10.0, 10.0], 1.0, 0, 600), None);
    }

    #[test]
    fn d3d9_color_and_ortho_helpers() {
        assert_eq!(rgba_to_d3dcolor([0x11, 0x22, 0x33, 0x44]), 0x4411_2233);
        assert_eq!(rgba_to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]), vec![3, 2, 1, 4, 30, 20, 10, 40]);
        let m = ortho_d3d9(1280.0, 720.0, 1280.0, 720.0);
        // Top-left (0, 0) maps to (-1 - 1/1280, 1 + 1/720)
        assert!((m[0] - 2.0 / 1280.0).abs() < 1e-6);
        assert!((m[5] + 2.0 / 720.0).abs() < 1e-6);
        assert!((m[12] - (-1.0 - 1.0 / 1280.0)).abs() < 1e-6);
        assert!((m[13] - (1.0 + 1.0 / 720.0)).abs() < 1e-6);
    }

    #[test]
    fn unpack_egui_vertex_round_trip() {
        let mut raw = Vec::new();
        for f in [12.5f32, 34.25, 0.25, 0.75] {
            raw.extend_from_slice(&f.to_le_bytes());
        }
        raw.extend_from_slice(&[200, 150, 100, 255]);
        assert_eq!(unpack_egui_vertex(&raw), Some((12.5, 34.25, 0.25, 0.75, [200, 150, 100, 255])));
        assert_eq!(unpack_egui_vertex(&raw[..19]), None);
    }

    #[test]
    fn tex_shadow_full_patch_and_bounds() {
        let mut s = TexShadow::new();
        // 2x2 texture
        assert!(s.set(7, None, [2, 2], true, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]));
        // Patch bottom-right pixel (1, 1)
        assert!(s.set(7, Some([1, 1]), [1, 1], false, &[99, 98, 97, 96]));
        let t = s.get(7).unwrap();
        assert!(!t.linear);
        assert_eq!(&t.rgba[12..16], &[99, 98, 97, 96]);
        assert_eq!(s.iter().count(), 1);
        // Out-of-bounds patch rejected
        assert!(!s.set(7, Some([2, 0]), [1, 1], false, &[0; 4]));
        // Wrong byte length rejected
        assert!(!s.set(8, None, [2, 2], true, &[0; 4]));
        s.free(7);
        assert!(s.get(7).is_none());
    }

    #[test]
    fn spirv_header_validation() {
        assert!(validate_spirv(&[0x0723_0203, 0x0001_0000, 0, 10, 0]));
        assert!(!validate_spirv(&[0x1234_5678, 0x0001_0000, 0, 10, 0]));
        assert!(!validate_spirv(&[0x0723_0203, 0x0001_0000, 0, 10]));
        assert!(vk_format_is_linear_out(50));
        assert!(!vk_format_is_linear_out(44));
    }

    #[test]
    fn converts_back_buffer_pixels_and_fps() {
        let mut out = Vec::new();
        to_rgb(PixelKind::Rgba8, &[1, 2, 3, 255], &mut out);
        to_rgb(PixelKind::Bgra8, &[1, 2, 3, 255], &mut out);
        to_rgb(PixelKind::Rgb10a2, &(1023u32 | (512 << 20)).to_le_bytes(), &mut out);
        to_rgb(PixelKind::Bgr10a2, &(1023u32 | (512 << 20)).to_le_bytes(), &mut out);
        to_rgb(PixelKind::Half, &[0x00, 0x3C, 0x00, 0x00, 0x00, 0x38, 0x00, 0x3C], &mut out);
        assert_eq!(out, vec![1, 2, 3, 3, 2, 1, 255, 0, 128, 128, 0, 255, 255, 0, 188]);
        assert_eq!(gl_rgba_flip_to_rgb(&[1, 2, 3, 255, 4, 5, 6, 255], 1, 2), vec![4, 5, 6, 1, 2, 3]);
        assert_eq!(parse_hotkey_env(Some("2:120"), (1, 9)), (2, 120));
        assert_eq!(parse_hotkey_env(Some("bad"), (1, 9)), (1, 9));
    }
}

/// How a back buffer's pixels are laid out, for screenshots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PixelKind {
    Rgba8,
    Bgra8,
    Rgb10a2,
    Bgr10a2,
    /// 16-bit float RGBA, linear (HDR or scRGB): converted to sRGB.
    Half,
}

pub fn to_rgb(kind: PixelKind, row: &[u8], out: &mut Vec<u8>) {
    match kind {
        PixelKind::Rgba8 => row.chunks_exact(4).for_each(|p| out.extend_from_slice(&p[..3])),
        PixelKind::Bgra8 => row.chunks_exact(4).for_each(|p| out.extend_from_slice(&[p[2], p[1], p[0]])),
        PixelKind::Rgb10a2 => row.chunks_exact(4).for_each(|p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            out.extend_from_slice(&[((v & 0x3FF) >> 2) as u8, (((v >> 10) & 0x3FF) >> 2) as u8, (((v >> 20) & 0x3FF) >> 2) as u8]);
        }),
        PixelKind::Bgr10a2 => row.chunks_exact(4).for_each(|p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            out.extend_from_slice(&[(((v >> 20) & 0x3FF) >> 2) as u8, (((v >> 10) & 0x3FF) >> 2) as u8, ((v & 0x3FF) >> 2) as u8]);
        }),
        PixelKind::Half => row.chunks_exact(8).for_each(|p| {
            for c in 0..3 {
                out.push(linear_to_srgb8(half_to_f32(u16::from_le_bytes([p[c * 2], p[c * 2 + 1]]))));
            }
        }),
    }
}

/// Flip bottom-up GL RGBA pixels into tightly packed top-down RGB.
pub fn gl_rgba_flip_to_rgb(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_bytes = (width as usize).saturating_mul(4);
    let mut out = Vec::with_capacity((width as usize).saturating_mul(height as usize).saturating_mul(3));
    if row_bytes == 0 {
        return out;
    }
    for y in (0..height as usize).rev() {
        let start = y * row_bytes;
        let end = start + row_bytes;
        if end <= rgba.len() {
            to_rgb(PixelKind::Rgba8, &rgba[start..end], &mut out);
        }
    }
    out
}

pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

pub fn linear_to_srgb8(v: f32) -> u8 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Parse `"<mods>:<vk>"` hotkey configuration strings.
pub fn parse_hotkey_env(s: Option<&str>, default: (u8, u32)) -> (u8, u32) {
    let Some(raw) = s else { return default };
    let Some((m, k)) = raw.split_once(':') else { return default };
    match (m.trim().parse::<u8>(), k.trim().parse::<u32>()) {
        (Ok(mods), Ok(vk)) if vk != 0 => (mods & 0x07, vk),
        _ => default,
    }
}

/// Tracks presented frames and returns an integer FPS once per second.
pub struct FpsCounter {
    frames: u32,
    since: std::time::Instant,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self { frames: 0, since: std::time::Instant::now() }
    }

    pub fn tick(&mut self) -> Option<u32> {
        self.frames += 1;
        let elapsed = self.since.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            let fps = (self.frames as f32 / elapsed).round() as u32;
            self.frames = 0;
            self.since = std::time::Instant::now();
            Some(fps)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod sec_validation_tests {
    use super::*;
    use super::super::wire::Mesh;

    #[test]
    fn tex_rgba_and_subrect_reject_overflow_and_short_buffers() {
        assert_eq!(validate_tex_rgba([4, 4], 64), Some((16, 64)));
        assert_eq!(validate_tex_rgba([4, 4], 63), None);
        assert_eq!(validate_tex_rgba([0, 4], 64), None);
        assert_eq!(validate_tex_rgba([u32::MAX, u32::MAX], 64), None);

        assert!(validate_tex_subrect([4, 4], [4, 4], [8, 8]));
        assert!(!validate_tex_subrect([5, 4], [4, 4], [8, 8]));
        assert!(!validate_tex_subrect([u32::MAX, 0], [1, 1], [8, 8]));
    }

    #[test]
    fn mesh_totals_reject_misaligned_vertices_and_overflow() {
        let good = Mesh { texture: 1, clip: [0.0; 4], vertices: vec![0u8; 60], indices: vec![0, 1, 2] };
        assert_eq!(validate_mesh_totals(&[good], 20), Some((60, 12)));

        let bad_align = Mesh { texture: 1, clip: [0.0; 4], vertices: vec![0u8; 21], indices: vec![0, 1, 2] };
        assert_eq!(validate_mesh_totals(&[bad_align], 20), None);

        let bad_index = Mesh { texture: 1, clip: [0.0; 4], vertices: vec![0u8; 40], indices: vec![0, 1, 2] };
        assert_eq!(validate_mesh_totals(&[bad_index], 20), None);

        assert_eq!(checked_grow_capacity(100, 64 * 1024), Some(64 * 1024));
        assert_eq!(checked_grow_capacity(usize::MAX, 64 * 1024), None);
    }
}
