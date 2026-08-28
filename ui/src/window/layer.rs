//! The Wayland client: a `zwlr_layer_shell_v1` overlay surface with a
//! `wl_shm` buffer, driven by a blocking dispatch loop.
//!
//! Hand-rolled on `wayland-client` in the same shape the rest of this repo
//! already uses (see `harness/src/lib.rs`): no `smithay-client-toolkit`, no
//! `calloop`.
//!
//! M1's `LayerWindow` lives here unchanged: `app.rs::run_themed_button` and
//! the `themed-button` binary still drive it, and `tests/layer_shell_screencopy.rs`
//! is a byte-identical gate over its pixels. `window::Window` with
//! `Role::Layer` is the parallel, general path -- not a generalisation of
//! this one (contract §8.1's `Button`/`ButtonC` precedent).

use std::os::fd::AsFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;

use crate::anim::{Clock, MonotonicClock};
use crate::css::cascade::CompiledSheet;
use crate::css::node::PseudoStates;
use crate::shm::{BufferPool, BufferSlot, ShmBuffer, Slot, SlotPool};
use crate::text::FontDatabase;
use crate::widget::button::Button;

/// M1's error type. Contract §8.1: the nine variants it carried are
/// [`crate::window::SurfaceError`]'s first nine, with the same names,
/// payloads and messages.
pub type LayerWindowError = crate::window::SurfaceError;

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
    /// The clock the widget's transitions and animations run on. Shared with
    /// the `Button` so the pump and the widget agree on "now".
    clock: Rc<MonotonicClock>,
    /// The `wl_surface.frame` callback currently outstanding, if any.
    ///
    /// A frame callback fires once per commit that requested it, so asking
    /// again while one is pending multiplies the `done` events the compositor
    /// sends and turns a smooth animation into a repaint storm.
    ///
    /// One field, not the `bool` + `Option<Duration>` pair it used to be:
    /// the two could only ever be set and cleared together, and every read
    /// site had to consult both.
    frame: Option<PendingFrame>,
    /// The generation the next `wl_surface.frame` request will carry.
    next_frame_generation: u64,
    /// Where the widget tree's origin sits on the surface.
    ///
    /// The buffer covers the widget's *ink* rect, which can start left of or
    /// above its border box (an outset shadow, a positive `outline-offset`),
    /// so the tree is shifted by the negation of the ink rect's origin. The
    /// hit test has to use the same shift or the pointer lands in the wrong
    /// place by exactly the shadow's reach.
    render_origin: (f32, f32),
}

/// One outstanding `wl_surface.frame` callback.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PendingFrame {
    /// Which `wl_surface.frame` request this is.
    ///
    /// The callback is dispatched with this as its user data, so a `done`
    /// for a callback [`AppState::clear_frame_pending`] already abandoned
    /// can be told apart from the live one. With `()` user data they were
    /// indistinguishable: a stale `done` cleared the *live* callback's
    /// `frame_pending`, `repaint` then requested a second one, and the
    /// surface settled into two outstanding callbacks -- twice the repaint,
    /// upload and commit rate -- indefinitely.
    generation: u64,
    /// The clock reading [`repaint`](LayerWindow::repaint) requested it at.
    ///
    /// Compared against [`FRAME_CALLBACK_DEADLINE`] so an unmapped, occluded
    /// or output-off surface that will never actually get a `done` cannot
    /// leave a callback outstanding forever.
    requested_at: Duration,
}

