//! The dedicated expiry-timer thread. A single min-heap of `(deadline, id)`
//! replaces per-notification timers — see the design spec's Decision 5.
//! No I/O beyond the two channels: `ticks` carries fresh deadlines and
//! cancellations in from the D-Bus service, `changes` carries
//! `Change::Closed(id, Expired)` out to the emitter thread.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::store::{Change, Store};

/// Wall-clock unix epoch in ms — matches the clock `Store` stores
/// `expire_at_ms` in, so [`Store::close_if_due`] compares like against like.
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A message from the D-Bus service thread to the expiry worker.
#[derive(Debug, Clone, Copy)]
pub enum Tick {
    /// A notification was (re)armed with a fresh expiry deadline.
    Deadline(u32, Instant),
    /// A notification was closed some other way (`CloseNotification`,
    /// `DismissAll`, an explicit re-`notify` that changed its deadline) —
    /// drop any pending heap entry for it so it can't double-close later.
    Cancel(u32),
}

/// Upper bound on how long a single `recv_timeout` call blocks when the heap
/// is empty, so the thread wakes periodically even with nothing scheduled
/// (harmless — it just loops back to blocking again) rather than parking on
/// an unbounded `recv()` that a test would have no deadline-based way to
/// reason about.
const IDLE_POLL: Duration = Duration::from_secs(3600);

/// Run the expiry worker loop. Blocks forever (or until `ticks` disconnects,
/// which only happens when the whole process is tearing down) — intended to
/// be the body of its own dedicated thread.
pub fn run(store: Arc<Mutex<Store>>, ticks: Receiver<Tick>, changes: Sender<Change>) {
    let mut heap: BinaryHeap<Reverse<(Instant, u32)>> = BinaryHeap::new();

    loop {
        let timeout = heap
            .peek()
            .map(|Reverse((deadline, _))| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(IDLE_POLL);

        match ticks.recv_timeout(timeout) {
            Ok(Tick::Deadline(id, at)) => heap.push(Reverse((at, id))),
            Ok(Tick::Cancel(id)) => heap.retain(|Reverse((_, i))| *i != id),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // Collect every heap entry whose (monotonic) deadline has passed,
        // then drain the batch under a single store lock (not one lock per
        // id). Each id is closed only if the store's *current* deadline is
        // genuinely due — `close_if_due` guards against a stale entry whose
        // notification was replaced/extended/suppressed after scheduling.
        let now = Instant::now();
        let mut due: Vec<u32> = Vec::new();
        while let Some(&Reverse((deadline, id))) = heap.peek() {
            if deadline > now {
                break;
            }
            heap.pop();
            due.push(id);
        }
        if !due.is_empty() {
            let now_ms = epoch_ms();
            let mut store = store.lock().unwrap();
            for id in due {
                if let Some(change) = store.close_if_due(id, now_ms) {
                    let _ = changes.send(change);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{CloseReason, IconSource, Urgency};
    use std::thread;

    fn store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::new(10)))
    }

    fn push(store: &Arc<Mutex<Store>>) -> u32 {
        store
            .lock()
            .unwrap()
            .notify(
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
                -1,
                0,
            )
            .0
    }

    #[test]
    fn pushed_deadline_fires_exactly_one_closed_expired() {
        let store = store();
        let id = push(&store);
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
        let store_clone = store.clone();
        let handle = thread::spawn(move || run(store_clone, tick_rx, chg_tx));

        tick_tx
            .send(Tick::Deadline(
                id,
                Instant::now() + Duration::from_millis(20),
            ))
            .unwrap();
        let change = chg_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("expiry fired");
        assert_eq!(change, Change::Closed(id, CloseReason::Expired));
        assert!(chg_rx.try_recv().is_err(), "fires exactly once");

        drop(tick_tx);
        let _ = handle.join();
    }

    #[test]
    fn cancel_removes_pending_deadline_so_it_never_fires() {
        let store = store();
        let id = push(&store);
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
        let store_clone = store.clone();
        let handle = thread::spawn(move || run(store_clone, tick_rx, chg_tx));

        tick_tx
            .send(Tick::Deadline(
                id,
                Instant::now() + Duration::from_millis(50),
            ))
            .unwrap();
        tick_tx.send(Tick::Cancel(id)).unwrap();
        // Give the worker time to process both before it would have fired.
        assert!(
            chg_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "cancelled deadline never fires"
        );

        drop(tick_tx);
        let _ = handle.join();
    }

    #[test]
    fn multiple_deadlines_fire_in_time_order() {
        let store = store();
        let a = push(&store);
        let b = push(&store);
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let (chg_tx, chg_rx) = crossbeam_channel::unbounded();
        let store_clone = store.clone();
        let handle = thread::spawn(move || run(store_clone, tick_rx, chg_tx));

        let now = Instant::now();
        // Send the later-firing one first to prove the heap orders by
        // deadline, not arrival order.
        tick_tx
            .send(Tick::Deadline(b, now + Duration::from_millis(100)))
            .unwrap();
        tick_tx
            .send(Tick::Deadline(a, now + Duration::from_millis(20)))
            .unwrap();

        let first = chg_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first fires");
        let second = chg_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second fires");
        assert_eq!(
            first,
            Change::Closed(a, CloseReason::Expired),
            "earlier deadline fires first"
        );
        assert_eq!(second, Change::Closed(b, CloseReason::Expired));

        drop(tick_tx);
        let _ = handle.join();
    }
}
