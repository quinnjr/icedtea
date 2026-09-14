//! M6 accesskit acceptance: roles, names and states through the adapter,
//! plus focus/disabled/checked transitions keeping stable node ids.
//!
//! Requires the `a11y` cargo feature (on by default).

#![cfg(feature = "a11y")]

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use accesskit::{HasPopup, Live, Role, TreeUpdate};
use icedtea_ui::a11y::A11yTree;
use icedtea_ui::anim::{Clock, ManualClock};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::ResolveEnv;
use icedtea_ui::css::node::Node;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::layout::{BoxDirection, Container, FixedMeasure, LayoutTree};
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::reconcile::{Instance, reconcile};
use icedtea_ui::view::render::{Animations, StyleMap, layout_tree, restyle_tree};
use icedtea_ui::view::{BuildCx, Kind, ListItem, Prop, PropName, View};
use icedtea_ui::widgets::{MessageType, WidgetEnum};

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

/// Lay a reconciled tree out the way the app loop does, so a test can hand
/// [`A11yTree::build_full_with_layout`] real allocations. Every container is
/// a column box and every leaf measures 40x20 — enough to give each node a
/// distinct, checkable box without pulling in a controller's own measure.
fn lay_out(hx: &mut Harness, root: &Node) -> LayoutTree {
    let mut styles = StyleMap::new();
    let mut anims = Animations::new();
    restyle_tree(
        root,
        &hx.sheet,
        &hx.env,
        &mut styles,
        &mut anims,
        Duration::ZERO,
    );
    let mut containers: HashMap<_, Container> = HashMap::new();
    for addr in styles.keys() {
        containers.insert(
            *addr,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
    }
    let mut tree = LayoutTree::new();
    let mut measure = FixedMeasure(taffy::Size {
        width: 40.0,
        height: 20.0,
    });
    layout_tree(
        root,
        &styles,
        &containers,
        &mut tree,
        &hx.env,
        (Some(400.0), Some(400.0)),
        &mut measure,
    )
    .expect("the a11y fixture lays out");
    tree
}

fn drop_down() -> View<Msg> {
    let items: Rc<[ListItem]> = Rc::from(vec![
        ListItem::new(0, "One"),
        ListItem::new(1, "Two"),
        ListItem::new(2, "Three"),
    ]);
    View::new(Kind::DropDown)
        .key("dd")
        .prop(PropName::Model, Prop::Items(items))
        .prop(PropName::Selected, Prop::Int(1))
}

#[test]
fn drop_down_options_are_exposed_with_expanded_state() {
    let mut hx = Harness::new();
    let mut view = drop_down();
    view.props.set(PropName::Expanded, Prop::Bool(true));
    let (_root, instances) = build_all(&mut hx, vec![view]);

    let mut tree = A11yTree::new();
    let update = tree.build_full(&instances, None);

    let combo = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::ComboBox)
        .map(|(id, n)| (*id, n.clone()))
        .expect("a ComboBox node");
    assert_eq!(
        combo.1.is_expanded(),
        Some(true),
        "Expanded(true) reaches the combo box"
    );
    assert_eq!(combo.1.has_popup(), Some(HasPopup::Listbox));
    let list_id = combo.1.children()[0];
    let list = update
        .nodes
        .iter()
        .find(|(id, _)| *id == list_id)
        .map(|(_, n)| n.clone())
        .expect("the listbox child");
    assert_eq!(list.role(), Role::ListBox);
    assert_eq!(list.children().len(), 3, "one option per model item");

    let option = |update: &TreeUpdate, label: &str| {
        update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListBoxOption && n.label() == Some(label))
            .map(|(id, n)| (*id, n.clone()))
            .unwrap_or_else(|| panic!("no option {label:?}"))
    };
    let one = option(&update, "One");
    assert_eq!(one.0, list.children()[0]);
    assert_eq!(one.1.is_selected(), Some(false));
    assert_eq!(one.1.position_in_set(), Some(0));
    assert_eq!(one.1.size_of_set(), Some(3));
    let two = option(&update, "Two");
    assert_eq!(
        two.1.is_selected(),
        Some(true),
        "selected index 1 marks option Two"
    );

    // A second build keeps the option ids: they key on the model item's own
    // id, not on the (rebuilt) controller row nodes.
    let rebuild = tree.build_full(&instances, None);
    assert_eq!(
        two.0,
        option(&rebuild, "Two").0,
        "option ids survive a rebuild"
    );
}

