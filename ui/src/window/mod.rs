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

use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface, wl_touch,
};
use wayland_client::{
    ConnectError, Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle,
    delegate_noop,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::anim::{AnimationState, Clock, MonotonicClock};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::shm::{BufferPool, BufferSlot};
use crate::text::FontDatabase;
use keyboard::{KeyEvent, Keymap};
use pointer::Scroll;
use popup::PopupKey;

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
pub(crate) fn apply_configure_states(root: &Node, states: SurfaceStates) {
    root.set_state(
        PseudoStates::BACKDROP,
        !states.contains(SurfaceStates::ACTIVATED),
    );
}

/// One of the three surface roles.
///
/// Task 12 adds `Layer` and `Popup` together with the role-shared accessors
/// (`wl_surface`, `size`, `scale`, `states`, `resize`, `commit_buffer`); Task
/// 10 needs only the role it maps.
pub enum Surface {
    Toplevel(toplevel::Toplevel),
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

/// Everything the dispatch handlers write and [`Window`] reads.
pub(crate) struct WindowState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    /// Read by Task 13's layer role.
    #[allow(dead_code)]
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// Read by Task 12's `move`/`resize`/`show_window_menu` plumbing.
    #[allow(dead_code)]
    seat: Option<wl_seat::WlSeat>,
    /// Task 11's seat-capability handler creates these three.
    #[allow(dead_code)]
    pointer: Option<wl_pointer::WlPointer>,
    #[allow(dead_code)]
    keyboard: Option<wl_keyboard::WlKeyboard>,
    #[allow(dead_code)]
    touch: Option<wl_touch::WlTouch>,
    /// Read by Task 12's `Surface::set_cursor_shape`.
    #[allow(dead_code)]
    cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    /// Compiled from `wl_keyboard.keymap`; `None` until it arrives.
    keymap: Option<Keymap>,
    /// The events `pump` will hand up, in arrival order.
    events: Vec<InputEvent>,
    /// Buffers the compositor released, by `wl_buffer` id: one window has
    /// several pools and `BufferSlot` alone cannot say which. Drained by
    /// Task 12's repaint.
    #[allow(dead_code)]
    released: Vec<(wayland_client::backend::ObjectId, BufferSlot)>,
    /// The most recent serial from any input event, for `grab`, `move`,
    /// `resize` and the clipboard.
    seat_serial: Option<u32>,
    /// The last `xdg_surface.configure` serial per surface, to ack on commit.
    #[allow(dead_code)]
    pending_ack: Vec<(SurfaceTarget, u32)>,
    configured: Option<(u32, u32)>,
    states: SurfaceStates,
    /// Task 11 updates this from `wl_surface.preferred_buffer_scale`.
    #[allow(dead_code)]
    scale: i32,
    closed: bool,
    /// A pending scroll frame, accumulated until `wl_pointer.frame`
    /// (Task 11).
    #[allow(dead_code)]
    axis: Option<Scroll>,
    /// Set by Task 14's popup dispatch.
    #[allow(dead_code)]
    pub(crate) popup_events: Vec<InputEvent>,
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
            keymap: None,
            events: Vec::new(),
            released: Vec::new(),
            seat_serial: None,
            pending_ack: Vec::new(),
            configured: None,
            states: SurfaceStates::empty(),
            scale: 1,
            closed: false,
            axis: None,
            popup_events: Vec::new(),
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
    ready: impl Fn(&WindowState) -> bool,
) -> Result<(), SurfaceError> {
    use rustix::event::{PollFlags, Timespec};
    use std::os::fd::AsFd;

    // `Duration::MAX` is `pump(None)`'s "block until something happens", and
    // `Instant + Duration::MAX` panics: an unrepresentable deadline is `None`,
    // which polls without a timespec rather than overflowing.
    let deadline = Instant::now().checked_add(timeout);
    loop {
        queue
            .dispatch_pending(state)
            .map_err(SurfaceError::Dispatch)?;
        if state.closed {
            return Err(SurfaceError::Closed);
        }
        if ready(state) {
            return Ok(());
        }

        let now = Instant::now();
        let remaining = match deadline {
            Some(deadline) if now >= deadline => return Err(SurfaceError::Timeout(timeout)),
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
        let mut fds = [rustix::event::PollFd::new(&fd, PollFlags::IN)];
        let timespec = remaining.map(|remaining| Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        });
        match rustix::event::poll(&mut fds, timespec.as_ref()) {
            // Only reachable with a timespec: a `None` deadline means poll
            // blocks until the fd speaks.
            Ok(0) => return Err(SurfaceError::Timeout(timeout)),
            Ok(_) => match guard.read() {
                Ok(_) => {}
                // A racing reader on another queue drained the socket.
                Err(wayland_client::backend::WaylandError::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(socket_error(err)),
            },
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

/// One window: a surface, its retained tree, and the pump that drives them.
pub struct Window {
    conn: Connection,
    queue: EventQueue<WindowState>,
    /// Task 12 reallocates the buffer pool through this on a resize.
    #[allow(dead_code)]
    qh: QueueHandle<WindowState>,
    state: WindowState,
    surface: Surface,
    /// Task 12 reallocates the buffer pool through this on a resize.
    #[allow(dead_code)]
    shm: wl_shm::WlShm,
    #[allow(dead_code)]
    buffers: BufferPool,
    #[allow(dead_code)]
    skia: skia_rs_safe::canvas::Surface,
    root: Node,
    layout: crate::layout::LayoutTree,
    styles: StyleMap,
    anim: AnimationState,
    /// Task 12's restyle pass resolves against these.
    #[allow(dead_code)]
    env: crate::css::computed::ResolveEnv,
    #[allow(dead_code)]
    images: crate::paint::ImageCache,
    sheet: CompiledSheet,
    fonts: FontDatabase,
    clock: Rc<dyn Clock>,
    /// Task 12's render gates its first attach on this.
    #[allow(dead_code)]
    phase: MapPhase,
    #[allow(dead_code)]
    dirty: bool,
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
        let mut queue: EventQueue<WindowState> = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = WindowState::new();
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
        let width = i32::try_from(spec.size.0.max(1)).unwrap_or(i32::MAX);
        let height = i32::try_from(spec.size.1.max(1)).unwrap_or(i32::MAX);

        let surface = match &spec.role {
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
                };
                toplevel.set_title(&spec.title);
                toplevel.set_app_id(&spec.app_id);
                Surface::Toplevel(toplevel)
            }
            // Task 13 maps `zwlr_layer_shell_v1` onto `Surface::Layer`. Until
            // it does, a layer request is refused locally rather than
            // half-mapped: `LayerWindow` is still the working layer client.
            Role::Layer(_) => {
                wl_surface.destroy();
                return Err(SurfaceError::Protocol(
                    "the zwlr_layer_shell_v1 role is not mapped by Window yet",
                ));
            }
        };
        // The empty commit: role state is double-buffered, and no buffer may
        // be attached until the configure it triggers comes back.
        wl_surface.commit();
        conn.flush().map_err(socket_error)?;

        wait_bounded(&conn, &mut queue, &mut state, CONFIGURE_TIMEOUT, |state| {
            state.configured.is_some()
        })?;
        let (width, height) = state.configured.map_or((width, height), |(w, h)| {
            (
                i32::try_from(w.max(1)).unwrap_or(width),
                i32::try_from(h.max(1)).unwrap_or(height),
            )
        });

        let buffers = BufferPool::new(&shm, &qh, width, height).map_err(SurfaceError::Shm)?;
        let skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(width, height)
            .ok_or(SurfaceError::Render("the raster surface to paint into"))?;
        let root = Node::with_classes("window", &["background"]);
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
            clock: Rc::new(MonotonicClock::new()),
            phase: MapPhase::Configured,
            dirty: true,
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
        let batch = std::mem::take(&mut self.state.events);
        // `:backdrop` is the one configure state the stylesheet reads, and the
        // root has to carry it before the caller restyles off this batch.
        for event in &batch {
            if let InputEvent::Configure { states, .. } = event {
                apply_configure_states(&self.root, *states);
            }
        }
        Ok(batch)
    }

    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
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

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.closed
    }
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
            "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(4), qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, version.min(5), qh, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
            "wp_cursor_shape_manager_v1" => {
                state.cursor_manager = Some(registry.bind(name, version.min(2), qh, ()));
            }
            // Task 15 binds the two selection managers here.
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
                let size = state.configured.unwrap_or((0, 0));
                state.events.push(InputEvent::Configure {
                    size,
                    states: state.states,
                });
            }
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
                // A zero dimension means "you choose": keep what we have.
                let (w, h) = (
                    u32::try_from(width).unwrap_or(0),
                    u32::try_from(height).unwrap_or(0),
                );
                let previous = state.configured.unwrap_or((0, 0));
                state.configured = Some((
                    if w == 0 { previous.0 } else { w },
                    if h == 0 { previous.1 } else { h },
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
    /// `enter`/`leave`/`preferred_buffer_transform` are Task 11's and Task
    /// 12's; the role's own surface has nothing to do with them here.
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
// Task 11 replaces this with the real capability handler and adds the pointer,
// keyboard and touch dispatch.
delegate_noop!(WindowState: ignore wl_seat::WlSeat);

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
        let result = super::wait_bounded(&conn, &mut queue, &mut state, Duration::MAX, |_| false);
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
}
