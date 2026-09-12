//! In-process accessibility tree: the retained [`Instance`] tree as an
//! `accesskit::TreeUpdate`.
//!
//! This is M6's first accesskit slice (see
//! `docs/superpowers/specs/2026-09-12-m6-accesskit-design.md`): it builds the
//! node mapping — role, name, state — with stable [`NodeId`]s across
//! reconciles, but wires no AT-SPI bus. The bus adapter (a follow-up) consumes
//! [`A11yTree::update`] output unchanged, so this slice needs no re-design,
//! only a sink.
//!
//! The adapter is read-only over the toolkit: it walks the [`Instance`] tree
//! (never the CSS subnodes a controller owns), reads names from props and
//! states from the retained [`Node`], and changes no rendering or input
//! behavior. [`NodeId`]: accesskit::NodeId

use accesskit::{Action, Node as A11yNode, NodeId, Role, Toggled, TreeId, TreeInfo, TreeUpdate};

use crate::css::node::{Node, PseudoStates};
use crate::view::{Instance, Kind, Prop, PropName};
use crate::widgets::types::SelectionMode;
use crate::window::focus::FOCUSABLE_CLASS;

/// The synthetic root's id. Real widgets allocate from 1.
const ROOT_ID: NodeId = NodeId(0);

/// The in-process a11y tree: stable id assignment plus the last update.
pub struct A11yTree {
    next: u64,
    /// Every instance node seen so far, so a reconcile that keeps an
    /// `Instance` keeps its [`NodeId`].
    ids: Vec<(Node, NodeId)>,
    last: Option<TreeUpdate>,
}

impl Default for A11yTree {
    fn default() -> Self {
        Self::new()
    }
}

impl A11yTree {
    /// An empty tree. The synthetic root takes [`ROOT_ID`].
    #[must_use]
    pub fn new() -> Self {
        A11yTree {
            next: 1,
            ids: Vec::new(),
            last: None,
        }
    }

    /// A full rebuild from the retained top-level instances.
    ///
    /// `focus` is the window focus ring's current node, if any; unknown when
    /// nothing is focused, in which case focus rests on the root (what
    /// accesskit requires every update to carry).
    pub fn build_full<Msg>(
        &mut self,
        instances: &[Instance<Msg>],
        focus: Option<&Node>,
    ) -> TreeUpdate {
        let mut out = Vec::new();
        let mut children = Vec::with_capacity(instances.len());
        for instance in instances {
            children.push(self.describe(instance, None, &mut out));
        }
        let mut root = A11yNode::new(Role::Window);
        root.set_children(children);
        out.push((ROOT_ID, root));

        let focus_id = focus.map(|node| self.id_for(node)).unwrap_or(ROOT_ID);
        let update = TreeUpdate {
            nodes: out,
            tree: Some(TreeInfo::new(ROOT_ID)),
            tree_id: TreeId::ROOT,
            focus: focus_id,
        };
        self.last = Some(update.clone());
        update
    }

    /// Rebuild after a reconcile. Identical to [`A11yTree::build_full`] in
    /// this slice — ids are stable by construction, which is what the
    /// transition test asserts; true diffing is bus-slice work.
    pub fn update<Msg>(&mut self, instances: &[Instance<Msg>], focus: Option<&Node>) -> TreeUpdate {
        self.build_full(instances, focus)
    }

    /// The last update this tree produced, if any.
    #[must_use]
    pub fn last_update(&self) -> Option<&TreeUpdate> {
        self.last.as_ref()
    }

    /// The node built for `id` in the last update, if any.
    #[must_use]
    pub fn node_for(&self, id: NodeId) -> Option<&A11yNode> {
        self.last.as_ref()?.nodes.iter().find_map(
            |(nid, node)| {
                if *nid == id { Some(node) } else { None }
            },
        )
    }

    /// The stable id for a retained node, allocating on first sight.
    fn id_for(&mut self, node: &Node) -> NodeId {
        if let Some((_, id)) = self.ids.iter().find(|(known, _)| known.ptr_eq(node)) {
            return *id;
        }
        let id = NodeId(self.next);
        self.next += 1;
        self.ids.push((node.clone(), id));
        id
    }

