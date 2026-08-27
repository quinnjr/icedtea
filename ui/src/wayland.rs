//! The Wayland client: a `zwlr_layer_shell_v1` overlay surface with a
//! `wl_shm` buffer, driven by a blocking dispatch loop.
//!
//! Hand-rolled on `wayland-client` in the same shape the rest of this repo
//! already uses (see `harness/src/lib.rs`): no `smithay-client-toolkit`, no
//! `calloop`.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{
    ConnectError, Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle,
    delegate_noop,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;

use crate::css::cascade::CompiledSheet;
use crate::css::node::PseudoStates;
use crate::shm::{BufferPool, BufferSlot, ShmBuffer, Slot, SlotPool};
use crate::text::FontDatabase;
use crate::widget::button::Button;

/// Everything that can go wrong opening or running the window.
#[derive(Debug)]
pub enum LayerWindowError {
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
    /// The compositor closed the layer surface before it ever configured it.
    Closed,
    /// The compositor did not configure the layer surface in time.
    Timeout(Duration),
}

impl std::fmt::Display for LayerWindowError {
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
            Self::Closed => write!(
                f,
                "the compositor closed the layer surface before configuring it"
            ),
            Self::Timeout(after) => write!(
                f,
                "the compositor did not configure the layer surface within {after:?}"
            ),
        }
    }
}

impl std::error::Error for LayerWindowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) => Some(e),
            Self::Shm(e) | Self::Socket(e) => Some(e),
            Self::Dispatch(e) => Some(e),
            Self::MissingGlobal(_)
            | Self::Render(_)
            | Self::NoFont
            | Self::Closed
            | Self::Timeout(_) => None,
        }
    }
}

/// Client state: bound globals, pointer position, and the widget.
pub struct AppState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    /// `(width, height)` from the most recent layer-surface `configure`.
    configured: Option<(u32, u32)>,
    /// Set when the widget's appearance changed and the surface needs a repaint.
    dirty: bool,
    /// Set when the compositor asked us to go away.
    closed: bool,
    /// Whether BTN_LEFT is currently down, wherever the pointer is.
    ///
    /// `:active` is "this button is being pressed", which survives the
    /// pointer wandering off the widget: GTK re-arms the visual state when
    /// the pointer comes back while the mouse button is still down. Hover
    /// alone cannot express that, which is why it is tracked separately.
    held: bool,
    /// Buffer slots the compositor released since the last repaint.
    released: Vec<BufferSlot>,
    sheet: CompiledSheet,
    fonts: FontDatabase,
    button: Button,
}

impl AppState {
    /// A client state with nothing bound yet.
    fn new(sheet: CompiledSheet, fonts: FontDatabase, button: Button) -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            configured: None,
            dirty: true,
            closed: false,
            held: false,
            released: Vec::new(),
            sheet,
            fonts,
            button,
        }
    }

    /// Forget everything that depended on having a pointer.
    ///
    /// The compositor can take the pointer capability away at any time (the
    /// last mouse is unplugged); whatever the widget was showing because of
    /// the pointer is no longer true, so hover and active are cleared and
    /// the surface repainted.
    fn on_pointer_gone(&mut self) {
        self.held = false;
        self.update_states(false, false);
    }

    /// The pointer is at `(x, y)` in surface coordinates.
    fn on_pointer_motion(&mut self, x: f64, y: f64) {
        let inside = self.button.contains((0.0, 0.0), x, y);
        self.update_states(inside, inside && self.held);
    }

    /// The pointer left the surface. `held` survives: the mouse button is
    /// still down, and coming back must re-arm `:active`.
    fn on_pointer_leave(&mut self) {
        self.update_states(false, false);
    }

    /// A pointer button changed. Only BTN_LEFT drives `:active` -- a right
    /// click, a scroll-wheel click or a side button must not press the
    /// widget.
    fn on_pointer_button(&mut self, button: u32, pressed: bool) {
        if button != BTN_LEFT {
            return;
        }
        // A release anywhere ends the press, on or off the widget.
        self.held = pressed;
        let inside = self.button.states().contains(PseudoStates::HOVER);
        self.update_states(inside, pressed && inside);
    }

    /// Recompute state from the pointer, restyling only on an actual change.
    fn update_states(&mut self, hover: bool, active: bool) {
        let current = self.button.states();
        if current.contains(PseudoStates::HOVER) == hover
            && current.contains(PseudoStates::ACTIVE) == active
        {
            return;
        }
        tracing::debug!(hover, active, "pointer state changed");
        let mut states = PseudoStates::empty();
        states.set(PseudoStates::HOVER, hover);
        states.set(PseudoStates::ACTIVE, active);
        self.button.set_states(states, &self.sheet, &mut self.fonts);
        self.dirty = true;
    }
}

