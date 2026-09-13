//! systemd-logind access: session discovery plus the [`Logind`] seam.
//!
//! Everything the daemon asks logind to do goes through the [`Logind`] trait,
//! so policy decisions are unit-testable with [`RecordingLogind`] and no
//! system bus. [`ZbusLogind`] is the thin production adapter over
//! `zbus::blocking::Connection::system()` and the `Manager`/`Session` proxies.
//!
//! Discovery resolves *this* login session in two steps (spec §logind
//! session/seat discovery): `$XDG_SESSION_ID` via `Manager.GetSession` when
//! set, else `Manager.GetSessionByPID(getpid())`. Failure is fatal at startup.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

/// Named the daemon claims in the `who` field of every `Inhibit` call.
const INHIBIT_WHO: &str = "icedtea-session";

/// `what` for the sleep delay inhibitor (spec Decision 5).
pub const SLEEP_INHIBIT_WHAT: &str = "sleep";
/// `why` for the sleep delay inhibitor.
pub const SLEEP_INHIBIT_WHY: &str = "lock screen before suspend";
/// `what` for the power-key/suspend-key/hibernate-key block inhibitor
/// (spec Decision 4). `handle-lid-switch` is deliberately absent — lid
/// handling stays with logind's own default.
pub const POWER_KEY_INHIBIT_WHAT: &str = "handle-power-key:handle-suspend-key:handle-hibernate-key";
/// `why` for the power-key block inhibitor.
pub const POWER_KEY_INHIBIT_WHY: &str = "desktop session handles these keys";

/// Whether an `Inhibit` is a one-shot `delay` (closing the fd lets the held-up
/// action proceed) or a lifetime `block` (logind must not act on the event at
/// all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InhibitMode {
    /// `"delay"` — hold up sleep until the fd closes.
    Delay,
    /// `"block"` — tell logind not to act on the event itself.
    Block,
}

