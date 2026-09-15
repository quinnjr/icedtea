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
use icedtea_contract::{
    CloseReason, IconSource, NOTIF_BUS_NAME, NOTIF_PATH, Notification, NotificationAction, Urgency,
};

/// The standard notifications interface `CloseNotification` and the
/// `NotificationClosed`/`ActionInvoked` signals live on (same bus name and
/// object path as the icedtea extension interface, different interface).
const STANDARD_IFACE: &str = "org.freedesktop.Notifications";

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

/// Max cards the model keeps in [`NotifModel::history`]. Mirrors the daemon's
/// own `HISTORY_MAX` (`notifications/src/store.rs`): the client never needs
/// more than the daemon remembers, and an unbounded `Vec` would let a chatty
/// or hostile app balloon the shell process via `Snapshot` clones.
pub const HISTORY_MAX: usize = 200;

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
    /// `NotificationClosed(id, _)` or `NotificationRemoved(id, reason)` with
    /// a real reason: the card is gone — drop it from the stack and the
    /// center list. A `Removed` with [`CloseReason::Undefined`] is instead a
    /// DND suppression (see `service.rs`'s `Change::Suppressed`), decoded as
    /// [`NotifUpdate::Suppressed`] below, never as this.
    Removed(u32),
    /// `NotificationRemoved(id, Undefined)`: DND hid the card without
    /// closing it — drop it from the stack but keep it in the center list.
    /// The card rejoins via `Added` when DND lifts (the daemon re-announces
    /// newly-visible cards), and the center re-reads `GetHistory` directly.
    Suppressed(u32),
    /// `ActionInvoked(id, key)`.
    ActionInvoked { id: u32, key: String },
    /// `DoNotDisturbChanged(on)`.
    DndChanged(bool),
}

/// Spawn the signal worker. It seeds with `GetActive`, then forwards every
/// matching signal as a [`NotifUpdate`] until the bus drops — and then
/// reconnects with capped backoff, re-seeding on every reconnect (the design
/// spec's stated mitigation for a daemon restart; `run` re-sends `Snapshot`
/// each time it starts).
///
/// Unlike `compositor_client::spawn` this never exits the process on error:
/// notifications are non-critical (a dead popup worker must not take the
/// panel down the way a stale taskbar would). `run` returning `Ok` means the
/// shell exited (inbox closed) and the thread ends; `Err` means the bus is
/// down and the loop retries.
pub fn spawn(tx: Sender<NotifUpdate>) {
    std::thread::spawn(move || {
        let mut backoff = std::time::Duration::from_secs(1);
        loop {
            let started = std::time::Instant::now();
            match zbus::block_on(run(tx.clone())) {
                Ok(()) => break, // The shell exited before the next signal.
                Err(err) => {
                    backoff = next_backoff(started.elapsed(), backoff);
                    tracing::warn!(%err, ?backoff, "notification worker lost the bus; retrying");
                    // Sleep interruptibly: a shell exit during backoff must
                    // end the thread within ~1s, not after the full delay.
                    let mut slept = std::time::Duration::ZERO;
                    while slept < backoff {
                        std::thread::sleep(std::time::Duration::from_secs(1).min(backoff - slept));
                        slept += std::time::Duration::from_secs(1);
                        if tx.is_closed() {
                            return; // The shell exited during the backoff sleep.
                        }
                    }
                }
            }
        }
    });
}

/// Pure backoff policy, extracted so tests can pin it: a run that lived a
/// healthy life resets to 1s (a fresh failure after hours of stability must
/// not wait out a delay earned by an old flap sequence), otherwise double
/// capped at 30s.
fn next_backoff(
    healthy_for: std::time::Duration,
    current: std::time::Duration,
) -> std::time::Duration {
    if healthy_for > std::time::Duration::from_secs(60) {
        std::time::Duration::from_secs(1)
    } else {
        (current * 2).min(std::time::Duration::from_secs(30))
    }
}

