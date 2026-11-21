//! Optional out-of-process GTK4/libadwaita bridge. No GTK symbols are linked
//! into Aurora's rendering process; the same executable hosts the helper.
use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NativeLibraryColors {
    pub bg: Color32,
    pub chrome: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub play: Color32,
    pub online: Color32,
    pub in_game: Color32,
    pub away: Color32,
    pub danger: Color32,
    pub danger_fill: Color32,
    pub discount: Color32,
    pub view: Color32,
}

impl NativeLibraryColors {
    pub fn apply_to(self, theme: &mut ui::Theme) {
        theme.bg = self.bg;
        theme.chrome = self.chrome;
        theme.surface = self.surface;
        theme.surface_hover = self.surface_hover;
        theme.raised = self.raised;
        theme.border = self.border;
        theme.text = self.text;
        theme.text_dim = self.text_dim;
        theme.text_faint = self.text_faint;
        theme.accent = self.accent;
        theme.accent_2 = self.accent;
        theme.on_accent = self.on_accent;
        theme.play = self.play;
        theme.online = self.online;
        theme.in_game = self.in_game;
        theme.away = self.away;
        theme.danger = self.danger;
        theme.discount = self.discount;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NativeLibraryPalette {
    pub light: NativeLibraryColors,
    pub dark: NativeLibraryColors,
    /// top, right, bottom, left, in GTK logical pixels.
    pub button_padding: [f32; 4],
    pub button_min_height: f32,
    pub gtk_version: [u32; 3],
    pub adwaita_version: [u32; 3],
    /// Native accent fill and standalone light/dark colors, in AdwAccentColor order.
    pub accents: [[Color32; 3]; 9],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeLibrarySnapshot {
    pub palette: NativeLibraryPalette,
    pub dark: bool,
    pub high_contrast: bool,
    pub accent: Color32,
    pub artwork: Option<ui::native_style::NativeControlArtwork>,
}

/// Reject corrupt/unbounded output before publishing native styling.
pub fn parse_helper_output(bytes: &[u8]) -> Result<NativeLibrarySnapshot, String> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("native helper output exceeds limit".into());
    }
    let value: NativeLibrarySnapshot = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !value.palette.button_min_height.is_finite()
        || !(1.0..=256.0).contains(&value.palette.button_min_height)
        || value
            .palette
            .button_padding
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=128.0).contains(v))
    {
        return Err("invalid native control metrics".into());
    }
    if value
        .artwork
        .as_ref()
        .is_some_and(|artwork| !artwork.is_valid())
    {
        return Err("invalid native control artwork".into());
    }
    Ok(value)
}

