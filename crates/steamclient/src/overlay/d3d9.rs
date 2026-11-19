//! Drawing the overlay inside a D3D9 / D3D9Ex game (native Windows and Proton/DXVK).
//!
//! Once `d3d9.dll` is loaded by the game, we create a throwaway D3D9 (or
//! D3D9Ex) device on a hidden window to read its vtable, then patch `Reset`
//! (16), `Present` (17), and for D3D9Ex `PresentEx` (121) and `ResetEx` (132)
//! using W^X-safe vtable patching.
//!
//! Before drawing our egui triangles in `Present` / `PresentEx`, we capture
//! the game's state into an `IDirect3DStateBlock9` (`D3DSBT_ALL`) plus the
//! active render target, depth-stencil surface, viewport, and scissor rect,
//! render using pre-transformed screen-space vertices (`D3DFVF_XYZRHW`) via
//! `DrawIndexedPrimitiveUP`, and restore every state afterwards.
//!
//! A CPU-side `TexShadow` mirrors all egui textures so `Reset` / `ResetEx`
//! releases every `D3DPOOL_DEFAULT` resource cleanly and rebuilds them on the
//! next frame without losing the font atlas or icons.

use super::util::{self, Backend, TexShadow};
use super::{Frame, TexOp, shared};
use crate::overlay::wire::VERTEX_SIZE;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D9::*;
use windows::core::{HRESULT, Interface, s, w};

type ResetFn = unsafe extern "system" fn(*mut c_void, *mut D3DPRESENT_PARAMETERS) -> HRESULT;
type PresentFn = unsafe extern "system" fn(*mut c_void, *const RECT, *const RECT, HWND, *const c_void) -> HRESULT;
type PresentExFn = unsafe extern "system" fn(*mut c_void, *const RECT, *const RECT, HWND, *const c_void, u32) -> HRESULT;
type ResetExFn = unsafe extern "system" fn(*mut c_void, *mut D3DPRESENT_PARAMETERS, *mut D3DDISPLAYMODEEX) -> HRESULT;

static REAL_RESET: AtomicUsize = AtomicUsize::new(0);
static REAL_PRESENT: AtomicUsize = AtomicUsize::new(0);
static REAL_PRESENT_EX: AtomicUsize = AtomicUsize::new(0);
static REAL_RESET_EX: AtomicUsize = AtomicUsize::new(0);

/// `IDirect3DDevice9` / `IDirect3DDevice9Ex` vtable slots.
const SLOT_RESET: usize = 16;
const SLOT_PRESENT: usize = 17;
const SLOT_PRESENT_EX: usize = 121;
const SLOT_RESET_EX: usize = 132;

const D3D_SDK_VERSION: u32 = 32;
const D3DUSAGE_DYNAMIC: u32 = 0x200;

static RENDERER: Mutex<Option<D3d9Renderer>> = Mutex::new(None);