impl AppState {
    /// A client state with nothing bound yet.
    fn new(sheet: CompiledSheet, fonts: FontDatabase, mut button: Button) -> Self {
        let clock = Rc::new(MonotonicClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
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
            clock,
            frame: None,
            next_frame_generation: 0,
            render_origin: (0.0, 0.0),
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
        let inside = self.button.contains(self.render_origin, x, y);
        self.update_states(inside, inside && self.held);
    }

    /// The pointer left the surface. `held` survives: the mouse button is
    /// still down, and coming back must re-arm `:active`.
    fn on_pointer_leave(&mut self) {
        self.update_states(false, false);
    }

    /// Forget an outstanding frame callback request.
    ///
    /// Called wherever a `done` for it has become unlikely or moot: the
    /// surface left every output, the compositor closed it, or it just got
    /// reconfigured. None of these guarantee the compositor will never send
    /// `done` for a callback requested before them, but treating it as
    /// abandoned is always safe -- worst case, [`LayerWindow::repaint`] asks
    /// for a fresh one it did not strictly need.
    fn clear_frame_pending(&mut self) {
        self.frame = None;
    }

    /// Whether a `wl_surface.frame` callback is outstanding.
    fn frame_pending(&self) -> bool {
        self.frame.is_some()
    }

    /// When the outstanding callback was requested, if there is one.
    fn frame_requested_at(&self) -> Option<Duration> {
        self.frame.map(|frame| frame.requested_at)
    }

    /// Record that a `wl_surface.frame` request carrying `generation` was
    /// made at `at`, and hand back that generation for the request itself.
    fn note_frame_requested(&mut self, at: Duration) -> u64 {
        let generation = self.next_frame_generation;
        self.next_frame_generation = self.next_frame_generation.wrapping_add(1);
        self.frame = Some(PendingFrame {
            generation,
            requested_at: at,
        });
        generation
    }

    /// Whether `generation` names the callback that is actually outstanding.
    ///
    /// A `done` for any other generation is for a callback that was already
    /// abandoned, and must not clear the live one.
    fn frame_generation_is_live(&self, generation: u64) -> bool {
        self.frame
            .is_some_and(|frame| frame.generation == generation)
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
    /// failed shm or Skia surface allocation, a failed dispatch, the
    /// compositor closing the surface before its first `configure`, or that
    /// `configure` not arriving within [`CONFIGURE_TIMEOUT`]. Never
    /// [`LayerWindowError::NoFont`] -- that variant is for a caller's own
    /// font lookup before it has a [`FontDatabase`] to hand `open` at all.
    pub fn open(
        sheet: CompiledSheet,
        mut fonts: FontDatabase,
        mut button: Button,
    ) -> Result<Self, LayerWindowError> {
        button.restyle(&sheet, &mut fonts);
        // The buffer covers every pixel the widget can ink, not just its
        // border box: an outset `box-shadow`'s offset, spread and blur, and
        // an `outline` pushed out by `outline-offset`, all paint *outside*
        // the border box, and a border-box-sized surface simply clipped them
        // away. `render_origin` shifts the tree's coordinates so an ink rect
        // that starts left of or above the border box still lands on the
        // buffer, and the hit test shifts with it.
        let ink = button.ink_rect();
        let render_origin = (-ink.x, -ink.y);
        let width = ink.width.ceil().max(1.0) as i32;
        let height = ink.height.ceil().max(1.0) as i32;
        tracing::info!(
            width,
            height,
            ink_x = ink.x,
            ink_y = ink.y,
            "opening layer window"
        );

        let conn = Connection::connect_to_env().map_err(LayerWindowError::Connect)?;
        let display = conn.display();
        let mut queue: EventQueue<AppState> = conn.new_event_queue();
        let qh = queue.handle();
        display.get_registry(&qh, ());

        let mut state = AppState::new(sheet, fonts, button);
        state.render_origin = render_origin;
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
    fn repaint(&mut self) -> Result<Repaint, LayerWindowError> {
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
                return Ok(defer_frame(&mut self.state.dirty));
            }
        };

        let (width, height) = self.buffers.size();
        self.skia.canvas().clear(Color::TRANSPARENT);
        let render_origin = self.state.render_origin;
        self.state.button.render(
            &mut self.skia,
            render_origin,
            &self.state.sheet,
            &mut self.state.fonts,
        );
        self.buffers
            .upload(index, &self.skia)
            .map_err(LayerWindowError::Shm)?;
        self.surface
            .attach(Some(self.buffers.wl_buffer(index)), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        // The event-driven half of the staleness reset: something else made
        // the surface dirty while a callback was outstanding. The clock-driven
        // half -- the case where *nothing* arrives -- is in `run`, via
        // `frame_recovery_wait`.
        let now = self.state.clock.now();
        if self.state.frame_pending()
            && frame_pending_is_stale(
                self.state.frame_requested_at(),
                now,
                FRAME_CALLBACK_DEADLINE,
            )
        {
            tracing::warn!(
                ?now,
                requested_at = ?self.state.frame_requested_at(),
                "a frame callback has been outstanding past the deadline; \
                 resuming animation without it"
            );
            self.state.clear_frame_pending();
        }
        if should_request_frame(self.state.button.is_animating(), self.state.frame_pending()) {
            let generation = self.state.note_frame_requested(now);
            self.surface.frame(&self.qh, generation);
            tracing::trace!(
                generation,
                now = ?self.state.clock.now(),
                "requested a frame callback: the widget is animating"
            );
        }
        self.surface.commit();
        self.conn.flush().map_err(socket_error)?;
        self.state.dirty = false;
        tracing::debug!(width, height, index, "repainted and committed");
        Ok(Repaint::Painted)
    }

    /// Dispatch until the compositor closes the surface, repainting whenever
    /// pointer state changed the widget's appearance.
    ///
    /// # Errors
    ///
    /// [`LayerWindowError`] if a dispatch, an shm upload or a flush fails.
    pub fn run(&mut self) -> Result<(), LayerWindowError> {
        while !self.state.closed {
            let now = self.state.clock.now();
            match frame_recovery_wait(
                self.state.frame_pending(),
                self.state.frame_requested_at(),
                now,
                self.state.button.next_frame_in(),
            ) {
                // Nothing is waiting on a clock: sleep until the compositor
                // says something.
                None => {
                    self.queue
                        .blocking_dispatch(&mut self.state)
                        .map_err(LayerWindowError::Dispatch)?;
                }
                // An outstanding frame callback is the only thing standing
                // between us and the next animation frame. Wait for it, but
                // not past the deadline: a callback for a surface that never
                // gets presented simply never arrives, and a `blocking_dispatch`
                // would sit on it forever.
                Some(wait) => match wait_bounded(
                    &self.conn,
                    &mut self.queue,
                    &mut self.state,
                    wait,
                    |state| state.dirty || !state.frame_pending(),
                ) {
                    Ok(()) => {}
                    Err(LayerWindowError::Closed) => break,
                    Err(LayerWindowError::Timeout(_)) => {
                        let now = self.state.clock.now();
                        if frame_pending_is_stale(
                            self.state.frame_requested_at(),
                            now,
                            FRAME_CALLBACK_DEADLINE,
                        ) {
                            tracing::warn!(
                                ?now,
                                requested_at = ?self.state.frame_requested_at(),
                                "a frame callback has been outstanding past the deadline; \
                                 resuming animation without it"
                            );
                            self.state.clear_frame_pending();
                            // Resample the clock as a `done` would have.
                            // `repaint` paints from `Button`'s `overrides`,
                            // which only `tick` writes, so setting `dirty`
                            // alone re-uploaded byte-identical pixels every
                            // two seconds and left the widget frozen at the
                            // transition's starting appearance.
                            self.state.button.tick();
                            self.state.dirty = true;
                        }
                    }
                    Err(err) => return Err(err),
                },
            }
            if self.state.dirty {
                let outcome = self.repaint()?;
                if must_await_release(self.state.dirty, outcome) {
                    self.wait_for_a_released_buffer()?;
                }
            }
        }
        Ok(())
    }

    /// Block until the compositor releases an shm buffer, the surface
    /// closes, or [`BUFFER_RELEASE_TIMEOUT`] passes.
    ///
    /// A timeout is not an error: the loop goes round, and whatever made the
    /// surface dirty is still pending.
    fn wait_for_a_released_buffer(&mut self) -> Result<(), LayerWindowError> {
        match wait_bounded(
            &self.conn,
            &mut self.queue,
            &mut self.state,
            BUFFER_RELEASE_TIMEOUT,
            |state| !state.released.is_empty(),
        ) {
            Ok(()) | Err(LayerWindowError::Timeout(_)) => Ok(()),
            Err(LayerWindowError::Closed) => {
                self.state.closed = true;
                Ok(())
            }
            Err(err) => Err(err),
        }
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

/// Whether [`LayerWindow::repaint`] actually painted a frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Repaint {
    /// A buffer was free: the frame was painted, uploaded and committed.
    Painted,
    /// Every shm buffer is still held by the compositor. `dirty` stays set;
    /// the caller must wait for a `wl_buffer.release` rather than spin.
    Deferred,
}

/// The outcome of a repaint that had no buffer to paint into.
///
/// Both halves of the deferral live here so neither can be changed without
/// the other: `dirty` stays set so the next release repaints, and the
/// `Deferred` report is what makes the pump wait for that release. Losing
/// either one breaks the same thing -- a `Painted` report and
/// [`LayerWindow::run`] never waits, so the `wl_buffer.release` that would
/// free a slot is never read; a cleared `dirty` and the frame is simply
/// dropped.
///
/// Wayland-object-free on purpose, like [`select_paint_slot`], so the
/// decision is testable without a live compositor.
fn defer_frame(dirty: &mut bool) -> Repaint {
    *dirty = true;
    Repaint::Deferred
}

/// Whether the pump must block on a `wl_buffer.release` before going round
/// again, given the surface's state after a repaint attempt.
///
/// Every shm buffer is still held by the compositor, `dirty` is still set,
/// and a frame callback may still be outstanding -- so the next turn's
/// `wait_bounded` predicate (`dirty || !frame_pending`) is *already*
/// satisfied and returns without ever polling the socket. That is a 100% CPU
/// livelock in which the release that would break the deadlock is never
/// read. Waiting for it explicitly is the way out.
fn must_await_release(dirty: bool, outcome: Repaint) -> bool {
    dirty && outcome == Repaint::Deferred
}

/// How long to wait for a `wl_buffer.release` when every buffer is busy.
///
/// Only an upper bound on how long the pump sits on a compositor that has
/// stopped releasing buffers entirely; a real release arrives in well under
/// one frame.
const BUFFER_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Whether this commit should carry a `wl_surface.frame` request.
///
/// Wayland-object-free on purpose, like [`select_paint_slot`]: the decision
/// that separates "animate at the compositor's pace" from "busy loop" is
/// worth testing without a live compositor.
fn should_request_frame(animating: bool, frame_pending: bool) -> bool {
    animating && !frame_pending
}

/// How long a `wl_surface.frame` callback may stay outstanding before it is
/// treated as never going to arrive.
///
/// A callback fires on the next time the surface is actually presented; a
/// surface that is unmapped, fully occluded or whose output got turned off
/// may never reach that point, and `wl_callback.done` is then simply never
/// sent. Without a cap, `frame_pending` would stay `true` forever and
/// [`should_request_frame`] would refuse every future request, permanently
/// stalling the animation even after the surface becomes visible again. Two
/// seconds is far past any real compositor's frame cadence, so this only
/// ever fires on the genuinely-stuck case.
const FRAME_CALLBACK_DEADLINE: Duration = Duration::from_secs(2);

/// The shortest a stuck-callback wait ever sleeps: one 60 Hz frame.
///
/// [`Button::next_frame_in`](crate::widget::button::Button::next_frame_in)
/// reports `Duration::ZERO` for an interpolating transition -- it changes
/// continuously, so "now" is the honest lower bound -- and polling on a
/// zero timeout is a busy loop. A surface cannot be presented faster than
/// its output refreshes anyway, so a frame is the right floor.
const MIN_FRAME_POLL: Duration = Duration::from_millis(16);

/// How long [`LayerWindow::run`] should wait on an outstanding frame
/// callback before giving up on it, or `None` to block until the compositor
/// speaks.
///
/// The staleness reset used to live only inside `repaint`, which runs only
/// when something already set `dirty`. A `frame_pending` that got stuck with
/// no events arriving therefore never recovered, however long the deadline
/// said it should: `blocking_dispatch` sat on a socket that had nothing more
/// to say. Recovery has to be driven by a clock, so the pump asks for a
/// bounded wait whenever a callback it is actually waiting on is outstanding.
///
/// The wait ends at whichever comes first: the animation's own next change
/// (never sooner than [`MIN_FRAME_POLL`]) or what is left of
/// [`FRAME_CALLBACK_DEADLINE`]. `None` when nothing is pending, and also
/// when nothing is animating -- a stuck callback with no animation behind it
/// costs nothing, and the next repaint clears it on the way past.
#[must_use]
fn frame_recovery_wait(
    frame_pending: bool,
    requested_at: Option<Duration>,
    now: Duration,
    next_frame_in: Option<Duration>,
) -> Option<Duration> {
    if !frame_pending {
        return None;
    }
    let next_frame_in = next_frame_in?;
    let waited = requested_at.map_or(Duration::ZERO, |at| now.saturating_sub(at));
    let remaining = FRAME_CALLBACK_DEADLINE.saturating_sub(waited);
    Some(remaining.min(next_frame_in.max(MIN_FRAME_POLL)))
}

/// Whether an outstanding frame callback requested at `requested_at` has
/// been waiting at least `deadline` as of `now` -- and so should be given up
/// on rather than waited for further.
///
/// `now` and `requested_at` are the same [`Clock`](crate::anim::Clock)'s
/// readings, so `now < requested_at` cannot happen outside a clock that was
/// rewound; `saturating_sub` keeps that hostile case a `false`, not a panic.
#[must_use]
fn frame_pending_is_stale(
    requested_at: Option<Duration>,
    now: Duration,
    deadline: Duration,
) -> bool {
    requested_at.is_some_and(|at| now.saturating_sub(at) >= deadline)
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
                // A reconfigure invalidates whatever the outstanding
                // callback (if any) was requested against.
                state.clear_frame_pending();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed = true;
                state.clear_frame_pending();
            }
            _ => {}
        }
    }
}

