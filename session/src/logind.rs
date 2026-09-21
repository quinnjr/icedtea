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
use std::thread::JoinHandle;
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

/// Named the daemon claims in the `who` field of every `Inhibit` call.
pub const INHIBIT_WHO: &str = "icedtea-session";

/// `interactive` passed to every power action (`Suspend`/`Hibernate`/
/// `PowerOff`/`Reboot`). `false` because these calls come from the user's own
/// authenticated session daemon, so logind may not raise a polkit prompt for
/// them — exactly how `loginctl` calls them from an already-privileged
/// session context. Named so the wire-level test can pin it.
pub const POWER_ACTION_INTERACTIVE: bool = false;

/// Deadline on every method call to the system bus.
///
/// Without a bound, a stalled `systemd-logind` (or a hung system bus) would
/// block the caller indefinitely; `Icedtea-session` makes these calls from the
/// pre-sleep path, where the sleep delay inhibitor is only released after them.
/// 750ms mirrors [`crate::wm_client::COMPOSITOR_CALL_TIMEOUT`] and keeps the
/// worst-case pre-sleep path (`Session.Lock` + the 3s budget + one in-flight
/// `IsLocked`) under logind's 5s `InhibitDelayMaxSec`.
pub const LOGIND_CALL_TIMEOUT: Duration = Duration::from_millis(750);

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
/// `xdg_session_id` is `$XDG_SESSION_ID` when set (the `pam_systemd` path). It
/// is environment-controlled and therefore untrusted: a same-user actor can
/// point it at another session and redirect this daemon's `Lock`/`Unlock`.
/// So it is treated as a *hint*, cross-checked against logind's authoritative
/// pid-derived session ([`SessionResolver::get_session_by_pid`], which walks
/// cgroup membership and cannot be spoofed from the environment):
///
/// * the hint and the pid session agree — use the hint;
/// * they disagree — the hint is a redirect, log and use the pid session;
/// * the hint lookup fails — it is stale or malformed, so it is not a usable
///   session; log and fall through to the authoritative pid lookup;
/// * the pid lookup fails — it is inconclusive (a `systemd --user` service is
///   outside the login session's cgroup), so the hint is trusted rather than
///   making discovery fatal.
///
/// Discovery only fails when a lookup that is *required* on its path fails:
/// with no usable hint that is the pid lookup, and after a failed hint lookup
/// it is the pid lookup too (the hint is not usable as a fallback). A failed
/// hint lookup must never abort before the pid attempt.
///
/// An empty string is treated as absent (a misconfigured or blank
/// `$XDG_SESSION_ID` must not be handed to `GetSession`), falling straight
/// through to the pid path.
pub fn resolve_session<C: SessionResolver + ?Sized>(
    conn: &C,
    xdg_session_id: Option<&str>,
    pid: u32,
) -> zbus::Result<OwnedObjectPath> {
    match xdg_session_id.filter(|id| !id.is_empty()) {
        Some(session_id) => {
            let hinted = match conn.get_session(session_id) {
                Ok(hinted) => hinted,
                Err(err) => {
                    tracing::warn!(
                        %err,
                        session_id,
                        "XDG_SESSION_ID lookup failed; falling back to the pid-derived session"
                    );
                    return conn.get_session_by_pid(pid);
                }
            };
            match conn.get_session_by_pid(pid) {
                Ok(actual) if actual != hinted => {
                    tracing::warn!(
                        hinted = %hinted,
                        actual = %actual,
                        "XDG_SESSION_ID resolves to a different session than this process's; \
                         ignoring the untrusted hint"
                    );
                    Ok(actual)
                }
                _ => Ok(hinted),
            }
        }
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
        let conn = zbus::blocking::connection::Builder::system()?
            .method_timeout(LOGIND_CALL_TIMEOUT)
            .build()?;
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
            .suspend(POWER_ACTION_INTERACTIVE)
            .map_err(io::Error::other)
    }

    fn hibernate(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .hibernate(POWER_ACTION_INTERACTIVE)
            .map_err(io::Error::other)
    }

    fn power_off(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .power_off(POWER_ACTION_INTERACTIVE)
            .map_err(io::Error::other)
    }

    fn reboot(&self) -> io::Result<()> {
        self.manager()
            .map_err(io::Error::other)?
            .reboot(POWER_ACTION_INTERACTIVE)
            .map_err(io::Error::other)
    }
}

