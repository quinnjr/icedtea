//! Contract §5's node-tree conformance gate.
//!
//! Each widget's retained `Node` subtree is rendered in GTK's own notation and
//! matched against the block vendored verbatim from GTK 4.22.4's sources. P6
//! extends this file with its own kinds; P8 only wires the whole set into the
//! gallery gate.

use icedtea_ui::view::{Kind, PropName, Props};
use icedtea_ui::widgets::{Orientation, WidgetEnum, fixture_matches, node_tree_of};

fn fixture(name: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gtk4.22-node-trees/"
    );
    std::fs::read_to_string(format!("{path}{name}.txt"))
        .unwrap_or_else(|e| panic!("missing fixture {name}.txt: {e}"))
}

fn check(kind: Kind, fixture_name: &str, props: &Props) {
    let rendered = node_tree_of(kind, props);
    if let Err(why) = fixture_matches(&fixture(fixture_name), &rendered) {
        panic!("{kind:?} does not match {fixture_name}.txt:\n{why}\nrendered:\n{rendered}");
    }
}

#[test]
fn a_separator_renders_one_node_carrying_its_orientation_class() {
    // mutation: drop the orientation class in SeparatorC::build and this fails
    // with "required class 'horizontal' missing".
    let mut props = Props::default();
    props.set(PropName::Orientation, Orientation::Horizontal.to_prop());
    check(Kind::Separator, "separator", &props);

    let mut vertical = Props::default();
    vertical.set(PropName::Orientation, Orientation::Vertical.to_prop());
    let rendered = node_tree_of(Kind::Separator, &vertical);
    assert_eq!(rendered.trim(), "separator.vertical");
}

#[test]
fn the_matcher_rejects_a_renamed_or_missing_subnode() {
    // mutation: make fixture_matches always return Ok(()) and this fails.
    assert!(fixture_matches("box\n╰── label", "box\n╰── label").is_ok());
    assert!(
        fixture_matches("box\n╰── label", "box\n╰── button").is_err(),
        "a renamed subnode must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── label", "box").is_err(),
        "a missing required subnode must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── [label]", "box").is_ok(),
        "an optional subnode may be absent"
    );
    assert!(
        fixture_matches("box.frame\n╰── label", "box\n╰── label").is_err(),
        "a missing always-class must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── <child>", "box\n╰── grid\n    ╰── label").is_ok(),
        "<child> admits an arbitrary subtree"
    );
}
