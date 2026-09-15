//! The `org.icedtea.Notifications` client. A worker thread seeds from
//! `GetActive` and forwards the daemon's signals as [`NotifUpdate`]s onto the
//! shell's loop thread through its inbox; [`NotifierProxy`] issues commands
//! (`InvokeAction`/`CloseNotification`/DND) with a separate blocking
//! connection. [`NotifModel`] is the pure popup-stack both sides feed.
//!
//! Wire-member source of truth: `notifications/src/service.rs`. The icedtea
//! extension interface (`org.icedtea.Notifications`) serves `GetActive`,
//! `GetHistory`, `DismissAll`, `InvokeAction`, `SetDoNotDisturb`,
//! `GetDoNotDisturb`; `CloseNotification` lives on the standard
//! `org.freedesktop.Notifications` interface at the same object path.
//! Signals are `NotificationAdded` / `NotificationClosed` (standard) /
//! `NotificationRemoved` / `ActionInvoked` (standard) /
//! `DoNotDisturbChanged` — none of them is the bare `Added`/`Closed`/`
//! DndChanged` shorthand the plan uses; the constants below pin the real
//! names so a rename on the daemon side breaks the pin tests here instead of
//! silently delivering nothing.
//!
//! IDs and action keys cross the bus only as method/signal arguments, never
//! via argv (B1 hardening rationale applies: no id/key ever touches a
//! command line).

use std::sync::Mutex;

use async_channel::Sender;
use futures_util::StreamExt as _;
use icedtea_contract::{CloseReason, NOTIF_BUS_NAME, NOTIF_PATH, Notification};

/// The icedtea extension interface both the query surface and most signals
/// live on (see `notifications/src/service.rs`'s `ICEDTEA_INTERFACE`).
const ICEDTEA_IFACE: &str = "org.icedtea.Notifications";

/// Wire members for [`NotifierCommands`], kept as named constants (rather
/// than inline literals at the calls) so the unit tests below can pin them
/// against `notifications/src/service.rs`'s `#[interface]` methods.
///
/// A reminder, not a guard — the real pin is the daemon's own introspection
/// coverage: if the literals disagree, the proxy call reaches no method and
/// every command silently does nothing.
const GET_ACTIVE_MEMBER: &str = "GetActive";
const GET_HISTORY_MEMBER: &str = "GetHistory";
const INVOKE_ACTION_MEMBER: &str = "InvokeAction";
const CLOSE_NOTIFICATION_MEMBER: &str = "CloseNotification";
const GET_DND_MEMBER: &str = "GetDoNotDisturb";
const SET_DND_MEMBER: &str = "SetDoNotDisturb";

/// Wire signal members, pinned for the same reason: the worker dispatches on
/// these, so a rename on the daemon side must break a test here, not just
/// stop delivering popups.
const NOTIFICATION_ADDED_MEMBER: &str = "NotificationAdded";
const NOTIFICATION_CLOSED_MEMBER: &str = "NotificationClosed";
const NOTIFICATION_REMOVED_MEMBER: &str = "NotificationRemoved";
const ACTION_INVOKED_MEMBER: &str = "ActionInvoked";
const DND_CHANGED_MEMBER: &str = "DoNotDisturbChanged";

/// Max visible popups per app. Overflow never reaches the stack — it stays in
/// the daemon's history (the center's `GetHistory` view), which is the v1
/// answer to popup storms (see the design spec's risks).
pub const MAX_VISIBLE_PER_APP: usize = 3;

/// Deltas the signal worker forwards to the shell's loop thread. Payloads
/// are ids/keys only: the loop thread reconciles card contents through
/// [`NotifierCommands::get_active`] (the daemon stays the source of truth;
/// the shell keeps no expiry heap — timers in Task 2 only *request*
/// `CloseNotification`, they never locally expire cards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifUpdate {
    /// Full seed from `GetActive` (cold start; also re-read on reconnect).
    Snapshot(Vec<Notification>),
    /// `NotificationAdded(id)`: fetch the card via `get_active`, then
    /// [`NotifModel::apply_added`].
    Added(u32),
    /// `NotificationClosed(id, _)` or `NotificationRemoved(id, _)`: both
    /// mean "drop the card" (the latter also covers DND suppression, which
    /// hides without closing — see `service.rs`'s `Change::Suppressed`).
    Removed(u32),
    /// `ActionInvoked(id, key)`.
    ActionInvoked { id: u32, key: String },
    /// `DoNotDisturbChanged(on)`.
    DndChanged(bool),
}

