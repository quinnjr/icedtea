//! The `org.icedtea.Compositor` client's lock-change subscription
//! (`ZbusWmClient::subscribe_lock_changed` → `run_lock_changed_loop_on`),
//! exercised against a minimal in-process fake compositor on a private
//! `dbus-daemon`.
//!
//! The production subscription rides the **session** bus, which a test must
//! not touch (a real compositor or a sibling test's bus is on it). Each test
//! here stands up its own `dbus-daemon`, serves a `#[zbus::interface]` stub at
//! `COMPOSITOR_PATH` that claims `org.icedtea.Compositor`, and points a
//! `ZbusWmClient` at it through `ZbusWmClient::with_connection`.
//!
//! Tests skip visibly (a greppable `LEXSKIP:` marker, the same posture
//! `fake_logind.rs` takes) when no `dbus-daemon` binary exists — **CI must
//! provide `dbus-daemon` for these to run**.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_contract::{
    COMPOSITOR_BUS_NAME, COMPOSITOR_CONTRACT_VERSION, COMPOSITOR_IFACE, COMPOSITOR_PATH,
};
use icedtea_session::wm_client::{LockState, WmClient, ZbusWmClient, version_mismatch};

/// How long a freshly spawned `dbus-daemon` gets to print its address.
const BUS_BOOT: Duration = Duration::from_secs(5);
const NO_BUS: &str = "no dbus-daemon (could not start a private session bus)";

/// A private session-bus instance this test owns outright, killed on drop.
struct PrivateBus {
    address: String,
    child: Child,
}

impl PrivateBus {
    fn spawn() -> Option<PrivateBus> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let started = Instant::now();
        if reader.read_line(&mut line).ok()? == 0 || started.elapsed() > BUS_BOOT {
            let _ = child.kill();
            return None;
        }
        let address = line.trim().to_string();
        if address.is_empty() {
            let _ = child.kill();
            return None;
        }
        Some(PrivateBus { address, child })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A stub `org.icedtea.Compositor`: a `Version` property and an `IsLocked`
/// method, enough for the version check and the "skew must not fake an
/// unlock" assertion. The `SessionLockChanged` signals are emitted directly on
/// the service connection.
struct CompositorStub {
    version: u32,
    locked: bool,
}

#[zbus::interface(name = "org.icedtea.Compositor")]
impl CompositorStub {
    fn is_locked(&self) -> bool {
        self.locked
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        self.version
    }
}

/// The fake compositor on its own private bus.
struct FakeCompositor {
    bus: PrivateBus,
    service: zbus::blocking::Connection,
}

impl FakeCompositor {
    /// `Err(NO_BUS)` when no `dbus-daemon` can be started (the visible-skip
    /// case); any other `Err` is a real setup failure the test must surface.
    fn spawn(version: u32, locked: bool) -> Result<FakeCompositor, String> {
        let bus = PrivateBus::spawn().ok_or_else(|| NO_BUS.to_string())?;
        let service = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .map_err(|e| format!("address: {e}"))?
            .name(COMPOSITOR_BUS_NAME)
            .map_err(|e| format!("claim {COMPOSITOR_BUS_NAME}: {e}"))?
            .serve_at(COMPOSITOR_PATH, CompositorStub { version, locked })
            .map_err(|e| format!("serve Compositor: {e}"))?
            .build()
            .map_err(|e| format!("build service: {e}"))?;
        Ok(FakeCompositor { bus, service })
    }

    /// A `ZbusWmClient` pointed at this private bus. `with_connection` reads
    /// the stub's `Version`, so the stub must be served before this is called.
    fn client(&self) -> ZbusWmClient {
        let conn = zbus::blocking::connection::Builder::address(self.bus.address.as_str())
            .expect("connect to the private bus")
            .build()
            .expect("build the client connection");
        ZbusWmClient::with_connection(conn)
    }

    fn emit_lock_changed(&self, seq: u64, locked: bool) {
        self.service
            .emit_signal(
                None::<&str>,
                COMPOSITOR_PATH,
                COMPOSITOR_IFACE,
                "SessionLockChanged",
                &(seq, locked),
            )
            .expect("emit SessionLockChanged");
    }

