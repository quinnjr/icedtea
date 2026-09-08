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

pub use layer::BTN_LEFT;
pub use pointer::{BTN_MIDDLE, BTN_RIGHT};

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_data_device_manager, wl_keyboard, wl_pointer,
    wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_touch,
};
use wayland_client::{
    ConnectError, Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum,
    delegate_noop,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1;
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::anim::{AnimationState, Clock, MonotonicClock};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::shm::{BufferPool, BufferSlot};
use crate::text::FontDatabase;
use keyboard::{KeyEvent, Keymap};
use pointer::{CursorShape, Scroll, ScrollSource};
use popup::{Popup, PopupAnchorPoint, PopupKey, PopupWindow, Positioner};
use selection::{Clipboard, ClipboardShared};

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
        let mut computed = ComputedStyle::resolve(sheet, &node, parent.as_deref(), env, &mut cx);
        // The root is the surface: a real GTK toplevel's own widget
        // allocation is always exactly the compositor-configured size, never
        // derived from its content -- and this registry has no `width`/
        // `height` CSS property to express that with (deviations 9/10), so
        // it is forced the same way the pointer.rs fixture's `window {
        // min-width/min-height }` does it by hand: an `anim::Overrides` floor
        // on `min-width`/`min-height`, which `box_sizing: ContentBox` (an
        // AUTO size) turns into "at least the available space". A `Node`
        // used only as a layout fixture (never restyled through `Window`,
        // e.g. `pointer.rs`'s own tests) is unaffected when it already
        // declares the same floor; one that does not now gets it for free,
        // which is the point.
        if parent.is_none() {
            let mut root_floor = crate::anim::Overrides::default();
            if let taffy::AvailableSpace::Definite(w) = available.width {
                root_floor.set(
                    crate::css::registry::Prop::MinWidth,
                    crate::css::value::Value::Length(crate::css::value::Length::px(w)),
                );
            }
            if let taffy::AvailableSpace::Definite(h) = available.height {
                root_floor.set(
                    crate::css::registry::Prop::MinHeight,
                    crate::css::value::Value::Length(crate::css::value::Length::px(h)),
                );
            }
            if !root_floor.is_empty() {
                computed = computed.with_overrides(&root_floor).into_owned();
            }
        }
        let computed = std::rc::Rc::new(computed);
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

/// Which of a window's surfaces an event arrived on.
///
/// Contract deviation 6: a popup is a separate `wl_surface` with its own
/// pointer and keyboard focus, and the contract's flat `InputEvent` cannot say
/// which one an event is for. The two `Enter` variants carry it; everything
/// after an enter belongs to that surface until the matching leave, which is
/// how Wayland itself defines focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTarget {
    Window,
    Popup(PopupKey),
}

/// What to open.
#[derive(Debug, Clone)]
pub struct SurfaceSpec {
    pub role: Role,
    /// The initial buffer size in surface-local pixels.
    pub size: (u32, u32),
    pub title: String,
    pub app_id: String,
}

/// The role, and the role-specific state that has to be set before the first
/// commit.
#[derive(Debug, Clone)]
pub enum Role {
    Toplevel,
    Layer(LayerSpec),
}

/// `zwlr_layer_surface_v1` state, all of it double-buffered and so settable
/// only before the first commit.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub layer: zwlr_layer_shell_v1::Layer,
    pub anchor: zwlr_layer_surface_v1::Anchor,
    /// top, right, bottom, left.
    pub margin: [i32; 4],
    pub exclusive_zone: i32,
    pub keyboard: zwlr_layer_surface_v1::KeyboardInteractivity,
}

/// Where a surface is in xdg-shell's mapping sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPhase {
    /// The role object exists; nothing has been committed.
    Created,
    /// The empty commit went out; the first `configure` has not come back.
    AwaitingConfigure,
    /// A `configure` arrived and was acked: buffers may be attached.
    Configured,
}

/// Whether a buffer may be attached yet.
///
/// xdg-shell's sequencing rule, in one place: role object -> role state ->
/// **commit with no buffer** -> `configure` -> `ack_configure` -> attach.
/// Attaching before the first configure is
/// `xdg_surface.error.unconfigured_buffer`, which kills the client.
#[must_use]
pub fn may_attach(phase: MapPhase) -> bool {
    matches!(phase, MapPhase::Configured)
}

/// Apply a configure's states to the root node.
///
/// `!ACTIVATED` is `:backdrop`, which is the only one of the nine that GTK's
/// stylesheet reads. The rest are the app's business (a maximised window hides
/// its resize grip) and reach it through `InputEvent::Configure`.
/// Copy `state`'s configure-derived fields onto the active surface.
///
/// Dispatch handlers (`impl Dispatch<_, _> for WindowState`) only ever see
/// `&mut WindowState`, never the `Surface` a live `Window` wraps it in, so
/// nothing else can keep `Surface::{size, scale, states}` current. Called
/// once after `Window::open`'s first configure and again from every
/// `Window::pump` that saw a `Configure` or `ScaleChanged` event.
fn sync_surface_from_state(surface: &mut Surface, state: &WindowState) {
    match surface {
        Surface::Toplevel(t) => {
            if let Some(size) = state.configured {
                t.size = size;
            }
            t.states = state.states;
            t.scale = state.scale;
        }
        Surface::Layer(l) => {
            if let Some(size) = state.configured {
                l.size = size;
            }
            l.scale = state.scale;
        }
        // A popup's geometry never comes from `WindowState::configured` --
        // `xdg_popup.configure` carries its own position and size, which
        // `Window::render_popups` folds in per popup.
        Surface::Popup(_) => {}
    }
}

pub(crate) fn apply_configure_states(root: &Node, states: SurfaceStates) {
    root.set_state(
        PseudoStates::BACKDROP,
        !states.contains(SurfaceStates::ACTIVATED),
    );
}

/// One of the three surface roles.
///
/// Every role answers the same seven accessors (`wl_surface`, `size`, `scale`,
/// `states`, `resize`, `set_cursor_shape`, `commit_buffer`); a window's own
/// surface is a `Toplevel` or a `Layer`, and each of its open popups is a
/// `Popup` held by a
/// `PopupWindow` (`crate::window::popup::PopupWindow`).
pub enum Surface {
    Toplevel(toplevel::Toplevel),
    Layer(layer::Layer),
    Popup(popup::Popup),
}

impl Surface {
    #[must_use]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            Self::Toplevel(t) => &t.wl_surface,
            Self::Layer(l) => l.wl_surface(),
            Self::Popup(p) => p.wl_surface(),
        }
    }

    /// The configured size in surface-local pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Toplevel(t) => t.size,
            Self::Layer(l) => l.size(),
            Self::Popup(p) => p.size(),
        }
    }

    #[must_use]
    pub fn scale(&self) -> i32 {
        match self {
            Self::Toplevel(t) => t.scale,
            Self::Layer(l) => l.scale(),
            Self::Popup(p) => p.scale(),
        }
    }

    #[must_use]
    pub fn states(&self) -> SurfaceStates {
        match self {
            Self::Toplevel(t) => t.states,
            // Neither a layer surface nor a popup carries an
            // `xdg_toplevel.state`; the dispatch in
            // `impl Dispatch<zwlr_layer_surface_v1::...>` always reports
            // ACTIVATED on configure, and a mapped popup is by definition the
            // thing the user is interacting with -- both keep their root out
            // of `:backdrop`.
            Self::Layer(_) | Self::Popup(_) => SurfaceStates::ACTIVATED,
        }
    }

    /// Ask for a new size.
    ///
    /// Honoured for a toplevel (`set_window_geometry`) and for a layer surface
    /// (`set_size`); **ignored for a popup**, whose size is the positioner's
    /// until a reposition (contract §3.1).
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] for a zero dimension, which every role
    /// rejects -- including the popup, so that a caller learns its request was
    /// nonsense rather than merely inapplicable.
    pub fn resize(&mut self, size: (u32, u32)) -> Result<(), SurfaceError> {
        if size.0 == 0 || size.1 == 0 {
            return Err(SurfaceError::Protocol("a surface size must be positive"));
        }
        match self {
            Self::Toplevel(t) => {
                t.size = size;
                let width = i32::try_from(size.0).unwrap_or(i32::MAX);
                let height = i32::try_from(size.1).unwrap_or(i32::MAX);
                t.set_window_geometry(0, 0, width, height);
            }
            Self::Layer(l) => l.set_size(size),
            Self::Popup(_) => {}
        }
        Ok(())
    }

    /// Which of a window's surfaces this is, for the `Dispatch` user-data a
    /// frame callback and an `xdg_surface.configure` are routed by.
    pub(crate) fn target(&self) -> SurfaceTarget {
        match self {
            Self::Toplevel(_) | Self::Layer(_) => SurfaceTarget::Window,
            Self::Popup(p) => SurfaceTarget::Popup(p.key),
        }
    }

    /// Name the cursor for this surface's pointer.
    ///
    /// A no-op without `wp_cursor_shape_v1`; client-side cursor themes are out
    /// of scope, so there is no fallback path to a `wl_surface` cursor.
    ///
    /// There is exactly one `wp_cursor_shape_device_v1` per window (one
    /// `wl_pointer`, bound by `Window::open`), and the shape it draws does not
    /// depend on which of the window's surfaces has focus; each role holds a
    /// handle on it so that this can read the device off `self`, and only the
    /// window's own role destroys it. See [`Window::set_cursor_shape`].
    pub fn set_cursor_shape(&mut self, shape: CursorShape, serial: u32) {
        let device = match self {
            Self::Toplevel(t) => t.cursor.as_ref(),
            Self::Layer(l) => l.cursor.as_ref(),
            Self::Popup(p) => p.cursor.as_ref(),
        };
        if let Some(device) = device {
            device.set_shape(serial, shape);
        }
    }

    /// Upload, attach, damage and commit, requesting a frame callback.
    ///
    /// `buffers`, `shm` and `qh` are parameters rather than fields `self`
    /// owns: `WindowState`'s own doc comment already settles this -- "the
    /// pools live on `Window`, and one window has several" -- so a `Surface`
    /// borrows the pool for the duration of one commit rather than owning it.
    /// `pub(crate)`, not `pub`, because `QueueHandle<WindowState>` is
    /// unnameable outside this crate. Both departures from contract §3.1's
    /// `pub fn commit_buffer(&mut self, skia)` are recorded as amendment
    /// **P3-A** in the M3 Part 0 contract's §10.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] if no buffer is free and the pool cannot grow, or
    /// if the upload fails.
    pub(crate) fn commit_buffer(
        &mut self,
        skia: &mut skia_rs_safe::canvas::Surface,
        buffers: &mut BufferPool,
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<WindowState>,
    ) -> Result<(), SurfaceError> {
        let target = self.target();
        commit_buffer_to(self.wl_surface(), target, skia, buffers, shm, qh)
    }
}

/// Upload, attach, damage and commit onto `wl_surface`, requesting a frame
/// callback.
///
/// The [`Surface`]-shaped half of a commit, factored out so a popup's own
/// `wl_surface` (which is not a [`Surface`] -- a window can have several
/// popups, each with its own pool, and `Surface` is one-per-window) can share
/// it with [`Surface::commit_buffer`] rather than duplicating the sequence.
///
/// # Errors
///
/// [`SurfaceError::Shm`] if no buffer is free and the pool cannot grow, or if
/// the upload fails.
pub(crate) fn commit_buffer_to(
    wl_surface: &wl_surface::WlSurface,
    target: SurfaceTarget,
    skia: &mut skia_rs_safe::canvas::Surface,
    buffers: &mut BufferPool,
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<WindowState>,
) -> Result<(), SurfaceError> {
    let Some(index) = buffers.acquire(shm, qh).map_err(SurfaceError::Shm)? else {
        // Every buffer is still held: the frame is deferred, exactly as
        // M1's `LayerWindow::repaint` defers it, and the next release
        // repaints.
        tracing::debug!("every shm buffer is still held; deferring this frame");
        return Ok(());
    };
    let (width, height) = buffers.size();
    buffers.upload(index, skia).map_err(SurfaceError::Shm)?;
    wl_surface.attach(Some(buffers.wl_buffer(index)), 0, 0);
    wl_surface.damage_buffer(0, 0, width, height);
    // The frame request is double-buffered like the attach, so it is only
    // sent on the path that actually commits: a deferred frame (above) must
    // not leave a request queued that the *next* commit would then apply
    // twice. `target` is what routes the callback back to the surface that
    // asked for it -- `InputEvent::Frame`'s only producer.
    wl_surface.frame(qh, target);
    wl_surface.commit();
    Ok(())
}

/// Everything the window layer hands upward. One flat enum; the reactive layer
/// (contract §4) is the only consumer.
#[derive(Debug, Clone)]
pub enum InputEvent {
    PointerEnter {
        x: f64,
        y: f64,
        serial: u32,
        /// Contract deviation 6.
        target: SurfaceTarget,
    },
    PointerMotion {
        x: f64,
        y: f64,
        time_ms: u32,
    },
    PointerLeave,
    PointerButton {
        button: u32,
        pressed: bool,
        serial: u32,
        time_ms: u32,
    },
    Scroll(Scroll),
    TouchDown {
        id: i32,
        x: f64,
        y: f64,
        serial: u32,
        time_ms: u32,
    },
    TouchMotion {
        id: i32,
        x: f64,
        y: f64,
        time_ms: u32,
    },
    TouchUp {
        id: i32,
        serial: u32,
        time_ms: u32,
    },
    KeyboardEnter {
        serial: u32,
        /// Contract deviation 6.
        target: SurfaceTarget,
    },
    KeyboardLeave,
    Key(KeyEvent),
    Configure {
        size: (u32, u32),
        states: SurfaceStates,
    },
    ScaleChanged(i32),
    Frame {
        now: Duration,
    },
    Close,
    PopupDone(PopupKey),
    Repositioned {
        key: PopupKey,
        token: u32,
    },
    SelectionChanged,
    PrimaryChanged,
    /// A watched fd is ready.
    ///
    /// Carries no readiness flags on purpose: the owner of the fd is the only
    /// thing that knows what to do with it, and re-encoding `revents` here
    /// would invite the toolkit to interpret a foreign protocol. One event per
    /// ready watch per [`Window::pump`] batch, in registration order, after
    /// whatever Wayland events the same wake dispatched.
    FdReady(WatchId),
}

