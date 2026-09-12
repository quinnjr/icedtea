//! M6 accesskit acceptance: roles, names and states through the adapter,
//! plus focus/disabled/checked transitions keeping stable node ids.
//!
//! Requires the `a11y` cargo feature (on by default).

#![cfg(feature = "a11y")]

use std::rc::Rc;

use accesskit::{Role, TreeUpdate};
use icedtea_ui::a11y::A11yTree;
use icedtea_ui::anim::{Clock, ManualClock};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::ResolveEnv;
use icedtea_ui::css::node::Node;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::reconcile::{Instance, reconcile};
use icedtea_ui::view::{BuildCx, Kind, Prop, PropName, View};

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum Msg {
    Unused,
}

struct Harness {
    sheet: CompiledSheet,
    fonts: FontDatabase,
    icons: IconTheme,
    clock: Rc<dyn Clock>,
    env: ResolveEnv,
}

impl Harness {
    fn new() -> Self {
        Harness {
            sheet: CompiledSheet::compile("window { color: #000; }"),
            fonts: FontDatabase::probe_only(),
            icons: IconTheme::with_name_and_roots("hicolor", vec![]),
            clock: Rc::new(ManualClock::new()),
            env: ResolveEnv::default(),
        }
    }

    fn cx(&mut self) -> BuildCx<'_> {
        BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }
}

fn button(label: &str) -> View<Msg> {
    View::new(Kind::Button)
        .key("button")
        .prop(PropName::Label, Prop::Str(Rc::from(label)))
}

fn label(text: &str) -> View<Msg> {
    View::new(Kind::Label)
        .key("label")
        .prop(PropName::Label, Prop::Str(Rc::from(text)))
}

fn entry(text: &str) -> View<Msg> {
    View::new(Kind::Entry)
        .key("entry")
        .prop(PropName::Text, Prop::Str(Rc::from(text)))
}

fn check(label: &str) -> View<Msg> {
    View::new(Kind::CheckButton)
        .key("check")
        .prop(PropName::Label, Prop::Str(Rc::from(label)))
}

fn switch() -> View<Msg> {
    View::new(Kind::Switch)
        .key("switch")
        .prop(PropName::Active, Prop::Bool(false))
}

fn list() -> View<Msg> {
    let row = |id: u64, text: &str| {
        View::new(Kind::ListBoxRow)
            .key(id)
            .child(View::new(Kind::Label).prop(PropName::Label, Prop::Str(Rc::from(text))))
    };
    View::new(Kind::ListBox)
        .key("list")
        .child(row(1, "Alpha"))
        .child(row(2, "Beta"))
}

fn build_all(hx: &mut Harness, views: Vec<View<Msg>>) -> (Node, Vec<Instance<Msg>>) {
    let root = Node::new("window");
    let mut instances = Vec::new();
    reconcile(&root, &mut instances, views, &mut hx.cx());
    (root, instances)
}

fn node_with_label(update: &TreeUpdate, label: &str) -> (accesskit::NodeId, accesskit::Node) {
    update
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(label) || n.value() == Some(label))
        .map(|(id, n)| (*id, n.clone()))
        .unwrap_or_else(|| panic!("no a11y node named {label:?}"))
}

#[test]
fn button_label_entry_list_expose_role_name_state() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(
        &mut hx,
        vec![
            button("Save"),
            label("Hello"),
            entry("abc"),
            check("Agree"),
            switch(),
            list(),
        ],
    );

    let mut tree = A11yTree::new();
    let update = tree.build_full(&instances, None);

    let (_, save) = node_with_label(&update, "Save");
    assert_eq!(save.role(), Role::Button);
    assert!(!save.is_disabled());

    let (_, hello) = node_with_label(&update, "Hello");
    assert_eq!(hello.role(), Role::Label);

    let (_, text) = node_with_label(&update, "abc");
    assert_eq!(text.role(), Role::TextInput);
    assert!(!text.is_disabled());

    let (_, agree) = node_with_label(&update, "Agree");
    assert_eq!(agree.role(), Role::CheckBox);

    let list_node = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::ListBox)
        .map(|(_, n)| n.clone())
        .expect("a ListBox node");
    let _ = list_node;
    let (_, alpha) = node_with_label(&update, "Alpha");
    assert_eq!(alpha.role(), Role::ListBoxOption);
}

#[test]
fn focus_disabled_checked_transitions_update_the_tree() {
    let mut hx = Harness::new();
    let (_root, mut instances) = build_all(
        &mut hx,
        vec![button("Save"), entry("abc"), check("Agree"), switch()],
    );

    let mut tree = A11yTree::new();
    let first = tree.build_full(&instances, None);
    let (save_id, _) = node_with_label(&first, "Save");
    let (text_id, _) = node_with_label(&first, "abc");

    // Drive the transitions through the reconciler: disable the entry,
    // check the switch, and move focus to the button.
    let root = Node::new("window");
    let _ = &root;
    let mut disabled_entry = entry("abc");
    disabled_entry
        .props
        .set(PropName::Sensitive, Prop::Bool(false));
    let mut checked_switch = switch();
    checked_switch.props.set(PropName::Active, Prop::Bool(true));
    reconcile(
        &_root,
        &mut instances,
        vec![
            button("Save"),
            disabled_entry,
            check("Agree"),
            checked_switch,
        ],
        &mut hx.cx(),
    );
    let focus_node = instances[0].node.clone();
    let second = tree.update(&instances, Some(&focus_node));

    // Same ids: the instances survived the reconcile.
    let (save_id2, _) = node_with_label(&second, "Save");
    let (text_id2, _) = node_with_label(&second, "abc");
    assert_eq!(save_id, save_id2, "button keeps its node id");
    assert_eq!(text_id, text_id2, "entry keeps its node id");

    assert_eq!(second.focus, save_id, "focus follows the ring");

    let entry_node = second
        .nodes
        .iter()
        .find(|(id, _)| *id == text_id2)
        .map(|(_, n)| n)
        .expect("entry still present");
    assert!(entry_node.is_disabled(), "sensitive(false) disables");

    let switch_node = second
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::Switch)
        .map(|(_, n)| n)
        .expect("switch still present");
    assert_eq!(
        switch_node.toggled(),
        Some(accesskit::Toggled::True),
        "active(true) checks the switch"
    );
}

#[test]
fn password_value_is_never_exposed() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(
        &mut hx,
        vec![
            View::new(Kind::PasswordEntry)
                .key("pw")
                .prop(PropName::Text, Prop::Str(Rc::from("s3cret"))),
        ],
    );

    let mut tree = A11yTree::new();
    let update = tree.build_full(&instances, None);
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::PasswordInput)
        .map(|(id, n)| (*id, n.clone()))
        .expect("a PasswordInput node");
    assert!(
        node.value().is_none() || node.value() == Some(""),
        "password text must not leak, got {:?}",
        node.value()
    );
}