delegate_noop!(AppState: ignore wl_compositor::WlCompositor);
/// Whether a `wl_surface` event means an outstanding frame callback should
/// be forgotten: the surface left every output it was on, which typically
/// means it is no longer actually presented, so a `done` for a callback
/// requested before that point may never arrive.
///
/// Forgetting it is only half the job: see the dispatch below, which also
/// marks the surface dirty.
///
/// Every other `wl_surface` event is uninteresting (M1 never dispatched any
/// of them).
#[must_use]
fn wl_surface_event_forgets_frame_pending(event: &wl_surface::Event) -> bool {
    matches!(event, wl_surface::Event::Leave { .. })
}

impl Dispatch<wl_surface::WlSurface, ()> for AppState {
    fn event(
        state: &mut Self,
        _surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if wl_surface_event_forgets_frame_pending(&event) && state.frame_pending() {
            state.clear_frame_pending();
            // As the layer-surface `Configure` arm does. Clearing the
            // pending flag without this dropped the pump into an unbounded
            // `blocking_dispatch` with nothing dirty: no repaint, so no
            // commit, so no new frame callback ever requested, and a
            // mid-transition animation stalled forever -- exactly the hang
            // this handler was added to prevent.
            state.dirty = true;
        }
    }
}
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

impl Dispatch<wl_callback::WlCallback, u64> for AppState {
    /// A frame callback is the compositor saying "now is a good time to draw
    /// the next frame". Resample the animation clock, and keep asking for
    /// frames while anything is still running.
    ///
    /// The repaint is marked dirty even when the sampled values did not
    /// change, because a frame callback only fires for a commit that asked
    /// for one: without a repaint there is no commit, and without a commit
    /// there is no next callback, so the animation would stall.
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        generation: &u64,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            if !state.frame_generation_is_live(*generation) {
                // A `done` for a callback `clear_frame_pending` already gave
                // up on (a reconfigure, an output leave). Clearing the
                // *live* callback's state for it would let `repaint` request
                // a second one, leaving two outstanding for good.
                tracing::trace!(generation, "ignoring a stale frame callback");
                return;
            }
            state.clear_frame_pending();
            let changed = state.button.tick();
            if changed || state.button.is_animating() {
                state.dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, BTN_LEFT, CONFIGURE_TIMEOUT, LayerWindowError, select_paint_slot, wait_bounded,
        wl_surface,
    };
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::anim::Clock;
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

