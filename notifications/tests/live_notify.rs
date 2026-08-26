//! A live D-Bus round-trip against the real `icedtea-notifications` binary
//! spawned as a subprocess on the real session bus -- the same posture as
//! `settings/tests/live_apply.rs` (spawns `icedtea-compositor`, not an
//! in-process harness) and `shell/tests/live_dbus.rs` (proves the real wire
//! members, not a mock). This is milestone N3's integration test: it drives
//! `Notify`/`CloseNotification`/`GetCapabilities`/`GetServerInformation`
//! (`org.freedesktop.Notifications`) and `GetActive`/`SetDoNotDisturb`/
//! `InvokeAction` (`org.icedtea.Notifications`) over a real `zbus`
//! connection and asserts on the real `NotificationClosed`/`ActionInvoked`
//! signals -- the coverage no unit test can give, since `service.rs`'s
//! `#[interface]` wiring (member names, signal shapes, bus-name-conflict
//! exit) is otherwise only checked by hand with `busctl`.
//!
//! Skipped (visibly) only when the session bus is unavailable or
//! `org.freedesktop.Notifications` is already owned (a real daemon --
//! mako, dunst, a stray previous instance, or another test run -- is
//! running).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use icedtea_contract::{NOTIF_BUS_NAME, NOTIF_PATH};
use zbus::zvariant::Value;

const ICEDTEA_IFACE: &str = "org.icedtea.Notifications";
/// How long to wait for the subprocess daemon to boot and claim the bus name.
const BOOT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for a signal that should fire promptly (well above any
/// scheduling jitter, well below "the daemon is actually stuck").
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Kills the child daemon process when dropped -- including on a test
/// panic, via unwind -- so a failing assertion never leaks a process holding
/// `org.freedesktop.Notifications` into the next test run.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The directory this test binary itself was built into
/// (`target/<profile>/deps/`'s parent), which is also where cargo places
/// every workspace binary.
fn target_profile_dir() -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop(); // deps/
    path.pop(); // debug/ or release/
    path
}

/// Path to the `icedtea-notifications` binary, building it first if this
/// profile directory doesn't have one yet.
fn daemon_binary() -> PathBuf {
    let bin = target_profile_dir().join("icedtea-notifications");
    if !bin.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let status = Command::new(cargo)
            .args(["build", "-p", "icedtea-notifications"])
            .status()
            .expect("failed to invoke cargo to build icedtea-notifications");
        assert!(
            status.success(),
            "cargo build -p icedtea-notifications failed"
        );
    }
    bin
}

/// Poll `org.freedesktop.DBus`'s `NameHasOwner` until `name` is owned or
/// `timeout` elapses.
fn wait_for_name_owner(conn: &zbus::blocking::Connection, name: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let owned = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(name,),
            )
            .ok()
            .and_then(|reply| reply.body().deserialize::<bool>().ok())
            .unwrap_or(false);
        if owned {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Subscribe to a signal member on the notifications object and forward every
/// deserialized body onto a channel from a dedicated background thread --
/// lets the test `recv_timeout` for a specific signal without racing message
/// delivery against when it happens to start listening.
fn subscribe<T>(iface: &str, member: &'static str) -> mpsc::Receiver<T>
where
    T: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type + Send + 'static,
{
    let conn = zbus::blocking::Connection::session().expect("session bus for signal subscription");
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(iface)
        .expect("interface")
        .path(NOTIF_PATH)
        .expect("path")
        .member(member)
        .expect("member")
        .build();
    let iter =
        zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None).expect("subscribe");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for msg in iter.flatten() {
            if let Ok(body) = msg.body().deserialize::<T>() {
                let _ = tx.send(body);
            }
        }
    });
    rx
}

fn call_std<A, R>(conn: &zbus::blocking::Connection, member: &str, args: &A) -> R
where
    A: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type,
{
    conn.call_method(
        Some(NOTIF_BUS_NAME),
        NOTIF_PATH,
        Some(NOTIF_BUS_NAME),
        member,
        args,
    )
    .unwrap_or_else(|err| panic!("{member} call failed: {err}"))
    .body()
    .deserialize()
    .unwrap_or_else(|err| panic!("{member} reply body did not deserialize: {err}"))
}

