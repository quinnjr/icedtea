//! The client-driven protocol test harness.
//!
//! Two halves:
//!
//! * [`Compositor`] boots the production compositor — the same
//!   `Display`/`Runtime`/`Backend`/`State` wiring `lib.rs::run()` does, minus
//!   the pieces a test has no use for (D-Bus service, wallpaper worker,
//!   config-reload pipe, background rect, signal source) — on a thread of its
//!   own, against the headless backend, and hands back its socket name plus
//!   the two production channels a test observes and drives it through: the
//!   `SeqEvent` stream and the `DbCommand` queue.
//! * [`TestClient`] is a real `wayland-client` that binds `wl_compositor`,
//!   `wl_shm` and `xdg_wm_base`, and maps one shm-backed xdg toplevel for
//!   real: commit, wait for configure, ack, attach a buffer, commit.
//!
//! Why a thread rather than a child process: `wlr::Runtime` and
//! `wlr::Display` are `!Send`, so the compositor thread has to *create*
//! everything itself and can never hand any of it back. Only the socket name
//! and the wake pipe's write half — both `Send` — cross the
//! `crossbeam_channel::bounded(1)` boot handshake.
//!
//! Why no `WAYLAND_DISPLAY`: libtest runs these tests on parallel threads and
//! each gets its own compositor with its own `add_socket_auto` name. A
//! process-global env var would be a race between them and would not even
//! name the right compositor. The client connects to an explicit path
//! instead, via `Connection::from_socket`.

// The harness is written against the API contract later tasks compile
// against, so any single test binary uses only part of it.
#![allow(dead_code)]

use std::io::Write as _;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use icedtea_compositor::dbus::DbCommand;
use icedtea_contract::{Event, SeqEvent, Snapshot};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
    wl_touch,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1,
    zwlr_data_control_source_v1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

/// How long any "wait for the compositor to do a thing" helper waits before
/// declaring the harness broken.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Size the client falls back to when the compositor's configure carries no
/// size of its own (0x0 means "you pick").
const FALLBACK_SIZE: (i32, i32) = (200, 100);

/// Set the headless-backend environment exactly once, no matter which test
/// thread reaches it first.
///
/// Copied rather than shared with `tests/headless_boot.rs`: each integration
/// test file is its own binary, so there is nothing to import. Same argument
/// as there — `Once::call_once` makes it structurally true that exactly one
/// thread ever runs the write and every other blocks until it returns, so no
/// thread can observe or cause a torn environment read.
fn ensure_headless_env() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // SAFETY (icedtea unsafe exception (c)): `Once::call_once` above
        // guarantees this closure runs on exactly one thread and that every
        // other thread calling `ensure_headless_env` blocks until it
        // finishes.
        unsafe {
            std::env::set_var("WLR_BACKENDS", "headless");
            std::env::set_var("WLR_HEADLESS_OUTPUTS", "1");
        }
    });
}

