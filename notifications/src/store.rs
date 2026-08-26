//! The pure notification store: live notifications plus a bounded
//! closed-history ring, do-not-disturb suppression, and the id-allocation and
//! expiry-deadline rules the D-Bus layer (`service.rs`, `expiry.rs`) drives.
//! No I/O — mirrors `clipboard/src/history.rs`'s split.

use icedtea_contract::{CloseReason, IconSource, Notification, NotificationAction, Urgency};

/// Bound on the closed-history ring — oldest entries evicted first once full.
pub const HISTORY_MAX: usize = 200;

/// `expire_timeout == -1` (the D-Bus spec's "use the server default") resolves
/// to this many milliseconds, matching dunst's default.
pub const SERVER_DEFAULT_EXPIRE_MS: u64 = 5000;

/// What a mutation produced, so the service layer knows which signal(s) to
/// emit without re-deriving it from a before/after diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added(u32),
    Closed(u32, CloseReason),
    ActionInvoked(u32, String),
    DndChanged(bool),
    /// A live notification became DND-suppressed (hidden from the active
    /// list) without being closed — the service turns this into an
    /// `org.icedtea.Notifications` `NotificationRemoved` only (no standard
    /// `NotificationClosed`, since the notification is not actually closed),
    /// so a popup UI tracking add/remove stays consistent with `GetActive`.
    Suppressed(u32),
}

/// What `Store::set_dnd` changed: the new toggle value plus which live
/// notifications crossed the visibility boundary, so the service can cancel
/// or (re-)arm expiry and emit the matching add/remove signals (see the
/// design spec's DND<->expiry coordination).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndChange {
    pub on: bool,
    /// Ids that became suppressed (were visible, now hidden): cancel their
    /// expiry tick and emit a removal.
    pub newly_suppressed: Vec<u32>,
    /// Ids that became visible (were hidden, now shown), each paired with a
    /// freshly-recomputed `expire_at_ms` (a full countdown from "now", so a
    /// notification that sat out DND isn't instantly expired): re-arm expiry
    /// and emit an add.
    pub newly_visible: Vec<(u32, Option<u64>)>,
}

/// Resolve the freedesktop `expire_timeout` argument (`-1`/`0`/`>0`) to a
/// relative duration in ms, or `None` for "never expire". Any other negative
/// value (a malformed/adversarial sender) degrades to the same behavior as
/// `-1` rather than under/overflowing on the `as u64` cast.
pub fn resolve_expire_timeout(expire_timeout: i32) -> Option<u64> {
    match expire_timeout {
        0 => None,
        n if n > 0 => Some(n as u64),
        _ => Some(SERVER_DEFAULT_EXPIRE_MS), // -1, or any other negative value
    }
}

/// The absolute deadline (unix epoch ms) a fresh or replaced notification
/// should expire at, or `None` if it never auto-expires. Critical urgency
/// always overrides the argument and never expires, per the universal
/// convention every reference daemon follows.
pub fn compute_expire_at(urgency: Urgency, expire_timeout: i32, now_ms: u64) -> Option<u64> {
    if urgency == Urgency::Critical {
        return None;
    }
    resolve_expire_timeout(expire_timeout).map(|delta| now_ms.saturating_add(delta))
}

/// Parse the `hints["urgency"]` byte per spec: `0` Low, `1` Normal, `2`
/// Critical, anything else (including absent) defaults to Normal.
pub fn parse_urgency(byte: Option<u8>) -> Urgency {
    match byte {
        Some(0) => Urgency::Low,
        Some(2) => Urgency::Critical,
        _ => Urgency::Normal,
    }
}

/// Live notifications (insertion order) plus a bounded closed-history ring,
/// and the session's do-not-disturb toggle.
pub struct Store {
    items: Vec<Notification>,
    closed_history: std::collections::VecDeque<Notification>,
    history_max: usize,
    next_id: u32,
    dnd: bool,
}

impl Store {
    pub fn new(history_max: usize) -> Self {
        Store {
            items: Vec::new(),
            closed_history: std::collections::VecDeque::new(),
            history_max,
            next_id: 1,
            dnd: false,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = if self.next_id == u32::MAX {
                1
            } else {
                self.next_id + 1
            };
            // Skip 0 (spec requires nonzero), any still-live id, and any id
            // still in closed_history — after a `u32::MAX` wrap a reused id
            // that collided with a closed-history entry would let two
            // distinct notifications share an id in the same session.
            if id != 0
                && !self.items.iter().any(|n| n.id == id)
                && !self.closed_history.iter().any(|n| n.id == id)
            {
                return id;
            }
        }
    }