    /// A body that is not `(seq, locked)`: the subscription must drop it
    /// without panicking.
    fn emit_wrong_arity(&self, seq: u64) {
        self.service
            .emit_signal(
                None::<&str>,
                COMPOSITOR_PATH,
                COMPOSITOR_IFACE,
                "SessionLockChanged",
                &(seq,),
            )
            .expect("emit malformed SessionLockChanged");
    }
}

fn fake_compositor(test: &str, version: u32, locked: bool) -> Option<FakeCompositor> {
    match FakeCompositor::spawn(version, locked) {
        Ok(fake) => Some(fake),
        Err(err) if err == NO_BUS => {
            eprintln!("LEXSKIP: {test} skipped — {NO_BUS}");
            None
        }
        Err(err) => panic!("fake compositor setup failed: {err}"),
    }
}

/// Keep emitting `emit()` until `pred` accepts the callback log or the
/// deadline passes. The subscription's async `AddMatch` races the first
/// emission, so a single blind emit would be flaky.
fn wait_until(
    seen: &Arc<Mutex<Vec<bool>>>,
    pred: impl Fn(&[bool]) -> bool,
    mut emit: impl FnMut(),
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if pred(&snapshot) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out; saw {snapshot:?}");
        emit();
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A retained subscriber records the compositor's changes in order, and a
/// wrong-arity body is ignored rather than panicking the loop thread.
#[test]
fn lock_changed_signals_reach_the_callback_in_order_and_bad_bodies_are_ignored() {
    let Some(fake) = fake_compositor(
        "lock_changed_signals_reach_the_callback_in_order_and_bad_bodies_are_ignored",
        COMPOSITOR_CONTRACT_VERSION,
        false,
    ) else {
        return;
    };
    let wm = fake.client();

    let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
    let sink = Arc::clone(&seen);
    let _handle = wm
        .subscribe_lock_changed(Box::new(move |locked| {
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(locked);
        }))
        .expect("the client supports subscriptions");

    wait_until(
        &seen,
        |v| v.contains(&true),
        || fake.emit_lock_changed(1, true),
    );
    wait_until(
        &seen,
        |v| v.last() == Some(&false),
        || fake.emit_lock_changed(2, false),
    );

    // A malformed body from the real sender is delivered to the loop but must
    // be dropped without a panic.
    fake.emit_wrong_arity(3);
    std::thread::sleep(Duration::from_millis(200));

    let log = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let deduped = log
        .iter()
        .copied()
        .fold(Vec::<bool>::new(), |mut acc, locked| {
            if acc.last() != Some(&locked) {
                acc.push(locked);
            }
            acc
        });
    assert_eq!(deduped, vec![true, false], "callback saw {log:?}");
}

/// A same-user peer that does not own `org.icedtea.Compositor` cannot forge an
/// unlock: the name-pinned match rule (and the loop's own owner check) drop
/// it, while the real compositor's signal still gets through.
#[test]
fn a_forged_lock_changed_from_another_peer_is_ignored() {
    let Some(fake) = fake_compositor(
        "a_forged_lock_changed_from_another_peer_is_ignored",
        COMPOSITOR_CONTRACT_VERSION,
        false,
    ) else {
        return;
    };
    let wm = fake.client();

    let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
    let sink = Arc::clone(&seen);
    let _handle = wm
        .subscribe_lock_changed(Box::new(move |locked| {
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(locked);
        }))
        .expect("the client supports subscriptions");

    // Establish that the subscription is live with a legitimate locked=true.
    wait_until(
        &seen,
        |v| v.contains(&true),
        || fake.emit_lock_changed(1, true),
    );

    // A peer on the same bus, owning no compositor name, emits the exact
    // interface/path/member with locked=false. It must not reach the callback.
    let forger = zbus::blocking::connection::Builder::address(fake.bus.address.as_str())
        .expect("connect the forger")
        .build()
        .expect("build the forger connection");
    forger
        .emit_signal(
            None::<&str>,
            COMPOSITOR_PATH,
            COMPOSITOR_IFACE,
            "SessionLockChanged",
            &(2u64, false),
        )
        .expect("forge SessionLockChanged");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&false),
        "a forged unlock must be ignored"
    );

    // The real compositor's unlocked signal is still delivered.
    wait_until(
        &seen,
        |v| v.last() == Some(&false),
        || fake.emit_lock_changed(3, false),
    );
}

/// A compositor advertising an older contract revision is flagged, and the
/// skew does not silently downgrade a real `IsLocked` answer to unlocked.
#[test]
fn an_older_contract_version_is_flagged_without_faking_an_unlock() {
    let Some(fake) = fake_compositor(
        "an_older_contract_version_is_flagged_without_faking_an_unlock",
        6,
        true,
    ) else {
        return;
    };
    let wm = fake.client();

    assert_eq!(wm.read_contract_version(), Some(6));
    let complaint = wm
        .contract_mismatch()
        .expect("a v6 compositor must be flagged");
    assert!(
        complaint.contains("v6") && complaint.contains(&format!("v{COMPOSITOR_CONTRACT_VERSION}")),
        "mismatch should name both revisions: {complaint}"
    );
    assert_eq!(
        wm.lock_state(),
        LockState::Locked,
        "version skew must not turn a real lock into Unlocked"
    );

    // The pure comparison itself, both skew directions and the absent case.
    assert_eq!(
        version_mismatch(
            Some(COMPOSITOR_CONTRACT_VERSION),
            COMPOSITOR_CONTRACT_VERSION
        ),
        None
    );
    assert!(version_mismatch(Some(1), COMPOSITOR_CONTRACT_VERSION).is_some());
    assert!(version_mismatch(None, COMPOSITOR_CONTRACT_VERSION).is_some());
}
