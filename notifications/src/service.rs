//! The two D-Bus interfaces registered at [`NOTIF_PATH`]: the standard
//! `org.freedesktop.Notifications` (what every sender talks to) and the
//! icedtea extension `org.icedtea.Notifications` (what the future popup UI
//! will talk to). zbus's `#[interface]` macro implements the `Interface`
//! trait once per macro invocation, and a type can't implement the same
//! trait twice — so this is two thin wrapper structs sharing one
//! [`Shared`] core via `Arc`, both registered at the same object path,
//! rather than two `#[interface]` blocks on one struct (which wouldn't
//! compile).
//!
//! Unlike the clipboard service (which forwards every call onto a
//! non-`Send` Wayland-owning thread via a command channel), `Store` is
//! plain data any thread can lock directly — see the design spec's framing
//! of why this daemon's concurrency shape is simpler. Method impls lock
//! [`Shared::store`], mutate it, and push the resulting `Change` onto
//! `Shared::changes` for the emitter thread below to turn into signals.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use icedtea_contract::{CloseReason, IconSource, Notification, NotificationAction, NOTIF_BUS_NAME, NOTIF_PATH};
use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Structure, Value};

use crate::expiry::Tick;
use crate::store::{parse_urgency, Change, Store};

/// Well-known bus name for the icedtea extension interface. The standard
/// interface is served at the same object path under [`NOTIF_BUS_NAME`]
/// (the freedesktop name, which *is* the well-known name the whole object
/// is registered under) — there is no separate bus name for
/// `org.icedtea.Notifications`; it rides the same connection/name, the same
/// way `org.icedtea.WM`'s and `org.icedtea.Clipboard`'s own interfaces each
/// get their own bus name, but here the icedtea interface deliberately
/// shares the standard one's name and object path rather than claiming a
/// second bus name, since a client that already knows the notifications
/// object (every sender does) can introspect it for the extension
/// interface without a second connection.
const ICEDTEA_INTERFACE: &str = "org.icedtea.Notifications";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Standard freedesktop close-reason wire codes — fixed by the spec, not the
/// same as [`CloseReason`]'s Rust enum discriminant order.
fn reason_code(reason: CloseReason) -> u32 {
    match reason {
        CloseReason::Expired => 1,
        CloseReason::Dismissed => 2,
        CloseReason::ClosedByRequest => 3,
        CloseReason::Undefined => 4,
    }
}

/// The standard `Notify` call's `actions` argument is a flat
/// `[key1, label1, key2, label2, ...]` list; an odd trailing entry (a
/// malformed/adversarial sender) is dropped rather than causing an error —
/// the spec doesn't define one, and dropping is strictly safer than a panic
/// that would break notifications for every other app.
fn pair_actions(flat: Vec<String>) -> Vec<NotificationAction> {
    let mut actions = Vec::with_capacity(flat.len() / 2);
    let mut it = flat.into_iter();
    while let (Some(key), Some(label)) = (it.next(), it.next()) {
        actions.push(NotificationAction { key, label });
    }
    actions
}

/// Take a scalar hint out of the map by key, converting it to `T` and
/// dropping it whether or not the conversion succeeds. Removing (rather than
/// cloning) means a large value — notably `image-data`'s pixel buffer — is
/// never duplicated. Returns `None` when the key is absent or the sender sent
/// the wrong D-Bus type, so every hint degrades to its default instead of
/// erroring the whole `Notify` call.
fn hint<T>(hints: &mut HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    hints.remove(key).and_then(|v| T::try_from(v).ok())
}

/// The `image-data` hint (and its deprecated `icon_data` alias)'s wire shape
/// is `(iiibiiay)`: width, height, rowstride, has-alpha, bits-per-sample,
/// channels, raw row-major pixel bytes. A sender that gets this wrong --
/// the hint absent, the wrong D-Bus type entirely, or a tuple with a
/// mismatched field count/arity -- degrades to `None` (falling back to the
/// `image-path` hint / `app_icon` argument) rather than breaking `Notify` for
/// every other app. See the design spec's hint-robustness risk and N3's
/// hardening pass. Takes the value out of the map by ownership so the (large)
/// pixel buffer is moved, never cloned.
fn parse_image_data_hint(hints: &mut HashMap<String, OwnedValue>) -> Option<IconSource> {
    let raw = hints.remove("image-data").or_else(|| hints.remove("icon_data"))?;
    let value: Value = raw.into();
    let structure = Structure::try_from(value).ok()?;
    let fields = structure.into_fields();
    let [width, height, rowstride, has_alpha, bits_per_sample, channels, data]: [Value; 7] =
        fields.try_into().ok()?;
    Some(IconSource::Pixels {
        width: i32::try_from(width).ok()?,
        height: i32::try_from(height).ok()?,
        rowstride: i32::try_from(rowstride).ok()?,
        has_alpha: bool::try_from(has_alpha).ok()?,
        bits_per_sample: i32::try_from(bits_per_sample).ok()?,
        channels: i32::try_from(channels).ok()?,
        data: Vec::<u8>::try_from(data).ok()?,
    })
}