impl InputEvent {
    /// `PointerEnter` on the main window surface -- the only target an
    /// offscreen or single-surface script can mean (contract §11 E3).
    #[must_use]
    pub fn pointer_enter(x: f64, y: f64, serial: u32) -> InputEvent {
        InputEvent::PointerEnter {
            x,
            y,
            serial,
            target: SurfaceTarget::Window,
        }
    }

    /// `KeyboardEnter` on the main window surface (contract §11 E3).
    #[must_use]
    pub fn keyboard_enter(serial: u32) -> InputEvent {
        InputEvent::KeyboardEnter {
            serial,
            target: SurfaceTarget::Window,
        }
    }
}

/// One `xdg_popup.configure`: which popup, its new position, its new size.
pub(crate) type PopupConfigure = (PopupKey, (i32, i32), (u32, u32));

/// Everything the dispatch handlers write and [`Window`] reads.
pub(crate) struct WindowState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    /// The layer role's shell global.
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// The seat `move`/`resize`/`show_window_menu` and the clipboard use.
    seat: Option<wl_seat::WlSeat>,
    /// The three input objects the seat-capability handler creates.
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    touch: Option<wl_touch::WlTouch>,
    /// Read by `Surface::set_cursor_shape`.
    cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    /// Read by `Window::clipboard`.
    data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    /// Read by `Window::clipboard`; primary selection is a convenience some
    /// compositors omit, so its absence is not fatal (deviation-free reading
    /// of contract §3.8 -- only `wl_data_device_manager` is fail-fast).
    primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    /// The offer/source bookkeeping [`selection::Clipboard`]'s `Dispatch`
    /// impls write into. `None` until [`Window::clipboard`] builds the
    /// device(s) that can ever receive such events -- nothing arrives before
    /// then.
    clipboard_shared: Option<Rc<RefCell<ClipboardShared>>>,
    /// Compiled from `wl_keyboard.keymap`; `None` until it arrives.
    keymap: Option<Keymap>,
    /// The events `pump` will hand up, in arrival order.
    events: Vec<InputEvent>,
    /// Buffers the compositor released, by `wl_buffer` id: one window has
    /// several pools and `BufferSlot` alone cannot say which. Drained by the
    /// repaint.
    released: Vec<(wayland_client::backend::ObjectId, BufferSlot)>,
    /// The most recent serial from any input event, for `grab`, `move`,
    /// `resize` and the clipboard.
    seat_serial: Option<u32>,
    /// The last `xdg_surface.configure` serial per surface, to ack on commit.
    pending_ack: Vec<(SurfaceTarget, u32)>,
    configured: Option<(u32, u32)>,
    /// The size the client asked for, from `SurfaceSpec::size`.
    ///
    /// The fallback for a `configure` with a zero dimension, which every
    /// xdg-shell compositor is entitled to send and which means "you choose".
    /// Without it the *first* such configure has no previous size to keep and
    /// collapses the window to 1x1 -- the size `Window::open`'s `.max(1)`
    /// floor produces from a `Some((0, 0))`.
    default_size: (u32, u32),
    states: SurfaceStates,
    /// The output scale the compositor last announced through
    /// `wl_surface.preferred_buffer_scale`.
    ///
    /// Read by [`sync_surface_from_state`] (so every role's `scale()` agrees
    /// with it) and by [`Window::open`], which seeds the icon theme's
    /// rasterisation scale from it; later changes arrive as
    /// [`InputEvent::ScaleChanged`] and go through [`Window::pump`].
    scale: i32,
    closed: bool,
    /// A pending scroll frame, accumulated until `wl_pointer.frame`.
    axis: Option<Scroll>,
    /// Set by Task 14's popup dispatch.
    #[allow(dead_code)]
    pub(crate) popup_events: Vec<InputEvent>,
    /// `xdg_popup.configure`'s position and size, per popup, since the last
    /// drain. `Window::render` folds these into the matching `PopupWindow`
    /// before it paints; a popup's own `xdg_surface.configure` (which only
    /// carries a serial) is acked generically above and does not need one.
    pub(crate) popup_configures: Vec<PopupConfigure>,
    /// Which [`SurfaceTarget`] each of this window's `wl_surface`s is, so
    /// `wl_pointer.enter`/`wl_keyboard.enter` (which only ever name a
    /// surface) can be resolved back to one. Pushed by `Window::open` for the
    /// main surface and by Task 14's `open_popup` for each popup.
    pub(crate) surface_targets: Vec<(wayland_client::backend::ObjectId, SurfaceTarget)>,
}

impl WindowState {
    fn new() -> Self {
        Self {
            compositor: None,
            shm: None,
            wm_base: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            keyboard: None,
            touch: None,
            cursor_manager: None,
            data_device_manager: None,
            primary_manager: None,
            clipboard_shared: None,
            keymap: None,
            events: Vec::new(),
            released: Vec::new(),
            seat_serial: None,
            pending_ack: Vec::new(),
            configured: None,
            default_size: (1, 1),
            states: SurfaceStates::empty(),
            scale: 1,
            closed: false,
            axis: None,
            popup_events: Vec::new(),
            popup_configures: Vec::new(),
            surface_targets: Vec::new(),
        }
    }

    /// A state with no connection behind it, for the bounded-wait tests.
    #[cfg(test)]
    fn new_for_test() -> Self {
        Self::new()
    }

    /// Record that the compositor closed the surface. Idempotent: a `Closed`
    /// from the layer surface and a destroy on the way out must not queue two
    /// `InputEvent::Close`s.
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.events.push(InputEvent::Close);
    }

    /// Which of this window's surfaces `surface` is.
    ///
    /// `wl_pointer.enter` names the surface; a popup is a different one, and
    /// the whole point of [`SurfaceTarget`] is not to guess.
    fn target_of(&self, surface: &wl_surface::WlSurface) -> SurfaceTarget {
        self.surface_targets
            .iter()
            .find(|(id, _)| *id == surface.id())
            .map_or(SurfaceTarget::Window, |(_, target)| *target)
    }
}

/// Opaque handle for one fd registered with [`Window::watch_fd`].
///
/// Ids are minted per window and never reused, so a stale id from a dropped
/// watch can never name a later one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

/// What a watch waits for.
///
/// `Read` is readability, `Write` writability, `ReadWrite` either. `HUP` and
/// `ERR` are always reported whatever the interest, and always arrive as an
/// [`InputEvent::FdReady`] — the toolkit never decides on its own that a
/// foreign fd is dead. It reports, and the owner calls [`Window::unwatch`].
///
/// By design an `FdReady` says only that the fd is ready, not which bit fired:
/// a `ReadWrite` (or hung-up) watcher must probe the fd itself to learn read
/// vs write vs EOF. A fd left at `POLLHUP` re-fires every poll, so the owner
/// MUST `unwatch` on EOF/HUP (`wait_bounded` warns once when one does not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interest {
    /// Readable, or hung up.
    Read,
    /// Writable.
    Write,
    /// Either.
    ReadWrite,
}

impl Interest {
    fn flags(self) -> rustix::event::PollFlags {
        use rustix::event::PollFlags;
        match self {
            Interest::Read => PollFlags::IN,
            Interest::Write => PollFlags::OUT,
            Interest::ReadWrite => PollFlags::IN | PollFlags::OUT,
        }
    }
}

/// How many consecutive poll passes a watch may fire on before the loop warns
/// that it is likely a hung-up fd the app is not unwatching. At ~60 fps this is
/// about ten seconds of a fd stuck at `POLLHUP` re-firing every frame.
const WATCH_HUNG_WARN_FRAMES: u32 = 600;

/// One registered fd: the window owns it until [`Window::unwatch`] drops it.
struct Watch {
    id: WatchId,
    fd: std::os::fd::OwnedFd,
    flags: rustix::event::PollFlags,
    /// Consecutive poll passes this watch has fired on. A fd hung up at
    /// `POLLHUP` re-reports ready every poll forever (the toolkit never
    /// unwatches on the owner's behalf — M5-D1 §2), so this climbs until the
    /// owner `unwatch`es. `Cell` so [`wait_bounded`] can bump it through the
    /// `&Watches` it already holds, without a signature change.
    fired_count: std::cell::Cell<u32>,
    /// Whether the hung-fd warning has already fired for the current run of
    /// consecutive fires; reset once the watch goes quiet, so a genuinely busy
    /// fd is not warned about twice for one stall.
    warned: std::cell::Cell<bool>,
}

/// Every live watch, in registration order.
#[derive(Default)]
struct Watches {
    entries: Vec<Watch>,
    next: u64,
}

impl Watches {
    fn add(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId {
        let id = WatchId(self.next);
        self.next += 1;
        self.entries.push(Watch {
            id,
            fd,
            flags: interest.flags(),
            fired_count: std::cell::Cell::new(0),
            warned: std::cell::Cell::new(false),
        });
        id
    }

    /// Drop the watch and close its fd. An unknown id is a no-op, not a panic:
    /// an owner that unwatches twice is not a protocol error (M5-D1 §4).
    fn remove(&mut self, id: WatchId) {
        self.entries.retain(|w| w.id != id);
    }

    fn ids(&self) -> Vec<WatchId> {
        self.entries.iter().map(|w| w.id).collect()
    }

    fn entries(&self) -> impl Iterator<Item = &Watch> {
        self.entries.iter()
    }
}

/// Block until `ready`, the surface closes, or `timeout` expires.
///
/// wayland-client 0.31's bounded-wait shape: dispatch what is queued,
/// `prepare_read` to register interest, `poll(2)` the connection fd for what is
/// left of the deadline, then read. No helper thread, and no
/// `blocking_dispatch` that could outlive the deadline.
fn wait_bounded(
    conn: &Connection,
    queue: &mut EventQueue<WindowState>,
    state: &mut WindowState,
    timeout: Duration,
    // M5-D1: the extra fds this window's owner registered. Empty for
    // `Window::open`, which has no owner yet.
    watches: &Watches,
    ready: impl Fn(&WindowState) -> bool,
) -> Result<(), SurfaceError> {
    use rustix::event::{PollFlags, Timespec};
    use std::os::fd::AsFd;

    // `Duration::MAX` is `pump(None)`'s "block until something happens", and
    // `Instant + Duration::MAX` panics: an unrepresentable deadline is `None`,
    // which polls without a timespec rather than overflowing.
    let deadline = Instant::now().checked_add(timeout);
    // M5-D1: watches that fired on the *previous* poll, published after the
    // dispatch below so a batch reads "Wayland events first, then FdReady",
    // which is the order M5-D1 §3 fixes.
    let mut fired: Vec<WatchId> = Vec::new();
    // A `Duration::ZERO` wait is "drain what is there", not "do nothing": the
    // deadline is already past on the first pass, and returning `Timeout`
    // there — as this used to — never flushed, never read the Wayland socket
    // and never polled a single watched fd. One controller reporting a zero
    // deadline (`GridViewC` did so unconditionally) then pinned the loop on
    // that early return and starved input, frame callbacks and every
    // `App::on_fd`/inbox watch. The first expiry therefore still runs one
    // non-blocking poll pass; the second time round is the real timeout.
    let mut expired = false;
    loop {
        queue
            .dispatch_pending(state)
            .map_err(SurfaceError::Dispatch)?;
        state
            .events
            .extend(fired.drain(..).map(InputEvent::FdReady));
        if state.closed {
            return Err(SurfaceError::Closed);
        }
        if ready(state) {
            return Ok(());
        }

        let now = Instant::now();
        let remaining = match deadline {
            Some(deadline) if now >= deadline => {
                if std::mem::replace(&mut expired, true) {
                    return Err(SurfaceError::Timeout(timeout));
                }
                Some(Duration::ZERO)
            }
            Some(deadline) => Some(deadline - now),
            None => None,
        };

        conn.flush().map_err(socket_error)?;
        // `None` means this queue already has events to dispatch: go round
        // rather than block on a socket that has nothing more to say.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };

        let fd = queue.as_fd();
        // M5-D1: element 0 is always the Wayland queue's fd; elements 1..
        // are the watches in registration order, which is what makes
        // `fds[index + 1]` name `watches.entries().nth(index)`.
        let mut fds = Vec::with_capacity(1 + watches.entries().count());
        fds.push(rustix::event::PollFd::new(&fd, PollFlags::IN));
        for watch in watches.entries() {
            fds.push(rustix::event::PollFd::new(&watch.fd, watch.flags));
        }
        let timespec = remaining.map(|remaining| Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        });
        match rustix::event::poll(&mut fds, timespec.as_ref()) {
            Ok(0) => return Err(SurfaceError::Timeout(timeout)),
            Ok(_) => {
                // M5-D1: a wake caused only by a watch must not read the
                // Wayland socket — `guard.read()` on a socket with nothing to
                // say is at best a wasted syscall and at worst a block.
                // Dropping the guard cancels the read intent.
                if fds[0].revents().is_empty() {
                    drop(guard);
                } else {
                    match guard.read() {
                        Ok(_) => {}
                        // A racing reader on another queue drained the socket.
                        Err(wayland_client::backend::WaylandError::Io(err))
                            if err.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(err) => return Err(socket_error(err)),
                    }
                }
                // M5-D1 §2/§3: one `FdReady` per ready watch, whatever bits
                // are set (`HUP`/`ERR` included, which poll reports without
                // being asked). Published at the top of the next iteration.
                for (index, watch) in watches.entries().enumerate() {
                    if fds[index + 1].revents().is_empty() {
                        // Quiet this pass: a later run of fires starts fresh
                        // and may warn again.
                        watch.fired_count.set(0);
                        watch.warned.set(false);
                        continue;
                    }
                    fired.push(watch.id);
                    // Cheap observability for a hung-up fd (`POLLHUP` re-fires
                    // every poll): count consecutive fires and warn once when
                    // the run gets long. The loop still keeps reporting — the
                    // owner alone decides to `unwatch` — this only makes a
                    // silent busy-loop visible.
                    let count = watch.fired_count.get().saturating_add(1);
                    watch.fired_count.set(count);
                    if count >= WATCH_HUNG_WARN_FRAMES && !watch.warned.get() {
                        watch.warned.set(true);
                        tracing::warn!(
                            id = ?watch.id,
                            consecutive = count,
                            "a watched fd has been continuously ready for many polls; \
                             if it has hung up (EOF/HUP) the app must Cmd::Unwatch it"
                        );
                    }
                }
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(SurfaceError::Socket(err.into())),
        }
    }
}

fn socket_error(err: wayland_client::backend::WaylandError) -> SurfaceError {
    match err {
        wayland_client::backend::WaylandError::Io(io) => SurfaceError::Socket(io),
        other => SurfaceError::Socket(std::io::Error::other(other.to_string())),
    }
}

/// One probe point on a live window, in window-surface coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    /// The node's `id` if it has one, else its CSS node name, indexed when the
    /// name repeats — `gallery::probe_points_of`'s labelling rule.
    pub label: String,
    /// Centre of the border box, floored.
    pub x: i32,
    /// Centre of the border box, floored.
    pub y: i32,
}

