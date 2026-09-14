//! The `org.icedtea.Session` zbus service.
//!
//! Registered exactly like `clipboard/src/service.rs::spawn`: a
//! `zbus::blocking::Connection::session()`, `object_server().at(...)`, and
//! `request_name(...)` on the session bus. Every method answers from a trait
//! seam ([`Logind`] for lock/power, [`WmClient`] for the compositor's lock
//! state and `Quit`), so the whole surface is unit-testable with
//! `RecordingLogind`/`RecordingWm` and no bus.
//!
//! A method never panics on a foreign (D-Bus dispatch) thread: the logind
//! methods propagate their `io::Error` as a [`zbus::fdo::Error::Failed`] so the
//! caller's `call_method` sees a D-Bus error reply (and the CLI exits
//! non-zero), and the compositor seam already collapses every failure to
//! `false`.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::interface;
use zbus::message::Header;
use zbus::zvariant::OwnedValue;

use crate::logind::Logind;
use crate::wm_client::{LockState, WmClient};

/// How long `Lock()` waits for the compositor to confirm the session actually
/// locked before replying with an error. `Session.Lock` only *sets*
/// `LockedHint` and emits the signal that spawns the locker; without a
/// confirmation a missing/denied `locker_command` would otherwise reply
/// success while the screen stays unlocked (the CLI would exit 0). Generous
/// enough for a healthy compositor to take the lock, short enough not to wedge
/// the caller.
const LOCK_CONFIRM_BUDGET: Duration = Duration::from_secs(2);
/// How often [`SessionInterface::confirm_locked`] re-checks the compositor.
const LOCK_CONFIRM_POLL: Duration = Duration::from_millis(50);

/// Outcome of [`SessionInterface::confirm_locked`]. Distinguishes a compositor
/// that answered "not yet locked" from one that could not be reached at all, so
/// `Lock()` reports the cause it actually observed rather than always blaming a
/// slow locker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockConfirm {
    /// The compositor confirmed the session locked.
    Confirmed,
    /// The compositor answered unlocked for the whole budget.
    TimedOut,
    /// The compositor could not be reached; no amount of polling would help.
    Unreachable,
}

/// Pure same-UID policy for the mutation gate: a caller may drive
/// `Lock`/`Suspend`/`Hibernate`/`PowerOff`/`Reboot`/`LogOut` only when its UID
/// equals this process's own. Extracted so the policy itself (`==`, nothing
/// subtler) is unit-testable without a bus.
///
/// Mirrors `compositor/src/dbus.rs`'s `sender_uid_allowed` deliberately: a
/// sandboxed same-user app reaches the session bus, so a session-bus peer is
/// not automatically trustworthy and the compositor's own gate is the one this
/// daemon's mutating surface has to match.
pub fn sender_uid_allowed(caller_uid: u32, own_uid: u32) -> bool {
    caller_uid == own_uid
}

/// This process's own effective UID, parsed from `/proc/self/status`'s `Uid:`
/// line (real, effective, saved, fs: the second field). Dependency-free, the
/// same source the compositor's gate reads.
fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok())
}

/// Resolves a caller's UID and this process's own UID for the mutation gate.
/// A seam so the fail-closed policy is testable with an injected foreign UID,
/// which a real private bus cannot produce.
trait CallerAuth: Send + Sync {
    /// The UID behind one D-Bus sender unique name, or `None` if it cannot be
    /// resolved (callers treat that as denied).
    fn caller_uid(&self, sender: &str) -> Option<u32>;
    /// This process's own UID, or `None` if it cannot be read (denied).
    fn own_uid(&self) -> Option<u32>;
}

/// The production [`CallerAuth`]: resolves the sender through
/// `org.freedesktop.DBus.GetConnectionCredentials` on the same bus the service
/// is registered on.
///
/// A fresh connection per lookup, deliberately (mirroring the compositor's
/// `caller_uid`): issuing the lookup on the dispatch connection from inside its
/// own method handler risks stalling that connection's executor, while a
/// short-lived second connection only ever blocks this handler thread. These
/// calls are user-initiated and rare, so one extra connection setup each is
/// negligible.
struct BusCallerAuth {
    /// `Some(address)` for an explicitly-addressed bus (tests, `spawn_on_bus`);
    /// `None` follows the ambient session bus (`spawn`).
    address: Option<String>,
}

