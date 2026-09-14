//! The `logind` bus adapter (`ZbusLogind`, the `Manager`/`Session` proxies,
//! `Inhibit` args, and the `PrepareForSleep`/`Lock`/`Unlock` signal dispatch)
//! exercised against a minimal in-process fake `org.freedesktop.login1`.
//!
//! Production logind is on the **system** bus, which a test must not touch:
//! `suspend`/`power-off` on a developer's or CI machine would take the box
//! down. So each test stands up a private `dbus-daemon`, claims
//! `org.freedesktop.login1` on it with a `#[zbus::interface]` mock, and points
//! `ZbusLogind` at that bus through its existing `with_connection` seam (and
//! [`run_signal_loop_on`] for the signal path). The production
//! `ZbusLogind::connect` / `run_signal_loop` still use the real system bus.
//!
//! Tests skip visibly (a greppable `LEXSKIP:` marker), the same posture
//! `settings/tests/interaction.rs` takes, when no `dbus-daemon` binary exists
//! — **CI must provide `dbus-daemon` for these to run**.

use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_session::logind::{
    INHIBIT_WHO, InhibitMode, Logind, LogindEvents, SharedLogindEvents, ZbusLogind,
    run_signal_loop_on,
};
use zbus::zvariant::{OwnedFd as ZOwnedFd, OwnedObjectPath};

/// How long a freshly spawned `dbus-daemon` gets to print its address.
const BUS_BOOT: Duration = Duration::from_secs(5);

const LOGIN1_NAME: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/c1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";
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

/// Everything the mock observed.
#[derive(Default)]
struct Recorded {
    calls: Mutex<Vec<String>>,
    inhibit: Mutex<Vec<(String, String, String)>>,
    interactive: Mutex<Vec<bool>>,
    locks: AtomicUsize,
    unlocks: AtomicUsize,
}

/// `org.freedesktop.login1.Manager` at [`MANAGER_PATH`].
struct ManagerIface {
    recorded: Arc<Recorded>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl ManagerIface {
    fn get_session(&self, session_id: &str) -> OwnedObjectPath {
        self.recorded
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("GetSession({session_id})"));
        OwnedObjectPath::try_from(SESSION_PATH).expect("a valid object path")
    }

    // logind's member is `GetSessionByPID`, not the PascalCase
    // `GetSessionByPid` the macro would generate.
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> OwnedObjectPath {
        self.recorded
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("GetSessionByPID({pid})"));
        OwnedObjectPath::try_from(SESSION_PATH).expect("a valid object path")
    }

    fn inhibit(&self, what: &str, who: &str, _why: &str, mode: &str) -> ZOwnedFd {
        self.recorded
            .inhibit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((what.to_string(), who.to_string(), mode.to_string()));
        let (read, write) = std::io::pipe().expect("a pipe");
        drop(write);
        let read: OwnedFd = read.into();
        read.into()
    }

    fn suspend(&self, interactive: bool) {
        self.record("suspend", interactive);
    }

    fn hibernate(&self, interactive: bool) {
        self.record("hibernate", interactive);
    }

    fn power_off(&self, interactive: bool) {
        self.record("power_off", interactive);
    }

    fn reboot(&self, interactive: bool) {
        self.record("reboot", interactive);
    }
}

impl ManagerIface {
    fn record(&self, action: &str, interactive: bool) {
        self.recorded
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{action}({interactive})"));
        self.recorded
            .interactive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(interactive);
    }
}

/// `org.freedesktop.login1.Session` at [`SESSION_PATH`].
struct SessionIface {
    recorded: Arc<Recorded>,
}

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl SessionIface {
    fn lock(&self) {
        self.recorded.locks.fetch_add(1, Ordering::SeqCst);
    }

    fn unlock(&self) {
        self.recorded.unlocks.fetch_add(1, Ordering::SeqCst);
    }
}