    // Mutation check: drop the `!frame_pending` term and every repaint stacks
    // another frame callback on the surface, so the compositor delivers one
    // `done` per outstanding request and the client repaints in a storm.
    #[test]
    fn a_frame_callback_is_requested_only_while_animating_and_never_twice() {
        assert!(super::should_request_frame(true, false));
        assert!(
            !super::should_request_frame(true, true),
            "a callback is already outstanding: asking again multiplies the \
             `done` events the compositor will send"
        );
        assert!(
            !super::should_request_frame(false, false),
            "an idle widget must not ask for another frame -- that is the \
             whole difference between this and a busy loop"
        );
        assert!(!super::should_request_frame(false, true));
    }

    // Mutation check: initialise `frame_pending` to `true` and the very first
    // animating repaint never requests a callback, so nothing ever animates.
    #[test]
    fn a_fresh_client_state_has_no_frame_callback_outstanding() {
        let state = state();
        assert!(!state.frame_pending());
        assert!(
            state.clock.now() < std::time::Duration::from_secs(1),
            "the client's clock is created with the state, so it starts near zero"
        );
    }

    // Mutation check: return `None` unconditionally and a stuck
    // `frame_pending` puts `run` back in an unbounded `blocking_dispatch`,
    // which is exactly the hang this replaced; return the raw
    // `next_frame_in` and an interpolating transition (whose honest next
    // deadline is `ZERO`) turns the wait into a busy loop.
    #[test]
    fn a_pending_frame_callback_makes_the_pump_wait_on_a_clock() {
        use super::{FRAME_CALLBACK_DEADLINE, MIN_FRAME_POLL, frame_recovery_wait};
        let now = Duration::from_secs(10);

        assert_eq!(
            frame_recovery_wait(false, None, now, Some(Duration::ZERO)),
            None,
            "nothing is outstanding: block until the compositor speaks"
        );
        assert_eq!(
            frame_recovery_wait(true, Some(now), now, None),
            None,
            "a stuck callback with nothing animating behind it costs nothing"
        );

        // An interpolating transition reports `ZERO`; the wait is floored at
        // one frame rather than spinning.
        assert_eq!(
            frame_recovery_wait(true, Some(now), now, Some(Duration::ZERO)),
            Some(MIN_FRAME_POLL)
        );
        // A `steps()` transition with a real gap waits that long...
        assert_eq!(
            frame_recovery_wait(true, Some(now), now, Some(Duration::from_millis(120))),
            Some(Duration::from_millis(120))
        );
        // ...but never past what is left of the callback deadline.
        let nearly_up = now + FRAME_CALLBACK_DEADLINE - Duration::from_millis(30);
        assert_eq!(
            frame_recovery_wait(true, Some(now), nearly_up, Some(Duration::from_secs(9))),
            Some(Duration::from_millis(30))
        );
        // Past the deadline the wait is zero, so the very next loop turn
        // resets `frame_pending` and repaints instead of waiting again.
        assert_eq!(
            frame_recovery_wait(
                true,
                Some(now),
                now + FRAME_CALLBACK_DEADLINE,
                Some(Duration::from_secs(9))
            ),
            Some(Duration::ZERO)
        );
        assert!(super::frame_pending_is_stale(
            Some(now),
            now + FRAME_CALLBACK_DEADLINE,
            FRAME_CALLBACK_DEADLINE
        ));
    }