impl InhibitMode {
    /// The logind wire string for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            InhibitMode::Delay => "delay",
            InhibitMode::Block => "block",
        }
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath>;

    // Written out because the PascalCase conversion would produce
    // `GetSessionByPid`, but logind's member is `GetSessionByPID`.
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;

    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedFd>;

    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
    fn hibernate(&self, interactive: bool) -> zbus::Result<()>;
    fn power_off(&self, interactive: bool) -> zbus::Result<()>;
    fn reboot(&self, interactive: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Session {
    fn lock(&self) -> zbus::Result<()>;
    fn unlock(&self) -> zbus::Result<()>;
}

/// The `Manager` lookups session discovery needs, abstracted so the *choice*
/// between `$XDG_SESSION_ID` and the pid fallback is unit-testable without a
/// bus. Implemented for a real blocking [`Connection`] in production and by
/// [`RecordingLogind`] under test.
pub trait SessionResolver {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath>;
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;
}

impl SessionResolver for Connection {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath> {
        ManagerProxyBlocking::new(self)?.get_session(session_id)
    }
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath> {
        ManagerProxyBlocking::new(self)?.get_session_by_pid(pid)
    }
}

/// Resolve the logind session object for this process.
///
/// `xdg_session_id` is `$XDG_SESSION_ID` when set (the `pam_systemd` path); it
/// is preferred because it does not depend on this process sharing the login
/// session's cgroup. An empty string is treated as absent (a misconfigured or
/// blank `$XDG_SESSION_ID` must not be handed to `GetSession`), so it falls
/// back to the pid path. With no id (an unusual/manual launch), the pid
/// fallback walks the cgroup membership, matching `loginctl`'s own "current
/// session".
pub fn resolve_session<C: SessionResolver + ?Sized>(
    conn: &C,
    xdg_session_id: Option<&str>,
    pid: u32,
) -> zbus::Result<OwnedObjectPath> {
    match xdg_session_id.filter(|id| !id.is_empty()) {
        Some(session_id) => conn.get_session(session_id),
        None => conn.get_session_by_pid(pid),
    }
}

/// The `logind` operations the session daemon performs. Abstracted so the
/// service/policy layers depend on behavior a test can record rather than a
/// live system bus.
pub trait Logind {
    /// The resolved `org.freedesktop.login1.Session` object path.
    fn session_path(&self) -> OwnedObjectPath;
    /// Take a `Manager.Inhibit(what, "icedtea-session", why, mode)` fd. The fd
    /// closes when the returned handle drops; closing a `delay` fd is what
    /// releases the held-up action.
    fn inhibit(&self, what: &str, why: &str, mode: InhibitMode) -> io::Result<OwnedFd>;
    fn lock(&self) -> io::Result<()>;
    fn unlock(&self) -> io::Result<()>;
    fn suspend(&self) -> io::Result<()>;
    fn hibernate(&self) -> io::Result<()>;
    fn power_off(&self) -> io::Result<()>;
    fn reboot(&self) -> io::Result<()>;
}

/// The `"sleep"` delay inhibitor, held as a `Drop` guard. Closing the fd is
/// what lets a pending suspend proceed, so the release happens on **every**
/// path — normal return, early return, or unwinding — the moment this guard
/// drops (spec Decision 5). `#[must_use]` so a caller cannot acquire one (and
/// hold up sleep) without binding it.
#[must_use = "the delay inhibitor is released as soon as this guard drops; bind it"]
pub struct SleepInhibitor {
    _fd: OwnedFd,
}

impl SleepInhibitor {
    /// Acquire a fresh delay inhibitor against `logind`.
    pub fn acquire(logind: &(dyn Logind + Send + Sync)) -> io::Result<Self> {
        Ok(Self {
            _fd: logind.inhibit(SLEEP_INHIBIT_WHAT, SLEEP_INHIBIT_WHY, InhibitMode::Delay)?,
        })
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        tracing::debug!("releasing the sleep delay inhibitor");
    }
}

/// The power-key/suspend-key/hibernate-key `block` inhibitor (spec Decision 4),
/// held for the daemon's whole lifetime so logind leaves those keys to the
/// compositor. Dropping it restores logind's defaults — the safe failure mode
/// when the daemon exits.
#[must_use = "dropping this guard hands the power keys back to logind; hold it for the daemon's lifetime"]
pub struct PowerKeyInhibitor {
    _fd: OwnedFd,
}

impl PowerKeyInhibitor {
    /// Acquire the block inhibitor against `logind`.
    pub fn acquire(logind: &(dyn Logind + Send + Sync)) -> io::Result<Self> {
        Ok(Self {
            _fd: logind.inhibit(
                POWER_KEY_INHIBIT_WHAT,
                POWER_KEY_INHIBIT_WHY,
                InhibitMode::Block,
            )?,
        })
    }
}

impl Drop for PowerKeyInhibitor {
    fn drop(&mut self) {
        tracing::debug!("releasing the power-key block inhibitor");
    }
}

/// The production [`Logind`] over the system bus.
pub struct ZbusLogind {
    conn: Connection,
    session_path: OwnedObjectPath,
}

impl ZbusLogind {
    /// Connect to the system bus and resolve this process's login session.
    /// `Err` if the bus is unavailable or no session can be resolved — the
    /// binary treats that as fatal.
    pub fn connect(xdg_session_id: Option<&str>, pid: u32) -> zbus::Result<Self> {
        let conn = Connection::system()?;
        Self::with_connection(conn, xdg_session_id, pid)
    }

    /// Resolve the session over an already-open system connection.
    pub fn with_connection(
        conn: Connection,
        xdg_session_id: Option<&str>,
        pid: u32,
    ) -> zbus::Result<Self> {
        let session_path = resolve_session(&conn, xdg_session_id, pid)?;
        Ok(Self { conn, session_path })
    }

    fn manager(&self) -> zbus::Result<ManagerProxyBlocking<'_>> {
        ManagerProxyBlocking::new(&self.conn)
    }

    fn session(&self) -> zbus::Result<SessionProxyBlocking<'_>> {
        SessionProxyBlocking::builder(&self.conn)
            .path(self.session_path.clone())?
            .build()
    }
}

impl Logind for ZbusLogind {
    fn session_path(&self) -> OwnedObjectPath {
        self.session_path.clone()
    }

    fn inhibit(&self, what: &str, why: &str, mode: InhibitMode) -> io::Result<OwnedFd> {
        let fd = self
            .manager()
            .map_err(io::Error::other)?
            .inhibit(what, INHIBIT_WHO, why, mode.as_str())
            .map_err(io::Error::other)?;
        Ok(fd.into())
    }

