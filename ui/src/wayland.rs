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
    ConnectError, Connection, Dispatch, DispatchError, EventQueue, QueueHandle, delegate_noop,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;

use crate::css::cascade::CompiledSheet;
use crate::css::select::PseudoStates;
use crate::shm::{BufferPool, BufferSlot};
use crate::text::FontStack;
use crate::widget::button::Button;

/// Everything that can go wrong opening or running the window.
#[derive(Debug)]
pub enum LayerWindowError {
    /// `wl_display` connection failed.
    Connect(ConnectError),
    /// The compositor never advertised a global the client requires.
    MissingGlobal(&'static str),
    /// shm buffer allocation, upload, or a socket flush failed.
    Io(std::io::Error),
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
            Self::Io(e) => write!(f, "shm buffer error: {e}"),
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

impl std::error::Error for LayerWindowError {}

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
    /// Last pointer position in surface coordinates.
    pointer_at: Option<(f64, f64)>,
    /// Buffer slots the compositor released since the last repaint.
    released: Vec<BufferSlot>,
    sheet: CompiledSheet,
    fonts: FontStack,
    button: Button,
}

impl AppState {
    /// A client state with nothing bound yet.
    fn new(sheet: CompiledSheet, fonts: FontStack, button: Button) -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            configured: None,
            dirty: true,
            closed: false,
            pointer_at: None,
            released: Vec::new(),
            sheet,
            fonts,
            button,
        }
    }

    /// Recompute state from the pointer, restyling only on an actual change.
    fn update_states(&mut self, hover: bool, active: bool) {
        let current = self.button.states();
        if current.hover == hover && current.active == active {
            return;
        }
        tracing::debug!(hover, active, "pointer state changed");
        self.button.set_states(
            PseudoStates {
                hover,
                active,
                ..PseudoStates::default()
            },
            &self.sheet,
            &self.fonts,
        );
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
    _layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    shm: wl_shm::WlShm,
    buffers: BufferPool,
    skia: Surface,
}

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

        conn.flush().map_err(flush_error)?;
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
                Err(err) => return Err(flush_error(err)),
            },
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(LayerWindowError::Io(err.into())),
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
        fonts: FontStack,
        mut button: Button,
    ) -> Result<Self, LayerWindowError> {
        button.restyle(&sheet, &fonts);
        let allocation = button.allocation();
        let width = allocation.width.ceil().max(1.0) as i32;
        let height = allocation.height.ceil().max(1.0) as i32;
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
        conn.flush().map_err(flush_error)?;

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

        let buffers = BufferPool::new(&shm, &qh, width, height).map_err(LayerWindowError::Io)?;
        let skia = Surface::new_raster_n32_premul(width, height)
            .ok_or_else(|| LayerWindowError::Io(std::io::Error::other("raster surface")))?;

        let mut window = Self {
            conn,
            queue,
            qh,
            state,
            surface,
            _layer_surface: layer_surface,
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
        for slot in std::mem::take(&mut self.state.released) {
            self.buffers.release(slot);
        }
        let Some(index) = self
            .buffers
            .acquire(&self.shm, &self.qh)
            .map_err(LayerWindowError::Io)?
        else {
            tracing::debug!(
                buffers = self.buffers.slots().len(),
                "every shm buffer is still held by the compositor; deferring the frame"
            );
            self.state.dirty = true;
            return Ok(());
        };

        let (width, height) = self.buffers.size();
        self.skia.canvas().clear(Color::TRANSPARENT);
        self.state
            .button
            .render(&mut self.skia, (0.0, 0.0), &self.state.fonts);
        self.buffers
            .upload(index, &self.skia)
            .map_err(LayerWindowError::Io)?;
        self.surface
            .attach(Some(self.buffers.wl_buffer(index)), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
        self.conn.flush().map_err(flush_error)?;
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

    /// The queue handle, for callers that need to create further objects.
    #[must_use]
    pub fn queue_handle(&self) -> &QueueHandle<AppState> {
        &self.qh
    }
}

/// `Connection::flush` reports a `WaylandError`, which is `io::Error` plus a
/// protocol variant; fold both into [`LayerWindowError::Io`] rather than
/// widening the error enum for a case that only ever means "the socket died".
fn flush_error(err: wayland_client::backend::WaylandError) -> LayerWindowError {
    match err {
        wayland_client::backend::WaylandError::Io(io) => LayerWindowError::Io(io),
        other => LayerWindowError::Io(std::io::Error::other(other.to_string())),
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
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
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
            } => {
                state.pointer_at = Some((surface_x, surface_y));
                let inside = state.button.contains((0.0, 0.0), surface_x, surface_y);
                let active = state.button.states().active && inside;
                state.update_states(inside, active);
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_at = None;
                state.update_states(false, false);
            }
            wl_pointer::Event::Button {
                state: button_state,
                ..
            } => {
                let pressed = matches!(
                    button_state,
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed)
                );
                let inside = state
                    .pointer_at
                    .is_some_and(|(x, y)| state.button.contains((0.0, 0.0), x, y));
                state.update_states(inside, pressed && inside);
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
    use super::{AppState, CONFIGURE_TIMEOUT, LayerWindowError, wait_bounded};
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::css::cascade::CompiledSheet;
    use crate::css::select::{CssNode, PseudoStates};
    use crate::text::FontStack;
    use crate::widget::button::Button;
    use std::time::{Duration, Instant};
    use wayland_client::{Connection, EventQueue};

    fn state() -> AppState {
        let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
        let fonts = FontStack::system().expect("system font");
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        let mut button = Button::new("Click me", &[], window);
        button.restyle(&sheet, &fonts);
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