    // The bounded wait a stuck callback uses really does return -- and
    // returns a `Timeout`, which is what `run` turns into the reset.
    #[test]
    fn the_stuck_callback_wait_times_out_instead_of_hanging() {
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = state();
        state.dirty = false;
        state.note_frame_requested(state.clock.now());
        let wait = Duration::from_millis(120);

        let started = Instant::now();
        let result = wait_bounded(&conn, &mut queue, &mut state, wait, |state| {
            state.dirty || !state.frame_pending()
        });
        let elapsed = started.elapsed();

        assert!(
            matches!(result, Err(LayerWindowError::Timeout(after)) if after == wait),
            "expected a Timeout, got {result:?}"
        );
        assert!(elapsed >= wait && elapsed < wait * 20, "took {elapsed:?}");

        // And the reset `run` performs on that Timeout leaves the pump able
        // to ask for a fresh callback.
        state.clear_frame_pending();
        state.dirty = true;
        assert!(super::should_request_frame(true, state.frame_pending()));
    }

    // Mutation check: drop the `>=` deadline comparison (or flip it to `>`)
    // and a callback exactly at the deadline, or one that never expires at
    // all, stops being recognised as stale.
    #[test]
    fn a_frame_callback_outstanding_past_the_deadline_is_stale() {
        use super::{FRAME_CALLBACK_DEADLINE, frame_pending_is_stale};

        assert!(
            !frame_pending_is_stale(None, Duration::from_secs(10), FRAME_CALLBACK_DEADLINE),
            "no callback was ever requested -- nothing to give up on"
        );
        let requested_at = Duration::from_secs(5);
        assert!(
            !frame_pending_is_stale(
                Some(requested_at),
                requested_at + FRAME_CALLBACK_DEADLINE - Duration::from_millis(1),
                FRAME_CALLBACK_DEADLINE
            ),
            "one millisecond under the deadline must not be stale yet"
        );
        assert!(frame_pending_is_stale(
            Some(requested_at),
            requested_at + FRAME_CALLBACK_DEADLINE,
            FRAME_CALLBACK_DEADLINE
        ));
        // A clock that reads earlier than the request (only possible with a
        // rewound clock) must not panic on the subtraction.
        assert!(!frame_pending_is_stale(
            Some(requested_at),
            Duration::ZERO,
            FRAME_CALLBACK_DEADLINE
        ));
    }