    /// Describe one instance (and, unless it synthesizes its children, its
    /// subtree), pushing into `out` in pre-order so a name lookup finds the
    /// parent before any same-named descendant.
    ///
    /// `option_at` is `(position, set size)` when this instance is a
    /// `ListBox` option among siblings.
    fn describe<Msg>(
        &mut self,
        instance: &Instance<Msg>,
        option_at: Option<(usize, usize)>,
        out: &mut Vec<(NodeId, A11yNode)>,
    ) -> NodeId {
        let id = self.id_for(&instance.node);
        // Reserve the parent's slot first: child ids are needed for
        // `set_children`, but the parent must still land before them.
        let slot = out.len();
        let mut children = Vec::with_capacity(instance.children.len());
        if instance.kind != Kind::DropDown {
            // `DropDown` takes no application children and its option rows
            // are controller-owned CSS subnodes, not `Instance`s; exposing
            // them is bus-slice work (spec §Mapping). Everything else
            // recurses over the retained child instances.
            let mut option_index = 0;
            let option_total = instance
                .children
                .iter()
                .filter(|child| child.kind == Kind::ListBoxRow)
                .count();
            for child in &instance.children {
                let at = (child.kind == Kind::ListBoxRow).then(|| {
                    let at = (option_index, option_total);
                    option_index += 1;
                    at
                });
                children.push(self.describe(child, at, out));
            }
        }
        let mut node = A11yNode::new(role_of(instance.kind));
        fill(instance, &mut node, option_at);
        apply_common(instance, &mut node);
        if !children.is_empty() {
            node.set_children(children);
        }
        out.insert(slot, (id, node));
        id
    }
}

/// The accesskit role for a widget kind. Everything outside the M6 slice's
/// mapping table is a `GenericContainer`, which assistive technologies
/// filter out of the platform tree.
fn role_of(kind: Kind) -> Role {
    match kind {
        Kind::Button | Kind::ToggleButton | Kind::MenuButton | Kind::LinkButton => Role::Button,
        Kind::Label => Role::Label,
        Kind::Entry => Role::TextInput,
        Kind::SearchEntry => Role::SearchInput,
        Kind::PasswordEntry => Role::PasswordInput,
        Kind::CheckButton => Role::CheckBox,
        Kind::Switch => Role::Switch,
        Kind::DropDown => Role::ComboBox,
        Kind::ListBox => Role::ListBox,
        Kind::ListBoxRow => Role::ListBoxOption,
        Kind::SpinButton => Role::SpinButton,
        Kind::Scale => Role::Slider,
        Kind::ProgressBar => Role::ProgressIndicator,
        Kind::LevelBar => Role::Meter,
        Kind::AlertDialog => Role::AlertDialog,
        Kind::Window | Kind::ShortcutsWindow | Kind::AboutDialog => Role::Window,
        _ => Role::GenericContainer,
    }
}

/// Per-kind name/state payload. Common flags (disabled, hidden, focusable,
/// tooltip) live in [`apply_common`].
fn fill<Msg>(instance: &Instance<Msg>, node: &mut A11yNode, option_at: Option<(usize, usize)>) {
    let props = &instance.props;
    match instance.kind {
        Kind::Button | Kind::ToggleButton | Kind::MenuButton => {
            if let Some(name) = label_text(instance) {
                node.set_label(name);
            }
            node.add_action(Action::Click);
        }
        Kind::LinkButton => {
            if let Some(name) = label_text(instance) {
                node.set_label(name);
            }
            if let Some(uri) = props.str(PropName::Uri) {
                node.set_role(Role::Link);
                node.set_url(uri);
            }
            node.add_action(Action::Click);
        }
        // accesskit carries a `Label`'s text in `value`, never in `label`.
        Kind::Label => {
            if let Some(text) = props.str(PropName::Label) {
                node.set_value(text);
            }
        }
        Kind::Entry | Kind::SearchEntry => {
            if let Some(text) = props.str(PropName::Text) {
                node.set_value(text);
            }
            if let Some(hint) = props.str(PropName::Placeholder) {
                node.set_placeholder(hint);
            }
            if matches!(props.get(PropName::Editable), Some(Prop::Bool(false))) {
                node.set_read_only();
            }
        }
        // A password's text must never reach the tree, masked or not.
        Kind::PasswordEntry => {
            if matches!(props.get(PropName::Editable), Some(Prop::Bool(false))) {
                node.set_read_only();
            }
        }
        Kind::CheckButton => {
            if let Some(name) = label_text(instance) {
                node.set_label(name);
            }
            node.set_toggled(toggled_of(&instance.node));
            node.add_action(Action::Click);
        }
        Kind::Switch => {
            if let Some(name) = label_text(instance) {
                node.set_label(name);
            }
            node.set_toggled(toggled_of(&instance.node));
            node.add_action(Action::Click);
        }
        Kind::DropDown => {
            if let Some(current) = drop_down_value(props) {
                node.set_value(current);
            }
            node.add_action(Action::Click);
        }
        Kind::ListBox => {
            if SelectionMode::from_u16(prop_u16(props, PropName::SelectionMode, 1))
                == SelectionMode::Multiple
            {
                node.set_multiselectable();
            }
        }
        Kind::ListBoxRow => {
            if let Some(name) = label_text(instance) {
                node.set_label(name);
            }
            node.set_selected(instance.node.states().contains(PseudoStates::SELECTED));
            if let Some((position, total)) = option_at {
                node.set_position_in_set(position);
                node.set_size_of_set(total);
            }
            node.add_action(Action::Click);
        }
        Kind::SpinButton | Kind::Scale => {
            set_numeric(props, node);
        }
        Kind::ProgressBar => {
            if let Some(fraction) = props
                .get(PropName::Fraction)
                .and_then(as_f64)
                .or_else(|| props.get(PropName::Value).and_then(as_f64))
            {
                node.set_numeric_value(fraction);
            }
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
        }
        Kind::LevelBar => {
            set_numeric(props, node);
        }
        _ => {}
    }
}

