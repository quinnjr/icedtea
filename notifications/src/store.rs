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
        Store { items: Vec::new(), closed_history: std::collections::VecDeque::new(), history_max, next_id: 1, dnd: false }
    }

    fn alloc_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = if self.next_id == u32::MAX { 1 } else { self.next_id + 1 };
            if id != 0 && !self.items.iter().any(|n| n.id == id) {
                return id;
            }
        }
    }

    /// `replaces_id != 0` updates that entry in place (same id, spec
    /// semantics) and reports `Change::Added(id)` again — the standard
    /// re-notify path (e.g. a download-progress notification updating its
    /// percentage) — rather than removing+re-adding, so identity is stable
    /// for the popup UI's animation state. If `replaces_id` doesn't match a
    /// live notification (already closed, or never existed), a fresh one is
    /// allocated instead — matching the D-Bus spec's own fallback.
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
    ) -> (u32, Change, Option<u64>) {
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
            return (id, Change::Added(id), expire_at_ms);
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
        (id, Change::Added(id), expire_at_ms)
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

    /// Record that an action was invoked on a still-live notification.
    /// `None` if `id` isn't live.
    pub fn invoke_action(&mut self, id: u32, key: &str) -> Option<Change> {
        self.items.iter().find(|n| n.id == id)?;
        Some(Change::ActionInvoked(id, key.to_string()))
    }

    /// Toggle do-not-disturb and recompute every live notification's
    /// `suppressed` flag. `None` if `on` matches the current state (no real
    /// change, so the service layer shouldn't emit `DndChanged`).
    pub fn set_dnd(&mut self, on: bool) -> Option<Change> {
        if self.dnd == on {
            return None;
        }
        self.dnd = on;
        for n in &mut self.items {
            n.suppressed = self.dnd && n.urgency != Urgency::Critical;
        }
        Some(Change::DndChanged(on))
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    /// Live, not DND-suppressed — what the popup UI should currently show.
    pub fn active(&self) -> Vec<Notification> {
        self.items.iter().filter(|n| !n.suppressed).cloned().collect()
    }

    /// The full "what did I miss" timeline: the bounded closed-history ring
    /// (oldest first) followed by everything still live.
    pub fn history(&self) -> Vec<Notification> {
        self.closed_history.iter().cloned().chain(self.items.iter().cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32) -> NotificationAction {
        NotificationAction { key: format!("key{id}"), label: format!("Label {id}") }
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
        assert_eq!(parse_urgency(Some(200)), Urgency::Normal, "out-of-range degrades to default");
    }

    #[test]
    fn compute_expire_at_follows_minus_one_zero_positive_semantics() {
        let now = 1_000_000;
        assert_eq!(
            compute_expire_at(Urgency::Normal, -1, now),
            Some(now + SERVER_DEFAULT_EXPIRE_MS),
            "-1 uses the server default"
        );
        assert_eq!(compute_expire_at(Urgency::Normal, 0, now), None, "0 never expires");
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
        assert_eq!(resolve_expire_timeout(i32::MIN), Some(SERVER_DEFAULT_EXPIRE_MS));
        assert_eq!(resolve_expire_timeout(-2), Some(SERVER_DEFAULT_EXPIRE_MS));
    }

    // --- Store::notify / replace-in-place ---

    #[test]
    fn notify_replace_in_place_keeps_same_id_and_refires_added() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        let (replaced_id, change, _) = s.notify(
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
        assert_eq!(change, Change::Added(id), "replace re-fires Added");
        assert_eq!(s.active().len(), 1, "still exactly one live notification");
        assert_eq!(s.active()[0].summary, "updated summary");
    }

    #[test]
    fn notify_replaces_id_not_live_allocates_fresh() {
        let mut s = store();
        let (id, change, _) = s.notify(
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
        assert_eq!(change, Change::Added(id));
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
        s.set_dnd(true);
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
        s.set_dnd(true);
        let id = push(&mut s, Urgency::Normal, 0);
        assert!(s.active().is_empty());
        assert_eq!(s.set_dnd(false), Some(Change::DndChanged(false)));
        let active = s.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id, "same id, not a new notification");
    }

    #[test]
    fn set_dnd_to_current_state_is_a_noop() {
        let mut s = store();
        assert_eq!(s.set_dnd(false), None, "already off");
        s.set_dnd(true);
        assert_eq!(s.set_dnd(true), None, "already on");
    }

    // --- close ---

    #[test]
    fn close_on_already_closed_id_is_a_noop() {
        let mut s = store();
        let id = push(&mut s, Urgency::Normal, 0);
        assert!(s.close(id, CloseReason::Dismissed).is_some());
        assert_eq!(s.close(id, CloseReason::Dismissed), None, "second close is a no-op");
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
        assert!(!s.history().iter().any(|n| n.id == id), "transient never enters history");
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
        assert_eq!(history[0].id, ids[2], "oldest evicted, ring keeps the newest 2, oldest-first");
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
        assert_eq!(s.invoke_action(id, "key1"), Some(Change::ActionInvoked(id, "key1".into())));
    }

    #[test]
    fn invoke_action_on_missing_id_is_none() {
        let mut s = store();
        assert_eq!(s.invoke_action(12345, "key1"), None);
    }
}