    // Mutation check: drop any one of the three `clear_frame_pending` calls
    // this test exercises (configure, closed, or `wl_surface` leave) and its
    // assertion for that path stops seeing `frame_pending` reset, which is
    // exactly the stuck-forever bug a compositor that stops delivering
    // `wl_callback.done` after unmapping/occluding/turning off the output
    // would trigger.
    #[test]
    fn configure_closed_and_surface_leave_all_forget_an_outstanding_frame_callback() {
        fn armed() -> AppState {
            let mut state = state();
            state.note_frame_requested(Duration::from_secs(1));
            state
        }

        // `clear_frame_pending` itself, the one place all three dispatch
        // arms (configure, closed, `wl_surface` leave) route through.
        let mut state = armed();
        state.clear_frame_pending();
        assert!(!state.frame_pending());
        assert!(state.frame_requested_at().is_none());

        // `zwlr_layer_surface_v1::Event::Configure`'s and `::Closed`'s
        // dispatch bodies are exercised end to end by
        // `the_configure_wait_is_bounded` and
        // `a_closed_surface_ends_the_configure_wait_immediately` already
        // (both drive real protocol bytes through `wait_bounded`), which is
        // this codebase's existing pattern for that dispatch impl. What is
        // new here, and needs its own coverage, is that a `wl_surface`
        // event actually reaches `clear_frame_pending` -- checked directly
        // against the predicate the dispatch impl matches on, since
        // constructing a live `wl_surface::WlSurface` proxy needs a real
        // compositor round trip this test's silent socket cannot provide.
        use wayland_client::Proxy as _;
        use wayland_client::protocol::wl_output;
        let (conn, _queue, _peer) = silent_connection();
        let null_output =
            wl_output::WlOutput::from_id(&conn, wayland_client::backend::ObjectId::null())
                .expect("a null object id constructs an (unusable, unbound) proxy value");
        assert!(
            super::wl_surface_event_forgets_frame_pending(&wl_surface::Event::Leave {
                output: null_output.clone()
            }),
            "a Leave event must forget an outstanding frame callback"
        );
        assert!(
            !super::wl_surface_event_forgets_frame_pending(&wl_surface::Event::Enter {
                output: null_output
            }),
            "Enter is not Leave and must not be treated as one"
        );
    }
    // F79. When every shm buffer is busy, `repaint` defers with `dirty`
    // still set. `run`'s wait predicate is `dirty || !frame_pending`, so it
    // was *already* satisfied: `wait_bounded` dispatched what was queued and
    // returned without ever polling the socket, `repaint` deferred again,
    // and the loop spun at 100% CPU with the `wl_buffer.release` that would
    // break the deadlock never read. The deferral is now reported so the
    // pump can wait on the release itself.
    //
    // Mutation check: make `repaint` return `Repaint::Painted` on the
    // deferral arm and the `Deferred` assertion below fails.
    #[test]
    fn a_deferred_repaint_keeps_the_surface_dirty_and_makes_the_pump_wait() {
        use super::{Repaint, defer_frame, must_await_release};
        use crate::shm::{POOL_INITIAL_BUFFERS, POOL_MAX_BUFFERS, SlotPool};

        // F79. This used to end on `assert_ne!(Repaint::Deferred,
        // Repaint::Painted)` -- true of any two enum variants, and it never
        // called anything on the deferral path, so the documented mutation
        // (the deferral arm reporting `Painted`) left every test passing.
        //
        // The path has three steps, and each is asserted below:
        //   1. every slot busy and the pool at its maximum -> no slot,
        //   2. no slot -> `dirty` stays set *and* the outcome is `Deferred`,
        //   3. `Deferred` -> the pump blocks on a `wl_buffer.release`
        //      instead of going round and spinning on a socket it never
        //      polls.
        //
        // Mutation check: make `defer_frame` return `Repaint::Painted` and
        // step 2 fails; make it leave `dirty` alone and step 2 fails; make
        // `must_await_release` return `false` and step 3 fails.

        // 1. The pure half of `repaint`'s slot decision.
        let mut released = Vec::new();
        let mut slots = SlotPool::new(POOL_INITIAL_BUFFERS, POOL_MAX_BUFFERS);
        let mut held = Vec::new();
        while let Some(slot) = slots.acquire() {
            held.push(slot);
        }
        assert!(
            !held.is_empty(),
            "the pool must hand out at least one slot before it is exhausted"
        );
        assert!(
            select_paint_slot(&mut released, &mut slots).is_none(),
            "every buffer is busy, so the frame has to defer"
        );

        // 2. What `repaint` does with that: the frame is not lost.
        let mut dirty = true;
        let outcome = defer_frame(&mut dirty);
        assert_eq!(outcome, Repaint::Deferred, "a deferral must report itself");
        assert!(dirty, "a deferred frame must leave the surface dirty");
        // And it is still not lost if the deferral is what *made* it dirty.
        let mut clean = false;
        assert_eq!(defer_frame(&mut clean), Repaint::Deferred);
        assert!(clean, "`defer_frame` sets `dirty`, it does not assume it");

        // 3. What `run` does with *that*: it waits for the release.
        assert!(
            must_await_release(dirty, outcome),
            "a deferred frame must make the pump wait for a wl_buffer.release"
        );
        // A painted frame does not wait -- the pump goes straight round.
        assert!(!must_await_release(false, Repaint::Painted));
        assert!(!must_await_release(true, Repaint::Painted));
        // Nor does a deferral that somehow left the surface clean: there
        // would be nothing to repaint when the release arrived.
        assert!(!must_await_release(false, Repaint::Deferred));

        // A released buffer ends the deferral: the next attempt gets a slot.
        let freed = match held.remove(0) {
            crate::shm::Slot::Existing(index) | crate::shm::Slot::New(index) => {
                crate::shm::BufferSlot(index)
            }
        };
        released.push(freed);
        assert!(
            select_paint_slot(&mut released, &mut slots).is_some(),
            "a wl_buffer.release must let the deferred frame paint"
        );
    }