/// A mapped layer-shell window showing one themed button.
pub struct LayerWindow {
    conn: Connection,
    queue: EventQueue<AppState>,
    qh: QueueHandle<AppState>,
    state: AppState,
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    shm: wl_shm::WlShm,
    buffers: BufferPool,
    skia: Surface,
}

/// `BTN_LEFT` from Linux's `input-event-codes.h`: the only pointer button
/// that presses a widget.
pub const BTN_LEFT: u32 = 0x110;

/// Margin from the anchored corner, in px. Fixed so a screencopy test knows
/// exactly where on the output the button lands.
pub const MARGIN: i32 = 0;

/// How long [`LayerWindow::open`] waits for the first `configure`.
///
/// A compositor that never configures -- or that closes the surface instead
/// -- used to hang the client forever in an unconditional
/// `blocking_dispatch` loop.
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Block until `ready` is satisfied, `closed` arrives, or `timeout` expires.
///
/// wayland-client 0.31's own bounded-wait shape: dispatch what is already
/// queued, `prepare_read` to register interest, then `poll(2)` the
/// connection fd with the remaining time before reading. No helper thread,
/// and no `blocking_dispatch` that could outlive the deadline.
fn wait_bounded(
    conn: &Connection,
    queue: &mut EventQueue<AppState>,
    state: &mut AppState,
    timeout: Duration,
    ready: impl Fn(&AppState) -> bool,
) -> Result<(), LayerWindowError> {
    use rustix::event::{PollFlags, Timespec};

    let deadline = Instant::now() + timeout;
    loop {
        queue
            .dispatch_pending(state)
            .map_err(LayerWindowError::Dispatch)?;
        if state.closed {
            return Err(LayerWindowError::Closed);
        }
        if ready(state) {
            return Ok(());
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(LayerWindowError::Timeout(timeout));
        }
        let remaining = deadline - now;

        conn.flush().map_err(socket_error)?;
        // `None` means this queue already has events to dispatch: go round
        // rather than block on a socket that has nothing more to say.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };

        let fd = queue.as_fd();
        let mut fds = [rustix::event::PollFd::new(&fd, PollFlags::IN)];
        let timespec = Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        match rustix::event::poll(&mut fds, Some(&timespec)) {
            Ok(0) => return Err(LayerWindowError::Timeout(timeout)),
            Ok(_) => match guard.read() {
                Ok(_) => {}
                // A racing reader on another queue drained the socket.
                Err(wayland_client::backend::WaylandError::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(socket_error(err)),
            },
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(LayerWindowError::Socket(err.into())),
        }
    }
}

impl LayerWindow {
    /// Connect, bind globals, map an overlay layer surface sized to the
    /// button, and paint it once.
    ///
    /// # Errors
    ///
    /// [`LayerWindowError`] for a failed connection, a missing global, a
    /// failed shm allocation or a failed dispatch.
    pub fn open(
        sheet: CompiledSheet,
        mut fonts: FontDatabase,
        mut button: Button,
    ) -> Result<Self, LayerWindowError> {
        button.restyle(&sheet, &mut fonts);
        let allocation = button.allocation();
        let width = allocation.border_box.width.ceil().max(1.0) as i32;
        let height = allocation.border_box.height.ceil().max(1.0) as i32;
        tracing::info!(width, height, "opening layer window");

        let conn = Connection::connect_to_env().map_err(LayerWindowError::Connect)?;
        let display = conn.display();
        let mut queue: EventQueue<AppState> = conn.new_event_queue();
        let qh = queue.handle();
        display.get_registry(&qh, ());

        let mut state = AppState::new(sheet, fonts, button);
        queue
            .roundtrip(&mut state)
            .map_err(LayerWindowError::Dispatch)?;
        // A second roundtrip: `wl_seat.capabilities` arrives after the bind.
        queue
            .roundtrip(&mut state)
            .map_err(LayerWindowError::Dispatch)?;

        let compositor = state
            .compositor
            .clone()
            .ok_or(LayerWindowError::MissingGlobal("wl_compositor"))?;
        let shm = state
            .shm
            .clone()
            .ok_or(LayerWindowError::MissingGlobal("wl_shm"))?;
        let layer_shell = state
            .layer_shell
            .clone()
            .ok_or(LayerWindowError::MissingGlobal("zwlr_layer_shell_v1"))?;

        let surface = compositor.create_surface(&qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "icedtea-ui-themed-button".to_string(),
            &qh,
            (),
        );
        layer_surface
            .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        layer_surface.set_size(width as u32, height as u32);
        layer_surface.set_margin(MARGIN, MARGIN, MARGIN, MARGIN);
        layer_surface.set_exclusive_zone(0);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();
        conn.flush().map_err(socket_error)?;

        wait_bounded(&conn, &mut queue, &mut state, CONFIGURE_TIMEOUT, |state| {
            state.configured.is_some()
        })?;
        tracing::debug!(configured = ?state.configured, "layer surface configured");
        if let Some((configured_width, configured_height)) = state.configured
            && (configured_width != width as u32 || configured_height != height as u32)
        {
            // The compositor's `configure` is stored but never resized against
            // (no resize logic in M1): a mismatch here means the buffer we are
            // about to attach does not match what the compositor asked for,
            // which a compositor may reject as `invalid_surface_state` instead
            // of clamping silently. Logging it turns that into a diagnosable
            // warning rather than a mystery protocol error.
            tracing::warn!(
                configured_width,
                configured_height,
                buffer_width = width,
                buffer_height = height,
                "layer surface configure size does not match the button's buffer size"
            );
        }

        let buffers = BufferPool::new(&shm, &qh, width, height).map_err(LayerWindowError::Shm)?;
        let skia = Surface::new_raster_n32_premul(width, height)
            .ok_or(LayerWindowError::Render("the raster surface to paint into"))?;

        let mut window = Self {
            conn,
            queue,
            qh,
            state,
            surface,
            layer_surface,
            shm,
            buffers,
            skia,
        };
        window.repaint()?;
        Ok(window)
    }

    /// Paint the widget into a buffer the compositor is not holding, upload
    /// it, and commit.
    ///
    /// A buffer belongs to the compositor from the commit that attaches it
    /// until its `wl_buffer.release`, so this never writes into a busy one.
    /// If every buffer is busy and the pool is already at its maximum the
    /// frame is *deferred*: `dirty` stays set and the next release wakes the
    /// dispatch loop, which repaints then.
    fn repaint(&mut self) -> Result<(), LayerWindowError> {
        let index = match select_paint_slot(&mut self.state.released, self.buffers.slots_mut()) {
            Some(Slot::Existing(index)) => index,
            Some(Slot::New(index)) => {
                tracing::debug!(index, "every shm buffer was busy; growing the pool");
                let (width, height) = self.buffers.size();
                let buffer = ShmBuffer::new(&self.shm, &self.qh, width, height, BufferSlot(index))
                    .map_err(LayerWindowError::Shm)?;
                self.buffers.install(index, buffer);
                index
            }
            None => {
                tracing::debug!(
                    buffers = self.buffers.slots().len(),
                    "every shm buffer is still held by the compositor; deferring the frame"
                );
                self.state.dirty = true;
                return Ok(());
            }
        };

        let (width, height) = self.buffers.size();
        self.skia.canvas().clear(Color::TRANSPARENT);
        self.state.button.render(
            &mut self.skia,
            (0.0, 0.0),
            &self.state.sheet,
            &mut self.state.fonts,
            None,
        );
        self.buffers
            .upload(index, &self.skia)
            .map_err(LayerWindowError::Shm)?;
        self.surface
            .attach(Some(self.buffers.wl_buffer(index)), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
        self.conn.flush().map_err(socket_error)?;
        self.state.dirty = false;
        tracing::debug!(width, height, index, "repainted and committed");
        Ok(())
    }

    /// Dispatch until the compositor closes the surface, repainting whenever
    /// pointer state changed the widget's appearance.
    ///
    /// # Errors
    ///
    /// [`LayerWindowError`] if a dispatch, an shm upload or a flush fails.
    pub fn run(&mut self) -> Result<(), LayerWindowError> {
        while !self.state.closed {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(LayerWindowError::Dispatch)?;
            if self.state.dirty {
                self.repaint()?;
            }
        }
        Ok(())
    }
}

impl Drop for LayerWindow {
    /// Hand every object back before the connection goes: the pointer, the
    /// layer surface, then the `wl_surface` it wrapped. The buffers destroy
    /// themselves (`ShmBuffer`'s own `Drop`). A compositor keeps these alive
    /// until the client says otherwise, so a client that opens and closes
    /// windows would otherwise leak one of each per window.
    fn drop(&mut self) {
        if let Some(pointer) = self.state.pointer.take()
            && pointer.version() >= 3
        {
            pointer.release();
        }
        self.layer_surface.destroy();
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}

/// The release→repaint decision `repaint` makes before it touches Wayland
/// at all: drain the buffers the compositor released since the last frame
/// into `slots`, then report what to do with the result --
/// `Some(Slot::Existing(_))` to write straight into that buffer,
/// `Some(Slot::New(_))` to grow the pool (which does need the live
/// `shm`/`qh`, so `repaint` handles that arm itself), or `None` to defer
/// the frame with `dirty` left set.
///
/// Pure -- no Wayland objects -- so the wiring is unit-testable without a
/// live connection, unlike `repaint` itself.
fn select_paint_slot(released: &mut Vec<BufferSlot>, slots: &mut SlotPool) -> Option<Slot> {
    for slot in released.drain(..) {
        slots.release(slot.0);
    }
    slots.acquire()
}

/// `Connection::flush`/`read` report a `WaylandError`, which is `io::Error`
/// plus a protocol variant; both mean the socket is unusable, so both fold
/// into [`LayerWindowError::Socket`].
fn socket_error(err: wayland_client::backend::WaylandError) -> LayerWindowError {
    match err {
        wayland_client::backend::WaylandError::Io(io) => LayerWindowError::Socket(io),
        other => LayerWindowError::Socket(std::io::Error::other(other.to_string())),
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for AppState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Capabilities are the seat's *whole* current set, not an addition:
        // a seat that loses its last pointer sends the event again without
        // the bit, and a client that only ever adds keeps dispatching to a
        // pointer the compositor has taken away.
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            let has_pointer = caps.contains(wl_seat::Capability::Pointer);
            match (has_pointer, state.pointer.take()) {
                (true, Some(pointer)) => state.pointer = Some(pointer),
                (true, None) => {
                    tracing::debug!("seat gained a pointer");
                    state.pointer = Some(seat.get_pointer(qh, ()));
                }
                (false, Some(pointer)) => {
                    tracing::debug!("seat lost its pointer");
                    if pointer.version() >= 3 {
                        pointer.release();
                    }
                    state.on_pointer_gone();
                }
                (false, None) => {}
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for AppState {
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
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.on_pointer_motion(surface_x, surface_y),
            wl_pointer::Event::Leave { .. } => state.on_pointer_leave(),
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                let pressed = matches!(
                    button_state,
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed)
                );
                state.on_pointer_button(button, pressed);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for AppState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
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
                state.configured = Some((width, height));
                state.dirty = true;
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(AppState: ignore wl_compositor::WlCompositor);
delegate_noop!(AppState: ignore wl_surface::WlSurface);
delegate_noop!(AppState: ignore wl_shm::WlShm);
delegate_noop!(AppState: ignore wl_shm_pool::WlShmPool);
impl Dispatch<wl_buffer::WlBuffer, BufferSlot> for AppState {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        slot: &BufferSlot,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            // Queued rather than applied here: the pool lives on
            // `LayerWindow`, which drains this at the top of every repaint.
            state.released.push(*slot);
        }
    }
}
delegate_noop!(AppState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);

#[cfg(test)]
mod tests {
    use super::{
        AppState, BTN_LEFT, CONFIGURE_TIMEOUT, LayerWindowError, select_paint_slot, wait_bounded,
    };
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::css::cascade::CompiledSheet;
    use crate::css::node::{Node, PseudoStates};
    use crate::shm::{BufferSlot, Slot, SlotPool};
    use crate::text::FontDatabase;
    use crate::widget::button::Button;
    use std::error::Error;
    use std::time::{Duration, Instant};
    use wayland_client::{Connection, EventQueue};

    fn state() -> AppState {
        let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
        let mut fonts = FontDatabase::probe_only();
        let window = Node::with_classes("window", &["background"]);
        let mut button = Button::new("Click me", &[], window);
        button.restyle(&sheet, &mut fonts);
        AppState::new(sheet, fonts, button)
    }

    /// A connection to a socket nobody ever writes to: a compositor that
    /// accepts the client and then says nothing at all.
    fn silent_connection() -> (
        Connection,
        EventQueue<AppState>,
        std::os::unix::net::UnixStream,
    ) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let conn = Connection::from_socket(ours).expect("connection");
        let queue = conn.new_event_queue();
        // `theirs` is returned so the peer stays open: dropping it would
        // make the fd readable (EOF) and turn the test into a race.
        (conn, queue, theirs)
    }

    /// A point comfortably inside the 78x34 button.
    const INSIDE: (f64, f64) = (30.0, 17.0);
    /// A point comfortably outside it.
    const OUTSIDE: (f64, f64) = (300.0, 300.0);

    #[test]
    fn every_error_says_what_actually_failed() {
        // A7: every failure was `Io`, whose message was "shm buffer error",
        // so a dead socket, a missing font and a failed surface allocation
        // all blamed the shm buffer.
        let cases: Vec<(LayerWindowError, &str)> = vec![
            (
                LayerWindowError::Shm(std::io::Error::other("ENOSPC")),
                "shm buffer",
            ),
            (
                LayerWindowError::Socket(std::io::Error::other("EPIPE")),
                "Wayland socket",
            ),
            (LayerWindowError::Render("the raster surface"), "raster"),
            (LayerWindowError::NoFont, "typeface"),
            (LayerWindowError::Closed, "closed"),
            (
                LayerWindowError::Timeout(Duration::from_secs(5)),
                "did not configure",
            ),
            (
                LayerWindowError::MissingGlobal("zwlr_layer_shell_v1"),
                "zwlr_layer_shell_v1",
            ),
        ];
        for (error, expected) in &cases {
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "{error:?} reads {message:?}, which does not mention {expected:?}"
            );
        }
        assert!(
            LayerWindowError::Shm(std::io::Error::other("ENOSPC"))
                .source()
                .is_some(),
            "the underlying io::Error is not reachable"
        );
        assert!(LayerWindowError::NoFont.source().is_none());
    }

    #[test]
    fn only_btn_left_presses_the_button() {
        // A6: *any* button code drove `:active`, so a right click or a
        // scroll-wheel click pressed the widget.
        let mut state = state();
        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        assert!(state.button.states().contains(PseudoStates::HOVER));

        for button in [0x111u32, 0x112, 0x113] {
            state.on_pointer_button(button, true);
            assert!(
                !state.button.states().contains(PseudoStates::ACTIVE),
                "button {button:#x} armed :active"
            );
        }

        state.on_pointer_button(BTN_LEFT, true);
        assert!(
            state.button.states().contains(PseudoStates::ACTIVE),
            "BTN_LEFT did not press it"
        );
    }

    #[test]
    fn re_entering_while_held_re_arms_active() {
        // A6: leaving cleared the only state the client tracked, so coming
        // back with the mouse button still down never re-armed `:active`.
        let mut state = state();
        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        state.on_pointer_button(BTN_LEFT, true);
        assert!(state.button.states().contains(PseudoStates::ACTIVE));

        // Drag off: the visual state drops, the press does not.
        state.on_pointer_motion(OUTSIDE.0, OUTSIDE.1);
        assert!(!state.button.states().contains(PseudoStates::HOVER));
        assert!(!state.button.states().contains(PseudoStates::ACTIVE));
        assert!(state.held, "dragging off must not end the press");

        // Drag back on: `:active` comes back.
        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        assert!(state.button.states().contains(PseudoStates::HOVER));
        assert!(
            state.button.states().contains(PseudoStates::ACTIVE),
            "re-entering while held did not re-arm :active"
        );
    }

    #[test]
    fn leaving_the_surface_keeps_the_press_but_drops_the_paint() {
        let mut state = state();
        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        state.on_pointer_button(BTN_LEFT, true);

        state.on_pointer_leave();
        assert!(!state.button.states().contains(PseudoStates::HOVER));
        assert!(!state.button.states().contains(PseudoStates::ACTIVE));
        assert!(state.held);
    }

    #[test]
    fn a_release_anywhere_ends_the_press() {
        let mut state = state();
        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        state.on_pointer_button(BTN_LEFT, true);
        state.on_pointer_motion(OUTSIDE.0, OUTSIDE.1);

        state.on_pointer_button(BTN_LEFT, false);
        assert!(!state.held, "a release off the widget must still end it");

        state.on_pointer_motion(INSIDE.0, INSIDE.1);
        assert!(state.button.states().contains(PseudoStates::HOVER));
        assert!(
            !state.button.states().contains(PseudoStates::ACTIVE),
            "re-entering after the release re-armed a press that had ended"
        );
    }

    #[test]
    fn a_press_that_starts_outside_never_arms_the_button() {
        let mut state = state();
        state.on_pointer_motion(OUTSIDE.0, OUTSIDE.1);
        state.on_pointer_button(BTN_LEFT, true);
        assert!(!state.button.states().contains(PseudoStates::ACTIVE));
        assert!(state.held);
    }

    #[test]
    fn losing_the_pointer_capability_clears_hover_and_active() {
        // A5: capabilities were additive-only, so a seat that lost its
        // pointer left the widget stuck in whatever state it was showing.
        let mut state = state();
        state.update_states(true, true);
        assert!(
            state.button.states().contains(PseudoStates::HOVER)
                && state.button.states().contains(PseudoStates::ACTIVE)
        );
        state.dirty = false;

        state.held = true;
        state.on_pointer_gone();
        assert!(!state.held, "the press cannot outlive the pointer");
        assert!(!state.button.states().contains(PseudoStates::HOVER));
        assert!(!state.button.states().contains(PseudoStates::ACTIVE));
        assert!(state.dirty, "clearing hover must schedule a repaint");
    }

    #[test]
    fn select_paint_slot_defers_while_every_buffer_is_busy_and_proceeds_once_one_is_released() {
        // The release->repaint wiring: `repaint` must leave `dirty` set and
        // write nothing while every buffer is still held, then proceed as
        // soon as a queued release is drained in. Exercised here as the
        // pure fragment `repaint` calls, since `repaint` itself needs a
        // live Wayland connection.
        let mut slots = SlotPool::new(2, 2);
        assert_eq!(slots.acquire(), Some(Slot::Existing(0)));
        assert_eq!(slots.acquire(), Some(Slot::Existing(1)));

        let mut released = Vec::new();
        assert_eq!(
            select_paint_slot(&mut released, &mut slots),
            None,
            "every buffer is busy and the pool is already at its maximum: \
             repaint must defer, not reuse or grow"
        );

        // The compositor releases buffer 1; `repaint`'s caller queues that
        // up in `released` for the next repaint to drain.
        released.push(BufferSlot(1));
        assert_eq!(
            select_paint_slot(&mut released, &mut slots),
            Some(Slot::Existing(1)),
            "a released slot must free up the very next repaint"
        );
        assert!(
            released.is_empty(),
            "the drained release must not be handed out again"
        );
    }

    #[test]
    fn the_configure_wait_is_bounded() {
        // A4: the wait was `while configured.is_none() { blocking_dispatch }`,
        // which never returns against a compositor that never configures.
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = state();
        let timeout = Duration::from_millis(150);

        let started = Instant::now();
        let result = wait_bounded(&conn, &mut queue, &mut state, timeout, |state| {
            state.configured.is_some()
        });
        let elapsed = started.elapsed();

        assert!(
            matches!(result, Err(LayerWindowError::Timeout(after)) if after == timeout),
            "expected a Timeout, got {result:?}"
        );
        assert!(
            elapsed >= timeout && elapsed < timeout * 20,
            "the wait took {elapsed:?}, not about {timeout:?}"
        );
    }

    #[test]
    fn a_closed_surface_ends_the_configure_wait_immediately() {
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = state();
        state.closed = true;

        let started = Instant::now();
        let result = wait_bounded(&conn, &mut queue, &mut state, CONFIGURE_TIMEOUT, |state| {
            state.configured.is_some()
        });

        assert!(
            matches!(result, Err(LayerWindowError::Closed)),
            "expected Closed, got {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "`closed` did not short-circuit the wait"
        );
    }

    #[test]
    fn an_already_satisfied_wait_returns_without_polling() {
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = state();
        state.configured = Some((78, 34));

        let started = Instant::now();
        wait_bounded(&conn, &mut queue, &mut state, CONFIGURE_TIMEOUT, |state| {
            state.configured.is_some()
        })
        .expect("already configured");
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