async fn run(tx: Sender<NotifUpdate>) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, NOTIF_BUS_NAME, NOTIF_PATH, ICEDTEA_IFACE).await?;

    // Trust note: whoever owns the well-known name is trusted as the daemon
    // (first-come ownership, the freedesktop convention mako/dunst share).
    // A squatter racing the real daemon at login would be pinned instead —
    // confining name ownership (bus policy / systemd activation) is system
    // integration outside this client; the sender pin below at least forces
    // an attacker to win the name openly rather than spoof signals quietly.
    let owner = resolve_owner(&conn).await?;
    let mut merged = subscribe(&conn, &owner).await?;

    // Subscribe BEFORE seeding: any signal emitted between the `GetActive`
    // read and the rule install would otherwise be lost with no resync until
    // the next reconnect. Snapshot-then-delta stays safe because every delta
    // handler is idempotent (`apply_added` dedups by id, closes/suppressions
    // are retains).
    let seed: Vec<Notification> = proxy.call(GET_ACTIVE_MEMBER, &()).await?;
    if tx.send(NotifUpdate::Snapshot(seed)).await.is_err() {
        return Ok(()); // The shell exited before the first signal.
    }

    loop {
        match merged.next().await {
            Some(Ok(msg)) => match classify(&msg, &owner) {
                SignalAction::Forward(update) => {
                    if tx.send(update).await.is_err() {
                        break; // The shell exited.
                    }
                }
                SignalAction::Drop => {}
                SignalAction::Reconnect => {
                    return Err(zbus::Error::Failure(
                        "notification daemon owner changed; re-resolving".to_string(),
                    ));
                }
            },
            Some(Err(err)) => {
                // A mid-stream transport error kills delivery for the rest of
                // the process unless someone retries — log loudly and return
                // `Err` so `spawn`'s reconnect loop picks it up.
                tracing::error!(?err, "notification signal stream failed");
                return Err(err);
            }
            None => {
                return Err(zbus::Error::Failure(
                    "notification signal stream ended unexpectedly".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Resolve the unique name currently owning the notification bus name.
/// Called on every (re)connect so a daemon restart re-pins cleanly.
async fn resolve_owner(conn: &zbus::Connection) -> zbus::Result<zbus::names::OwnedUniqueName> {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &(NOTIF_BUS_NAME,),
    )
    .await?
    .body()
    .deserialize()
}

/// Install the sender-pinned notification rule plus the bus-daemon
/// `NameOwnerChanged` watch, merged into one stream. The pin goes stale if
/// the daemon restarts under a new unique name — the bus stays up, so no
/// transport error would ever fire and the worker would idle forever on a
/// dead pin; the owner watch lets `classify` bail to `spawn`'s reconnect
/// path (fresh owner + fresh seed) instead.
async fn subscribe(
    conn: &zbus::Connection,
    owner: &zbus::names::OwnedUniqueName,
) -> zbus::Result<impl futures_util::Stream<Item = zbus::Result<zbus::message::Message>>> {
    // Signals span both interfaces at this path (standard:
    // `NotificationClosed`/`ActionInvoked`; icedtea: the rest), so one
    // sender-pinned rule covers the path and `classify` checks the interface.
    // Pinning the sender rejects impostors: any session-bus peer can emit a
    // signal with our path and member, so a path-only rule would let a
    // hostile app spoof `DoNotDisturbChanged` (silently suppressing popups)
    // or `NotificationRemoved` (hiding a live alert).
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(NOTIF_PATH)?
        .sender(owner.clone())?
        .build();
    let stream = zbus::MessageStream::for_match_rule(rule, conn, None).await?;
    let owner_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .path("/org/freedesktop/DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .build();
    let owner_stream = zbus::MessageStream::for_match_rule(owner_rule, conn, None).await?;
    Ok(futures_util::stream::select(stream, owner_stream))
}

/// What `run` should do with one incoming signal. Pure — every sender,
/// interface, member, and body decision lives here so unit tests can drive
/// it with `Message::signal` bodies and no bus.
#[derive(Debug, PartialEq, Eq)]
enum SignalAction {
    /// Forward to the shell loop thread.
    Forward(NotifUpdate),
    /// Drop (impostor, unknown member, malformed body) — already logged.
    Drop,
    /// The daemon owner changed: bail to `spawn`'s reconnect path.
    Reconnect,
}

fn classify(msg: &zbus::message::Message, owner: &zbus::names::OwnedUniqueName) -> SignalAction {
    let header = msg.header();
    let sender = header.sender().map(|s| s.as_str().to_string());
    let iface = header.interface().map(|i| i.as_str().to_string());
    let member = header.member().map(|m| m.as_str().to_string());
    // Owner changes come from the bus daemon itself, never from the pinned
    // notification daemon — authenticate the sender BEFORE honoring them, so
    // a same-named signal from anyone else can only be dropped, never obeyed.
    if member.as_deref() == Some("NameOwnerChanged") {
        if sender.as_deref() != Some("org.freedesktop.DBus") {
            tracing::warn!(?sender, "NameOwnerChanged from non-bus sender; dropping");
            return SignalAction::Drop;
        }
        match msg.body().deserialize::<(String, String, String)>() {
            Ok((name, _, _)) if name == NOTIF_BUS_NAME => return SignalAction::Reconnect,
            Ok(_) => return SignalAction::Drop,
            // Fail closed: an undecodable owner-change from the bus itself
            // may mean our pin just died — re-resolve rather than idle on a
            // possibly-stale pin with nothing in the logs.
            Err(err) => {
                tracing::warn!(?err, "NameOwnerChanged body decode failed; reconnecting");
                return SignalAction::Reconnect;
            }
        }
    }
    // Belt and suspenders: the match rule already pins the sender, but a
    // rule is only as trustworthy as the bus that enforces it — drop
    // anything that is not from the daemon owner or not on one of the two
    // notification interfaces.
    let from_daemon = sender.as_deref() == Some(owner.as_str());
    let known_iface = matches!(iface.as_deref(), Some(STANDARD_IFACE) | Some(ICEDTEA_IFACE));
    if !from_daemon || !known_iface {
        tracing::warn!(
            ?sender,
            ?iface,
            "dropping signal from unexpected sender/interface"
        );
        return SignalAction::Drop;
    }
    let body = msg.body();
    let update = match member.as_deref() {
        Some(NOTIFICATION_ADDED_MEMBER) => decode_id(&body).map(NotifUpdate::Added),
        Some(NOTIFICATION_CLOSED_MEMBER) => decode_closed(&body).map(NotifUpdate::Removed),
        // `NotificationRemoved` is ambiguous on the wire: a real close
        // carries its reason, while DND suppression always carries
        // `Undefined` (see `service.rs`'s `Change::Suppressed` comment).
        // Decode accordingly so a suppression hides the popup but keeps the
        // center entry.
        Some(NOTIFICATION_REMOVED_MEMBER) => decode_removed(&body).map(|(id, suppressed)| {
            if suppressed {
                NotifUpdate::Suppressed(id)
            } else {
                NotifUpdate::Removed(id)
            }
        }),
        Some(ACTION_INVOKED_MEMBER) => {
            decode_action(&body).map(|(id, key)| NotifUpdate::ActionInvoked { id, key })
        }
        Some(DND_CHANGED_MEMBER) => decode_bool(&body).map(NotifUpdate::DndChanged),
        // The rule admits every signal the daemon emits on this path: an
        // unknown member is either a daemon addition we should handle or a
        // mis-pinned constant — either way it must be visible, never
        // silently discarded.
        _ => {
            tracing::warn!(?member, ?iface, "unknown notification signal; dropping");
            None
        }
    };
    match update {
        Some(update) => SignalAction::Forward(update),
        None => SignalAction::Drop,
    }
}

/// The one warn-and-drop arm behind every signal decoder: a daemon payload
/// change must be visible in the logs (the member-name pin tests stay green
/// through a payload change), never a silent delivery stop.
fn decode_failed<T>(member: &str, err: impl std::fmt::Debug) -> Option<T> {
    tracing::warn!(?err, member, "signal body decode failed; dropping");
    None
}

/// Standard `NotificationClosed(id, reason_code)`: the code is informational
/// only (any close drops the card), so keep just the id.
fn decode_closed(body: &zbus::message::Body) -> Option<u32> {
    match body.deserialize::<(u32, u32)>() {
        Ok((id, _)) => Some(id),
        Err(err) => decode_failed(NOTIFICATION_CLOSED_MEMBER, err),
    }
}

/// Icedtea `NotificationRemoved(id, reason)`: `Undefined` means DND
/// suppression (hidden, not closed), anything else a real close.
fn decode_removed(body: &zbus::message::Body) -> Option<(u32, bool)> {
    match body.deserialize::<(u32, CloseReason)>() {
        Ok((id, reason)) => Some((id, reason == CloseReason::Undefined)),
        Err(err) => decode_failed(NOTIFICATION_REMOVED_MEMBER, err),
    }
}

/// Standard `ActionInvoked(id, key)`.
fn decode_action(body: &zbus::message::Body) -> Option<(u32, String)> {
    match body.deserialize::<(u32, String)>() {
        Ok(v) => Some(v),
        Err(err) => decode_failed(ACTION_INVOKED_MEMBER, err),
    }
}

/// `emit_signal` bodies for id-only signals arrive as a 1-tuple struct;
/// accept that form (falling back to a bare value so an emitter-side shape
/// change degrades to a dropped signal, never a panic on this thread). One
/// macro stamps out every single-value decoder; the warn-and-drop policy
/// lives in [`decode_failed`], shared with the tuple decoders.
macro_rules! decode_single {
    ($name:ident, $ty:ty, $member:expr) => {
        fn $name(body: &zbus::message::Body) -> Option<$ty> {
            if let Ok((v,)) = body.deserialize::<($ty,)>() {
                return Some(v);
            }
            match body.deserialize::<$ty>() {
                Ok(v) => Some(v),
                Err(err) => decode_failed($member, err),
            }
        }
    };
}
decode_single!(decode_id, u32, NOTIFICATION_ADDED_MEMBER);
decode_single!(decode_bool, bool, DND_CHANGED_MEMBER);

/// The notification command surface — abstracted so a test can inject a
/// recording mock in place of the real D-Bus proxy.
///
/// Every method takes ids/keys as plain arguments crossing D-Bus directly;
/// nothing here ever builds an argv (B1 hardening rationale applies).
///
/// Reads return `None` when the bus is unreachable — deliberately distinct
/// from an empty `Vec` / `false`, so the caller retains its last-known model
/// instead of reconciling against a phantom empty set (a transient bus error
/// must not wipe the stack or flip DND off).
pub trait NotifierCommands {
    fn get_active(&self) -> Option<Vec<Notification>>;
    fn get_history(&self) -> Option<Vec<Notification>>;
    fn invoke_action(&self, id: u32, key: &str);
    fn close_notification(&self, id: u32);
    fn get_dnd(&self) -> Option<bool>;
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

    /// Inject a connection (tests, or a Task 4 harness stub bus) instead of
    /// grabbing the session bus — the seam that keeps this proxy testable.
    pub fn from_connection(conn: zbus::blocking::Connection) -> Self {
        NotifierProxy { conn }
    }

    /// One call-warn-`None` helper behind every read, so the
    /// "unreachable, retain last-known" policy lives in exactly one place.
    fn call<T>(&self, iface: &str, member: &str) -> Option<T>
    where
        T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        match self
            .conn
            .call_method(Some(NOTIF_BUS_NAME), NOTIF_PATH, Some(iface), member, &())
        {
            Ok(msg) => match msg.body().deserialize::<T>() {
                Ok(v) => Some(v),
                Err(err) => {
                    tracing::warn!(?err, member, "reply decode failed; treating as unreachable");
                    None
                }
            },
            Err(err) => {
                tracing::warn!(?err, member, "call failed; treating as unreachable");
                None
            }
        }
    }

    fn get_vec(&self, iface: &str, member: &str) -> Option<Vec<Notification>> {
        self.call(iface, member)
    }
}

impl NotifierCommands for NotifierProxy {
    // zbus's #[interface] exposes Rust methods in PascalCase, so the wire
    // members are GetActive/GetHistory/... (matching GetState's rule).
    fn get_active(&self) -> Option<Vec<Notification>> {
        self.get_vec(ICEDTEA_IFACE, GET_ACTIVE_MEMBER)
    }
    fn get_history(&self) -> Option<Vec<Notification>> {
        self.get_vec(ICEDTEA_IFACE, GET_HISTORY_MEMBER)
    }
    fn invoke_action(&self, id: u32, key: &str) {
        // Fire-and-forget by design (the loop thread must not block on a
        // click), but a failure is a lie the UI would otherwise tell — log
        // it so a dead daemon at click time is diagnosable.
        if let Err(err) = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            INVOKE_ACTION_MEMBER,
            &(id, key),
        ) {
            tracing::warn!(?err, id, "InvokeAction failed; action did not run");
        }
    }
    // `CloseNotification` lives on the *standard* interface, not the icedtea
    // extension one — same bus name and object path, different interface.
    fn close_notification(&self, id: u32) {
        if let Err(err) = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(STANDARD_IFACE),
            CLOSE_NOTIFICATION_MEMBER,
            &(id,),
        ) {
            tracing::warn!(?err, id, "CloseNotification failed; card stays up");
        }
    }
    fn get_dnd(&self) -> Option<bool> {
        self.call(ICEDTEA_IFACE, GET_DND_MEMBER)
    }
    fn set_dnd(&self, on: bool) {
        if let Err(err) = self.conn.call_method(
            Some(NOTIF_BUS_NAME),
            NOTIF_PATH,
            Some(ICEDTEA_IFACE),
            SET_DND_MEMBER,
            &(on,),
        ) {
            tracing::warn!(?err, on, "SetDoNotDisturb failed; daemon flag unchanged");
        }
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
        snapshot(&self.invoked)
    }

    pub fn closed(&self) -> Vec<u32> {
        snapshot(&self.closed)
    }

    /// Every `set_dnd` argument in order (the last is the current intent).
    pub fn dnd_sets(&self) -> Vec<bool> {
        snapshot(&self.dnd_sets)
    }

    pub fn dnd(&self) -> bool {
        snapshot(&self.dnd)
    }
}

/// Clone a mutex-guarded stub, degrading to `Default` on poisoning — the one
/// place the mock's "never panic, even on a foreign thread" policy lives, so
/// a policy change (e.g. logging poisoning) edits here, not six accessors.
fn snapshot<T: Clone + Default>(m: &Mutex<T>) -> T {
    m.lock().ok().map(|g| g.clone()).unwrap_or_default()
}

/// Record a write, ignoring it on poisoning (same policy as [`snapshot`]).
fn record<T>(m: &Mutex<Vec<T>>, v: T) {
    if let Ok(mut g) = m.lock() {
        g.push(v);
    }
}

impl NotifierCommands for MockNotifier {
    fn get_active(&self) -> Option<Vec<Notification>> {
        Some(snapshot(&self.active))
    }
    fn get_history(&self) -> Option<Vec<Notification>> {
        Some(snapshot(&self.history))
    }
    fn invoke_action(&self, id: u32, key: &str) {
        record(&self.invoked, (id, key.to_string()));
    }
    fn close_notification(&self, id: u32) {
        record(&self.closed, id);
    }
    fn get_dnd(&self) -> Option<bool> {
        Some(self.dnd())
    }
    fn set_dnd(&self, on: bool) {
        if let Ok(mut g) = self.dnd.lock() {
            *g = on;
        }
        record(&self.dnd_sets, on);
    }
}

/// The pure popup stack. Newest-first; at most [`MAX_VISIBLE_PER_APP`]
/// visible cards per app (overflow stays in [`NotifModel::history`] — the
/// center's list — and never reaches the stack); DND suppresses popups
/// *except* [`Urgency::Critical`] ones (mirroring the daemon's
/// `dnd && urgency != Critical` rule in `store.rs` — a critical alert must
/// still pop while DND is on); `history` is bounded by [`HISTORY_MAX`].
/// `history` mirrors the *live* set only (closed cards leave it; the
/// daemon's bounded closed ring is re-read via `GetHistory` by the center
/// itself). No expiry heap anywhere: the daemon owns expiry, shell timers
/// only request `CloseNotification`.
#[derive(Debug, Default)]
pub struct NotifModel {
    visible: Vec<Notification>,
    history: Vec<Notification>,
    dnd: bool,
}

impl NotifModel {
    /// Record an added notification: always history (replacing any card with
    /// the same id — the daemon updates in place on `replaces_id` and
    /// re-emits `Added` for the existing id, so a blind push would duplicate
    /// the card); visible newest-first with the per-app cap enforced, unless
    /// DND is on and the card is not critical.
    pub fn apply_added(&mut self, n: Notification) {
        self.history.retain(|m| m.id != n.id);
        self.history.push(n.clone());
        // Eviction must also retire popups: without this, a cross-app flood
        // (200 `mail` cards after 3 visible `chat` cards) ages the `chat`
        // cards out of `history` while their popups stay up — visible would
        // stop being a subset of history and the center would miss live
        // popups.
        if self.truncate_history() {
            let live: std::collections::HashSet<u32> = self.history.iter().map(|n| n.id).collect();
            self.visible.retain(|n| live.contains(&n.id));
        }
        if self.dnd && n.urgency != Urgency::Critical {
            return;
        }
        self.visible.retain(|m| m.id != n.id);
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

    /// Hide a DND-suppressed card from the stack without closing it: the
    /// center entry survives (unlike [`NotifModel::apply_closed`]). The card
    /// rejoins via `Added` when DND lifts.
    pub fn apply_suppressed(&mut self, id: u32) {
        self.visible.retain(|n| n.id != id);
    }

    /// Seed (or re-seed after reconnect/center-open) from `GetActive`:
    /// replaces both views, visible capped, DND still suppressing popups
    /// except critical ones. Note `GetActive` hides suppressed cards, so a
    /// re-seed while DND is on drops them from `history` too — they rejoin
    /// via `Added` when DND lifts (the daemon guarantees the re-announce),
    /// and the center's own `GetHistory` read is unaffected.
    pub fn apply_snapshot(&mut self, active: Vec<Notification>) {
        // Bound the daemon-supplied vector BEFORE cloning it into the views:
        // a hostile or compromised name-owner's oversized `GetActive` reply
        // must never be fully allocated into the shell.
        self.history = active;
        self.truncate_history();
        self.visible.clear();
        // `GetActive` arrives oldest-first; the stack is newest-first. The
        // visible stack is a subset of the (bounded) history.
        let mut visible: Vec<Notification> = self.history.clone().into_iter().rev().collect();
        if self.dnd {
            visible.retain(|n| n.urgency == Urgency::Critical);
        }
        self.visible = visible;
        self.enforce_cap();
    }

    /// Mirror the daemon's DND flag (driven by `get_dnd` / `DndChanged`).
    /// Flag-only: removals for newly-suppressed cards arrive as `Suppressed`
    /// updates (→ [`NotifModel::apply_suppressed`]), so clearing here would
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

    /// The one history-bound enforcer (oldest evicted first): every path
    /// that grows `history` funnels through here so the bound can neither
    /// drift between call sites nor degrade to O(n²) `remove(0)` loops.
    /// Returns whether anything was evicted (the caller retires orphaned
    /// popups in that case).
    fn truncate_history(&mut self) -> bool {
        let len = self.history.len();
        if len > HISTORY_MAX {
            self.history.drain(..len - HISTORY_MAX);
            true
        } else {
            false
        }
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

/// Build a minimal `Notification` for tests. `pub` (but hidden) rather than
/// `#[cfg(test)]` so integration tests (`shell/tests/notif.rs`) share the
/// single canonical factory instead of duplicating the 13-field literal —
/// `contract::Notification` keeps evolving, and two lockstep literals would
/// silently diverge.
#[doc(hidden)]
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
    fn closed_removes_from_stack_and_center() {
        let mut m = NotifModel::default();
        m.apply_added(mk("mail", 1));
        m.apply_closed(1);
        assert!(m.visible().is_empty());
        assert!(m.history().is_empty());
    }

    #[test]
    fn dnd_suppresses_popups_but_still_records_center() {
        let mut m = NotifModel::default();
        m.set_dnd(true);
        m.apply_added(mk("mail", 2));
        assert!(m.visible().is_empty());
        assert_eq!(m.history().len(), 1);
    }

    #[test]
    fn dnd_enable_keeps_existing_popups_until_suppressed_arrives() {
        let mut m = NotifModel::default();
        m.apply_added(mk("mail", 1));
        m.set_dnd(true);
        // Flag-only: the card stays until the daemon's Suppressed update.
        assert_eq!(m.visible().len(), 1);
        m.apply_suppressed(1);
        assert!(m.visible().is_empty());
        // ...but the center entry survives the suppression.
        assert_eq!(m.history().len(), 1);
    }

    #[test]
    fn critical_pops_even_while_dnd_is_on() {
        let mut m = NotifModel::default();
        m.set_dnd(true);
        let mut n = mk("alert", 9);
        n.urgency = Urgency::Critical;
        m.apply_added(n);
        assert_eq!(m.visible().len(), 1);
        // A critical re-announce after a snapshot under DND stays visible.
        let mut crit = mk("alert", 9);
        crit.urgency = Urgency::Critical;
        m.apply_snapshot(vec![crit, mk("mail", 10)]);
        assert!(m.visible().iter().any(|n| n.id == 9));
        assert!(!m.visible().iter().any(|n| n.id == 10));
    }

    #[test]
    fn re_added_same_id_replaces_in_place_instead_of_duplicating() {
        let mut m = NotifModel::default();
        m.apply_added(mk("dl", 3));
        // The daemon updates in place on `replaces_id` and re-emits Added
        // for the existing id (e.g. download progress): no second card.
        let mut updated = mk("dl", 3);
        updated.summary = "50%".into();
        m.apply_added(updated);
        assert_eq!(m.visible().len(), 1);
        assert_eq!(m.history().len(), 1);
        assert_eq!(m.visible()[0].summary, "50%");
        // A later close removes the single card, not "both at once".
        m.apply_closed(3);
        assert!(m.visible().is_empty() && m.history().is_empty());
    }

    /// The daemon source itself, read at compile time: every wire member the
    /// client uses must appear in `notifications/src/service.rs` (its
    /// `#[interface]` methods for commands, `emit_signal` calls for
    /// signals). A rename on the daemon side breaks this test instead of
    /// silently delivering nothing — the previous literal-equality asserts
    /// compared each constant to the string it was defined with and proved
    /// nothing about the daemon.
    const DAEMON_SERVICE: &str = include_str!("../../notifications/src/service.rs");

    /// Pins the query/command wire members against the daemon's
    /// `IcedteaInterface` (`fn get_active` → `GetActive`, same PascalCase
    /// rule as the compositor's `SpawnApp`): zbus derives the PascalCase
    /// member from the snake_case fn, so the fn's presence IS the pin — the
    /// quoted member string never appears in the daemon source.
    #[test]
    fn method_members_match_the_daemon_interface() {
        // Explicit wire→method table: zbus derives the PascalCase member
        // from the snake_case fn, but the test pins both spellings
        // literally rather than reimplementing the convention.
        for (member, method) in [
            (GET_ACTIVE_MEMBER, "fn get_active"),
            (GET_HISTORY_MEMBER, "fn get_history"),
            (INVOKE_ACTION_MEMBER, "fn invoke_action"),
            (GET_DND_MEMBER, "fn get_do_not_disturb"),
            (SET_DND_MEMBER, "fn set_do_not_disturb"),
        ] {
            assert!(
                DAEMON_SERVICE.contains(method),
                "{member} has no matching {method} in notifications/src/service.rs"
            );
        }
    }

    /// `CloseNotification` is the odd one out: it lives on the *standard*
    /// interface (`StdInterface::close_notification`), not the icedtea
    /// extension one.
    #[test]
    fn close_member_matches_the_standard_interface() {
        assert_eq!(CLOSE_NOTIFICATION_MEMBER, "CloseNotification");
        assert!(
            DAEMON_SERVICE.contains("fn close_notification"),
            "daemon lost StdInterface::close_notification"
        );
    }

    /// Pins the signal members against `service.rs`'s `emit`: the shorthand
    /// `Added`/`Closed`/`DndChanged` from the plan is NOT what crosses the
    /// bus.
    #[test]
    fn signal_members_match_the_daemon_emitter() {
        for member in [
            NOTIFICATION_ADDED_MEMBER,
            NOTIFICATION_CLOSED_MEMBER,
            NOTIFICATION_REMOVED_MEMBER,
            ACTION_INVOKED_MEMBER,
            DND_CHANGED_MEMBER,
        ] {
            assert!(
                DAEMON_SERVICE.contains(&format!("\"{member}\"")),
                "{member} is never emitted in notifications/src/service.rs"
            );
        }
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
    fn snapshot_caps_per_app_but_keeps_full_history() {
        let mut m = NotifModel::default();
        let seed: Vec<Notification> = (0..5).map(|i| mk("chat", i)).collect();
        m.apply_snapshot(seed);
        // The popup-storm guard holds on the re-seed path too: deleting
        // `enforce_cap()` from `apply_snapshot` must break this test.
        assert_eq!(m.visible().len(), 3);
        let ids: Vec<u32> = m.visible().iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![4, 3, 2]);
        assert_eq!(m.history().len(), 5);
    }

    #[test]
    fn history_never_exceeds_the_daemon_ring_bound() {
        let mut m = NotifModel::default();
        for i in 0..(HISTORY_MAX + 50) {
            m.apply_added(mk("spam", i as u32));
        }
        assert_eq!(m.history().len(), HISTORY_MAX);
        // Oldest evicted first: the survivors are the newest cards.
        assert_eq!(m.history()[0].id, 50);
    }

    #[test]
    fn history_eviction_retires_orphaned_popups() {
        let mut m = NotifModel::default();
        for i in 0..3 {
            m.apply_added(mk("chat", i));
        }
        assert_eq!(m.visible().len(), 3);
        // A cross-app flood ages the `chat` cards out of history: their
        // popups must retire too, keeping visible a subset of history.
        for i in 0..HISTORY_MAX {
            m.apply_added(mk("mail", 1000 + i as u32));
        }
        assert_eq!(m.history().len(), HISTORY_MAX);
        assert!(!m.visible().iter().any(|n| n.app_name == "chat"));
        let live: std::collections::HashSet<u32> = m.history().iter().map(|n| n.id).collect();
        assert!(m.visible().iter().all(|n| live.contains(&n.id)));
    }

    #[test]
    fn snapshot_truncates_history_to_the_daemon_ring_bound() {
        let mut m = NotifModel::default();
        let seed: Vec<Notification> = (0..HISTORY_MAX + 50)
            .map(|i| mk("chat", i as u32))
            .collect();
        m.apply_snapshot(seed);
        // Deleting the `drain(..drop)` block (or draining the newest end)
        // must break this test: a reconnect re-seed is the balloon path.
        assert_eq!(m.history().len(), HISTORY_MAX);
        assert_eq!(m.history()[0].id, 50);
    }

    /// `HISTORY_MAX` mirrors the daemon's closed-ring bound by value — the
    /// coupling is deliberate (the shell's live-set history never needs more
    /// than the daemon remembers), and this pin breaks the moment the daemon
    /// bumps its ring without updating the client.
    #[test]
    fn history_bound_matches_the_daemon_ring() {
        const DAEMON_STORE: &str = include_str!("../../notifications/src/store.rs");
        assert!(
            DAEMON_STORE.contains(&format!("pub const HISTORY_MAX: usize = {HISTORY_MAX};")),
            "daemon HISTORY_MAX drifted from the client's {HISTORY_MAX}"
        );
    }

    /// The signal decoders, driven with real `Message::signal` bodies (no
    /// bus needed): the 1-tuple struct form the daemon emits, the bare-value
    /// fallback, and a malformed body that must degrade to `None` — never a
    /// panic on the worker thread. Every decoder test builds through this
    /// one helper so a bus-shape or assertion-style change edits one place.
    fn signal_msg<T>(iface: &str, member: &str, body: &T) -> zbus::message::Message
    where
        T: serde::Serialize + zbus::zvariant::Type,
    {
        zbus::message::Message::signal(NOTIF_PATH, iface, member)
            .expect("test signal builds")
            .build(body)
            .expect("test body serializes")
    }

    fn signal_body<T>(iface: &str, member: &str, body: &T) -> zbus::message::Body
    where
        T: serde::Serialize + zbus::zvariant::Type,
    {
        signal_msg(iface, member, body).body()
    }

    /// A classified message stamped with a sender, like the bus would stamp
    /// it: `classify` authenticates the sender, so tests must set one.
    fn signal_from<T>(sender: &str, iface: &str, member: &str, body: &T) -> zbus::message::Message
    where
        T: serde::Serialize + zbus::zvariant::Type,
    {
        zbus::message::Message::signal(NOTIF_PATH, iface, member)
            .expect("test signal builds")
            .sender(sender)
            .expect("test sender sets")
            .build(body)
            .expect("test body serializes")
    }

    fn owner() -> zbus::names::OwnedUniqueName {
        ":1.42".try_into().expect("test owner parses")
    }

    const BUS_DAEMON: &str = "org.freedesktop.DBus";

    #[test]
    fn decode_accepts_tuple_and_bare_id_forms() {
        assert_eq!(
            decode_id(&signal_body(
                ICEDTEA_IFACE,
                NOTIFICATION_ADDED_MEMBER,
                &(7u32,)
            )),
            Some(7)
        );
        let bare = signal_body(ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &7u32);
        assert_eq!(decode_id(&bare), Some(7));
    }

    #[test]
    fn decode_drops_malformed_bodies_without_panic() {
        let wrong_shape = signal_body(ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &("not-an-id",));
        assert_eq!(decode_id(&wrong_shape), None);
        assert_eq!(decode_bool(&wrong_shape), None);
        assert_eq!(decode_closed(&wrong_shape), None);
        assert_eq!(decode_removed(&wrong_shape), None);
        assert_eq!(decode_action(&wrong_shape), None);
        // A reasoned close still decodes; an Undefined reason is suppression.
        let closed = signal_body(STANDARD_IFACE, NOTIFICATION_CLOSED_MEMBER, &(4u32, 1u32));
        assert_eq!(decode_closed(&closed), Some(4));
        let suppressed = signal_body(
            ICEDTEA_IFACE,
            NOTIFICATION_REMOVED_MEMBER,
            &(5u32, CloseReason::Undefined),
        );
        assert_eq!(decode_removed(&suppressed), Some((5, true)));
        let really_closed = signal_body(
            ICEDTEA_IFACE,
            NOTIFICATION_REMOVED_MEMBER,
            &(6u32, CloseReason::Expired),
        );
        assert_eq!(decode_removed(&really_closed), Some((6, false)));
        // Actions decode id+key together; a dropped key kills the click.
        let action = signal_body(
            STANDARD_IFACE,
            ACTION_INVOKED_MEMBER,
            &(7u32, "default".to_string()),
        );
        assert_eq!(decode_action(&action), Some((7, "default".to_string())));
    }

    /// `classify` is the worker's whole dispatch decision, pure: every arm
    /// below drives it with stamped `Message::signal` messages and no bus.
    /// Gutting any arm (the plan's deletion gate) must break its test.
    #[test]
    fn classify_forwards_daemon_signals() {
        let o = owner();
        let added = signal_from(":1.42", ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &(7u32,));
        assert_eq!(
            classify(&added, &o),
            SignalAction::Forward(NotifUpdate::Added(7))
        );

        let suppressed = signal_from(
            ":1.42",
            ICEDTEA_IFACE,
            NOTIFICATION_REMOVED_MEMBER,
            &(5u32, CloseReason::Undefined),
        );
        assert_eq!(
            classify(&suppressed, &o),
            SignalAction::Forward(NotifUpdate::Suppressed(5))
        );

        let closed = signal_from(
            ":1.42",
            STANDARD_IFACE,
            NOTIFICATION_CLOSED_MEMBER,
            &(4u32, 1u32),
        );
        assert_eq!(
            classify(&closed, &o),
            SignalAction::Forward(NotifUpdate::Removed(4))
        );

        let action = signal_from(
            ":1.42",
            STANDARD_IFACE,
            ACTION_INVOKED_MEMBER,
            &(7u32, "default".to_string()),
        );
        assert_eq!(
            classify(&action, &o),
            SignalAction::Forward(NotifUpdate::ActionInvoked {
                id: 7,
                key: "default".to_string()
            })
        );

        let dnd = signal_from(":1.42", ICEDTEA_IFACE, DND_CHANGED_MEMBER, &(true,));
        assert_eq!(
            classify(&dnd, &o),
            SignalAction::Forward(NotifUpdate::DndChanged(true))
        );
    }

    #[test]
    fn classify_drops_impostors_and_unknown_members() {
        let o = owner();
        // Same path and member, wrong sender: a hostile peer's spoof.
        let spoof = signal_from(":9.99", ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &(7u32,));
        assert_eq!(classify(&spoof, &o), SignalAction::Drop);
        // Right sender, wrong interface.
        let wrong_iface = signal_from(
            ":1.42",
            "org.example.Evil",
            NOTIFICATION_ADDED_MEMBER,
            &(7u32,),
        );
        assert_eq!(classify(&wrong_iface, &o), SignalAction::Drop);
        // No sender at all (never happens on a real bus, must still drop).
        let nosender = signal_msg(ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &(7u32,));
        assert_eq!(classify(&nosender, &o), SignalAction::Drop);
        // Unknown member from the real daemon: visible drop, not silent.
        let unknown = signal_from(":1.42", ICEDTEA_IFACE, "SomethingNew", &(7u32,));
        assert_eq!(classify(&unknown, &o), SignalAction::Drop);
        // Malformed body on a known member: drop, never panic.
        let malformed = signal_from(":1.42", ICEDTEA_IFACE, NOTIFICATION_ADDED_MEMBER, &("x",));
        assert_eq!(classify(&malformed, &o), SignalAction::Drop);
    }

    #[test]
    fn classify_reconnects_only_on_bus_authenticated_owner_change() {
        let o = owner();
        // Genuine restart: bus daemon reports our name moved owners.
        let changed = signal_from(
            BUS_DAEMON,
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &(
                NOTIF_BUS_NAME.to_string(),
                ":1.10".to_string(),
                ":1.11".to_string(),
            ),
        );
        assert_eq!(classify(&changed, &o), SignalAction::Reconnect);
        // Someone else's name churn is none of our business.
        let other = signal_from(
            BUS_DAEMON,
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &(
                "org.example.Other".to_string(),
                ":1.10".to_string(),
                ":1.11".to_string(),
            ),
        );
        assert_eq!(classify(&other, &o), SignalAction::Drop);
        // Same-named signal from a non-bus sender: dropped, never obeyed.
        let forged = signal_from(
            ":9.99",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &(
                NOTIF_BUS_NAME.to_string(),
                ":1.10".to_string(),
                ":1.11".to_string(),
            ),
        );
        assert_eq!(classify(&forged, &o), SignalAction::Drop);
        // Undecodable body from the bus itself: fail closed (reconnect and
        // re-resolve) rather than idle on a possibly-stale pin.
        let garbled = signal_from(
            BUS_DAEMON,
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &("x",),
        );
        assert_eq!(classify(&garbled, &o), SignalAction::Reconnect);
    }

    #[test]
    fn backoff_resets_after_a_healthy_run_and_caps_at_30s() {
        use std::time::Duration;
        assert_eq!(
            next_backoff(Duration::from_secs(61), Duration::from_secs(16)),
            Duration::from_secs(1)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(0), Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(0), Duration::from_secs(1)),
            Duration::from_secs(2)
        );
        // Exactly 60s is not "healthy": strict `>` keeps the backoff.
        assert_eq!(
            next_backoff(Duration::from_secs(60), Duration::from_secs(8)),
            Duration::from_secs(16)
        );
    }

    #[test]
    fn mock_records_writes_and_stubs_reads() {
        let mock = MockNotifier::with_active(vec![mk("mail", 7)]);
        assert_eq!(mock.get_active().expect("stubbed").len(), 1);
        mock.invoke_action(7, "default");
        mock.close_notification(7);
        mock.set_dnd(true);
        assert_eq!(mock.invoked(), vec![(7, "default".to_string())]);
        assert_eq!(mock.closed(), vec![7]);
        assert_eq!(mock.get_dnd(), Some(true));
        assert_eq!(mock.dnd_sets(), vec![true]);
    }
}
