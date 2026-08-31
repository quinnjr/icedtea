//! Keyed reconciliation of a `View` description tree into retained
//! [`Node`](crate::css::node::Node)s.

use std::rc::Rc;

use crate::anim::Clock;
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ResolveEnv;
use crate::icons::IconTheme;
use crate::text::FontDatabase;
use crate::view::PropName;

/// Everything building or updating a controller needs, threaded down the
/// whole reconcile.
pub struct BuildCx<'a> {
    /// The compiled theme, for `@keyframes` lookup and colour resolution.
    pub sheet: &'a CompiledSheet,
    /// Font matching, loading and shaping.
    pub fonts: &'a mut FontDatabase,
    /// Icon lookup and rendering.
    pub icons: &'a mut IconTheme,
    /// The animation clock; `ManualClock` under test.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
}

impl std::fmt::Debug for BuildCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildCx")
            .field("rules", &self.sheet.rules.len())
            .field("icon_theme", &self.icons.name())
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

/// One reconciliation step, at one level of the tree.
///
/// The list a `reconcile` call returns covers **that level only**: a
/// `Recurse` says the child at `index` was descended into, and the ops the
/// recursion produced are not spliced into the parent's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// A new child was built and inserted at `index`.
    Insert {
        /// Position in the new child list.
        index: usize,
    },
    /// The child that was at `index` was dropped.
    Remove {
        /// Position in the *previous* child list.
        index: usize,
    },
    /// A kept child moved.
    Move {
        /// Position in the previous child list.
        from: usize,
        /// Position in the new child list.
        to: usize,
    },
    /// One property of the child at `index` changed.
    SetProp {
        /// Position in the new child list.
        index: usize,
        /// Which property.
        name: PropName,
    },
    /// The child at `index` took this frame's handler set.
    SetHandlers {
        /// Position in the new child list.
        index: usize,
    },
    /// The child at `index` was reconciled recursively.
    Recurse {
        /// Position in the new child list.
        index: usize,
    },
}

use std::collections::HashMap;

use crate::css::node::Node;
use crate::layout::Container;
use crate::view::controller::{Controller, build_controller};
use crate::view::render::{NodeAddr, container_for, node_addr};
use crate::view::{Handlers, Key, Kind, Prop, Props, View};

/// One retained widget: the node it owns, the props it was last built from,
/// this frame's handlers, its controller, and its children.
pub struct Instance<Msg> {
    /// The retained M2 node.
    pub node: Node,
    /// Which widget.
    pub kind: Kind,
    /// The identity that matched it to this frame's view.
    pub key: Option<Key>,
    /// Last frame's props.
    pub props: Props,
    /// This frame's handlers — replaced wholesale, never diffed.
    pub handlers: Handlers<Msg>,
    /// The behaviour and the state the model must not own.
    pub controller: Box<dyn Controller<Msg>>,
    /// Child instances, in view order (including invisible ones).
    pub children: Vec<Instance<Msg>>,
}

impl<Msg> std::fmt::Debug for Instance<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("props", &self.props)
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

impl<Msg: Clone + 'static> Instance<Msg> {
    /// Whether this instance is attached to its parent node.
    ///
    /// GTK's `visible`: an invisible widget keeps its state but takes no
    /// space, receives no events and paints nothing. Detaching the node is
    /// how that falls out of M2's tree for free.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.props.bool(crate::view::PropName::Visible, true)
    }

    /// The instance owning `node`, searching this instance and its subtree.
    #[must_use]
    pub fn find(&self, node: &Node) -> Option<&Instance<Msg>> {
        if self.node.ptr_eq(node) {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(node))
    }

    /// The mutable form of [`Instance::find`].
    pub fn find_mut(&mut self, node: &Node) -> Option<&mut Instance<Msg>> {
        if self.node.ptr_eq(node) {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(node))
    }
}