/// Label and centre every laid-out node under `root`.
///
/// Same derivation as `gallery::probe_points_of`: walk `root.descendants()`,
/// label by id-or-node-name with a repeat index, take the centre of the border
/// box **floored**. The flooring is load-bearing and identical to
/// `gallery::centre`'s: `f32::floor` before the cast, not a
/// bare `as i32` (which truncates toward zero) or `f32::round` (ties away from
/// zero): both move a positive and a negative half-pixel centre in opposite
/// directions, so shifting a box by an integer number of pixels would not
/// always shift its rounded centre by that same integer. `floor(v - k) ==
/// floor(v) - k` for every real `v` and integer `k`, so a scrolled copy of a
/// box centres exactly that many pixels from its unscrolled original. A node
/// with no allocation is skipped. Cheap: it reads the layout tree the last
/// frame already computed and lays nothing out.
#[must_use]
pub fn probe_points_of(root: &Node, layout: &crate::layout::LayoutTree) -> Vec<ProbePoint> {
    use std::collections::BTreeMap;

    let centre = |label: String, alloc: &crate::layout::Allocation| {
        let r = alloc.border_box;
        ProbePoint {
            label,
            x: (r.x + r.width / 2.0).floor() as i32,
            y: (r.y + r.height / 2.0).floor() as i32,
        }
    };
    let label_of = |node: &Node, index: usize, repeats: bool| -> String {
        node.id().map_or_else(
            || {
                let name = node.name();
                if repeats {
                    format!("{name}{index}")
                } else {
                    name.to_string()
                }
            },
            |id| id.as_str().to_string(),
        )
    };

    let mut points = Vec::new();
    if let Some(alloc) = layout.allocation(root) {
        points.push(centre("root".to_string(), &alloc));
    }
    // A `display: none` subtree paints nothing and receives no pointer event,
    // so reporting its nodes' centres as probe points published coordinates a
    // gate cannot click — a hidden `Stack` page's widgets were listed beside
    // the visible page's own. The whole subtree is skipped, not just the node
    // that carries the flag: only the subtree root is ever recorded.
    let descendants: Vec<Node> = root
        .descendants()
        .filter(|node| !crate::view::app::is_hidden(layout, node))
        .collect();
    let mut counts: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        *counts.entry(node.name()).or_default() += 1;
    }
    let mut seen: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        let name = node.name();
        let index = seen.entry(name.clone()).or_default();
        let repeats = counts.get(&name).copied().unwrap_or(0) > 1;
        let label = label_of(node, *index, repeats);
        *index += 1;
        if let Some(alloc) = layout.allocation(node) {
            points.push(centre(label, &alloc));
        }
    }
    points
}

/// The allocation of the node under `root` whose [`Node::id`] is `id`.
#[must_use]
pub fn allocation_of(
    root: &Node,
    layout: &crate::layout::LayoutTree,
    id: &str,
) -> Option<crate::layout::Allocation> {
    root.descendants()
        .find(|node| node.id().is_some_and(|found| found.as_str() == id))
        .and_then(|node| layout.allocation(&node))
}

/// One window: a surface, its retained tree, and the pump that drives them.
pub struct Window {
    conn: Connection,
    queue: EventQueue<WindowState>,
    /// `Window::render` reallocates the buffer pool through this on a resize.
    qh: QueueHandle<WindowState>,
    state: WindowState,
    surface: Surface,
    /// `Window::render` reallocates the buffer pool through this on a resize.
    shm: wl_shm::WlShm,
    /// The main surface's buffer pool. Owned here, not on `Surface`: one
    /// window can have several pools once popups exist (Task 14), and
    /// `WindowState`'s own doc comment already settles that they live on
    /// `Window`.
    buffers: BufferPool,
    skia: skia_rs_safe::canvas::Surface,
    root: Node,
    layout: crate::layout::LayoutTree,
    styles: StyleMap,
    anim: AnimationState,
    env: crate::css::computed::ResolveEnv,
    images: crate::paint::ImageCache,
    sheet: CompiledSheet,
    fonts: FontDatabase,
    /// Icon-theme lookup and rasterisation for `-gtk-icontheme()` etc.
    ///
    /// Selected from the environment (`gtk-icon-theme-name` plus the XDG
    /// roots) by [`crate::icons::IconTheme::from_env`], so a client driving a
    /// `Window` directly gets the user's real icons; replaceable through
    /// [`Window::set_icon_theme`] for a client that wants a hermetic one.
    icons: crate::icons::IconTheme,
    clock: Rc<dyn Clock>,
    /// `Window::render` gates its first attach on this.
    phase: MapPhase,
    dirty: bool,
    /// Open popups, innermost (most recently opened) last -- the order
    /// `close_popup` must tear down in, and the order a nested `get_popup`
    /// picks its parent from.
    popups: Vec<PopupWindow>,
    /// The next [`PopupKey`] to mint. Never reused, so a stale event that
    /// names an already-closed popup is silently ignored rather than
    /// mistaken for a new one.
    next_popup_key: u64,
    /// Built lazily by [`Window::clipboard`] -- see its doc comment.
    clipboard: Option<Clipboard>,
    /// The text a leaf node paints and measures, keyed by node identity.
    ///
    /// M2's `text.rs` is single-line and single-run and M3's `TextLayout` is
    /// P5's, so this is deliberately the smallest thing that can put real
    /// glyphs on a real surface: one shaped run per node. P5 deletes this
    /// field in the commit that lands `TextLayout`.
    texts: std::collections::HashMap<NodeAddr, String>,
    /// Foreign fds this window polls alongside its own connection (M5-D1).
    watches: Watches,
}

/// How long [`Window::open`] waits for the first `configure`.
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5);

impl Window {
    /// Connect, bind, map the surface `spec` describes, and wait for its first
    /// configure.
    ///
    /// # Errors
    ///
    /// [`SurfaceError`] for a failed connection, a missing global, a failed shm
    /// or Skia allocation, a failed dispatch, the compositor closing the
    /// surface before configuring it, or that configure not arriving within
    /// [`CONFIGURE_TIMEOUT`].
    pub fn open(
        spec: SurfaceSpec,
        sheet: CompiledSheet,
        fonts: FontDatabase,
    ) -> Result<Self, SurfaceError> {
        let conn = Connection::connect_to_env().map_err(SurfaceError::Connect)?;
        Self::open_on(conn, spec, sheet, fonts)
    }

    /// [`Window::open`] against one named socket, ignoring `$WAYLAND_DISPLAY`.
    ///
    /// The environment is process-global: a test that sets `WAYLAND_DISPLAY`
    /// to reach its own compositor mutates it for every other test running
    /// beside it in the same binary (and, on edition 2024, does so through an
    /// `unsafe` call that is undefined behaviour the moment another thread
    /// reads the environment). Naming the socket is the way out —
    /// `outputs::OutputsConnection::connect_to_path`'s own reason for
    /// existing.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Socket`] if the socket cannot be connected, and
    /// everything [`Window::open`] answers with after that.
    pub fn open_at_path(
        path: impl AsRef<std::path::Path>,
        spec: SurfaceSpec,
        sheet: CompiledSheet,
        fonts: FontDatabase,
    ) -> Result<Self, SurfaceError> {
        let stream =
            std::os::unix::net::UnixStream::connect(path.as_ref()).map_err(SurfaceError::Socket)?;
        let conn = Connection::from_socket(stream)
            .map_err(|err| SurfaceError::Socket(std::io::Error::other(err.to_string())))?;
        Self::open_on(conn, spec, sheet, fonts)
    }

    /// The body both constructors share, over a connection already made.
    fn open_on(
        conn: Connection,
        spec: SurfaceSpec,
        sheet: CompiledSheet,
        fonts: FontDatabase,
    ) -> Result<Self, SurfaceError> {
        let mut queue: EventQueue<WindowState> = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = WindowState::new();
        // Recorded before anything can configure us: a `configure` with a zero
        // dimension means "you choose", and this is the choice.
        state.default_size = (spec.size.0.max(1), spec.size.1.max(1));
        // Two roundtrips: `wl_seat.capabilities` arrives after the bind.
        queue
            .roundtrip(&mut state)
            .map_err(SurfaceError::Dispatch)?;
        queue
            .roundtrip(&mut state)
            .map_err(SurfaceError::Dispatch)?;

        let compositor = state
            .compositor
            .clone()
            .ok_or(SurfaceError::MissingGlobal("wl_compositor"))?;
        let shm = state
            .shm
            .clone()
            .ok_or(SurfaceError::MissingGlobal("wl_shm"))?;
        let wl_surface = compositor.create_surface(&qh, SurfaceTarget::Window);
        state
            .surface_targets
            .push((wl_surface.id(), SurfaceTarget::Window));
        let width = i32::try_from(spec.size.0.max(1)).unwrap_or(i32::MAX);
        let height = i32::try_from(spec.size.1.max(1)).unwrap_or(i32::MAX);

        // One cursor-shape device per window, not per surface: there is
        // exactly one `wl_pointer`, bound already if the seat advertised the
        // capability in the roundtrips above.
        let cursor = state
            .cursor_manager
            .as_ref()
            .zip(state.pointer.as_ref())
            .map(|(manager, pointer)| manager.get_pointer(pointer, &qh, ()));

        let mut surface = match &spec.role {
            Role::Toplevel => {
                let wm_base = state
                    .wm_base
                    .clone()
                    .ok_or(SurfaceError::MissingGlobal("xdg_wm_base"))?;
                let xdg_surface = wm_base.get_xdg_surface(&wl_surface, &qh, SurfaceTarget::Window);
                let xdg_toplevel = xdg_surface.get_toplevel(&qh, ());
                let toplevel = toplevel::Toplevel {
                    wl_surface: wl_surface.clone(),
                    xdg_surface,
                    xdg_toplevel,
                    size: spec.size,
                    scale: 1,
                    states: SurfaceStates::empty(),
                    cursor,
                };
                toplevel.set_title(&spec.title);
                toplevel.set_app_id(&spec.app_id);
                Surface::Toplevel(toplevel)
            }
            Role::Layer(layer_spec) => {
                let mut layer = layer::Layer::create(
                    &state,
                    &qh,
                    &wl_surface,
                    layer_spec,
                    (width, height),
                    &spec.title,
                )?;
                layer.cursor = cursor;
                Surface::Layer(layer)
            }
        };
        // The empty commit: role state is double-buffered, and no buffer may
        // be attached until the configure it triggers comes back.
        wl_surface.commit();
        conn.flush().map_err(socket_error)?;

        wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            CONFIGURE_TIMEOUT,
            &Watches::default(),
            |state| state.configured.is_some(),
        )?;
        let (width, height) = state.configured.map_or((width, height), |(w, h)| {
            (
                i32::try_from(w.max(1)).unwrap_or(width),
                i32::try_from(h.max(1)).unwrap_or(height),
            )
        });
        sync_surface_from_state(&mut surface, &state);