pub fn run_adwaita_helper() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let snapshot = linux::query()?;
        println!(
            "{}",
            serde_json::to_string(&snapshot).map_err(|e| e.to_string())?
        );
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    Err("libadwaita is only available on Linux".into())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use libloading::Library;
    use std::ffi::{c_char, c_int, c_void};

    // GTK4 GdkRGBA uses floats; GTK3's similarly named structure uses doubles.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rgba {
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Border {
        left: i16,
        right: i16,
        top: i16,
        bottom: i16,
    }

    unsafe fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
        // SAFETY: Each call declares the exact public GTK/libadwaita ABI.
        unsafe {
            library
                .get::<T>(name)
                .map(|value| *value)
                .map_err(|e| e.to_string())
        }
    }

    fn rgba(color: Rgba) -> Result<Color32, String> {
        let channels = [color.r, color.g, color.b, color.a];
        if channels
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("native library returned an invalid color".into());
        }
        let c = channels.map(|v| (v * 255.0).round() as u8);
        Ok(Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
    }

    fn over(foreground: Color32, background: Color32) -> Color32 {
        let f = foreground.to_srgba_unmultiplied();
        let b = background.to_srgba_unmultiplied();
        let alpha = f[3] as f32 / 255.0;
        let c = |i: usize| (f[i] as f32 * alpha + b[i] as f32 * (1.0 - alpha)).round() as u8;
        Color32::from_rgb(c(0), c(1), c(2))
    }

    /// Snapshot GTK's own CSS background/frame at 2×, without showing a window.
    unsafe fn render_button_frames(
        gtk: &Library,
        manager: *mut c_void,
        set_scheme: unsafe extern "C" fn(*mut c_void, c_int),
        scheme: c_int,
    ) -> Result<[ui::native_style::NativeControlFrame; 3], String> {
        // SAFETY: all symbols use their public C declarations; all GTK pointers
        // belong to this helper's main thread and Cairo resources are destroyed.
        unsafe {
            let cairo = Library::new("libcairo.so.2").map_err(|e| e.to_string())?;
            let glib = Library::new("libglib-2.0.so.0").map_err(|e| e.to_string())?;
            let iteration: unsafe extern "C" fn(*mut c_void, c_int) -> c_int =
                symbol(&glib, b"g_main_context_iteration\0")?;
            let pump = || {
                for _ in 0..32 {
                    if iteration(std::ptr::null_mut(), 0) == 0 {
                        break;
                    }
                }
            };
            let snapshot_new: unsafe extern "C" fn() -> *mut c_void =
                symbol(gtk, b"gtk_snapshot_new\0")?;
            let to_node: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                symbol(gtk, b"gtk_snapshot_free_to_node\0")?;
            let background: unsafe extern "C" fn(*mut c_void, *mut c_void, f64, f64, f64, f64) =
                symbol(gtk, b"gtk_snapshot_render_background\0")?;
            let border: unsafe extern "C" fn(*mut c_void, *mut c_void, f64, f64, f64, f64) =
                symbol(gtk, b"gtk_snapshot_render_frame\0")?;
            let set_state: unsafe extern "C" fn(*mut c_void, u32, c_int) =
                symbol(gtk, b"gtk_widget_set_state_flags\0")?;
            let draw: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                symbol(gtk, b"gsk_render_node_draw\0")?;
            let unref: unsafe extern "C" fn(*mut c_void) = symbol(gtk, b"gsk_render_node_unref\0")?;
            let create_surface: unsafe extern "C" fn(c_int, c_int, c_int) -> *mut c_void =
                symbol(&cairo, b"cairo_image_surface_create\0")?;
            let create: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                symbol(&cairo, b"cairo_create\0")?;
            let scale: unsafe extern "C" fn(*mut c_void, f64, f64) =
                symbol(&cairo, b"cairo_scale\0")?;
            let flush: unsafe extern "C" fn(*mut c_void) =
                symbol(&cairo, b"cairo_surface_flush\0")?;
            let data: unsafe extern "C" fn(*mut c_void) -> *const u8 =
                symbol(&cairo, b"cairo_image_surface_get_data\0")?;
            let stride: unsafe extern "C" fn(*mut c_void) -> c_int =
                symbol(&cairo, b"cairo_image_surface_get_stride\0")?;
            let status: unsafe extern "C" fn(*mut c_void) -> c_int =
                symbol(&cairo, b"cairo_surface_status\0")?;
            let destroy: unsafe extern "C" fn(*mut c_void) = symbol(&cairo, b"cairo_destroy\0")?;
            let destroy_surface: unsafe extern "C" fn(*mut c_void) =
                symbol(&cairo, b"cairo_surface_destroy\0")?;
            set_scheme(manager, scheme);
            pump();
            // Adwaita's scheme is scoped to widget roots. A fresh AdwWindow
            // resolves the requested scheme without showing a native window.
            let adw = Library::new("libadwaita-1.so.0").map_err(|e| e.to_string())?;
            let button_new: unsafe extern "C" fn() -> *mut c_void =
                symbol(gtk, b"gtk_button_new\0")?;
            let window_new: unsafe extern "C" fn() -> *mut c_void =
                symbol(&adw, b"adw_window_new\0")?;
            let set_content: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                symbol(&adw, b"adw_window_set_content\0")?;
            let destroy_window: unsafe extern "C" fn(*mut c_void) =
                symbol(gtk, b"gtk_window_destroy\0")?;
            let style_get: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                symbol(gtk, b"gtk_widget_get_style_context\0")?;
            let button = button_new();
            let window = window_new();
            if button.is_null() || window.is_null() {
                return Err("native control creation failed".into());
            }
            set_content(window, button);
            pump();
            let context = style_get(button);
            let mut frames = Vec::with_capacity(3);
            for state in [0, 2, 3] {
                // normal, PRELIGHT, PRELIGHT|ACTIVE
                set_state(button, state, 1);
                pump();
                let snapshot = snapshot_new();
                if snapshot.is_null() {
                    return Err("GTK snapshot creation failed".into());
                }
                background(snapshot, context, 0.0, 0.0, 80.0, 34.0);
                border(snapshot, context, 0.0, 0.0, 80.0, 34.0);
                let node = to_node(snapshot);
                if node.is_null() {
                    return Err("GTK returned an empty button snapshot".into());
                }
                let surface = create_surface(0, 160, 68); // CAIRO_FORMAT_ARGB32
                if surface.is_null() || status(surface) != 0 {
                    unref(node);
                    if !surface.is_null() {
                        destroy_surface(surface);
                    }
                    return Err("Cairo surface creation failed".into());
                }
                let cr = create(surface);
                scale(cr, 2.0, 2.0);
                draw(node, cr);
                flush(surface);
                let base = data(surface);
                let row_stride = stride(surface);
                if base.is_null() || row_stride < 160 * 4 {
                    destroy(cr);
                    destroy_surface(surface);
                    unref(node);
                    return Err("invalid Cairo pixel buffer".into());
                }
                let mut pixels = Vec::with_capacity(160 * 68 * 4);
                for row in 0..68 {
                    let bytes =
                        std::slice::from_raw_parts(base.add(row * row_stride as usize), 160 * 4);
                    for pixel in bytes.chunks_exact(4) {
                        let value = u32::from_ne_bytes(pixel.try_into().unwrap());
                        let alpha = (value >> 24) as u8;
                        let straight = |channel: u8| {
                            if alpha == 0 {
                                0
                            } else {
                                ((u32::from(channel) * 255 + u32::from(alpha) / 2)
                                    / u32::from(alpha))
                                .min(255) as u8
                            }
                        };
                        pixels.extend_from_slice(&[
                            straight((value >> 16) as u8),
                            straight((value >> 8) as u8),
                            straight(value as u8),
                            alpha,
                        ]);
                    }
                }
                destroy(cr);
                destroy_surface(surface);
                unref(node);
                frames.push(ui::native_style::NativeControlFrame {
                    width: 160,
                    height: 68,
                    pixels,
                });
            }
            set_state(button, 0, 1);
            destroy_window(window);
            frames
                .try_into()
                .map_err(|_| "missing native frames".into())
        }
    }

    pub fn query() -> Result<NativeLibrarySnapshot, String> {
        // This function runs only in the short-lived helper's main thread.
        unsafe {
            let gtk = Library::new("libgtk-4.so.1").map_err(|e| e.to_string())?;
            let adw = Library::new("libadwaita-1.so.0").map_err(|e| e.to_string())?;
            let glib = Library::new("libglib-2.0.so.0").map_err(|e| e.to_string())?;
            let gobject = Library::new("libgobject-2.0.so.0").map_err(|e| e.to_string())?;
            let init: unsafe extern "C" fn() -> c_int = symbol(&gtk, b"gtk_init_check\0")?;
            if init() == 0 {
                return Err("GTK4 could not connect to a display".into());
            }
            let adw_init: unsafe extern "C" fn() = symbol(&adw, b"adw_init\0")?;
            adw_init();
            let settings_get: unsafe extern "C" fn() -> *mut c_void =
                symbol(&gtk, b"gtk_settings_get_default\0")?;
            let property_set: unsafe extern "C" fn(*mut c_void, *const c_char, ...) =
                symbol(&gobject, b"g_object_set\0")?;
            property_set(
                settings_get(),
                c"gtk-enable-animations".as_ptr(),
                0 as c_int,
                std::ptr::null::<c_char>(),
            );
            let manager_get: unsafe extern "C" fn() -> *mut c_void =
                symbol(&adw, b"adw_style_manager_get_default\0")?;
            let manager = manager_get();
            if manager.is_null() {
                return Err("missing Adwaita style manager".into());
            }
            let set_scheme: unsafe extern "C" fn(*mut c_void, c_int) =
                symbol(&adw, b"adw_style_manager_set_color_scheme\0")?;
            let get_dark: unsafe extern "C" fn(*mut c_void) -> c_int =
                symbol(&adw, b"adw_style_manager_get_dark\0")?;
            let get_contrast: unsafe extern "C" fn(*mut c_void) -> c_int =
                symbol(&adw, b"adw_style_manager_get_high_contrast\0")?;
            let get_accent: unsafe extern "C" fn(*mut c_void) -> *const Rgba =
                symbol(&adw, b"adw_style_manager_get_accent_color_rgba\0")?;
            let to_standalone: unsafe extern "C" fn(*const Rgba, c_int, *mut Rgba) =
                symbol(&adw, b"adw_rgba_to_standalone\0")?;
            let accent_rgba: unsafe extern "C" fn(c_int, *mut Rgba) =
                symbol(&adw, b"adw_accent_color_to_rgba\0")?;
            let free_rgba: unsafe extern "C" fn(*const Rgba) = symbol(&gtk, b"gdk_rgba_free\0")?;
            let button_new: unsafe extern "C" fn(*const c_char) -> *mut c_void =
                symbol(&gtk, b"gtk_button_new_with_label\0")?;
            let window_new: unsafe extern "C" fn() -> *mut c_void =
                symbol(&adw, b"adw_window_new\0")?;
            let set_child: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                symbol(&adw, b"adw_window_set_content\0")?;
            let style_get: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                symbol(&gtk, b"gtk_widget_get_style_context\0")?;
            let lookup: unsafe extern "C" fn(*mut c_void, *const c_char, *mut Rgba) -> c_int =
                symbol(&gtk, b"gtk_style_context_lookup_color\0")?;
            let padding_get: unsafe extern "C" fn(*mut c_void, *mut Border) =
                symbol(&gtk, b"gtk_style_context_get_padding\0")?;
            let measure: unsafe extern "C" fn(
                *mut c_void,
                c_int,
                c_int,
                *mut c_int,
                *mut c_int,
                *mut c_int,
                *mut c_int,
            ) = symbol(&gtk, b"gtk_widget_measure\0")?;
            let destroy: unsafe extern "C" fn(*mut c_void) = symbol(&gtk, b"gtk_window_destroy\0")?;
            let iteration: unsafe extern "C" fn(*mut c_void, c_int) -> c_int =
                symbol(&glib, b"g_main_context_iteration\0")?;
            let window = window_new();
            let button = button_new(c"Button".as_ptr());
            if window.is_null() || button.is_null() {
                return Err("native widget creation failed".into());
            }
            set_child(window, button);
            let context = style_get(button);
            let pump = || {
                for _ in 0..32 {
                    if iteration(std::ptr::null_mut(), 0) == 0 {
                        break;
                    }
                }
            };
            let read_color = |name: &str| -> Result<Color32, String> {
                let name = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
                let mut value = Rgba::default();
                if lookup(context, name.as_ptr(), &mut value) == 0 {
                    return Err(format!(
                        "Adwaita color unavailable: {}",
                        name.to_string_lossy()
                    ));
                }
                rgba(value)
            };
            let standalone = |color: Color32, dark: bool| -> Result<Color32, String> {
                let c = color.to_srgba_unmultiplied().map(|v| v as f32 / 255.0);
                let input = Rgba {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                    a: c[3],
                };
                let mut output = Rgba::default();
                to_standalone(&input, i32::from(dark), &mut output);
                rgba(output)
            };
            let profile = |scheme: c_int| -> Result<NativeLibraryColors, String> {
                set_scheme(manager, scheme);
                pump();
                let bg = read_color("window_bg_color")?;
                let chrome = read_color("headerbar_bg_color")?;
                let text = read_color("window_fg_color")?;
                let surface = over(read_color("card_bg_color")?, bg);
                let accent = read_color("accent_bg_color")?;
                let play = read_color("success_bg_color")?;
                let danger_fill = read_color("destructive_bg_color")?;
                let dark = scheme == 4;
                Ok(NativeLibraryColors {
                    bg,
                    chrome,
                    surface,
                    surface_hover: over(read_color("card_bg_color")?, surface),
                    raised: read_color("popover_bg_color")?,
                    border: read_color("headerbar_shade_color")?,
                    text,
                    text_dim: text.gamma_multiply(0.55),
                    text_faint: text.gamma_multiply(0.5),
                    accent,
                    accent_text: standalone(accent, dark)?,
                    on_accent: read_color("accent_fg_color")?,
                    play,
                    online: standalone(accent, dark)?,
                    in_game: standalone(play, dark)?,
                    away: standalone(read_color("warning_bg_color")?, dark)?,
                    danger: standalone(danger_fill, dark)?,
                    danger_fill,
                    discount: play,
                    view: read_color("view_bg_color")?,
                })
            };
            // AdwColorScheme FORCE_LIGHT=1 and FORCE_DARK=4 are stable C enum values.
            let light = profile(1)?;
            let dark = profile(4)?;
            let light_artwork = render_button_frames(&gtk, manager, set_scheme, 1)?;
            let dark_artwork = render_button_frames(&gtk, manager, set_scheme, 4)?;
            let mut accents = [[Color32::TRANSPARENT; 3]; 9];
            for (index, colors) in accents.iter_mut().enumerate() {
                let mut native = Rgba::default();
                accent_rgba(index as c_int, &mut native);
                let fill = rgba(native)?;
                *colors = [fill, standalone(fill, false)?, standalone(fill, true)?];
            }
            set_scheme(manager, 0);
            pump();
            let mut padding = Border::default();
            padding_get(context, &mut padding);
            let mut minimum = 0;
            let mut natural = 0;
            measure(
                button,
                1,
                -1,
                &mut minimum,
                &mut natural,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let accent_ptr = get_accent(manager);
            if accent_ptr.is_null() {
                destroy(window);
                return Err("missing native accent".into());
            }
            let accent = rgba(*accent_ptr)?;
            free_rgba(accent_ptr);
            let mut gtk_version = [0; 3];
            let mut adwaita_version = [0; 3];
            for (i, name) in [
                b"gtk_get_major_version\0".as_slice(),
                b"gtk_get_minor_version\0",
                b"gtk_get_micro_version\0",
            ]
            .iter()
            .enumerate()
            {
                let get: unsafe extern "C" fn() -> u32 = symbol(&gtk, name)?;
                gtk_version[i] = get();
            }
            for (i, name) in [
                b"adw_get_major_version\0".as_slice(),
                b"adw_get_minor_version\0",
                b"adw_get_micro_version\0",
            ]
            .iter()
            .enumerate()
            {
                let get: unsafe extern "C" fn() -> u32 = symbol(&adw, name)?;
                adwaita_version[i] = get();
            }
            let snapshot = NativeLibrarySnapshot {
                palette: NativeLibraryPalette {
                    light,
                    dark,
                    button_padding: [
                        padding.top as f32,
                        padding.right as f32,
                        padding.bottom as f32,
                        padding.left as f32,
                    ],
                    button_min_height: minimum.max(natural) as f32,
                    gtk_version,
                    adwaita_version,
                    accents,
                },
                dark: get_dark(manager) != 0,
                high_contrast: get_contrast(manager) != 0,
                accent,
                artwork: Some(ui::native_style::NativeControlArtwork {
                    light: light_artwork,
                    dark: dark_artwork,
                }),
            };
            destroy(window);
            // GTK global objects still reference library code; retain handles until
            // helper process exit rather than unloading beneath those objects.
            std::mem::forget(gtk);
            std::mem::forget(adw);
            std::mem::forget(glib);
            std::mem::forget(gobject);
            parse_helper_output(&serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_native_output_is_rejected() {
        for raw in [
            b"{}".as_slice(),
            b"null",
            b"{\"palette\":{\"button_min_height\":null}}",
            b"not json",
        ] {
            assert!(parse_helper_output(raw).is_err());
        }
        assert!(parse_helper_output(&vec![b' '; 2 * 1024 * 1024 + 1]).is_err());
    }
    #[test]
    #[ignore = "requires installed GTK4/libadwaita and a display"]
    fn installed_library_produces_valid_profiles() {
        #[cfg(target_os = "linux")]
        {
            let snapshot = linux::query().expect("installed native libraries");
            assert_ne!(snapshot.palette.light.bg, snapshot.palette.dark.bg);
            assert!(snapshot.palette.button_min_height >= 24.0);
            let artwork = snapshot.artwork.expect("GTK-rendered controls");
            assert_ne!(
                artwork.light[0], artwork.dark[0],
                "each appearance needs its own widget root"
            );
            assert_ne!(artwork.dark[0], artwork.dark[1], "GTK hover state");
            assert_ne!(artwork.dark[1], artwork.dark[2], "GTK pressed state");
        }
    }
}