/// Build a fresh instance and its whole subtree.
fn build_instance<Msg: Clone + 'static>(view: View<Msg>, cx: &mut BuildCx<'_>) -> Instance<Msg> {
    let node = Node::new(view.kind.css_name());
    let controller = build_controller::<Msg>(view.kind, &node, &view.props, cx);
    let mut instance = Instance {
        node,
        kind: view.kind,
        key: view.key,
        props: view.props,
        handlers: view.handlers,
        controller,
        children: Vec::new(),
    };
    let node = instance.node.clone();
    reconcile(&node, &mut instance.children, view.children, cx);
    instance
}

/// Indices into `values` of a longest strictly-increasing subsequence.
///
/// The classic patience-sorting solution: `tails[len - 1]` holds the index of
/// the smallest possible tail of an increasing run of length `len`, and
/// `prev` threads each element back to its predecessor so the run can be
/// reconstructed. O(n log n), which matters for a `ListView` model with
/// thousands of rows.
pub(crate) fn longest_increasing_subsequence(values: &[usize]) -> Vec<usize> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut tails: Vec<usize> = Vec::with_capacity(values.len());
    let mut back: Vec<usize> = vec![usize::MAX; values.len()];
    for (index, &value) in values.iter().enumerate() {
        let position = tails.partition_point(|&tail| values[tail] < value);
        if position > 0 {
            back[index] = tails[position - 1];
        }
        if position == tails.len() {
            tails.push(index);
        } else {
            tails[position] = index;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cursor = *tails.last().expect("values is non-empty");
    loop {
        out.push(cursor);
        if back[cursor] == usize::MAX {
            break;
        }
        cursor = back[cursor];
    }
    out.reverse();
    out
}

/// Reconcile one level: diff `next` against `prev`, mutating `prev` in place
/// into this frame's instance list, and return the ops applied **at this
/// level** (a `Recurse` stands for the whole child call).
///
/// Matching, in order:
/// * a keyed view matches the previous instance with the same key **and**
///   the same kind — a kind change under one key is a rebuild, because the
///   controller and the node name both differ;
/// * an unkeyed view matches the next unconsumed previous instance of the
///   same kind, positionally;
/// * anything unmatched on the right is built, anything unmatched on the
///   left is dropped (its controller drops with it).
///
/// Identity survives every path but the rebuild: the `Node`, its animation
/// state, its focus, its shaping caches and any attached popup all belong to
/// the `Instance`, which is moved, never recreated.
pub fn reconcile<Msg: Clone + 'static>(
    parent: &Node,
    prev: &mut Vec<Instance<Msg>>,
    next: Vec<View<Msg>>,
    cx: &mut BuildCx<'_>,
) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let mut taken: Vec<Option<Instance<Msg>>> =
        std::mem::take(prev).into_iter().map(Some).collect();

    // 1. Decide, for every new child, which previous instance it reuses.
    let mut matches: Vec<Option<usize>> = Vec::with_capacity(next.len());
    let mut used = vec![false; taken.len()];
    for view in &next {
        let found = match &view.key {
            Some(key) => taken.iter().position(|slot| {
                slot.as_ref()
                    .is_some_and(|inst| inst.key.as_ref() == Some(key) && inst.kind == view.kind)
            }),
            None => taken.iter().enumerate().position(|(index, slot)| {
                !used[index]
                    && slot
                        .as_ref()
                        .is_some_and(|inst| inst.key.is_none() && inst.kind == view.kind)
            }),
        };
        if let Some(index) = found {
            used[index] = true;
        }
        matches.push(found);
    }

    // 2. Anything not reused is gone. Detach first so a rebuild under the
    //    same key cannot leave two nodes claiming one position.
    for (index, slot) in taken.iter_mut().enumerate() {
        if used[index] {
            continue;
        }
        if let Some(instance) = slot.take() {
            instance.node.detach();
            ops.push(Op::Remove { index });
            drop(instance);
        }
    }

    // Which reused children need no `Move`: the longest run whose previous
    // positions are already ascending. Everything outside it moves, and
    // that is the minimum — `Op::Move` is the expensive op, because
    // `Node::insert_child` reparents and dirties a restyle.
    let reused: Vec<usize> = matches.iter().flatten().copied().collect();
    let stable: std::collections::HashSet<usize> = longest_increasing_subsequence(&reused)
        .into_iter()
        .map(|position| reused[position])
        .collect();

    // 3. Build this frame's list.
    let mut built: Vec<Instance<Msg>> = Vec::with_capacity(next.len());
    for (index, view) in next.into_iter().enumerate() {
        match matches[index].and_then(|from| taken[from].take().map(|inst| (from, inst))) {
            Some((from, mut instance)) => {
                let changed = view.props.diff(&instance.props);
                for name in changed {
                    let value = view.props.get(name).cloned().unwrap_or(Prop::None);
                    instance
                        .controller
                        .set_prop(&instance.node, name, &value, cx);
                    ops.push(Op::SetProp { index, name });
                }
                instance.props = view.props;
                instance.handlers = view.handlers;
                ops.push(Op::SetHandlers { index });
                if !stable.contains(&from) {
                    ops.push(Op::Move { from, to: index });
                }
                let node = instance.node.clone();
                if !reconcile(&node, &mut instance.children, view.children, cx).is_empty() {
                    ops.push(Op::Recurse { index });
                }
                built.push(instance);
            }
            None => {
                built.push(build_instance(view, cx));
                ops.push(Op::Insert { index });
            }
        }
    }

    // 4. Re-attach the visible nodes in order, touching only what moved.
    //    Every `insert_child` bumps the tree generation and dirties a
    //    restyle, so the comparison here is what keeps a steady frame from
    //    restyling the whole window.
    let mut position = 0usize;
    for instance in &built {
        if !instance.is_visible() {
            if instance.node.parent().is_some() {
                instance.node.detach();
            }
            continue;
        }
        let in_place = parent
            .child(position)
            .is_some_and(|current| current.ptr_eq(&instance.node));
        if !in_place {
            parent.insert_child(position, &instance.node);
        }
        position += 1;
    }
    while parent.child_count() > position {
        if let Some(extra) = parent.child(position) {
            extra.detach();
        } else {
            break;
        }
    }

    *prev = built;
    ops
}