        let buffers = BufferPool::new(&shm, &qh, width, height).map_err(SurfaceError::Shm)?;
        let skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(width, height)
            .ok_or(SurfaceError::Render("the raster surface to paint into"))?;
        let root = Node::with_classes("window", &["background"]);
        // The user's real icon theme, at whatever output scale the first
        // `wl_surface.preferred_buffer_scale` already reported.
        let mut icons = crate::icons::IconTheme::from_env();
        icons.set_scale(u32::try_from(state.scale).unwrap_or(1));
        Ok(Self {
            conn,
            queue,
            qh,
            state,
            surface,
            shm,
            buffers,
            skia,
            root,
            layout: crate::layout::LayoutTree::new(),
            styles: StyleMap::new(),
            anim: AnimationState::new(),
            env: crate::css::computed::ResolveEnv::default(),
            images: crate::paint::ImageCache::new(),
            sheet,
            fonts,
            icons,
            clock: Rc::new(MonotonicClock::new()),
            phase: MapPhase::Configured,
            dirty: true,
            popups: Vec::new(),
            next_popup_key: 1,
            clipboard: None,
            texts: std::collections::HashMap::new(),
            watches: Watches::default(),
        })
    }

    /// Dispatch, and hand up everything that arrived.
    ///
    /// `None` blocks until an event or a close. `Some(Duration::ZERO)` is a
    /// non-blocking drain, which is what a caller with an animation running
    /// wants.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Dispatch`] or [`SurfaceError::Socket`]. A timeout is not
    /// an error here: it is an empty batch.
    pub fn pump(&mut self, timeout: Option<Duration>) -> Result<Vec<InputEvent>, SurfaceError> {
        let wait = timeout.unwrap_or(Duration::MAX);
        match wait_bounded(
            &self.conn,
            &mut self.queue,
            &mut self.state,
            wait,
            &self.watches,
            |state| !state.events.is_empty(),
        ) {
            Ok(()) | Err(SurfaceError::Timeout(_)) | Err(SurfaceError::Closed) => {}
            Err(err) => return Err(err),
        }
        // The repeat timer is ours, not the compositor's: it has to be sampled
        // wherever the pump wakes up, including on a timeout.
        let now = self.clock.now();
        if let Some(keymap) = self.state.keymap.as_mut()
            && let Some(event) = keymap.repeat_due(now)
        {
            self.state.events.push(InputEvent::Key(event));
        }
        let mut batch = std::mem::take(&mut self.state.events);
        // A `wl_callback.done` carries a compositor timestamp in an unrelated
        // clock domain, so the dispatch pushes `Frame { now: Duration::ZERO }`
        // and the reading of *our* clock -- the one `AnimationState` and the
        // repeat timer are sampled against -- is stamped in here, where a
        // clock exists.
        stamp_frames(&mut batch, now);
        // `:backdrop` is the one configure state the stylesheet reads, and the
        // root has to carry it before the caller restyles off this batch.
        for event in &batch {
            match event {
                InputEvent::Configure { states, .. } => {
                    apply_configure_states(&self.root, *states);
                    sync_surface_from_state(&mut self.surface, &self.state);
                    self.dirty = true;
                }
                InputEvent::ScaleChanged(factor) => {
                    sync_surface_from_state(&mut self.surface, &self.state);
                    // HiDPI icons: the rasterisation scale is the output
                    // scale (contract §10 P7-D42 / deviation 3). Without
                    // this the theme stays at 1 forever and a 2x display
                    // gets 16 px icons stretched to 32.
                    self.icons.set_scale(u32::try_from(*factor).unwrap_or(1));
                    self.dirty = true;
                }
                _ => {}
            }
        }
        // The repeat clock is ours (see `wl_keyboard::Event::Key`'s dispatch
        // comment): arm it here, from the batch's Key events, so a held key
        // actually starts repeating. Every one of them in order, not just the
        // last: `arm_repeat` decides per key, and a batch holding
        // `a down, Shift down` must arm on the `a` and then leave it alone.
        // A synthetic repeat event pushed above by `repeat_due` must not
        // re-arm from itself.
        if let Some(keymap) = self.state.keymap.as_mut() {
            for event in &batch {
                if let InputEvent::Key(key) = event
                    && !key.repeat
                {
                    keymap.arm_repeat(key, now);
                }
            }
        }
        Ok(batch)
    }

    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Every laid-out node of this window's tree, labelled and centred.
    ///
    /// The live counterpart of `App::probe`, which is offscreen-only: a
    /// harness test against a running client has no other way to ask where a
    /// widget ended up, and the gate rules forbid hard-coded coordinates.
    /// Reads the tree the last [`Window::render`] laid out, or -- for a window
    /// driven by `App::run` -- the tree its own reconcile loop last laid out,
    /// published here for the run of an `App::on_frame` hook (P5-D5).
    #[must_use]
    pub fn probe_points(&self) -> Vec<ProbePoint> {
        probe_points_of(&self.root, &self.layout)
    }

    /// The border box of the node whose [`Node::id`] is `id`, in
    /// window-surface coordinates. `None` for an unknown id or a node that has
    /// not been laid out.
    #[must_use]
    pub fn allocation(&self, id: &str) -> Option<crate::layout::Allocation> {
        allocation_of(&self.root, &self.layout, id)
    }

    /// Hand this window's layout tree to its caller, leaving an empty one
    /// behind.
    ///
    /// `App::run` keeps its own reconcile loop's [`crate::layout::LayoutTree`]
    /// on its `Runtime` rather than on `Window` (it must survive across
    /// frames for taffy's incremental dirty tracking), so [`Window::layout`]
    /// itself is otherwise never touched once such a loop is driving the
    /// window. `take_layout`/[`Window::set_layout`] let `App::run` swap the
    /// loop's tree onto `self` for the span of an `on_frame` hook (P5-D5), so
    /// [`Window::probe_points`] and [`Window::allocation`] answer for what the
    /// loop just laid out, then take it back for the next iteration.
    pub(crate) fn take_layout(&mut self) -> crate::layout::LayoutTree {
        std::mem::take(&mut self.layout)
    }

    /// See [`Window::take_layout`].
    pub(crate) fn set_layout(&mut self, layout: crate::layout::LayoutTree) {
        self.layout = layout;
    }

    /// Register `fd` in this window's poll set.
    ///
    /// The window takes ownership so the caller cannot close the fd out from
    /// under the loop; [`Window::unwatch`] is what closes it. Readiness is
    /// reported as [`InputEvent::FdReady`] from [`Window::pump`], and nothing
    /// is read from the fd by the toolkit — that is the owner's job.
    pub fn watch_fd(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId {
        self.watches.add(fd, interest)
    }

    /// Drop a watch and close its fd. An unknown id is a no-op.
    pub fn unwatch(&mut self, id: WatchId) {
        self.watches.remove(id);
    }

    /// Every live watch, in registration order.
    #[must_use]
    pub fn watches(&self) -> Vec<WatchId> {
        self.watches.ids()
    }

    /// The icon theme this window's own paints resolve through.
    pub fn icons(&mut self) -> &mut crate::icons::IconTheme {
        &mut self.icons
    }

    /// Take ownership of the window's icon theme, leaving a hermetic
    /// placeholder behind.
    ///
    /// `App::run` drives every paint itself, through [`Window::paint_with`]
    /// and [`Window::paint_popup_with`], and needs the theme as a `&mut` for
    /// a whole frame — which it cannot hold across its own `&mut Window`
    /// borrows. So it takes the theme the window selected from the
    /// environment and owns it for the rest of the run. What is left behind
    /// is only ever used by [`Window::render`], which the reactive loop never
    /// calls.
    pub fn take_icon_theme(&mut self) -> crate::icons::IconTheme {
        std::mem::replace(
            &mut self.icons,
            crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new()),
        )
    }

    /// Replace the icon theme, keeping the output scale the compositor
    /// announced.
    ///
    /// [`Window::open`] builds one from the environment; a client that wants
    /// a hermetic theme (a test, or a shell shipping its own icon set) hands
    /// one in here. `App::run` does exactly this so the window and the
    /// reactive loop resolve icons through the same theme.
    pub fn set_icon_theme(&mut self, mut icons: crate::icons::IconTheme) {
        icons.set_scale(u32::try_from(self.state.scale).unwrap_or(1));
        self.icons = icons;
        self.dirty = true;
    }

    pub fn layout(&mut self) -> &mut crate::layout::LayoutTree {
        &mut self.layout
    }

    pub fn animations(&mut self) -> &mut AnimationState {
        &mut self.anim
    }

    #[must_use]
    pub fn styles(&self) -> &StyleMap {
        &self.styles
    }

    #[must_use]
    pub fn sheet(&self) -> &CompiledSheet {
        &self.sheet
    }

    pub fn fonts(&mut self) -> &mut FontDatabase {
        &mut self.fonts
    }

    #[must_use]
    pub fn clock(&self) -> &Rc<dyn Clock> {
        &self.clock
    }

    pub fn set_clock(&mut self, clock: Rc<dyn Clock>) {
        self.clock = clock;
    }

    #[must_use]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// The most recent input serial, for `grab`/`move`/`resize`/clipboard.
    #[must_use]
    pub fn seat_serial(&self) -> Option<u32> {
        self.state.seat_serial
    }

    /// The clipboard and primary selection, building the `wl_data_device`
    /// (and, if advertised, the primary-selection device) the first time this
    /// is called.
    ///
    /// A compositor that never advertised `wl_data_device_manager` gets an
    /// in-process clipboard (with a warning) rather than a panic, matching
    /// the primary-selection path (§3.8 as amended by the M3 review-fix
    /// wave).
    ///
    /// # Panics
    ///
    /// If the compositor never advertised a `wl_seat`.
    pub fn clipboard(&mut self) -> &mut Clipboard {
        if self.clipboard.is_none() {
            let seat = self
                .state
                .seat
                .clone()
                .expect("compositor did not advertise wl_seat");
            let shared = Rc::new(RefCell::new(ClipboardShared::default()));
            self.state.clipboard_shared = Some(Rc::clone(&shared));
            self.clipboard = Some(Clipboard::new(
                self.conn.clone(),
                self.qh.clone(),
                self.state.data_device_manager.clone(),
                &seat,
                self.state.primary_manager.clone(),
                shared,
            ));
        }
        self.clipboard.as_mut().expect("just initialized above")
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.closed
    }

    /// Name the cursor for whichever surface currently has pointer focus.
    ///
    /// The convenience entry point over [`Surface::set_cursor_shape`], for a
    /// caller that holds the `Window` rather than the role: the serial to pass
    /// is the pointer's most recent enter/motion serial, which
    /// [`Window::seat_serial`] reports.
    pub fn set_cursor_shape(&mut self, shape: CursorShape, serial: u32) {
        self.surface.set_cursor_shape(shape, serial);
    }

    /// The main surface's configured size in surface-local pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.surface.size()
    }

    /// Set the toplevel's title.
    ///
    /// A no-op for a layer surface or a popup -- neither has an
    /// `xdg_toplevel` to name (contract §3.2's requests table is a
    /// toplevel-only one).
    pub fn set_title(&self, title: &str) {
        if let Surface::Toplevel(toplevel) = &self.surface {
            toplevel.set_title(title);
        }
    }

    /// Minimise the toplevel. A no-op for a layer surface or a popup.
    pub fn minimize(&self) {
        if let Surface::Toplevel(toplevel) = &self.surface {
            toplevel.set_minimized();
        }
    }

    /// Flip the toplevel's maximised state, read off its last-known
    /// `xdg_toplevel.configure` states. A no-op for a layer surface or a
    /// popup.
    pub fn toggle_maximized(&self) {
        if let Surface::Toplevel(toplevel) = &self.surface {
            let now_maximized = self.surface.states().contains(SurfaceStates::MAXIMIZED);
            toplevel.set_maximized(!now_maximized);
        }
    }

    /// Restyle the dirty tree, relayout, repaint, attach and commit.
    ///
    /// Returns whether anything was actually painted, so a caller can tell an
    /// idle frame from a real one.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] for a failed buffer allocation or upload,
    /// [`SurfaceError::Socket`] for a failed flush, [`SurfaceError::Render`]
    /// if the raster surface cannot be reallocated after a resize.
    pub fn render(&mut self) -> Result<bool, SurfaceError> {
        let now = self.clock.now();
        let mut painted = self.render_main(now)?;
        // Popups are rendered whether or not the main surface just repainted:
        // a menu opened this frame is dirty on its own, and the main window
        // may well be idle underneath it.
        painted |= self.render_popups(now)?;
        if painted {
            self.conn.flush().map_err(socket_error)?;
        }
        Ok(painted)
    }

    /// The main surface's half of [`Window::render`].
    fn render_main(&mut self, now: Duration) -> Result<bool, SurfaceError> {
        let overrides = self.anim.sample(now);
        if !should_paint(self.dirty, self.anim.is_active(now)) {
            return Ok(false);
        }
        if !may_attach(self.phase) {
            // Nothing to paint into yet; the first configure will mark dirty.
            return Ok(false);
        }
        self.drain_releases();
        self.resize_backing()?;
        let (width, height) = self.surface.size();
        let available = taffy::Size {
            width: taffy::AvailableSpace::Definite(width as f32),
            height: taffy::AvailableSpace::Definite(height as f32),
        };
        let mut measure = TextMeasure {
            texts: &self.texts,
            fonts: &mut self.fonts,
        };
        if let Err(err) = restyle(
            &self.root,
            &self.sheet,
            &self.env,
            &mut self.styles,
            &mut self.layout,
            available,
            &mut measure,
        ) {
            tracing::error!(%err, "layout failed; keeping the previous frame");
            return Ok(false);
        }
        self.skia
            .canvas()
            .clear(skia_rs_safe::core::Color::TRANSPARENT);
        paint_tree(
            &mut self.skia,
            &self.root,
            &self.layout,
            &self.styles,
            &overrides,
            &self.env,
            &self.sheet,
            &mut self.fonts,
            &mut self.images,
            &mut self.icons,
            &self.texts,
        );
        self.surface
            .commit_buffer(&mut self.skia, &mut self.buffers, &self.shm, &self.qh)?;
        self.dirty = false;
        Ok(true)
    }

    /// Paint one frame with `f` and commit it.
    ///
    /// Acquires a free buffer from the pool, clears the window's own skia
    /// surface, runs `f` against it, uploads the result and commits.
    /// `Ok(false)` means every buffer was still held by the compositor and
    /// nothing was painted -- the caller retries on the next frame callback.
    ///
    /// Contract deviation D2: `Window::render` paints the window's own tree,
    /// but §9 gives the recursive paint walker to P4, so the reactive layer
    /// needs a way to paint *its* tree through the same buffer machinery.
    ///
    /// Reconciliation: the contract's sketch names a pre-existing
    /// `acquire_slot`/`upload_and_commit` pair on `Window`; P3 instead factored
    /// that sequence as the free functions `commit_buffer_to` and
    /// `Surface::commit_buffer`, which paint a *known* tree rather than
    /// accepting an arbitrary closure and give no way to learn whether a
    /// buffer was actually acquired. `paint_with` inlines the same
    /// acquire/upload/attach/damage/commit sequence those helpers already use,
    /// so the `Ok(false)` contract (`BufferPool::acquire` returning `None`)
    /// stays visible to the caller.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] if the buffer pool cannot grow or the upload
    /// fails, [`SurfaceError::Closed`] if the surface is gone,
    /// [`SurfaceError::Render`] if the raster surface cannot be reallocated
    /// after a resize, [`SurfaceError::Socket`] if the flush fails.
    pub fn paint_with(
        &mut self,
        f: impl FnOnce(&mut skia_rs_safe::canvas::Surface),
    ) -> Result<bool, SurfaceError> {
        if self.is_closed() {
            return Err(SurfaceError::Closed);
        }
        if !may_attach(self.phase) {
            // Nothing to paint into yet; the first configure will mark dirty.
            return Ok(false);
        }
        self.drain_releases();
        self.resize_backing()?;
        let Some(index) = self
            .buffers
            .acquire(&self.shm, &self.qh)
            .map_err(SurfaceError::Shm)?
        else {
            tracing::debug!("every shm buffer is still held; deferring this frame");
            return Ok(false);
        };
        self.skia
            .canvas()
            .clear(skia_rs_safe::core::Color::TRANSPARENT);
        f(&mut self.skia);
        let (width, height) = self.buffers.size();
        self.buffers
            .upload(index, &self.skia)
            .map_err(SurfaceError::Shm)?;
        let wl_surface = self.surface.wl_surface().clone();
        let target = self.surface.target();
        wl_surface.attach(Some(self.buffers.wl_buffer(index)), 0, 0);
        wl_surface.damage_buffer(0, 0, width, height);
        wl_surface.frame(&self.qh, target);
        wl_surface.commit();
        self.conn.flush().map_err(socket_error)?;
        self.dirty = false;
        Ok(true)
    }

    /// Mark `node`'s tree as needing a repaint.
    ///
    /// Node-level granularity is P4's; M3's window repaints the whole surface,
    /// because a partial repaint needs a damage rect per node and P4 owns the
    /// walker that could compute one.
    pub fn mark_dirty(&mut self, node: &Node) {
        debug_assert!(
            node.root().ptr_eq(&self.root.root()),
            "mark_dirty on a node from another tree"
        );
        self.dirty = true;
    }

    /// The text a leaf node paints and measures. P5's `TextLayout` replaces
    /// it.
    pub fn set_node_text(&mut self, node: &Node, text: &str) {
        self.texts.insert(node_addr(node), text.to_owned());
        self.dirty = true;
    }

    #[must_use]
    pub fn node_text(&self, node: &Node) -> Option<&str> {
        self.texts.get(&node_addr(node)).map(String::as_str)
    }

    /// The soonest of the main tree's animation deadline, the keyboard
    /// repeat's, and **every open popup's own animation deadline**.
    ///
    /// A popup has its own `AnimationState` and `render_popups` paints it on
    /// its own schedule, so a caller that sleeps for `pump(next_deadline())`
    /// would otherwise never be woken by an animating menu -- its transition
    /// would stall at its first frame until some unrelated event arrived.
    ///
    /// `Duration::ZERO` is "now", never "spin": a continuously interpolating
    /// transition honestly has no later deadline than this instant.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        let now = self.clock.now();
        next_deadline_of(
            self.anim.next_deadline(now),
            self.state
                .keymap
                .as_ref()
                .and_then(|keymap| keymap.repeat_deadline(now)),
            self.popups.iter().map(|p| p.anim.next_deadline(now)),
        )
    }

    /// Move any `wl_buffer.release`s that belong to the active surface's pool
    /// out of `WindowState`'s pending queue and back into it.
    ///
    /// The plan's `acquire_from_state` folded into this: `WindowState` is the
    /// only thing a `Dispatch` impl can reach, so the release is recorded
    /// there by `wl_buffer` id; only `Window` can match that id back to the
    /// pool it belongs to and hand the slot back.
    fn drain_releases(&mut self) {
        let released = std::mem::take(&mut self.state.released);
        for (id, slot) in released {
            let slot_of =
                (0..self.buffers.slots().len()).find(|&i| self.buffers.wl_buffer(i).id() == id);
            if release_matches(slot_of, slot.0) {
                self.buffers.release(slot);
                continue;
            }
            // Not the main pool's buffer: a popup's own pool (Task 14) may
            // recognise it instead.
            if let Some(popup) = self
                .popups
                .iter_mut()
                .find(|p| (0..p.buffers.slots().len()).any(|i| p.buffers.wl_buffer(i).id() == id))
            {
                popup.buffers.release(slot);
            }
        }
    }

    /// Reallocate the buffer pool and the raster surface after a configure
    /// changed the size.
    fn resize_backing(&mut self) -> Result<(), SurfaceError> {
        let (width, height) = self.surface.size();
        let (want_w, want_h) = (
            i32::try_from(width.max(1)).unwrap_or(i32::MAX),
            i32::try_from(height.max(1)).unwrap_or(i32::MAX),
        );
        if self.buffers.size() == (want_w, want_h) {
            return Ok(());
        }
        // A fresh pool, not a resized one: the compositor may still hold the
        // old buffers, and a `wl_shm_pool` cannot shrink.
        self.buffers =
            BufferPool::new(&self.shm, &self.qh, want_w, want_h).map_err(SurfaceError::Shm)?;
        self.skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(want_w, want_h)
            .ok_or(SurfaceError::Render("the raster surface to paint into"))?;
        Ok(())
    }

    /// Open a popup anchored to a node or a rect of this window.
    ///
    /// Sequencing, per xdg-shell: positioner -> `get_popup` -> `grab` ->
    /// commit with no buffer -> configure -> ack -> attach. The grab, if any,
    /// uses the window's most recent input serial.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] for an invalid positioner or an anchor node
    /// with no allocation, [`SurfaceError::MissingGlobal`] without
    /// `xdg_wm_base`, [`SurfaceError::Shm`]/[`SurfaceError::Render`] for the
    /// popup's own backing store.
    pub fn open_popup(
        &mut self,
        parent: PopupAnchorPoint,
        mut positioner: Positioner,
    ) -> Result<PopupKey, SurfaceError> {
        if let PopupAnchorPoint::Node(node) = &parent {
            let alloc = self.layout.allocation(node).ok_or(SurfaceError::Protocol(
                "the popup's anchor node is not laid out",
            ))?;
            positioner.anchor_rect = alloc.border_box;
        } else if let PopupAnchorPoint::Rect(rect) = &parent {
            positioner.anchor_rect = *rect;
        }
        positioner.validate()?;
        let wm_base = self
            .state
            .wm_base
            .clone()
            .ok_or(SurfaceError::MissingGlobal("xdg_wm_base"))?;
        let key = PopupKey(self.next_popup_key);
        self.next_popup_key += 1;

        let xdg_positioner = wm_base.create_positioner(&self.qh, ());
        positioner.apply(&xdg_positioner);
        let wl_surface = self
            .state
            .compositor
            .clone()
            .ok_or(SurfaceError::MissingGlobal("wl_compositor"))?
            .create_surface(&self.qh, SurfaceTarget::Popup(key));
        let xdg_surface = wm_base.get_xdg_surface(&wl_surface, &self.qh, SurfaceTarget::Popup(key));
        // The parent is the topmost popup if there is one: a nested menu
        // hangs off its opener, not off the window. A layer surface has no
        // `xdg_surface` of its own -- xdg-shell's answer for that case is a
        // `None` parent here plus `zwlr_layer_surface_v1.get_popup` below,
        // sent before this popup's own first commit.
        let parent_xdg = self
            .popups
            .last()
            .and_then(|p| p.popup().map(|popup| popup.xdg_surface.clone()))
            .or_else(|| self.xdg_surface_of_window());
        let xdg_popup = xdg_surface.get_popup(
            parent_xdg.as_ref(),
            &xdg_positioner,
            &self.qh,
            SurfaceTarget::Popup(key),
        );
        if parent_xdg.is_none()
            && self.popups.is_empty()
            && let Surface::Layer(layer) = &self.surface
        {
            layer.layer_surface().get_popup(&xdg_popup);
        }
        let popup = Popup {
            key,
            wl_surface: wl_surface.clone(),
            xdg_surface,
            xdg_popup,
            position: (0, 0),
            size: positioner.size,
            scale: 1,
            // A shared handle on the window's one cursor-shape device, so a
            // popup can answer `Surface::set_cursor_shape` from `self`. Only
            // the window's own role destroys it -- see `Popup::cursor`.
            cursor: match &self.surface {
                Surface::Toplevel(t) => t.cursor.clone(),
                Surface::Layer(l) => l.cursor.clone(),
                Surface::Popup(p) => p.cursor.clone(),
            },
        };
        if let (Some(seat), Some(serial)) = (&self.state.seat, self.state.seat_serial) {
            // A menu takes the seat: a click outside dismisses the whole chain
            // and keyboard focus returns to the parent, which is the
            // compositor's job once the grab exists.
            popup.grab(seat, serial);
        }
        wl_surface.commit();
        xdg_positioner.destroy();
        self.conn.flush().map_err(socket_error)?;
        self.state
            .surface_targets
            .push((wl_surface.id(), SurfaceTarget::Popup(key)));

        let (w, h) = (
            i32::try_from(positioner.size.0).unwrap_or(1).max(1),
            i32::try_from(positioner.size.1).unwrap_or(1).max(1),
        );
        self.popups.push(PopupWindow {
            key,
            surface: Surface::Popup(popup),
            root: Node::with_classes("popup", &["background"]),
            layout: crate::layout::LayoutTree::new(),
            styles: StyleMap::new(),
            anim: AnimationState::new(),
            buffers: BufferPool::new(&self.shm, &self.qh, w, h).map_err(SurfaceError::Shm)?,
            skia: skia_rs_safe::canvas::Surface::new_raster_n32_premul(w, h)
                .ok_or(SurfaceError::Render("the popup's raster surface"))?,
            dirty: true,
        });
        Ok(key)
    }

    /// Destroy `key` and every popup above it, topmost first.
    ///
    /// The protocol requires reverse-creation order; destroying a popup with
    /// children alive is `xdg_wm_base.error.not_the_topmost_popup`, which kills
    /// the client.
    pub fn close_popup(&mut self, key: PopupKey) {
        let Some(index) = self.popups.iter().position(|p| p.key == key) else {
            return;
        };
        while self.popups.len() > index {
            let removed = self.popups.pop().expect("the index is in range");
            let id = removed.surface.wl_surface().id();
            self.state.surface_targets.retain(|(got, _)| *got != id);
            self.state
                .pending_ack
                .retain(|(t, _)| *t != SurfaceTarget::Popup(removed.key));
            self.state
                .popup_configures
                .retain(|(k, ..)| *k != removed.key);
        }
        let _ = self.conn.flush();
    }

    #[must_use]
    pub fn popup_root(&self, key: PopupKey) -> Option<Node> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.root.clone())
    }

    /// Where the compositor last placed popup `key`, in this window's frame
    /// space. `None` for an unknown key or one that has taken no configure.
    ///
    /// P5-D8: the enumerated exception to M5-D9's window-only probe. A popover
    /// is a second surface the compositor may have slid to keep it on screen,
    /// so a gate that clicks a row needs where the surface actually landed, not
    /// where the window's own tree thinks it is.
    #[must_use]
    pub fn popup_position(&self, key: PopupKey) -> Option<(i32, i32)> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| p.popup().map(Popup::position))
    }

    /// [`Window::probe_points`] for one popup's own tree, in that popup's
    /// surface coordinates (its own top-left is the origin).
    ///
    /// P5-D8: shares `probe_points_of`'s one labelling rule with the window's
    /// own probe. Empty for an unknown key; empty, too, until the reactive loop
    /// has laid the popup's tree out onto this `Window` (`App::run` keeps that
    /// layout on its `Runtime` and swaps it in only for the span of an
    /// `on_frame` hook — see `App::run`'s popup-layout swap).
    #[must_use]
    pub fn popup_probe_points(&self, key: PopupKey) -> Vec<ProbePoint> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .map_or_else(Vec::new, |p| probe_points_of(&p.root, &p.layout))
    }

    pub fn popup_layout(&mut self, key: PopupKey) -> Option<&mut crate::layout::LayoutTree> {
        self.popups
            .iter_mut()
            .find(|p| p.key == key)
            .map(|p| &mut p.layout)
    }

    pub fn popup_animations(&mut self, key: PopupKey) -> Option<&mut AnimationState> {
        self.popups
            .iter_mut()
            .find(|p| p.key == key)
            .map(|p| &mut p.anim)
    }

    /// Paint one popup surface with `f` and commit it.
    ///
    /// The popup equivalent of [`Window::paint_with`], and the seam contract
    /// §10 P6-D39's second limit names: `App::run` drives the main surface
    /// through `paint_with` and must not call [`Window::render`] (which would
    /// repaint the main surface from the window's own style map), so without
    /// this a popup opened by the reactive loop never got a buffer at all.
    ///
    /// `Ok(false)` means the popup is unknown, has never been configured (no
    /// buffer may be attached before its first `xdg_surface.configure`, or
    /// the compositor kills the client), or every buffer in its pool is still
    /// held; the caller retries on the next frame.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] if the popup's pool cannot grow or the upload
    /// fails, [`SurfaceError::Render`] if its raster surface cannot be
    /// reallocated, [`SurfaceError::Socket`] if the flush fails.
    pub fn paint_popup_with(
        &mut self,
        key: PopupKey,
        f: impl FnOnce(&mut skia_rs_safe::canvas::Surface),
    ) -> Result<bool, SurfaceError> {
        // Fold any `xdg_popup.configure` that arrived since the last call, so
        // the size painted into is the size the compositor last granted.
        for (configured_key, position, size) in std::mem::take(&mut self.state.popup_configures) {
            if let Some(popup) = self.popups.iter_mut().find(|p| p.key == configured_key)
                && let Some(role) = popup.popup_mut()
            {
                role.position = position;
                role.size = size;
                popup.dirty = true;
            }
        }
        let configured = self
            .state
            .pending_ack
            .iter()
            .any(|(t, _)| *t == SurfaceTarget::Popup(key));
        if !configured {
            return Ok(false);
        }
        let Some(popup) = self.popups.iter_mut().find(|p| p.key == key) else {
            return Ok(false);
        };
        let (width, height) = popup.surface.size();
        let (w, h) = (
            i32::try_from(width).unwrap_or(1).max(1),
            i32::try_from(height).unwrap_or(1).max(1),
        );
        if popup.buffers.size() != (w, h) {
            popup.buffers =
                BufferPool::new(&self.shm, &self.qh, w, h).map_err(SurfaceError::Shm)?;
            popup.skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(w, h)
                .ok_or(SurfaceError::Render("the popup's raster surface"))?;
        }
        popup
            .skia
            .canvas()
            .clear(skia_rs_safe::core::Color::TRANSPARENT);
        f(&mut popup.skia);
        popup
            .surface
            .commit_buffer(&mut popup.skia, &mut popup.buffers, &self.shm, &self.qh)?;
        popup.dirty = false;
        self.conn.flush().map_err(socket_error)?;
        Ok(true)
    }

    /// The size the compositor last granted popup `key`, if it has one.
    #[must_use]
    pub fn popup_size(&self, key: PopupKey) -> Option<(u32, u32)> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.surface.size())
    }

    pub fn popup_mark_dirty(&mut self, key: PopupKey) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.key == key) {
            p.dirty = true;
        }
    }

    /// The main surface's own `xdg_surface`, for a nested `get_popup`'s
    /// parent -- `None` for a layer-shell root, which has no `xdg_surface` at
    /// all (`open_popup` takes a different path for that case).
    fn xdg_surface_of_window(&self) -> Option<xdg_surface::XdgSurface> {
        match &self.surface {
            Surface::Toplevel(t) => Some(t.xdg_surface.clone()),
            // A layer surface has no `xdg_surface`, and a `Window`'s own
            // surface is never a popup (only its `PopupWindow`s are).
            Surface::Layer(_) | Surface::Popup(_) => None,
        }
    }

    /// Fold any `xdg_popup.configure`s that arrived since the last render
    /// into their popups, and repaint every dirty one that has a real
    /// `xdg_surface.configure` to attach against.
    ///
    /// A popup that has never been configured must not have a buffer
    /// attached -- `xdg_surface.error.unconfigured_buffer` kills the client
    /// -- so this checks `pending_ack` (populated for every surface, not
    /// just the window's) rather than assuming a freshly opened popup is
    /// ready the moment `render` is next called.
    fn render_popups(&mut self, now: Duration) -> Result<bool, SurfaceError> {
        for (key, position, size) in std::mem::take(&mut self.state.popup_configures) {
            if let Some(popup) = self.popups.iter_mut().find(|p| p.key == key)
                && let Some(role) = popup.popup_mut()
            {
                role.position = position;
                role.size = size;
                popup.dirty = true;
            }
        }
        let mut painted_any = false;
        for popup in &mut self.popups {
            let configured = self
                .state
                .pending_ack
                .iter()
                .any(|(t, _)| *t == SurfaceTarget::Popup(popup.key));
            if !configured {
                continue;
            }
            let overrides = popup.anim.sample(now);
            if !should_paint(popup.dirty, popup.anim.is_active(now)) {
                continue;
            }
            let (width, height) = popup.surface.size();
            let (w, h) = (
                i32::try_from(width).unwrap_or(1).max(1),
                i32::try_from(height).unwrap_or(1).max(1),
            );
            if popup.buffers.size() != (w, h) {
                popup.buffers =
                    BufferPool::new(&self.shm, &self.qh, w, h).map_err(SurfaceError::Shm)?;
                popup.skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(w, h)
                    .ok_or(SurfaceError::Render("the popup's raster surface"))?;
            }
            let available = taffy::Size {
                width: taffy::AvailableSpace::Definite(w as f32),
                height: taffy::AvailableSpace::Definite(h as f32),
            };
            let mut measure = crate::layout::FixedMeasure(taffy::Size {
                width: 0.0,
                height: 0.0,
            });
            if let Err(err) = restyle(
                &popup.root,
                &self.sheet,
                &self.env,
                &mut popup.styles,
                &mut popup.layout,
                available,
                &mut measure,
            ) {
                tracing::error!(%err, "popup layout failed; keeping the previous frame");
                continue;
            }
            popup
                .skia
                .canvas()
                .clear(skia_rs_safe::core::Color::TRANSPARENT);
            paint_tree(
                &mut popup.skia,
                &popup.root,
                &popup.layout,
                &popup.styles,
                &overrides,
                &self.env,
                &self.sheet,
                &mut self.fonts,
                &mut self.images,
                &mut self.icons,
                &self.texts,
            );
            popup.surface.commit_buffer(
                &mut popup.skia,
                &mut popup.buffers,
                &self.shm,
                &self.qh,
            )?;
            popup.dirty = false;
            painted_any = true;
        }
        Ok(painted_any)
    }
}

