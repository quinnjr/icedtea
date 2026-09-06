//! The cross-page interaction gate M5 contract §2.8 assigns to P4.
//!
//! It drives the real `icedtea-settings` binary under `icedtea_harness`,
//! stands up its own `org.icedtea.Compositor` service and records the
//! `ReloadConfig` calls it receives (plan P4-D13); it skips visibly when the
//! isolated bus cannot be started, the same posture `settings/tests/
//! live_apply.rs` takes with the real compositor.
//!
//! ## Why a private bus, not the developer's session bus
//!
//! The brief this test file was written from (and `zbus`'s worked-example
//! idiom generally) claims `org.icedtea.Compositor` on
//! `zbus::blocking::connection::Builder::session()` — the developer's own,
//! real session bus. `settings/src/compositor_reload.rs`'s `ReloadClient::
//! on_bus` doc (read-only, not edited here) already records why that is
//! wrong for a test: "a unit test that reached the developer's live bus
//! would blocking-call `ReloadConfig` on whatever owns
//! `org.icedtea.Compositor` there — forcing a real compositor to reload
//! mid-`cargo test`", and `settings/src/ipc/reload.rs`'s
//! `spawn_worker_on_bus` doc records the same finding independently
//! ("finding 15"). This test drives the *actual* `icedtea-settings` binary
//! rather than calling `ReloadClient` directly, so it cannot pass a bus
//! address the way those unit tests do — the only knob the production
//! reload worker reads is `$DBUS_SESSION_BUS_ADDRESS`
//! (`ReloadClient::new` -> `zbus::blocking::Connection::session()`). So this
//! test starts its own private `dbus-daemon`, serves the recording mock
//! there, and points the spawned child's `DBUS_SESSION_BUS_ADDRESS` at it
//! (via `SettingsDriver::open_on_bus`) — the developer's real session bus is
//! never touched, and a `ReloadConfig` call has nowhere else to land but the
//! mock.

#[path = "support/displays.rs"]
mod support;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH};
use support::{REACT, SettingsDriver, TEST_THEME};

/// How long a freshly spawned `dbus-daemon` gets to print its address.
const BUS_BOOT: Duration = Duration::from_secs(5);

/// A private session-bus instance this test owns outright, killed on drop.
///
/// Reconciliation: the brief's literal test claims `org.icedtea.Compositor`
/// on the developer's own session bus (see the module docs above for why
/// that is wrong here) and skips only when that bus is unavailable or the
/// name already owned. This type replaces that posture with a bus nothing
/// else is using, so "already owned" cannot happen — the only remaining skip
/// condition is "no `dbus-daemon` binary" / "it never printed an address".
struct PrivateBus {
    address: String,
    child: Child,
}

impl PrivateBus {
    /// Spawn `dbus-daemon --session --nofork --print-address` and read its
    /// first line of stdout as the bus address. `None` if the binary is
    /// missing or never prints one within [`BUS_BOOT`] — the visible-skip
    /// path every gate in this suite that depends on external state takes.
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
        // `read_line` blocks, but a `dbus-daemon` that boots at all writes
        // its address within milliseconds; `BUS_BOOT` only bounds a daemon
        // that never will (the process is still reaped either way).
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

/// The recording stand-in for a running compositor: it answers `ReloadConfig`
/// and counts the calls. Method names are PascalCase on the wire, which is
/// exactly the regression inherited decision 10 exists for.
struct RecordingCompositor {
    calls: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.icedtea.Compositor")]
impl RecordingCompositor {
    fn reload_config(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Contract §2.8: Apply reaches `ReloadConfig`.
///
/// Mutation check: rename `reload_config` below to some other name (e.g.
/// `reload_cfg`) so `zbus::interface`'s default PascalCase conversion no
/// longer produces the `ReloadConfig` member `settings/src/
/// compositor_reload.rs` actually calls; the recorder never sees a call and
/// this fails. Restore. (That file is read-only for P4 — perform the
/// mutation here, in the test's own mock, never there.)
#[test]
fn apply_reaches_reload_config_on_the_mock() {
    let Some(bus) = PrivateBus::spawn() else {
        eprintln!("skipping: could not start a private session bus (no dbus-daemon?)");
        return;
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let _service = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .expect("connect to the private bus")
        .name(COMPOSITOR_BUS_NAME)
        .expect("claim the bus name")
        .serve_at(
            COMPOSITOR_PATH,
            RecordingCompositor {
                calls: Arc::clone(&calls),
            },
        )
        .expect("serve the interface")
        .build()
        .expect("build the service");

    let mut driver = SettingsDriver::open_on_bus(TEST_THEME, "behavior", Some(&bus.address));

    // Dirty the model: toggle a switch, which enables Revert and Apply.
    let before_toggle = driver.frame_count();
    let switch = driver.alloc("behavior_raise_on_focus");
    driver.click(switch.x + switch.w / 2, switch.y + switch.h / 2);

    // Reconciliation: the brief's literal code clicks Apply once, right after
    // the switch, with no wait between the two. `Apply`'s `sensitive` is a
    // *view* property that only flips once the toggle's own repaint has
    // landed (`ui/src/view/app.rs`'s `write_probe_report` marks a fresh paint
    // with a `frame <n>` line); a click sent before that repaint lands on the
    // *previous*, still-disabled tree and reaches nothing. Retrying the send
    // itself (rather than waiting for the repaint first) was tried and
    // rejected: `SettingsDriver::click`'s round trip is slower than the app's
    // own processing of an already-landed click, so a short retry loop
    // fired the *same still-sensitive* Apply more than once — each one a
    // genuine `Msg::Apply` (the model stays dirty until its own async
    // `Cmd::Task` answers with `Msg::Applied`) — and the mock saw more than
    // one `ReloadConfig`. Waiting for the repaint first and then clicking
    // exactly once avoids both failure modes.
    assert!(
        driver.wait_for_frame_after(before_toggle, REACT),
        "the switch's own repaint never landed within {REACT:?}"
    );
    let apply = driver.alloc("apply");
    driver.click(apply.x + apply.w / 2, apply.y + apply.h / 2);

    let deadline = Instant::now() + REACT;
    while Instant::now() < deadline && calls.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "Apply must reach ReloadConfig exactly once"
    );
}
