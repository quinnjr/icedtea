//! The `org.icedtea.Compositor` client seam.
//!
//! The session daemon reads the compositor's lock state and asks it to end the
//! session through [`WmClient`], so both actions are unit-testable with
//! [`RecordingWm`] and no session bus. [`ZbusWmClient`] is the production
//! adapter over `zbus::blocking::Connection::session()`, mirroring
//! `shell/src/compositor_client.rs`'s `CompositorProxy`.

use std::io;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_IFACE, COMPOSITOR_PATH};
use zbus::blocking::Connection;

/// Deadline on every method call this client makes.
///
/// `IsLocked` is polled inside the pre-sleep wait; without a bound a stalled
/// compositor would hold that wait — and therefore the sleep delay inhibitor —
/// open indefinitely, defeating the 3s budget. 750ms keeps the worst case
/// (`SLEEP_LOCK_BUDGET` + one in-flight call = 3.75s) under logind's 5s
/// `InhibitDelayMaxSec`, and is ample for a healthy compositor's reply.
/// `Quit` is bounded too: a wedged compositor must not hang the logout CLI.
pub const COMPOSITOR_CALL_TIMEOUT: Duration = Duration::from_millis(750);

/// Wire member for [`WmClient::is_locked`], kept named so the pin test below
/// can check it against the compositor's `IsLocked` introspection test
/// (`compositor/src/dbus.rs`'s `is_locked_is_exposed_as_is_locked_with_bool_out`).
const IS_LOCKED_MEMBER: &str = "IsLocked";

/// Wire member for [`WmClient::quit`], pinned the same way against
/// `compositor/src/dbus.rs`'s `Quit` introspection test.
const QUIT_MEMBER: &str = "Quit";

/// Wire member for the compositor's `SessionLockChanged(bool)` signal
/// (`(seq, locked)` on the wire, `compositor/src/dbus.rs`'s emitter arm).
/// Named so the subscription's match rule and the pin test below agree; the
/// real guard is the compositor's own signal/emitter test.
const SESSION_LOCK_CHANGED_MEMBER: &str = "SessionLockChanged";

/// A tri-state read of the compositor's lock state.
///
/// Unlike [`WmClient::is_locked`], which collapses every failure to `false`,
/// this distinguishes "the compositor answered unlocked" from "the compositor
/// could not be reached", so the pre-sleep wait can tell a slow locker from an
/// unreachable compositor instead of guessing from call latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// The compositor confirms the session is locked.
    Locked,
    /// The compositor answered that the session is unlocked.
    Unlocked,
    /// The compositor could not be reached, or its reply could not be decoded.
    Unreachable,
}

/// The compositor operations the session daemon needs, abstracted so the
/// service depends on behavior a test can record rather than a live session
/// bus.
pub trait WmClient {
    /// Whether the session is currently locked (`org.icedtea.Compositor`'s
    /// `IsLocked`). Every failure collapses to `false`, never a panic on a
    /// foreign dispatch thread.
    fn is_locked(&self) -> bool;
    /// A tri-state lock-state read. The provided default delegates to
    /// [`Self::is_locked`] (`true` → [`LockState::Locked`], `false` →
    /// [`LockState::Unlocked`]), so a double that does not model
    /// unreachability compiles unchanged; [`ZbusWmClient`] overrides it to
    /// report [`LockState::Unreachable`] on a call/decode failure.
    fn lock_state(&self) -> LockState {
        if self.is_locked() {
            LockState::Locked
        } else {
            LockState::Unlocked
        }
    }
    /// End the session through the compositor's `Quit` path. Returns whether
    /// the call was issued: a dead bus means the session is already going away
    /// (`false`), never a panic.
    fn quit(&self) -> bool;
    /// Subscribe to the compositor's `SessionLockChanged(bool)` signal,
    /// invoking `on_change(locked)` on a dedicated thread for every change.
    ///
    /// Provided (rather than required) so existing test doubles — including
    /// ones a sibling owns — compile unchanged; a double that does not model
    /// the subscription inherits the error-returning default. `ZbusWmClient`
    /// overrides it with the real signal-match loop. The daemon's `flow`
    /// should consume this to kill a still-running locker on an external
    /// unlock that arrives without a matching logind `Unlock`.
    fn subscribe_lock_changed(
        &self,
        _on_change: Box<dyn FnMut(bool) + Send + 'static>,
    ) -> io::Result<JoinHandle<()>> {
        Err(io::Error::other(
            "this WmClient does not support lock-change subscriptions",
        ))
    }
}

