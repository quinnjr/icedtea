//! `icedtea-clipboard` — a headless `wlr-data-control` clipboard manager.
//!
//! Owns the data-control Wayland connection and a bounded, deduping history,
//! and serves it over `org.icedtea.Clipboard` for the shell's popover. Threaded
//! like the compositor's D-Bus service: the Wayland event loop runs on the main
//! thread and the zbus service on its own, message-passing only.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use icedtea_clipboard::{history, manager, service};
use wayland_client::Connection;

/// Max unpinned history entries kept.
const HISTORY_MAX: usize = 50;

fn main() {
    let conn = Connection::connect_to_env().expect("no Wayland display (is WAYLAND_DISPLAY set?)");

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (wake_read, wake_write) = UnixStream::pair().expect("wake pipe");
    let snapshot = Arc::new(Mutex::new(Vec::new()));

    // The service keeps the D-Bus connection alive for the process lifetime.
    let _dbus = service::spawn(snapshot.clone(), cmd_tx, wake_write, chg_rx)
        .expect("failed to register org.icedtea.Clipboard (session bus? another daemon?)");

    // Runs until the Wayland connection dies.
    manager::run(conn, history::History::new(HISTORY_MAX), cmd_rx, chg_tx, wake_read, snapshot);
}