impl BusCallerAuth {
    /// Resolve callers on the ambient session bus.
    fn ambient() -> Self {
        Self { address: None }
    }

    /// Resolve callers on the bus at `address`.
    fn at(address: &str) -> Self {
        Self {
            address: Some(address.to_string()),
        }
    }

    /// Open a short-lived connection to resolve a caller's credentials.
    fn open(&self) -> zbus::Result<Connection> {
        match &self.address {
            Some(address) => {
                zbus::blocking::connection::Builder::address(address.as_str())?.build()
            }
            None => Connection::session(),
        }
    }
}

impl CallerAuth for BusCallerAuth {
    fn caller_uid(&self, sender: &str) -> Option<u32> {
        let conn = self.open().ok()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionCredentials",
                &(sender,),
            )
            .ok()?;
        let creds: HashMap<String, OwnedValue> = reply.body().deserialize().ok()?;
        creds.get("UnixUserID").and_then(|v| u32::try_from(v).ok())
    }

    fn own_uid(&self) -> Option<u32> {
        current_uid()
    }
}

/// The `org.icedtea.Session` bus name.
pub const SESSION_BUS_NAME: &str = "org.icedtea.Session";
/// The `org.icedtea.Session` object path.
pub const SESSION_PATH: &str = "/org/icedtea/Session";
/// The `org.icedtea.Session` interface name, for clients (the CLI) that call
/// its methods.
pub const SESSION_IFACE: &str = "org.icedtea.Session";

/// A shared [`Logind`] seam.
pub type SharedLogind = Arc<dyn Logind + Send + Sync>;
/// A shared [`WmClient`] seam.
pub type SharedWm = Arc<dyn WmClient + Send + Sync>;

/// The `org.icedtea.Session` interface object.
pub struct SessionInterface {
    logind: SharedLogind,
    wm: SharedWm,
    /// How long [`SessionInterface::lock`] waits for confirmation; a field so
    /// tests can shorten it.
    lock_confirm_budget: Duration,
    /// Resolves the calling sender's UID for the mutation gate.
    auth: Arc<dyn CallerAuth>,
}

impl SessionInterface {
    /// Build the interface from its two trait seams, gating mutations on the
    /// ambient session bus.
    pub fn new(logind: SharedLogind, wm: SharedWm) -> Self {
        Self::with_auth(logind, wm, Arc::new(BusCallerAuth::ambient()))
    }

    /// [`Self::new`] with an injected credential resolver, so the fail-closed
    /// gate can be tested against a foreign UID the private test bus cannot
    /// produce.
    fn with_auth(logind: SharedLogind, wm: SharedWm, auth: Arc<dyn CallerAuth>) -> Self {
        Self {
            logind,
            wm,
            lock_confirm_budget: LOCK_CONFIRM_BUDGET,
            auth,
        }
    }

    /// Fail-closed same-UID gate for every mutating method. `IsLocked` is
    /// deliberately not gated: it is a read-only presence/timing oracle, and
    /// the compositor's own `SessionLockChanged` signal already broadcasts the
    /// same state to any same-user peer.
    fn authorize(&self, header: &Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_sender(header.sender().map(|sender| sender.as_str()))
    }

    /// The gate body, split from [`Self::authorize`] so it is testable without
    /// constructing a message header.
    fn authorize_sender(&self, sender: Option<&str>) -> zbus::fdo::Result<()> {
        let allowed = match sender {
            Some(sender) => match (self.auth.caller_uid(sender), self.auth.own_uid()) {
                (Some(caller), Some(own)) => sender_uid_allowed(caller, own),
                // Unresolvable sender or unreadable own UID: deny.
                _ => false,
            },
            None => false,
        };
        if allowed {
            return Ok(());
        }
        tracing::warn!(
            sender = ?sender,
            "denying org.icedtea.Session mutation from foreign-UID or unresolvable sender"
        );
        Err(zbus::fdo::Error::AccessDenied(
            "org.icedtea.Session mutations are restricted to the session owner".to_string(),
        ))
    }

    /// Poll the compositor until it reports locked, the budget elapses, or the
    /// compositor is unreachable. Delegates to [`crate::flow::poll_until_locked`]
    /// so this and the pre-sleep wait share one confirmation policy; used by
    /// [`Self::lock`] so a lock request that spawned nothing (no/denied
    /// `locker_command`) is not reported as a success, and so an unreachable
    /// compositor is reported as such without burning the whole budget on
    /// dispatch threads.
    fn confirm_locked(&self) -> LockConfirm {
        match crate::flow::poll_until_locked(&*self.wm, self.lock_confirm_budget, LOCK_CONFIRM_POLL)
        {
            LockState::Locked => LockConfirm::Confirmed,
            LockState::Unlocked => LockConfirm::TimedOut,
            LockState::Unreachable => LockConfirm::Unreachable,
        }
    }
}