/// Install D3D9/D3D9Ex vtable hooks if `d3d9.dll` is loaded in the process.
/// Safe to call repeatedly.
pub fn install_if_loaded() -> bool {
    if REAL_PRESENT.load(Ordering::Acquire) != 0 {
        return true;
    }
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPED};
        let Ok(module) = GetModuleHandleW(w!("d3d9.dll")) else {
            return false;
        };
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Aurora D3D9 probe"),
            WS_OVERLAPPED,
            0,
            0,
            64,
            64,
            None,
            None,
            None,
            None,
        ) else {
            return false;
        };

        let mut pp = D3DPRESENT_PARAMETERS {
            BackBufferWidth: 64,
            BackBufferHeight: 64,
            BackBufferFormat: D3DFMT_UNKNOWN,
            BackBufferCount: 1,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: hwnd,
            Windowed: true.into(),
            ..Default::default()
        };

        // Prefer Direct3DCreate9Ex so we also capture PresentEx / ResetEx slots.
        type Create9ExFn = unsafe extern "system" fn(u32, *mut Option<IDirect3D9Ex>) -> HRESULT;
        type Create9Fn = unsafe extern "system" fn(u32) -> *mut c_void;

        let mut patched = false;
        if let Some(p) = GetProcAddress(module, s!("Direct3DCreate9Ex")) {
            let create_ex: Create9ExFn = std::mem::transmute(p);
            let mut d3d9ex: Option<IDirect3D9Ex> = None;
            if create_ex(D3D_SDK_VERSION, &mut d3d9ex).is_ok() {
                if let Some(d3d) = d3d9ex {
                    for flags in [D3DCREATE_HARDWARE_VERTEXPROCESSING | D3DCREATE_NOWINDOWCHANGES, D3DCREATE_SOFTWARE_VERTEXPROCESSING | D3DCREATE_NOWINDOWCHANGES] {
                        let mut dev: Option<IDirect3DDevice9Ex> = None;
                        if d3d.CreateDeviceEx(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, hwnd, flags as u32, &mut pp, std::ptr::null_mut(), &mut dev).is_ok() {
                            if let Some(dev) = dev {
                                let vtable = *(dev.as_raw() as *mut *mut usize);
                                util::patch_vtable(vtable, SLOT_RESET, hook_reset as *const () as usize, &REAL_RESET);
                                util::patch_vtable(vtable, SLOT_RESET_EX, hook_reset_ex as *const () as usize, &REAL_RESET_EX);
                                util::patch_vtable(vtable, SLOT_PRESENT_EX, hook_present_ex as *const () as usize, &REAL_PRESENT_EX);
                                patched = util::patch_vtable(vtable, SLOT_PRESENT, hook_present as *const () as usize, &REAL_PRESENT);
                                break;
                            }
                        }
                    }
                }
            }
        }

        if !patched {
            if let Some(p) = GetProcAddress(module, s!("Direct3DCreate9")) {
                let create: Create9Fn = std::mem::transmute(p);
                let raw = create(D3D_SDK_VERSION);
                if !raw.is_null() {
                    let d3d = IDirect3D9::from_raw(raw);
                    for flags in [D3DCREATE_HARDWARE_VERTEXPROCESSING | D3DCREATE_NOWINDOWCHANGES, D3DCREATE_SOFTWARE_VERTEXPROCESSING | D3DCREATE_NOWINDOWCHANGES] {
                        let mut dev: Option<IDirect3DDevice9> = None;
                        if d3d.CreateDevice(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, hwnd, flags as u32, &mut pp, &mut dev).is_ok() {
                            if let Some(dev) = dev {
                                let vtable = *(dev.as_raw() as *mut *mut usize);
                                util::patch_vtable(vtable, SLOT_RESET, hook_reset as *const () as usize, &REAL_RESET);
                                patched = util::patch_vtable(vtable, SLOT_PRESENT, hook_present as *const () as usize, &REAL_PRESENT);
                                break;
                            }
                        }
                    }
                }
            }
        }

        let _ = DestroyWindow(hwnd);
        if patched {
            crate::log!("overlay: D3D9 hooks in");
        }
        patched
    }
}

unsafe extern "system" fn hook_present(this: *mut c_void, src: *const RECT, dst: *const RECT, override_win: HWND, dirty: *const c_void) -> HRESULT {
    let _ = std::panic::catch_unwind(|| unsafe { present_inner(this, override_win) });
    let real: PresentFn = unsafe { std::mem::transmute(REAL_PRESENT.load(Ordering::Acquire)) };
    unsafe { real(this, src, dst, override_win, dirty) }
}

unsafe extern "system" fn hook_present_ex(this: *mut c_void, src: *const RECT, dst: *const RECT, override_win: HWND, dirty: *const c_void, flags: u32) -> HRESULT {
    let _ = std::panic::catch_unwind(|| unsafe { present_inner(this, override_win) });
    let real: PresentExFn = unsafe { std::mem::transmute(REAL_PRESENT_EX.load(Ordering::Acquire)) };
    unsafe { real(this, src, dst, override_win, dirty, flags) }
}

unsafe extern "system" fn hook_reset(this: *mut c_void, pp: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    before_reset(this);
    let real: ResetFn = unsafe { std::mem::transmute(REAL_RESET.load(Ordering::Acquire)) };
    unsafe { real(this, pp) }
}

unsafe extern "system" fn hook_reset_ex(this: *mut c_void, pp: *mut D3DPRESENT_PARAMETERS, mode: *mut D3DDISPLAYMODEEX) -> HRESULT {
    before_reset(this);
    let real: ResetExFn = unsafe { std::mem::transmute(REAL_RESET_EX.load(Ordering::Acquire)) };
    unsafe { real(this, pp, mode) }
}