fn call_icedtea<A, R>(conn: &zbus::blocking::Connection, member: &str, args: &A) -> R
where
    A: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type,
{
    conn.call_method(
        Some(NOTIF_BUS_NAME),
        NOTIF_PATH,
        Some(ICEDTEA_IFACE),
        member,
        args,
    )
    .unwrap_or_else(|err| panic!("{member} call failed: {err}"))
    .body()
    .deserialize()
    .unwrap_or_else(|err| panic!("{member} reply body did not deserialize: {err}"))
}

#[allow(clippy::too_many_arguments)]
fn notify(
    conn: &zbus::blocking::Connection,
    app_name: &str,
    replaces_id: u32,
    actions: Vec<String>,
    urgency: u8,
    expire_timeout: i32,
) -> u32 {
    let mut hints: HashMap<String, Value> = HashMap::new();
    hints.insert("urgency".into(), Value::U8(urgency));
    call_std(
        conn,
        "Notify",
        &(
            app_name,
            replaces_id,
            "",
            "summary",
            "body",
            actions,
            hints,
            expire_timeout,
        ),
    )
}

/// `Notify` with an explicit `resident` hint (and a never-expire timeout, so
/// only an action or DND — not a timer — can affect it).
fn notify_resident(
    conn: &zbus::blocking::Connection,
    app_name: &str,
    actions: Vec<String>,
    resident: bool,
) -> u32 {
    let mut hints: HashMap<String, Value> = HashMap::new();
    hints.insert("urgency".into(), Value::U8(1));
    hints.insert("resident".into(), Value::Bool(resident));
    call_std(
        conn,
        "Notify",
        &(app_name, 0u32, "", "summary", "body", actions, hints, 0i32),
    )
}

