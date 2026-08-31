//! Contract §5's node-tree conformance gate.
//!
//! Each widget's retained `Node` subtree is rendered in GTK's own notation and
//! matched against the block vendored verbatim from GTK 4.22.4's sources. P6
//! extends this file with its own kinds; P8 only wires the whole set into the
//! gallery gate.

use icedtea_ui::view::{Kind, Prop, PropName, Props};
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

#[test]
fn a_label_renders_one_node_and_a_selection_subnode_only_when_selected() {
    // mutation: build the `selection` node unconditionally in LabelC::build and
    // the first assertion sees it in a non-selectable label.
    let mut plain = Props::default();
    plain.set(PropName::Label, Prop::Str("hello".into()));
    check(Kind::Label, "label", &plain);
    assert_eq!(node_tree_of(Kind::Label, &plain).trim(), "label");

    let mut selectable = plain.clone();
    selectable.set(PropName::Selectable, Prop::Bool(true));
    let rendered = node_tree_of(Kind::Label, &selectable);
    assert!(
        rendered.contains("selection"),
        "a selectable label gets one: {rendered}"
    );
    check(Kind::Label, "label", &selectable);
}

#[test]
fn a_spinner_and_a_statusbar_render_their_single_nodes() {
    // mutation: append any subnode in SpinnerC::build and this fails with
    // "rendered node at 'spinner/…' is not in the fixture".
    check(Kind::Spinner, "spinner", &Props::default());
    assert_eq!(
        node_tree_of(Kind::Spinner, &Props::default()).trim(),
        "spinner"
    );
    check(Kind::Statusbar, "statusbar", &Props::default());
}

#[test]
fn a_level_bar_builds_a_trough_of_filled_and_empty_blocks() {
    // mutation: emit one block instead of one per discrete step and the
    // discrete case renders a single `block` where the fixture wants both.
    use icedtea_ui::widgets::LevelBarMode;
    let mut props = Props::default();
    props.set(PropName::Value, Prop::Float(0.5));
    check(Kind::LevelBar, "level_bar", &props);

    let mut discrete = props.clone();
    discrete.set(PropName::Model, LevelBarMode::Discrete.to_prop());
    discrete.set(PropName::Upper, Prop::Float(4.0));
    let rendered = node_tree_of(Kind::LevelBar, &discrete);
    assert!(rendered.contains("levelbar.discrete"), "{rendered}");
    assert_eq!(
        rendered.matches("block").count(),
        4,
        "one block per step: {rendered}"
    );
    check(Kind::LevelBar, "level_bar", &discrete);
}

#[test]
fn a_progress_bar_shows_its_text_subnode_only_when_asked() {
    // mutation: build the `text` node unconditionally and the first assertion
    // finds it with show_text off.
    let mut props = Props::default();
    props.set(PropName::Fraction, Prop::Float(0.25));
    assert!(!node_tree_of(Kind::ProgressBar, &props).contains("text"));
    check(Kind::ProgressBar, "progress_bar", &props);

    props.set(PropName::ShowText, Prop::Bool(true));
    assert!(node_tree_of(Kind::ProgressBar, &props).contains("text"));
    check(Kind::ProgressBar, "progress_bar", &props);
}

#[test]
fn an_info_bar_carries_its_message_type_class_and_a_close_button() {
    // mutation: stop adding the message-type class and the `.warning`
    // assertion fails.
    use icedtea_ui::widgets::MessageType;
    let mut props = Props::default();
    props.set(PropName::MessageType, MessageType::Warning.to_prop());
    let rendered = node_tree_of(Kind::InfoBar, &props);
    assert!(rendered.starts_with("infobar.warning"), "{rendered}");
    check(Kind::InfoBar, "info_bar", &props);

    props.set(PropName::Buttons, Prop::Bool(true));
    let with_close = node_tree_of(Kind::InfoBar, &props);
    assert!(with_close.contains("button.close"), "{with_close}");
}

#[test]
fn a_scrollbar_nests_range_trough_and_slider_and_takes_its_orientation_class() {
    // mutation: drop the `trough` node and this fails with "fixture requires a
    // node at 'scrollbar/range/trough'".
    let mut props = Props::default();
    props.set(PropName::Orientation, Orientation::Vertical.to_prop());
    check(Kind::Scrollbar, "scrollbar", &props);
    assert!(node_tree_of(Kind::Scrollbar, &props).starts_with("scrollbar.vertical"));
}

#[test]
fn an_image_takes_its_icon_size_class_and_a_picture_takes_none() {
    // mutation: always add `.large-icons` and the Normal case fails.
    use icedtea_ui::widgets::IconSize;
    let mut props = Props::default();
    props.set(PropName::IconSize, IconSize::Large.to_prop());
    assert!(node_tree_of(Kind::Image, &props).starts_with("image.large-icons"));
    check(Kind::Image, "image", &props);

    props.set(PropName::IconSize, IconSize::Inherit.to_prop());
    assert_eq!(node_tree_of(Kind::Image, &props).trim(), "image");
    check(Kind::Picture, "picture", &Props::default());
}