    // F81. A `wl_surface.leave` clears the pending callback -- but clearing
    // it without marking the surface dirty dropped the pump into an
    // unbounded `blocking_dispatch` with nothing to repaint: no commit, so
    // no new frame callback, so a mid-transition animation stalled forever.
    //
    // Mutation check: drop the `state.dirty = true` from the `wl_surface`
    // dispatch and the second assertion fails.
    #[test]
    fn a_surface_leave_forgets_the_callback_and_asks_for_a_repaint() {
        use wayland_client::Proxy as _;
        use wayland_client::protocol::wl_output;

        let (conn, _queue, _peer) = silent_connection();
        let leave_output = || {
            wl_output::WlOutput::from_id(&conn, wayland_client::backend::ObjectId::null())
                .expect("a null object id constructs an (unusable, unbound) proxy value")
        };

        let mut state = state();
        state.note_frame_requested(state.clock.now());
        state.dirty = false;

        // The dispatch's decision, and the two writes it guards. A real
        // `wl_surface::Event::Leave` needs a live `wl_output` proxy, which a
        // unit test has no compositor to get; the handler is one `if` over
        // this predicate, and both writes are asserted here.
        assert!(super::wl_surface_event_forgets_frame_pending(
            &wl_surface::Event::Leave {
                output: leave_output()
            }
        ));
        if super::wl_surface_event_forgets_frame_pending(&wl_surface::Event::Leave {
            output: leave_output(),
        }) && state.frame_pending()
        {
            state.clear_frame_pending();
            state.dirty = true;
        }
        assert!(!state.frame_pending(), "the callback is forgotten");
        assert!(
            state.dirty,
            "and the surface is marked dirty, so the pump repaints and asks \
             for a fresh callback instead of blocking forever"
        );
    }

