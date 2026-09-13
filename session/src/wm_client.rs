//! The `org.icedtea.Compositor` client seam.
//!
//! The session daemon reads the compositor's lock state and asks it to end the
//! session through [`WmClient`], so both actions are unit-testable with
//! [`RecordingWm`] and no session bus. [`ZbusWmClient`] is the production
//! adapter over `zbus::blocking::Connection::session()`, mirroring
//! `shell/src/compositor_client.rs`'s `CompositorProxy`.

use std::sync::{Arc, Mutex};

use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_IFACE, COMPOSITOR_PATH};
use zbus::blocking::Connection;

/// Wire member for [`WmClient::is_locked`], kept named so the pin test below
/// can check it against the compositor's `IsLocked` introspection test
/// (`compositor/src/dbus.rs`'s `is_locked_is_exposed_as_is_locked_with_bool_out`).
const IS_LOCKED_MEMBER: &str = "IsLocked";

/// Wire member for [`WmClient::quit`], pinned the same way against
/// `compositor/src/dbus.rs`'s `Quit` introspection test.
const QUIT_MEMBER: &str = "Quit";

/// The compositor operations the session daemon needs, abstracted so the
/// service depends on behavior a test can record rather than a live session
/// bus.
pub trait WmClient {
    /// Whether the session is currently locked (`org.icedtea.Compositor`'s
    /// `IsLocked`). Every failure collapses to `false`, never a panic on a
    /// foreign dispatch thread.
    fn is_locked(&self) -> bool;
    /// End the session through the compositor's `Quit` path. Returns whether
    /// the call was issued: a dead bus means the session is already going away
    /// (`false`), never a panic.
    fn quit(&self) -> bool;
}

/// The production [`WmClient`] over the session bus.
pub struct ZbusWmClient {
    conn: Connection,
}

impl ZbusWmClient {
    /// Open the session bus. `Err` if it is unavailable.
    pub fn new() -> zbus::Result<Self> {
        Ok(Self {
            conn: Connection::session()?,
        })
    }

    fn call(&self, member: &str) -> zbus::Result<zbus::Message> {
        self.conn.call_method(
            Some(COMPOSITOR_BUS_NAME),
            COMPOSITOR_PATH,
            Some(COMPOSITOR_IFACE),
            member,
            &(),
        )
    }
}

impl WmClient for ZbusWmClient {
    fn is_locked(&self) -> bool {
        let msg = match self.call(IS_LOCKED_MEMBER) {
            Ok(msg) => msg,
            Err(err) => {
                tracing::warn!(?err, "IsLocked call failed; collapsing to false");
                return false;
            }
        };
        match msg.body().deserialize::<bool>() {
            Ok(locked) => locked,
            Err(err) => {
                tracing::warn!(?err, "IsLocked reply decode failed; collapsing to false");
                false
            }
        }
    }

    fn quit(&self) -> bool {
        self.call(QUIT_MEMBER).is_ok()
    }
}

/// One recorded interaction with the [`WmClient`] seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmCall {
    IsLocked,
    Quit,
}

/// A [`WmClient`] test double: records every call in order and answers with a
/// configured lock state. The whole service surface is testable against this
/// with no real D-Bus.
#[derive(Debug, Clone)]
pub struct RecordingWm {
    locked: bool,
    calls: Arc<Mutex<Vec<WmCall>>>,
}

impl RecordingWm {
    /// Build a recording double that reports `locked` from `is_locked`.
    pub fn new(locked: bool) -> Self {
        Self {
            locked,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every call recorded so far, in order.
    pub fn calls(&self) -> Vec<WmCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, call: WmCall) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(call);
    }
}

impl WmClient for RecordingWm {
    fn is_locked(&self) -> bool {
        self.record(WmCall::IsLocked);
        self.locked
    }

    fn quit(&self) -> bool {
        self.record(WmCall::Quit);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The double itself: it records what it was asked and answers with the
    /// state it was built with.
    #[test]
    fn recording_wm_reports_and_records_its_lock_state() {
        let wm = RecordingWm::new(true);
        assert!(wm.is_locked());
        assert_eq!(wm.calls(), vec![WmCall::IsLocked]);

        let wm = RecordingWm::new(false);
        assert!(!wm.is_locked());
    }

    #[test]
    fn recording_wm_records_quit_and_reports_it_issued() {
        let wm = RecordingWm::new(false);
        assert!(wm.quit());
        assert_eq!(wm.calls(), vec![WmCall::Quit]);
    }

    /// The wire members the proxy calls must stay identical to the members
    /// `compositor/src/dbus.rs` exposes (pinned on that side by the `IsLocked`
    /// and `Quit` introspection tests). A mismatch reaches no method and
    /// silently degrades `is_locked` to `false` / the logout to nothing.
    #[test]
    fn wire_members_match_the_compositor_interface() {
        assert_eq!(IS_LOCKED_MEMBER, "IsLocked");
        assert_eq!(QUIT_MEMBER, "Quit");
    }
}