/// Collect every instance's layout container, for
/// [`layout_tree`](crate::view::render::layout_tree).
pub fn containers_of<Msg: Clone + 'static>(
    instances: &[Instance<Msg>],
    out: &mut HashMap<NodeAddr, Container>,
) {
    for instance in instances {
        let visible_children = instance.children.iter().any(Instance::is_visible);
        out.insert(
            node_addr(&instance.node),
            container_for(instance.kind, &instance.props, visible_children),
        );
        containers_of(&instance.children, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::cascade::CompiledSheet;
    use crate::css::node::Node;
    use crate::view::builders::widget;
    use crate::view::{Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    #[allow(
        dead_code,
        reason = "Hit documents the Msg shape; never constructed by these tests"
    )]
    enum Msg {
        Hit,
    }

    struct Fixture {
        sheet: CompiledSheet,
        fonts: crate::text::FontDatabase,
        icons: crate::icons::IconTheme,
        clock: std::rc::Rc<dyn crate::anim::Clock>,
        env: crate::css::computed::ResolveEnv,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                sheet: CompiledSheet::compile("window { color: #000; } button { color: #111; }"),
                fonts: crate::text::FontDatabase::probe_only(),
                icons: crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]),
                clock: std::rc::Rc::new(crate::anim::ManualClock::new()),
                env: crate::css::computed::ResolveEnv::default(),
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

    fn labelled(kind: Kind, key: &str, label: &str) -> View<Msg> {
        widget::<Msg>(kind).key(key).prop(PropName::Label, label)
    }

    fn names(parent: &Node) -> Vec<String> {
        parent
            .children()
            .iter()
            .map(|c| c.name().to_string())
            .collect()
    }

    #[test]
    fn a_first_reconcile_inserts_every_child_and_attaches_its_node() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B"),
            ],
            &mut f.cx(),
        );
        assert_eq!(ops, vec![Op::Insert { index: 0 }, Op::Insert { index: 1 }]);
        assert_eq!(prev.len(), 2);
        assert_eq!(
            names(&parent),
            vec!["button".to_owned(), "label".to_owned()]
        );
        assert_eq!(prev[0].kind, Kind::Button);
        assert_eq!(
            prev[0].key,
            Some(crate::view::Key::Name(std::rc::Rc::from("a")))
        );
    }

    #[test]
    fn an_unchanged_frame_emits_only_set_handlers() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A")],
            &mut f.cx(),
        );
        let node_before = prev[0].node.clone();
        let generation_before = parent.generation();

        let ops = reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A")],
            &mut f.cx(),
        );
        assert_eq!(ops, vec![Op::SetHandlers { index: 0 }]);
        assert!(
            prev[0].node.ptr_eq(&node_before),
            "identity was not preserved across an unchanged frame"
        );
        assert_eq!(
            parent.generation(),
            generation_before,
            "a steady frame bumped the tree generation"
        );
    }

    #[test]
    fn a_changed_prop_emits_one_set_prop_and_reaches_the_controller() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A")],
            &mut f.cx(),
        );
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "Z")],
            &mut f.cx(),
        );
        assert_eq!(
            ops,
            vec![
                Op::SetProp {
                    index: 0,
                    name: PropName::Label
                },
                Op::SetHandlers { index: 0 },
            ]
        );
        assert_eq!(prev[0].props.str(PropName::Label), Some("Z"));
    }

    #[test]
    fn a_removed_child_is_dropped_and_detached_and_its_controller_goes_with_it() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B"),
            ],
            &mut f.cx(),
        );
        let gone = prev[1].node.clone();
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A")],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Remove { index: 1 }));
        assert_eq!(prev.len(), 1);
        assert_eq!(names(&parent), vec!["button".to_owned()]);
        assert!(
            gone.parent().is_none(),
            "the removed node is still attached"
        );
    }

    #[test]
    fn a_kind_change_under_the_same_key_rebuilds_rather_than_reusing() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A")],
            &mut f.cx(),
        );
        let old = prev[0].node.clone();
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Label, "a", "A")],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Remove { index: 0 }));
        assert!(ops.contains(&Op::Insert { index: 0 }));
        assert!(!prev[0].node.ptr_eq(&old));
        assert_eq!(names(&parent), vec!["label".to_owned()]);
    }

    #[test]
    fn unkeyed_children_match_positionally_within_their_kind() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Label).prop(PropName::Label, "one"),
                widget::<Msg>(Kind::Button).prop(PropName::Label, "two"),
                widget::<Msg>(Kind::Label).prop(PropName::Label, "three"),
            ],
            &mut f.cx(),
        );
        let first_label = prev[0].node.clone();
        let second_label = prev[2].node.clone();

        // The button disappears; the two labels must keep their identity in
        // their own positional order, not slide onto each other.
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Label).prop(PropName::Label, "one"),
                widget::<Msg>(Kind::Label).prop(PropName::Label, "three"),
            ],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Remove { index: 1 }));
        assert!(prev[0].node.ptr_eq(&first_label));
        assert!(prev[1].node.ptr_eq(&second_label));
    }

    #[test]
    fn an_invisible_child_keeps_its_instance_but_leaves_the_node_tree() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B"),
            ],
            &mut f.cx(),
        );
        let hidden = prev[1].node.clone();

        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B").visible(false),
            ],
            &mut f.cx(),
        );
        assert_eq!(prev.len(), 2, "the instance survived");
        assert!(!prev[1].is_visible());
        assert!(
            hidden.parent().is_none(),
            "an invisible child stayed attached"
        );
        assert_eq!(names(&parent), vec!["button".to_owned()]);

        // And it comes back, in its own position.
        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B"),
            ],
            &mut f.cx(),
        );
        assert!(prev[1].node.ptr_eq(&hidden));
        assert_eq!(
            names(&parent),
            vec!["button".to_owned(), "label".to_owned()]
        );
    }

    #[test]
    fn children_are_reconciled_recursively_and_the_op_says_so() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Box)
                    .key("outer")
                    .child(labelled(Kind::Label, "in", "x")),
            ],
            &mut f.cx(),
        );
        let inner = prev[0].children[0].node.clone();

        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Box)
                    .key("outer")
                    .child(labelled(Kind::Label, "in", "y")),
            ],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Recurse { index: 0 }));
        assert!(prev[0].children[0].node.ptr_eq(&inner));
        assert_eq!(prev[0].children[0].props.str(PropName::Label), Some("y"));
    }

    #[test]
    fn find_locates_an_instance_by_its_node_anywhere_in_the_subtree() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Box)
                    .key("outer")
                    .child(labelled(Kind::Label, "in", "x")),
            ],
            &mut f.cx(),
        );
        let inner = prev[0].children[0].node.clone();
        let found = prev[0]
            .find(&inner)
            .expect("the nested instance is findable");
        assert_eq!(found.kind, Kind::Label);
        assert!(prev[0].find(&Node::new("stranger")).is_none());
    }

    #[test]
    fn lis_returns_the_longest_run_and_prefers_the_earlier_one_on_a_tie() {
        assert_eq!(longest_increasing_subsequence(&[]), Vec::<usize>::new());
        assert_eq!(longest_increasing_subsequence(&[5]), vec![0]);
        assert_eq!(
            longest_increasing_subsequence(&[0, 1, 2, 3]),
            vec![0, 1, 2, 3]
        );
        assert_eq!(longest_increasing_subsequence(&[3, 2, 1, 0]), vec![3]);
        // 2, 3, 5 (indices 1, 2, 4) is the longest run in 4 2 3 1 5.
        assert_eq!(
            longest_increasing_subsequence(&[4, 2, 3, 1, 5]),
            vec![1, 2, 4]
        );
    }

    #[test]
    fn moving_one_child_to_the_front_emits_exactly_one_move() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let rows = |order: [&str; 3]| {
            order
                .into_iter()
                .map(|k| labelled(Kind::Label, k, k))
                .collect::<Vec<_>>()
        };
        reconcile(&parent, &mut prev, rows(["a", "b", "c"]), &mut f.cx());
        let a = prev[0].node.clone();
        let b = prev[1].node.clone();
        let c = prev[2].node.clone();

        let ops = reconcile(&parent, &mut prev, rows(["c", "a", "b"]), &mut f.cx());
        let moves: Vec<&Op> = ops
            .iter()
            .filter(|op| matches!(op, Op::Move { .. }))
            .collect();
        assert_eq!(
            moves,
            vec![&Op::Move { from: 2, to: 0 }],
            "a rotation by one must cost one Move, not three: {ops:?}"
        );
        // Identity survived, and the node order followed.
        assert!(prev[0].node.ptr_eq(&c));
        assert!(prev[1].node.ptr_eq(&a));
        assert!(prev[2].node.ptr_eq(&b));
        assert_eq!(
            parent
                .children()
                .iter()
                .map(|n| n.ptr_eq(&c) as u8 * 3 + n.ptr_eq(&a) as u8 + n.ptr_eq(&b) as u8 * 2)
                .collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }

    #[test]
    fn a_reversal_moves_everything_but_one() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let rows = |order: [&str; 4]| {
            order
                .into_iter()
                .map(|k| labelled(Kind::Label, k, k))
                .collect::<Vec<_>>()
        };
        reconcile(&parent, &mut prev, rows(["a", "b", "c", "d"]), &mut f.cx());
        let ops = reconcile(&parent, &mut prev, rows(["d", "c", "b", "a"]), &mut f.cx());
        let moves = ops
            .iter()
            .filter(|op| matches!(op, Op::Move { .. }))
            .count();
        assert_eq!(moves, 3, "a full reversal keeps exactly one child in place");
    }

    #[test]
    fn a_pure_insertion_in_the_middle_moves_nothing() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Label, "a", "a"),
                labelled(Kind::Label, "c", "c"),
            ],
            &mut f.cx(),
        );
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Label, "a", "a"),
                labelled(Kind::Label, "b", "b"),
                labelled(Kind::Label, "c", "c"),
            ],
            &mut f.cx(),
        );
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Move { .. })),
            "an insertion emitted a Move: {ops:?}"
        );
        assert!(ops.contains(&Op::Insert { index: 1 }));
        assert_eq!(prev.len(), 3);
        assert_eq!(names(&parent).len(), 3);
    }
}
