# icedtea Notification Daemon — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Roadmap slot:** Track A, item **A4** (`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`)
**Branch:** target `develop` (compositor/system track; toolkit-independent)

## Goal

A standalone, buildable-now crate that implements the standard
`org.freedesktop.Notifications` D-Bus service, so `notify-send`, libnotify
apps, and every desktop app that posts notifications (Firefox, Discord,
NetworkManager applets, etc.) stop silently failing on icedtea — one of the
roadmap's four hardest blockers to daily use. It owns the notification model,
a bounded history, expiration, and do-not-disturb, and exposes an
icedtea-specific query/control surface that the *future* popup UI (Track B,
item B3, gated on the pure-Rust toolkit rebuild) will render from. This spec
covers the daemon (backend + state) only; no popup surface, no rendering, no
toolkit dependency — same split the roadmap draws for A4 vs B3.

## Grounding: the pattern this follows

`icedtea-clipboard` (`clipboard/`) is the direct model — a small headless
daemon crate that owns a pure, no-I/O history model
(`clipboard/src/history.rs`), a zbus service that answers from a shared
snapshot and pushes commands into the model's owning thread
(`clipboard/src/service.rs`, `clipboard/src/manager.rs`), a `main.rs` that
wires them together (`clipboard/src/main.rs`), and a systemd user unit
(`clipboard/systemd/icedtea-clipboard.service`) matching
`PartOf=graphical-session.target` / `WantedBy=graphical-session.target`. The
`org.icedtea.Clipboard` IPC vocabulary lives in the shared `contract` crate
(`contract/src/clipboard.rs`), not in the daemon crate, so the future
consumer (there: `icedtea-shell`; here: the future notification popup) can
depend on the types without depending on the daemon's internals — the same
split `contract::event`/`contract::types` gives `org.icedtea.WM`.

