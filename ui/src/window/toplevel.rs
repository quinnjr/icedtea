//! The `xdg_toplevel` surface role.

use wayland_client::protocol::{wl_seat, wl_surface};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel};

use crate::window::SurfaceStates;

/// A mapped (or mapping) `xdg_toplevel`.
pub struct Toplevel {
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) xdg_toplevel: xdg_toplevel::XdgToplevel,
    /// The configured size in surface-local pixels. Synced from
    /// `WindowState` by `Window::open`/`Window::pump` on every configure --
    /// dispatch handlers only ever see `WindowState`, never this struct, so
    /// they cannot write it themselves.
    pub(crate) size: (u32, u32),
    pub(crate) scale: i32,
    pub(crate) states: SurfaceStates,
    /// The cursor-shape device for this window's pointer, if the compositor
    /// offers `wp_cursor_shape_manager_v1` and a pointer exists. One device
    /// per window rather than per-role: there is exactly one `wl_pointer`,
    /// and the shape it draws does not depend on which of the window's
    /// surfaces has focus.
    pub(crate) cursor: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
}

impl Toplevel {
    pub fn set_title(&self, title: &str) {
        self.xdg_toplevel.set_title(title.to_owned());
    }

    pub fn set_app_id(&self, app_id: &str) {
        self.xdg_toplevel.set_app_id(app_id.to_owned());
    }

    /// Both sizes are in *window geometry*, not buffer pixels.
    pub fn set_min_size(&self, size: (u32, u32)) {
        self.xdg_toplevel
            .set_min_size(clamp_i32(size.0), clamp_i32(size.1));
    }

    pub fn set_max_size(&self, size: (u32, u32)) {
        self.xdg_toplevel
            .set_max_size(clamp_i32(size.0), clamp_i32(size.1));
    }

    pub fn set_maximized(&self, on: bool) {
        if on {
            self.xdg_toplevel.set_maximized();
        } else {
            self.xdg_toplevel.unset_maximized();
        }
    }

    pub fn set_fullscreen(&self, on: bool) {
        if on {
            self.xdg_toplevel.set_fullscreen(None);
        } else {
            self.xdg_toplevel.unset_fullscreen();
        }
    }

    pub fn set_minimized(&self) {
        self.xdg_toplevel.set_minimized();
    }

    /// An interactive move. The serial must come from a real input event.
    pub fn move_(&self, seat: &wl_seat::WlSeat, serial: u32) {
        self.xdg_toplevel._move(seat, serial);
    }

    pub fn resize(&self, seat: &wl_seat::WlSeat, serial: u32, edges: xdg_toplevel::ResizeEdge) {
        self.xdg_toplevel.resize(seat, serial, edges);
    }

    pub fn show_window_menu(&self, seat: &wl_seat::WlSeat, serial: u32, at: (i32, i32)) {
        self.xdg_toplevel.show_window_menu(seat, serial, at.0, at.1);
    }

    /// The window-geometry rect the compositor should treat as the window.
    ///
    /// Our buffer is the *ink* rect, which can start outside the border box
    /// (an outset shadow), so the geometry is the visible frame inside it.
    // Task 12's render pipeline is the only caller; it lives here because the
    // `xdg_surface` handle is this role's private business.
    pub(crate) fn set_window_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        self.xdg_surface
            .set_window_geometry(x, y, width.max(1), height.max(1));
    }
}

impl Drop for Toplevel {
    /// Role object first, then the `xdg_surface`, then the `wl_surface`:
    /// destroying an `xdg_surface` that still has a role raises
    /// `defunct_role_object`. The cursor-shape device has no such ordering
    /// requirement -- it is tied to the `wl_pointer`, not this surface.
    fn drop(&mut self) {
        if let Some(cursor) = self.cursor.take() {
            cursor.destroy();
        }
        self.xdg_toplevel.destroy();
        self.xdg_surface.destroy();
        self.wl_surface.destroy();
    }
}

/// A `u32` size as the `i32` the protocol wants. A size that does not fit is a
/// caller bug, not a protocol error: clamp rather than wrap into a negative.
fn clamp_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