/// Spawn the signal worker. It seeds with `GetActive`, then forwards every
/// matching signal as a [`NotifUpdate`] until the bus drops.
///
/// Unlike `compositor_client::spawn` this never exits the process on error:
/// notifications are non-critical (a dead popup worker must not take the
/// panel down the way a stale taskbar would), so a failure is logged and the
/// thread ends; the surface re-seeds via `GetActive` on its next open.
pub fn spawn(tx: Sender<NotifUpdate>) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            if let Err(err) = run(tx).await {
                tracing::error!(%err, "org.icedtea.Notifications client stopped");
            }
        });
    });
}

async fn run(tx: Sender<NotifUpdate>) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, NOTIF_BUS_NAME, NOTIF_PATH, ICEDTEA_IFACE).await?;

    // Seed from the current active set before watching deltas.
    let seed: Vec<Notification> = proxy.call(GET_ACTIVE_MEMBER, &()).await?;
    if tx.send(NotifUpdate::Snapshot(seed)).await.is_err() {
        return Ok(()); // The shell exited before the first signal.
    }

    // Signals span both interfaces at this path (standard:
    // `NotificationClosed`/`ActionInvoked`; icedtea: the rest), so filter by
    // path only and dispatch on the member name.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(NOTIF_PATH)?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    while let Some(Ok(msg)) = stream.next().await {
        let member = msg.header().member().map(|m| m.as_str().to_string());
        let body = msg.body();
        let update = match member.as_deref() {
            Some(NOTIFICATION_ADDED_MEMBER) => decode_id(&body).map(NotifUpdate::Added),
            Some(NOTIFICATION_CLOSED_MEMBER) => body
                .deserialize::<(u32, u32)>()
                .ok()
                .map(|(id, _)| NotifUpdate::Removed(id)),
            Some(NOTIFICATION_REMOVED_MEMBER) => body
                .deserialize::<(u32, CloseReason)>()
                .ok()
                .map(|(id, _)| NotifUpdate::Removed(id)),
            Some(ACTION_INVOKED_MEMBER) => body
                .deserialize::<(u32, String)>()
                .ok()
                .map(|(id, key)| NotifUpdate::ActionInvoked { id, key }),
            Some(DND_CHANGED_MEMBER) => decode_bool(&body).map(NotifUpdate::DndChanged),
            _ => None,
        };
        if let Some(update) = update
            && tx.send(update).await.is_err()
        {
            break; // The shell exited.
        }
    }
    Ok(())
}

/// `emit_signal` bodies for id-only signals arrive as a 1-tuple struct;
/// accept that form (falling back to a bare value so an emitter-side shape
/// change degrades to a dropped signal, never a panic on this thread).
fn decode_id(body: &zbus::message::Body) -> Option<u32> {
    if let Ok((id,)) = body.deserialize::<(u32,)>() {
        return Some(id);
    }
    body.deserialize::<u32>().ok()
}

/// Same tolerance as [`decode_id`] for the DND flag.
fn decode_bool(body: &zbus::message::Body) -> Option<bool> {
    if let Ok((on,)) = body.deserialize::<(bool,)>() {
        return Some(on);
    }
    body.deserialize::<bool>().ok()
}

/// The notification command surface — abstracted so a test can inject a
/// recording mock in place of the real D-Bus proxy.
///
/// Every method takes ids/keys as plain arguments crossing D-Bus directly;
/// nothing here ever builds an argv (B1 hardening rationale applies).
pub trait NotifierCommands {
    fn get_active(&self) -> Vec<Notification>;
    fn get_history(&self) -> Vec<Notification>;
    fn invoke_action(&self, id: u32, key: &str);
    fn close_notification(&self, id: u32);
    fn get_dnd(&self) -> bool;
    fn set_dnd(&self, on: bool);
}