/// Disabled, hidden, focusable, tooltip — the flags every node shares.
fn apply_common<Msg>(instance: &Instance<Msg>, node: &mut A11yNode) {
    if instance.node.states().contains(PseudoStates::DISABLED) {
        node.set_disabled();
    }
    if !instance.props.bool(PropName::Visible, true) {
        node.set_hidden();
    }
    if instance
        .node
        .classes()
        .iter()
        .any(|class| class.as_str() == FOCUSABLE_CLASS)
    {
        node.add_action(Action::Focus);
    }
    if let Some(tip) = instance.props.str(PropName::Tooltip) {
        node.set_tooltip(tip);
    }
}

/// `:checked`/`:indeterminate` as accesskit sees them.
fn toggled_of(node: &Node) -> Toggled {
    let states = node.states();
    if states.contains(PseudoStates::INDETERMINATE) {
        Toggled::Mixed
    } else if states.contains(PseudoStates::CHECKED) {
        Toggled::True
    } else {
        Toggled::False
    }
}

/// A control's name: its `Label` prop, else the first `Label` descendant's
/// text (a `button_from` child, a row's label).
fn label_text<Msg>(instance: &Instance<Msg>) -> Option<String> {
    if let Some(text) = instance.props.str(PropName::Label) {
        return Some(text.to_owned());
    }
    instance.children.iter().find_map(label_text)
}

/// The selected item's text, from the `Model` + `Selected` props.
fn drop_down_value(props: &crate::view::Props) -> Option<String> {
    let items = match props.get(PropName::Model) {
        Some(Prop::Items(items)) => items,
        _ => return None,
    };
    let selected = props.int(PropName::Selected, 0).max(0) as usize;
    items.get(selected).map(|item| item.text.to_string())
}

/// `Value`/`Lower`/`Upper`/`StepIncrement` onto a range node, when present.
fn set_numeric(props: &crate::view::Props, node: &mut A11yNode) {
    if let Some(value) = props.get(PropName::Value).and_then(as_f64) {
        node.set_numeric_value(value);
    }
    if let Some(min) = props.get(PropName::Lower).and_then(as_f64) {
        node.set_min_numeric_value(min);
    }
    if let Some(max) = props.get(PropName::Upper).and_then(as_f64) {
        node.set_max_numeric_value(max);
    }
    if let Some(step) = props.get(PropName::StepIncrement).and_then(as_f64) {
        node.set_numeric_value_step(step);
    }
}

/// A numeric prop in either of the shapes a builder may hand over.
fn as_f64(prop: &Prop) -> Option<f64> {
    match prop {
        Prop::Float(value) if value.is_finite() => Some(*value),
        #[allow(clippy::cast_precision_loss, reason = "a11y bounds never reach 2^53")]
        Prop::Int(value) => Some(*value as f64),
        _ => None,
    }
}

/// A `u16` enum prop; anything else is `default`.
fn prop_u16(props: &crate::view::Props, name: PropName, default: u16) -> u16 {
    match props.get(name) {
        Some(Prop::Enum(raw)) => *raw,
        Some(Prop::Int(raw)) => u16::try_from(*raw).unwrap_or(default),
        _ => default,
    }
}
