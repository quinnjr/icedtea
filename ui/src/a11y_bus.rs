//! AT-SPI bus connection for the in-process a11y tree (feature `a11y-bus`).
//!
//! This is M6's second accesskit slice, the sink the first one was designed
//! around: [`crate::a11y::A11yTree`] produces a full
//! [`accesskit::TreeUpdate`] every frame and this module hands it to
//! `accesskit_unix`'s platform adapter, which publishes it on the session
//! D-Bus for assistive technologies.
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

use std::sync::{Arc, Mutex};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};

use crate::a11y::A11yTree;

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
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.latest.lock().unwrap().clone()
    }
}

/// Nothing to drop: the tree lives in [`A11yBus`].
struct Deactivation;

impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {}
}

/// Records action requests from assistive technologies.
struct Actions {
    log: Arc<Mutex<Vec<ActionRequest>>>,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        self.log.lock().unwrap().push(request);
    }
}

/// Owns the AT-SPI adapter and publishes the in-process tree to it.
pub struct A11yBus {
    sink: Box<dyn BusSink>,
    latest: Arc<Mutex<Option<TreeUpdate>>>,
    actions: Arc<Mutex<Vec<ActionRequest>>>,
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
        let actions = Arc::new(Mutex::new(Vec::new()));
        let adapter = accesskit_unix::Adapter::new(
            Activation {
                latest: Arc::clone(&latest),
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
        }
    }

    /// Construct over a substitute sink, so tests can drive the publish path
    /// with no session bus. The activation cache still fills, so
    /// [`A11yBus::latest`] behaves as it does in production.
    #[must_use]
    pub fn with_sink(sink: Box<dyn BusSink>) -> Self {
        A11yBus {
            sink,
            latest: Arc::new(Mutex::new(None)),
            actions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Publish a full tree: cache it for activation and hand it to the sink.
    pub fn publish(&mut self, update: TreeUpdate) {
        *self.latest.lock().unwrap() = Some(update.clone());
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
        self.latest.lock().unwrap().clone()
    }

    /// Take every action request recorded since the last call. Dispatching
    /// these into `App` is the remaining follow-up.
    #[must_use]
    pub fn take_actions(&self) -> Vec<ActionRequest> {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}
