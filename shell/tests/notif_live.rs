//! A live D-Bus round-trip between the shell's real `NotifierProxy` and the
//! real `icedtea-notifications` service. This is the coverage the
//! mock-based `notif.rs` tests deliberately avoid — and the exact thing that
//! hid the shipping bug where the shell called snake_case members while
//! zbus exposes PascalCase. The member-name pin tests in `notif_client.rs`
//! prove the constant strings appear in the daemon source; this test proves
//! the proxy sends them on the right interface with the right bodies: if
//! `CloseNotification` moved interfaces or an `(id,)` body became `(id,
//! key)`, this fails.
//!
//! Skipped (visibly) only when the session bus is unavailable or
//! `org.freedesktop.Notifications` is already owned (a real daemon — mako,
//! dunst, or a stray instance — is running).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use icedtea_contract::NOTIF_BUS_NAME;
use icedtea_notifications::service;
use icedtea_notifications::store::Store;
use icedtea_notifications::{expiry, store::HISTORY_MAX};
use icedtea_shell::notif_client::{NotifierCommands, NotifierProxy};

fn notify(conn: &zbus::blocking::Connection, summary: &str) -> u32 {
    let reply = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            icedtea_contract::NOTIF_PATH,
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "live-test",
                0u32,
                "",
                summary,
                "body",
                vec!["default".to_string(), "Open".to_string()],
                HashMap::<String, zbus::zvariant::OwnedValue>::new(),
                50_000i32,
            ),
        )
        .expect("Notify call failed");
    reply
        .body()
        .deserialize::<u32>()
        .expect("Notify reply is the id")
}

#[test]
fn notifier_proxy_commands_reach_the_real_service() {
    let store = Arc::new(Mutex::new(Store::new(HISTORY_MAX)));
    let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
    std::thread::spawn({
        let store = store.clone();
        let chg_tx = chg_tx.clone();
        move || expiry::run(store, tick_rx, chg_tx)
    });

    let _daemon = match service::spawn(store, chg_tx, chg_rx, tick_tx) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!(
                "SKIP: cannot register {NOTIF_BUS_NAME} ({err}) -- bus down or a daemon is running"
            );
            return;
        }
    };

    let conn = zbus::blocking::Connection::session().expect("session bus");
    let proxy = NotifierProxy::from_connection(conn);

    // DND round-trips through the daemon flag, not just the mock.
    proxy.set_dnd(true);
    assert_eq!(proxy.get_dnd(), Some(true));

    // A card posted while DND is on stays out of the active set but the
    // daemon still answers for it; the proxy's GetActive proves the member
    // and interface are right.
    let id = notify(
        &zbus::blocking::Connection::session().expect("session bus"),
        "live card",
    );
    let active = proxy.get_active().expect("GetActive must succeed");
    assert!(
        !active.iter().any(|n| n.id == id),
        "DND-suppressed card must not be active"
    );

    proxy.set_dnd(false);
    assert_eq!(proxy.get_dnd(), Some(false));
    let active = proxy.get_active().expect("GetActive must succeed");
    assert!(active.iter().any(|n| n.id == id));

    // Actions and dismissal reach the service: invoking the default action
    // on a non-resident card removes it (daemon `invoke_action` semantics).
    proxy.invoke_action(id, "default");
    let active = proxy.get_active().expect("GetActive must succeed");
    assert!(!active.iter().any(|n| n.id == id));

    // Close-by-id works too, and the closed card lands in GetHistory (the
    // daemon's closed ring) — proving both members at once.
    let id2 = notify(
        &zbus::blocking::Connection::session().expect("session bus"),
        "second card",
    );
    proxy.close_notification(id2);
    let active = proxy.get_active().expect("GetActive must succeed");
    assert!(!active.iter().any(|n| n.id == id2));
    let history = proxy.get_history().expect("GetHistory must succeed");
    assert!(history.iter().any(|n| n.id == id2));
}