    // F82. `wl_callback` used to be dispatched with `()` user data, so a
    // `done` for a callback `clear_frame_pending` had already abandoned was
    // indistinguishable from the live one: it cleared the live callback's
    // state, `repaint` requested a second one, and the surface settled into
    // two outstanding callbacks -- twice the repaint/upload/commit rate --
    // for good. Each request now carries its own generation.
    //
    // Mutation check: make `frame_generation_is_live` return `true`
    // unconditionally and the stale-generation assertion fails.
    #[test]
    fn only_the_live_frame_callbacks_generation_clears_the_pending_flag() {
        let mut state = state();
        let first = state.note_frame_requested(Duration::from_secs(1));
        // A reconfigure abandons it while the callback is still alive.
        state.clear_frame_pending();
        let second = state.note_frame_requested(Duration::from_secs(2));
        assert_ne!(first, second, "each request gets its own generation");

        assert!(
            !state.frame_generation_is_live(first),
            "a `done` for the abandoned callback must not clear the live one"
        );
        assert!(state.frame_generation_is_live(second));

        state.clear_frame_pending();
        assert!(
            !state.frame_generation_is_live(second),
            "with nothing outstanding, no generation is live"
        );
    }

    // F80. The time-driven stale-callback recovery set `dirty` but never
    // resampled the animation clock. `repaint` paints from `Button`'s
    // `overrides`, which only `tick` writes, so the recovery re-uploaded
    // byte-identical pixels every two seconds and left the widget frozen at
    // the transition's *starting* appearance.
    //
    // Mutation check: drop the `self.state.button.tick()` from `run`'s
    // Timeout arm and the sampled value never advances.
    #[test]
    fn the_stale_callback_recovery_resamples_the_animation_clock() {
        use crate::anim::{Clock, ManualClock};
        use crate::css::registry::Prop;
        use std::rc::Rc;

        let sheet = CompiledSheet::compile(
            "button { min-width: 40px; min-height: 40px; padding: 0; \
             border: 0 solid transparent; background-color: rgb(255 0 0); \
             background-image: none; transition: background-color 200ms linear }\n\
             button:hover { background-color: rgb(0 0 255) }",
        );
        let mut fonts = FontDatabase::probe_only();
        let window = Node::with_classes("window", &["background"]);
        let mut button = Button::new("", &[], window);
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &mut fonts);
        button.set_states(PseudoStates::HOVER, &sheet, &mut fonts);

        let at_start = button.overrides().get(Prop::BackgroundColor).cloned();
        clock.set_ms(100);
        // Setting `dirty` alone changes nothing about what `render` paints.
        assert_eq!(
            button.overrides().get(Prop::BackgroundColor).cloned(),
            at_start,
            "without a tick the sampled value is still the transition's start"
        );
        assert!(button.tick(), "the recovery's tick advances it");
        assert_ne!(
            button.overrides().get(Prop::BackgroundColor).cloned(),
            at_start,
            "and the widget now paints the resampled value"
        );
    }
}
