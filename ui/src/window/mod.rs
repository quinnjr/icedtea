//! The window and event layer: three Wayland surface roles over one
//! connection, keyboard/pointer/touch input, focus and selection.
//!
//! Hand-rolled on `wayland-client` in the shape the rest of this repo uses
//! (`harness/src/lib.rs`, M1's `wayland.rs`): no `smithay-client-toolkit`, no
//! `calloop`. `window/layer.rs` also carries M1's `LayerWindow` unchanged --
//! the `themed-button` demo and its pixel gate still run on it.

pub mod focus;
pub mod keyboard;
pub mod layer;
pub mod pointer;
pub mod popup;
pub mod selection;
pub mod toplevel;

use std::time::Duration;

use wayland_client::{ConnectError, DispatchError};

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;

/// A node's identity, as a key for per-frame caches.
///
/// `selectors::OpaqueElement` is the payload address `LayoutTree` already keys
/// on (`layout.rs:311`), is `Copy + Eq + Hash`, and -- unlike a raw `usize` --
/// is the identity the `selectors` crate itself guarantees is stable for as
/// long as any handle to the node lives.
pub use selectors::OpaqueElement as NodeAddr;

/// `node`'s [`NodeAddr`].
#[must_use]
pub fn node_addr(node: &Node) -> NodeAddr {
    <Node as selectors::Element>::opaque(node)
}

/// One frame's computed styles, keyed by node.
///
/// Rebuilt by [`Window::render`] on every restyle and handed to
/// [`pointer::hit_test`] by reference; P4 takes over producing it.
pub type StyleMap = std::collections::HashMap<NodeAddr, std::rc::Rc<ComputedStyle>>;

/// Cascade, compute and lay out the whole tree.
///
/// Iterative, not recursive: the tree comes from a reconciler and a widget
/// author, and a 10,000-deep one must be a slow frame rather than a stack
/// overflow. Each node is resolved against its parent's computed style
/// (`ComputedStyle::resolve`, not `resolve_chain`) because the walk already
/// holds it -- `resolve_chain` would re-walk the ancestors for every node,
/// turning a linear pass quadratic.
///
/// `styles` is cleared first: a node removed since the last frame must not
/// keep a stale entry that [`pointer::hit_test`] would then read.
///
/// # Errors
///
/// [`crate::layout::LayoutError`] from `LayoutTree::sync`/`compute`.
pub fn restyle(
    root: &Node,
    sheet: &crate::css::cascade::CompiledSheet,
    env: &crate::css::computed::ResolveEnv,
    styles: &mut StyleMap,
    tree: &mut crate::layout::LayoutTree,
    available: taffy::Size<taffy::AvailableSpace>,
    measure: &mut dyn crate::layout::Measure,
) -> Result<(), crate::layout::LayoutError> {
    styles.clear();
    tree.sync(root)?;
    let mut cx = crate::css::select::MatchCx::new();
    let mut stack: Vec<(Node, Option<std::rc::Rc<ComputedStyle>>)> = vec![(root.clone(), None)];
    while let Some((node, parent)) = stack.pop() {
        let computed = std::rc::Rc::new(ComputedStyle::resolve(
            sheet,
            &node,
            parent.as_deref(),
            env,
            &mut cx,
        ));
        // M2's `Container` has exactly two variants; P6 widens it (contract
        // §3.6) and this is the one place that chooses. `Container::default()`
        // is `BoxDirection::Column`, which is right for a single-child
        // container (a `window`'s lone child), but GTK's own `GtkBox`
        // defaults its `orientation` property to horizontal -- P6's
        // `BoxC { orientation, .. }` (contract) will carry this explicitly;
        // until then a node literally named `box` gets GTK's real default
        // rather than this pass's generic one.
        let container = if node.child_count() == 0 {
            crate::layout::Container::Leaf
        } else if &*node.name() == "box" {
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Row,
            }
        } else {
            crate::layout::Container::default()
        };
        tree.set_style(&node, &computed, container, env);
        styles.insert(node_addr(&node), std::rc::Rc::clone(&computed));
        for child in node.children() {
            stack.push((child, Some(std::rc::Rc::clone(&computed))));
        }
    }
    tree.compute(root, available, measure)
}

