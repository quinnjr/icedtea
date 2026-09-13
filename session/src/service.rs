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

use std::io;
use std::sync::Arc;

use zbus::blocking::Connection;
use zbus::interface;

use crate::logind::Logind;
use crate::wm_client::WmClient;

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
}

impl SessionInterface {
    /// Build the interface from its two trait seams.
    pub fn new(logind: SharedLogind, wm: SharedWm) -> Self {
        Self { logind, wm }
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
    /// `Session.Lock()` on the login1 proxy.
    fn lock(&self) -> zbus::fdo::Result<()> {
        call_logind("lock", self.logind.lock())
    }

    /// `Manager.Suspend(false)`: `false` is logind's non-interactive flag (this
    /// call already comes from the authenticated session's own daemon; see
    /// `ZbusLogind`).
    fn suspend(&self) -> zbus::fdo::Result<()> {
        call_logind("suspend", self.logind.suspend())
    }

    /// `Manager.Hibernate(false)`.
    fn hibernate(&self) -> zbus::fdo::Result<()> {
        call_logind("hibernate", self.logind.hibernate())
    }

    /// `Manager.PowerOff(false)`.
    fn power_off(&self) -> zbus::fdo::Result<()> {
        call_logind("power_off", self.logind.power_off())
    }

    /// `Manager.Reboot(false)`.
    fn reboot(&self) -> zbus::fdo::Result<()> {
        call_logind("reboot", self.logind.reboot())
    }

    /// Forward to `org.icedtea.Compositor`'s `Quit` via [`WmClient`]. A `false`
    /// from `quit` (compositor unreachable) is an error reply, so the CLI's
    /// `logout` exits non-zero rather than reporting a logout that did not
    /// happen.
    fn log_out(&self) -> zbus::fdo::Result<()> {
        if self.wm.quit() {
            return Ok(());
        }
        tracing::warn!("org.icedtea.Session.LogOut failed: compositor unreachable");
        Err(zbus::fdo::Error::Failed(
            "log out failed: compositor unreachable".to_string(),
        ))
    }

    /// Mirror the compositor's `IsLocked` value.
    fn is_locked(&self) -> bool {
        self.wm.is_locked()
    }
}

/// Register `org.icedtea.Session` on the session bus. Returns the connection,
/// which the caller keeps alive for the service's life. `Err` if the session
/// bus is unavailable or the name is already owned (another daemon running) —
/// the binary treats that as fatal.
pub fn spawn(logind: SharedLogind, wm: SharedWm) -> zbus::Result<Connection> {
    let conn = Connection::session()?;
    conn.object_server()
        .at(SESSION_PATH, SessionInterface::new(logind, wm))?;
    conn.request_name(SESSION_BUS_NAME)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::fd::OwnedFd;
    use std::sync::Arc;

    use zbus::object_server::Interface as _;
    use zbus::zvariant::OwnedObjectPath;

    use super::*;
    use crate::logind::{Logind, LogindCall, RecordingLogind};
    use crate::wm_client::{RecordingWm, WmCall};

    fn service(locked: bool) -> (SessionInterface, Arc<RecordingLogind>, Arc<RecordingWm>) {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let wm = Arc::new(RecordingWm::new(locked));
        let iface = SessionInterface::new(logind.clone(), wm.clone());
        (iface, logind, wm)
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
        fn inhibit(&self, _what: &str, _why: &str) -> io::Result<OwnedFd> {
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
        iface.log_out().expect("recording wm accepts quit");
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
        match iface.log_out() {
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
    fn lock_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.lock().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Lock]);
    }

    #[test]
    fn suspend_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.suspend().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Suspend]);
    }

    #[test]
    fn hibernate_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.hibernate().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::Hibernate]);
    }

    #[test]
    fn power_off_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.power_off().expect("recording logind succeeds");
        assert_eq!(logind.calls(), vec![LogindCall::PowerOff]);
    }

    #[test]
    fn reboot_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.reboot().expect("recording logind succeeds");
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
            ("lock", iface.lock()),
            ("suspend", iface.suspend()),
            ("hibernate", iface.hibernate()),
            ("power_off", iface.power_off()),
            ("reboot", iface.reboot()),
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
}
