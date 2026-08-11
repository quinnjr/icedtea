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

    // Wake sources for the two `crossbeam_channel`s the loop can only drain
    // from inside a handler (see `backend::wake_source`'s doc): without
    // these, a D-Bus command or a finished config reload sent while the loop
    // is idle in `Until::Stop`'s blocking `dispatch(-1)` sits unseen until
    // some unrelated event happens to wake it, or never. Registration
    // failure here is as fatal as the setup above it -- there would be no
    // way to ever apply a D-Bus command or a config reload, which is not a
    // compositor worth booting.
    let (cmd_wake_write, cmd_wake_id) =
        backend::wake_source(&runtime).expect("failed to register the D-Bus command wake pipe");
    state.set_cmd_wake_source(cmd_wake_id);

    let (reload_wake_write, reload_wake_id) =
        backend::wake_source(&runtime).expect("failed to register the config-reload wake pipe");
    state.set_config_reload_wake_source(reload_wake_id);
    state.set_config_reload_wake(reload_wake_write);

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

    let wallpaper_rx = render::spawn_wallpaper_decode(state.config.appearance.wallpaper.clone());

    if let Err(err) = backend.run_all(&display, &mut state, &runtime, wlr::Until::Stop) {
        tracing::error!(?err, "event loop ended with an error");
    }

    // The decode worker may still be running; taking its result (if any) here
    // keeps the channel from being dropped mid-send. `set_decoded` is called
    // on a real result, matching the pre-port (calloop) handler's contract;
    // the ported-forward diagnostic is the `Disconnected` arm below, which
    // restores the warning the calloop version logged when the worker died
    // without ever sending one (a panic) -- Task 4's stub run() dropped that
    // diagnostic along with the rest of the event loop, and this is where it
    // belongs again. Neither arm paints anything: there is no image node
    // until parity work gives the scene one.
    match wallpaper_rx.try_recv() {
        Ok(image) => state.wallpaper.set_decoded(image),
        Err(crossbeam_channel::TryRecvError::Disconnected) => {
            tracing::warn!(
                "wallpaper decode worker thread exited without producing a result \
                 (it likely panicked); wallpaper stays solid-color"
            );
        }
        Err(crossbeam_channel::TryRecvError::Empty) => {}
    }

    dbus_quit_signal.store(true, Ordering::Relaxed);
    let _ = dbus_emitter_thread.join();
}