#[test]
fn full_notify_lifecycle_over_the_real_bus() {
    let probe = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("SKIP: no session bus available ({err})");
            return;
        }
    };
    if wait_for_name_owner(&probe, NOTIF_BUS_NAME, Duration::from_millis(1)) {
        eprintln!(
            "SKIP: {NOTIF_BUS_NAME} is already owned -- a real notification daemon is running"
        );
        return;
    }

    let child = Command::new(daemon_binary())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn icedtea-notifications");
    let _guard = ChildGuard(child);

    assert!(
        wait_for_name_owner(&probe, NOTIF_BUS_NAME, BOOT_TIMEOUT),
        "icedtea-notifications never registered {NOTIF_BUS_NAME} within {BOOT_TIMEOUT:?}"
    );

    // Subscribe before triggering anything, so no signal can race ahead of
    // the listener.
    let closed_rx: mpsc::Receiver<(u32, u32)> = subscribe(NOTIF_BUS_NAME, "NotificationClosed");
    let action_rx: mpsc::Receiver<(u32, String)> = subscribe(NOTIF_BUS_NAME, "ActionInvoked");
    let dnd_rx: mpsc::Receiver<(bool,)> = subscribe(ICEDTEA_IFACE, "DoNotDisturbChanged");

    let conn = zbus::blocking::Connection::session().expect("session bus");

    // --- GetCapabilities / GetServerInformation literals ---
    let caps: Vec<String> = call_std(&conn, "GetCapabilities", &());
    for expected in [
        "actions",
        "body",
        "body-markup",
        "icon-static",
        "persistence",
    ] {
        assert!(
            caps.contains(&expected.to_string()),
            "GetCapabilities missing {expected:?}: {caps:?}"
        );
    }
    let info: (String, String, String, String) = call_std(&conn, "GetServerInformation", &());
    assert_eq!(info.0, "icedtea-notifications");
    assert_eq!(info.1, "icedtea");
    assert_eq!(info.3, "1.2");

    // --- Notify -> nonzero id present in GetActive ---
    let id = notify(&conn, "app-one", 0, vec![], 1, -1);
    assert_ne!(id, 0, "Notify must return a nonzero id");
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        active.iter().any(|n| n.id == id),
        "fresh notification must appear in GetActive: {active:?}"
    );

    // --- CloseNotification -> NotificationClosed(id, 3) + leaves GetActive ---
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(NOTIF_BUS_NAME),
            "CloseNotification",
            &(id,),
        )
        .expect("CloseNotification call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let (closed_id, reason) = closed_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("NotificationClosed never fired for CloseNotification");
    assert_eq!(closed_id, id);
    assert_eq!(
        reason, 3,
        "CloseNotification's reason code must be 3 (ClosedByRequest)"
    );
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        !active.iter().any(|n| n.id == id),
        "closed notification must leave GetActive"
    );

    // --- Notify expire_timeout=50 -> NotificationClosed(id, 1) within a short deadline ---
    let expiring_id = notify(&conn, "app-two", 0, vec![], 1, 50);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = None;
    while Instant::now() < deadline {
        if let Ok((closed_id, reason)) = closed_rx.recv_timeout(Duration::from_millis(200))
            && closed_id == expiring_id
        {
            found = Some(reason);
            break;
        }
    }
    assert_eq!(
        found,
        Some(1),
        "expire_timeout=50 must self-close with reason 1 (Expired) promptly"
    );

    // --- SetDoNotDisturb(true): Normal absent, Critical present ---
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "SetDoNotDisturb",
            &(true,),
        )
        .expect("SetDoNotDisturb(true) call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let (on,) = dnd_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("DoNotDisturbChanged never fired for SetDoNotDisturb(true)");
    assert!(on, "DoNotDisturbChanged must report true");
    assert!(
        call_icedtea::<_, bool>(&conn, "GetDoNotDisturb", &()),
        "GetDoNotDisturb must report true"
    );

    let normal_id = notify(&conn, "app-three", 0, vec![], 1, 0);
    let critical_id = notify(&conn, "app-four", 0, vec![], 2, 0);
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        !active.iter().any(|n| n.id == normal_id),
        "Normal urgency must be DND-suppressed from GetActive"
    );
    assert!(
        active.iter().any(|n| n.id == critical_id),
        "Critical urgency must still surface under DND"
    );

    // --- toggle DND off -> Normal reappears ---
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "SetDoNotDisturb",
            &(false,),
        )
        .expect("SetDoNotDisturb(false) call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let (on,) = dnd_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("DoNotDisturbChanged never fired for SetDoNotDisturb(false)");
    assert!(!on, "DoNotDisturbChanged must report false");
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        active.iter().any(|n| n.id == normal_id),
        "Normal urgency must reappear once DND is off, same id"
    );

    // --- register an action + InvokeAction -> ActionInvoked(id, key) ---
    let action_id = notify(
        &conn,
        "app-five",
        0,
        vec!["default".into(), "Open".into()],
        1,
        0,
    );
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "InvokeAction",
            &(action_id, "default"),
        )
        .expect("InvokeAction call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let (invoked_id, key) = action_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("ActionInvoked never fired");
    assert_eq!(invoked_id, action_id);
    assert_eq!(key, "default");

    // --- #3 resident hint: a non-resident notification is removed once an
    // action is invoked; a resident one stays. ---
    let non_resident = notify_resident(
        &conn,
        "app-nonres",
        vec!["default".into(), "Open".into()],
        false,
    );
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "InvokeAction",
            &(non_resident, "default"),
        )
        .expect("InvokeAction call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    // The action fires, then the non-resident notification auto-closes as Dismissed (2).
    let (_, _) = action_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("ActionInvoked (non-resident) never fired");
    let closed = wait_for_closed(&closed_rx, non_resident, SIGNAL_TIMEOUT);
    assert_eq!(
        closed,
        Some(2),
        "non-resident notification must auto-close (Dismissed=2) after its action"
    );
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        !active.iter().any(|n| n.id == non_resident),
        "non-resident must leave GetActive after its action"
    );

    let resident = notify_resident(
        &conn,
        "app-res",
        vec!["default".into(), "Open".into()],
        true,
    );
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "InvokeAction",
            &(resident, "default"),
        )
        .expect("InvokeAction call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let (_, _) = action_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("ActionInvoked (resident) never fired");
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        active.iter().any(|n| n.id == resident),
        "resident notification must remain in GetActive after its action"
    );

    // --- #4 DND<->expiry: a Normal notification with a short timeout posted
    // under DND must not silently expire while hidden; it reappears (fresh
    // countdown) when DND lifts. ---
    let added_rx: mpsc::Receiver<(u32,)> = subscribe(ICEDTEA_IFACE, "NotificationAdded");
    set_dnd(&conn, true);
    let short = notify(&conn, "app-dnd-short", 0, vec![], 1, 150);
    // #6: a suppressed notification is NOT announced as Added.
    assert!(
        !drain_added(&added_rx, short, Duration::from_millis(400)),
        "a DND-suppressed notification must not emit NotificationAdded"
    );
    // Well past its 150ms timeout: it must still be alive (not expired while hidden).
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        !active.iter().any(|n| n.id == short),
        "short notification stays suppressed under DND"
    );
    set_dnd(&conn, false);
    // #6: becoming visible emits NotificationAdded.
    assert!(
        drain_added(&added_rx, short, SIGNAL_TIMEOUT),
        "un-suppressing must emit NotificationAdded"
    );
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        active.iter().any(|n| n.id == short),
        "notification posted under DND with a short timeout must reappear (fresh countdown) once DND lifts, not be silently gone"
    );
    // And with its fresh countdown it eventually expires on its own.
    let closed = wait_for_closed(&closed_rx, short, Duration::from_secs(3));
    assert_eq!(
        closed,
        Some(1),
        "after DND lifts, the fresh countdown eventually expires it (Expired=1)"
    );

    // --- #2 DismissAll clears DND-suppressed notifications too. ---
    set_dnd(&conn, true);
    let hidden = notify(&conn, "app-dnd-hidden", 0, vec![], 1, 0);
    let active: Vec<icedtea_contract::Notification> = call_icedtea(&conn, "GetActive", &());
    assert!(
        !active.iter().any(|n| n.id == hidden),
        "hidden under DND, absent from GetActive"
    );
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "DismissAll",
            &(),
        )
        .expect("DismissAll call failed")
        .body()
        .deserialize()
        .unwrap_or(());
    let closed = wait_for_closed(&closed_rx, hidden, SIGNAL_TIMEOUT);
    assert_eq!(
        closed,
        Some(2),
        "DismissAll must dismiss (2) even a DND-suppressed notification"
    );
    set_dnd(&conn, false);
}

/// Call `SetDoNotDisturb(on)` and block until it returns.
fn set_dnd(conn: &zbus::blocking::Connection, on: bool) {
    let () = conn
        .call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            "SetDoNotDisturb",
            &(on,),
        )
        .expect("SetDoNotDisturb call failed")
        .body()
        .deserialize()
        .unwrap_or(());
}

/// Drain `NotificationClosed` signals up to `timeout`, returning the reason
/// code for `id` if it closes within the window (ignoring unrelated ids).
fn wait_for_closed(rx: &mpsc::Receiver<(u32, u32)>, id: u32, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok((closed_id, reason)) = rx.recv_timeout(Duration::from_millis(100))
            && closed_id == id
        {
            return Some(reason);
        }
    }
    None
}

/// Watch `NotificationAdded` for `window`, returning whether `id` was
/// announced (ignoring unrelated ids).
fn drain_added(rx: &mpsc::Receiver<(u32,)>, id: u32, window: Duration) -> bool {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        if let Ok((added_id,)) = rx.recv_timeout(Duration::from_millis(100))
            && added_id == id
        {
            return true;
        }
    }
    false
}