fn before_reset(this: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut guard) = RENDERER.lock() {
            if let Some(r) = guard.as_mut().filter(|r| r.device_raw == this as usize) {
                r.release_gpu_resources();
            }
        }
    });
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D3d9Vertex {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    col: u32,
    u: f32,
    v: f32,
}

const FVF_OVERLAY: u32 = D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1;

struct D3d9Tex {
    texture: IDirect3DTexture9,
    linear: bool,
}

struct D3d9Renderer {
    device_raw: usize,
    device: IDirect3DDevice9,
    hwnd: HWND,
    size: (u32, u32),
    state_block: Option<IDirect3DStateBlock9>,
    textures: HashMap<u64, D3d9Tex>,
    shadow: TexShadow,
    verts: Vec<D3d9Vertex>,
    last_present: Instant,
}

// Accessed only under RENDERER's mutex.
unsafe impl Send for D3d9Renderer {}

unsafe fn backbuffer_info(dev: &IDirect3DDevice9, override_win: HWND) -> Option<(HWND, (u32, u32))> {
    unsafe {
        let mut params = D3DDEVICE_CREATION_PARAMETERS::default();
        let hwnd = if !override_win.is_invalid() {
            override_win
        } else if dev.GetCreationParameters(&mut params).is_ok() {
            params.hFocusWindow
        } else {
            HWND::default()
        };
        let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO).ok()?;
        let mut desc = D3DSURFACE_DESC::default();
        bb.GetDesc(&mut desc).ok()?;
        if desc.Width == 0 || desc.Height == 0 {
            return None;
        }
        Some((hwnd, (desc.Width, desc.Height)))
    }
}

unsafe fn present_inner(this: *mut c_void, override_win: HWND) {
    let active = util::active_backend();
    if active != Backend::None && active != Backend::D3d9 {
        return;
    }
    let Some(dev) = (unsafe { IDirect3DDevice9::from_raw_borrowed(&this) }) else { return };
    let Ok(mut guard) = RENDERER.lock() else { return };

    if let Some(r) = guard.as_ref() {
        if r.device_raw != this as usize {
            if r.last_present.elapsed().as_secs_f32() < 1.0 {
                return;
            }
            crate::log!("overlay: D3D9 following a new device");
            *guard = None;
        }
    }

    if guard.is_none() {
        let Some((hwnd, size)) = (unsafe { backbuffer_info(dev, override_win) }) else { return };
        if !util::claim_backend(Backend::D3d9) {
            return;
        }
        crate::log!("overlay: drawing into the game's D3D9 device ({}x{})", size.0, size.1);
        super::input::attach(hwnd);
        super::hooked(size);
        *guard = Some(D3d9Renderer {
            device_raw: this as usize,
            device: dev.clone(),
            hwnd,
            size,
            state_block: None,
            textures: HashMap::new(),
            shadow: TexShadow::new(),
            verts: Vec::new(),
            last_present: Instant::now(),
        });
    }

    let r = guard.as_mut().expect("just set");
    r.last_present = Instant::now();
    if let Some((hwnd, size)) = unsafe { backbuffer_info(dev, override_win) } {
        if size != r.size {
            r.size = size;
            super::resized(size);
        }
        if !hwnd.is_invalid() && hwnd != r.hwnd {
            r.hwnd = hwnd;
            super::input::attach(r.hwnd);
        }
    }

    let (ops, frame) = shared::take_for_present();
    for op in ops {
        unsafe { r.apply(op) };
    }
    if let Some(frame) = frame.filter(|f| !f.meshes.is_empty()) {
        if let Err(e) = unsafe { r.draw(&frame) } {
            crate::log!("overlay: D3D9 draw failed: {e}");
        }
    }
}

impl D3d9Renderer {
    fn release_gpu_resources(&mut self) {
        self.state_block = None;
        self.textures.clear();
    }