/// Run one logind operation and, on failure, turn its `io::Error` into a D-Bus
/// error reply naming the operation. A successful logind call replies `Ok`;
/// a failed one makes the caller's `call_method` return `Err`, which the CLI
/// turns into a non-zero exit — the action's outcome is never swallowed.
fn call_logind(operation: &str, result: io::Result<()>) -> zbus::fdo::Result<()> {
    result.map_err(|err| {
        tracing::warn!(%err, operation, "org.icedtea.Session logind call failed");
        zbus::fdo::Error::Failed(format!("{operation} failed: {err}"))
    })
}

#[interface(name = "org.icedtea.Session")]
impl SessionInterface {
    /// `Session.Lock()` on the login1 proxy, confirmed against the compositor.
    ///
    /// `Session.Lock` only sets logind's `LockedHint` and emits the signal that
    /// drives the locker-spawn funnel; it does not by itself lock the screen.
    /// A successful call is therefore not proof of a lock — a missing or
    /// refused `locker_command` leaves the session unlocked. This replies `Ok`
    /// only once the compositor confirms the lock within
    /// [`LOCK_CONFIRM_BUDGET`]; otherwise it is an error reply (the CLI exits
    /// non-zero) with an error-level log. An unreachable compositor is reported
    /// distinctly and without waiting out the budget.
    fn lock(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_lock()
    }

    /// `Manager.Suspend(false)`: `false` is logind's non-interactive flag (this
    /// call already comes from the authenticated session's own daemon; see
    /// `ZbusLogind`).
    fn suspend(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_suspend()
    }

    /// `Manager.Hibernate(false)`.
    fn hibernate(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_hibernate()
    }

    /// `Manager.PowerOff(false)`.
    fn power_off(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_power_off()
    }

    /// `Manager.Reboot(false)`.
    fn reboot(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_reboot()
    }

    /// Forward to `org.icedtea.Compositor`'s `Quit` via [`WmClient`]. A `false`
    /// from `quit` (compositor unreachable) is an error reply, so the CLI's
    /// `logout` exits non-zero rather than reporting a logout that did not
    /// happen.
    fn log_out(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&header)?;
        self.op_log_out()
    }

    /// Mirror the compositor's `IsLocked` value. Read-only, deliberately
    /// ungated (see [`Self::authorize`]).
    fn is_locked(&self) -> bool {
        self.wm.is_locked()
    }
}

impl SessionInterface {
    /// The `Lock` operation body, without the sender gate.
    fn op_lock(&self) -> zbus::fdo::Result<()> {
        call_logind("lock", self.logind.lock())?;
        match self.confirm_locked() {
            LockConfirm::Confirmed => Ok(()),
            LockConfirm::Unreachable => {
                tracing::error!(
                    "org.icedtea.Session.Lock: compositor unreachable; reporting failure"
                );
                Err(zbus::fdo::Error::Failed(
                    "lock failed: compositor unreachable".to_string(),
                ))
            }
            LockConfirm::TimedOut => {
                tracing::error!(
                    "org.icedtea.Session.Lock: compositor did not confirm the lock; \
                     reporting failure"
                );
                Err(zbus::fdo::Error::Failed(
                    "lock failed: session did not lock".to_string(),
                ))
            }
        }
    }

    /// The `Suspend` operation body, without the sender gate.
    fn op_suspend(&self) -> zbus::fdo::Result<()> {
        call_logind("suspend", self.logind.suspend())
    }

    /// The `Hibernate` operation body, without the sender gate.
    fn op_hibernate(&self) -> zbus::fdo::Result<()> {
        call_logind("hibernate", self.logind.hibernate())
    }

    /// The `PowerOff` operation body, without the sender gate.
    fn op_power_off(&self) -> zbus::fdo::Result<()> {
        call_logind("power_off", self.logind.power_off())
    }

    /// The `Reboot` operation body, without the sender gate.
    fn op_reboot(&self) -> zbus::fdo::Result<()> {
        call_logind("reboot", self.logind.reboot())
    }

