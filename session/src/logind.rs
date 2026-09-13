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

use std::io;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

/// Named the daemon claims in the `who` field of every `Inhibit` call.
const INHIBIT_WHO: &str = "icedtea-session";

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
/// session's cgroup. With no id (an unusual/manual launch), the pid fallback
/// walks the cgroup membership, matching `loginctl`'s own "current session".
pub fn resolve_session<C: SessionResolver + ?Sized>(
    conn: &C,
    xdg_session_id: Option<&str>,
    pid: u32,
) -> zbus::Result<OwnedObjectPath> {
    match xdg_session_id {
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
    /// Take a `Manager.Inhibit(what, "icedtea-session", why, "delay")` fd.
    fn inhibit(&self, what: &str, why: &str) -> io::Result<OwnedFd>;
    fn lock(&self) -> io::Result<()>;
    fn unlock(&self) -> io::Result<()>;
    fn suspend(&self) -> io::Result<()>;
    fn hibernate(&self) -> io::Result<()>;
    fn power_off(&self) -> io::Result<()>;
    fn reboot(&self) -> io::Result<()>;
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

    fn inhibit(&self, what: &str, why: &str) -> io::Result<OwnedFd> {
        let fd = self
            .manager()
            .map_err(io::Error::other)?
            .inhibit(what, INHIBIT_WHO, why, "delay")
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
    Inhibit { what: String, why: String },
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
}

impl RecordingLogind {
    /// Build a recording double whose session resolves to `session_path`.
    pub fn new(session_path: &str) -> Self {
        let session_path = OwnedObjectPath::try_from(session_path)
            .expect("a logind session path is a valid D-Bus object path");
        Self {
            session_path,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every call recorded so far, in order.
    pub fn calls(&self) -> Vec<LogindCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
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

    fn inhibit(&self, what: &str, why: &str) -> io::Result<OwnedFd> {
        self.record(LogindCall::Inhibit {
            what: what.to_string(),
            why: why.to_string(),
        });
        let (read, write) = io::pipe()?;
        drop(write);
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
    fn recording_logind_records_every_trait_call_in_order() {
        let logind = RecordingLogind::new("/org/freedesktop/login1/session/c1");
        assert_eq!(
            logind.session_path().as_str(),
            "/org/freedesktop/login1/session/c1"
        );
        let _fd = logind
            .inhibit("sleep", "lock screen before suspend")
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
                    what: "sleep".to_string(),
                    why: "lock screen before suspend".to_string(),
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
}
