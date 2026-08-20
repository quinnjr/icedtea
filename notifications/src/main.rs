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

use icedtea_contract::NOTIF_BUS_NAME;
use icedtea_notifications::service::SpawnError;
use icedtea_notifications::{expiry, service, store::Store};

/// Bound on the closed-history ring (see `store::HISTORY_MAX`'s doc).
const HISTORY_MAX: usize = icedtea_notifications::store::HISTORY_MAX;

fn main() {
    let store = Arc::new(Mutex::new(Store::new(HISTORY_MAX)));
    let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (tick_tx, tick_rx) = crossbeam_channel::unbounded();

    std::thread::spawn({
        let store = store.clone();
        let chg_tx = chg_tx.clone();
        move || expiry::run(store, tick_rx, chg_tx)
    });

    let _dbus = match service::spawn(store, chg_tx, chg_rx, tick_tx) {
        Ok(conn) => conn,
        Err(SpawnError::NameTaken(msg)) => {
            // Decision 6: a bus-name conflict is fatal and loud — but NOT a
            // crash. Exiting cleanly (code 0) with `Restart=on-failure` in the
            // unit means systemd does NOT respawn us into a crash-loop against
            // an already-running mako/dunst; we simply, quietly, stay out of
            // the way. Never steal the name.
            eprintln!("icedtea-notifications: {msg}");
            eprintln!(
                "Another notification daemon (mako, dunst, or a stray previous instance) already \
                 owns {NOTIF_BUS_NAME}. Disable it before starting icedtea-notifications. Exiting."
            );
            std::process::exit(0);
        }
        Err(SpawnError::Bus(err)) => {
            // A genuine D-Bus fault (no session bus, registration failure).
            // Exit non-zero so `Restart=on-failure` can retry a transient one.
            eprintln!("icedtea-notifications: fatal D-Bus error: {err}");
            std::process::exit(1);
        }
    };

    // No Wayland loop to drive; park the main thread for the process's
    // life. Not a busy spin -- `park()` blocks until unparked, which
    // nothing here ever does, so this simply keeps the process (and its
    // background threads) alive until systemd sends a signal.
    loop {
        std::thread::park();
    }
}
