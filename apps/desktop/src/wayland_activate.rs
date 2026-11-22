//! Bringing the main window back on Wayland.
//!
//! winit can't hide, un-minimize or focus a window on Wayland: the compositor
//! only raises a window for an XDG activation token, and the tray host (GNOME's
//! AppIndicator extension, KDE) hands one over with each click. This sends
//! `xdg_activation_v1.activate(token, surface)` for winit's own surface, on
//! winit's Wayland connection with an event queue of its own. The surface is
//! resolved from the current frame each time so remapping can replace it.

use std::ffi::c_void;
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

pub struct Activator {
    conn: Connection,
    activation: XdgActivationV1,
    _queue: EventQueue<State>,
}

pub struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Activator {
    /// Bind xdg-activation on the display connection shared with winit.
    pub fn new(display: &impl raw_window_handle::HasDisplayHandle) -> Option<Self> {
        use raw_window_handle::RawDisplayHandle;
        let RawDisplayHandle::Wayland(d) = display.display_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit owns the display for the lifetime of the app, including
        // while its root window is detached. This backend does not own the display.
        unsafe { Self::from_display(d.display.as_ptr()) }.map_err(|e| tracing::warn!("Wayland activation unavailable: {e}")).ok()
    }

    unsafe fn from_display(display: *mut c_void) -> Result<Self, String> {
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let activation: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).map_err(|e| e.to_string())?;
        Ok(Self { conn, activation, _queue: queue })
    }

    /// Ask the compositor to raise (and un-minimize) the window.
    pub fn activate(&self, frame: &eframe::Frame, token: &str) {
        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::Wayland(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: the borrowed frame keeps the current Wayland window alive.
        let Ok(id) = (unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }) else {
            return;
        };
        let Ok(surface) = WlSurface::from_id(&self.conn, id) else {
            return;
        };
        self.activation.activate(token.to_owned(), &surface);
        if let Err(e) = self.conn.flush() {
            tracing::warn!("Wayland activation failed: {e}");
        }
    }
}
