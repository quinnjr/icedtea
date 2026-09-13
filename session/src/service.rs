//! The `org.icedtea.Session` zbus service.
//!
//! Registered exactly like `clipboard/src/service.rs::spawn`: a
//! `zbus::blocking::Connection::session()`, `object_server().at(...)`, and
//! `request_name(...)` on the session bus. Every method answers from a trait
//! seam ([`Logind`] for lock/power, [`WmClient`] for the compositor's lock
//! state and `Quit`), so the whole surface is unit-testable with
//! `RecordingLogind`/`RecordingWm` and no bus.
//!
//! A method never panics on a foreign (D-Bus dispatch) thread: logind/power
//! errors are logged and dropped, and the compositor seam already collapses
//! every failure to `false`.

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

#[interface(name = "org.icedtea.Session")]
impl SessionInterface {
    /// `Session.Lock()` on the login1 proxy.
    fn lock(&self) {
        if let Err(err) = self.logind.lock() {
            tracing::warn!(%err, "org.icedtea.Session.Lock failed");
        }
    }

    /// `Manager.Suspend(false)`: `false` is logind's non-interactive flag (this
    /// call already comes from the authenticated session's own daemon; see
    /// `ZbusLogind`).
    fn suspend(&self) {
        if let Err(err) = self.logind.suspend() {
            tracing::warn!(%err, "org.icedtea.Session.Suspend failed");
        }
    }

    /// `Manager.Hibernate(false)`.
    fn hibernate(&self) {
        if let Err(err) = self.logind.hibernate() {
            tracing::warn!(%err, "org.icedtea.Session.Hibernate failed");
        }
    }

    /// `Manager.PowerOff(false)`.
    fn power_off(&self) {
        if let Err(err) = self.logind.power_off() {
            tracing::warn!(%err, "org.icedtea.Session.PowerOff failed");
        }
    }

    /// `Manager.Reboot(false)`.
    fn reboot(&self) {
        if let Err(err) = self.logind.reboot() {
            tracing::warn!(%err, "org.icedtea.Session.Reboot failed");
        }
    }

    /// Forward to `org.icedtea.Compositor`'s `Quit` via [`WmClient`].
    fn log_out(&self) {
        let _ = self.wm.quit();
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
    use std::sync::Arc;

    use zbus::object_server::Interface as _;

    use super::*;
    use crate::logind::{LogindCall, RecordingLogind};
    use crate::wm_client::{RecordingWm, WmCall};

    fn service(locked: bool) -> (SessionInterface, Arc<RecordingLogind>, Arc<RecordingWm>) {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let wm = Arc::new(RecordingWm::new(locked));
        let iface = SessionInterface::new(logind.clone(), wm.clone());
        (iface, logind, wm)
    }

    /// `LogOut()` must reach the compositor's `Quit()` exactly once — not zero
    /// (the session never ends) and not twice (a double logout).
    #[test]
    fn log_out_forwards_exactly_once_to_quit() {
        let (iface, _logind, wm) = service(false);
        iface.log_out();
        assert_eq!(wm.calls(), vec![WmCall::Quit]);
    }

    #[test]
    fn lock_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.lock();
        assert_eq!(logind.calls(), vec![LogindCall::Lock]);
    }

    #[test]
    fn suspend_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.suspend();
        assert_eq!(logind.calls(), vec![LogindCall::Suspend]);
    }

    #[test]
    fn hibernate_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.hibernate();
        assert_eq!(logind.calls(), vec![LogindCall::Hibernate]);
    }

    #[test]
    fn power_off_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.power_off();
        assert_eq!(logind.calls(), vec![LogindCall::PowerOff]);
    }

    #[test]
    fn reboot_calls_logind_once() {
        let (iface, logind, _wm) = service(false);
        iface.reboot();
        assert_eq!(logind.calls(), vec![LogindCall::Reboot]);
    }

    #[test]
    fn is_locked_mirrors_the_wm_client() {
        let (iface, _logind, _wm) = service(true);
        assert!(iface.is_locked());

        let (iface, _logind, _wm) = service(false);
        assert!(!iface.is_locked());
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