bitflags::bitflags! {
    /// `xdg_toplevel.configure` states, plus the layer/popup analogues.
    ///
    /// A layer surface and a popup have no states at all, so theirs is always
    /// empty -- which is what makes `!ACTIVATED` mean `:backdrop` only for a
    /// toplevel.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct SurfaceStates: u16 {
        /// `xdg_toplevel.state.maximized`
        const MAXIMIZED = 1 << 0;
        /// `xdg_toplevel.state.fullscreen`
        const FULLSCREEN = 1 << 1;
        /// `xdg_toplevel.state.resizing`
        const RESIZING = 1 << 2;
        /// `xdg_toplevel.state.activated`. Clear => `:backdrop` on the root.
        const ACTIVATED = 1 << 3;
        /// `xdg_toplevel.state.tiled_left`
        const TILED_LEFT = 1 << 4;
        /// `xdg_toplevel.state.tiled_right`
        const TILED_RIGHT = 1 << 5;
        /// `xdg_toplevel.state.tiled_top`
        const TILED_TOP = 1 << 6;
        /// `xdg_toplevel.state.tiled_bottom`
        const TILED_BOTTOM = 1 << 7;
        /// `xdg_toplevel.state.suspended`
        const SUSPENDED = 1 << 8;
    }
}

impl SurfaceStates {
    /// Decode `xdg_toplevel.configure`'s `states` array.
    ///
    /// The wire carries a packed array of native-endian `u32`s, straight from
    /// the compositor, so this is an untrusted decode: a ragged tail is
    /// dropped rather than read past, and an unknown state (xdg-shell 7's
    /// `constrained_*`, or anything a future version adds) is ignored rather
    /// than folded into some arbitrary flag.
    #[must_use]
    pub fn from_wire(states: &[u8]) -> SurfaceStates {
        let mut out = SurfaceStates::empty();
        for chunk in states.chunks_exact(4) {
            let raw = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            out |= match raw {
                1 => SurfaceStates::MAXIMIZED,
                2 => SurfaceStates::FULLSCREEN,
                3 => SurfaceStates::RESIZING,
                4 => SurfaceStates::ACTIVATED,
                5 => SurfaceStates::TILED_LEFT,
                6 => SurfaceStates::TILED_RIGHT,
                7 => SurfaceStates::TILED_TOP,
                8 => SurfaceStates::TILED_BOTTOM,
                9 => SurfaceStates::SUSPENDED,
                _ => SurfaceStates::empty(),
            };
        }
        out
    }
}

/// Everything that can go wrong opening, running or closing a surface.
///
/// A superset of M1's `LayerWindowError`: the nine carried variants keep their
/// names, payloads and messages, so `wayland::LayerWindowError` is a type alias
/// for this (contract §8.1) and M1's own error test still passes verbatim.
#[derive(Debug)]
pub enum SurfaceError {
    /// `wl_display` connection failed.
    Connect(ConnectError),
    /// The compositor never advertised a global the client requires.
    MissingGlobal(&'static str),
    /// Allocating or uploading a `wl_shm` buffer failed.
    Shm(std::io::Error),
    /// The Wayland socket itself failed: a flush, a poll, or a read.
    Socket(std::io::Error),
    /// A Skia surface could not be allocated.
    Render(&'static str),
    /// No usable UI typeface is installed.
    NoFont,
    /// The event queue failed.
    Dispatch(DispatchError),
    /// The compositor closed the surface before it ever configured it.
    Closed,
    /// The compositor did not configure the surface in time.
    Timeout(Duration),
    /// The client would have violated the protocol; the request was refused
    /// locally rather than sent (a positioner with no size, a popup destroyed
    /// out of order).
    Protocol(&'static str),
    /// The compositor's keymap could not be mapped or compiled.
    Keymap,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "cannot connect to the Wayland display: {e}"),
            Self::MissingGlobal(name) => write!(f, "compositor did not advertise {name}"),
            Self::Shm(e) => write!(f, "cannot allocate or upload the shm buffer: {e}"),
            Self::Socket(e) => write!(f, "the Wayland socket failed: {e}"),
            Self::Render(what) => write!(f, "cannot allocate {what}"),
            Self::NoFont => write!(
                f,
                "no UI typeface found; install dejavu, liberation or noto sans"
            ),
            Self::Dispatch(e) => write!(f, "Wayland dispatch failed: {e}"),
            Self::Closed => write!(f, "the compositor closed the surface before configuring it"),
            Self::Timeout(after) => write!(
                f,
                "the compositor did not configure the surface within {after:?}"
            ),
            Self::Protocol(what) => write!(f, "refusing to send an invalid request: {what}"),
            Self::Keymap => write!(f, "the compositor's keymap could not be compiled"),
        }
    }
}

