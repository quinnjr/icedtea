//! The `xdg_popup` role: positioners, grabs, and the destroy-topmost-first
//! chain.

use wayland_client::Proxy;
use wayland_client::protocol::wl_surface;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1;
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface};

pub use xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};

use crate::css::node::Node;
use crate::layout::Rect;
use crate::window::SurfaceError;

/// A popup's identity within one window. Opaque; only [`crate::window::Window`]
/// mints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) u64);

/// The toolkit's positioner, converted to `xdg_positioner` requests on use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Positioner {
    /// In the parent window's frame space.
    pub anchor_rect: Rect,
    pub size: (u32, u32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    pub constraint: ConstraintAdjustment,
    pub offset: (i32, i32),
    pub reactive: bool,
}

impl Positioner {
    /// The shape a menu wants: drop from the anchor's bottom-left, grow down
    /// and right, flip vertically at the screen edge and slide horizontally.
    ///
    /// GTK's own menu positioning, and the reason `ConstraintAdjustment` has a
    /// documented precedence: flip, then slide, then resize.
    #[must_use]
    pub fn menu(anchor_rect: Rect, size: (u32, u32)) -> Self {
        Self {
            anchor_rect,
            size,
            anchor: Anchor::BottomLeft,
            gravity: Gravity::BottomRight,
            constraint: ConstraintAdjustment::FlipY | ConstraintAdjustment::SlideX,
            offset: (0, 0),
            reactive: false,
        }
    }

    /// The protocol's completeness rule.
    ///
    /// A positioner with a zero size or a zero-area anchor rect makes
    /// `xdg_wm_base` raise `invalid_positioner`, which is fatal to the client.
    /// Checking here turns that into a `Result` the caller can handle.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] naming which half is missing.
    pub fn validate(&self) -> Result<(), SurfaceError> {
        if self.size.0 == 0 || self.size.1 == 0 {
            return Err(SurfaceError::Protocol(
                "xdg_positioner needs a non-zero size",
            ));
        }
        let (_, _, w, h) = self.anchor_rect_i32();
        if w < 1 || h < 1 {
            return Err(SurfaceError::Protocol(
                "xdg_positioner needs a non-empty anchor rect",
            ));
        }
        Ok(())
    }

    /// The anchor rect as the protocol's four `i32`s.
    ///
    /// Untrusted arithmetic: the rect comes from a layout pass, and a NaN or
    /// an out-of-range float must become a legal number rather than whatever
    /// `as i32` happens to produce.
    #[must_use]
    pub(crate) fn anchor_rect_i32(&self) -> (i32, i32, i32, i32) {
        let clamp = |v: f32, min: i32| -> i32 {
            if v.is_nan() {
                return min;
            }
            v.clamp(-1.0e9, 1.0e9).round() as i32
        };
        (
            clamp(self.anchor_rect.x, 0),
            clamp(self.anchor_rect.y, 0),
            // No `.max(1)` floor here: `validate` must be able to observe a
            // genuinely zero-area rect (an anchor whose layout collapsed) and
            // refuse it, rather than have this function quietly launder it
            // into the minimum legal size first.
            clamp(self.anchor_rect.width, 1),
            clamp(self.anchor_rect.height, 1),
        )
    }

    /// Build the protocol object and set every rule on it.
    pub(crate) fn apply(&self, positioner: &xdg_positioner::XdgPositioner) {
        let (x, y, w, h) = self.anchor_rect_i32();
        positioner.set_size(
            i32::try_from(self.size.0).unwrap_or(i32::MAX).max(1),
            i32::try_from(self.size.1).unwrap_or(i32::MAX).max(1),
        );
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(self.anchor);
        positioner.set_gravity(self.gravity);
        positioner.set_constraint_adjustment(self.constraint);
        positioner.set_offset(self.offset.0, self.offset.1);
        if self.reactive && positioner.version() >= 3 {
            positioner.set_reactive();
        }
    }
}

/// Where a popup hangs.
#[derive(Debug, Clone)]
pub enum PopupAnchorPoint {
    /// A node of the parent window; its allocation becomes the anchor rect.
    Node(Node),
    Rect(Rect),
}

/// A mapped (or mapping) `xdg_popup`.
pub struct Popup {
    pub(crate) key: PopupKey,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) xdg_popup: xdg_popup::XdgPopup,
    /// The last `xdg_popup.configure` position, relative to the parent's
    /// window geometry.
    pub(crate) position: (i32, i32),
    pub(crate) size: (u32, u32),
    pub(crate) scale: i32,
    /// A handle on the *window's* cursor-shape device, cloned so that
    /// [`Surface::set_cursor_shape`](crate::window::Surface::set_cursor_shape)
    /// can answer for every role from `self` alone, which is the contract's
    /// two-argument signature.
    ///
    /// Deliberately **not** destroyed in [`Drop`]: there is one
    /// `wp_cursor_shape_device_v1` per `wl_pointer`, owned by the window's
    /// own role object ([`Toplevel`](crate::window::toplevel::Toplevel) or
    /// [`Layer`](crate::window::layer::Layer)), and destroying it a second
    /// time when a menu closes would be a protocol error.
    pub(crate) cursor: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
}

impl Popup {
    #[must_use]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.wl_surface
    }
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
    #[must_use]
    pub fn scale(&self) -> i32 {
        self.scale
    }
    /// Where the compositor placed it, after unconstraining.
    #[must_use]
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Take an explicit grab. Must be sent before the first commit, with a
    /// serial from a real input event.
    pub(crate) fn grab(&self, seat: &wayland_client::protocol::wl_seat::WlSeat, serial: u32) {
        self.xdg_popup.grab(seat, serial);
    }
}