    unsafe fn upload_shadow(&mut self, id: u64) {
        let Some(entry) = self.shadow.get(id) else { return };
        let [w, h] = entry.size;
        let bgra = util::rgba_to_bgra(&entry.rgba);
        let linear = entry.linear;
        unsafe {
            let mut tex: Option<IDirect3DTexture9> = None;
            if self
                .device
                .CreateTexture(w, h, 1, D3DUSAGE_DYNAMIC, D3DFMT_A8R8G8B8, D3DPOOL_DEFAULT, &mut tex, std::ptr::null_mut())
                .is_err()
            {
                return;
            }
            let Some(tex) = tex else { return };
            let mut locked = D3DLOCKED_RECT::default();
            if tex.LockRect(0, &mut locked, std::ptr::null(), D3DLOCK_DISCARD as u32).is_ok() {
                let row_bytes = (w as usize) * 4;
                for y in 0..h as usize {
                    std::ptr::copy_nonoverlapping(
                        bgra.as_ptr().add(y * row_bytes),
                        (locked.pBits as *mut u8).add(y * locked.Pitch as usize),
                        row_bytes,
                    );
                }
                let _ = tex.UnlockRect(0);
                self.textures.insert(id, D3d9Tex { texture: tex, linear });
            }
        }
    }

    unsafe fn apply(&mut self, op: TexOp) {
        match op {
            TexOp::Free(id) => {
                self.shadow.free(id);
                self.textures.remove(&id);
            }
            TexOp::Set(t) => {
                if self.shadow.set(t.id, t.pos, t.size, t.linear, &t.rgba) {
                    unsafe { self.upload_shadow(t.id) };
                }
            }
        }
    }

    unsafe fn ensure_gpu_resources(&mut self) -> windows::core::Result<()> {
        unsafe {
            if self.state_block.is_none() {
                self.state_block = Some(self.device.CreateStateBlock(D3DSBT_ALL)?);
            }
            // Rebuild any textures dropped during Reset from the CPU shadow store.
            let missing: Vec<u64> = self
                .shadow
                .iter()
                .filter_map(|(id, _)| (!self.textures.contains_key(&id)).then_some(id))
                .collect();
            for id in missing {
                self.upload_shadow(id);
            }
            Ok(())
        }
    }

