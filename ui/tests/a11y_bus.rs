//! M6 accesskit bus slice: the AT-SPI adapter is constructed and fed the
//! in-process tree, and the activation seam serves the last full update.
//!
//! Requires the `a11y` and `a11y-bus` cargo features.
//!
//! The D-Bus half cannot be exercised headlessly — activation needs a real
//! assistive technology on the session bus — so the publish path is driven
//! through a recording [`BusSink`], and a second test proves the real
//! `accesskit_unix::Adapter` can be constructed and published to without a
//! session bus (it stays inert until activated).

#![cfg(all(feature = "a11y", feature = "a11y-bus"))]

use std::sync::{Arc, Mutex};

use accesskit::{Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
use icedtea_ui::a11y_bus::{A11yBus, BusSink};

#[derive(Clone, Default)]
struct Recording {
    updates: Arc<Mutex<Vec<TreeUpdate>>>,
    focused: Arc<Mutex<Vec<bool>>>,
}

impl BusSink for Recording {
    fn push(&mut self, update: TreeUpdate) {
        self.updates.lock().unwrap().push(update);
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused.lock().unwrap().push(focused);
    }
}

fn sample_update() -> TreeUpdate {
    let mut root = Node::new(Role::Window);
    root.set_children([NodeId(1)]);
    let mut button = Node::new(Role::Button);
    button.set_label("Save");
    TreeUpdate {
        nodes: vec![(NodeId(0), root), (NodeId(1), button)],
        tree: Some(TreeInfo::new(NodeId(0))),
        tree_id: TreeId::ROOT,
        focus: NodeId(1),
    }
}

#[test]
fn publishing_feeds_the_sink_and_fills_the_activation_cache() {
    let recorder = Recording::default();
    let mut bus = A11yBus::with_sink(Box::new(recorder.clone()));

    bus.publish(sample_update());
    bus.set_focused(true);

    let updates = recorder.updates.lock().unwrap();
    assert_eq!(updates.len(), 1, "the sink got the tree");
    assert_eq!(updates[0].focus, NodeId(1));
    assert_eq!(recorder.focused.lock().unwrap().as_slice(), &[true]);

    // The activation handler is served the same tree.
    let latest = bus.latest().expect("a cached tree");
    assert_eq!(latest.nodes.len(), 2);
    assert_eq!(latest.focus, NodeId(1));
}

#[test]
fn the_atspi_adapter_is_constructed_and_fed_without_a_session_bus() {
    // No AT is present, so the adapter stays inactive: `push` is a cache
    // write and a no-op platform update, never a panic. This is the seam a
    // live session turns into real bus traffic.
    let mut bus = A11yBus::new();
    assert!(bus.latest().is_none());
    bus.publish(sample_update());
    bus.set_focused(true);
    assert_eq!(
        bus.latest().map(|update| update.nodes.len()),
        Some(2),
        "the tree is cached for activation"
    );
    assert!(bus.take_actions().is_empty(), "no AT asked for anything");
}

#[test]
fn publishing_an_unbuilt_tree_is_a_no_op() {
    let recorder = Recording::default();
    let mut bus = A11yBus::with_sink(Box::new(recorder.clone()));
    let tree = icedtea_ui::a11y::A11yTree::new();

    bus.publish_tree(&tree);

    assert!(
        recorder.updates.lock().unwrap().is_empty(),
        "nothing pushed"
    );
    assert!(bus.latest().is_none(), "nothing cached");
    assert_eq!(bus.publish_count(), 0, "nothing counted");
}

#[test]
fn an_unactivated_bus_reports_inactive_and_counts_its_publishes() {
    let recorder = Recording::default();
    let mut bus = A11yBus::with_sink(Box::new(recorder.clone()));

    assert!(
        !bus.is_active(),
        "no assistive technology has activated the adapter"
    );
    bus.publish(sample_update());
    bus.publish(sample_update());

    assert_eq!(bus.publish_count(), 2, "each publish is counted");
    assert!(
        !bus.is_active(),
        "publishing with no AT never flips activation"
    );
    assert_eq!(recorder.updates.lock().unwrap().len(), 2);
}