impl Drop for Popup {
    /// Role object, then `xdg_surface`, then `wl_surface`. The *chain* order
    /// (topmost first) is `Window::close_popup`'s job -- destroying a popup
    /// that is not the topmost is `xdg_wm_base.error.not_the_topmost_popup`.
    fn drop(&mut self) {
        self.xdg_popup.destroy();
        self.xdg_surface.destroy();
        self.wl_surface.destroy();
    }
}

/// A popup and the retained tree it shows.
///
/// The role object is held as a [`Surface`](crate::window::Surface) rather
/// than as a bare [`Popup`], so that a popup goes through exactly the same
/// `wl_surface`/`size`/`scale`/`states`/`commit_buffer` accessors the other
/// two roles do -- contract §3.1's third `Surface` variant is the one the
/// window layer itself uses, not a decorative arm.
pub(crate) struct PopupWindow {
    /// Duplicated out of `surface` so that the many `find(|p| p.key == key)`
    /// lookups do not have to pattern-match a role they already know.
    pub(crate) key: PopupKey,
    pub(crate) surface: crate::window::Surface,
    pub(crate) root: Node,
    pub(crate) layout: crate::layout::LayoutTree,
    pub(crate) styles: crate::window::StyleMap,
    pub(crate) anim: crate::anim::AnimationState,
    pub(crate) buffers: crate::shm::BufferPool,
    pub(crate) skia: skia_rs_safe::canvas::Surface,
    pub(crate) dirty: bool,
}

impl PopupWindow {
    /// The `xdg_popup` role inside `self.surface`.
    ///
    /// Infallible in practice -- `Window::open_popup` is the only constructor
    /// and always stores `Surface::Popup` -- but an `Option` rather than a
    /// panic, so a future role mix-up degrades to "this popup has no
    /// geometry" instead of killing the client.
    pub(crate) fn popup(&self) -> Option<&Popup> {
        match &self.surface {
            crate::window::Surface::Popup(popup) => Some(popup),
            _ => None,
        }
    }

    pub(crate) fn popup_mut(&mut self) -> Option<&mut Popup> {
        match &mut self.surface {
            crate::window::Surface::Popup(popup) => Some(popup),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Anchor, ConstraintAdjustment, Gravity, PopupAnchorPoint, PopupKey, Positioner};
    use crate::css::node::Node;
    use crate::layout::Rect;
    use crate::window::SurfaceError;

    #[test]
    fn a_menu_positioner_hangs_below_its_anchor_and_flips_when_it_must() {
        let p = Positioner::menu(Rect::new(10.0, 20.0, 80.0, 34.0), (200, 300));
        assert_eq!(
            p.anchor,
            Anchor::BottomLeft,
            "a menu drops from the bottom-left corner"
        );
        assert_eq!(
            p.gravity,
            Gravity::BottomRight,
            "and grows down and to the right"
        );
        assert!(
            p.constraint.contains(ConstraintAdjustment::FlipY),
            "a menu near the bottom flips up"
        );
        assert!(
            p.constraint.contains(ConstraintAdjustment::SlideX),
            "and slides rather than clip"
        );
        assert_eq!(p.size, (200, 300));
        assert!(
            !p.reactive,
            "a menu does not follow its parent; a tooltip would"
        );
    }

    #[test]
    fn a_positioner_the_compositor_would_reject_is_refused_locally() {
        // xdg_wm_base raises `invalid_positioner` and *kills the client* for a
        // positioner with no size or no anchor rect. Refusing locally turns a
        // fatal protocol error into a Result.
        let good = Positioner::menu(Rect::new(0.0, 0.0, 1.0, 1.0), (64, 48));
        assert!(good.validate().is_ok());
        for bad in [
            Positioner {
                size: (0, 48),
                ..good
            },
            Positioner {
                size: (64, 0),
                ..good
            },
            Positioner {
                anchor_rect: Rect::new(0.0, 0.0, 0.0, 10.0),
                ..good
            },
            Positioner {
                anchor_rect: Rect::new(0.0, 0.0, 10.0, 0.0),
                ..good
            },
        ] {
            assert!(
                matches!(bad.validate(), Err(SurfaceError::Protocol(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_hostile_anchor_rect_is_clamped_not_sent() {
        // The anchor rect comes from a node's allocation, which comes from
        // taffy, which can produce a NaN if a widget author hands it one.
        // Sending NaN as an i32 is instant undefined behaviour on the wire.
        for rect in [
            Rect::new(f32::NAN, 0.0, 10.0, 10.0),
            Rect::new(0.0, f32::INFINITY, 10.0, 10.0),
            Rect::new(-1.0e30, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, f32::MAX, f32::MAX),
        ] {
            let p = Positioner {
                anchor_rect: rect,
                ..Positioner::menu(Rect::new(0.0, 0.0, 1.0, 1.0), (64, 48))
            };
            let (x, y, w, h) = p.anchor_rect_i32();
            for value in [x, y, w, h] {
                assert!(value > i32::MIN, "clamped, not wrapped: {value}");
            }
            assert!(
                w >= 1 && h >= 1,
                "a degenerate rect becomes the minimum legal one"
            );
        }
    }

    #[test]
    fn popup_keys_are_unique_and_a_node_anchor_is_accepted() {
        assert_ne!(PopupKey(1), PopupKey(2));
        let node = Node::new("menubutton");
        let anchor = PopupAnchorPoint::Node(node.clone());
        match anchor {
            PopupAnchorPoint::Node(got) => assert!(got.ptr_eq(&node)),
            PopupAnchorPoint::Rect(_) => panic!("wrong variant"),
        }
    }
}