    /// `replaces_id != 0` updates that entry in place (same id, spec
    /// semantics) — the standard re-notify path (e.g. a download-progress
    /// notification updating its percentage) — rather than removing+re-adding,
    /// so identity is stable for the popup UI's animation state. If
    /// `replaces_id` doesn't match a live notification (already closed, or
    /// never existed), a fresh one is allocated instead — matching the D-Bus
    /// spec's own fallback.
    ///
    /// Returns `(id, suppressed, expire_at_ms)`: `suppressed` tells the
    /// service whether the notification is currently DND-hidden (so it must
    /// *not* emit `NotificationAdded` nor arm expiry — a suppressed
    /// notification neither shows nor counts down while hidden), and
    /// `expire_at_ms` is the deadline to arm when it *is* visible.
    #[allow(clippy::too_many_arguments)]
    pub fn notify(
        &mut self,
        app_name: String,
        replaces_id: u32,
        icon: IconSource,
        summary: String,
        body: String,
        actions: Vec<NotificationAction>,
        urgency: Urgency,
        category: Option<String>,
        resident: bool,
        transient: bool,
        expire_timeout: i32,
        now_ms: u64,
    ) -> (u32, bool, Option<u64>) {
        let expire_at_ms = compute_expire_at(urgency, expire_timeout, now_ms);
        let suppressed = self.dnd && urgency != Urgency::Critical;

        if replaces_id != 0
            && let Some(existing) = self.items.iter_mut().find(|n| n.id == replaces_id)
        {
            existing.app_name = app_name;
            existing.icon = icon;
            existing.summary = summary;
            existing.body = body;
            existing.actions = actions;
            existing.urgency = urgency;
            existing.category = category;
            existing.resident = resident;
            existing.transient = transient;
            existing.created_at_ms = now_ms;
            existing.expire_at_ms = expire_at_ms;
            existing.suppressed = suppressed;
            let id = existing.id;
            return (id, suppressed, expire_at_ms);
        }

        let id = self.alloc_id();
        let notification = Notification {
            id,
            app_name,
            icon,
            summary,
            body,
            actions,
            urgency,
            category,
            resident,
            transient,
            created_at_ms: now_ms,
            expire_at_ms,
            suppressed,
        };
        self.items.push(notification);
        (id, suppressed, expire_at_ms)
    }

    /// Close a live notification. A no-op (`None`) if `id` isn't live —
    /// already closed, or never existed — so a stale expiry-worker pop or a
    /// double `CloseNotification` call is harmless.
    pub fn close(&mut self, id: u32, reason: CloseReason) -> Option<Change> {
        let pos = self.items.iter().position(|n| n.id == id)?;
        let mut notification = self.items.remove(pos);
        // `transient` hints "skip history, popup-only" — it never enters the
        // closed-history ring.
        if !notification.transient {
            notification.suppressed = false;
            self.closed_history.push_back(notification);
            while self.closed_history.len() > self.history_max {
                self.closed_history.pop_front();
            }
        }
        Some(Change::Closed(id, reason))
    }

    /// Expire a notification **only if** its *current* stored deadline is
    /// actually due at `now_ms` — the expiry worker calls this instead of
    /// `close(id, Expired)` on a popped heap entry so a stale deadline can't
    /// wrongly close a notification whose deadline changed after the entry
    /// was scheduled. Skips closing when the notification: is no longer live
    /// (already closed — the entry is simply gone, no wrong-reason close);
    /// was replaced to never-expire (`expire_at_ms == None`); was extended
    /// (`expire_at_ms > now_ms`, a later heap entry will handle it); or is
    /// currently DND-suppressed (a hidden notification must not expire —
    /// its countdown is re-armed fresh when DND lifts).
    pub fn close_if_due(&mut self, id: u32, now_ms: u64) -> Option<Change> {
        let n = self.items.iter().find(|n| n.id == id)?;
        match n.expire_at_ms {
            Some(deadline) if !n.suppressed && deadline <= now_ms => {
                self.close(id, CloseReason::Expired)
            }
            _ => None,
        }
    }