/// Serializes compositor *creation* across test threads.
///
/// Not a nicety, and not about the socket: wlroots keeps a single
/// process-global, unsynchronized `wl_array` of buffer-resource interfaces
/// (`buffer_resource_interfaces` in `types/buffer/resource.c`).
/// `wlr_buffer_register_resource_interface` — reached from
/// `init_graphics`, via the shm/linux-dmabuf/wl_drm globals — grows it with
/// `wl_array_add`, which *reallocs*, while `wlr_buffer_try_from_resource`
/// walks the same array from every running compositor's
/// `wl_surface.attach` handler, with no lock on either side. Two harness
/// compositors booting on different threads while a third dispatched a
/// client's attach segfaulted this test binary roughly one run in six
/// (confirmed under gdb: three threads inside
/// `wlr_buffer_try_from_resource`, one boot in flight).
///
/// Holding this lock across the whole boot makes those writes single
/// threaded, which is enough to close the race outright rather than merely
/// narrow it: registration is deduplicated by interface pointer, and every
/// compositor in the process registers the same static interfaces, so the
/// array is only ever *written* by the first boot — and no other compositor
/// exists to be reading it at that point. Every later boot only walks the
/// array to find its interfaces already there.
///
/// This is a wlroots-side process-global, not a harness bug and not
/// something the compositor could opt out of, so serializing boot is the
/// fix rather than serializing the tests: clients, event loops and
/// assertions all still run fully in parallel.
///
/// **Constraint on future changes.** The paragraph above is only sound
/// because *every* boot in the process registers the identical set of
/// static interface pointers, which is what makes "written only by the
/// first boot" true. This lock excludes writers from each other; it does
/// **not** exclude readers, and it cannot — the readers are other
/// compositors' running event loops. So a test binary that ever boots a
/// compositor with a *different* graphics configuration (a different
/// renderer, or one that skips linux-dmabuf) re-opens the race outright:
/// that boot would take the `wl_array_add` path with other compositors
/// live and walking the array. If a later task needs that, the boots with
/// differing configurations have to be kept out of the same process, not
/// merely out of each other's way.
static BOOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A headless compositor running the production event loop on its own thread.
pub struct Compositor {
    /// The socket name `add_socket_auto` picked; a `WAYLAND_DISPLAY` value,
    /// relative to `XDG_RUNTIME_DIR`.
    pub socket: String,
    /// The production event stream (`State::new`'s `dbus_tx`), exactly what
    /// the D-Bus emitter thread would consume.
    pub events: crossbeam_channel::Receiver<SeqEvent>,
    /// The production command queue (`State::set_command_receiver`), exactly
    /// what `dbus::WmInterface` would push onto. Prefer [`Compositor::send`],
    /// which also nudges the wake pipe.
    pub commands: crossbeam_channel::Sender<DbCommand>,
    /// Write half of the command wake pipe, so a send reaches a loop blocked
    /// in `Until::Stop`'s `dispatch(-1)`.
    wake: UnixStream,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Compositor {
    /// Boot a headless compositor on its own thread; returns once its socket
    /// exists. Panics (test context) on any boot failure.
    pub fn spawn() -> Compositor {
        ensure_headless_env();

        let (boot_tx, boot_rx) = crossbeam_channel::bounded::<(String, UnixStream)>(1);
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<SeqEvent>();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<DbCommand>();

        let thread = std::thread::spawn(move || {
            // See `BOOT_LOCK`: everything from here to the handshake below
            // touches wlroots' process-global buffer-interface registry.
            // Poison is irrelevant — a panicking boot leaves the registry
            // no worse off than a successful one, and the next `spawn`
            // failing for an unrelated earlier panic would only obscure it.
            let boot_guard = BOOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

            // Same order as `lib.rs::run()`, and load-bearing for the same
            // reasons documented there.
            let display = wlr::Display::new().expect("display");
            let runtime = wlr::Runtime::new().expect("runtime");
            let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
            runtime.init_graphics(&display, &backend).expect("graphics");
            runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
            // Unlike `lib.rs::run()`, which degrades, a harness that cannot
            // advertise xdg-decoration is simply broken: the SSD negotiation
            // test would then assert against a global that was never there.
            runtime
                .create_xdg_decoration_manager(&display)
                .expect("zxdg_decoration_manager_v1");
            // Same "harness cannot degrade" tone as the decoration manager
            // just above: a test that maps a layer panel needs the global
            // to actually exist, not just for `lib.rs::run()`'s production
            // boot to log and move on.
            runtime.create_layer_shell(&display, 4).expect("zwlr_layer_shell_v1");
            // Same "harness cannot degrade" tone: the selection tests bind
            // these globals directly and would assert against ones that were
            // never advertised.
            runtime
                .create_primary_selection_manager(&display)
                .expect("zwp_primary_selection_device_manager_v1");
            runtime
                .create_data_control_manager(&display)
                .expect("zwlr_data_control_manager_v1");
            runtime
                .create_virtual_keyboard_manager(&display)
                .expect("zwp_virtual_keyboard_manager_v1");
            // Same "harness cannot degrade" tone: the DnD tests inject
            // pointer motion/buttons and would have no manager to bind.
            runtime
                .create_virtual_pointer_manager(&display)
                .expect("zwlr_virtual_pointer_manager_v1");
            runtime.create_seat(&display, "seat0").expect("seat0");
            // Test-only: makes the seat advertise the touch capability so
            // headless clients can bind `wl_touch` and injected touch
            // points are accepted. Harness-only -- the real
            // `compositor/src/lib.rs` boot must NOT call this. Must come
            // AFTER `create_seat`: that call does not itself trigger a
            // capability recompute, so calling this before the seat exists
            // sets the flag but its own immediate recompute is a no-op (no
            // seat yet) and nothing re-triggers one afterward -- the seat
            // would never actually advertise touch.
            runtime.enable_test_touch();

            // `state` is declared after `display`/`runtime`/`backend` so that
            // ordinary end-of-scope drop order drops it first: `attach` hands
            // it a `Runtime` clone, which must not outlive the `Display`.
            let mut state =
                icedtea_compositor::state::State::new(icedtea_config::default_config(), event_tx);
            state.wayland.attach(runtime.clone());
            state.set_command_receiver(cmd_rx);

            let (cmd_wake_write, cmd_wake_id) =
                icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
            state.set_cmd_wake_source(cmd_wake_id);

            let socket = display.add_socket_auto().expect("wayland socket");
            // Deliberately no `set_var("WAYLAND_DISPLAY", ...)`: see the
            // module doc. The handshake is the only way out of this thread —
            // everything above is `!Send`.
            boot_tx.send((socket, cmd_wake_write)).expect("boot handshake");
            drop(boot_tx);
            drop(boot_guard);

            if let Err(err) = backend.run_all(&display, &mut state, &runtime, wlr::Until::Stop) {
                panic!("run_all: {err:?}");
            }
        });

        let (socket, wake) = boot_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("compositor thread never completed its boot handshake");

        Compositor {
            socket,
            events: event_rx,
            commands: cmd_tx,
            wake,
            thread: Some(thread),
        }
    }

    /// Absolute path of this compositor's socket.
    pub fn socket_path(&self) -> std::path::PathBuf {
        let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR must be set");
        std::path::Path::new(&dir).join(&self.socket)
    }

    /// Send a command the way `dbus::WmInterface::send` does: onto the
    /// channel, then a nudge on the wake pipe.
    pub fn send(&self, cmd: DbCommand) {
        self.commands.send(cmd).expect("compositor command channel closed");
        icedtea_compositor::backend::wake(&self.wake);
    }

    /// `GetState` round trip, with a [`TIMEOUT`] deadline.
    pub fn snapshot(&self) -> Snapshot {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.send(DbCommand::GetState(reply_tx));
        reply_rx
            .recv_timeout(TIMEOUT)
            .expect("compositor never answered GetState")
    }

    /// Synthesize a touch-down at `(x, y)` for touch point `id` on the
    /// compositor thread, via `wlr::Runtime::inject_touch_down`. Returns the
    /// grab serial it minted, or `None` if there was no seat or no surface
    /// under the point. Blocks on the reply so the injection has actually
    /// run before this returns -- see `DbCommand::InjectTouchDown`'s doc.
    pub fn inject_touch_down(&self, x: f64, y: f64, id: i32, time_msec: u32) -> Option<u32> {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.send(DbCommand::InjectTouchDown { x, y, id, time_msec, reply: reply_tx });
        reply_rx.recv_timeout(TIMEOUT).expect("compositor never answered InjectTouchDown")
    }

    /// Synthesize a touch-motion to `(x, y)` for touch point `id`. Blocks on
    /// the reply -- see [`Self::inject_touch_down`]'s doc.
    pub fn inject_touch_motion(&self, x: f64, y: f64, id: i32, time_msec: u32) {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.send(DbCommand::InjectTouchMotion { x, y, id, time_msec, reply: reply_tx });
        reply_rx.recv_timeout(TIMEOUT).expect("compositor never answered InjectTouchMotion");
    }

    /// Synthesize a touch-up for touch point `id`. Blocks on the reply --
    /// see [`Self::inject_touch_down`]'s doc.
    pub fn inject_touch_up(&self, id: i32, time_msec: u32) {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.send(DbCommand::InjectTouchUp { id, time_msec, reply: reply_tx });
        reply_rx.recv_timeout(TIMEOUT).expect("compositor never answered InjectTouchUp");
    }

    /// Block until an event matching `pred` arrives; panics on timeout.
    ///
    /// The predicate sees the inner [`Event`]; the `seq` wrapper is dropped
    /// because nothing a test asserts on depends on it (the sequence
    /// contract itself is `contract`'s own unit tests' job).
    pub fn wait_event(&self, pred: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + TIMEOUT;
        let mut seen: Vec<Event> = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for a matching event; saw: {seen:?}");
            }
            match self.events.recv_timeout(remaining) {
                Ok(SeqEvent { event, .. }) => {
                    if pred(&event) {
                        return event;
                    }
                    seen.push(event);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("timed out waiting for a matching event; saw: {seen:?}")
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    panic!("compositor event channel closed; saw: {seen:?}")
                }
            }
        }
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // The production shutdown path: `Quit` on the command channel plus a
        // wake, which `State::should_stop` honours through `quitting`.
        let _ = self.commands.send(DbCommand::Quit);
        icedtea_compositor::backend::wake(&self.wake);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
            && !std::thread::panicking()
        {
            panic!("compositor thread panicked");
        }
    }
}