/// Whether the window should paint this frame.
///
/// Both halves matter: an idle window that repaints anyway burns a buffer, a
/// commit and a frame callback per frame forever, and an animating one that
/// does not repaint stalls at its first frame.
#[must_use]
fn should_paint(dirty: bool, animating: bool) -> bool {
    dirty || animating
}

/// The sooner of two deadlines.
///
/// `Duration::ZERO` means "now" and is a real answer, so this is a `min` over
/// the `Some`s, never an `or`.
#[must_use]
pub fn fold_deadlines(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (some, None) | (None, some) => some,
    }
}

/// The size a `configure` with a possibly-zero dimension actually asks for.
///
/// A zero means "you choose": keep the size the compositor last chose, and on
/// the *first* configure -- when there is none -- fall back to the size the
/// client asked for in its [`SurfaceSpec`]. Taking the zero literally maps a
/// 1x1 surface, because `Window::open` floors what it gets at 1.
///
/// Shared by the `xdg_toplevel` and `zwlr_layer_surface_v1` dispatches: the
/// rule is the protocols' own and is identical in both.
#[must_use]
pub(crate) fn configured_size_with_default(
    previous: Option<(u32, u32)>,
    default: (u32, u32),
    width: u32,
    height: u32,
) -> (u32, u32) {
    layer::configured_size(previous.unwrap_or(default), width, height)
}

