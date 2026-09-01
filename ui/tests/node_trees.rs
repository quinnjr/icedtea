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

#[test]
fn a_popover_always_carries_background_and_wraps_its_child_in_contents() {
    // mutation: append the child directly to the popover node instead of to
    // `contents` and this fails with "rendered node at 'popover/label' is not
    // in the fixture".
    check(Kind::Popover, "popover", &Props::default());
    let rendered = node_tree_of(Kind::Popover, &Props::default());
    assert!(rendered.starts_with("popover.background"), "{rendered}");
    assert!(rendered.contains("arrow"), "{rendered}");
    assert!(rendered.contains("contents"), "{rendered}");
}

#[test]
fn the_three_button_kinds_share_the_button_node_and_differ_by_class() {
    // mutation: stop adding `.text-button` from the content and the first
    // assertion fails — GTK sets it from what the button actually contains.
    let mut labelled = Props::default();
    labelled.set(PropName::Label, Prop::Str("Ok".into()));
    assert!(node_tree_of(Kind::Button, &labelled).starts_with("button.text-button"));
    check(Kind::Button, "button", &labelled);

    assert!(node_tree_of(Kind::ToggleButton, &labelled).starts_with("button.toggle"));
    check(Kind::ToggleButton, "toggle_button", &labelled);

    let mut link = labelled.clone();
    link.set(PropName::Uri, Prop::Str("https://gtk.org".into()));
    assert!(node_tree_of(Kind::LinkButton, &link).contains("button.link"));
    check(Kind::LinkButton, "link_button", &link);
}

#[test]
fn a_check_button_names_its_indicator_check_and_switches_builtin_when_grouped() {
    // mutation: return Builtin::Check unconditionally from CheckButtonC::builtin
    // and the grouped assertion fails.
    use icedtea_ui::icons::builtin::Builtin;
    use icedtea_ui::widgets::check_button::CheckButtonC;

    let mut props = Props::default();
    props.set(PropName::Label, Prop::Str("Enable".into()));
    check(Kind::CheckButton, "check_button", &props);
    let rendered = node_tree_of(Kind::CheckButton, &props);
    assert!(
        rendered.starts_with("checkbutton.text-button"),
        "{rendered}"
    );
    assert!(rendered.contains("check"), "{rendered}");

    props.set(PropName::Group, Prop::Str("mode".into()));
    let grouped = node_tree_of(Kind::CheckButton, &props);
    assert!(
        grouped.contains("checkbutton.text-button.grouped"),
        "{grouped}"
    );
    // GTK keeps the node named `check` and swaps the *builtin* to a radio.
    assert!(grouped.contains("check"), "{grouped}");
    assert_eq!(CheckButtonC::builtin_for(true, false), Builtin::Radio);
    assert_eq!(CheckButtonC::builtin_for(false, false), Builtin::Check);
    assert_eq!(
        CheckButtonC::builtin_for(false, true),
        Builtin::CheckIndeterminate
    );
    check(Kind::CheckButton, "check_button", &props);
}

#[test]
fn a_switch_has_two_images_and_a_slider() {
    // mutation: build one image and this fails with "fixture requires a node at
    // 'switch/image'" only after both are gone — so drop the slider instead to
    // see the failure immediately.
    check(Kind::Switch, "switch", &Props::default());
    let rendered = node_tree_of(Kind::Switch, &Props::default());
    assert_eq!(rendered.matches("image").count(), 2, "{rendered}");
    assert!(rendered.contains("slider"), "{rendered}");
}

#[test]
fn a_menu_button_wraps_a_toggle_button_and_a_drop_down_holds_a_popover_list() {
    // mutation: name the menubutton's child `button` without `.toggle` and the
    // matcher reports "required class 'toggle' missing".
    check(Kind::MenuButton, "menu_button", &Props::default());
    let rendered = node_tree_of(Kind::MenuButton, &Props::default());
    assert!(rendered.contains("button.toggle"), "{rendered}");

    let mut props = Props::default();
    props.set(
        PropName::Model,
        Prop::Items(std::rc::Rc::from(vec![
            icedtea_ui::widgets::ListItem::new(0, "One"),
            icedtea_ui::widgets::ListItem::new(1, "Two"),
        ])),
    );
    check(Kind::DropDown, "drop_down", &props);
    let dropdown = node_tree_of(Kind::DropDown, &props);
    // Reconciliation: the plan counts the bare substring "row", but "arrow"
    // ends in "row" and `GtkDropDown:show-arrow` defaults to TRUE, so the
    // row count is taken off the row node's own full name instead.
    assert_eq!(dropdown.matches("row.activatable").count(), 2, "{dropdown}");
    assert!(dropdown.contains("popover.background.menu"), "{dropdown}");
}