/// The fake login1 service on its own private bus.
struct FakeLogind {
    bus: PrivateBus,
    recorded: Arc<Recorded>,
    service: zbus::blocking::Connection,
}

impl FakeLogind {
    /// `Err(NO_BUS)` when no `dbus-daemon` can be started (the visible-skip
    /// case); any other `Err` is a real setup failure the test must surface.
    fn spawn() -> Result<FakeLogind, String> {
        let bus = PrivateBus::spawn().ok_or_else(|| NO_BUS.to_string())?;
        let recorded = Arc::new(Recorded::default());
        let service = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .map_err(|e| format!("address: {e}"))?
            .name(LOGIN1_NAME)
            .map_err(|e| format!("claim {LOGIN1_NAME}: {e}"))?
            .serve_at(
                MANAGER_PATH,
                ManagerIface {
                    recorded: Arc::clone(&recorded),
                },
            )
            .map_err(|e| format!("serve Manager: {e}"))?
            .serve_at(
                SESSION_PATH,
                SessionIface {
                    recorded: Arc::clone(&recorded),
                },
            )
            .map_err(|e| format!("serve Session: {e}"))?
            .build()
            .map_err(|e| format!("build service: {e}"))?;
        Ok(FakeLogind {
            bus,
            recorded,
            service,
        })
    }

    /// A `ZbusLogind` whose discovery and calls go to this fake.
    fn logind(&self, xdg_session_id: Option<&str>, pid: u32) -> ZbusLogind {
        let conn = zbus::blocking::connection::Builder::address(self.bus.address.as_str())
            .expect("connect to the private bus")
            .build()
            .expect("build the client connection");
        ZbusLogind::with_connection(conn, xdg_session_id, pid).expect("resolve the fake session")
    }