/// The production [`WmClient`] over the session bus.
pub struct ZbusWmClient {
    conn: Connection,
}

impl ZbusWmClient {
    /// Open the session bus with [`COMPOSITOR_CALL_TIMEOUT`] on every call
    /// made through it. `Err` if the bus is unavailable.
    pub fn new() -> zbus::Result<Self> {
        let conn = zbus::blocking::connection::Builder::session()?
            .method_timeout(COMPOSITOR_CALL_TIMEOUT)
            .build()?;
        Ok(Self { conn })
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

    fn lock_state(&self) -> LockState {
        let msg = match self.call(IS_LOCKED_MEMBER) {
            Ok(msg) => msg,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "IsLocked call failed; treating the compositor as unreachable"
                );
                return LockState::Unreachable;
            }
        };
        match msg.body().deserialize::<bool>() {
            Ok(true) => LockState::Locked,
            Ok(false) => LockState::Unlocked,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "IsLocked reply decode failed; treating the compositor as unreachable"
                );
                LockState::Unreachable
            }
        }
    }

    fn quit(&self) -> bool {
        self.call(QUIT_MEMBER).is_ok()
    }

    fn subscribe_lock_changed(
        &self,
        mut on_change: Box<dyn FnMut(bool) + Send + 'static>,
    ) -> io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("icedtea-session-wm-lock".to_string())
            .spawn(move || {
                zbus::block_on(async move {
                    if let Err(err) = run_lock_changed_loop(&mut *on_change).await {
                        // A subscription that cannot be re-established is
                        // logged but not fatal: unlike the logind loop, lock
                        // state is still observable through `is_locked`, so
                        // this daemon degrades to polling rather than dying.
                        tracing::error!(
                            %err,
                            "compositor SessionLockChanged subscription ended; lock-state changes \
                             will not be pushed"
                        );
                    }
                });
            })
    }
}

/// Forward every `org.icedtea.Compositor` `SessionLockChanged(bool)` signal to
/// `on_change` until the connection drops. The signal body is `(seq, locked)`
/// (matching the compositor emitter's `WorkspaceSet`-style shape).
async fn run_lock_changed_loop(on_change: &mut (dyn FnMut(bool) + Send)) -> zbus::Result<()> {
    use futures_util::StreamExt as _;

    let conn = zbus::Connection::session().await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(COMPOSITOR_IFACE)?
        .path(COMPOSITOR_PATH)?
        .member(SESSION_LOCK_CHANGED_MEMBER)?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    while let Some(message) = stream.next().await {
        let message = message?;
        if let Ok((_seq, locked)) = message.body().deserialize::<(u64, bool)>() {
            on_change(locked);
        }
    }
    Ok(())
}

/// One recorded interaction with the [`WmClient`] seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmCall {
    IsLocked,
    Quit,
    SubscribeLockChanged,
}

/// A stored lock-change subscriber, wrapped so the double stays `Debug`.
struct StoredCallback(Box<dyn FnMut(bool) + Send + 'static>);

impl std::fmt::Debug for StoredCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredCallback(..)")
    }
}

/// A [`WmClient`] test double: records every call in order and answers with a
/// configured lock state. The whole service surface is testable against this
/// with no real D-Bus. A subscribed lock-change callback is retained so a test
/// can deliver a compositor signal with [`RecordingWm::fire_lock_changed`].
#[derive(Debug, Clone)]
pub struct RecordingWm {
    locked: bool,
    quit_result: bool,
    calls: Arc<Mutex<Vec<WmCall>>>,
    lock_changed: Arc<Mutex<Option<StoredCallback>>>,
}