#[test]
fn a_drop_downs_search_filter_never_panics_and_honours_its_mode() {
    // mutation: use `starts_with` for Substring and the Substring assertion
    // returns an empty vec.
    use icedtea_ui::widgets::drop_down::DropDownC;
    use icedtea_ui::widgets::{ListItem, MatchMode};
    let items = vec![
        ListItem::new(0, "Alpha"),
        ListItem::new(1, "beta"),
        ListItem::new(2, "\u{00e9}clair"),
    ];
    assert_eq!(
        DropDownC::filter(&items, "", MatchMode::Substring),
        vec![0, 1, 2]
    );
    assert_eq!(
        DropDownC::filter(&items, "et", MatchMode::Substring),
        vec![1]
    );
    assert_eq!(DropDownC::filter(&items, "be", MatchMode::Prefix), vec![1]);
    assert_eq!(DropDownC::filter(&items, "beta", MatchMode::Exact), vec![1]);
    assert!(DropDownC::filter(&items, "\u{00e9}", MatchMode::Prefix).contains(&2));
    // Hostile input: a lone surrogate cannot exist in a &str, but a very long
    // needle and a needle longer than every haystack must both be fine.
    assert!(DropDownC::filter(&items, &"x".repeat(100_000), MatchMode::Substring).is_empty());
}

#[test]
fn the_dialog_buttons_and_their_dialog_bodies_match_gtks_trees() {
    // mutation: name the colour button's child `button` without `.color` and
    // the matcher reports "required class 'color' missing".
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;

    let mut colour = Props::default();
    colour.set(
        PropName::Value,
        Prop::Float(ColorDialogC::pack(Rgba {
            r: 0.2,
            g: 0.5,
            b: 0.9,
            a: 1.0,
        })),
    );
    check(Kind::ColorDialogButton, "color_dialog_button", &colour);
    check(Kind::ColorDialog, "color_dialog", &colour);
    let body = node_tree_of(Kind::ColorDialog, &colour);
    assert!(body.starts_with("window.dialog"), "{body}");
    assert!(body.matches("colorswatch").count() >= 2, "{body}");

    let mut font = Props::default();
    font.set(PropName::Text, Prop::Str("Cantarell 11".into()));
    check(Kind::FontDialogButton, "font_dialog_button", &font);
    check(Kind::FontDialog, "font_dialog", &font);
}

#[test]
fn a_packed_colour_round_trips_exactly() {
    // mutation: pack with `* 255.0` and no rounding and the round trip drifts.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;
    for rgba in [
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        },
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        Rgba {
            r: 0.2,
            g: 0.5,
            b: 0.9,
            a: 0.5,
        },
    ] {
        let back = ColorDialogC::unpack(ColorDialogC::pack(rgba));
        for (a, b) in [
            (rgba.r, back.r),
            (rgba.g, back.g),
            (rgba.b, back.b),
            (rgba.a, back.a),
        ] {
            assert!((a - b).abs() <= 1.0 / 255.0, "{a} vs {b}");
        }
    }
    // Hostile input: NaN and out-of-range packs must clamp, not panic.
    let _ = ColorDialogC::unpack(f64::NAN);
    let _ = ColorDialogC::unpack(-1.0);
    let _ = ColorDialogC::unpack(f64::MAX);
}

#[test]
fn an_entry_renders_the_shared_text_subtree_and_its_optional_icons() {
    // mutation: skip the `placeholder` node in TextEditState::build and this
    // fails with "fixture requires a node at 'entry/text/placeholder'".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("hello".into()));
    check(Kind::Entry, "entry", &props);
    let bare = node_tree_of(Kind::Entry, &props);
    assert!(bare.contains("undershoot.left"), "{bare}");
    assert!(
        !bare.contains("progress"),
        "no progress node until asked: {bare}"
    );

    props.set(PropName::Fraction, Prop::Float(0.4));
    let with_progress = node_tree_of(Kind::Entry, &props);
    assert!(with_progress.contains("progress"), "{with_progress}");
    check(Kind::Entry, "entry", &props);
}

#[test]
fn the_search_and_password_entries_carry_their_classes_and_indicators() {
    // mutation: drop the `.search` class in SearchEntryC::build and the matcher
    // reports "required class 'search' missing".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("q".into()));
    check(Kind::SearchEntry, "search_entry", &props);
    assert!(node_tree_of(Kind::SearchEntry, &props).starts_with("entry.search"));

    check(Kind::PasswordEntry, "password_entry", &props);
    let password = node_tree_of(Kind::PasswordEntry, &props);
    assert!(password.starts_with("entry.password"), "{password}");
    assert!(password.contains("image.caps-lock-indicator"), "{password}");
}