Key difference from clipboard: this daemon has **no Wayland connection**. It
does not touch `wlr-data-control` or any compositor protocol; its only I/O is
D-Bus. That removes clipboard's whole reason for a self-pipe-multiplexed
event loop (`manager.rs`'s `rustix::event::poll` over two fds) — there is
only one external input source (D-Bus calls) plus a timer, so the daemon's
concurrency shape is simpler than clipboard's, not a copy of it.

## Decisions of record

1. **Two D-Bus interfaces, one object.** The daemon claims the well-known bus
   name `org.freedesktop.Notifications` (session bus, required verbatim —
   every notification-sending app looks this up) at object path
   `/org/freedesktop/Notifications`, and registers **two** interfaces on that
   one object:
   - `org.freedesktop.Notifications` — the standard freedesktop spec
     (`Notify`, `CloseNotification`, `GetCapabilities`,
     `GetServerInformation`, signals `NotificationClosed`/`ActionInvoked`).
     This is what every third-party sender talks to; zbus supports multiple
     `#[interface]` impls on one object path, so no second object is needed.
   - `org.icedtea.Notifications` — an icedtea extension for the future popup
     UI: query the active/history lists, control do-not-disturb, and invoke an
     action programmatically. Mirrors how `org.icedtea.Clipboard` sits
     alongside — there is no standard freedesktop protocol for "list my
     current notifications," so icedtea defines its own, the same way it
     defined its own clipboard-history surface.
2. **Model lives in `contract`, not the daemon crate.** New
   `contract/src/notifications.rs` (parallel to `contract/src/clipboard.rs`)
   defines `Notification`, `NotificationAction`, `Urgency`, `IconSource`,
   `CloseReason`, plus `NOTIF_BUS_NAME`/`NOTIF_PATH` consts — the wire types
   the future shell popup will consume, exactly as `ClipEntry` does today.
   Only the **icedtea** interface's types go in `contract`; the standard
   `org.freedesktop.Notifications` method signatures use plain D-Bus scalar
   types (`String`, `u32`, `Vec<String>`, `HashMap<String, Value>`) per the
   spec itself, not custom structs, so third-party clients need no icedtea
   type to talk to it.
3. **No disk persistence.** Like clipboard history, the notification store is
   pure in-memory (`Vec`/`VecDeque` behind an `Arc<Mutex<_>>>`), bounded, and
   reset on daemon restart. "Persistence" in the freedesktop capability sense
   means *within-session* history survival past a notification's own
   auto-dismiss (so a user can review what they missed), not cross-reboot
   storage — matching every reference daemon (dunst, mako) and matching how
   `icedtea-clipboard` never touches `redb`. Cross-reboot storage is not a use
   case for ephemeral notifications and is explicitly out of scope.
4. **Do-not-disturb is a daemon-owned, in-memory toggle**, not a config-crate
   field — it is transient session state (the way alt-tab state or the
   clipboard's own in-memory history is), not a persisted preference. It is
   exposed only via `org.icedtea.Notifications.{Get,Set}DoNotDisturb` and a
   `DoNotDisturbChanged` signal; there is no `org.freedesktop.Notifications`
   standard verb for it (the spec has none). While active, `Notify()` still
   returns a valid id and the notification still lands in history (so senders
   never see an error and nothing is lost), but it is excluded from the
   *active* list the popup UI reads unless its urgency is `Critical` —
   critical notifications (e.g. low battery, USB device fault) always surface,
   matching the near-universal DND convention every reference implementation
   (GNOME Shell, KDE Plasma, dunst) follows.
5. **Expiration is a single dedicated timer thread**, not per-notification
   timers. It holds a min-heap of `(deadline: Instant, id: u32)`, blocks on
   `channel.recv_timeout(next_deadline)`, and on wake either (a) accepts a
   freshly-`Notify()`'d deadline pushed onto the heap, or (b) pops and closes
   every entry whose deadline has passed, publishing `Change::Closed(id,
   Expired)` per entry. `expire_timeout` semantics per spec: `-1` = server
   default (a configurable constant, defaulting to 5000ms, matching dunst's
   default), `0` = never expire, `>0` = that many ms. **Critical urgency
   notifications ignore all three and never auto-expire** (the universal
   convention — they wait for the user or an explicit `CloseNotification`).
6. **Bus-name conflict is fatal, loud, and unreplaced** — same posture as
   `icedtea-clipboard` (`service::spawn`'s doc: "another daemon running" is
   fatal). `request_name` is called with default (non-replacing) zbus
   semantics: if another daemon (mako, dunst, a stray previous instance)
   already owns `org.freedesktop.Notifications`, `main()` exits with a clear
   "another notification daemon is already running — disable it before
   starting icedtea-notifications" error rather than silently stealing the
   name or silently no-op'ing. Auto-replacing a user's chosen third-party
   daemon out from under them is a worse failure mode than a loud exit.
7. **The daemon never renders anything and never times a popup's fade/dismiss
   animation** — that is entirely Track-B state once the toolkit exists. The
   daemon's only opinion is data: what exists, what's active, what's DND-
   suppressed, and when something auto-expired.
8. **systemd user unit**, `notifications/systemd/icedtea-notifications.service`,
   identical shape to `clipboard/systemd/icedtea-clipboard.service`
   (`Type=simple`, `PartOf=graphical-session.target`,
   `After=graphical-session.target`, `Restart=always`, `RestartSec=1`,
   `WantedBy=graphical-session.target`) — this daemon has no Wayland
   dependency, but it belongs to the same graphical-session lifecycle as the
   clipboard daemon and shell (started/stopped with the session, not
   system-wide).

## Architecture

### Component 1 — `contract` extension (`contract/src/notifications.rs`)

```rust
pub const NOTIF_BUS_NAME: &str = "org.freedesktop.Notifications";
pub const NOTIF_PATH: &str = "/org/freedesktop/Notifications";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum Urgency { Low, Normal, Critical }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum CloseReason { Expired, Dismissed, ClosedByRequest, Undefined }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum IconSource {
    None,
    /// A themed icon name or an absolute/`file://` path — the spec's
    /// `app_icon` argument and the deprecated `image-path` hint collapse to
    /// this; the popup UI resolves it through the icon theme or the
    /// filesystem, the daemon never interprets it.
    Named(String),
    /// The spec's raw `image-data`/`icon_data` hint: width, height,
    /// rowstride, has-alpha, bits-per-sample, channels, and the raw
    /// row-major pixel bytes, passed through unmodified.
    Pixels { width: i32, height: i32, rowstride: i32, has_alpha: bool,
              bits_per_sample: i32, channels: i32, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NotificationAction { pub key: String, pub label: String }

/// A stored notification as it crosses `org.icedtea.Notifications`. `id` is
/// the same id returned by the standard `Notify` call and used in
/// `NotificationClosed`/`ActionInvoked`/`CloseNotification` — one id space,
/// shared across both interfaces, so the popup UI's `id` always matches the
/// sender's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub icon: IconSource,
    pub summary: String,
    pub body: String,
    pub actions: Vec<NotificationAction>,
    pub urgency: Urgency,
    pub category: Option<String>,
    pub resident: bool,   // hint: don't auto-remove after an action is invoked
    pub transient: bool,  // hint: skip history, popup-only
    pub created_at_ms: u64,   // unix epoch ms, for popup ordering/relative time
    pub expire_at_ms: Option<u64>, // None => never (0) or critical
    pub suppressed: bool, // true while DND-hidden from the active list
}
```

`lib.rs` gains `pub mod notifications;` and re-exports, exactly like the
existing `pub use clipboard::{...}` line — this spec does not modify
`contract/src/lib.rs` (only cites the one-line addition a build task will
make).

### Component 2 — the store (`notifications/src/store.rs`)

Pure, no I/O — the `history.rs` analogue. Owns `Vec<Notification>` (insertion
order; the popup UI sorts/groups as it likes) plus `next_id: u32` (spec
requires nonzero, monotonically-issued-per-session ids; wraps past `u32::MAX`
back to 1, skipping 0, which is astronomically unlikely to collide with a
still-live notification but is handled by skipping any id still present) and
`dnd: bool`.

```rust
pub enum Change {
    Added(u32),
    Closed(u32, CloseReason),
    ActionInvoked(u32, String),
    DndChanged(bool),
}

impl Store {
    /// `replaces_id != 0` updates that entry in place (same id, spec
    /// semantics) and reports `Change::Added(id)` again (the standard
    /// re-notify path — e.g. a download-progress notification updating its
    /// percentage) rather than removing+re-adding, so identity is stable for
    /// the popup UI's animation state.
    pub fn notify(&mut self, app_name: String, replaces_id: u32, icon: IconSource,
                   summary: String, body: String, actions: Vec<NotificationAction>,
                   urgency: Urgency, category: Option<String>, resident: bool,
                   transient: bool, expire_timeout_ms: Option<u64>, now_ms: u64) -> (u32, Change);
    pub fn close(&mut self, id: u32, reason: CloseReason) -> Option<Change>;
    pub fn invoke_action(&mut self, id: u32, key: &str) -> Option<Change>;
    pub fn set_dnd(&mut self, on: bool) -> Option<Change>; // recomputes every `suppressed`
    pub fn active(&self) -> Vec<Notification>;  // !suppressed, still open
    pub fn history(&self) -> Vec<Notification>; // everything not yet closed... 
    // ...plus a bounded closed-history ring (HISTORY_MAX = 200, oldest evicted
    // first) for "what did I miss" — closed entries move here, mirroring
    // clipboard's `evict()` bound but on *closed* entries, not live ones (a
    // live/open notification is never evicted, only closed ones age out).
}
```

Urgency parsing from the `hints` map's `"urgency"` byte (`0/1/2`, default `1`
Normal per spec) and expiry-deadline computation
(`-1`→server default, `0`→`None` (never), `>0`→`now+n`, Critical→always
`None` regardless of the argument) are pure functions unit-tested directly —
same shape as clipboard's `make_preview`/`kind_of`/`pick_mime` free functions.

### Component 3 — expiry worker (`notifications/src/expiry.rs`)

```rust
pub enum Tick { Deadline(u32, Instant), Cancel(u32) }

pub fn run(store: Arc<Mutex<Store>>, ticks: Receiver<Tick>, changes: Sender<Change>) {
    let mut heap: BinaryHeap<Reverse<(Instant, u32)>> = BinaryHeap::new();
    loop {
        let timeout = heap.peek().map(|Reverse((deadline, _))| deadline.saturating_duration_since(Instant::now()));
        match ticks.recv_timeout(timeout.unwrap_or(Duration::from_secs(3600))) {
            Ok(Tick::Deadline(id, at)) => heap.push(Reverse((at, id))),
            Ok(Tick::Cancel(id)) => heap.retain(|Reverse((_, i))| *i != id),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        while let Some(&Reverse((deadline, id))) = heap.peek() {
            if deadline > now { break; }
            heap.pop();
            if let Some(change) = store.lock().unwrap().close(id, CloseReason::Expired) {
                let _ = changes.send(change);
            }
        }
    }
}
```

`Tick::Cancel` fires from `CloseNotification`/`close_by_action` so a
manually-dismissed notification's stale heap entry becomes a no-op instead of
double-closing (the `store.close` on the later pop simply finds nothing at
that id and returns `None`, but cancelling keeps the heap from growing
unboundedly across a long-running session).

### Component 4 — the D-Bus service (`notifications/src/service.rs`)

`Store` lives behind `Arc<Mutex<Store>>`, shared directly by the D-Bus
interface methods (no command-channel indirection is needed here, unlike
clipboard's manager thread, because there is no non-`Send` Wayland connection
to protect — the store is plain data any thread can lock). The service struct
holds the `Arc<Mutex<Store>>`, a `Sender<Change>` (to the emitter), and a
`Sender<Tick>` (to the expiry worker).

```rust
#[interface(name = "org.freedesktop.Notifications")]
impl NotificationService {
    fn notify(&self, app_name: String, replaces_id: u32, app_icon: String,
              summary: String, body: String, actions: Vec<String>,
              hints: HashMap<String, OwnedValue>, expire_timeout: i32) -> u32 { .. }
    fn close_notification(&self, id: u32) { .. } // -> CloseReason::ClosedByRequest
    fn get_capabilities(&self) -> Vec<String> {
        vec!["actions".into(), "body".into(), "body-markup".into(),
             "icon-static".into(), "persistence".into()]
    }
    fn get_server_information(&self) -> (String, String, String, String) {
        ("icedtea-notifications".into(), "icedtea".into(),
         env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

#[interface(name = "org.icedtea.Notifications")]
impl NotificationService {
    fn get_active(&self) -> Vec<Notification> { .. }
    fn get_history(&self) -> Vec<Notification> { .. }
    fn dismiss_all(&self) { .. } // closes every active entry, Dismissed
    fn invoke_action(&self, id: u32, key: String) { .. } // emits ActionInvoked
    fn set_do_not_disturb(&self, on: bool) { .. }
    fn get_do_not_disturb(&self) -> bool { .. }
}
```

`actions: Vec<String>` from the standard `Notify` call is the spec's flat
`[key1, label1, key2, label2, ...]` encoding; `notify()` pairs it into
`Vec<NotificationAction>` (an odd-length list is truncated, defensively —
the spec doesn't define the error, and dropping a trailing unpaired entry is
strictly safer than panicking on a malformed sender).

The emitter thread (parallel to clipboard's `service::spawn` emitter) drains
the `Receiver<Change>` and turns each into the right signal:

- `Change::Added(id)` → `org.icedtea.Notifications` `NotificationAdded(id)`
  only (no standard-spec signal exists for "a notification arrived" — every
  sender already knows, since `Notify()`'s return value told it).
- `Change::Closed(id, reason)` → `org.freedesktop.Notifications`
  `NotificationClosed(id, reason as u32)` **and** `org.icedtea.Notifications`
  `NotificationRemoved(id, reason)` — the standard signal is spec-mandated
  (senders may listen for their own notification closing); the icedtea one is
  for the popup UI to remove the card without re-deriving the reason.
- `Change::ActionInvoked(id, key)` → standard `ActionInvoked(id, key)`.
- `Change::DndChanged(on)` → `org.icedtea.Notifications`
  `DoNotDisturbChanged(on)`.

### Component 5 — `main.rs`

```rust
fn main() {
    let store = Arc::new(Mutex::new(Store::new(HISTORY_MAX)));
    let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (tick_tx, tick_rx) = crossbeam_channel::unbounded();

    std::thread::spawn({ let store = store.clone(); move || expiry::run(store, tick_rx, chg_tx.clone()) });

    let conn = service::spawn(store, chg_rx, tick_tx)
        .expect("failed to register org.freedesktop.Notifications (another daemon running?)");

    // No Wayland loop to drive; park the main thread — the zbus executor
    // thread(s) and the expiry thread do all the work. A `park()` loop
    // (not a busy spin) keeps the process alive until killed by systemd.
    loop { std::thread::park(); }
}
```

This is the one structural place the daemon diverges most visibly from
clipboard's `main.rs`: clipboard's `manager::run` *is* the blocking main-thread
loop (it owns the non-`Send` Wayland `Connection`); this daemon has no such
object, so `main` just wires channels and parks.

## Decomposition into milestones

1. **N1 — Model + contract types.** `contract/src/notifications.rs` (types,
   wire-signature lock test in clipboard's `clip_entry_wire_signature_is_locked`
   style) and `notifications/src/store.rs` (pure, unit-tested: replace-in-place,
   urgency parsing, expiry-deadline computation incl. critical-never-expires,
   DND suppression/unsuppression on toggle, bounded closed-history eviction).
   No D-Bus yet — buildable and fully tested standalone, exactly as
   `history.rs`'s tests need no Wayland or zbus.
2. **N2 — D-Bus service + expiry worker + main/systemd.** `service.rs`
   (both interfaces), `expiry.rs`, `main.rs`, `notifications/systemd/
   icedtea-notifications.service`. This is the deliverable that makes
   `notify-send "hi"` succeed against a real session bus and makes
   `busctl --user call org.freedesktop.Notifications /org/freedesktop/
   Notifications org.freedesktop.Notifications GetCapabilities` return real
   data.
3. **N3 — DND, history bound, and hardening pass.** `SetDoNotDisturb`/
   `GetDoNotDisturb`/`DoNotDisturbChanged`, `DismissAll`, the closed-history
   ring's eviction under load, and a review pass for the two things every
   D-Bus daemon in this repo has needed a fix for at least once (per the
   clipboard/session-lock/pointer-constraints history in project memory):
   malformed/adversarial input (empty `app_name`, huge `body`, an
   `expire_timeout` of `i32::MIN`, an `actions` list with an odd length,
   hints with the wrong variant type for `"urgency"`) and the bus-name-
   conflict path actually exiting cleanly rather than half-registering.

## Testing

- **Unit (store, pure):** replace-in-place keeps the same id and re-fires
  `Added`; a Critical notification's `expire_at_ms` is always `None`
  regardless of `expire_timeout` argument; `-1`/`0`/`>0` timeout semantics;
  DND suppresses Normal/Low but never Critical, and toggling DND off
  un-suppresses without re-adding; `close()` on an already-closed id is a
  no-op (`None`); closed-history eviction keeps exactly `HISTORY_MAX`,
  oldest-first, and never evicts a still-open notification.
- **Unit (expiry worker):** given a store and a synthetic clock/`Tick`
  sequence, a pushed deadline fires exactly one `Change::Closed(_, Expired)`
  at its time and not before; `Cancel` removes a pending deadline so it never
  fires; multiple deadlines fire in time order.
- **Integration (`zbus`, live session bus — the `settings/tests/live_apply.rs`
  pattern):** spawn the real `icedtea-notifications` binary as a subprocess
  against a scratch environment, skip visibly if the session bus is
  unavailable or `org.freedesktop.Notifications` is already owned (a real
  daemon or another test running concurrently); then, over a real `zbus`
  connection:
  - Call `Notify` with a normal-urgency notification, assert a nonzero id and
    that `org.icedtea.Notifications.GetActive` includes it.
  - Call `CloseNotification`, assert a `NotificationClosed(id, 3)` signal
    fires and the id leaves `GetActive`.
  - `Notify` with `expire_timeout=50`, assert `NotificationClosed(id, 1)`
    (Expired) fires within a short deadline without any explicit close call.
  - `SetDoNotDisturb(true)`, `Notify` a Normal-urgency one (absent from
    `GetActive`) and a Critical one (present), assert the split; toggle DND
    off, assert the Normal one reappears in `GetActive`.
  - `GetCapabilities`/`GetServerInformation` return the documented literal
    values.
  - Register an action, call `org.icedtea.Notifications.InvokeAction`, assert
    an `ActionInvoked(id, key)` signal on the standard interface.
- **Manual smoke test** (documented in the crate's doc comment, not
  automated): `notify-send -u critical "test" "body"` against a running
  `icedtea-notifications` instance succeeds where it previously errored with
  "no notification daemon".

## Risks

- **Bus-name contention with an already-running third-party daemon**
  (dunst/mako/GNOME's) on a dev machine — mitigated by decision #6 (fail
  loud, never steal); the live-bus integration tests skip visibly rather than
  fighting over the name, matching the existing `live_apply.rs`/`live_dbus.rs`
  posture.
- **Hint-type robustness** — `hints: a{sv}` is sender-controlled and
  loosely typed (e.g. `urgency` sent as the wrong D-Bus type, or an
  `image-data` tuple with a mismatched arity); every hint read must degrade
  to a documented default rather than panic or error the whole `Notify` call,
  since a malformed hint from one buggy app must never break notifications
  for every other app. Covered by N3's hardening pass.
- **No popup UI to validate end-to-end "did the user actually see it"** until
  Track B/B3 lands — this spec can only prove the backend is spec-correct and
  queryable, not that notifications are usable yet; accepted, matches the
  A4/B3 split the roadmap already draws.
- **`org.freedesktop.impl.portal.Notification`** (the portal interface for
  sandboxed/Flatpak apps' notifications) is a *related but separate* surface
  from `org.freedesktop.Notifications` — the xdg-desktop-portal design
  (`2026-08-20-icedtea-xdg-portal-design.md`) explicitly lists it as
  out-of-scope-for-now. When it's picked up, its natural implementation is a
  thin portal-side forward into this daemon's `Notify`, not a second
  notification store — noted here so that future work doesn't duplicate
  state.
- **Action semantics without a renderer** — `GetCapabilities` advertises
  `"actions"`; until Track B exists nothing calls
  `org.icedtea.Notifications.InvokeAction`, so action buttons are inert in
  practice today. This is honest (the daemon *does* support the full
  round-trip; only the "click a button" affordance is missing) and matches
  how `notify-send`/most senders already tolerate a daemon that stores but
  never visually renders actions.

## Out of scope

- **The popup UI itself** (Track B, item B3) — layer-shell surfaces, stacking,
  fade animations, click-to-dismiss, rendering icons/markup. This spec is the
  backend those surfaces will read from via `org.icedtea.Notifications`.
- **Sound** (`sound-file`/`suppress-sound` hints) — no audio backend exists in
  icedtea yet (PipeWire integration is unscoped cross-cutting work in the
  roadmap); hints are stored/ignored, not acted on.
- **Cross-reboot persistence** — see decision #3.
- **Per-app notification policy/settings pages** (e.g. "mute Discord") — that
  is Track B/B7 (settings breadth) once there's a UI to configure it from;
  the daemon's DND is a single global session toggle only.
- **`org.freedesktop.impl.portal.Notification`** — separate portal surface,
  noted under Risks; not built here.
- **Markup sanitization/rendering** of `body-markup`'s subset of Pango markup
  — the daemon passes `body` through verbatim; sanitizing/rendering it is a
  popup-UI (Track B) concern, not a backend one.