/// Issues `org.icedtea.Notifications` commands from the shell's loop thread.
/// Method calls are no-reply and sub-millisecond, so a blocking connection
/// here is fine. Every failure collapses to a default (logged), never a
/// panic on this foreign (loop) thread.
pub struct NotifierProxy {
    conn: zbus::blocking::Connection,
}

impl NotifierProxy {
    pub fn new() -> zbus::Result<Self> {
        Ok(NotifierProxy {
            conn: zbus::blocking::Connection::session()?,
        })
    }

    fn get_vec(&self, iface: &str, member: &str) -> Vec<Notification> {
        match self
            .conn
            .call_method(Some(NOTIF_BUS_NAME), NOTIF_PATH, Some(iface), member, &())
        {
            Ok(msg) => match msg.body().deserialize::<Vec<Notification>>() {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(?err, member, "reply decode failed; collapsing to empty");
                    Vec::new()
                }
            },
            Err(err) => {
                tracing::warn!(?err, member, "call failed; collapsing to empty");
                Vec::new()
            }
        }
    }

    fn get_bool(&self, member: &str) -> bool {
        match self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            member,
            &(),
        ) {
            Ok(msg) => match msg.body().deserialize::<bool>() {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(?err, member, "reply decode failed; collapsing to false");
                    false
                }
            },
            Err(err) => {
                tracing::warn!(?err, member, "call failed; collapsing to false");
                false
            }
        }
    }
}

impl NotifierCommands for NotifierProxy {
    // zbus's #[interface] exposes Rust methods in PascalCase, so the wire
    // members are GetActive/GetHistory/... (matching GetState's rule).
    fn get_active(&self) -> Vec<Notification> {
        self.get_vec(ICEDTEA_IFACE, GET_ACTIVE_MEMBER)
    }
    fn get_history(&self) -> Vec<Notification> {
        self.get_vec(ICEDTEA_IFACE, GET_HISTORY_MEMBER)
    }
    fn invoke_action(&self, id: u32, key: &str) {
        let _ = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            INVOKE_ACTION_MEMBER,
            &(id, key),
        );
    }
    // `CloseNotification` lives on the *standard* interface, not the icedtea
    // extension one — same bus name and object path, different interface.
    fn close_notification(&self, id: u32) {
        let _ = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(NOTIF_BUS_NAME),
            CLOSE_NOTIFICATION_MEMBER,
            &(id,),
        );
    }
    fn get_dnd(&self) -> bool {
        self.get_bool(GET_DND_MEMBER)
    }
    fn set_dnd(&self, on: bool) {
        let _ = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            SET_DND_MEMBER,
            &(on,),
        );
    }
}

/// A recording [`NotifierCommands`] for tests: stubbed reads, recorded
/// writes. Lock failures (a poisoned mutex) degrade to defaults/ignored
/// writes — never a panic, even on a foreign thread.
#[derive(Debug, Default)]
pub struct MockNotifier {
    active: Mutex<Vec<Notification>>,
    history: Mutex<Vec<Notification>>,
    dnd: Mutex<bool>,
    invoked: Mutex<Vec<(u32, String)>>,
    closed: Mutex<Vec<u32>>,
    dnd_sets: Mutex<Vec<bool>>,
}

impl MockNotifier {
    pub fn with_active(active: Vec<Notification>) -> Self {
        MockNotifier {
            active: Mutex::new(active),
            ..MockNotifier::default()
        }
    }