/// State shared by both interface wrappers.
struct Shared {
    store: Arc<Mutex<Store>>,
    changes: Sender<Change>,
    ticks: Sender<Tick>,
}

impl Shared {
    /// Schedule (or reschedule) `id`'s expiry deadline. Always cancels any
    /// prior pending tick for `id` first — covers both a fresh notification
    /// (a no-op cancel, nothing pending yet) and `Notify`'s replace-in-place
    /// path (a live notification getting a new deadline), so a stale timer
    /// from the *previous* deadline can never fire early against the
    /// replaced one. `expire_at_ms` of `None` (never-expire, or critical)
    /// leaves nothing scheduled after the cancel.
    fn reschedule(&self, id: u32, expire_at_ms: Option<u64>, now_ms: u64) {
        let _ = self.ticks.send(Tick::Cancel(id));
        if let Some(deadline_ms) = expire_at_ms {
            let delay = Duration::from_millis(deadline_ms.saturating_sub(now_ms));
            let _ = self.ticks.send(Tick::Deadline(id, Instant::now() + delay));
        }
    }

    /// Cancel `id`'s pending expiry tick and close it with `reason`, emitting
    /// the resulting `Change` if it was live. Shared by `CloseNotification`,
    /// `DismissAll`, and the non-resident-action removal path so all three
    /// treat "cancel timer + close + emit" identically. The caller passes the
    /// already-locked store guard.
    fn cancel_and_close(&self, store: &mut Store, id: u32, reason: CloseReason) {
        let _ = self.ticks.send(Tick::Cancel(id));
        if let Some(change) = store.close(id, reason) {
            let _ = self.changes.send(change);
        }
    }
}

pub struct StdInterface(Arc<Shared>);
pub struct IcedteaInterface(Arc<Shared>);

#[interface(name = "org.freedesktop.Notifications")]
impl StdInterface {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        mut hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        // Icon precedence per the freedesktop spec: raw pixels
        // (`image-data`/`icon_data`) win, then the `image-path` hint (a themed
        // name or filesystem path), then the `app_icon` argument, then
        // nothing. See `contract::notifications::IconSource`.
        let icon = parse_image_data_hint(&mut hints)
            .or_else(|| hint::<String>(&mut hints, "image-path").filter(|p| !p.is_empty()).map(IconSource::Named))
            .unwrap_or(if app_icon.is_empty() { IconSource::None } else { IconSource::Named(app_icon) });
        let actions = pair_actions(actions);
        let urgency = parse_urgency(hint::<u8>(&mut hints, "urgency"));
        let category = hint::<String>(&mut hints, "category");
        let resident = hint::<bool>(&mut hints, "resident").unwrap_or(false);
        let transient = hint::<bool>(&mut hints, "transient").unwrap_or(false);
        let now = now_ms();

        let (id, suppressed, expire_at_ms) = {
            let mut store = self.0.store.lock().unwrap();
            store.notify(
                app_name,
                replaces_id,
                icon,
                summary,
                body,
                actions,
                urgency,
                category,
                resident,
                transient,
                expire_timeout,
                now,
            )
        };