impl std::error::Error for SurfaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) => Some(e),
            Self::Shm(e) | Self::Socket(e) => Some(e),
            Self::Dispatch(e) => Some(e),
            Self::MissingGlobal(_)
            | Self::Render(_)
            | Self::NoFont
            | Self::Closed
            | Self::Timeout(_)
            | Self::Protocol(_)
            | Self::Keymap => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SurfaceError, SurfaceStates, node_addr};
    use crate::css::node::Node;
    use std::error::Error;
    use std::time::Duration;

    /// The `xdg_toplevel::State` values, as the protocol numbers them.
    fn wire(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn states_from_the_wire_decode_every_flag() {
        assert_eq!(SurfaceStates::from_wire(&wire(&[])), SurfaceStates::empty());
        assert_eq!(
            SurfaceStates::from_wire(&wire(&[1, 2, 3, 4])),
            SurfaceStates::MAXIMIZED
                | SurfaceStates::FULLSCREEN
                | SurfaceStates::RESIZING
                | SurfaceStates::ACTIVATED
        );
        assert_eq!(
            SurfaceStates::from_wire(&wire(&[5, 6, 7, 8, 9])),
            SurfaceStates::TILED_LEFT
                | SurfaceStates::TILED_RIGHT
                | SurfaceStates::TILED_TOP
                | SurfaceStates::TILED_BOTTOM
                | SurfaceStates::SUSPENDED
        );
        assert!(
            !SurfaceStates::from_wire(&wire(&[4])).contains(SurfaceStates::MAXIMIZED),
            "activated must not imply maximized"
        );
    }

    #[test]
    fn a_hostile_states_array_never_panics() {
        // The compositor hands this straight to us: a short tail, an unknown
        // state (xdg-shell 7 adds constrained_* = 10..=13, and a future
        // version will add more) and a 64 KiB array all have to be survivable.
        for bytes in [
            vec![0u8],
            vec![0u8, 0, 0],
            vec![0xFF; 7],
            wire(&[10, 11, 12, 13, 999, u32::MAX]),
            vec![0u8; 65_536],
        ] {
            let states = SurfaceStates::from_wire(&bytes);
            assert!(
                SurfaceStates::all().contains(states),
                "from_wire invented a flag outside SurfaceStates::all()"
            );
        }
        // A trailing partial u32 is dropped, not read out of bounds.
        let mut ragged = wire(&[4]);
        ragged.push(9);
        assert_eq!(SurfaceStates::from_wire(&ragged), SurfaceStates::ACTIVATED);
    }

    #[test]
    fn every_surface_error_says_what_actually_failed() {
        let cases: Vec<(SurfaceError, &str)> = vec![
            (
                SurfaceError::Shm(std::io::Error::other("ENOSPC")),
                "shm buffer",
            ),
            (
                SurfaceError::Socket(std::io::Error::other("EPIPE")),
                "Wayland socket",
            ),
            (SurfaceError::Render("the raster surface"), "raster"),
            (SurfaceError::NoFont, "typeface"),
            (SurfaceError::Closed, "closed"),
            (
                SurfaceError::Timeout(Duration::from_secs(5)),
                "did not configure",
            ),
            (
                SurfaceError::MissingGlobal("zwlr_layer_shell_v1"),
                "zwlr_layer_shell_v1",
            ),
            (
                SurfaceError::Protocol("xdg_positioner needs a size"),
                "xdg_positioner",
            ),
            (SurfaceError::Keymap, "keymap"),
        ];
        for (error, expected) in &cases {
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "{error:?} reads {message:?}, which does not mention {expected:?}"
            );
        }
        assert!(
            SurfaceError::Shm(std::io::Error::other("ENOSPC"))
                .source()
                .is_some(),
            "the underlying io::Error is not reachable"
        );
        assert!(SurfaceError::NoFont.source().is_none());
        assert!(SurfaceError::Keymap.source().is_none());
    }

    #[test]
    fn node_addr_is_a_stable_per_node_identity() {
        let a = Node::new("button");
        let b = Node::new("button");
        assert_eq!(
            node_addr(&a),
            node_addr(&a.clone()),
            "a handle is the same node"
        );
        assert_ne!(node_addr(&a), node_addr(&b), "same name, different node");
        // Identity survives mutation: a StyleMap keyed on it must not lose
        // entries when a class is added mid-frame.
        let before = node_addr(&a);
        a.add_class("flat");
        assert_eq!(before, node_addr(&a));
    }
}