/// Fold the window's own two deadlines together with one per open popup.
///
/// Separated from [`Window::next_deadline`] so the fold can be tested without
/// a live Wayland connection: a `Window` needs a compositor, an
/// `Option<Duration>` does not.
#[must_use]
pub(crate) fn next_deadline_of(
    anim: Option<Duration>,
    repeat: Option<Duration>,
    popups: impl IntoIterator<Item = Option<Duration>>,
) -> Option<Duration> {
    popups
        .into_iter()
        .fold(fold_deadlines(anim, repeat), fold_deadlines)
}

/// Stamp every [`InputEvent::Frame`] in `batch` with `now`.
///
/// `wl_callback.done`'s own timestamp is in the compositor's clock domain and
/// its `Dispatch` impl has no clock to read, so the dispatch pushes a
/// placeholder and the pump -- which does have one -- fills it in. Separated
/// from [`Window::pump`] for the same reason as [`next_deadline_of`].
pub(crate) fn stamp_frames(batch: &mut [InputEvent], now: Duration) {
    for event in batch {
        if let InputEvent::Frame { now: slot } = event {
            *slot = now;
        }
    }
}

/// Whether a `wl_buffer.release` belongs to the pool that reports `slot_of`.
#[must_use]
fn release_matches(slot_of: Option<usize>, slot: usize) -> bool {
    slot_of == Some(slot)
}

/// Measures a leaf from its text, or to nothing.
struct TextMeasure<'a> {
    texts: &'a std::collections::HashMap<NodeAddr, String>,
    fonts: &'a mut FontDatabase,
}

impl crate::layout::Measure for TextMeasure<'_> {
    fn measure(
        &mut self,
        node: &Node,
        style: &crate::css::computed::ComputedStyle,
        known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        let Some(text) = self.texts.get(&node_addr(node)) else {
            return taffy::Size {
                width: known.width.unwrap_or(0.0),
                height: known.height.unwrap_or(0.0),
            };
        };
        let text_style = crate::text::TextStyle::from_computed(style);
        let Some(face) = self.fonts.match_face(&text_style.query()) else {
            return taffy::Size {
                width: 0.0,
                height: 0.0,
            };
        };
        let shaped = self.fonts.shape(&text_style.shape_key(text, &face));
        taffy::Size {
            width: known.width.unwrap_or(shaped.metrics.width),
            height: known
                .height
                .unwrap_or_else(|| text_style.line_height_px(&shaped.metrics)),
        }
    }
}