/// One recorded interaction with the [`Logind`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogindCall {
    GetSession(String),
    GetSessionByPid(u32),
    /// `who` is deliberately absent: it is the production constant
    /// [`INHIBIT_WHO`], not a caller argument, so recording it here would only
    /// re-assert the constant the seam test already reads. The wire-level test
    /// in `tests/fake_logind.rs` pins the actual `who` sent to logind.
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

/// A test hook invoked synchronously when [`RecordingLogind::lock`] runs,
/// standing in for the `Lock` signal logind would emit on the bus. Kept
/// `Debug` so the double's derives are unchanged.
#[derive(Clone)]
struct LockEcho(Arc<dyn Fn() + Send + Sync>);

impl std::fmt::Debug for LockEcho {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LockEcho")
    }
}

/// A [`Logind`] test double: records every call in order and answers with a
/// configured session path. Every policy/discovery decision is testable
/// against this with no real D-Bus.
#[derive(Debug, Clone)]
pub struct RecordingLogind {
    session_path: OwnedObjectPath,
    /// What [`SessionResolver::get_session_by_pid`] answers. Equal to
    /// `session_path` unless [`Self::with_paths`] deliberately diverges them,
    /// so the untrusted-env-hint cross-check in [`resolve_session`] is
    /// testable.
    pid_session_path: OwnedObjectPath,
    calls: Arc<Mutex<Vec<LogindCall>>>,
    /// The read end of every pipe whose *write* end [`Logind::inhibit`] handed
    /// out, so [`Self::live_inhibitors`] can tell whether the caller's guard
    /// still holds its end open. Read, never written: a non-blocking read
    /// returns `0` (EOF) exactly when every write end is closed, which is
    /// deterministic where probing the write side with `write_all` was not
    /// (the OS may still buffer a write, and a sibling fd can keep the pipe
    /// from ever reporting `EPIPE`).
    inhibitors: Arc<Mutex<Vec<File>>>,
    /// Optional `Lock` signal echo: invoked after recording a [`Logind::lock`]
    /// call, so a test can drive the single funnel (`LockFlow::on_lock`) the
    /// way the real logind signal loop does.
    lock_echo: Arc<Mutex<Option<LockEcho>>>,
    /// When set, [`SessionResolver::get_session`] fails with this message,
    /// modelling a stale or malformed `$XDG_SESSION_ID`.
    hint_lookup_error: Option<String>,
    /// When set, [`SessionResolver::get_session_by_pid`] fails with this
    /// message, modelling a `systemd --user` service outside the login
    /// session's cgroup.
    pid_lookup_error: Option<String>,
}

impl RecordingLogind {
    /// Build a recording double whose session resolves to `session_path`.
    pub fn new(session_path: &str) -> Self {
        Self::with_paths(session_path, session_path)
    }

    /// Build a double whose `$XDG_SESSION_ID` lookup answers `hinted` and whose
    /// pid fallback answers `pid_path`, so [`resolve_session`]'s redirect check
    /// can be exercised.
    pub fn with_paths(hinted_path: &str, pid_path: &str) -> Self {
        let session_path = OwnedObjectPath::try_from(hinted_path)
            .expect("a logind session path is a valid D-Bus object path");
        let pid_session_path = OwnedObjectPath::try_from(pid_path)
            .expect("a logind session path is a valid D-Bus object path");
        Self {
            session_path,
            pid_session_path,
            calls: Arc::new(Mutex::new(Vec::new())),
            inhibitors: Arc::new(Mutex::new(Vec::new())),
            lock_echo: Arc::new(Mutex::new(None)),
            hint_lookup_error: None,
            pid_lookup_error: None,
        }
    }