    /// The `LogOut` operation body, without the sender gate.
    fn op_log_out(&self) -> zbus::fdo::Result<()> {
        if self.wm.quit() {
            return Ok(());
        }
        tracing::warn!("org.icedtea.Session.LogOut failed: compositor unreachable");
        Err(zbus::fdo::Error::Failed(
            "log out failed: compositor unreachable".to_string(),
        ))
    }
}

/// Register `org.icedtea.Session` on the ambient session bus. Returns the
/// connection, which the caller keeps alive for the service's life. `Err` if
/// the session bus is unavailable or the name is already owned (another daemon
/// running) — the binary treats that as fatal.
pub fn spawn(logind: SharedLogind, wm: SharedWm) -> zbus::Result<Connection> {
    let conn = Connection::session()?;
    conn.object_server()
        .at(SESSION_PATH, SessionInterface::new(logind, wm))?;
    conn.request_name(SESSION_BUS_NAME)?;
    Ok(conn)
}

/// [`spawn`] against an explicitly-addressed bus (a private test bus), so the
/// integration tests never touch the process environment to redirect the
/// session bus. The credential lookup follows the same address, so the
/// same-UID gate is exercised over the private bus too.
pub fn spawn_on_bus(logind: SharedLogind, wm: SharedWm, address: &str) -> zbus::Result<Connection> {
    let conn = zbus::blocking::connection::Builder::address(address)?.build()?;
    let iface = SessionInterface::with_auth(logind, wm, Arc::new(BusCallerAuth::at(address)));
    conn.object_server().at(SESSION_PATH, iface)?;
    conn.request_name(SESSION_BUS_NAME)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use zbus::object_server::Interface as _;
    use zbus::zvariant::OwnedObjectPath;

    use super::*;
    use crate::logind::{InhibitMode, Logind, LogindCall, RecordingLogind};
    use crate::wm_client::{RecordingWm, WmCall};

    fn service(locked: bool) -> (SessionInterface, Arc<RecordingLogind>, Arc<RecordingWm>) {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let wm = Arc::new(RecordingWm::new(locked));
        let iface = SessionInterface::new(logind.clone(), wm.clone());
        (iface, logind, wm)
    }

    /// A [`CallerAuth`] double: it answers with the UIDs it was built with, so
    /// the foreign-UID branch the private test bus cannot produce is testable.
    struct StubAuth {
        caller: Option<u32>,
        own: Option<u32>,
    }

    impl CallerAuth for StubAuth {
        fn caller_uid(&self, _sender: &str) -> Option<u32> {
            self.caller
        }
        fn own_uid(&self) -> Option<u32> {
            self.own
        }
    }

    fn iface_with_auth(caller: Option<u32>, own: Option<u32>) -> SessionInterface {
        SessionInterface::with_auth(
            Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1")),
            Arc::new(RecordingWm::new(false)),
            Arc::new(StubAuth { caller, own }),
        )
    }

    /// A [`WmClient`] that answers `lock_state` from a scripted queue, so a
    /// delayed lock confirmation and an unreachable compositor are drivable.
    /// Past the script it repeats `fallback`.
    struct ScriptedWm {
        states: Mutex<VecDeque<LockState>>,
        fallback: LockState,
    }

    impl ScriptedWm {
        fn new(states: impl IntoIterator<Item = LockState>) -> Self {
            Self::with_fallback(states, LockState::Unlocked)
        }

        fn with_fallback(states: impl IntoIterator<Item = LockState>, fallback: LockState) -> Self {
            Self {
                states: Mutex::new(states.into_iter().collect()),
                fallback,
            }
        }
    }

    impl WmClient for ScriptedWm {
        fn is_locked(&self) -> bool {
            self.lock_state() == LockState::Locked
        }
        fn lock_state(&self) -> LockState {
            self.states
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or(self.fallback)
        }
        fn quit(&self) -> bool {
            true
        }
    }

    fn scripted_service(states: impl IntoIterator<Item = LockState>) -> SessionInterface {
        SessionInterface::new(
            Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1")),
            Arc::new(ScriptedWm::new(states)),
        )
    }

    fn scripted_service_with_fallback(
        states: impl IntoIterator<Item = LockState>,
        fallback: LockState,
    ) -> SessionInterface {
        SessionInterface::new(
            Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1")),
            Arc::new(ScriptedWm::with_fallback(states, fallback)),
        )
    }

    /// A [`Logind`] whose every fallible operation fails, to prove the service
    /// turns a failed logind call into a D-Bus error reply rather than a
    /// success reply.
    struct FailingLogind;

    impl Logind for FailingLogind {
        fn session_path(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from("/org/freedesktop/login1/session/c1")
                .expect("a logind session path is a valid D-Bus object path")
        }
        fn inhibit(&self, _what: &str, _why: &str, _mode: InhibitMode) -> io::Result<OwnedFd> {
            Err(io::Error::other("logind unavailable"))
        }
        fn lock(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
        fn unlock(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
        fn suspend(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
        fn hibernate(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
        fn power_off(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
        fn reboot(&self) -> io::Result<()> {
            Err(io::Error::other("logind unavailable"))
        }
    }

    /// `LogOut()` must reach the compositor's `Quit()` exactly once — not zero
    /// (the session never ends) and not twice (a double logout).
    #[test]
    fn log_out_forwards_exactly_once_to_quit() {
        let (iface, _logind, wm) = service(false);
        iface.op_log_out().expect("recording wm accepts quit");
        assert_eq!(wm.calls(), vec![WmCall::Quit]);
    }

    /// A compositor that refuses `Quit` (unreachable/dead bus) must be an error
    /// reply: the CLI's `logout` then exits non-zero instead of reporting a
    /// logout that never happened.
    #[test]
    fn log_out_is_an_error_reply_when_the_compositor_refuses() {
        let iface = SessionInterface::new(
            Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1")),
            Arc::new(RecordingWm::with_quit_result(false, false)),
        );
        match iface.op_log_out() {
            Err(zbus::fdo::Error::Failed(message)) => {
                assert!(
                    message.contains("log out"),
                    "names the operation: {message}"
                );
            }
            other => panic!("expected fdo::Error::Failed, got {other:?}"),
        }
    }

    #[test]
    fn lock_calls_logind_once_and_confirms_with_the_compositor() {
        let (iface, logind, wm) = service(true);
        iface
            .op_lock()
            .expect("recording logind succeeds and wm confirms");
        assert_eq!(logind.calls(), vec![LogindCall::Lock]);
        assert_eq!(wm.calls(), vec![WmCall::IsLocked]);
    }

    /// A `Lock()` that never reaches a confirmed locked state — no locker was
    /// spawned, or the spawn was refused — must be an error reply, so the
    /// `lock` CLI exits non-zero instead of reporting a lock that never
    /// happened.
    #[test]
    fn lock_is_an_error_reply_when_the_compositor_never_confirms() {
        let (mut iface, logind, wm) = service(false);
        iface.lock_confirm_budget = Duration::from_millis(50);
        match iface.op_lock() {
            Err(zbus::fdo::Error::Failed(message)) => {
                assert!(message.contains("lock"), "names the operation: {message}");
            }
            other => panic!("expected fdo::Error::Failed, got {other:?}"),
        }
        assert_eq!(
            logind.calls(),
            vec![LogindCall::Lock],
            "the session lock was still requested once"
        );
        assert!(wm.calls().contains(&WmCall::IsLocked));
    }

    #[test]
    fn suspend_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.op_suspend().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Suspend]);
    }

    #[test]
    fn hibernate_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.op_hibernate().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Hibernate]);
    }

    #[test]
    fn power_off_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.op_power_off().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::PowerOff]);
    }

    #[test]
    fn reboot_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.op_reboot().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Reboot]);
    }

    #[test]
    fn is_locked_mirrors_the_wm_client() {
        let (iface, _logind, _wm) = service(true);
        assert!(iface.is_locked());

        let (iface, _logind, _wm) = service(false);
        assert!(!iface.is_locked());
    }

    /// A failed logind operation must be an error reply, never a success
    /// reply: the caller's `call_method` then returns `Err` and the CLI exits
    /// non-zero instead of reporting a power action that never happened.
    #[test]
    fn a_failing_logind_call_is_reported_as_an_fdo_error_reply() {
        let iface =
            SessionInterface::new(Arc::new(FailingLogind), Arc::new(RecordingWm::new(false)));
        for (operation, result) in [
            ("lock", iface.op_lock()),
            ("suspend", iface.op_suspend()),
            ("hibernate", iface.op_hibernate()),
            ("power_off", iface.op_power_off()),
            ("reboot", iface.op_reboot()),
        ] {
            match result {
                Err(zbus::fdo::Error::Failed(message)) => {
                    assert!(
                        message.contains(operation),
                        "{operation}: error reply names the operation: {message}"
                    );
                }
                other => panic!("{operation}: expected fdo::Error::Failed, got {other:?}"),
            }
        }
    }

    /// Pin the wire surface: every spec method is exposed under its PascalCase
    /// member, and `IsLocked` returns one bool. Generated from the live
    /// interface, so removing or renaming a method fails here rather than
    /// silently reaching no method at runtime.
    #[test]
    fn interface_exposes_every_spec_member() {
        let (iface, _logind, _wm) = service(false);
        let mut xml = String::new();
        iface.introspect_to_writer(&mut xml, 0);
        for member in [
            "Lock",
            "Suspend",
            "Hibernate",
            "PowerOff",
            "Reboot",
            "LogOut",
            "IsLocked",
        ] {
            assert!(
                xml.contains(&format!("name=\"{member}\"")),
                "missing {member}: {xml}"
            );
        }
        let is_locked = xml
            .split("<method")
            .find(|chunk| chunk.contains("name=\"IsLocked\""))
            .expect("IsLocked present in introspection XML");
        let body = &is_locked[..is_locked.find("</method>").unwrap_or(is_locked.len())];
        assert!(
            body.contains("type=\"b\"") && body.contains("direction=\"out\""),
            "IsLocked returns one bool out-arg: {body}"
        );
    }

    /// The mutation gate is a plain UID equality: same UID is allowed, any
    /// other UID is not. The D-Bus plumbing around it is exercised over a
    /// private bus by `session/tests/cli.rs`.
    #[test]
    fn sender_uid_gate_allows_only_the_same_uid() {
        assert!(sender_uid_allowed(1000, 1000));
        assert!(sender_uid_allowed(0, 0));
        assert!(!sender_uid_allowed(1000, 1001));
        assert!(!sender_uid_allowed(1001, 1000));
    }

    /// Fail-closed gate: a same-UID caller is accepted, a foreign UID is
    /// `AccessDenied`, and an unresolvable sender or own UID is denied too (a
    /// missing credential must never read as "allowed").
    #[test]
    fn the_mutation_gate_accepts_the_same_uid_and_denies_everything_else() {
        assert!(
            iface_with_auth(Some(1000), Some(1000))
                .authorize_sender(Some(":1.5"))
                .is_ok(),
            "same-UID caller must be allowed"
        );

        for (caller, own, label) in [
            (Some(1001), Some(1000), "foreign UID"),
            (None, Some(1000), "unresolvable sender"),
            (Some(1000), None, "unreadable own UID"),
        ] {
            let result = iface_with_auth(caller, own).authorize_sender(Some(":1.5"));
            assert!(
                matches!(result, Err(zbus::fdo::Error::AccessDenied(_))),
                "{label}: expected AccessDenied, got {result:?}"
            );
        }

        let no_sender = iface_with_auth(Some(1000), Some(1000)).authorize_sender(None);
        assert!(
            matches!(no_sender, Err(zbus::fdo::Error::AccessDenied(_))),
            "a header with no sender must be denied: {no_sender:?}"
        );
    }

    /// A compositor that locks only after a few polls must still confirm: the
    /// budget is not a single-shot check. `false, false, true` → `Ok`.
    #[test]
    fn lock_succeeds_on_a_delayed_compositor_confirmation() {
        let iface = scripted_service([LockState::Unlocked, LockState::Unlocked, LockState::Locked]);
        iface
            .op_lock()
            .expect("a compositor that locks after two polls must confirm");
    }

    /// An unreachable compositor must end the confirmation wait at once with a
    /// distinct "compositor unreachable" error, not sleep out the budget and
    /// then misreport a slow locker. The budget is 30s, so only the
    /// short-circuit can make this return quickly.
    #[test]
    fn lock_fails_fast_when_the_compositor_is_unreachable() {
        let mut iface =
            scripted_service_with_fallback([LockState::Unreachable], LockState::Unreachable);
        iface.lock_confirm_budget = Duration::from_secs(30);
        let started = Instant::now();
        match iface.op_lock() {
            Err(zbus::fdo::Error::Failed(message)) => {
                assert!(
                    message.contains("unreachable"),
                    "names the unreachable compositor: {message}"
                );
            }
            other => panic!("expected fdo::Error::Failed, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an unreachable compositor must short-circuit, not burn the budget"
        );
    }
}