#[test]
fn a_text_view_builds_its_four_borders_and_a_text_node() {
    // mutation: build three borders instead of four and this fails with
    // "fixture requires a node at 'textview/border'" for the missing side —
    // the four `border` nodes share a path, so drop the `.bottom` class
    // instead to see "required class 'bottom' missing".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("hello\nworld".into()));
    check(Kind::TextView, "text_view", &props);
    let rendered = node_tree_of(Kind::TextView, &props);
    assert!(rendered.starts_with("textview.view"), "{rendered}");
    assert_eq!(rendered.matches("border").count(), 4, "{rendered}");
}

#[test]
fn a_scale_builds_a_trough_with_a_highlight_and_one_mark_node_per_mark() {
    // mutation: skip the `indicator` subnode under each mark and this fails
    // with "fixture requires a node at 'scale/marks/mark/indicator'".
    let mut props = Props::default();
    props.set(PropName::Lower, Prop::Float(0.0));
    props.set(PropName::Upper, Prop::Float(100.0));
    props.set(PropName::Value, Prop::Float(50.0));
    check(Kind::Scale, "scale", &props);

    props.set(
        PropName::MarksTop,
        Prop::Classes(std::rc::Rc::from(vec![
            std::rc::Rc::from("0=Min"),
            std::rc::Rc::from("100=Max"),
        ])),
    );
    let rendered = node_tree_of(Kind::Scale, &props);
    assert_eq!(rendered.matches("mark\n").count(), 2, "{rendered}");
    assert!(rendered.starts_with("scale.marks-before"), "{rendered}");
    check(Kind::Scale, "scale", &props);
}

#[test]
fn window_controls_follow_the_decoration_layout_rule_verbatim() {
    // mutation: stop splitting the layout on ':' and the `start` side emits the
    // right-hand buttons too, failing the `close` assertion below.
    use icedtea_ui::widgets::{Side, WindowButton, window_controls::WindowControlsC};
    assert_eq!(
        WindowControlsC::tokens("menu:minimize,maximize,close", Side::Start),
        Vec::<WindowButton>::new(),
        "`menu` produces no child in 4.22.4 and nothing else is on the left"
    );
    assert_eq!(
        WindowControlsC::tokens("menu:minimize,maximize,close", Side::End),
        vec![
            WindowButton::Minimize,
            WindowButton::Maximize,
            WindowButton::Close
        ],
    );
    assert_eq!(
        WindowControlsC::tokens("icon,close:", Side::Start),
        vec![WindowButton::Icon, WindowButton::Close],
        "tokens are walked in order"
    );

    let mut props = Props::default();
    props.set(PropName::Side, Side::End.to_prop());
    props.set(
        PropName::Decoration,
        Prop::Str("menu:minimize,maximize,close".into()),
    );
    let rendered = node_tree_of(Kind::WindowControls, &props);
    assert!(rendered.contains("button.close"), "{rendered}");
    assert!(!rendered.contains(".empty"), "{rendered}");
    check(Kind::WindowControls, "window_controls", &props);

    props.set(PropName::Decoration, Prop::Str("menu:".into()));
    let empty = node_tree_of(Kind::WindowControls, &props);
    assert!(empty.starts_with("windowcontrols.end.empty"), "{empty}");
}

#[test]
fn a_drawing_area_is_one_widget_node() {
    // mutation: name the node "drawingarea" and this fails with "rendered node
    // at 'drawingarea' is not in the fixture".
    check(Kind::DrawingArea, "drawing_area", &Props::default());
}

#[test]
fn a_calendar_builds_its_header_and_a_grid_of_day_labels() {
    // mutation: emit 28 day labels regardless of month and the March
    // assertion below reports 31 != 28.
    use icedtea_ui::widgets::calendar::CalendarC;
    assert_eq!(CalendarC::days_in_month(2024, 2), 29, "2024 is a leap year");
    assert_eq!(CalendarC::days_in_month(1900, 2), 28, "1900 is not");
    assert_eq!(CalendarC::days_in_month(2026, 3), 31);
    assert_eq!(
        CalendarC::days_in_month(2026, 13),
        31,
        "an out-of-range month clamps"
    );
    assert_eq!(
        CalendarC::first_weekday(2026, 1),
        3,
        "1 Jan 2026 is a Thursday"
    );

    let mut props = Props::default();
    props.set(PropName::Row, Prop::Int(2026));
    props.set(PropName::Column, Prop::Int(3));
    props.set(PropName::Value, Prop::Float(15.0));
    check(Kind::Calendar, "calendar", &props);
    let rendered = node_tree_of(Kind::Calendar, &props);
    assert!(rendered.starts_with("calendar.view"), "{rendered}");
    assert_eq!(
        rendered.matches("label.day-number").count(),
        31,
        "{rendered}"
    );
}