    /// Record that an action was invoked on a still-live notification.
    /// `None` if `id` isn't live.
    pub fn invoke_action(&mut self, id: u32, key: &str) -> Option<Change> {
        self.items.iter().find(|n| n.id == id)?;
        Some(Change::ActionInvoked(id, key.to_string()))
    }

    /// The `resident` hint of a live notification, or `None` if `id` isn't
    /// live. `resident == false` means the notification is removed once one
    /// of its actions is invoked; `true` keeps it up.
    pub fn is_resident(&self, id: u32) -> Option<bool> {
        self.items.iter().find(|n| n.id == id).map(|n| n.resident)
    }

    /// Every live notification's id, **including DND-suppressed ones** —
    /// unlike [`active`](Self::active), which hides suppressed entries. Used
    /// by `DismissAll` so a "clear everything" also clears notifications
    /// hidden behind do-not-disturb.
    pub fn live_ids(&self) -> Vec<u32> {
        self.items.iter().map(|n| n.id).collect()
    }

    /// Toggle do-not-disturb and recompute every live notification's
    /// `suppressed` flag, reporting which notifications crossed the
    /// visibility boundary so the service can cancel/re-arm expiry and emit
    /// the matching signals. `None` if `on` matches the current state (no
    /// real change, so the service layer shouldn't emit anything).
    ///
    /// A notification that *becomes visible* (DND turned off) gets a **fresh
    /// countdown**: its `expire_at_ms` is recomputed as `now_ms + original
    /// timeout` (and `created_at_ms` reset to `now_ms`, keeping the timeout
    /// stable across repeated toggles), so a Normal notification posted under
    /// DND with a short timeout isn't instantly expired the moment it
    /// surfaces — it counts down from when the user could first see it.
    pub fn set_dnd(&mut self, on: bool, now_ms: u64) -> Option<DndChange> {
        if self.dnd == on {
            return None;
        }
        self.dnd = on;
        let dnd = self.dnd;
        let mut newly_suppressed = Vec::new();
        let mut newly_visible = Vec::new();
        for n in &mut self.items {
            let want_suppressed = dnd && n.urgency != Urgency::Critical;
            if want_suppressed == n.suppressed {
                continue;
            }
            if want_suppressed {
                n.suppressed = true;
                newly_suppressed.push(n.id);
            } else {
                n.suppressed = false;
                // Fresh countdown from now for a timed notification that sat
                // out DND; a never-expire one (`None`) stays never-expire.
                if let Some(deadline) = n.expire_at_ms {
                    let timeout = deadline.saturating_sub(n.created_at_ms);
                    n.created_at_ms = now_ms;
                    n.expire_at_ms = Some(now_ms.saturating_add(timeout));
                }
                newly_visible.push((n.id, n.expire_at_ms));
            }
        }
        Some(DndChange {
            on,
            newly_suppressed,
            newly_visible,
        })
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    /// Live, not DND-suppressed — what the popup UI should currently show.
    pub fn active(&self) -> Vec<Notification> {
        self.items
            .iter()
            .filter(|n| !n.suppressed)
            .cloned()
            .collect()
    }

    /// The full "what did I miss" timeline: the bounded closed-history ring
    /// (oldest first) followed by everything still live.
    pub fn history(&self) -> Vec<Notification> {
        self.closed_history
            .iter()
            .cloned()
            .chain(self.items.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32) -> NotificationAction {
        NotificationAction {
            key: format!("key{id}"),
            label: format!("Label {id}"),
        }
    }

    fn store() -> Store {
        Store::new(3)
    }

    fn push(store: &mut Store, urgency: Urgency, now_ms: u64) -> u32 {
        store
            .notify(
                "app".into(),
                0,
                IconSource::None,
                "summary".into(),
                "body".into(),
                vec![n(1)],
                urgency,
                None,
                false,
                false,
                -1,
                now_ms,
            )
            .0
    }

    // --- pure functions ---

    #[test]
    fn parse_urgency_defaults_to_normal() {
        assert_eq!(parse_urgency(Some(0)), Urgency::Low);
        assert_eq!(parse_urgency(Some(1)), Urgency::Normal);
        assert_eq!(parse_urgency(Some(2)), Urgency::Critical);
        assert_eq!(parse_urgency(None), Urgency::Normal);
        assert_eq!(
            parse_urgency(Some(200)),
            Urgency::Normal,
            "out-of-range degrades to default"
        );
    }

    #[test]
    fn compute_expire_at_follows_minus_one_zero_positive_semantics() {
        let now = 1_000_000;
        assert_eq!(
            compute_expire_at(Urgency::Normal, -1, now),
            Some(now + SERVER_DEFAULT_EXPIRE_MS),
            "-1 uses the server default"
        );
        assert_eq!(
            compute_expire_at(Urgency::Normal, 0, now),
            None,
            "0 never expires"
        );
        assert_eq!(
            compute_expire_at(Urgency::Normal, 2500, now),
            Some(now + 2500),
            ">0 is a relative ms timeout"
        );
    }

    #[test]
    fn compute_expire_at_critical_never_expires_regardless_of_timeout() {
        let now = 1_000_000;
        for timeout in [-1, 0, 1, 50_000, i32::MIN, i32::MAX] {
            assert_eq!(
                compute_expire_at(Urgency::Critical, timeout, now),
                None,
                "critical ignores expire_timeout={timeout}"
            );
        }
    }

    #[test]
    fn resolve_expire_timeout_handles_adversarial_negatives() {
        // Not just -1: any other negative value degrades to the same
        // server-default behavior instead of wrapping through `as u64`.
        assert_eq!(
            resolve_expire_timeout(i32::MIN),
            Some(SERVER_DEFAULT_EXPIRE_MS)
        );
        assert_eq!(resolve_expire_timeout(-2), Some(SERVER_DEFAULT_EXPIRE_MS));
    }

    // --- Store::notify / replace-in-place ---

    #[test]
    fn notify_replace_in_place_keeps_same_id_and_refires_added() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        let (replaced_id, suppressed, _) = s.notify(
            "app".into(),
            id,
            IconSource::None,
            "updated summary".into(),
            "updated body".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            false,
            -1,
            10,
        );
        assert_eq!(replaced_id, id, "replace keeps the same id");
        assert!(!suppressed, "not DND-suppressed");
        assert_eq!(s.active().len(), 1, "still exactly one live notification");
        assert_eq!(s.active()[0].summary, "updated summary");
    }

    #[test]
    fn notify_replaces_id_not_live_allocates_fresh() {
        let mut s = store();
        let (id, suppressed, _) = s.notify(
            "app".into(),
            999, // nothing has this id yet
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            false,
            -1,
            0,
        );
        assert_ne!(id, 0);
        assert!(!suppressed);
    }

    #[test]
    fn alloc_id_skips_ids_still_in_closed_history() {
        // Simulate a `u32::MAX` wrap landing next_id on an id that is closed
        // but still in the history ring: it must be skipped, not reused.
        let mut s = store();
        let closed = push(&mut s, Urgency::Normal, 0);
        s.close(closed, CloseReason::Dismissed);
        assert!(
            s.history().iter().any(|n| n.id == closed),
            "still in history"
        );
        s.next_id = closed; // force the collision
        let fresh = push(&mut s, Urgency::Normal, 0);
        assert_ne!(
            fresh, closed,
            "must not reuse an id still held in closed_history"
        );
    }

    #[test]
    fn notify_ids_are_nonzero_and_monotonic() {
        let mut s = store();
        let a = push(&mut s, Urgency::Normal, 0);
        let b = push(&mut s, Urgency::Normal, 0);
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        assert!(b > a);
    }

    // --- DND ---

    #[test]
    fn dnd_suppresses_normal_and_low_but_never_critical() {
        let mut s = store();
        s.set_dnd(true, 0);
        let low = push(&mut s, Urgency::Low, 0);
        let normal = push(&mut s, Urgency::Normal, 0);
        let critical = push(&mut s, Urgency::Critical, 0);
        let active_ids: Vec<u32> = s.active().iter().map(|n| n.id).collect();
        assert!(!active_ids.contains(&low));
        assert!(!active_ids.contains(&normal));
        assert!(active_ids.contains(&critical), "critical always surfaces");
    }

    #[test]
    fn dnd_toggle_off_unsuppresses_without_readding() {
        let mut s = store();
        s.set_dnd(true, 0);
        let id = push(&mut s, Urgency::Normal, 0);
        assert!(s.active().is_empty());
        let dc = s.set_dnd(false, 100).expect("state changed");
        assert!(!dc.on);
        assert!(dc.newly_suppressed.is_empty());
        assert_eq!(dc.newly_visible.len(), 1);
        assert_eq!(
            dc.newly_visible[0].0, id,
            "the suppressed one became visible"
        );
        let active = s.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id, "same id, not a new notification");
    }

