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
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

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
            runtime.create_seat(&display, "seat0").expect("seat0");

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

// wayland-client requires a `Dispatch` impl per bound interface; these five
// carry nothing the harness asserts on.
delegate_noop!(ClientState: ignore wl_compositor::WlCompositor);
delegate_noop!(ClientState: ignore wl_surface::WlSurface);
delegate_noop!(ClientState: ignore wl_shm::WlShm);
delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
delegate_noop!(ClientState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);

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
        let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR must be set");
        let path = std::path::Path::new(&dir).join(socket);
        let stream = UnixStream::connect(&path)
            .unwrap_or_else(|e| panic!("connecting to {}: {e}", path.display()));
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
        let stride = w * 4;
        let len = (stride * h) as usize;

        let fd: OwnedFd = rustix::fs::memfd_create("icedtea-harness-shm", rustix::fs::MemfdFlags::CLOEXEC)
            .expect("memfd_create");
        rustix::fs::ftruncate(&fd, len as u64).expect("ftruncate");
        let mut shm_file = std::fs::File::from(fd);
        // Solid opaque grey. `Xrgb8888` (not `Argb8888`): it is the one
        // format every wlroots renderer is required to advertise.
        shm_file.write_all(&vec![0x80u8; len]).expect("write shm");
        shm_file.flush().expect("flush shm");

        let pool = shm.create_pool(shm_file.as_fd(), len as i32, &qh, ());
        let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Xrgb8888, &qh, ());

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
}
