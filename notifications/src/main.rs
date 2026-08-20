//! `icedtea-notifications` — the `org.freedesktop.Notifications` /
//! `org.icedtea.Notifications` D-Bus daemon. See
//! `docs/superpowers/specs/2026-08-20-icedtea-notifications-daemon-design.md`
//! for the design. This is milestone N2: the D-Bus service, expiry worker,
//! and `main` wiring (N3 adds DND/history hardening on top of what's
//! already here).
//!
//! No Wayland connection and no non-`Send` object to own a blocking main
//! loop — unlike `icedtea-clipboard`'s `main.rs`, which *is* that loop.
//! Here `main` just wires the store, spawns the expiry thread and the D-Bus
//! service, then parks: the zbus executor thread(s) and the expiry thread
//! do all the work, and the process stays alive until systemd kills it.

use std::sync::{Arc, Mutex};

use icedtea_notifications::{expiry, service, store::Store};

/// Bound on the closed-history ring (see `store::HISTORY_MAX`'s doc).
const HISTORY_MAX: usize = icedtea_notifications::store::HISTORY_MAX;

fn main() {
    let store = Arc::new(Mutex::new(Store::new(HISTORY_MAX)));
    let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (tick_tx, tick_rx) = crossbeam_channel::unbounded();

    std::thread::spawn({
        let store = store.clone();
        move || expiry::run(store, tick_rx, chg_tx)
    });

    let _dbus = service::spawn(store, chg_rx, tick_tx).expect(
        "failed to register org.freedesktop.Notifications -- another notification daemon \
         (mako, dunst, a stray previous instance) is already running; disable it before \
         starting icedtea-notifications",
    );

    // No Wayland loop to drive; park the main thread for the process's
    // life. Not a busy spin -- `park()` blocks until unparked, which
    // nothing here ever does, so this simply keeps the process (and its
    // background threads) alive until systemd sends a signal.
    loop {
        std::thread::park();
    }
}