#[test]
fn collapsed_drop_down_reports_not_expanded() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(&mut hx, vec![drop_down()]);
    let update = A11yTree::new().build_full(&instances, None);
    let combo = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::ComboBox)
        .map(|(_, n)| n.clone())
        .expect("a ComboBox node");
    assert_eq!(combo.is_expanded(), Some(false), "Expanded defaults false");
}

#[test]
fn layout_allocations_become_accesskit_bounds() {
    let mut hx = Harness::new();
    let (root, instances) = build_all(&mut hx, vec![button("Save")]);
    let layout = lay_out(&mut hx, &root);
    let mut tree = A11yTree::new();
    let update = tree.build_full_with_layout(&instances, None, &layout);

    let alloc = layout
        .allocation(&instances[0].node)
        .expect("button allocation")
        .border_box;
    let (_, save) = node_with_label(&update, "Save");
    let bounds = save.bounds().expect("the button carries bounds");
    assert!((bounds.width() - f64::from(alloc.width)).abs() < 0.5);
    assert!((bounds.height() - f64::from(alloc.height)).abs() < 0.5);
    assert!((bounds.x0 - f64::from(alloc.x)).abs() < 0.5);
    assert!((bounds.y0 - f64::from(alloc.y)).abs() < 0.5);

    // The synthetic root carries the union of the top-level boxes.
    let root_node = update
        .nodes
        .iter()
        .find(|(id, _)| *id == accesskit::NodeId(0))
        .expect("the root node");
    assert!(
        root_node.1.bounds().is_some(),
        "root bounds from the layout"
    );
}

#[test]
fn live_regions_come_from_the_widget_model() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(
        &mut hx,
        vec![
            View::new(Kind::Statusbar)
                .key("status")
                .prop(PropName::Text, Prop::Str(Rc::from("Ready"))),
            View::new(Kind::InfoBar)
                .key("info")
                .prop(PropName::Reveal, Prop::Bool(true))
                .prop(PropName::MessageType, MessageType::Error.to_prop()),
            View::new(Kind::AlertDialog)
                .key("alert")
                .prop(PropName::Message, Prop::Str(Rc::from("Discard changes?"))),
        ],
    );
    let update = A11yTree::new().build_full(&instances, None);

    let node = |update: &TreeUpdate, role: Role| -> accesskit::Node {
        update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == role)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| panic!("no node with role {role:?}"))
    };
    let status = node(&update, Role::Status);
    assert_eq!(status.value(), Some("Ready"));
    assert_eq!(status.live(), Some(Live::Polite));
    assert_eq!(
        node(&update, Role::Alert).live(),
        Some(Live::Assertive),
        "an error info bar interrupts"
    );
    assert_eq!(
        node(&update, Role::AlertDialog).live(),
        Some(Live::Assertive)
    );
    assert_eq!(
        node(&update, Role::AlertDialog).label(),
        Some("Discard changes?")
    );
}

#[test]
fn an_empty_statusbar_is_not_a_live_region() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(&mut hx, vec![View::new(Kind::Statusbar).key("status")]);
    let update = A11yTree::new().build_full(&instances, None);
    let status = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::Status)
        .map(|(_, n)| n.clone())
        .expect("a Status node");
    assert_eq!(status.value(), None, "no message to invent");
    assert_eq!(status.live(), None, "nothing to announce");
}

#[test]
fn focus_on_a_gone_node_falls_back_to_the_root() {
    let mut hx = Harness::new();
    let (_root, instances) = build_all(&mut hx, vec![button("Save")]);

    let mut tree = A11yTree::new();
    // A focus ring can outlive the instance it points at: the node below
    // was never reconciled, so it owns no a11y id.
    let gone = Node::new("button");
    let update = tree.build_full(&instances, Some(&gone));
    assert_eq!(
        update.focus,
        accesskit::NodeId(0),
        "a focus no node owns must rest on the root, never dangle"
    );
}
