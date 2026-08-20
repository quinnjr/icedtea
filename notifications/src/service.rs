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

/// The `image-data` hint (and its deprecated `icon_data` alias)'s wire shape
/// is `(iiibiiay)`: width, height, rowstride, has-alpha, bits-per-sample,
/// channels, raw row-major pixel bytes. A sender that gets this wrong --
/// the hint absent, the wrong D-Bus type entirely, or a tuple with a
/// mismatched field count/arity -- degrades to `None` (falling back to the
/// `app_icon` argument) rather than breaking `Notify` for every other app.
/// See the design spec's hint-robustness risk and N3's hardening pass.
fn parse_image_data_hint(hints: &HashMap<String, OwnedValue>) -> Option<IconSource> {
    let raw = hints.get("image-data").or_else(|| hints.get("icon_data"))?;
    let value: Value = raw.clone().into();
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
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        // `image-data`/`icon_data` hint (raw pixels) takes priority over the
        // `app_icon` argument (a themed name or path), matching the
        // freedesktop spec's own precedence; a malformed/absent hint falls
        // back to `app_icon`.
        let icon = parse_image_data_hint(&hints)
            .unwrap_or(if app_icon.is_empty() { IconSource::None } else { IconSource::Named(app_icon) });
        let actions = pair_actions(actions);
        let urgency_byte = hints.get("urgency").and_then(|v| u8::try_from(v.clone()).ok());
        let urgency = parse_urgency(urgency_byte);
        let category = hints.get("category").and_then(|v| String::try_from(v.clone()).ok());
        let resident = hints.get("resident").and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false);
        let transient = hints.get("transient").and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false);
        let now = now_ms();

        let (id, change, expire_at_ms) = {
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

        self.0.reschedule(id, expire_at_ms, now);
        let _ = self.0.changes.send(change);
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.0.ticks.send(Tick::Cancel(id));
        if let Some(change) = self.0.store.lock().unwrap().close(id, CloseReason::ClosedByRequest) {
            let _ = self.0.changes.send(change);
        }
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
        let ids: Vec<u32> = store.active().iter().map(|n| n.id).collect();
        for id in ids {
            let _ = self.0.ticks.send(Tick::Cancel(id));
            if let Some(change) = store.close(id, CloseReason::Dismissed) {
                let _ = self.0.changes.send(change);
            }
        }
    }

    fn invoke_action(&self, id: u32, key: String) {
        if let Some(change) = self.0.store.lock().unwrap().invoke_action(id, &key) {
            let _ = self.0.changes.send(change);
        }
    }

    fn set_do_not_disturb(&self, on: bool) {
        if let Some(change) = self.0.store.lock().unwrap().set_dnd(on) {
            let _ = self.0.changes.send(change);
        }
    }

    fn get_do_not_disturb(&self) -> bool {
        self.0.store.lock().unwrap().dnd()
    }
}

/// Register both interfaces at [`NOTIF_PATH`], claim [`NOTIF_BUS_NAME`], and
/// start the emitter thread that drains `changes` and turns each `Change`
/// into the right signal(s). Requests the name with `DoNotQueue` only (no
/// `AllowReplacement`/`ReplaceExisting`): if another daemon already owns the
/// name, this returns `Err` and the caller (`main`) treats that as fatal;
/// this daemon itself is never replaceable, so it can't silently lose the
/// name to a later process either — see the design spec's Decision 6.
pub fn spawn(store: Arc<Mutex<Store>>, changes: Receiver<Change>, ticks: Sender<Tick>) -> zbus::Result<Connection> {
    let conn = Connection::session()?;

    let (svc_tx, svc_rx) = crossbeam_channel::unbounded();
    let shared = Arc::new(Shared { store, changes: svc_tx, ticks });

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
    let reply =
        conn.request_name_with_flags(NOTIF_BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())?;
    if reply != zbus::fdo::RequestNameReply::PrimaryOwner {
        // Another notification daemon already owns the name (`DoNotQueue`
        // yields `Exists` rather than queuing). Return an error so `main`'s
        // friendly message surfaces; never steal the name (Decision 6:
        // fatal + loud, never replace).
        return Err(zbus::Error::Failure(format!(
            "another notification daemon already owns {NOTIF_BUS_NAME} (got {reply:?})"
        )));
    }

    let emitter = conn.clone();
    std::thread::spawn(move || {
        let dest: Option<&str> = None;
        // Two sources feed one signal stream: `svc_rx` carries changes
        // produced synchronously by an interface method call (`Notify`,
        // `CloseNotification`, `DismissAll`, `InvokeAction`,
        // `SetDoNotDisturb`); `changes` (the `Receiver<Change>` passed in
        // from `main`) carries changes produced asynchronously by the
        // expiry worker (`Change::Closed(_, Expired)`). `Select` merges
        // them into one ordered emitter loop rather than needing two
        // separate emitter threads racing to emit on the same connection.
        let mut sel = crossbeam_channel::Select::new();
        let svc_idx = sel.recv(&svc_rx);
        let ext_idx = sel.recv(&changes);
        loop {
            let op = sel.select();
            let change = match op.index() {
                i if i == svc_idx => op.recv(&svc_rx),
                i if i == ext_idx => op.recv(&changes),
                _ => unreachable!(),
            };
            let change = match change {
                Ok(change) => change,
                Err(_) => break,
            };
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
    };
    if let Err(err) = result {
        tracing::warn!(error = %err, "failed to emit D-Bus signal");
    }
}
