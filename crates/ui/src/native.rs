//! Opening the app window with a renderer this PC can run.
//!
//! The app draws with OpenGL (egui_glow). Some Windows PCs can't give it
//! OpenGL 2.0: no GPU driver installed (Windows' built-in OpenGL is 1.1), or
//! older/odd hardware whose driver only does Direct3D. There, if OpenGL fails
//! before the app starts, the window opens again with Direct3D 12 (wgpu).
//! Direct3D 12 also runs on DX11-class GPUs (feature level 11_0 and up), and
//! without any GPU driver Windows' software renderer (WARP) takes over.

/// Like [`eframe::run_native`], with the Direct3D 12 fallback on Windows.
pub fn run<'a>(app_name: &str, options: eframe::NativeOptions, app_creator: eframe::AppCreator<'a>) -> eframe::Result {
    #[cfg(windows)]
    {
        use std::{cell::RefCell, rc::Rc};
        // The creator runs once, in whichever attempt gets a working window.
        let creator = Rc::new(RefCell::new(Some(app_creator)));
        let attempt = || -> eframe::AppCreator<'a> {
            let creator = creator.clone();
            Box::new(move |cc| (creator.borrow_mut().take().expect("app created twice"))(cc))
        };
        // With both renderers built in, eframe would pick wgpu: OpenGL stays
        // the first choice (it's what every working PC has been using).
        let opengl = eframe::NativeOptions { renderer: eframe::Renderer::Glow, ..options.clone() };
        let opengl = eframe::run_native(app_name, opengl, attempt());
        // An error after the app started isn't about the renderer.
        if opengl.is_ok() || creator.borrow().is_none() {
            return opengl;
        }
        if let Err(e) = &opengl {
            log::warn!("OpenGL couldn't start ({e}), using Direct3D 12 instead");
        }
        let options = eframe::NativeOptions { renderer: eframe::Renderer::Wgpu, wgpu_options: d3d12(), ..options };
        eframe::run_native(app_name, options, attempt())
    }
    #[cfg(not(windows))]
    eframe::run_native(app_name, options, app_creator)
}

/// wgpu on Direct3D 12 only, preferring a real GPU over WARP.
#[cfg(windows)]
fn d3d12() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
    use eframe::wgpu;
    let mut config = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        // WGPU_BACKEND still overrides it, for testing.
        setup.instance_descriptor.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
        setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
            let usable: Vec<_> = adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
            for a in &usable {
                log::info!("Direct3D adapter: {:?}", a.get_info());
            }
            usable
                .iter()
                .find(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
                .or(usable.first())
                .map(|a| (*a).clone())
                .ok_or_else(|| "no Direct3D 12 adapter can draw to this window".to_owned())
        }));
    }
    config
}