    /// Make `$XDG_SESSION_ID`'s [`SessionResolver::get_session`] lookup fail
    /// with `message`.
    #[must_use]
    pub fn with_hint_lookup_error(mut self, message: impl Into<String>) -> Self {
        self.hint_lookup_error = Some(message.into());
        self
    }

    /// Make the pid-derived [`SessionResolver::get_session_by_pid`] lookup fail
    /// with `message`.
    #[must_use]
    pub fn with_pid_lookup_error(mut self, message: impl Into<String>) -> Self {
        self.pid_lookup_error = Some(message.into());
        self
    }

    /// Register the `Lock` signal echo invoked synchronously from
    /// [`Logind::lock`]. The real logind emits the signal on the bus and the
    /// daemon's signal loop calls `events.lock()`; this models that without a
    /// bus, so the funnel can be driven in tests.
    pub fn set_lock_echo(&self, echo: impl Fn() + Send + Sync + 'static) {
        *self.lock_echo.lock().unwrap_or_else(|e| e.into_inner()) = Some(LockEcho(Arc::new(echo)));
    }

    /// Every call recorded so far, in order.
    pub fn calls(&self) -> Vec<LogindCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many inhibitor fds [`Logind::inhibit`] handed out are still open.
    /// The read end of each returned pipe is retained; a non-blocking read
    /// reports EOF (`Ok(0)`) exactly when the caller's guard has dropped its
    /// write end, so a closed guard is observed deterministically rather than
    /// relying on a write-side probe.
    pub fn live_inhibitors(&self) -> usize {
        use std::os::fd::AsFd as _;
        // A pipe's read end reports `POLLHUP` exactly when every write end is
        // closed, which is the guard-dropped state -- deterministic, unlike a
        // write-side probe whose result depends on buffering and on whether
        // any other copy of the fd is open.
        let mut live = self.inhibitors.lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|read| {
            let borrowed = read.as_fd();
            let mut fds = [rustix::event::PollFd::new(
                &borrowed,
                rustix::event::PollFlags::HUP,
            )];
            let zero = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            match rustix::event::poll(&mut fds, Some(&zero)) {
                Ok(_) => !fds[0].revents().contains(rustix::event::PollFlags::HUP),
                // A poll error is treated as "still held" so a broken probe
                // cannot report a live guard as released.
                Err(_) => true,
            }
        });
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
        if let Some(message) = &self.hint_lookup_error {
            return Err(zbus::Error::Failure(message.clone()));
        }
        Ok(self.session_path.clone())
    }

    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath> {
        self.record(LogindCall::GetSessionByPid(pid));
        if let Some(message) = &self.pid_lookup_error {
            return Err(zbus::Error::Failure(message.clone()));
        }
        Ok(self.pid_session_path.clone())
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
        // Keep the *read* end and hand the caller the write end:
        // `live_inhibitors` reads it and sees EOF once the guard's write end
        // is closed, which is the deterministic direction (a write-side probe
        // can be buffered and is defeated by any other copy of the fd).
        self.inhibitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(File::from(OwnedFd::from(read)));
        Ok(write.into())
    }

    fn lock(&self) -> io::Result<()> {
        self.record(LogindCall::Lock);
        let echo = self
            .lock_echo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(echo) = echo {
            (echo.0)();
        }
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

/// How many *consecutive* signal-loop failures the supervisor tolerates before
/// it gives up and takes the process down.
const SIGNAL_LOOP_MAX_ATTEMPTS: u32 = 5;
/// First delay between signal-loop re-subscription attempts.
const SIGNAL_LOOP_BACKOFF: Duration = Duration::from_millis(500);
/// Ceiling on the signal-loop re-subscription backoff.
const SIGNAL_LOOP_BACKOFF_MAX: Duration = Duration::from_secs(8);
/// A subscription that stays up at least this long is healthy: it clears the
/// consecutive-failure count and the backoff, so unrelated transient drops
/// spread across a long-lived daemon never accumulate into the exit cap.
const SIGNAL_LOOP_RESET_AFTER: Duration = Duration::from_secs(30);

/// Tuning for [`supervise_with`]. Production uses [`SUPERVISE_POLICY`]; tests
/// use a zero-backoff policy so the loop runs without real waits.
#[derive(Debug, Clone, Copy)]
struct SupervisePolicy {
    /// Consecutive failed runs tolerated before `exit` is called.
    max_attempts: u32,
    /// Delay before the first re-subscription attempt.
    backoff: Duration,
    /// Ceiling on the exponential backoff.
    backoff_max: Duration,
    /// A run that stayed up at least this long resets the count.
    reset_after: Duration,
}

/// The production supervision policy.
const SUPERVISE_POLICY: SupervisePolicy = SupervisePolicy {
    max_attempts: SIGNAL_LOOP_MAX_ATTEMPTS,
    backoff: SIGNAL_LOOP_BACKOFF,
    backoff_max: SIGNAL_LOOP_BACKOFF_MAX,
    reset_after: SIGNAL_LOOP_RESET_AFTER,
};

/// Subscribe to logind's `PrepareForSleep` and this session's `Lock`/`Unlock`
/// on a dedicated thread (`zbus::block_on`, no tokio), forwarding each to
/// `events`.
///
/// The thread is supervised, not detached into silence: whenever
/// [`run_signal_loop`] returns — including a *clean* stream end, which would
/// otherwise leave the process alive and registered while the only
/// locker-spawn site (the `Lock` handler) can never run — the loop logs an
/// error, re-subscribes with exponential backoff, and after
/// [`SIGNAL_LOOP_MAX_ATTEMPTS`] *consecutive* failures calls
/// [`std::process::exit`] so the systemd unit's `Restart=always` recovers the
/// daemon. The returned [`JoinHandle`] is for the caller to monitor; the
/// thread itself exits the process rather than completing.
pub fn spawn_signal_loop(
    session_path: OwnedObjectPath,
    events: SharedLogindEvents,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("icedtea-session-logind".to_string())
        .spawn(move || supervise_signal_loop(session_path, events))
}

/// Keep [`run_signal_loop`] subscribed, or terminate the process so systemd
/// restarts it. Never returns.
fn supervise_signal_loop(session_path: OwnedObjectPath, events: SharedLogindEvents) -> ! {
    supervise_with(
        SUPERVISE_POLICY,
        || zbus::block_on(run_signal_loop(&session_path, events.clone())),
        exit_process,
    );
    unreachable!("supervise_with returns only after calling exit, which does not return")
}

/// [`std::process::exit`] behind a `()`-returning signature so it fits the
/// `exit` seam [`supervise_with`] exposes to tests.
fn exit_process(code: i32) {
    std::process::exit(code);
}

/// Run `run` under `policy`, calling `exit` when it cannot stay subscribed.
///
/// `run` returns when one subscription ends: `Ok` for a clean stream end,
/// `Err` for a failed subscription. Either way the daemon is no longer
/// receiving signals, so both count as a fault. A run that lasted at least
/// `policy.reset_after` is healthy and resets the consecutive-failure count, so
/// the cap measures *consecutive* failures rather than a daemon-lifetime total.
///
/// Split from [`supervise_signal_loop`] so the whole decision — including the
/// `process::exit` — is unit-testable without a system bus or a real process
/// exit.
fn supervise_with(
    policy: SupervisePolicy,
    mut run: impl FnMut() -> zbus::Result<()>,
    mut exit: impl FnMut(i32),
) {
    let mut attempt = 0u32;
    let mut backoff = policy.backoff;
    loop {
        let started = std::time::Instant::now();
        let result = run();
        if started.elapsed() >= policy.reset_after {
            if attempt > 0 {
                tracing::info!(
                    attempt,
                    "logind signal subscription recovered; resetting the failure count"
                );
            }
            attempt = 0;
            backoff = policy.backoff;
        }
        attempt += 1;
        match result {
            Ok(()) => tracing::error!(
                attempt,
                "logind signal stream ended cleanly while the daemon is still registered; re-subscribing"
            ),
            Err(err) => tracing::error!(
                attempt,
                %err,
                "logind signal subscription failed; re-subscribing"
            ),
        }
        if attempt >= policy.max_attempts {
            tracing::error!(
                attempt,
                "logind signal loop could not stay subscribed; exiting so the service manager restarts it"
            );
            exit(1);
            return;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(policy.backoff_max);
    }
}

async fn run_signal_loop(
    session_path: &OwnedObjectPath,
    events: SharedLogindEvents,
) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    run_signal_loop_on(conn, session_path, events).await
}

/// [`run_signal_loop`] over a caller-supplied connection. Production passes a
/// system-bus connection; the fake-logind integration test passes a private
/// session bus so the dispatch of real `PrepareForSleep`/`Lock`/`Unlock`
/// signals is exercised end to end without touching `systemd-logind`.
pub async fn run_signal_loop_on(
    conn: zbus::Connection,
    session_path: &OwnedObjectPath,
    events: SharedLogindEvents,
) -> zbus::Result<()> {
    use futures_util::StreamExt as _;

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
    fn resolve_session_trusts_the_xdg_hint_when_the_pid_agrees() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c7");
        let path = resolve_session(&logind, Some("c7"), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c7");
        // The hint is cross-checked against the authoritative pid session.
        assert_eq!(
            logind.calls(),
            vec![
                LogindCall::GetSession("c7".to_string()),
                LogindCall::GetSessionByPid(4242),
            ]
        );
    }

    #[test]
    fn resolve_session_falls_back_to_the_pid_without_an_id() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let path = resolve_session(&logind, None, 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(logind.calls(), vec![LogindCall::GetSessionByPid(4242)]);
    }

    /// A same-user actor can point `$XDG_SESSION_ID` at another session. The
    /// pid-derived session is authoritative, so the redirect is dropped.
    #[test]
    fn resolve_session_ignores_a_hint_that_points_at_a_different_session() {
        let logind = RecordingLogind::with_paths(
            "/org/freedesktop/login1/session/attacker",
            "/org/freedesktop/login1/session/c1",
        );
        let path = resolve_session(&logind, Some("attacker"), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(
            logind.calls(),
            vec![
                LogindCall::GetSession("attacker".to_string()),
                LogindCall::GetSessionByPid(4242),
            ]
        );
    }

    #[test]
    fn resolve_session_treats_an_empty_xdg_session_id_as_absent() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        let path = resolve_session(&logind, Some(""), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(logind.calls(), vec![LogindCall::GetSessionByPid(4242)]);
    }

    /// A stale or malformed `$XDG_SESSION_ID` must not make discovery fatal.
    /// The failed hint lookup falls through to the authoritative pid session.
    #[test]
    fn resolve_session_falls_back_to_the_pid_when_the_hint_lookup_fails() {
        let logind = RecordingLogind::with_paths(
            "/org/freedesktop/login1/session/attacker",
            "/org/freedesktop/login1/session/c1",
        )
        .with_hint_lookup_error("NoSuchSession");
        let path = resolve_session(&logind, Some("attacker"), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(
            logind.calls(),
            vec![
                LogindCall::GetSession("attacker".to_string()),
                LogindCall::GetSessionByPid(4242),
            ],
            "the pid lookup must still be attempted after the hint lookup fails"
        );
    }

    /// A `systemd --user` service is outside the login session's cgroup, so its
    /// pid lookup is inconclusive. The untrusted hint is then the only signal
    /// and is trusted rather than making discovery fatal.
    #[test]
    fn resolve_session_trusts_the_hint_when_the_pid_lookup_fails() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1")
            .with_pid_lookup_error("NoSuchProcess");
        let path = resolve_session(&logind, Some("c1"), 4242).expect("resolves");
        assert_eq!(path.as_str(), "/org/freedesktop/login1/session/c1");
        assert_eq!(
            logind.calls(),
            vec![
                LogindCall::GetSession("c1".to_string()),
                LogindCall::GetSessionByPid(4242),
            ]
        );
    }

    /// Both lookups failing is the only fatal discovery path when a hint is set.
    #[test]
    fn resolve_session_is_fatal_when_the_hint_and_pid_lookups_both_fail() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1")
            .with_hint_lookup_error("NoSuchSession")
            .with_pid_lookup_error("NoSuchProcess");
        let err = resolve_session(&logind, Some("c1"), 4242).expect_err("both lookups fail");
        assert!(matches!(err, zbus::Error::Failure(message) if message == "NoSuchProcess"));
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

    /// The `who` string and the non-interactive power flag are both pinned:
    /// the fake-logind integration test checks them on the actual wire, and
    /// this keeps the constants themselves honest.
    #[test]
    fn inhibit_who_and_power_interactive_are_the_reviewed_values() {
        assert_eq!(INHIBIT_WHO, "icedtea-session");
        // `as u8` rather than `!POWER_ACTION_INTERACTIVE` so clippy's
        // `assertions_on_constants` does not fire on the pin.
        assert_eq!(POWER_ACTION_INTERACTIVE as u8, 0);
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

    /// Test policy with no real backoff waits, so the supervisor can be driven
    /// to its exit in-process. `reset_after` is set high unless a test wants to
    /// exercise the healthy-run reset.
    fn test_policy(max_attempts: u32, reset_after: Duration) -> SupervisePolicy {
        SupervisePolicy {
            max_attempts,
            backoff: Duration::ZERO,
            backoff_max: Duration::ZERO,
            reset_after,
        }
    }

    /// A runner that always errors must be retried exactly
    /// [`SIGNAL_LOOP_MAX_ATTEMPTS`] times and then exit the process once.
    #[test]
    fn supervise_with_exits_after_the_maximum_consecutive_failures() {
        let mut runs = 0u32;
        let mut exits = Vec::new();
        supervise_with(
            test_policy(5, Duration::from_secs(3600)),
            || {
                runs += 1;
                Err(zbus::Error::Failure("stream failed".to_string()))
            },
            |code| exits.push(code),
        );
        assert_eq!(runs, 5, "the runner is retried up to the cap");
        assert_eq!(exits, vec![1], "the process is exited exactly once");
    }

    /// A clean stream end is a fault too: the daemon is still registered but
    /// can no longer receive signals.
    #[test]
    fn supervise_with_treats_a_clean_stream_end_as_a_fault() {
        let mut runs = 0u32;
        let mut exits = Vec::new();
        supervise_with(
            test_policy(5, Duration::from_secs(3600)),
            || {
                runs += 1;
                Ok(())
            },
            |code| exits.push(code),
        );
        assert_eq!(runs, 5);
        assert_eq!(exits, vec![1]);
    }

    /// A subscription that survives `reset_after` is healthy and clears the
    /// consecutive-failure count, so two early transient drops plus the later
    /// streak is two events, not one cumulative five. Without the reset the
    /// third (healthy) run would already reach the cap and exit.
    #[test]
    fn supervise_with_resets_the_count_after_a_healthy_run() {
        let mut runs = 0u32;
        let mut exits = Vec::new();
        supervise_with(
            test_policy(3, Duration::from_millis(50)),
            || {
                runs += 1;
                if runs == 3 {
                    // Stay up past the health threshold before dropping, then
                    // drop immediately on the runs after it. The sleep is
                    // measured against a real monotonic clock, so it guarantees
                    // the reset regardless of scheduling.
                    std::thread::sleep(Duration::from_millis(80));
                }
                Err(zbus::Error::Failure("stream failed".to_string()))
            },
            |code| exits.push(code),
        );
        assert_eq!(
            runs, 5,
            "two failures, a healthy run that resets, then three consecutive failures"
        );
        assert_eq!(exits, vec![1]);
    }
}