/// What the client's `Dispatch` impls accumulate.
#[derive(Default)]
struct ClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// Every `mode` this client has been sent on its decoration object, in
    /// arrival order (1 = client-side, 2 = server-side). A `Vec` rather than
    /// a latest-only field because "the compositor answered *once*, with the
    /// right mode" is a stronger and more useful claim than "it eventually
    /// said the right thing", and only the history can express it.
    decoration_modes: Vec<u32>,
    /// Most recent `xdg_toplevel.configure` size.
    configured: Option<(i32, i32)>,
    /// How many `xdg_surface.configure` events have arrived, ever.
    ///
    /// Both the gate `map_toplevel` waits on before attaching its first
    /// buffer (`> 0`) and — via [`TestClient::configure_count`] — the only
    /// reliable "a *new* configure arrived" signal a test has. A counter
    /// rather than a flag or a serial because every configure is acked in
    /// the handler itself, so no serial ever survives to the caller, while
    /// `configured`/`states` only ever report the *latest* values:
    /// [`TestClient::wait_until`] evaluates its predicate before it pumps,
    /// so a predicate phrased over those alone returns `true` instantly on
    /// stale state if it already happened to hold. Anything asserting an
    /// idempotent-looking round trip ("request maximize while already
    /// maximized", "resize to the same geometry") must latch this counter
    /// first and wait for it to advance.
    configures: u32,
    /// `xdg_toplevel` states from the most recent configure.
    states: Vec<u32>,
    closed: bool,
    /// Most recent `zwlr_layer_surface_v1.configure` size.
    layer_configured: Option<(u32, u32)>,
    /// How many `zwlr_layer_surface_v1.configure` events have arrived, ever
    /// — the layer analogue of [`ClientState::configures`], and for the same
    /// reason: `layer_configured` only ever reports the *latest* size, so a
    /// predicate over it alone cannot tell "a fresh configure arrived" from
    /// "the old one is still sitting there". The unmap/remap round trip
    /// (final review I1) is exactly that case: the placement a remapped
    /// panel is configured with is byte-identical to the one it had before
    /// it unmapped, so only the counter can see the second one.
    layer_configures: u32,
    /// Every global the compositor advertised on the registry, in
    /// `(interface, name)` arrival order. Recorded for every global, not just
    /// the bound ones, so a test can assert a global *exists* without this
    /// harness having to bind it.
    globals: Vec<(String, u32)>,

    // --- selection (M4.1) ---
    seat: Option<wl_seat::WlSeat>,
    /// This client's keyboard, created when the seat advertises the keyboard
    /// capability — the source of the input serial `set_selection` needs.
    keyboard: Option<wl_keyboard::WlKeyboard>,
    virtual_keyboard_manager:
        Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,
    /// Lets a test spawn a [`VirtualPointerClient`] and mint pointer motion
    /// and button events without a real input device — the M4.2 drag-and-drop
    /// grab serial's source.
    virtual_pointer_manager:
        Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    /// This client's data device, created from the manager + seat during
    /// connect so it is listening before the client is ever focused.
    data_device: Option<wl_data_device::WlDataDevice>,
    /// The source this client last offered, kept alive so it can answer
    /// `send` for as long as it owns the selection.
    data_source: Option<wl_data_source::WlDataSource>,
    /// The most recent selection `wl_data_offer` the compositor delivered (the
    /// clipboard this client would paste from), or `None` if the selection was
    /// cleared.
    current_offer: Option<wl_data_offer::WlDataOffer>,
    /// Mimes advertised on the in-flight offer, reset when a new offer is
    /// introduced. One selection at a time, so a single vec suffices.
    offer_mimes: Vec<String>,
    /// The last input-event serial this client saw (keyboard enter/key). `None`
    /// on a headless seat with no keyboard capability — `set_selection` then
    /// passes 0.
    last_serial: Option<u32>,
    /// What this client's own data source answers `send` with.
    offered_mime: String,
    offered_payload: Vec<u8>,
    /// How many `wl_data_source.send` requests this client has serviced — the
    /// signal a reader uses to know the owner has written the payload. Shared
    /// by the `wl_data_source` and `zwlr_data_control_source_v1` send handlers,
    /// since a client owns the selection through one or the other, never both.
    source_sends: u32,

    // --- data-control (M4.1) ---
    data_control_manager: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    data_control_device: Option<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1>,
    /// The source a data-control client last set, kept alive to answer `send`.
    data_control_source: Option<zwlr_data_control_source_v1::ZwlrDataControlSourceV1>,
    /// The current data-control selection offer (the clipboard a manager reads).
    data_control_offer: Option<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1>,
    /// Mimes advertised on the in-flight data-control offer, reset per offer.
    data_control_mimes: Vec<String>,

    // --- primary selection (M4.1) ---
    primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    primary_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    primary_source: Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    primary_offer: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
    primary_mimes: Vec<String>,

    // --- pointer + drag-and-drop (M4.2) ---
    /// This client's pointer, created when the seat advertises the pointer
    /// capability -- the source of the implicit-grab serial `start_drag`
    /// needs. Mirrors `keyboard` above.
    pointer: Option<wl_pointer::WlPointer>,
    /// The last serial this client saw on `wl_pointer` (enter or button).
    /// `start_drag`'s serial must be the *button* press that grabbed the
    /// surface, and `button` always arrives after `enter`, so the latest
    /// value here is always the right one to pass.
    last_pointer_serial: Option<u32>,
    /// The drag offer delivered on `wl_data_device.enter` while this client
    /// is a drag-and-drop destination, or `None` before a drag has entered
    /// (or after it has left).
    dnd_offer: Option<wl_data_offer::WlDataOffer>,
    /// Whether `wl_data_device.drop` has arrived for the current drag.
    dropped: bool,

    // --- touch drag-and-drop (M4.2) ---
    /// This client's touch object, created when the seat advertises the
    /// touch capability -- mirrors `pointer`/`keyboard` above. The
    /// destination side of a touch drag learns about it entirely through
    /// `wl_data_device`, never through this, so its events are unused
    /// (`delegate_noop!` below); it only needs to exist so `get_touch` is
    /// gated on the capability rather than called unconditionally, which
    /// is a fatal protocol error on a seat that has not advertised touch.
    touch: Option<wl_touch::WlTouch>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ClientState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            state.globals.push((interface.clone(), name));
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => {
                    state.wm_base = Some(registry.bind(name, version.min(6), qh, ()));
                }
                "zxdg_decoration_manager_v1" => {
                    state.decoration_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                "wl_data_device_manager" => {
                    state.data_device_manager = Some(registry.bind(name, version.min(3), qh, ()));
                }
                "zwlr_data_control_manager_v1" => {
                    state.data_control_manager = Some(registry.bind(name, version.min(2), qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.virtual_keyboard_manager =
                        Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    state.virtual_pointer_manager =
                        Some(registry.bind(name, version.min(2), qh, ()));
                }
                "zwp_primary_selection_device_manager_v1" => {
                    state.primary_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // A client that never pongs gets killed by the compositor.
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for ClientState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Ack right here rather than in the pump: an unacked configure wedges
        // every later one the compositor would send (it will not send a new
        // one while the previous is outstanding), which silently breaks any
        // test that resizes, maximizes or fullscreens after mapping.
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configures = state.configures.saturating_add(1);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, states } => {
                state.configured = Some((width, height));
                state.states = states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
            }
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The mode is recorded raw (`1` client-side, `2` server-side) rather
        // than as the generated enum: the assertion a test wants to make is
        // about the value that crossed the wire.
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event
            && let Ok(mode) = mode.into_result()
        {
            state.decoration_modes.push(mode as u32);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Acked right here, the same reasoning `xdg_surface`'s `Dispatch`
        // impl gives: an unacked configure wedges every later one, and
        // `map_layer_panel`'s own commit-after-ack sequencing depends on
        // this having already happened by the time it runs.
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, height } = event {
            surface.ack_configure(serial);
            state.layer_configured = Some((width, height));
            state.layer_configures += 1;
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // A new offer is being introduced; its `offer(mime)` events follow
            // before the `selection`/`enter` that names it. Reset the mime
            // list so it reflects only this offer.
            wl_data_device::Event::DataOffer { .. } => state.offer_mimes.clear(),
            // The clipboard this client would paste from (or `None` if cleared).
            wl_data_device::Event::Selection { id } => state.current_offer = id,
            // A drag entered a surface owned by this client. `offer_mimes`
            // was already populated by the `data_offer`/`offer` events that
            // preceded this one on the wire (same dispatch pass, in order),
            // so accepting the single mime our test sources ever offer is
            // safe here. Real destination clients do the same accept +
            // set_actions dance before the compositor will deliver `drop`.
            wl_data_device::Event::Enter { serial, id, .. } => {
                state.dropped = false;
                if let Some(offer) = id {
                    if let Some(mime) = state.offer_mimes.first().cloned() {
                        offer.accept(serial, Some(mime));
                    }
                    offer.set_actions(
                        wl_data_device_manager::DndAction::Copy,
                        wl_data_device_manager::DndAction::Copy,
                    );
                    state.dnd_offer = Some(offer);
                }
            }
            // The drag session ended (successfully); `read_drag_offer` drives
            // the actual byte transfer from here.
            wl_data_device::Event::Drop => state.dropped = true,
            // wlroots sends `leave` immediately after a successful `drop`
            // too (not only for a drag that left without dropping), so this
            // must not blindly drop the offer: `Drop` is always dispatched
            // first (same wire order, same dispatch pass) and sets
            // `dropped`, so by the time this runs the flag already
            // distinguishes the two cases. Only a "left without dropping"
            // leave invalidates the offer here -- a successful drop's offer
            // stays live for `read_drag_offer`'s `receive`/`finish`.
            wl_data_device::Event::Leave => {
                if !state.dropped {
                    state.dnd_offer = None;
                }
            }
            wl_data_device::Event::Motion { .. } => {}
            _ => {}
        }
    }

    // `data_offer` (opcode 0) introduces a server-created wl_data_offer.
    event_created_child!(ClientState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offer_mimes.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The paste side asked for the data on `fd`: write our payload and drop
        // the fd (close), so the reader sees EOF after the bytes.
        if let wl_data_source::Event::Send { mime_type, fd } = event
            && mime_type == state.offered_mime
        {
            let mut f = std::fs::File::from(fd);
            let _ = f.write_all(&state.offered_payload);
            state.source_sends = state.source_sends.saturating_add(1);
        }
    }
}

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { .. } => state.data_control_mimes.clear(),
            zwlr_data_control_device_v1::Event::Selection { id } => state.data_control_offer = id,
            // PrimarySelection / Finished not needed for the clipboard read.
            _ => {}
        }
    }

    // `data_offer` (opcode 0) introduces a server-created data-control offer.
    event_created_child!(ClientState, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.data_control_mimes.push(mime_type);
        }
    }
}

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Same payload-write shape as the `wl_data_source` handler; shares the
        // offered payload + send counter (a client owns via one protocol only).
        if let zwlr_data_control_source_v1::Event::Send { mime_type, fd } = event
            && mime_type == state.offered_mime
        {
            let mut f = std::fs::File::from(fd);
            let _ = f.write_all(&state.offered_payload);
            state.source_sends = state.source_sends.saturating_add(1);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for ClientState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Create a keyboard the moment the seat advertises the capability, so
        // that on focus this client receives `wl_keyboard.enter` — the input
        // serial `set_selection` is validated against.
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            // Same reasoning as the keyboard above: created the moment the
            // capability appears, so this client is already listening for
            // `enter`/`button` by the time a virtual pointer moves over it —
            // the source of the implicit-grab serial `start_drag` needs.
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            // Gated the same way -- an unconditional `get_touch` is a fatal
            // `wl_seat.get_touch called when no touch capability` protocol
            // error on a seat that has not (yet) advertised touch.
            if caps.contains(wl_seat::Capability::Touch) && state.touch.is_none() {
                state.touch = Some(seat.get_touch(qh, ()));
            }
        }
    }
}