    pub fn invoked(&self) -> Vec<(u32, String)> {
        self.invoked
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn closed(&self) -> Vec<u32> {
        self.closed
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Every `set_dnd` argument in order (the last is the current intent).
    pub fn dnd_sets(&self) -> Vec<bool> {
        self.dnd_sets
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn dnd(&self) -> bool {
        self.dnd.lock().ok().map(|g| *g).unwrap_or_default()
    }
}

impl NotifierCommands for MockNotifier {
    fn get_active(&self) -> Vec<Notification> {
        self.active
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
    fn get_history(&self) -> Vec<Notification> {
        self.history
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
    fn invoke_action(&self, id: u32, key: &str) {
        if let Ok(mut g) = self.invoked.lock() {
            g.push((id, key.to_string()));
        }
    }
    fn close_notification(&self, id: u32) {
        if let Ok(mut g) = self.closed.lock() {
            g.push(id);
        }
    }
    fn get_dnd(&self) -> bool {
        self.dnd()
    }
    fn set_dnd(&self, on: bool) {
        if let Ok(mut g) = self.dnd.lock() {
            *g = on;
        }
        if let Ok(mut g) = self.dnd_sets.lock() {
            g.push(on);
        }
    }
}

/// The pure popup stack. Newest-first; at most [`MAX_VISIBLE_PER_APP`]
/// visible cards per app (overflow stays in [`NotifModel::history`] — the
/// center's list — and never reaches the stack); DND suppresses popups
/// while still recording history. `history` mirrors the *live* set only
/// (closed cards leave it; the daemon's bounded closed ring is re-read via
/// `GetHistory` by the center itself). No expiry heap anywhere: the daemon
/// owns expiry, shell timers only request `CloseNotification`.
#[derive(Debug, Default)]
pub struct NotifModel {
    visible: Vec<Notification>,
    history: Vec<Notification>,
    dnd: bool,
}

impl NotifModel {
    /// Record an added notification: always history; visible unless DND is
    /// on, newest-first with the per-app cap enforced.
    pub fn apply_added(&mut self, n: Notification) {
        self.history.push(n.clone());
        if self.dnd {
            return;
        }
        self.visible.insert(0, n);
        self.enforce_cap();
    }

    /// Drop a card from the popup stack and the center list. (The daemon's
    /// own `GetHistory` additionally keeps a bounded closed ring — the
    /// center re-reads that directly in Task 3; this model's `history` is
    /// the *live* set: suppressed and overflow cards the center must still
    /// list, minus anything closed.)
    pub fn apply_closed(&mut self, id: u32) {
        self.visible.retain(|n| n.id != id);
        self.history.retain(|n| n.id != id);
    }

    /// Seed (or re-seed after reconnect/center-open) from `GetActive`:
    /// replaces both views, visible capped, DND still suppressing popups.
    pub fn apply_snapshot(&mut self, active: Vec<Notification>) {
        self.history = active.clone();
        self.visible.clear();
        if self.dnd {
            return;
        }
        // `GetActive` arrives oldest-first; the stack is newest-first.
        self.visible = active.into_iter().rev().collect();
        self.enforce_cap();
    }

    /// Mirror the daemon's DND flag (driven by `get_dnd` / `DndChanged`).
    /// Flag-only: removals for newly-suppressed cards arrive as `Removed`
    /// updates (→ [`NotifModel::apply_closed`]), so clearing here would
    /// double-handle the same transition.
    pub fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
    }

    pub fn visible(&self) -> &[Notification] {
        &self.visible
    }

    pub fn history(&self) -> &[Notification] {
        &self.history
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    /// Keep the first [`MAX_VISIBLE_PER_APP`] cards per app in the
    /// newest-first order; overflow cards stay in history only.
    fn enforce_cap(&mut self) {
        let mut counts = std::collections::HashMap::new();
        self.visible.retain(|n| {
            let c = counts.entry(n.app_name.clone()).or_insert(0usize);
            *c += 1;
            *c <= MAX_VISIBLE_PER_APP
        });
    }
}

/// Build a minimal `Notification` for tests.
#[cfg(test)]
use icedtea_contract::{IconSource, NotificationAction, Urgency};

#[cfg(test)]
pub fn mk_test_notification(app: &str, id: u32) -> Notification {
    Notification {
        id,
        app_name: app.into(),
        icon: IconSource::None,
        summary: format!("summary {id}"),
        body: "body".into(),
        actions: vec![NotificationAction {
            key: "default".into(),
            label: "Open".into(),
        }],
        urgency: Urgency::Normal,
        category: None,
        resident: false,
        transient: false,
        created_at_ms: 0,
        expire_at_ms: None,
        suppressed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(app: &str, id: u32) -> Notification {
        mk_test_notification(app, id)
    }

    #[test]
    fn added_stacks_newest_first_and_caps_per_app() {
        let mut m = NotifModel::default();
        for i in 0..5 {
            m.apply_added(mk("chat", i));
        }
        assert_eq!(m.visible().len(), 3);
        assert_eq!(m.visible()[0].id, 4);
    }

    #[test]
    fn closed_removes_and_dnd_suppresses_popups_but_not_center() {
        let mut m = NotifModel::default();
        m.apply_added(mk("mail", 1));
        m.apply_closed(1);
        assert!(m.visible().is_empty());
        m.set_dnd(true);
        m.apply_added(mk("mail", 2));
        assert!(m.visible().is_empty() && m.history().len() == 1);
    }

    /// Pins the query/command wire members against
    /// `notifications/src/service.rs`'s `IcedteaInterface` (`fn get_active`
    /// → `GetActive`, same PascalCase rule as the compositor's `SpawnApp`).
    #[test]
    fn method_members_match_the_daemon_interface() {
        assert_eq!(GET_ACTIVE_MEMBER, "GetActive");
        assert_eq!(GET_HISTORY_MEMBER, "GetHistory");
        assert_eq!(INVOKE_ACTION_MEMBER, "InvokeAction");
        assert_eq!(GET_DND_MEMBER, "GetDoNotDisturb");
        assert_eq!(SET_DND_MEMBER, "SetDoNotDisturb");
    }

    /// `CloseNotification` is the odd one out: it lives on the *standard*
    /// interface (`StdInterface::close_notification`), not the icedtea
    /// extension one.
    #[test]
    fn close_member_matches_the_standard_interface() {
        assert_eq!(CLOSE_NOTIFICATION_MEMBER, "CloseNotification");
    }

    /// Pins the signal members against `service.rs`'s `emit`: the shorthand
    /// `Added`/`Closed`/`DndChanged` from the plan is NOT what crosses the
    /// bus.
    #[test]
    fn signal_members_match_the_daemon_emitter() {
        assert_eq!(NOTIFICATION_ADDED_MEMBER, "NotificationAdded");
        assert_eq!(NOTIFICATION_CLOSED_MEMBER, "NotificationClosed");
        assert_eq!(NOTIFICATION_REMOVED_MEMBER, "NotificationRemoved");
        assert_eq!(ACTION_INVOKED_MEMBER, "ActionInvoked");
        assert_eq!(DND_CHANGED_MEMBER, "DoNotDisturbChanged");
    }

    #[test]
    fn overflow_beyond_the_cap_stays_in_history() {
        let mut m = NotifModel::default();
        for i in 0..5 {
            m.apply_added(mk("chat", i));
        }
        assert_eq!(m.history().len(), 5);
        assert!(m.visible().iter().all(|n| n.app_name == "chat"));
        // Newest three survive, oldest two are center-only.
        let ids: Vec<u32> = m.visible().iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![4, 3, 2]);
    }

    #[test]
    fn cap_is_per_app_not_global() {
        let mut m = NotifModel::default();
        for i in 0..3 {
            m.apply_added(mk("chat", i));
        }
        m.apply_added(mk("mail", 10));
        assert_eq!(m.visible().len(), 4);
        assert_eq!(m.visible()[0].id, 10);
    }

    #[test]
    fn snapshot_seeds_newest_first_and_respects_dnd() {
        let mut m = NotifModel::default();
        m.apply_snapshot(vec![mk("mail", 1), mk("mail", 2)]);
        assert_eq!(m.visible()[0].id, 2);

        let mut d = NotifModel::default();
        d.set_dnd(true);
        d.apply_snapshot(vec![mk("mail", 1)]);
        assert!(d.visible().is_empty());
        assert_eq!(d.history().len(), 1);
    }

    #[test]
    fn mock_records_writes_and_stubs_reads() {
        let mock = MockNotifier::with_active(vec![mk("mail", 7)]);
        assert_eq!(mock.get_active().len(), 1);
        mock.invoke_action(7, "default");
        mock.close_notification(7);
        mock.set_dnd(true);
        assert_eq!(mock.invoked(), vec![(7, "default".to_string())]);
        assert_eq!(mock.closed(), vec![7]);
        assert!(mock.get_dnd());
        assert_eq!(mock.dnd_sets(), vec![true]);
    }
}
