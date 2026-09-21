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
            // SKIP only for environmental conditions (no bus, name taken by
            // a real daemon). Any other spawn failure is a service bug and
            // must fail — and CI sets NOTIF_LIVE_MUST_RUN=1 so even an
            // environmental skip fails loudly instead of passing vacuously.
            let must_run = std::env::var("NOTIF_LIVE_MUST_RUN").is_ok();
            let no_bus = format!("{err:?}").contains("No such file")
                || format!("{err:?}").contains("Connection refused");
            let environmental = matches!(
                err,
                icedtea_notifications::service::SpawnError::NameTaken(_)
            ) || no_bus;
            if must_run || !environmental {
                panic!("live notification service failed to spawn: {err:?}");
            }
            eprintln!(
                "SKIP: cannot register {NOTIF_BUS_NAME} ({err:?}) -- bus down or a daemon is running"
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

/// The `None`-on-failure arms the trait contract depends on ("unreachable,
/// retain last-known — never reconcile against a phantom empty set"),
/// proven on a private bus where the name is unowned: the bus daemon itself
/// answers `NameHasNoOwner`, so every read must be `None`, never a collapsed
/// default. Hermetic — never touches (or steals) the session-bus name, so it
/// runs everywhere `dbus-daemon` exists and cannot interfere with the live
/// round-trip test above.
#[test]
fn proxy_reads_return_none_when_the_name_is_unowned() {
    use std::io::BufRead as _;

    let must_run = std::env::var("NOTIF_LIVE_MUST_RUN").is_ok();
    let mut bus = match std::process::Command::new("dbus-daemon")
        .args(["--session", "--print-address=1", "--nofork"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(bus) => bus,
        Err(err) => {
            if must_run {
                panic!("NOTIF_LIVE_MUST_RUN=1 but dbus-daemon is unavailable: {err:?}");
            }
            eprintln!("SKIP: dbus-daemon unavailable ({err:?})");
            return;
        }
    };
    let address = {
        let stdout = bus.stdout.take().expect("piped stdout");
        std::io::BufReader::new(stdout)
            .lines()
            .next()
            .expect("bus prints an address")
            .expect("address line reads")
    };
    let conn = zbus::blocking::connection::Builder::address(address.as_str())
        .expect("private bus address parses")
        .build()
        .expect("private bus connects");
    let proxy = NotifierProxy::from_connection(conn);
    let (active, history, dnd) = (proxy.get_active(), proxy.get_history(), proxy.get_dnd());
    let _ = bus.kill();
    assert_eq!(active, None);
    assert_eq!(history, None);
    assert_eq!(dnd, None);
}
