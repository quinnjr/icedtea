//! `icedtea-clipboard` — a headless `wlr-data-control` clipboard manager.
//!
//! Owns the data-control Wayland connection and a bounded, deduping history,
//! and serves it over `org.icedtea.Clipboard` for the shell's popover. Threaded
//! like the compositor's D-Bus service: the Wayland event loop and the zbus
//! service run on separate threads, message-passing only.

// Removed once `manager`/`service` consume `history` (Task 4); until then the
// pure reducer is only exercised by its own unit tests.
#![allow(dead_code)]

mod history;
mod manager;

fn main() {
    // Fleshed out in the service-wiring task.
}