#[test]
fn a_spin_button_has_two_stepper_buttons_and_an_editable_label_has_a_stack() {
    // mutation: name the steppers `button.up`/`button.down` in the wrong order
    // and the fixture's `╰── button.up` last-child position fails.
    use icedtea_ui::widgets::spin_button::SpinButtonC;
    assert_eq!(SpinButtonC::format(1.5, 2), "1.50");
    assert_eq!(SpinButtonC::format(1.5, 0), "2");
    assert_eq!(
        SpinButtonC::format(f64::NAN, 2),
        "0.00",
        "NaN never renders as NaN"
    );

    let mut props = Props::default();
    props.set(PropName::Value, Prop::Float(3.0));
    props.set(PropName::Lower, Prop::Float(0.0));
    props.set(PropName::Upper, Prop::Float(10.0));
    check(Kind::SpinButton, "spin_button", &props);

    let mut label = Props::default();
    label.set(PropName::Text, Prop::Str("Name".into()));
    check(Kind::EditableLabel, "editable_label", &label);
    let rendered = node_tree_of(Kind::EditableLabel, &label);
    assert!(rendered.contains("stack"), "{rendered}");
}

/// Every kind P5 owns, in contract §5.1–§5.3 order.
const P5_KINDS: &[(Kind, &str)] = &[
    (Kind::Label, "label"),
    (Kind::Spinner, "spinner"),
    (Kind::Statusbar, "statusbar"),
    (Kind::LevelBar, "level_bar"),
    (Kind::ProgressBar, "progress_bar"),
    (Kind::InfoBar, "info_bar"),
    (Kind::Scrollbar, "scrollbar"),
    (Kind::Image, "image"),
    (Kind::Picture, "picture"),
    (Kind::Separator, "separator"),
    (Kind::TextView, "text_view"),
    (Kind::Scale, "scale"),
    (Kind::DrawingArea, "drawing_area"),
    (Kind::WindowControls, "window_controls"),
    (Kind::Calendar, "calendar"),
    (Kind::Popover, "popover"),
    (Kind::Button, "button"),
    (Kind::ToggleButton, "toggle_button"),
    (Kind::LinkButton, "link_button"),
    (Kind::CheckButton, "check_button"),
    (Kind::MenuButton, "menu_button"),
    (Kind::Switch, "switch"),
    (Kind::DropDown, "drop_down"),
    (Kind::ColorDialogButton, "color_dialog_button"),
    (Kind::ColorDialog, "color_dialog"),
    (Kind::FontDialogButton, "font_dialog_button"),
    (Kind::FontDialog, "font_dialog"),
    (Kind::Entry, "entry"),
    (Kind::SearchEntry, "search_entry"),
    (Kind::PasswordEntry, "password_entry"),
    (Kind::SpinButton, "spin_button"),
    (Kind::EditableLabel, "editable_label"),
];

#[test]
fn every_p5_kind_has_a_fixture_and_matches_it_with_default_props() {
    // mutation: delete any entry from P5_KINDS and the count assertion fails;
    // delete a fixture file and `fixture()` panics by name.
    assert_eq!(
        P5_KINDS.len(),
        32,
        "contract §5.1-§5.3 owns exactly 32 kinds"
    );
    for (kind, name) in P5_KINDS {
        check(*kind, name, &Props::default());
    }
}

#[test]
fn no_p5_kind_falls_through_to_the_unimplemented_controller() {
    // mutation: remove any dispatch arm from build_controller and that kind's
    // tree renders as a bare node, failing its fixture's required subnodes.
    // Kinds whose GTK tree really is one bare node are listed explicitly, so
    // this test cannot be satisfied by accident.
    const BARE: &[Kind] = &[
        Kind::Label,
        Kind::Spinner,
        Kind::InfoBar,
        Kind::WindowControls,
        Kind::Separator,
        Kind::Image,
        Kind::Picture,
        Kind::DrawingArea,
        Kind::Button,
        Kind::ToggleButton,
    ];
    for (kind, _) in P5_KINDS {
        let rendered = node_tree_of(*kind, &Props::default());
        let has_subnodes = rendered.lines().count() > 1;
        assert_eq!(
            has_subnodes,
            !BARE.contains(kind),
            "{kind:?} rendered:\n{rendered}"
        );
    }
}