delegate_noop!(ClientState: ignore wl_touch::WlTouch);

impl Dispatch<wl_pointer::WlPointer, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Any serial-bearing pointer event is a valid serial; `button` (the
        // implicit grab `start_drag` validates against) always arrives after
        // `enter`, so the latest one recorded is always the right one.
        match event {
            wl_pointer::Event::Enter { serial, .. } => state.last_pointer_serial = Some(serial),
            wl_pointer::Event::Button { serial, .. } => state.last_pointer_serial = Some(serial),
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Any serial-bearing keyboard event is a valid serial for set_selection;
        // `enter` (on focus) is the one this harness relies on. The `keymap`
        // event's fd is dropped with the event.
        match event {
            wl_keyboard::Event::Enter { serial, .. } => state.last_serial = Some(serial),
            wl_keyboard::Event::Key { serial, .. } => state.last_serial = Some(serial),
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { .. } => state.primary_mimes.clear(),
            zwp_primary_selection_device_v1::Event::Selection { id } => state.primary_offer = id,
            _ => {}
        }
    }

    // `data_offer` (opcode 0) introduces a server-created primary offer.
    event_created_child!(ClientState, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            state.primary_mimes.push(mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_source_v1::Event::Send { mime_type, fd } = event
            && mime_type == state.offered_mime
        {
            let mut f = std::fs::File::from(fd);
            let _ = f.write_all(&state.offered_payload);
            state.source_sends = state.source_sends.saturating_add(1);
        }
    }
}

// wayland-client requires a `Dispatch` impl per bound interface; these carry
// nothing the harness asserts on.
delegate_noop!(ClientState: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(ClientState: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(ClientState: ignore zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(ClientState: ignore zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);
delegate_noop!(ClientState: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(ClientState: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);
delegate_noop!(ClientState: ignore zwlr_data_control_manager_v1::ZwlrDataControlManagerV1);
delegate_noop!(ClientState: ignore wl_compositor::WlCompositor);
delegate_noop!(ClientState: ignore wl_surface::WlSurface);
delegate_noop!(ClientState: ignore wl_shm::WlShm);
delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
delegate_noop!(ClientState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(ClientState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);

/// A real wayland client with exactly one mapped xdg toplevel.
pub struct TestClient {
    // Field order is drop order: the wayland objects go before the queue and
    // the connection that own their backing.
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    buffer: wl_buffer::WlBuffer,
    pool: wl_shm_pool::WlShmPool,
    toplevel: xdg_toplevel::XdgToplevel,
    xdg_surface: xdg_surface::XdgSurface,
    surface: wl_surface::WlSurface,
    /// The shm file stays open for as long as the pool refers to it.
    shm_file: std::fs::File,
    state: ClientState,
    queue: EventQueue<ClientState>,
    qh: QueueHandle<ClientState>,
    conn: Connection,
}

/// Connect to `socket` and bind every global this harness knows about.
///
/// Factored out of [`TestClient::map`] so [`LayerPanelClient::spawn`] can
/// share the identical connect-and-bind sequence -- both clients need the
/// same registry roundtrip, and a second implementation of it would be one
/// more place for the "twice: globals, then binds settle" comment below to
/// drift out of sync with reality.
fn connect_and_bind(
    socket: &str,
) -> (Connection, EventQueue<ClientState>, QueueHandle<ClientState>, ClientState) {
    let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR must be set");
    let path = std::path::Path::new(&dir).join(socket);
    let stream =
        UnixStream::connect(&path).unwrap_or_else(|e| panic!("connecting to {}: {e}", path.display()));
    let conn = Connection::from_socket(stream).expect("wayland connection");

    let mut queue = conn.new_event_queue::<ClientState>();
    let qh = queue.handle();
    let mut state = ClientState::default();

    let display = conn.display();
    let _registry = display.get_registry(&qh, ());
    // Twice: the first hears the globals, the second lets the binds (and
    // anything they announce, e.g. `wl_shm.format`) settle.
    queue.roundtrip(&mut state).expect("registry roundtrip");
    queue.roundtrip(&mut state).expect("bind roundtrip");

    // Create this client's data device now — before it is ever focused — so it
    // is already listening when the compositor delivers the selection offer on
    // keyboard-enter. Harmless for clients that never touch the clipboard.
    if let (Some(mgr), Some(seat)) = (state.data_device_manager.as_ref(), state.seat.as_ref()) {
        state.data_device = Some(mgr.get_data_device(seat, &qh, ()));
    }
    // Likewise the data-control device: a clipboard manager observes the seat
    // selection without ever being focused, so it must be listening from
    // connect. Harmless for clients that never read the clipboard.
    if let (Some(mgr), Some(seat)) = (state.data_control_manager.as_ref(), state.seat.as_ref()) {
        state.data_control_device = Some(mgr.get_data_device(seat, &qh, ()));
    }
    if let (Some(mgr), Some(seat)) = (state.primary_manager.as_ref(), state.seat.as_ref()) {
        state.primary_device = Some(mgr.get_device(seat, &qh, ()));
    }
    queue.roundtrip(&mut state).expect("data-device roundtrip");

    (conn, queue, qh, state)
}

/// Connect, complete the registry roundtrip, and return the interface names of
/// every global the compositor advertised. Lets a test assert a global exists
/// without the harness having to bind it.
pub fn advertised_globals(socket: &str) -> Vec<String> {
    let (_conn, _queue, _qh, state) = connect_and_bind(socket);
    state.globals.into_iter().map(|(interface, _)| interface).collect()
}

/// Read the current clipboard selection that `reader` has been offered, in
/// `mime`, driving `owner` (whose `wl_data_source` supplies the bytes) until it
/// has answered. Returns the transferred bytes.
///
/// The owner writes its whole payload inside one `send` dispatch, before the
/// reader drains the pipe, so the payload must fit the pipe buffer (~64 KiB on
/// Linux). Every selection payload in this suite is a short string; a larger
/// one would need a concurrent read instead of this write-then-read shape.
///
/// The transfer is inherently two-sided: `reader.receive` hands the compositor
/// an fd it forwards to `owner`'s data source as a `send`; `owner` must be
/// pumped for its `Dispatch` to write the payload and close its copy, at which
/// point `reader` sees EOF. A free function rather than a method because it
/// needs both clients at once.
pub fn read_selection(reader: &mut TestClient, owner: &mut TestClient, mime: &str) -> Vec<u8> {
    let offer = reader
        .state
        .current_offer
        .clone()
        .expect("no wl_data_offer was delivered to the reader (is it focused?)");
    let (read_end, write_end) = std::io::pipe().expect("pipe");
    offer.receive(mime.to_string(), write_end.as_fd());
    reader.conn.flush().expect("flush receive");
    // The compositor dups the write end into `owner`'s `send`; drop ours so the
    // owner's copy is the only writer left and EOF is reachable.
    drop(write_end);

    // Drive the owner until its data source has serviced the send (wrote the
    // payload and dropped its fd). Deadline-guarded so a wedged transfer fails
    // rather than hangs.
    let before = owner.state.source_sends;
    let deadline = Instant::now() + TIMEOUT;
    while owner.state.source_sends == before {
        assert!(
            Instant::now() < deadline,
            "the selection owner never serviced a data_source.send within {TIMEOUT:?}"
        );
        owner.pump();
        reader.pump();
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read selection");
    buf
}

/// As [`read_selection`], but reads the drag offer delivered to `dst` on
/// `wl_data_device.enter` (M4.2 drag-and-drop) rather than the selection
/// offer, and `finish`es it afterward -- the same "tell the compositor the
/// transfer completed" step a real DnD destination performs once it has
/// successfully received the data. `src` is the drag's origin client, whose
/// `wl_data_source` supplies the bytes exactly as the selection owner's does.
pub fn read_drag_offer(dst: &mut TestClient, src: &mut TestClient, mime: &str) -> Vec<u8> {
    let offer = dst
        .state
        .dnd_offer
        .clone()
        .expect("no drag offer was delivered to the destination (did the drag ever enter it?)");
    let (read_end, write_end) = std::io::pipe().expect("pipe");
    offer.receive(mime.to_string(), write_end.as_fd());
    dst.conn.flush().expect("flush receive");
    drop(write_end);

    let before = src.state.source_sends;
    let deadline = Instant::now() + TIMEOUT;
    while src.state.source_sends == before {
        assert!(
            Instant::now() < deadline,
            "the drag source never serviced a data_source.send within {TIMEOUT:?}"
        );
        src.pump();
        dst.pump();
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read drag offer");

    offer.finish();
    dst.conn.flush().expect("flush finish");
    let _ = dst.queue.roundtrip(&mut dst.state);

    buf
}

/// As [`read_selection`], but over the primary (middle-click) selection: the
/// reader reads its primary offer, the owner's primary source supplies bytes.
pub fn read_primary(reader: &mut TestClient, owner: &mut TestClient, mime: &str) -> Vec<u8> {
    let offer = reader
        .state
        .primary_offer
        .clone()
        .expect("no primary offer delivered to the reader");
    let (read_end, write_end) = std::io::pipe().expect("pipe");
    offer.receive(mime.to_string(), write_end.as_fd());
    reader.conn.flush().expect("flush receive");
    drop(write_end);

    let before = owner.state.source_sends;
    let deadline = Instant::now() + TIMEOUT;
    while owner.state.source_sends == before {
        assert!(
            Instant::now() < deadline,
            "the primary owner never serviced a send within {TIMEOUT:?}"
        );
        owner.pump();
        reader.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read primary selection");
    buf
}

/// As [`read_selection`], but the selection owner is a data-control client
/// (whose `zwlr_data_control_source_v1` supplies the bytes). The reader is a
/// focused `wl_data_device` client.
pub fn read_selection_from_data_control(
    reader: &mut TestClient,
    owner: &mut DataControlClient,
    mime: &str,
) -> Vec<u8> {
    let offer = reader
        .state
        .current_offer
        .clone()
        .expect("no wl_data_offer was delivered to the reader");
    let (read_end, write_end) = std::io::pipe().expect("pipe");
    offer.receive(mime.to_string(), write_end.as_fd());
    reader.conn.flush().expect("flush receive");
    drop(write_end);

    let before = owner.source_sends();
    let deadline = Instant::now() + TIMEOUT;
    while owner.source_sends() == before {
        assert!(
            Instant::now() < deadline,
            "the data-control owner never serviced a send within {TIMEOUT:?}"
        );
        owner.pump();
        reader.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read selection");
    buf
}

/// Build a `w`x`h` shm-backed buffer, solid opaque grey. Shared by
/// [`TestClient::map`] and [`LayerPanelClient::spawn`] -- both need exactly
/// this to answer their respective compositor-chosen size with a real
/// attach.
fn create_shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<ClientState>,
    w: i32,
    h: i32,
) -> (std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer) {
    let stride = w * 4;
    let len = (stride * h) as usize;

    let fd: OwnedFd = rustix::fs::memfd_create("icedtea-harness-shm", rustix::fs::MemfdFlags::CLOEXEC)
        .expect("memfd_create");
    rustix::fs::ftruncate(&fd, len as u64).expect("ftruncate");
    let mut shm_file = std::fs::File::from(fd);
    // Solid opaque grey. `Xrgb8888` (not `Argb8888`): it is the one format
    // every wlroots renderer is required to advertise.
    shm_file.write_all(&vec![0x80u8; len]).expect("write shm");
    shm_file.flush().expect("flush shm");

    let pool = shm.create_pool(shm_file.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Xrgb8888, qh, ());
    (shm_file, pool, buffer)
}

impl TestClient {
    /// Connect to `socket`, bind the globals, and map one shm-backed toplevel
    /// for real: commit, await configure, ack, attach, commit.
    pub fn map_toplevel(socket: &str, app_id: &str, title: &str) -> TestClient {
        Self::map(socket, app_id, title, false)
    }

    /// As [`TestClient::map_toplevel`], but the toplevel also gets a
    /// `zxdg_toplevel_decoration_v1` and states no mode preference of its
    /// own -- the "the compositor decides" path a client that defers to the
    /// server takes. Read the compositor's answer with
    /// [`TestClient::decoration_mode`].
    ///
    /// Created here rather than through a `create_decoration()` a test could
    /// call after mapping, because the protocol forbids that outright:
    /// wlroots raises `xdg_toplevel_decoration must not have a buffer at
    /// creation` (an unrecoverable protocol error that kills the
    /// connection) for a decoration created against a surface that already
    /// has one. The decoration therefore has to exist before the very first
    /// buffer attach, which is inside this constructor.
    pub fn map_decorated_toplevel(socket: &str, app_id: &str, title: &str) -> TestClient {
        Self::map(socket, app_id, title, true)
    }

    fn map(socket: &str, app_id: &str, title: &str, decorated: bool) -> TestClient {
        let (conn, mut queue, qh, mut state) = connect_and_bind(socket);

        let compositor = state.compositor.clone().expect("compositor did not advertise wl_compositor");
        let shm = state.shm.clone().expect("compositor did not advertise wl_shm");
        let wm_base = state.wm_base.clone().expect("compositor did not advertise xdg_wm_base");

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id.to_string());
        toplevel.set_title(title.to_string());
        let decoration = decorated.then(|| {
            let manager = state
                .decoration_manager
                .clone()
                .expect("compositor did not advertise zxdg_decoration_manager_v1");
            // No `set_mode` call at all: the client states no preference and
            // leaves the decision entirely to the compositor.
            manager.get_toplevel_decoration(&toplevel, &qh, ())
        });
        // The initial-commit handshake: an empty commit, then wait for the
        // compositor's first configure (acked inside the `Dispatch` impl).
        surface.commit();
        conn.flush().expect("flush");

        // `roundtrip`, not `blocking_dispatch`, for the same reason
        // `wait_until` uses it: `blocking_dispatch` returns only once *some*
        // event has been dispatched, so against a compositor that is alive
        // but never answers the initial commit it blocks forever and the
        // deadline below is never re-evaluated — a wedged CI job with no
        // output instead of a 5s assertion failure. A roundtrip's own
        // `wl_display.sync` reply is guaranteed by any live loop, so the
        // deadline stays honest.
        let deadline = Instant::now() + TIMEOUT;
        while state.configures == 0 {
            assert!(
                Instant::now() < deadline,
                "no xdg_surface.configure within {TIMEOUT:?}"
            );
            queue.roundtrip(&mut state).expect("configure roundtrip");
        }

        // A configure of 0x0 means "you choose"; the compositor's
        // `initial_commit` normally sizes us, so this is the safety net.
        let (w, h) = match state.configured {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => FALLBACK_SIZE,
        };
        let (shm_file, pool, buffer) = create_shm_buffer(&shm, &qh, w, h);

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
        conn.flush().expect("flush");
        queue.roundtrip(&mut state).expect("map roundtrip");

        TestClient {
            decoration,
            buffer,
            pool,
            toplevel,
            xdg_surface,
            surface,
            shm_file,
            state,
            queue,
            qh,
            conn,
        }
    }

    /// Pump the client queue until `pred(self)` holds or [`TIMEOUT`] elapses.
    /// Returns whether the predicate ever held.
    ///
    /// A `roundtrip` rather than `blocking_dispatch`: a roundtrip always
    /// returns (the compositor answers `wl_display.sync` immediately), which
    /// keeps the deadline honest — `blocking_dispatch` would sit forever on a
    /// compositor that has nothing to say, turning a failed assertion into a
    /// hung test.
    pub fn wait_until(&mut self, pred: impl Fn(&TestClient) -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if pred(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            self.queue.roundtrip(&mut self.state).expect("roundtrip");
            if pred(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Most recent `xdg_toplevel.configure` size.
    pub fn last_configure(&self) -> Option<(i32, i32)> {
        self.state.configured
    }

    /// `xdg_toplevel` states from the most recent configure.
    pub fn states(&self) -> &[u32] {
        &self.state.states
    }

    /// How many `xdg_surface.configure` events this client has seen.
    ///
    /// Monotonic, so it is the one signal that distinguishes "a new
    /// configure arrived" from "the old one still says what I expected".
    /// [`TestClient::wait_until`] checks its predicate *before* pumping, so
    /// any assertion whose expected end state may already hold — a second
    /// maximize, a resize to the same geometry — has to latch this first:
    ///
    /// ```ignore
    /// let n = client.configure_count();
    /// comp.send(/* … */);
    /// assert!(client.wait_until(|c| c.configure_count() > n));
    /// ```
    pub fn configure_count(&self) -> u32 {
        self.state.configures
    }

    /// Whether `xdg_toplevel.close` has arrived.
    pub fn closed(&self) -> bool {
        self.state.closed
    }

    /// The mode from the most recent decoration `configure`: `1` client-side,
    /// `2` server-side, `None` if none has arrived.
    pub fn decoration_mode(&self) -> Option<u32> {
        self.state.decoration_modes.last().copied()
    }

    /// Every decoration mode this client has been sent, in order.
    pub fn decoration_modes(&self) -> &[u32] {
        &self.state.decoration_modes
    }

    pub fn set_title(&mut self, title: &str) {
        self.toplevel.set_title(title.to_string());
        self.surface.commit();
        self.conn.flush().expect("flush");
    }

    pub fn request_maximize(&mut self, on: bool) {
        if on {
            self.toplevel.set_maximized();
        } else {
            self.toplevel.unset_maximized();
        }
        self.conn.flush().expect("flush");
    }

    pub fn request_fullscreen(&mut self, on: bool) {
        if on {
            self.toplevel.set_fullscreen(None);
        } else {
            self.toplevel.unset_fullscreen();
        }
        self.conn.flush().expect("flush");
    }

    /// Unmap without destroying: attach a null buffer and commit, exactly
    /// what a real client does to hide itself while keeping its toplevel
    /// alive (xdg-shell's "you may attach `null` to unmap" — the model row
    /// survives this, unlike [`TestClient::detach`]).
    pub fn unmap(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.conn.flush().expect("flush");
    }

    /// Destroy the toplevel cleanly, in the order xdg-shell requires
    /// (toplevel, then xdg_surface, then the wl_surface), and flush so the
    /// compositor sees it before the connection goes away.
    pub fn detach(self) {
        if let Some(decoration) = &self.decoration {
            decoration.destroy();
        }
        self.buffer.destroy();
        self.pool.destroy();
        self.toplevel.destroy();
        self.xdg_surface.destroy();
        self.surface.destroy();
        self.conn.flush().expect("flush");
        // `roundtrip` so the destroys have provably been processed before the
        // connection drops: a bare flush leaves it racing the socket close,
        // and the two produce different server-side teardown paths.
        let mut this = self;
        let TestClient { state, queue, .. } = &mut this;
        let _ = queue.roundtrip(state);
    }

    /// Own the clipboard: create a `wl_data_source` advertising `mime` with
    /// `payload`, and `set_selection` it with the last input serial this client
    /// saw (0 on a headless seat with no keyboard). The client must currently
    /// hold keyboard focus for the compositor to honor it.
    pub fn set_selection_text(&mut self, mime: &str, payload: &[u8]) {
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("compositor did not advertise wl_data_device_manager");
        let device = self.state.data_device.clone().expect("no data device");
        self.state.offered_mime = mime.to_string();
        self.state.offered_payload = payload.to_vec();
        let source = manager.create_data_source(&self.qh, ());
        source.offer(mime.to_string());
        device.set_selection(Some(&source), self.state.last_serial.unwrap_or(0));
        self.state.data_source = Some(source);
        self.conn.flush().expect("flush set_selection");
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// Whether this client created its `wl_data_device` (manager + seat both
    /// advertised).
    pub fn has_data_device(&self) -> bool {
        self.state.data_device.is_some()
    }

    /// Start a drag-and-drop session from this client's own surface: create a
    /// `wl_data_source` advertising `mime` with `payload`, declare it
    /// supports the `copy` action, and `start_drag` it with `serial` -- the
    /// implicit-grab serial of the `wl_pointer.button` press that began the
    /// drag ([`TestClient::last_pointer_serial`]). No icon: this harness
    /// passes `None`, so the icon path is not exercised here.
    pub fn start_drag_text(&mut self, mime: &str, payload: &[u8], serial: u32) {
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("compositor did not advertise wl_data_device_manager");
        let device = self.state.data_device.clone().expect("no data device");
        self.state.offered_mime = mime.to_string();
        self.state.offered_payload = payload.to_vec();
        let source = manager.create_data_source(&self.qh, ());
        source.offer(mime.to_string());
        source.set_actions(wl_data_device_manager::DndAction::Copy);
        device.start_drag(Some(&source), &self.surface, None, serial);
        self.state.data_source = Some(source);
        self.conn.flush().expect("flush start_drag");
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// The last serial this client saw on `wl_pointer` (enter or button) --
    /// the implicit-grab serial [`TestClient::start_drag_text`] needs. `None`
    /// if the seat has no pointer capability or the pointer never entered
    /// this client's surface.
    pub fn last_pointer_serial(&self) -> Option<u32> {
        self.state.last_pointer_serial
    }

    /// Whether `wl_data_device.drop` has arrived for the drag currently
    /// entering this client's surface.
    pub fn got_drop(&self) -> bool {
        self.state.dropped
    }

    /// Whether a drag offer has been delivered to this client via
    /// `wl_data_device.enter`.
    pub fn has_drag_offer(&self) -> bool {
        self.state.dnd_offer.is_some()
    }

    /// Whether a selection `wl_data_offer` has been delivered to this client.
    pub fn has_selection_offer(&self) -> bool {
        self.state.current_offer.is_some()
    }

    /// Own the primary (middle-click) selection with `payload` under `mime`.
    /// Like [`set_selection_text`](Self::set_selection_text), needs an input
    /// serial and keyboard focus.
    pub fn set_primary_text(&mut self, mime: &str, payload: &[u8]) {
        let manager = self.state.primary_manager.clone().expect("no primary manager");
        let device = self.state.primary_device.clone().expect("no primary device");
        self.state.offered_mime = mime.to_string();
        self.state.offered_payload = payload.to_vec();
        let source = manager.create_source(&self.qh, ());
        source.offer(mime.to_string());
        device.set_selection(Some(&source), self.state.last_serial.unwrap_or(0));
        self.state.primary_source = Some(source);
        self.conn.flush().expect("flush set_primary");
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// Whether a primary-selection offer has been delivered to this client.
    pub fn has_primary_offer(&self) -> bool {
        self.state.primary_offer.is_some()
    }

    /// Whether this client has captured an input serial (via `wl_keyboard`),
    /// which `set_selection` needs. Requires a keyboard on the seat (e.g. an
    /// injected virtual keyboard) and this client to hold focus.
    pub fn has_input_serial(&self) -> bool {
        self.state.last_serial.is_some()
    }

    /// One `roundtrip`, exposed so a transfer helper can drive an owner client
    /// whose data source must answer `send`.
    pub fn pump(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// How many `send` requests this client's source(s) have serviced.
    pub fn source_sends(&self) -> u32 {
        self.state.source_sends
    }

    /// Map a top-anchored `zwlr_layer_shell_v1` panel: `TOP | LEFT | RIGHT`
    /// anchor, `exclusive` reserved along the top edge, real shm-backed
    /// attach once the compositor answers with a size. See
    /// [`LayerPanelClient`] for the returned handle's own API --
    /// `wait_until`/`layer_configure`, mirroring `TestClient`'s own.
    pub fn map_layer_panel(socket: &str, exclusive: i32) -> LayerPanelClient {
        LayerPanelClient::spawn(socket, exclusive)
    }
}

/// A real `zwlr_layer_shell_v1` client, mapped as a top-anchored panel.
///
/// A separate type from [`TestClient`] rather than an `Option`-ified
/// generalization of it: a layer surface has no `xdg_toplevel` (no
/// maximize/fullscreen requests, no decoration, no `states()`), and
/// threading `Option`s for all of that through every one of `TestClient`'s
/// existing toplevel-only methods would make every call site (this crate's
/// whole existing suite) responsible for a case that can never apply to it.
pub struct LayerPanelClient {
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    /// The mapped buffer. Re-attached verbatim by [`LayerPanelClient::remap`]
    /// (the panel maps back at the same size it had), and kept alive for as
    /// long as the client is regardless: dropping the pool/buffer/file early
    /// would free the backing memory out from under a compositor that may
    /// still be reading it.
    buffer: wl_buffer::WlBuffer,
    /// The size `buffer` was created at, for `remap`'s damage rectangle.
    size: (i32, i32),
    _pool: wl_shm_pool::WlShmPool,
    _shm_file: std::fs::File,
    state: ClientState,
    queue: EventQueue<ClientState>,
    conn: Connection,
}

impl LayerPanelClient {
    /// Connect, bind `zwlr_layer_shell_v1`, and map a top-anchored panel
    /// reserving `exclusive` pixels: `set_anchor(TOP | LEFT | RIGHT)`,
    /// `set_exclusive_zone(exclusive)`, `set_size(0, exclusive as u32)`,
    /// commit, wait for `Configure`, ack (inside the `Dispatch` impl), then
    /// a real shm-backed attach at the compositor-chosen size and a second
    /// commit -- the same "commit, await configure, ack, attach, commit"
    /// shape [`TestClient::map`] follows for a toplevel.
    fn spawn(socket: &str, exclusive: i32) -> LayerPanelClient {
        let (conn, mut queue, qh, mut state) = connect_and_bind(socket);

        let compositor = state.compositor.clone().expect("compositor did not advertise wl_compositor");
        let shm = state.shm.clone().expect("compositor did not advertise wl_shm");
        let layer_shell = state
            .layer_shell
            .clone()
            .expect("compositor did not advertise zwlr_layer_shell_v1");

        let surface = compositor.create_surface(&qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "harness-panel".to_string(),
            &qh,
            (),
        );
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer_surface.set_exclusive_zone(exclusive);
        layer_surface.set_size(0, exclusive as u32);
        surface.commit();
        conn.flush().expect("flush");

        let deadline = Instant::now() + TIMEOUT;
        while state.layer_configured.is_none() {
            assert!(
                Instant::now() < deadline,
                "no zwlr_layer_surface_v1.configure within {TIMEOUT:?}"
            );
            queue.roundtrip(&mut state).expect("configure roundtrip");
        }

        let (w, h) = state.layer_configured.expect("just checked above");
        let (w, h) = if w > 0 && h > 0 { (w as i32, h as i32) } else { FALLBACK_SIZE };
        let (shm_file, pool, buffer) = create_shm_buffer(&shm, &qh, w, h);

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
        conn.flush().expect("flush");
        queue.roundtrip(&mut state).expect("map roundtrip");

        LayerPanelClient {
            surface,
            layer_surface,
            buffer,
            size: (w, h),
            _pool: pool,
            _shm_file: shm_file,
            state,
            queue,
            conn,
        }
    }

    /// Pump the client queue until `pred(self)` holds or [`TIMEOUT`]
    /// elapses. Mirrors [`TestClient::wait_until`] exactly.
    pub fn wait_until(&mut self, pred: impl Fn(&LayerPanelClient) -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if pred(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            self.queue.roundtrip(&mut self.state).expect("roundtrip");
            if pred(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Most recent `zwlr_layer_surface_v1.configure` size.
    pub fn layer_configure(&self) -> Option<(i32, i32)> {
        self.state.layer_configured.map(|(w, h)| (w as i32, h as i32))
    }

    /// Unmap without destroying: attach a null buffer and commit. Mirrors
    /// [`TestClient::unmap`] -- wlr-layer-shell defines the identical
    /// "attach null to unmap, the surface returns to its
    /// right-after-`get_layer_surface` state" contract (`layer.rs`'s own
    /// module doc), and the entry survives on the compositor side so a
    /// remap would find it again.
    pub fn unmap(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.conn.flush().expect("flush");
    }

    /// How many `zwlr_layer_surface_v1.configure` events have arrived, ever.
    /// See [`ClientState::layer_configures`] for why a remap test cannot use
    /// [`Self::layer_configure`] instead.
    pub fn layer_configure_count(&self) -> u32 {
        self.state.layer_configures
    }

    /// Map again after [`Self::unmap`], following the protocol's own
    /// re-initialization sequence: an empty commit (wlroots cleared the
    /// surface's `initialized` flag on the unmap, so this is a fresh
    /// *initial* commit and the compositor owes a mandatory configure),
    /// then — once that configure has arrived and been acked in the
    /// `Dispatch` impl — the buffer attach and the commit that maps it.
    ///
    /// Returns whether the mandatory configure actually arrived within
    /// [`TIMEOUT`]. `false` is the exact symptom of final review I1: the
    /// compositor recomputes the identical placement, its storm guard
    /// suppresses the send, and the client waits forever for a configure it
    /// can never map without.
    pub fn remap(&mut self) -> bool {
        let before = self.state.layer_configures;
        self.surface.commit();
        self.conn.flush().expect("flush");
        if !self.wait_until(|c| c.layer_configure_count() > before) {
            return false;
        }
        let (w, h) = self.size;
        self.surface.attach(Some(&self.buffer), 0, 0);
        self.surface.damage_buffer(0, 0, w, h);
        self.surface.commit();
        self.conn.flush().expect("flush");
        true
    }
}

impl Drop for LayerPanelClient {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
        self.conn.flush().expect("flush");
        let _ = self.queue.roundtrip(&mut self.state);
    }
}

/// Injects a virtual keyboard so the seat gains keyboard capability, which is
/// what lets other clients receive `wl_keyboard.enter` and mint the input
/// serial `wl_data_device.set_selection` requires. The headless backend has no
/// physical input device, so without this the seat advertises no keyboard and
/// `set_selection` can never be validated. Kept alive for the test's duration;
/// dropping it destroys the virtual keyboard and the capability with it.
pub struct VirtualKeyboardClient {
    conn: Connection,
    queue: EventQueue<ClientState>,
    state: ClientState,
    _vk: zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
}

impl VirtualKeyboardClient {
    /// Connect, create a virtual keyboard on the seat, and settle. Panics if
    /// the compositor did not advertise `zwp_virtual_keyboard_manager_v1`.
    pub fn spawn(socket: &str) -> VirtualKeyboardClient {
        let (conn, mut queue, qh, mut state) = connect_and_bind(socket);
        let manager = state
            .virtual_keyboard_manager
            .clone()
            .expect("compositor did not advertise zwp_virtual_keyboard_manager_v1");
        let seat = state.seat.clone().expect("no seat");
        let vk = manager.create_virtual_keyboard(&seat, &qh, ());
        conn.flush().expect("flush vk create");
        // Two roundtrips so the compositor processes new_virtual_keyboard and
        // the seat's capability change is on the wire before callers connect.
        queue.roundtrip(&mut state).expect("vk roundtrip");
        queue.roundtrip(&mut state).expect("vk settle");
        VirtualKeyboardClient { conn, queue, state, _vk: vk }
    }

    /// One roundtrip, to keep the injector responsive during a test.
    pub fn pump(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
    }
}

/// A `zwlr_virtual_pointer_manager_v1` client — an on-screen-keyboard-style
/// pointer injector. It mints the pointer motion and button events (and thus
/// the serial) a drag-and-drop grab needs on a headless seat that has no real
/// pointer device.
pub struct VirtualPointerClient {
    conn: Connection,
    queue: EventQueue<ClientState>,
    state: ClientState,
    vp: zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    /// Monotonic millisecond timestamp handed to every request; the
    /// compositor only cares that it does not go backwards, so a simple
    /// incrementing counter is fine for tests.
    time: u32,
}

impl VirtualPointerClient {
    /// Connect, create a virtual pointer on the seat, and settle. Panics if
    /// the compositor did not advertise `zwlr_virtual_pointer_manager_v1`.
    pub fn spawn(socket: &str) -> VirtualPointerClient {
        let (conn, mut queue, qh, mut state) = connect_and_bind(socket);
        let manager = state
            .virtual_pointer_manager
            .clone()
            .expect("compositor did not advertise zwlr_virtual_pointer_manager_v1");
        let seat = state.seat.clone();
        let vp = manager.create_virtual_pointer(seat.as_ref(), &qh, ());
        conn.flush().expect("flush vp create");
        // Two roundtrips so the compositor processes new_virtual_pointer and
        // the seat's capability change is on the wire before callers connect.
        queue.roundtrip(&mut state).expect("vp roundtrip");
        queue.roundtrip(&mut state).expect("vp settle");
        VirtualPointerClient { conn, queue, state, vp, time: 0 }
    }

    /// Next monotonic timestamp for a request.
    fn next_time(&mut self) -> u32 {
        self.time = self.time.saturating_add(1);
        self.time
    }

    /// Move the pointer to an absolute position in a `x_extent` by `y_extent`
    /// coordinate space + flush.
    pub fn motion_absolute(&mut self, x: f64, y: f64, x_extent: u32, y_extent: u32) {
        let time = self.next_time();
        self.vp.motion_absolute(time, x as u32, y as u32, x_extent, y_extent);
        self.conn.flush().expect("flush motion_absolute");
    }

    /// Press or release a button (Linux input-event code, e.g. `0x110` for
    /// left) + flush.
    pub fn button(&mut self, button: u32, pressed: bool) {
        let time = self.next_time();
        let state =
            if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
        self.vp.button(time, button, state);
        self.conn.flush().expect("flush button");
    }

    /// End the current event sequence + flush.
    pub fn frame(&mut self) {
        self.vp.frame();
        self.conn.flush().expect("flush frame");
    }

    /// One roundtrip, to keep the injector responsive during a test.
    pub fn pump(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
    }
}

/// A `zwlr_data_control_manager_v1` client — a clipboard manager. It never maps
/// a surface and never holds focus, yet can both set the selection (no input
/// serial required, unlike `wl_data_device`) and read it. This is what makes
/// the selection stack testable on a headless seat that has no input device to
/// mint the serial `wl_data_device.set_selection` would need.
pub struct DataControlClient {
    conn: Connection,
    queue: EventQueue<ClientState>,
    qh: QueueHandle<ClientState>,
    state: ClientState,
}

impl DataControlClient {
    /// Connect and bind; the data-control device is created inside
    /// [`connect_and_bind`]. Panics if the compositor did not advertise
    /// `zwlr_data_control_manager_v1`.
    pub fn spawn(socket: &str) -> DataControlClient {
        let (conn, queue, qh, state) = connect_and_bind(socket);
        assert!(
            state.data_control_device.is_some(),
            "compositor did not advertise zwlr_data_control_manager_v1"
        );
        DataControlClient { conn, queue, qh, state }
    }

    /// Own the clipboard with `payload` under `mime`. No serial: data-control
    /// is designed for focus-less clipboard managers.
    pub fn set_clipboard(&mut self, mime: &str, payload: &[u8]) {
        let manager = self.state.data_control_manager.clone().expect("no data-control manager");
        let device = self.state.data_control_device.clone().expect("no data-control device");
        self.state.offered_mime = mime.to_string();
        self.state.offered_payload = payload.to_vec();
        let source = manager.create_data_source(&self.qh, ());
        source.offer(mime.to_string());
        device.set_selection(Some(&source));
        self.state.data_control_source = Some(source);
        self.conn.flush().expect("flush data-control set");
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// Whether a data-control selection offer has been delivered.
    pub fn has_offer(&self) -> bool {
        self.state.data_control_offer.is_some()
    }

    /// Pump the queue until `pred` holds or [`TIMEOUT`] elapses.
    pub fn wait_until(&mut self, pred: impl Fn(&DataControlClient) -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if pred(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            let _ = self.queue.roundtrip(&mut self.state);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// One roundtrip, so a transfer helper can drive this client as the owner.
    pub fn pump(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// How many `send` requests this client's source has serviced — lets a
    /// transfer helper know the owner has written the payload.
    pub fn source_sends(&self) -> u32 {
        self.state.source_sends
    }

    /// Read the current data-control selection when the owner services `send`
    /// on its OWN thread (e.g. the `icedtea-clipboard` daemon re-pasting). We
    /// only need to send the receive and read; a spawned thread does the
    /// blocking read so a broken owner fails on the deadline instead of
    /// hanging the test.
    pub fn read_offer_blocking(&mut self, mime: &str) -> Vec<u8> {
        let offer = self
            .state
            .data_control_offer
            .clone()
            .expect("no data-control offer delivered");
        let (read_end, write_end) = std::io::pipe().expect("pipe");
        offer.receive(mime.to_string(), write_end.as_fd());
        self.conn.flush().expect("flush receive");
        drop(write_end);

        let handle = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = Vec::new();
            let mut read_end = read_end;
            let _ = read_end.read_to_end(&mut buf);
            buf
        });
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if handle.is_finished() {
                return handle.join().expect("read thread panicked");
            }
            assert!(Instant::now() < deadline, "self-served selection read timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Read the current data-control selection when the owner is a *regular*
    /// `wl_data_device` client (an ordinary app that copied). This is the
    /// direction the M4.6 clipboard daemon relies on: an app copies, the
    /// manager observes it. `owner`'s `wl_data_source` supplies the bytes.
    pub fn read_from_wl_data_device_owner(
        &mut self,
        owner: &mut TestClient,
        mime: &str,
    ) -> Vec<u8> {
        let offer = self
            .state
            .data_control_offer
            .clone()
            .expect("no data-control offer delivered");
        let (read_end, write_end) = std::io::pipe().expect("pipe");
        offer.receive(mime.to_string(), write_end.as_fd());
        self.conn.flush().expect("flush receive");
        drop(write_end);

        let before = owner.source_sends();
        let deadline = Instant::now() + TIMEOUT;
        while owner.source_sends() == before {
            assert!(
                Instant::now() < deadline,
                "the wl_data_device owner never serviced a send within {TIMEOUT:?}"
            );
            owner.pump();
            self.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read data-control selection");
        buf
    }

    /// Read the current data-control selection in `mime`, driving `owner`
    /// (whose source supplies the bytes) until it answers. Mirrors
    /// [`read_selection`] but the reader side is a data-control device.
    pub fn read_selection(&mut self, owner: &mut DataControlClient, mime: &str) -> Vec<u8> {
        let offer = self
            .state
            .data_control_offer
            .clone()
            .expect("no data-control offer delivered");
        let (read_end, write_end) = std::io::pipe().expect("pipe");
        offer.receive(mime.to_string(), write_end.as_fd());
        self.conn.flush().expect("flush receive");
        drop(write_end);

        let before = owner.state.source_sends;
        let deadline = Instant::now() + TIMEOUT;
        while owner.state.source_sends == before {
            assert!(
                Instant::now() < deadline,
                "the data-control owner never serviced a send within {TIMEOUT:?}"
            );
            owner.pump();
            self.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut { read_end }, &mut buf).expect("read data-control selection");
        buf
    }
}