impl RecordingWm {
    /// Build a recording double that reports `locked` from `is_locked` and
    /// succeeds at `quit`.
    pub fn new(locked: bool) -> Self {
        Self::with_quit_result(locked, true)
    }

    /// Build a recording double whose `quit` returns `quit_result`, so the
    /// service's "compositor unreachable" path is testable.
    pub fn with_quit_result(locked: bool, quit_result: bool) -> Self {
        Self {
            locked,
            quit_result,
            calls: Arc::new(Mutex::new(Vec::new())),
            lock_changed: Arc::new(Mutex::new(None)),
        }
    }

    /// Every call recorded so far, in order.
    pub fn calls(&self) -> Vec<WmCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Deliver a compositor `SessionLockChanged(locked)` to the retained
    /// subscriber (if any); a no-op when nothing subscribed.
    pub fn fire_lock_changed(&self, locked: bool) {
        let mut guard = self.lock_changed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(callback) = guard.as_mut() {
            (callback.0)(locked);
        }
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
        self.quit_result
    }

    fn subscribe_lock_changed(
        &self,
        on_change: Box<dyn FnMut(bool) + Send + 'static>,
    ) -> io::Result<JoinHandle<()>> {
        self.record(WmCall::SubscribeLockChanged);
        *self.lock_changed.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(StoredCallback(on_change));
        Ok(std::thread::spawn(|| {}))
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

    #[test]
    fn recording_wm_can_refuse_quit() {
        let wm = RecordingWm::with_quit_result(false, false);
        assert!(!wm.quit());
        assert_eq!(wm.calls(), vec![WmCall::Quit]);
    }

    /// Reminder, not a guard. These literals are only checked against each
    /// other here; the real cross-check is the compositor's own introspection
    /// tests (`compositor/src/dbus.rs`'s `IsLocked`/`Quit` method tests and
    /// its `SessionLockChanged` emitter/signal-name test). If the two sides
    /// ever disagree, the proxy call reaches no method and silently degrades
    /// `is_locked` to `false` / the logout to nothing.
    #[test]
    fn wire_members_are_the_documented_literals() {
        assert_eq!(IS_LOCKED_MEMBER, "IsLocked");
        assert_eq!(QUIT_MEMBER, "Quit");
        assert_eq!(SESSION_LOCK_CHANGED_MEMBER, "SessionLockChanged");
    }

    /// The recording double advertises the subscription so a flow test can
    /// tell that the daemon wired it up, and returns a (detached) handle
    /// without a bus.
    #[test]
    fn recording_wm_records_the_lock_changed_subscription() {
        let wm = RecordingWm::new(false);
        let handle = wm
            .subscribe_lock_changed(Box::new(|_| {}))
            .expect("the double supports subscriptions");
        assert_eq!(wm.calls(), vec![WmCall::SubscribeLockChanged]);
        drop(handle);
    }

    /// The default `lock_state` maps the double's `is_locked` flag to
    /// `Locked`/`Unlocked`, so doubles that do not model unreachability keep
    /// working unchanged.
    #[test]
    fn recording_wm_default_lock_state_matches_its_locked_flag() {
        assert_eq!(RecordingWm::new(true).lock_state(), LockState::Locked);
        assert_eq!(RecordingWm::new(false).lock_state(), LockState::Unlocked);
    }

    /// A retained subscriber receives exactly the changes fired at it, in
    /// order, so a flow test can drive the subscription without a bus.
    #[test]
    fn recording_wm_delivers_fired_lock_changes_to_the_subscriber() {
        let wm = RecordingWm::new(false);
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let handle = wm
            .subscribe_lock_changed(Box::new(move |locked| {
                sink.lock().unwrap_or_else(|e| e.into_inner()).push(locked);
            }))
            .expect("the double supports subscriptions");

        wm.fire_lock_changed(true);
        wm.fire_lock_changed(false);

        assert_eq!(
            *got.lock().unwrap_or_else(|e| e.into_inner()),
            vec![true, false]
        );
        drop(handle);
    }
}