    fn lock(&self) -> io::Result<()> {
        self.session()
            .map_err(io::Error::other)?
            .lock()
            .map_err(io::Error::other)
    }

    fn unlock(&self) -> io::Result<()> {
        self.session()
            .map_err(io::Error::other)?
            .unlock()
            .map_err(io::Error::other)
    }

    fn suspend(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .suspend(false)
            .map_err(io::Error::other)
    }

    fn hibernate(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .hibernate(false)
            .map_err(io::Error::other)
    }

    fn power_off(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .power_off(false)
            .map_err(io::Error::other)
    }

    fn reboot(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .reboot(false)
            .map_err(io::Error::other)
    }
}

/// One recorded interaction with the [`Logind`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogindCall {
    GetSession(String),
    GetSessionByPid(u32),
    Inhibit {
        what: String,
        why: String,
        mode: InhibitMode,
    },
    Lock,
    Unlock,
    Suspend,
    Hibernate,
    PowerOff,
    Reboot,
}

/// A [`Logind`] test double: records every call in order and answers with a
/// configured session path. Every policy/discovery decision is testable
/// against this with no real D-Bus.
#[derive(Debug, Clone)]
pub struct RecordingLogind {
    session_path: OwnedObjectPath,
    calls: Arc<Mutex<Vec<LogindCall>>>,
    /// The write end of every pipe whose read end [`Logind::inhibit`] handed
    /// out, so [`Self::live_inhibitors`] can tell whether the caller's guard
    /// still holds it open.
    inhibitors: Arc<Mutex<Vec<File>>>,
}

impl RecordingLogind {
    /// Build a recording double whose session resolves to `session_path`.
    pub fn new(session_path: &str) -> Self {
        let session_path = OwnedObjectPath::try_from(session_path)
            .expect("a logind session path is a valid D-Bus object path");
        Self {
            session_path,
            calls: Arc::new(Mutex::new(Vec::new())),
            inhibitors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every call recorded so far, in order.
    pub fn calls(&self) -> Vec<LogindCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many inhibitor fds [`Logind::inhibit`] handed out are still open.
    /// The write end of each returned read-end pipe is retained; a write that
    /// fails with `EPIPE` means the caller's guard has dropped and closed it.
    pub fn live_inhibitors(&self) -> usize {
        use std::io::Write as _;
        let mut live = self.inhibitors.lock().unwrap_or_else(|e| e.into_inner());
        live.retain_mut(|write| write.write_all(&[0u8]).is_ok());
        live.len()
    }

    fn record(&self, call: LogindCall) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(call);
    }
}

impl SessionResolver for RecordingLogind {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath> {
        self.record(LogindCall::GetSession(session_id.to_string()));
        Ok(self.session_path.clone())
    }

    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath> {
        self.record(LogindCall::GetSessionByPid(pid));
        Ok(self.session_path.clone())
    }
}

impl Logind for RecordingLogind {
    fn session_path(&self) -> OwnedObjectPath {
        self.session_path.clone()
    }

    fn inhibit(&self, what: &str, why: &str, mode: InhibitMode) -> io::Result<OwnedFd> {
        self.record(LogindCall::Inhibit {
            what: what.to_string(),
            why: why.to_string(),
            mode,
        });
        let (read, write) = io::pipe()?;
        // Keep the write end: `live_inhibitors` probes it to detect that the
        // read end the caller holds has been closed.
        let write: OwnedFd = write.into();
        self.inhibitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(File::from(write));
        Ok(read.into())
    }

    fn lock(&self) -> io::Result<()> {
        self.record(LogindCall::Lock);
        Ok(())
    }

    fn unlock(&self) -> io::Result<()> {
        self.record(LogindCall::Unlock);
        Ok(())
    }

    fn suspend(&self) -> io::Result<()> {
        self.record(LogindCall::Suspend);
        Ok(())
    }

    fn hibernate(&self) -> io::Result<()> {
        self.record(LogindCall::Hibernate);
        Ok(())
    }

    fn power_off(&self) -> io::Result<()> {
        self.record(LogindCall::PowerOff);
        Ok(())
    }

