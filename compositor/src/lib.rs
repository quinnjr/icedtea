//! `icedtea-compositor` library crate: everything the binary needs lives
//! here (mirroring anvil's `main.rs`/`lib.rs` split) so that integration
//! tests -- and any future embedder -- can drive `State`, `apply_action`,
//! and `handle_command` directly without going through a real event loop,
//! DRM session, or D-Bus connection.

pub mod backend;
pub mod config_combo;
pub mod dbus;
pub mod decoration;
pub mod input;
pub mod layout;
pub mod render;
pub mod state;
pub mod text;
pub mod wayland;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use icedtea_contract::SeqEvent;

use state::State;

/// Boot and run the compositor.
///
/// Ordering is load-bearing and each step says why:
///
/// 1. `apply_backend_choice` before anything, because `autocreate` reads
///    `WLR_BACKENDS` when it runs and never again.
/// 2. `Runtime::new` before `init_graphics`, which needs the display, the
///    backend, and the runtime all to already exist. There is no ordering
///    requirement between `Runtime::new` and `Backend::autocreate`
///    themselves: `Backend::autocreate` announces nothing on its own -- the
///    backend's existing outputs are announced to `OutputHandler::new_output`
///    from inside `run_all`'s own start (`ensure_started`), well after
///    `state.wayland.attach` below has already handed the seam a runtime to
///    enable/init an output against.
/// 3. `state` is constructed after `display`/`runtime`/`backend` (and so, by
///    ordinary end-of-scope drop order, is dropped before them): `attach`
///    gives `state.wayland` a `Runtime` clone, and `wlr` documents that a
///    `Runtime` must not outlive the `Display` it was initialized against.
/// 4. `add_socket_auto` and the `WAYLAND_DISPLAY` set happen after the
///    backend, so that a nested backend opens its window against the
///    *host* session's display before that variable is clobbered for our
///    own children -- and, just as importantly, before the D-Bus service or
///    the wallpaper decode worker are spawned: the `set_var` below is
///    single-threaded-with-respect-to-the-environment only if nothing else
///    is running yet, so both of those threads start *after* it, not before.
pub fn run() {
    let choice = backend::BackendChoice::from_args(std::env::args());
    backend::apply_backend_choice(choice);

    let db_path = icedtea_config::default_db_path();
    let config = icedtea_config::load_or_default(&db_path);

    let display = wlr::Display::new().expect("failed to create the wayland display");
    let runtime = wlr::Runtime::new().expect("failed to create the scene graph");
    let backend = wlr::Backend::autocreate(&display.event_loop())
        .expect("failed to create a backend; is a session available, or WLR_BACKENDS set?");
    runtime
        .init_graphics(&display, &backend)
        .expect("failed to create the renderer and the core protocol globals");
    runtime
        .create_xdg_shell(&display, 6)
        .expect("failed to advertise xdg_wm_base");
    // Not fatal, matching the shutdown source's tone below: without the
    // manager a client simply never gets to state a decoration preference
    // and draws its own or not on its own defaults, which is a degraded
    // compositor, not a dead one.
    if let Err(err) = runtime.create_xdg_decoration_manager(&display) {
        tracing::error!(%err, "xdg-decoration negotiation is unavailable");
    }
    // Same non-fatal tone as the decoration manager just above: without
    // this global, panels/bars simply cannot bind `zwlr_layer_shell_v1` and
    // the desktop runs with no shell chrome, which is degraded, not dead.
    if let Err(err) = runtime.create_layer_shell(&display, 4) {
        tracing::error!(%err, "layer-shell is unavailable");
    }
    // Same non-fatal tone: without these, middle-click paste and
    // clipboard-manager access are simply absent, which is degraded, not
    // dead. Regular clipboard (`wl_data_device`) already came up in
    // `init_graphics`; the seat's selection request events are wired in the
    // backend regardless.
    if let Err(err) = runtime.create_primary_selection_manager(&display) {
        tracing::error!(%err, "primary selection (middle-click paste) is unavailable");
    }
    if let Err(err) = runtime.create_data_control_manager(&display) {
        tracing::error!(%err, "data-control (clipboard manager access) is unavailable");
    }
    runtime
        .create_seat(&display, "seat0")
        .expect("failed to create the seat");

    let (dbus_tx, dbus_events_rx) = crossbeam_channel::unbounded::<SeqEvent>();
    let mut state = State::new(config, dbus_tx);
    state.wayland.attach(runtime.clone());

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<dbus::DbCommand>();
    state.set_command_receiver(cmd_rx);

    let (config_reload_tx, config_reload_rx) =
        crossbeam_channel::unbounded::<icedtea_config::Config>();
    state.set_config_reload_sender(config_reload_tx);
    state.set_config_reload_receiver(config_reload_rx);

    // Sized to nothing until an output arrives with a mode; `new_output`
    // resizes it. Lowered now so nothing later has to remember to.
    let background = runtime
        .add_rect(1, 1, render::wallpaper_color(&state.config.appearance))
        .expect("failed to create the background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    match backend::shutdown_source(&runtime) {
        Ok(source) => state.set_shutdown_source(source),
        // Not fatal: the compositor runs, it just cannot be stopped with a
        // signal, and saying so is better than refusing to boot.
        Err(err) => tracing::error!(%err, "SIGINT/SIGTERM will not stop the compositor"),
    }

    // Wake sources for the three `crossbeam_channel`s the loop can only
    // drain from inside a handler (see `backend::wake_source`'s doc):
    // without these, a D-Bus command, a finished config reload, or a
    // finished wallpaper decode sent while the loop is idle in
    // `Until::Stop`'s blocking `dispatch(-1)` sits unseen until some
    // unrelated event happens to wake it, or never. Registration failure
    // here is as fatal as the setup above it -- there would be no way to
    // ever apply a D-Bus command, a config reload, or a wallpaper decode,
    // which is not a compositor worth booting.
    let (cmd_wake_write, cmd_wake_id) =
        backend::wake_source(&runtime).expect("failed to register the D-Bus command wake pipe");
    state.set_cmd_wake_source(cmd_wake_id);

    let (reload_wake_write, reload_wake_id) =
        backend::wake_source(&runtime).expect("failed to register the config-reload wake pipe");
    state.set_config_reload_wake_source(reload_wake_id);
    state.set_config_reload_wake(reload_wake_write);

    let (wallpaper_wake_write, wallpaper_wake_id) =
        backend::wake_source(&runtime).expect("failed to register the wallpaper-decode wake pipe");
    state.set_wallpaper_wake_source(wallpaper_wake_id);
    // Review finding C1: `wallpaper_wake_write` is *not* handed to the
    // decode worker thread directly -- that was the bug (see
    // `State::spawn_wallpaper`'s doc). Keeping the original here and letting
    // `spawn_wallpaper` hand the worker a `try_clone`d copy is the same
    // pattern `config_reload_wake`/`spawn_config_reload` already use.
    state.set_wallpaper_wake(wallpaper_wake_write);

    let socket = display
        .add_socket_auto()
        .expect("failed to create a wayland socket; is XDG_RUNTIME_DIR set?");
    // SAFETY (icedtea unsafe exception (c)): single-threaded, full stop --
    // nothing above this point spawns a thread. The D-Bus service and the
    // wallpaper decode worker are both started below, after this write, so
    // nothing else in the process can be reading or writing the environment
    // concurrently with it.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket);
    }
    tracing::info!(%socket, "listening on wayland socket");

    let dbus_quit_signal = Arc::new(AtomicBool::new(false));
    let (_dbus_conn, dbus_emitter_thread) =
        dbus::spawn_service(dbus_events_rx, cmd_tx, dbus_quit_signal.clone(), cmd_wake_write);

    let wallpaper_path = state.config.appearance.wallpaper.clone();
    state.spawn_wallpaper(wallpaper_path);

    if let Err(err) = backend.run_all(&display, &mut state, &runtime, wlr::Until::Stop) {
        tracing::error!(?err, "event loop ended with an error");
    }

    // Live delivery is `wallpaper_wake_source`'s `fd_ready` arm above,
    // wired the same way the D-Bus command and config-reload channels are:
    // the decode worker nudges the wake pipe after it sends, so a result
    // that arrives while the loop is running is applied within that same
    // turn, not after `run_all` returns. This call is the shutdown safety
    // net for the one case that wiring can't cover -- a result that arrives
    // (or a decode that finishes) after `run_all` has already returned --
    // so the channel isn't dropped mid-send and `state.wallpaper` still
    // ends up correct even on a compositor that quit immediately after
    // boot. `drain_wallpaper` is panic-free on a disconnected sender, same
    // as the live path.
    state.drain_wallpaper();

    dbus_quit_signal.store(true, Ordering::Relaxed);
    let _ = dbus_emitter_thread.join();
}