    fn clear_calls(&self) {
        self.recorded
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn calls(&self) -> Vec<String> {
        self.recorded
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn emit_prepare_for_sleep(&self, going_to_sleep: bool) {
        self.service
            .emit_signal(
                None::<&str>,
                MANAGER_PATH,
                MANAGER_IFACE,
                "PrepareForSleep",
                &going_to_sleep,
            )
            .expect("emit PrepareForSleep");
    }

    fn emit_session_lock(&self) {
        self.service
            .emit_signal(None::<&str>, SESSION_PATH, SESSION_IFACE, "Lock", &())
            .expect("emit Lock");
    }

    fn emit_session_unlock(&self) {
        self.service
            .emit_signal(None::<&str>, SESSION_PATH, SESSION_IFACE, "Unlock", &())
            .expect("emit Unlock");
    }
}

/// The signal loop's sink: records what the daemon would act on.
#[derive(Default)]
struct RecordingEvents {
    sleeps: Mutex<Vec<bool>>,
    locks: AtomicUsize,
    unlocks: AtomicUsize,
}

impl LogindEvents for RecordingEvents {
    fn prepare_for_sleep(&self, going_to_sleep: bool) {
        self.sleeps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(going_to_sleep);
    }

    fn lock(&self) {
        self.locks.fetch_add(1, Ordering::SeqCst);
    }

    fn unlock(&self) {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
    }
}

fn fake_logind(test: &str) -> Option<FakeLogind> {
    match FakeLogind::spawn() {
        Ok(fake) => Some(fake),
        Err(err) if err == NO_BUS => {
            eprintln!("LEXSKIP: {test} skipped — {NO_BUS}");
            None
        }
        Err(err) => panic!("fake logind setup failed: {err}"),
    }
}

/// Success criterion 1: both discovery paths. With `$XDG_SESSION_ID` set the
/// hint is cross-checked against `GetSessionByPID`; with no id the pid lookup
/// is used directly.
#[test]
fn discovery_uses_the_xdg_session_id_and_the_pid_fallback() {
    let Some(fake) = fake_logind("discovery_uses_the_xdg_session_id_and_the_pid_fallback") else {
        return;
    };

    let hinted = fake.logind(Some("c1"), 4242);
    assert_eq!(hinted.session_path().as_str(), SESSION_PATH);
    let calls = fake.calls();
    assert!(calls.contains(&"GetSession(c1)".to_string()), "{calls:?}");
    assert!(
        calls.contains(&"GetSessionByPID(4242)".to_string()),
        "the hint must be cross-checked: {calls:?}"
    );

    fake.clear_calls();
    let by_pid = fake.logind(None, 4242);
    assert_eq!(by_pid.session_path().as_str(), SESSION_PATH);
    let calls = fake.calls();
    assert_eq!(calls, vec!["GetSessionByPID(4242)".to_string()]);
}

/// `Inhibit` reaches logind with the reviewed `who`, `what`, and `delay` mode.
#[test]
fn inhibit_sends_the_reviewed_who_what_and_mode() {
    let Some(fake) = fake_logind("inhibit_sends_the_reviewed_who_what_and_mode") else {
        return;
    };
    let logind = fake.logind(None, 4242);
    let _fd = logind
        .inhibit("sleep", "lock screen before suspend", InhibitMode::Delay)
        .expect("inhibit returns an fd");

    let seen = fake
        .recorded
        .inhibit
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(
        seen,
        vec![(
            "sleep".to_string(),
            INHIBIT_WHO.to_string(),
            "delay".to_string()
        )]
    );
    assert_eq!(INHIBIT_WHO, "icedtea-session");
}

/// All four power actions reach logind with `interactive == false`: a change
/// to `true` compiles but would make logind prompt polkit, so the wire is
/// pinned here.
#[test]
fn power_actions_are_non_interactive_on_the_wire() {
    let Some(fake) = fake_logind("power_actions_are_non_interactive_on_the_wire") else {
        return;
    };
    let logind = fake.logind(None, 4242);
    logind.suspend().expect("suspend");
    logind.hibernate().expect("hibernate");
    logind.power_off().expect("power off");
    logind.reboot().expect("reboot");

    let seen = fake
        .recorded
        .interactive
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(
        seen,
        vec![false; 4],
        "power actions must reach logind non-interactively"
    );
}

/// `Session.Lock`/`Session.Unlock` reach the session object.
#[test]
fn lock_and_unlock_reach_the_session_object() {
    let Some(fake) = fake_logind("lock_and_unlock_reach_the_session_object") else {
        return;
    };
    let logind = fake.logind(None, 4242);
    logind.lock().expect("lock");
    logind.unlock().expect("unlock");

    assert_eq!(fake.recorded.locks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.recorded.unlocks.load(Ordering::SeqCst), 1);
}

/// Real `PrepareForSleep`/`Lock`/`Unlock` signals emitted by (fake) logind are
/// classified and delivered to the events sink — the one path the pure
/// `classify_signal` unit test cannot cover.
#[test]
fn logind_signals_reach_the_events_sink() {
    let Some(fake) = fake_logind("logind_signals_reach_the_events_sink") else {
        return;
    };

    let sink = Arc::new(RecordingEvents::default());
    let events: SharedLogindEvents = sink.clone();
    let path = OwnedObjectPath::try_from(SESSION_PATH).expect("a valid object path");
    let conn = zbus::block_on(async {
        zbus::connection::Builder::address(fake.bus.address.as_str())?
            .build()
            .await
    })
    .expect("client connection to the private bus");
    let _loop_thread = std::thread::spawn(move || {
        zbus::block_on(async move {
            let _ = run_signal_loop_on(conn, &path, events).await;
        });
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        fake.emit_prepare_for_sleep(true);
        fake.emit_session_lock();
        fake.emit_session_unlock();
        if sink
            .sleeps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&true)
            && sink.locks.load(Ordering::SeqCst) > 0
            && sink.unlocks.load(Ordering::SeqCst) > 0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "logind signals never reached the events sink"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
