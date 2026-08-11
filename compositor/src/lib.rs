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

/// Boot the compositor: loads config, wires the D-Bus service and the
/// wallpaper decode worker, then drains whatever they produced once and
/// exits. There is no compositor backend to run yet -- `wayland.rs`'s
/// no-op seam has nothing behind it until task 5 wires `wlr` onto it -- so
/// there is no event loop here either; that returns alongside the backend.
pub fn run() {
    let choice = backend::BackendChoice::from_args(std::env::args());

    let db_path = icedtea_config::default_db_path();
    let config = icedtea_config::load_or_default(&db_path);

    let (dbus_tx, dbus_events_rx) = crossbeam_channel::unbounded::<SeqEvent>();
    let mut state = State::new(config, dbus_tx);

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<dbus::DbCommand>();
    let dbus_quit_signal = Arc::new(AtomicBool::new(false));
    let (_dbus_conn, dbus_emitter_thread) =
        dbus::spawn_service(dbus_events_rx, cmd_tx, dbus_quit_signal.clone());

    let (config_reload_tx, config_reload_rx) = crossbeam_channel::unbounded::<icedtea_config::Config>();
    state.set_config_reload_sender(config_reload_tx);
    state.set_config_reload_receiver(config_reload_rx);

    let wallpaper_rx = render::spawn_wallpaper_decode(state.config.appearance.wallpaper.clone());

    tracing::error!(
        ?choice,
        "no compositor backend is wired up in this build: the smithay backend has been \
         removed and the wlr one lands in the next commit. Nothing will be displayed."
    );

    // Drain what the workers produced so their threads are not left blocked
    // on a full channel, then shut down. This is the whole of the loop until
    // the next commit gives it a real one.
    for cmd in cmd_rx.try_iter() {
        state.handle_command(cmd);
    }
    state.drain_config_reload();
    if let Ok(image) = wallpaper_rx.try_recv() {
        state.wallpaper.set_decoded(image);
    }

    dbus_quit_signal.store(true, Ordering::Relaxed);
    let _ = dbus_emitter_thread.join();
}