    #[test]
    fn set_dnd_to_current_state_is_a_noop() {
        let mut s = store();
        assert!(s.set_dnd(false, 0).is_none(), "already off");
        s.set_dnd(true, 0);
        assert!(s.set_dnd(true, 0).is_none(), "already on");
    }

    #[test]
    fn dnd_off_gives_a_fresh_countdown_from_now_not_the_stale_deadline() {
        // Posted under DND at t=0 with a 5s server-default timeout: its
        // deadline is 5000. Lift DND far later (t=1_000_000): the notification
        // must not be already-past-due; its countdown restarts from now.
        let mut s = store();
        s.set_dnd(true, 0);
        let id = push(&mut s, Urgency::Normal, 0); // expire_at_ms == 5000
        let dc = s.set_dnd(false, 1_000_000).expect("state changed");
        let (vid, new_deadline) = dc.newly_visible[0];
        assert_eq!(vid, id);
        assert_eq!(
            new_deadline,
            Some(1_000_000 + SERVER_DEFAULT_EXPIRE_MS),
            "fresh countdown from the un-suppress moment, not the stale t=0 deadline"
        );
        // And the stored deadline is not in the past relative to un-suppress.
        assert!(s.active()[0].expire_at_ms.unwrap() > 1_000_000);
    }

