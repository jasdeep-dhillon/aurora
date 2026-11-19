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
}