/// Paint the whole tree, depth first, each node inside its own effect layer.
///
/// This is the caller `paint::paint_node_with_children` was shaped for and
/// M2's `#[allow(unused_variables)]` on its `node` parameter anticipated. P4
/// replaces it with the reconciler-aware walker that also carries per-node
/// animation overrides; until then the window's own `Overrides` apply to the
/// root, which is where M3's window-level transitions live.
// Nine parameters over clippy's default seven: this is `Window::render`'s
// only caller, threading through exactly the borrows a single frame needs
// (the raster target, the tree, the two per-frame maps, the sampled
// animation overrides and the three caches `PaintCx` wraps) with no `self`
// to hang them on -- P4's reconciler-aware walker replaces this whole
// function rather than growing a context struct for one caller.
#[allow(clippy::too_many_arguments)]
fn paint_tree(
    skia: &mut skia_rs_safe::canvas::Surface,
    root: &Node,
    layout: &crate::layout::LayoutTree,
    styles: &StyleMap,
    overrides: &crate::anim::Overrides,
    env: &crate::css::computed::ResolveEnv,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    images: &mut crate::paint::ImageCache,
    icons: &mut crate::icons::IconTheme,
    texts: &std::collections::HashMap<NodeAddr, String>,
) {
    // Shaped up front, once per node, into a map that outlives the whole
    // recursive walk: `PaintCx::text` borrows into it, and a `Rc<ShapedText>`
    // freshly shaped inside a single `paint_subtree` call would not live
    // long enough to satisfy that borrow across sibling/child calls.
    let mut shaped_texts: std::collections::HashMap<
        NodeAddr,
        std::rc::Rc<crate::text::ShapedText>,
    > = std::collections::HashMap::new();
    for (addr, text) in texts {
        let Some(style) = styles.get(addr) else {
            continue;
        };
        let text_style = crate::text::TextStyle::from_computed(style);
        let Some(face) = fonts.match_face(&text_style.query()) else {
            continue;
        };
        shaped_texts.insert(*addr, fonts.shape(&text_style.shape_key(text, &face)));
    }
    let mut cx = crate::paint::PaintCx {
        env,
        colors: &sheet.colors,
        fonts,
        images,
        icons,
        text: None,
    };
    let mut canvas = skia.canvas();
    paint_subtree(
        &mut canvas,
        root,
        layout,
        styles,
        Some(overrides),
        &mut cx,
        0,
        &shaped_texts,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_subtree<'a>(
    canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
    node: &Node,
    layout: &crate::layout::LayoutTree,
    styles: &StyleMap,
    overrides: Option<&crate::anim::Overrides>,
    cx: &mut crate::paint::PaintCx<'a>,
    depth: usize,
    shaped_texts: &'a std::collections::HashMap<NodeAddr, std::rc::Rc<crate::text::ShapedText>>,
) {
    if depth >= pointer::MAX_HIT_DEPTH {
        return;
    }
    let (Some(alloc), Some(style)) = (layout.allocation(node), styles.get(&node_addr(node))) else {
        return;
    };
    let children = node.children();
    cx.text = shaped_texts.get(&node_addr(node)).map(std::rc::Rc::as_ref);
    crate::paint::paint_node_with_children(
        canvas,
        node,
        style,
        &alloc,
        overrides,
        cx,
        |canvas, cx| {
            cx.text = None;
            for child in children {
                // Only the root carries the window's sampled overrides: a
                // child never computed those properties for itself.
                paint_subtree(
                    canvas,
                    &child,
                    layout,
                    styles,
                    None,
                    cx,
                    depth + 1,
                    shaped_texts,
                );
            }
        },
    );
}

impl Dispatch<wl_registry::WlRegistry, ()> for WindowState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            // Bound at 6 (not 4): `wl_surface.preferred_buffer_scale` is
            // since-version 6, and a `wl_surface` created from a lower-bound
            // `wl_compositor` never gets that event at all.
            "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(6), qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, version.min(5), qh, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
            "wp_cursor_shape_manager_v1" => {
                state.cursor_manager = Some(registry.bind(name, version.min(2), qh, ()));
            }
            "wl_data_device_manager" => {
                state.data_device_manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "zwp_primary_selection_device_manager_v1" => {
                state.primary_manager = Some(registry.bind(name, version.min(1), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WindowState {
    /// A ping unanswered for long enough gets the client declared unresponsive
    /// and killed, so it is answered here and never surfaces as an
    /// `InputEvent`.
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, SurfaceTarget> for WindowState {
    /// The terminal event of a configure sequence: the role-specific one
    /// arrived first and is already recorded.
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        target: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.pending_ack.retain(|(t, _)| t != target);
            state.pending_ack.push((*target, serial));
            if *target == SurfaceTarget::Window {
                let size = state.configured.unwrap_or(state.default_size);
                state.events.push(InputEvent::Configure {
                    size,
                    states: state.states,
                });
            }
        }
    }
}

impl Dispatch<xdg_positioner::XdgPositioner, ()> for WindowState {
    fn event(
        _: &mut Self,
        _: &xdg_positioner::XdgPositioner,
        _: xdg_positioner::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_popup::XdgPopup, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        target: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let SurfaceTarget::Popup(key) = target else {
            return;
        };
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                state.popup_configures.push((
                    *key,
                    (x, y),
                    (
                        u32::try_from(width).unwrap_or(1).max(1),
                        u32::try_from(height).unwrap_or(1).max(1),
                    ),
                ));
            }
            // The compositor dismissed it (a click outside a grab, the parent
            // going away). The client must destroy it; `Window` does that when
            // the app acts on the event.
            xdg_popup::Event::PopupDone => state.events.push(InputEvent::PopupDone(*key)),
            xdg_popup::Event::Repositioned { token } => {
                state
                    .events
                    .push(InputEvent::Repositioned { key: *key, token });
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                // "You choose" falls back to the last configured size, or --
                // on the *first* configure, when there is none -- to the size
                // the client asked for in its `SurfaceSpec`. Taking the
                // event's own zero here would map a 1x1 surface.
                let size = configured_size_with_default(
                    state.configured,
                    state.default_size,
                    width,
                    height,
                );
                state.configured = Some(size);
                // A layer surface has no `xdg_toplevel.state`; ACTIVATED keeps
                // the root out of `:backdrop`.
                state.states = SurfaceStates::ACTIVATED;
                state.events.push(InputEvent::Configure {
                    size,
                    states: state.states,
                });
            }
            zwlr_layer_surface_v1::Event::Closed => state.close(),
            _ => {}
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                // A zero dimension means "you choose": keep what we have, and
                // on the *first* configure -- when there is nothing to keep --
                // fall back to the size the client asked for in its
                // `SurfaceSpec`, not to zero. A compositor sending 0x0 so the
                // client can pick its own size is the ordinary xdg-shell case,
                // and `Window::open`'s `.max(1)` would turn a `Some((0, 0))`
                // into a 1x1 buffer.
                let (w, h) = (
                    u32::try_from(width).unwrap_or(0),
                    u32::try_from(height).unwrap_or(0),
                );
                state.configured = Some(configured_size_with_default(
                    state.configured,
                    state.default_size,
                    w,
                    h,
                ));
                state.states = SurfaceStates::from_wire(&states);
            }
            xdg_toplevel::Event::Close => state.close(),
            // `ConfigureBounds` and `WmCapabilities` are advisory; a window
            // that ignores them is well-behaved, and M3 has no UI for either.
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, SurfaceTarget> for WindowState {
    /// `enter`/`leave`/`preferred_buffer_transform` are Task 12's; only the
    /// scale factor is this task's.
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && factor > 0
            && factor != state.scale
        {
            state.scale = factor;
            state.events.push(InputEvent::ScaleChanged(factor));
        }
    }
}

impl Dispatch<wl_callback::WlCallback, SurfaceTarget> for WindowState {
    /// `wl_surface.frame`'s callback: the one clock-agnostic "you may draw
    /// now" signal. `Window::render` (Task 12) fills in `now`; the state
    /// holds no clock of its own.
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.events.push(InputEvent::Frame {
                now: Duration::ZERO,
            });
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferSlot> for WindowState {
    /// Queued rather than applied here: the pools live on [`Window`], and one
    /// window has several, so the buffer's own id is what tells them apart.
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        slot: &BufferSlot,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            state.released.push((buffer.id(), *slot));
        }
    }
}

delegate_noop!(WindowState: ignore wl_compositor::WlCompositor);
delegate_noop!(WindowState: ignore wl_shm::WlShm);
delegate_noop!(WindowState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WindowState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(WindowState: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(WindowState: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);

impl Dispatch<wl_seat::WlSeat, ()> for WindowState {
    /// Capabilities are the seat's *whole* current set, not an addition.
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        sync_capability(
            caps.contains(wl_seat::Capability::Pointer),
            &mut state.pointer,
            || seat.get_pointer(qh, ()),
            |p| {
                if p.version() >= 3 {
                    p.release();
                }
            },
        );
        sync_capability(
            caps.contains(wl_seat::Capability::Keyboard),
            &mut state.keyboard,
            || seat.get_keyboard(qh, ()),
            |k| {
                if k.version() >= 3 {
                    k.release();
                }
            },
        );
        sync_capability(
            caps.contains(wl_seat::Capability::Touch),
            &mut state.touch,
            || seat.get_touch(qh, ()),
            |t| {
                if t.version() >= 3 {
                    t.release();
                }
            },
        );
        if state.keyboard.is_none() {
            state.keymap = None;
        }
    }
}

/// Add or drop one seat capability's object, idempotently.
///
/// `wl_seat.capabilities` reports the seat's whole current set on every
/// change, not a delta: a client that only ever adds keeps dispatching to a
/// pointer the compositor took away.
fn sync_capability<T>(
    present: bool,
    slot: &mut Option<T>,
    create: impl FnOnce() -> T,
    destroy: impl FnOnce(&T),
) {
    match (present, slot.take()) {
        (true, Some(existing)) => *slot = Some(existing),
        (true, None) => *slot = Some(create()),
        (false, Some(existing)) => destroy(&existing),
        (false, None) => {}
    }
}

/// `wl_pointer.axis_source`.
///
/// An unknown source (a future protocol addition) reads as a wheel: the
/// conservative choice, since a wheel scroll never coasts after the user
/// stopped, while guessing `Finger` would make an unrecognised source coast
/// forever.
fn scroll_source_of(source: WEnum<wl_pointer::AxisSource>) -> ScrollSource {
    match source {
        WEnum::Value(wl_pointer::AxisSource::Finger) => ScrollSource::Finger,
        WEnum::Value(wl_pointer::AxisSource::Continuous) => ScrollSource::Continuous,
        WEnum::Value(wl_pointer::AxisSource::WheelTilt) => ScrollSource::WheelTilt,
        _ => ScrollSource::Wheel,
    }
}

/// Fold one `wl_pointer.axis` event into the scroll frame being assembled.
///
/// `axis`, `axis_source` and `axis_stop` are separate events that mean
/// nothing apart: two axes reported within the same frame are one diagonal
/// scroll, not two, so this accumulates rather than overwrites.
fn accumulate_axis(
    pending: &mut Option<Scroll>,
    axis: WEnum<wl_pointer::Axis>,
    value: f32,
    time_ms: u32,
) {
    let scroll = pending.get_or_insert(Scroll {
        dx: 0.0,
        dy: 0.0,
        source: ScrollSource::Wheel,
        stop: false,
        time_ms,
    });
    match axis {
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => scroll.dx += value,
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => scroll.dy += value,
        _ => {}
    }
    scroll.time_ms = time_ms;
}

impl Dispatch<wl_pointer::WlPointer, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.seat_serial = Some(serial);
                let target = state.target_of(&surface);
                state.events.push(InputEvent::PointerEnter {
                    x: surface_x,
                    y: surface_y,
                    serial,
                    target,
                });
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => {
                state.events.push(InputEvent::PointerMotion {
                    x: surface_x,
                    y: surface_y,
                    time_ms: time,
                });
            }
            wl_pointer::Event::Leave { serial, .. } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::PointerLeave);
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: pressed,
            } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::PointerButton {
                    button,
                    pressed: matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed)),
                    serial,
                    time_ms: time,
                });
            }
            wl_pointer::Event::Axis { time, axis, value } => {
                accumulate_axis(&mut state.axis, axis, value as f32, time);
            }
            wl_pointer::Event::AxisSource { axis_source } => {
                let source = scroll_source_of(axis_source);
                state
                    .axis
                    .get_or_insert(Scroll {
                        dx: 0.0,
                        dy: 0.0,
                        source,
                        stop: false,
                        time_ms: 0,
                    })
                    .source = source;
            }
            wl_pointer::Event::AxisStop { time, .. } => {
                let scroll = state.axis.get_or_insert(Scroll {
                    dx: 0.0,
                    dy: 0.0,
                    source: ScrollSource::Finger,
                    stop: true,
                    time_ms: time,
                });
                scroll.stop = true;
                scroll.time_ms = time;
            }
            // One `frame` is one logical scroll: the axis, its source and its
            // stop arrive as separate events and mean nothing apart.
            wl_pointer::Event::Frame => {
                if let Some(scroll) = state.axis.take() {
                    state.events.push(InputEvent::Scroll(scroll));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if !matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    tracing::warn!(?format, "ignoring a keymap in an unknown format");
                    return;
                }
                match Keymap::from_fd(fd, size as usize) {
                    Ok(keymap) => state.keymap = Some(keymap),
                    // Untrusted input: a compositor that hands us a keymap we
                    // cannot compile leaves us keyboardless, not dead.
                    Err(err) => tracing::error!(%err, "the compositor's keymap did not compile"),
                }
            }
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                state.seat_serial = Some(serial);
                let target = state.target_of(&surface);
                state
                    .events
                    .push(InputEvent::KeyboardEnter { serial, target });
            }
            wl_keyboard::Event::Leave { serial, .. } => {
                state.seat_serial = Some(serial);
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.clear_repeat();
                }
                state.events.push(InputEvent::KeyboardLeave);
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                state.seat_serial = Some(serial);
                let pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                let Some(keymap) = state.keymap.as_mut() else {
                    return;
                };
                let event = keymap.translate(key, pressed, serial, time);
                // The repeat clock is the animation clock, which the state
                // does not hold; `Window::pump` arms it from the event it sees.
                state.events.push(InputEvent::Key(event));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.update_mask(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.set_repeat_info(rate, delay);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down {
                serial,
                time,
                id,
                x,
                y,
                ..
            } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::TouchDown {
                    id,
                    x,
                    y,
                    serial,
                    time_ms: time,
                });
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                state.events.push(InputEvent::TouchMotion {
                    id,
                    x,
                    y,
                    time_ms: time,
                });
            }
            wl_touch::Event::Up { serial, time, id } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::TouchUp {
                    id,
                    serial,
                    time_ms: time,
                });
            }
            _ => {}
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

    use super::{InputEvent, MapPhase, Role, SurfaceSpec, SurfaceTarget, may_attach};
    use crate::css::node::PseudoStates;

    #[test]
    fn a_configure_without_activated_backdrops_the_root() {
        // The whole reason SurfaceStates exists: GTK paints an unfocused
        // window's chrome differently, through `:backdrop`.
        let root = Node::new("window");
        super::apply_configure_states(&root, SurfaceStates::ACTIVATED | SurfaceStates::MAXIMIZED);
        assert!(!root.states().contains(PseudoStates::BACKDROP));
        super::apply_configure_states(&root, SurfaceStates::MAXIMIZED);
        assert!(root.states().contains(PseudoStates::BACKDROP));
        super::apply_configure_states(&root, SurfaceStates::ACTIVATED);
        assert!(!root.states().contains(PseudoStates::BACKDROP));
        // A layer surface never gets states at all, and must not therefore be
        // permanently backdropped: the caller passes ACTIVATED for those.
        assert!(SurfaceStates::from_wire(&[]).is_empty());
    }

    #[test]
    fn nothing_is_attached_before_the_first_configure() {
        // xdg-shell's own sequencing rule: create the role, commit with *no*
        // buffer, wait for configure, ack, then attach. Attaching early is a
        // protocol error that kills the client.
        assert!(!may_attach(MapPhase::Created));
        assert!(!may_attach(MapPhase::AwaitingConfigure));
        assert!(may_attach(MapPhase::Configured));
    }

    #[test]
    fn interest_maps_onto_poll_flags() {
        use rustix::event::PollFlags;
        assert_eq!(super::Interest::Read.flags(), PollFlags::IN);
        assert_eq!(super::Interest::Write.flags(), PollFlags::OUT);
        assert_eq!(
            super::Interest::ReadWrite.flags(),
            PollFlags::IN | PollFlags::OUT
        );
    }

    #[test]
    fn a_watch_set_polls_the_wayland_fd_first_and_the_watches_in_registration_order() {
        // The poll set's shape is the whole of M5-D1's semantics §1: element 0
        // is always the connection, elements 1.. are the watches as registered.
        // Building it is a pure function of the registry, so it is tested
        // without a compositor; `ui/tests/ingress.rs` proves the live wake.
        use rustix::event::PollFlags;
        let (a_read, _a_write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .expect("a pipe");
        let (b_read, _b_write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .expect("a pipe");
        let mut watches = super::Watches::default();
        let first = watches.add(a_read, super::Interest::Read);
        let second = watches.add(b_read, super::Interest::ReadWrite);
        assert_eq!(watches.ids(), vec![first, second]);
        assert_ne!(first, second, "ids are never reused inside one window");
        let flags: Vec<PollFlags> = watches.entries().map(|w| w.flags).collect();
        assert_eq!(flags, vec![PollFlags::IN, PollFlags::IN | PollFlags::OUT]);

        watches.remove(first);
        assert_eq!(watches.ids(), vec![second]);
        // Removing an id twice, or one that was never issued, is a no-op.
        watches.remove(first);
        watches.remove(super::WatchId(4242));
        assert_eq!(watches.ids(), vec![second]);
    }

    #[test]
    fn a_pump_on_a_silent_compositor_times_out_rather_than_hanging() {
        // M1's `the_configure_wait_is_bounded`, generalised: a compositor that
        // accepts the connection and then says nothing must not hang the pump.
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = super::WindowState::new_for_test();
        let started = std::time::Instant::now();
        let result = super::wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::from_millis(120),
            &super::Watches::default(),
            |state| !state.events.is_empty(),
        );
        assert!(
            matches!(result, Err(SurfaceError::Timeout(_))),
            "{result:?}"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "it returned early"
        );
        assert!(started.elapsed() < Duration::from_secs(5), "it hung");
    }

    #[test]
    fn a_closed_surface_ends_the_wait_immediately_and_reports_close_once() {
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = super::WindowState::new_for_test();
        state.close();
        let started = std::time::Instant::now();
        let result = super::wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::from_secs(5),
            &super::Watches::default(),
            |_| false,
        );
        assert!(matches!(result, Err(SurfaceError::Closed)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            state
                .events
                .iter()
                .filter(|e| matches!(e, InputEvent::Close))
                .count(),
            1,
            "closing twice must not queue two Close events"
        );
        state.close();
        assert_eq!(
            state
                .events
                .iter()
                .filter(|e| matches!(e, InputEvent::Close))
                .count(),
            1
        );
    }

    #[test]
    fn an_unbounded_wait_never_overflows_its_deadline() {
        // `pump(None)` is `Duration::MAX`, and `Instant + Duration::MAX`
        // panics. The close short-circuit is what makes this observable
        // without actually blocking forever.
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = super::WindowState::new_for_test();
        state.close();
        let result = super::wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::MAX,
            &super::Watches::default(),
            |_| false,
        );
        assert!(matches!(result, Err(SurfaceError::Closed)), "{result:?}");
    }

    #[test]
    fn a_surface_spec_names_a_role_and_a_size() {
        let spec = SurfaceSpec {
            role: Role::Toplevel,
            size: (400, 300),
            title: "Settings".into(),
            app_id: "org.icedtea.Settings".into(),
        };
        assert_eq!(spec.size, (400, 300));
        assert!(matches!(spec.role, Role::Toplevel));
        assert_eq!(SurfaceTarget::Window, SurfaceTarget::Window);
        assert_ne!(
            SurfaceTarget::Window,
            SurfaceTarget::Popup(crate::window::popup::PopupKey(1))
        );
    }

    #[test]
    fn a_scroll_is_assembled_across_its_parts_and_emitted_whole() {
        // `axis`, `axis_source` and `axis_stop` are separate events and mean
        // nothing apart: two axes in one frame are one diagonal scroll, not two.
        use super::accumulate_axis;
        use wayland_client::WEnum;
        use wayland_client::protocol::wl_pointer;

        let mut pending = None;
        accumulate_axis(
            &mut pending,
            WEnum::Value(wl_pointer::Axis::VerticalScroll),
            -10.0,
            5,
        );
        accumulate_axis(
            &mut pending,
            WEnum::Value(wl_pointer::Axis::HorizontalScroll),
            4.0,
            5,
        );
        accumulate_axis(
            &mut pending,
            WEnum::Value(wl_pointer::Axis::VerticalScroll),
            -6.0,
            7,
        );
        let scroll = pending.take().expect("a pending scroll");
        assert_eq!(scroll.dy, -16.0, "the two vertical steps accumulate");
        assert_eq!(scroll.dx, 4.0);
        assert_eq!(scroll.time_ms, 7, "the latest timestamp wins");
        assert!(pending.is_none(), "taking the frame clears it");
    }

    #[test]
    fn an_unknown_axis_or_source_is_ignored_rather_than_guessed() {
        use super::{accumulate_axis, scroll_source_of};
        use crate::window::pointer::ScrollSource;
        use wayland_client::WEnum;
        use wayland_client::protocol::wl_pointer;

        assert_eq!(
            scroll_source_of(WEnum::Value(wl_pointer::AxisSource::Finger)),
            ScrollSource::Finger
        );
        assert_eq!(
            scroll_source_of(WEnum::Value(wl_pointer::AxisSource::Wheel)),
            ScrollSource::Wheel
        );
        assert_eq!(
            scroll_source_of(WEnum::Value(wl_pointer::AxisSource::WheelTilt)),
            ScrollSource::WheelTilt
        );
        assert_eq!(
            scroll_source_of(WEnum::Unknown(99)),
            ScrollSource::Wheel,
            "an unknown source must not coast: a wheel is the conservative reading"
        );
        let mut pending = None;
        accumulate_axis(&mut pending, WEnum::Unknown(7), 10.0, 1);
        assert!(
            pending.as_ref().is_none_or(|s| s.dx == 0.0 && s.dy == 0.0),
            "an unknown axis moved something"
        );
    }

    #[test]
    fn a_seat_capability_is_created_once_and_destroyed_once() {
        // `wl_seat.capabilities` is the seat's whole current set, not a delta:
        // a client that only ever adds keeps dispatching to a pointer the
        // compositor took away.
        use super::sync_capability;
        let (mut created, mut destroyed) = (0, 0);
        let mut slot: Option<i32> = None;
        for present in [true, true, true] {
            sync_capability(
                present,
                &mut slot,
                || {
                    created += 1;
                    7
                },
                |_| destroyed += 1,
            );
        }
        assert_eq!(
            (created, destroyed),
            (1, 0),
            "an unchanged capability is not recreated"
        );
        sync_capability(
            false,
            &mut slot,
            || {
                created += 1;
                7
            },
            |_| destroyed += 1,
        );
        assert_eq!((created, destroyed), (1, 1));
        assert!(slot.is_none());
        sync_capability(
            false,
            &mut slot,
            || {
                created += 1;
                7
            },
            |_| destroyed += 1,
        );
        assert_eq!(
            (created, destroyed),
            (1, 1),
            "losing what we never had does nothing"
        );
    }

    /// A connection to a socket nobody ever writes to.
    fn silent_connection() -> (
        wayland_client::Connection,
        wayland_client::EventQueue<super::WindowState>,
        std::os::unix::net::UnixStream,
    ) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let conn = wayland_client::Connection::from_socket(ours).expect("connection");
        let queue = conn.new_event_queue();
        (conn, queue, theirs)
    }

    /// A `display: none` subtree is not on screen, so it publishes no probe
    /// point: a gate that aimed a pointer at one clicked the visible page.
    ///
    /// Mutation check: walk `root.descendants()` unfiltered again and the
    /// hidden page's own node is reported with a centre of its own. Restore.
    #[test]
    fn probe_points_skip_a_hidden_subtree() {
        use crate::css::node::Node;
        use crate::layout::{FixedMeasure, LayoutTree};
        use taffy::style::AvailableSpace;

        let root = Node::new("window");
        let shown = Node::new("box");
        let hidden = Node::new("stackpage");
        let buried = Node::new("entry");
        hidden.append_child(&buried);
        root.append_child(&shown);
        root.append_child(&hidden);
        crate::widgets::set_displayed(&hidden, false);

        let mut layout = LayoutTree::new();
        layout.sync(&root).expect("sync");
        crate::widgets::flush_layout(&mut layout);
        layout
            .compute(
                &root,
                taffy::Size {
                    width: AvailableSpace::Definite(200.0),
                    height: AvailableSpace::Definite(200.0),
                },
                &mut FixedMeasure(taffy::Size {
                    width: 40.0,
                    height: 20.0,
                }),
            )
            .expect("compute");

        let labels: Vec<String> = super::probe_points_of(&root, &layout)
            .into_iter()
            .map(|point| point.label)
            .collect();
        assert!(labels.iter().any(|l| l == "box"), "{labels:?}");
        assert!(
            !labels.iter().any(|l| l == "stackpage"),
            "the hidden page is not a probe target: {labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "entry"),
            "and neither is anything buried in it: {labels:?}"
        );
    }

    /// A `Duration::ZERO` wait still polls: it is "drain what is there", and
    /// an app whose controllers report a zero deadline every frame
    /// (`GridViewC` did) gets its watched fds and its Wayland socket read.
    ///
    /// Mutation check: return `Err(Timeout)` on the first expiry again (what
    /// `wait_bounded` did before the poll set was ever built on a zero wait)
    /// and no `FdReady` is produced -- the starvation this asserts against.
    /// Restore.
    #[test]
    fn a_zero_wait_still_polls_the_watched_fds_once() {
        use super::{InputEvent, Interest, Watches, wait_bounded};

        let (conn, mut queue, _theirs) = silent_connection();
        let mut state = super::WindowState::new();

        let (read, write) = rustix::pipe::pipe().expect("pipe");
        rustix::io::write(&write, b"x").expect("wake byte");
        let mut watches = Watches::default();
        let id = watches.add(read, Interest::Read);

        let outcome = wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::ZERO,
            &watches,
            |state| !state.events.is_empty(),
        );
        assert!(
            outcome.is_ok(),
            "the ready watch is an answer, not a timeout"
        );
        assert!(
            state
                .events
                .iter()
                .any(|event| matches!(event, InputEvent::FdReady(fired) if *fired == id)),
            "the watch that was ready is reported: {:?}",
            state.events
        );
    }

    #[test]
    fn deadlines_fold_to_the_soonest_and_zero_means_now() {
        use super::fold_deadlines;
        let ms = Duration::from_millis;
        assert_eq!(fold_deadlines(None, None), None);
        assert_eq!(fold_deadlines(Some(ms(16)), None), Some(ms(16)));
        assert_eq!(fold_deadlines(None, Some(ms(16))), Some(ms(16)));
        assert_eq!(fold_deadlines(Some(ms(40)), Some(ms(16))), Some(ms(16)));
        assert_eq!(
            fold_deadlines(Some(Duration::ZERO), Some(ms(16))),
            Some(Duration::ZERO),
            "ZERO is a legitimate `now`, not a missing answer"
        );
    }

    #[test]
    fn a_dirty_tree_and_a_live_animation_are_the_only_reasons_to_paint() {
        use super::should_paint;
        assert!(should_paint(true, false), "a dirty tree paints");
        assert!(should_paint(false, true), "a live animation paints");
        assert!(should_paint(true, true));
        assert!(
            !should_paint(false, false),
            "an idle window must not repaint: every commit costs a buffer and a frame callback"
        );
    }

    #[test]
    fn a_first_configure_with_a_zero_dimension_falls_back_to_the_requested_size() {
        // "You choose" is the ordinary xdg-shell first configure: a compositor
        // that has no opinion sends 0x0. There is no previous size to keep at
        // that point, and taking the zero literally collapses the window --
        // `Window::open`'s `.max(1)` floor turns `Some((0, 0))` into a 1x1
        // buffer. The client's own `SurfaceSpec::size` is the fallback, for
        // the toplevel role exactly as for the layer one.
        use super::configured_size_with_default;
        let spec = (800, 600);
        assert_eq!(
            configured_size_with_default(None, spec, 0, 0),
            (800, 600),
            "0x0 on the first configure means the size we asked for"
        );
        assert_eq!(
            configured_size_with_default(None, spec, 1024, 0),
            (1024, 600)
        );
        assert_eq!(configured_size_with_default(None, spec, 0, 480), (800, 480));
        assert_eq!(
            configured_size_with_default(None, spec, 1024, 480),
            (1024, 480),
            "a concrete configure still wins outright"
        );
        // And a *later* zero keeps what the compositor last chose, not the
        // spec: `previous` is `state.configured` once there is one.
        assert_eq!(
            configured_size_with_default(Some((1024, 480)), spec, 0, 0),
            (1024, 480)
        );
    }

    #[test]
    fn an_animating_popup_gets_the_window_woken_for_it() {
        // A popup has its own `AnimationState` and `render_popups` paints it
        // on its own schedule. If its deadline never reaches `next_deadline`,
        // a caller sleeping for `pump(window.next_deadline())` stalls the
        // menu's transition at its first frame until something unrelated
        // arrives.
        use super::next_deadline_of;
        let ms = Duration::from_millis;
        assert_eq!(
            next_deadline_of(None, None, [Some(ms(8))]),
            Some(ms(8)),
            "an idle window with an animating popup still has a deadline"
        );
        assert_eq!(
            next_deadline_of(Some(ms(50)), None, [Some(ms(8)), None]),
            Some(ms(8)),
            "the soonest of the window's and every popup's wins"
        );
        assert_eq!(
            next_deadline_of(Some(ms(4)), Some(ms(20)), [Some(ms(8))]),
            Some(ms(4)),
            "and the window's own can still be the soonest"
        );
        assert_eq!(
            next_deadline_of(None, None, [None, None]),
            None,
            "idle popups add no deadline at all"
        );
        assert_eq!(
            next_deadline_of(None, None, [Some(Duration::ZERO)]),
            Some(Duration::ZERO),
            "ZERO is a real answer -- \"now\" -- not \"no deadline\""
        );
    }

    #[test]
    fn a_frame_callback_is_stamped_with_the_pump_s_own_clock() {
        // `wl_callback.done`'s timestamp is in the compositor's clock domain
        // and the `Dispatch` impl has no clock at all, so it pushes a
        // placeholder. A `Frame` that reached the caller still holding
        // `Duration::ZERO` would tell an animation it is forever at time zero.
        use super::{InputEvent, stamp_frames};
        let now = Duration::from_millis(1234);
        let mut batch = vec![
            InputEvent::Frame {
                now: Duration::ZERO,
            },
            InputEvent::PointerLeave,
            InputEvent::Frame {
                now: Duration::ZERO,
            },
        ];
        stamp_frames(&mut batch, now);
        let stamped: Vec<Duration> = batch
            .iter()
            .filter_map(|e| match e {
                InputEvent::Frame { now } => Some(*now),
                _ => None,
            })
            .collect();
        assert_eq!(stamped, vec![now, now], "every Frame in the batch is dated");
        assert!(
            matches!(batch[1], InputEvent::PointerLeave),
            "and nothing else in the batch is touched"
        );
    }

    #[test]
    fn a_released_buffer_is_matched_to_its_own_pool() {
        // One window has a pool per surface, and `BufferSlot` indices collide
        // across them: releasing slot 0 of a popup's pool must not free slot 0
        // of the window's.
        use super::release_matches;
        assert!(release_matches(Some(7), 7));
        assert!(!release_matches(Some(7), 8));
        assert!(
            !release_matches(None, 7),
            "a slot the pool never had is not ours"
        );
    }
}