    fn reboot(&self) -> io::Result<()> {
        self.record(LogindCall::Reboot);
        Ok(())
    }
}

/// One logind signal the daemon acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogindSignal {
    /// `Manager.PrepareForSleep(bool)`.
    PrepareForSleep,
    /// `Session.Lock()` on this session.
    Lock,
    /// `Session.Unlock()` on this session.
    Unlock,
}

/// Classify an incoming logind signal from its interface, member, and object
/// path. Pure, so the dispatch table is unit-tested without a bus. `Lock` and
/// `Unlock` are scoped to `session_path`; logind emits them for every session
/// on the bus.
pub fn classify_signal(
    interface: Option<&str>,
    member: Option<&str>,
    path: Option<&str>,
    session_path: &str,
) -> Option<LogindSignal> {
    match (interface, member) {
        (Some("org.freedesktop.login1.Manager"), Some("PrepareForSleep")) => {
            Some(LogindSignal::PrepareForSleep)
        }
        (Some("org.freedesktop.login1.Session"), Some("Lock")) if path == Some(session_path) => {
            Some(LogindSignal::Lock)
        }
        (Some("org.freedesktop.login1.Session"), Some("Unlock")) if path == Some(session_path) => {
            Some(LogindSignal::Unlock)
        }
        _ => None,
    }
}

/// What the signal loop needs from the flow, so `logind.rs` does not depend on
/// the concrete flow type (and the dispatch stays unit-testable).
pub trait LogindEvents: Send + Sync {
    /// `PrepareForSleep(true)` is imminent sleep, `false` is resume.
    fn prepare_for_sleep(&self, going_to_sleep: bool);
    /// An explicit `Session.Lock` (an external lock, or our own `Lock()` call).
    fn lock(&self);
    /// An explicit `Session.Unlock`.
    fn unlock(&self);
}

/// A shared [`LogindEvents`] sink.
pub type SharedLogindEvents = Arc<dyn LogindEvents>;

/// Subscribe to logind's `PrepareForSleep` and this session's `Lock`/`Unlock`
/// on a dedicated thread (`zbus::block_on`, no tokio), forwarding each to
/// `events`. Errors are logged; a dropped bus ends the loop.
pub fn spawn_signal_loop(session_path: OwnedObjectPath, events: SharedLogindEvents) {
    let spawned = std::thread::Builder::new()
        .name("icedtea-session-logind".to_string())
        .spawn(move || {
            if let Err(err) = zbus::block_on(run_signal_loop(&session_path, events)) {
                tracing::error!(%err, "logind signal subscription ended");
            }
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "could not spawn the logind signal thread");
    }
}