    #[test]
    fn dnd_off_keeps_never_expire_as_never_expire() {
        let mut s = store();
        s.set_dnd(true, 0);
        // expire_timeout 0 => never expires
        let (id, _, _) = s.notify(
            "app".into(),
            0,
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            false,
            0,
            0,
        );
        let dc = s.set_dnd(false, 500).expect("changed");
        assert_eq!(
            dc.newly_visible,
            vec![(id, None)],
            "never-expire stays never-expire"
        );
    }

    #[test]
    fn dnd_on_reports_newly_suppressed_ids() {
        let mut s = store();
        let normal = push(&mut s, Urgency::Normal, 0);
        let critical = push(&mut s, Urgency::Critical, 0);
        let dc = s.set_dnd(true, 0).expect("changed");
        assert_eq!(
            dc.newly_suppressed,
            vec![normal],
            "only the normal one is hidden"
        );
        assert!(dc.newly_visible.is_empty());
        assert!(
            s.active().iter().any(|n| n.id == critical),
            "critical stays visible"
        );
    }

    // --- close ---

    #[test]
    fn close_on_already_closed_id_is_a_noop() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        assert!(s.close(id, CloseReason::Dismissed).is_some());
        assert_eq!(
            s.close(id, CloseReason::Dismissed),
            None,
            "second close is a no-op"
        );
    }

    #[test]
    fn close_moves_entry_into_history_and_removes_from_active() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        s.close(id, CloseReason::Dismissed);
        assert!(s.active().is_empty());
        assert!(s.history().iter().any(|n| n.id == id));
    }

    #[test]
    fn close_skips_history_for_transient_notifications() {
        let mut s = store();
        let (id, _, _) = s.notify(
            "app".into(),
            0,
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            true, // transient
            -1,
            0,
        );
        s.close(id, CloseReason::Dismissed);
        assert!(
            !s.history().iter().any(|n| n.id == id),
            "transient never enters history"
        );
    }

    // --- closed-history ring ---

    #[test]
    fn closed_history_ring_bounds_to_max_oldest_first() {
        let mut s = Store::new(2);
        let mut ids = vec![];
        for i in 0..4 {
            let id = push(&mut s, Urgency::Normal, i);
            s.close(id, CloseReason::Dismissed);
            ids.push(id);
        }
        let history = s.history();
        assert_eq!(history.len(), 2, "ring bounded to history_max");
        assert_eq!(
            history[0].id, ids[2],
            "oldest evicted, ring keeps the newest 2, oldest-first"
        );
        assert_eq!(history[1].id, ids[3]);
    }

    #[test]
    fn closed_history_never_evicts_a_still_open_notification() {
        let mut s = Store::new(1);
        let open_id = push(&mut s, Urgency::Normal, 0);
        for i in 1..5 {
            let id = push(&mut s, Urgency::Normal, i);
            s.close(id, CloseReason::Dismissed);
        }
        // The ring bound only applies to closed_history; the still-open
        // notification is never touched by eviction.
        assert!(s.active().iter().any(|n| n.id == open_id));
        assert!(s.history().iter().any(|n| n.id == open_id));
    }

    // --- invoke_action ---

    #[test]
    fn invoke_action_on_live_id_returns_change() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        assert_eq!(
            s.invoke_action(id, "key1"),
            Some(Change::ActionInvoked(id, "key1".into()))
        );
    }

    #[test]
    fn invoke_action_on_missing_id_is_none() {
        let mut s = store();
        assert_eq!(s.invoke_action(12345, "key1"), None);
    }

    // --- close_if_due (stale-deadline guard) ---

    #[test]
    fn close_if_due_closes_a_genuinely_due_notification() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0); // expire_at_ms == 5000
        assert_eq!(
            s.close_if_due(id, 5000),
            Some(Change::Closed(id, CloseReason::Expired)),
            "deadline reached => Expired"
        );
        assert!(s.active().is_empty());
    }

    #[test]
    fn close_if_due_skips_when_deadline_extended_by_replace() {
        // Stale heap entry pops at the old deadline, but a replace-in-place
        // pushed the deadline out — the old entry must not expire it.
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0); // expire_at_ms == 5000
        // Replace with a much longer timeout; new deadline 10 + 60_000.
        s.notify(
            "app".into(),
            id,
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            false,
            60_000,
            10,
        );
        assert_eq!(
            s.close_if_due(id, 5000),
            None,
            "extended deadline not yet due"
        );
        assert!(s.active().iter().any(|n| n.id == id), "still live");
    }

    #[test]
    fn close_if_due_skips_when_replaced_to_never_expire() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0); // expire_at_ms == 5000
        s.notify(
            "app".into(),
            id,
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            false,
            false,
            0, /* never */
            10,
        );
        assert_eq!(
            s.close_if_due(id, 1_000_000),
            None,
            "never-expire is never due"
        );
        assert!(s.active().iter().any(|n| n.id == id));
    }

    #[test]
    fn close_if_due_on_already_closed_id_is_a_noop() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        s.close(id, CloseReason::Dismissed);
        assert_eq!(
            s.close_if_due(id, 1_000_000),
            None,
            "already gone, no wrong-reason close"
        );
    }

    #[test]
    fn close_if_due_skips_a_suppressed_notification() {
        // A hidden (DND-suppressed) notification must not expire while hidden.
        let mut s = store();
        s.set_dnd(true, 0);
        let id = push(&mut s, Urgency::Normal, 0); // suppressed, expire_at_ms == 5000
        assert_eq!(
            s.close_if_due(id, 1_000_000),
            None,
            "suppressed never expires while hidden"
        );
        assert!(s.live_ids().contains(&id), "still live");
    }

    // --- live_ids / is_resident ---

    #[test]
    fn live_ids_includes_dnd_suppressed_entries() {
        let mut s = store();
        s.set_dnd(true, 0);
        let hidden = push(&mut s, Urgency::Normal, 0);
        let critical = push(&mut s, Urgency::Critical, 0);
        assert!(
            s.active().iter().all(|n| n.id != hidden),
            "hidden from active()"
        );
        let ids = s.live_ids();
        assert!(ids.contains(&hidden), "but present in live_ids()");
        assert!(ids.contains(&critical));
    }

    #[test]
    fn is_resident_reports_the_hint() {
        let mut s = store();
        let (resident_id, _, _) = s.notify(
            "app".into(),
            0,
            IconSource::None,
            "s".into(),
            "b".into(),
            vec![],
            Urgency::Normal,
            None,
            true, /* resident */
            false,
            -1,
            0,
        );
        let non_resident = push(&mut s, Urgency::Normal, 0);
        assert_eq!(s.is_resident(resident_id), Some(true));
        assert_eq!(s.is_resident(non_resident), Some(false));
        assert_eq!(s.is_resident(999), None, "unknown id");
    }
}
