//! AT-SPI bus connection for the in-process a11y tree (feature `a11y-bus`).
//!
//! This is M6's second accesskit slice, the sink the first one was designed
//! around: [`crate::a11y::A11yTree`] produces a full
//! [`accesskit::TreeUpdate`] every frame and this module hands it to
//! `accesskit_unix`'s platform adapter, which publishes it on the session
//! D-Bus for assistive technologies.
//!
//! **Unattached seam.** This module is not yet wired into the frame loop:
//! `App::run` owns the only place the retained tree, the layout and the
//! controllers are all live together, and nothing calls [`A11yBus::publish`]
//! from there yet. The adapter is therefore built and exercised only by this
//! crate's tests; a production caller has to construct an `A11yBus`, build the
//! tree after layout with
//! [`A11yTree::build_full_with_layout`](crate::a11y::A11yTree::build_full_with_layout),
//! call [`A11yBus::publish_tree`], drain [`A11yBus::take_actions`] and call
//! [`A11yBus::set_focused`]. Until that wiring lands, [`A11yBus::is_active`],
//! [`A11yBus::publish_count`] and the one-shot `warn!` on publishing while no
//! AT has activated the adapter are the health signals that say whether the
//! sink is doing anything.
//!
//! Version pairing: `accesskit_unix 0.23.0` (2026-08-29) depends on
//! `accesskit ^0.25`, the same schema `ui/Cargo.toml` pins, so the
//! `TreeUpdate` crosses the boundary unchanged — no downgrade or shim. The
//! spec's note that the bus crate "trails" was written against 0.22.x; the
//! 0.23 release closed that gap.
//!
//! The adapter is built lazily: nothing touches D-Bus until an assistive
//! technology asks for the tree, at which point
//! [`ActivationHandler::request_initial_tree`] returns the last full update.
//! Action requests (Click/Focus/SetValue) are recorded but not yet dispatched
//! back into `App` — that is the remaining follow-up the spec lists.
//!
//! [`ActivationHandler::request_initial_tree`]: accesskit::ActivationHandler::request_initial_tree

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};

use crate::a11y::A11yTree;

/// Most action requests kept between drains. ATs can be chatty while the
/// toolkit is not draining, so the log is a bounded drop-oldest ring rather
/// than an unbounded `Vec`.
const ACTION_LOG_CAP: usize = 256;

/// Lock `mutex`, recovering from a poisoned lock instead of panicking. A
/// panic in an `accesskit_unix` callback must not take the UI thread with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The sink a published [`TreeUpdate`] is handed to.
///
/// `accesskit_unix::Adapter` is the production implementation; tests
/// substitute a recorder so the publish path is exercised without a session
/// bus.
pub trait BusSink {
    /// Hand a full tree to the platform. Called on the UI thread; the
    /// adapter's own state machine decides whether it is live yet.
    fn push(&mut self, update: TreeUpdate);
    /// Tell the platform whether the window currently has input focus.
    fn set_focused(&mut self, focused: bool);
}

impl BusSink for accesskit_unix::Adapter {
    fn push(&mut self, update: TreeUpdate) {
        // `update_if_active` is a no-op until an AT activates the adapter;
        // the activation handler then returns the cached tree instead.
        self.update_if_active(|| update);
    }

    fn set_focused(&mut self, focused: bool) {
        self.update_window_focus_state(focused);
    }
}

/// Serves the last published tree to the adapter when it activates.
struct Activation {
    latest: Arc<Mutex<Option<TreeUpdate>>>,
    activated: Arc<AtomicBool>,
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        // An AT reached the adapter: this is the only signal `accesskit_unix`
        // gives that the bus is live (it swallows its own init errors).
        self.activated.store(true, Ordering::Relaxed);
        lock(&self.latest).clone()
    }
}

/// Nothing to drop: the tree lives in [`A11yBus`].
struct Deactivation;

impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {}
}

/// Records action requests from assistive technologies.
struct Actions {
    log: Arc<Mutex<VecDeque<ActionRequest>>>,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        let mut log = lock(&self.log);
        if log.len() >= ACTION_LOG_CAP {
            log.pop_front();
            tracing::warn!(
                cap = ACTION_LOG_CAP,
                "a11y: dropping the oldest AT action; the app is not draining requests"
            );
        }
        log.push_back(request);
    }
}