        if suppressed {
            // DND-hidden: do not announce it as Added and do not arm expiry
            // (a hidden notification must not count down and vanish). Still
            // cancel any prior tick in case this was a replace-in-place of a
            // notification that used to be visible.
            self.0.reschedule(id, None, now);
        } else {
            // #7: push Added BEFORE arming expiry, so on a single ordered
            // change stream Added always precedes any Closed for this id
            // (even a 1ms timeout can't emit Closed first).
            let _ = self.0.changes.send(Change::Added(id));
            self.0.reschedule(id, expire_at_ms, now);
        }
        id
    }

    fn close_notification(&self, id: u32) {
        let mut store = self.0.store.lock().unwrap();
        self.0.cancel_and_close(&mut store, id, CloseReason::ClosedByRequest);
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["actions".into(), "body".into(), "body-markup".into(), "icon-static".into(), "persistence".into()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("icedtea-notifications".into(), "icedtea".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

#[interface(name = "org.icedtea.Notifications")]
impl IcedteaInterface {
    fn get_active(&self) -> Vec<Notification> {
        self.0.store.lock().unwrap().active()
    }

    fn get_history(&self) -> Vec<Notification> {
        self.0.store.lock().unwrap().history()
    }

    fn dismiss_all(&self) {
        let mut store = self.0.store.lock().unwrap();
        // Iterate *all* live notifications (`live_ids`), not just `active()`:
        // "clear everything" must also clear DND-suppressed ones, which
        // `active()` hides.
        for id in store.live_ids() {
            self.0.cancel_and_close(&mut store, id, CloseReason::Dismissed);
        }
    }

    fn invoke_action(&self, id: u32, key: String) {
        let mut store = self.0.store.lock().unwrap();
        let Some(change) = store.invoke_action(id, &key) else {
            return;
        };
        let _ = self.0.changes.send(change);
        // `resident == false` (the default): the notification is removed once
        // an action is invoked. `resident == true` keeps it up. See the spec.
        if store.is_resident(id) == Some(false) {
            self.0.cancel_and_close(&mut store, id, CloseReason::Dismissed);
        }
    }

    fn set_do_not_disturb(&self, on: bool) {
        let now = now_ms();
        let dnd_change = { self.0.store.lock().unwrap().set_dnd(on, now) };
        let Some(dc) = dnd_change else {
            return;
        };
        let _ = self.0.changes.send(Change::DndChanged(dc.on));
        // Notifications that just became hidden: cancel their expiry (a
        // hidden notification must not count down) and emit a removal so a
        // UI tracking add/remove stays consistent with GetActive.
        for id in dc.newly_suppressed {
            let _ = self.0.ticks.send(Tick::Cancel(id));
            let _ = self.0.changes.send(Change::Suppressed(id));
        }
        // Notifications that just became visible: announce them and (re-)arm
        // their fresh countdown. Added is emitted before arming (#7 ordering).
        for (id, expire_at_ms) in dc.newly_visible {
            let _ = self.0.changes.send(Change::Added(id));
            self.0.reschedule(id, expire_at_ms, now);
        }
    }

    fn get_do_not_disturb(&self) -> bool {
        self.0.store.lock().unwrap().dnd()
    }
}

/// Why [`spawn`] failed. Distinguished so `main` can exit *cleanly* (never
/// restarted by systemd) on a bus-name conflict — the expected "another
/// daemon is running" case — while a genuine D-Bus fault still exits with a
/// failure code so systemd's `Restart=on-failure` can retry.
#[derive(Debug)]
pub enum SpawnError {
    /// Another daemon already owns `org.freedesktop.Notifications`. Fatal but
    /// expected: log loudly and exit cleanly, never steal the name.
    NameTaken(String),
    /// A genuine D-Bus error (no session bus, registration failure, ...).
    Bus(zbus::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::NameTaken(msg) => write!(f, "{msg}"),
            SpawnError::Bus(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<zbus::Error> for SpawnError {
    fn from(err: zbus::Error) -> Self {
        SpawnError::Bus(err)
    }
}

/// Register both interfaces at [`NOTIF_PATH`], claim [`NOTIF_BUS_NAME`], and
/// start the emitter thread that drains `changes_rx` and turns each `Change`
/// into the right signal(s). `changes_tx` is the send half the interface
/// methods push onto; the expiry worker holds a clone of the *same* sender,
/// so every `Change` — whether produced synchronously by a method call or
/// asynchronously by an expiry — flows through one ordered channel (no
/// `Select` racing two channels, which could otherwise emit a Closed before
/// its own Added; see #7). Requests the name with `DoNotQueue` only (no
/// `AllowReplacement`/`ReplaceExisting`): if another daemon already owns the
/// name, this returns [`SpawnError::NameTaken`] and the caller (`main`)
/// treats that as fatal-but-clean; this daemon itself is never replaceable,
/// so it can't silently lose the name to a later process either — see the
/// design spec's Decision 6.
pub fn spawn(
    store: Arc<Mutex<Store>>,
    changes_tx: Sender<Change>,
    changes_rx: Receiver<Change>,
    ticks: Sender<Tick>,
) -> Result<Connection, SpawnError> {
    let conn = Connection::session()?;

    let shared = Arc::new(Shared { store, changes: changes_tx, ticks });

    conn.object_server().at(NOTIF_PATH, StdInterface(shared.clone()))?;
    conn.object_server().at(NOTIF_PATH, IcedteaInterface(shared))?;
    // `request_name`'s default flags are `AllowReplacement | ReplaceExisting
    // | DoNotQueue` (see zbus's own fdo/dbus.rs test), which would let us
    // steal the name from a replacement-allowing owner and would mark
    // *this* daemon replaceable so a later process could silently steal it
    // back — the opposite of "fatal+loud" on name conflict. `DoNotQueue`
    // alone (no `AllowReplacement`/`ReplaceExisting`) is what the design
    // spec's Decision 6 actually calls for: fail loudly if anyone else owns
    // the name, and never let anyone take it from us.
    let name_taken =
        || SpawnError::NameTaken(format!("another notification daemon already owns {NOTIF_BUS_NAME}"));
    // A conflict surfaces two ways depending on the bus: as `Err(NameTaken)`
    // from `request_name_with_flags` itself, or as an `Ok(_)` reply that
    // isn't `PrimaryOwner` (`DoNotQueue` yields `Exists` rather than queuing).
    // Both mean "someone else owns it" and both must exit cleanly (Decision 6:
    // fatal + loud, never steal the name), so map both to `NameTaken` — never
    // the generic `Bus` variant that `main` would treat as a restartable
    // failure and crash-loop on.
    match conn.request_name_with_flags(NOTIF_BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {}
        Ok(_) => return Err(name_taken()),
        Err(zbus::Error::NameTaken) => return Err(name_taken()),
        Err(err) => return Err(SpawnError::Bus(err)),
    }

    let emitter = conn.clone();
    std::thread::spawn(move || {
        let dest: Option<&str> = None;
        // One ordered change stream feeds the emitter: interface methods and
        // the expiry worker both send on clones of `changes_tx`, so ordering
        // between an Added and a later Closed for the same id is preserved by
        // the channel's FIFO (no `Select` to reorder them).
        while let Ok(change) = changes_rx.recv() {
            emit(&emitter, dest, change);
        }
    });

    Ok(conn)
}

fn emit(conn: &Connection, dest: Option<&str>, change: Change) {
    let result = match change {
        Change::Added(id) => conn.emit_signal(dest, NOTIF_PATH, ICEDTEA_INTERFACE, "NotificationAdded", &(id,)),
        Change::Closed(id, reason) => {
            let std_result =
                conn.emit_signal(dest, NOTIF_PATH, NOTIF_BUS_NAME, "NotificationClosed", &(id, reason_code(reason)));
            let icedtea_result =
                conn.emit_signal(dest, NOTIF_PATH, ICEDTEA_INTERFACE, "NotificationRemoved", &(id, reason));
            std_result.and(icedtea_result)
        }
        Change::ActionInvoked(id, key) => {
            conn.emit_signal(dest, NOTIF_PATH, NOTIF_BUS_NAME, "ActionInvoked", &(id, key))
        }
        Change::DndChanged(on) => {
            conn.emit_signal(dest, NOTIF_PATH, ICEDTEA_INTERFACE, "DoNotDisturbChanged", &(on,))
        }
        // A DND-suppressed notification is *hidden*, not closed: emit only the
        // icedtea `NotificationRemoved` (so a popup UI drops the card and
        // stays consistent with GetActive), never the standard
        // `NotificationClosed` (the sender must not think its notification
        // went away). `Undefined` marks "removed from the active view without
        // a real close reason".
        Change::Suppressed(id) => {
            conn.emit_signal(dest, NOTIF_PATH, ICEDTEA_INTERFACE, "NotificationRemoved", &(id, CloseReason::Undefined))
        }
    };
    if let Err(err) = result {
        tracing::warn!(error = %err, "failed to emit D-Bus signal");
    }
}