    unsafe fn draw(&mut self, frame: &Frame) -> windows::core::Result<()> {
        unsafe {
            self.ensure_gpu_resources()?;
            let Some(sb) = self.state_block.clone() else { return Ok(()) };
            let dev = &self.device;

            // Capture full D3D9 state plus RT/DSV/viewport/scissor.
            sb.Capture()?;
            let prev_rt = dev.GetRenderTarget(0).ok();
            let prev_dsv = dev.GetDepthStencilSurface().ok();
            let mut prev_vp = D3DVIEWPORT9::default();
            let has_vp = dev.GetViewport(&mut prev_vp).is_ok();
            let mut prev_scissor = RECT::default();
            let has_scissor = dev.GetScissorRect(&mut prev_scissor).is_ok();

            let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
            let _ = dev.SetRenderTarget(0, &bb);
            let _ = dev.SetDepthStencilSurface(None::<&IDirect3DSurface9>);

            let vp = D3DVIEWPORT9 { X: 0, Y: 0, Width: self.size.0, Height: self.size.1, MinZ: 0.0, MaxZ: 1.0 };
            let _ = dev.SetViewport(&vp);

            let _ = dev.SetPixelShader(None::<&IDirect3DPixelShader9>);
            let _ = dev.SetVertexShader(None::<&IDirect3DVertexShader9>);
            let _ = dev.SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID.0 as u32);
            let _ = dev.SetRenderState(D3DRS_SHADEMODE, D3DSHADE_GOURAUD.0 as u32);
            let _ = dev.SetRenderState(D3DRS_ZENABLE, 0);
            let _ = dev.SetRenderState(D3DRS_ZWRITEENABLE, 0);
            let _ = dev.SetRenderState(D3DRS_ALPHATESTENABLE, 0);
            let _ = dev.SetRenderState(D3DRS_CULLMODE, D3DCULL_NONE.0 as u32);
            let _ = dev.SetRenderState(D3DRS_LIGHTING, 0);
            let _ = dev.SetRenderState(D3DRS_FOGENABLE, 0);
            let _ = dev.SetRenderState(D3DRS_SRGBWRITEENABLE, 0);
            let _ = dev.SetRenderState(D3DRS_SCISSORTESTENABLE, 1);
            let _ = dev.SetRenderState(D3DRS_ALPHABLENDENABLE, 1);
            let _ = dev.SetRenderState(D3DRS_BLENDOP, D3DBLENDOP_ADD.0 as u32);
            let _ = dev.SetRenderState(D3DRS_SRCBLEND, D3DBLEND_ONE.0 as u32);
            let _ = dev.SetRenderState(D3DRS_DESTBLEND, D3DBLEND_INVSRCALPHA.0 as u32);
            let _ = dev.SetRenderState(D3DRS_SEPARATEALPHABLENDENABLE, 1);
            let _ = dev.SetRenderState(D3DRS_BLENDOPALPHA, D3DBLENDOP_ADD.0 as u32);
            let _ = dev.SetRenderState(D3DRS_SRCBLENDALPHA, D3DBLEND_INVDESTALPHA.0 as u32);
            let _ = dev.SetRenderState(D3DRS_DESTBLENDALPHA, D3DBLEND_ONE.0 as u32);

            let _ = dev.SetTextureStageState(0, D3DTSS_COLOROP, D3DTOP_MODULATE.0 as u32);
            let _ = dev.SetTextureStageState(0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
            let _ = dev.SetTextureStageState(0, D3DTSS_COLORARG2, D3DTA_DIFFUSE);
            let _ = dev.SetTextureStageState(0, D3DTSS_ALPHAOP, D3DTOP_MODULATE.0 as u32);
            let _ = dev.SetTextureStageState(0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
            let _ = dev.SetTextureStageState(0, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE);
            let _ = dev.SetTextureStageState(1, D3DTSS_COLOROP, D3DTOP_DISABLE.0 as u32);
            let _ = dev.SetTextureStageState(1, D3DTSS_ALPHAOP, D3DTOP_DISABLE.0 as u32);

            let _ = dev.SetSamplerState(0, D3DSAMP_ADDRESSU, D3DTADDRESS_CLAMP.0 as u32);
            let _ = dev.SetSamplerState(0, D3DSAMP_ADDRESSV, D3DTADDRESS_CLAMP.0 as u32);
            let _ = dev.SetFVF(FVF_OVERLAY);

            let ppp = frame.pixels_per_point.max(0.1);
            let begun = dev.BeginScene().is_ok();
            for m in &frame.meshes {
                let tri_count = (m.indices.len() / 3) as u32;
                if tri_count == 0 || m.vertices.len() < VERTEX_SIZE {
                    continue;
                }
                let Some([left, top, right, bottom]) = util::clip_rect_pixels(m.clip, ppp, self.size.0, self.size.1) else {
                    continue;
                };
                let Some(tex) = self.textures.get(&m.texture) else { continue };

                self.verts.clear();
                for chunk in m.vertices.chunks_exact(VERTEX_SIZE) {
                    if let Some((x, y, u, v, col)) = util::unpack_egui_vertex(chunk) {
                        self.verts.push(D3d9Vertex {
                            x: x * ppp - 0.5,
                            y: y * ppp - 0.5,
                            z: 0.0,
                            rhw: 1.0,
                            col: util::rgba_to_d3dcolor(col),
                            u,
                            v,
                        });
                    }
                }
                if self.verts.is_empty() {
                    continue;
                }

                let filter = if tex.linear { D3DTEXF_LINEAR.0 as u32 } else { D3DTEXF_POINT.0 as u32 };
                let _ = dev.SetSamplerState(0, D3DSAMP_MINFILTER, filter);
                let _ = dev.SetSamplerState(0, D3DSAMP_MAGFILTER, filter);
                let _ = dev.SetSamplerState(0, D3DSAMP_MIPFILTER, D3DTEXF_NONE.0 as u32);
                let base_tex: IDirect3DBaseTexture9 = tex.texture.cast()?;
                let _ = dev.SetTexture(0, &base_tex);
                let scissor = RECT { left, top, right, bottom };
                let _ = dev.SetScissorRect(&scissor);
                let _ = dev.DrawIndexedPrimitiveUP(
                    D3DPT_TRIANGLELIST,
                    0,
                    self.verts.len() as u32,
                    tri_count,
                    m.indices.as_ptr().cast(),
                    D3DFMT_INDEX32,
                    self.verts.as_ptr().cast(),
                    size_of::<D3d9Vertex>() as u32,
                );
            }
            if begun {
                let _ = dev.EndScene();
            }

            // Restore game state.
            let _ = sb.Apply();
            if let Some(rt) = prev_rt {
                let _ = dev.SetRenderTarget(0, &rt);
            }
            let _ = dev.SetDepthStencilSurface(prev_dsv.as_ref());
            if has_vp {
                let _ = dev.SetViewport(&prev_vp);
            }
            if has_scissor {
                let _ = dev.SetScissorRect(&prev_scissor);
            }
            Ok(())
        }
    }
}