async fn run_signal_loop(
    session_path: &OwnedObjectPath,
    events: SharedLogindEvents,
) -> zbus::Result<()> {
    use futures_util::StreamExt as _;

    let conn = zbus::Connection::system().await?;
    // One rule per connection: logind sends both Manager and Session signals
    // from the same well-known sender, so dispatch is by interface/path below.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    while let Some(message) = stream.next().await {
        let message = match message {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!(%err, "logind signal stream error");
                continue;
            }
        };
        let header = message.header();
        let signal = classify_signal(
            header.interface().map(|iface| iface.as_str()),
            header.member().map(|member| member.as_str()),
            header.path().map(|path| path.as_str()),
            session_path.as_str(),
        );
        match signal {
            Some(LogindSignal::PrepareForSleep) => match message.body().deserialize::<bool>() {
                Ok(going_to_sleep) => events.prepare_for_sleep(going_to_sleep),
                Err(err) => tracing::warn!(%err, "PrepareForSleep body decode failed"),
            },
            Some(LogindSignal::Lock) => events.lock(),
            Some(LogindSignal::Unlock) => events.unlock(),
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_session_prefers_the_xdg_session_id() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c7");
        let path = resolve_session(&logind, Some("c7"), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c7");
        assert_eq!(
            logind.calls(),
            vec![LogindCall::GetSession("c7".to_string())]
        );
    }

    #[test]
    fn resolve_session_falls_back_to_the_pid_without_an_id() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let path = resolve_session(&logind, None, 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(logind.calls(), vec![LogindCall::GetSessionByPid(4242)]);
    }

    #[test]
    fn resolve_session_ignores_the_pid_when_an_id_is_present() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let _ = resolve_session(&logind, Some("c1"), 4242).expect("resolves");
        assert!(!logind.calls().contains(&LogindCall::GetSessionByPid(4242)));
    }

    #[test]
    fn resolve_session_treats_an_empty_xdg_session_id_as_absent() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let path = resolve_session(&logind, Some(""), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(logind.calls(), vec![LogindCall::GetSessionByPid(4242)]);
    }

    #[test]
    fn recording_logind_records_every_trait_call_in_order() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        assert_eq!(
            logind.session_path().as_str(),
            "/org/freedesktop/login1/session/c1"
        );
        let _fd = logind
            .inhibit(SLEEP_INHIBIT_WHAT, SLEEP_INHIBIT_WHY, InhibitMode::Delay)
            .expect("inhibit returns an fd");
        logind.lock().expect("lock");
        logind.unlock().expect("unlock");
        logind.suspend().expect("suspend");
        logind.hibernate().expect("hibernate");
        logind.power_off().expect("power off");
        logind.reboot().expect("reboot");
        assert_eq!(
            logind.calls(),
            vec![
                LogindCall::Inhibit {
                    what: SLEEP_INHIBIT_WHAT.to_string(),
                    why: SLEEP_INHIBIT_WHY.to_string(),
                    mode: InhibitMode::Delay,
                },
                LogindCall::Lock,
                LogindCall::Unlock,
                LogindCall::Suspend,
                LogindCall::Hibernate,
                LogindCall::PowerOff,
                LogindCall::Reboot,
            ]
        );
    }

    #[test]
    fn inhibit_modes_map_to_their_logind_wire_strings() {
        assert_eq!(InhibitMode::Delay.as_str(), "delay");
        assert_eq!(InhibitMode::Block.as_str(), "block");
    }

    /// A delay guard holds the fd open; dropping it closes the fd (releasing
    /// sleep). This is the spec's "release on every path" invariant.
    #[test]
    fn a_sleep_inhibitor_is_released_when_its_guard_drops() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let guard = SleepInhibitor::acquire(&logind).expect("inhibit returns a guard");
        assert_eq!(logind.live_inhibitors(), 1);
        drop(guard);
        assert_eq!(logind.live_inhibitors(), 0);
    }

    /// The power-key block inhibitor is acquired with the `handle-*` set and
    /// `block` mode (spec Decision 4).
    #[test]
    fn the_power_key_inhibitor_blocks_the_handle_keys() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let _guard = PowerKeyInhibitor::acquire(&logind).expect("inhibit returns a guard");
        assert_eq!(
            logind.calls(),
            vec![LogindCall::Inhibit {
                what: POWER_KEY_INHIBIT_WHAT.to_string(),
                why: POWER_KEY_INHIBIT_WHY.to_string(),
                mode: InhibitMode::Block,
            }]
        );
    }

    /// The dispatch table, isolated from the bus: only `PrepareForSleep`, and
    /// only this session's `Lock`/`Unlock`, are acted on.
    #[test]
    fn classify_signal_accepts_only_the_relevant_logind_signals() {
        let session = "/org/freedesktop/login1/session/c1";
        let manager = "org.freedesktop.login1.Manager";
        let session_iface = "org.freedesktop.login1.Session";

        assert_eq!(
            classify_signal(Some(manager), Some("PrepareForSleep"), None, session),
            Some(LogindSignal::PrepareForSleep)
        );
        assert_eq!(
            classify_signal(Some(session_iface), Some("Lock"), Some(session), session),
            Some(LogindSignal::Lock)
        );
        assert_eq!(
            classify_signal(Some(session_iface), Some("Unlock"), Some(session), session),
            Some(LogindSignal::Unlock)
        );
        // Another session's Lock/Unlock must not be ours.
        assert_eq!(
            classify_signal(
                Some(session_iface),
                Some("Lock"),
                Some("/org/freedesktop/login1/session/c2"),
                session
            ),
            None
        );
        // Unrelated login1 signals (SessionNew, SeatNew, ...) are ignored.
        assert_eq!(
            classify_signal(Some(manager), Some("SessionNew"), None, session),
            None
        );
        assert_eq!(classify_signal(None, None, None, session), None);
    }
}