/// Owns the AT-SPI adapter and publishes the in-process tree to it.
pub struct A11yBus {
    sink: Box<dyn BusSink>,
    latest: Arc<Mutex<Option<TreeUpdate>>>,
    actions: Arc<Mutex<VecDeque<ActionRequest>>>,
    activated: Arc<AtomicBool>,
    published: u64,
    warned_inert: bool,
}

impl Default for A11yBus {
    fn default() -> Self {
        Self::new()
    }
}

impl A11yBus {
    /// Construct the real AT-SPI adapter. No D-Bus connection is made until
    /// an assistive technology activates it.
    #[must_use]
    pub fn new() -> Self {
        let latest = Arc::new(Mutex::new(None));
        let actions = Arc::new(Mutex::new(VecDeque::new()));
        let activated = Arc::new(AtomicBool::new(false));
        let adapter = accesskit_unix::Adapter::new(
            Activation {
                latest: Arc::clone(&latest),
                activated: Arc::clone(&activated),
            },
            Actions {
                log: Arc::clone(&actions),
            },
            Deactivation,
        );
        A11yBus {
            sink: Box::new(adapter),
            latest,
            actions,
            activated,
            published: 0,
            warned_inert: false,
        }
    }

    /// Construct over a substitute sink, so tests can drive the publish path
    /// with no session bus. The activation cache still fills, so
    /// [`A11yBus::latest`] behaves as it does in production.
    ///
    /// The substitute has no activation handler, so [`A11yBus::is_active`]
    /// stays false; that is the point — it is only ever true for an adapter an
    /// AT actually reached.
    #[must_use]
    pub fn with_sink(sink: Box<dyn BusSink>) -> Self {
        A11yBus {
            sink,
            latest: Arc::new(Mutex::new(None)),
            actions: Arc::new(Mutex::new(VecDeque::new())),
            activated: Arc::new(AtomicBool::new(false)),
            published: 0,
            warned_inert: false,
        }
    }

    /// Publish a full tree: cache it for activation and hand it to the sink.
    pub fn publish(&mut self, update: TreeUpdate) {
        *lock(&self.latest) = Some(update.clone());
        self.published += 1;
        if !self.activated.load(Ordering::Relaxed) && !self.warned_inert {
            self.warned_inert = true;
            // Warn once, not every frame: publishing with no AT present is
            // normal, but it means every value here is going nowhere.
            tracing::warn!(
                "a11y: publishing a tree before any assistive technology activated the adapter; \
                 the update is cached but reaches no bus"
            );
        }
        self.sink.push(update);
    }

    /// Publish the last update [`A11yTree`] produced. A tree that has not
    /// built yet publishes nothing.
    pub fn publish_tree(&mut self, tree: &A11yTree) {
        if let Some(update) = tree.last_update() {
            self.publish(update.clone());
        }
    }

    /// Announce the window's input-focus state to the platform.
    pub fn set_focused(&mut self, focused: bool) {
        self.sink.set_focused(focused);
    }

    /// The tree an activation would be served right now, if any.
    #[must_use]
    pub fn latest(&self) -> Option<TreeUpdate> {
        lock(&self.latest).clone()
    }

    /// Whether an assistive technology has activated the adapter. Until this
    /// is true the bus has not reached any AT, which is the failure
    /// `accesskit_unix` swallows internally.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.activated.load(Ordering::Relaxed)
    }

    /// How many trees [`A11yBus::publish`] has handed to the sink.
    #[must_use]
    pub fn publish_count(&self) -> u64 {
        self.published
    }

    /// Take every action request recorded since the last call. Dispatching
    /// these into `App` is the remaining follow-up.
    #[must_use]
    pub fn take_actions(&self) -> Vec<ActionRequest> {
        lock(&self.actions).drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::{NodeId, TreeId};

    fn request() -> ActionRequest {
        ActionRequest {
            action: accesskit::Action::Click,
            target_tree: TreeId::ROOT,
            target_node: NodeId(1),
            data: None,
        }
    }

    #[test]
    fn the_action_log_drops_oldest_at_its_bound() {
        let log = Arc::new(Mutex::new(VecDeque::new()));
        let mut actions = Actions {
            log: Arc::clone(&log),
        };
        for _ in 0..(ACTION_LOG_CAP + 5) {
            actions.do_action(request());
        }
        assert_eq!(
            lock(&log).len(),
            ACTION_LOG_CAP,
            "the log is a bounded ring, not an unbounded Vec"
        );
    }
}
