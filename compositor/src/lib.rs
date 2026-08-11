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
/// 2. `Runtime::new` before `Backend::autocreate`, because the scene has to
///    exist before an output can be announced into it — a backend announces
///    the outputs it already has from inside its own start, and nothing
///    replays that announcement.
/// 3. `add_socket_auto` and the `WAYLAND_DISPLAY` set after the backend, so
///    that a nested backend opens its window against the *host* session's
///    display before that variable is clobbered for our own children.
pub fn run() {
    let choice = backend::BackendChoice::from_args(std::env::args());
    backend::apply_backend_choice(choice);

    let db_path = icedtea_config::default_db_path();
    let config = icedtea_config::load_or_default(&db_path);

    let (dbus_tx, dbus_events_rx) = crossbeam_channel::unbounded::<SeqEvent>();
    let mut state = State::new(config, dbus_tx);

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<dbus::DbCommand>();
    let dbus_quit_signal = Arc::new(AtomicBool::new(false));
    let (_dbus_conn, dbus_emitter_thread) =
        dbus::spawn_service(dbus_events_rx, cmd_tx, dbus_quit_signal.clone());
    state.set_command_receiver(cmd_rx);

    let (config_reload_tx, config_reload_rx) =
        crossbeam_channel::unbounded::<icedtea_config::Config>();
    state.set_config_reload_sender(config_reload_tx);
    state.set_config_reload_receiver(config_reload_rx);

    let display = wlr::Display::new().expect("failed to create the wayland display");
    let runtime = wlr::Runtime::new().expect("failed to create the scene graph");
    let backend = wlr::Backend::autocreate(&display.event_loop())
        .expect("failed to create a backend; is a session available, or WLR_BACKENDS set?");
    runtime
        .init_graphics(&display, &backend)
        .expect("failed to create the renderer and the core protocol globals");
    state.wayland.attach(runtime.clone());

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

    let socket = display
        .add_socket_auto()
        .expect("failed to create a wayland socket; is XDG_RUNTIME_DIR set?");
    // SAFETY (icedtea unsafe exception (c)): single-threaded with respect to
    // the environment at this point. The D-Bus thread and the wallpaper
    // worker are already running but neither reads the environment, and this
    // must happen after `autocreate` so a nested backend connected to the
    // host's display first.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket);
    }
    tracing::info!(%socket, "listening on wayland socket");

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
