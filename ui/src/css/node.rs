//! The styled-node tree the CSS engine matches, cascades and paints against.
//!
//! GTK widgets are not a DOM, but they are a *tree*: every node has an element
//! name (`window`, `button`, `label`), optional id, style classes, pseudo-class
//! state, a writing direction, a parent and ordered children — and nothing else
//! (no attributes, no namespaces, no shadow roots). [`Node`] models exactly that.
//!
//! M1's `css::select::CssNode` was immutable and had only a parent, which made
//! `:first-child`, `:last-child`, `:only-child`, `:nth-child()`, `+` and `~`
//! unimplementable. This tree is mutable through shared handles instead: a
//! [`Node`] is an `Rc` handle, cloning it yields another handle to the same node,
//! and every mutation bumps a per-tree generation so a restyle pass can tell what
//! changed without diffing.

use std::borrow::Borrow;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::{Rc, Weak};

use cssparser::ToCss;
use precomputed_hash::PrecomputedHash;
use selectors::bloom::BLOOM_HASH_MASK;

/// FNV-1a, 32-bit. Cheap, stable, and adequate for the ancestor-hash
/// filtering `selectors` uses it for.
pub(crate) fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// An interned CSS identifier with a cached hash.
///
/// `selectors` requires `PrecomputedHash` on its `Identifier`, `LocalName`
/// and `NamespaceUrl` types; one newtype covers all of them.
#[derive(Clone, Debug)]
pub struct CssString {
    text: String,
    hash: u32,
}

impl CssString {
    /// Intern `text`, computing its hash once.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            hash: fnv1a(text.as_bytes()),
        }
    }

    /// The underlying text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl PartialEq for CssString {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for CssString {}

impl Default for CssString {
    fn default() -> Self {
        Self::new("")
    }
}

impl From<&str> for CssString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Borrow<str> for CssString {
    fn borrow(&self) -> &str {
        &self.text
    }
}

impl PrecomputedHash for CssString {
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

impl ToCss for CssString {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.text)
    }
}

/// Writing direction, the argument `:dir()` matches against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Left-to-right. GTK's default, and this engine's.
    #[default]
    Ltr,
    /// Right-to-left.
    Rtl,
}

impl Direction {
    /// Parse `ltr`/`rtl`, ASCII-case-insensitively.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("ltr") {
            Some(Self::Ltr)
        } else if text.eq_ignore_ascii_case("rtl") {
            Some(Self::Rtl)
        } else {
            None
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

bitflags::bitflags! {
    /// The pseudo-class state a node carries.
    ///
    /// `FOCUS_WITHIN` and `FOCUS_VISIBLE` are **derived**, never set directly:
    /// see [`PseudoStates::DERIVED`] and [`Node::set_states`].
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct PseudoStates: u16 {
        /// `:hover`
        const HOVER = 1 << 0;
        /// `:active`
        const ACTIVE = 1 << 1;
        /// `:focus`
        const FOCUS = 1 << 2;
        /// `:focus-visible` — derived from `FOCUS`.
        const FOCUS_VISIBLE = 1 << 3;
        /// `:focus-within` — derived from a descendant's `FOCUS`.
        const FOCUS_WITHIN = 1 << 4;
        /// `:checked`
        const CHECKED = 1 << 5;
        /// `:indeterminate`
        const INDETERMINATE = 1 << 6;
        /// `:disabled`
        const DISABLED = 1 << 7;
        /// `:backdrop`
        const BACKDROP = 1 << 8;
        /// `:selected`
        const SELECTED = 1 << 9;
        /// `:drop(active)`
        const DROP_ACTIVE = 1 << 10;
        /// `:link` — GTK never sets it, so it never matches.
        const LINK = 1 << 11;
        /// `:visited` — GTK never sets it, so it never matches.
        const VISITED = 1 << 12;
    }
}

impl PseudoStates {
    /// The flags the tree derives from `FOCUS`. [`Node::set_states`] masks these
    /// out of its argument, and [`Node::states`] adds them back.
    ///
    /// GTK propagates *both* of these up the ancestor chain from the focused
    /// node (contract §3). That is a deliberate divergence from CSS, where
    /// `:focus-visible` belongs to the focused element alone.
    pub const DERIVED: PseudoStates = PseudoStates::FOCUS_WITHIN.union(PseudoStates::FOCUS_VISIBLE);
}

/// The interior-mutable payload behind a [`Node`].
// `own_states`, `focus_count`, `direction` and `parent` are wired up starting
// Task 2 (state/direction accessors, tree mutation); they are declared here
// because they are part of `NodeInner`'s one true shape, not reallocated
// later. `-D warnings`'s `dead_code` lint would otherwise fail this task's
// gate for fields no Task 1 method reads yet.
#[allow(dead_code)]
struct NodeInner {
    /// Element name, interned for hashing and comparison.
    name: CssString,
    /// The same text as a cheap-to-clone handle, for [`Node::name`].
    name_rc: Rc<str>,
    id: RefCell<Option<CssString>>,
    classes: RefCell<Vec<CssString>>,
    /// Flags the caller set. Never contains [`PseudoStates::DERIVED`].
    own_states: Cell<PseudoStates>,
    /// How many nodes in this subtree (self included) carry `FOCUS`.
    focus_count: Cell<u32>,
    /// `None` == inherit the parent's direction.
    direction: Cell<Option<Direction>>,
    parent: RefCell<Weak<NodeInner>>,
    children: RefCell<Vec<Node>>,
    /// Shared by every node of one tree; bumped by every mutation anywhere.
    tree: RefCell<Rc<Cell<u64>>>,
    /// The value `tree` held when this node was last touched.
    self_generation: Cell<u64>,
}

/// A styled node: name, id, classes, pseudo-class state, direction, children.
///
/// `Clone` is another handle to the *same* node, not a copy — mutating through
/// one handle is visible through all of them, which is what makes a widget layer
/// able to hold onto its node while the tree is restructured around it.
#[derive(Clone)]
pub struct Node(Rc<NodeInner>);

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately shallow: `Element` requires `Debug`, and a recursive dump
        // of a real widget tree in an assertion message is unreadable.
        f.debug_struct("Node")
            .field("name", &self.0.name.as_str())
            .field("id", &self.0.id.borrow().as_ref().map(CssString::as_str))
            .field(
                "classes",
                &self
                    .0
                    .classes
                    .borrow()
                    .iter()
                    .map(CssString::as_str)
                    .collect::<Vec<_>>(),
            )
            .field("children", &self.0.children.borrow().len())
            .finish()
    }
}

impl Node {
    /// A detached node with no classes, no id and default state.
    #[must_use]
    pub fn new(name: &str) -> Node {
        Node::with_classes(name, &[])
    }

    /// A detached node with the given style classes, in order.
    #[must_use]
    pub fn with_classes(name: &str, classes: &[&str]) -> Node {
        Node(Rc::new(NodeInner {
            name: CssString::new(name),
            name_rc: Rc::from(name),
            id: RefCell::new(None),
            classes: RefCell::new(classes.iter().map(|c| CssString::new(c)).collect()),
            own_states: Cell::new(PseudoStates::empty()),
            focus_count: Cell::new(0),
            direction: Cell::new(None),
            parent: RefCell::new(Weak::new()),
            children: RefCell::new(Vec::new()),
            tree: RefCell::new(Rc::new(Cell::new(1))),
            self_generation: Cell::new(1),
        }))
    }

    /// This node's element name.
    #[must_use]
    pub fn name(&self) -> Rc<str> {
        Rc::clone(&self.0.name_rc)
    }

    /// This node's id, if one was set.
    #[must_use]
    pub fn id(&self) -> Option<CssString> {
        self.0.id.borrow().clone()
    }

    /// This node's style classes, in insertion order.
    #[must_use]
    pub fn classes(&self) -> Vec<CssString> {
        self.0.classes.borrow().clone()
    }

    /// Set or clear the id.
    pub fn set_id(&self, id: Option<&str>) {
        let next = id.map(CssString::new);
        if *self.0.id.borrow() == next {
            return;
        }
        *self.0.id.borrow_mut() = next;
        self.touch();
    }

    /// Add a style class. `false` if it was already present (nothing changed).
    pub fn add_class(&self, class: &str) -> bool {
        {
            let mut classes = self.0.classes.borrow_mut();
            if classes.iter().any(|c| c.as_str() == class) {
                return false;
            }
            classes.push(CssString::new(class));
        }
        self.touch();
        true
    }

    /// Remove a style class. `false` if it was not present (nothing changed).
    pub fn remove_class(&self, class: &str) -> bool {
        {
            let mut classes = self.0.classes.borrow_mut();
            let Some(position) = classes.iter().position(|c| c.as_str() == class) else {
                return false;
            };
            classes.remove(position);
        }
        self.touch();
        true
    }

    /// Replace the whole class list.
    pub fn set_classes(&self, classes: &[&str]) {
        let next: Vec<CssString> = classes.iter().map(|c| CssString::new(c)).collect();
        if *self.0.classes.borrow() == next {
            return;
        }
        *self.0.classes.borrow_mut() = next;
        self.touch();
    }

    /// This node's pseudo-class state, derived flags included.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        let mut states = self.0.own_states.get();
        if self.0.focus_count.get() > 0 {
            states |= PseudoStates::DERIVED;
        }
        states
    }

    /// Replace this node's own state flags.
    ///
    /// [`PseudoStates::DERIVED`] is masked out of `states`: `:focus-within` and
    /// `:focus-visible` are maintained by the tree from `FOCUS`, never set by a
    /// caller.
    pub fn set_states(&self, states: PseudoStates) {
        let states = states.difference(PseudoStates::DERIVED);
        let previous = self.0.own_states.get();
        if previous == states {
            return;
        }
        self.0.own_states.set(states);
        match (
            previous.contains(PseudoStates::FOCUS),
            states.contains(PseudoStates::FOCUS),
        ) {
            (false, true) => self.add_focus_count(1),
            (true, false) => self.sub_focus_count(1),
            _ => {}
        }
        self.touch();
    }

    /// Turn one state flag on or off, leaving the others alone.
    pub fn set_state(&self, flag: PseudoStates, on: bool) {
        let mut states = self.0.own_states.get();
        states.set(flag.difference(PseudoStates::DERIVED), on);
        self.set_states(states);
    }

    /// The tree-wide generation: any mutation anywhere in this tree bumps it.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.0.tree.borrow().get()
    }

    /// The tree generation at this node's last own mutation. Comparable across
    /// nodes of one tree, and against a watermark taken from [`Node::generation`].
    #[must_use]
    pub fn self_generation(&self) -> u64 {
        self.0.self_generation.get()
    }

    /// Whether two handles refer to the same node.
    #[must_use]
    pub fn ptr_eq(&self, other: &Node) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// The node's payload address — a stable per-node identity for caches.
    ///
    /// Unused until Task 3 (`RuleBuckets`/`MatchCx` cache keys); kept here
    /// because it is part of this task's frozen `Produces` interface.
    #[allow(dead_code)]
    pub(crate) fn addr(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }

    /// Borrow the id without cloning it.
    ///
    /// Unused until Task 2/3's matching code; kept here because it is part
    /// of this task's frozen `Produces` interface.
    #[allow(dead_code)]
    pub(crate) fn borrow_id<R>(&self, f: impl FnOnce(Option<&CssString>) -> R) -> R {
        f(self.0.id.borrow().as_ref())
    }

    /// Borrow the class list without cloning it.
    ///
    /// Unused until Task 2/3's matching code; kept here because it is part
    /// of this task's frozen `Produces` interface.
    #[allow(dead_code)]
    pub(crate) fn borrow_classes<R>(&self, f: impl FnOnce(&[CssString]) -> R) -> R {
        f(&self.0.classes.borrow())
    }

    /// Feed every identity hash (name, id, each class) to `f`, pre-masked for
    /// `selectors`' bloom filter, whose queries mask with `BLOOM_HASH_MASK`.
    #[allow(dead_code)]
    pub(crate) fn for_each_identity_hash(&self, mut f: impl FnMut(u32)) {
        f(self.0.name.precomputed_hash() & BLOOM_HASH_MASK);
        if let Some(id) = self.0.id.borrow().as_ref() {
            f(id.precomputed_hash() & BLOOM_HASH_MASK);
        }
        for class in self.0.classes.borrow().iter() {
            f(class.precomputed_hash() & BLOOM_HASH_MASK);
        }
    }

    /// This node's parent, if it is attached.
    #[must_use]
    pub fn parent(&self) -> Option<Node> {
        self.0.parent.borrow().upgrade().map(Node)
    }

    /// This node's children, in order.
    #[must_use]
    pub fn children(&self) -> Vec<Node> {
        self.0.children.borrow().clone()
    }

    /// The child at `index`, if any.
    #[must_use]
    pub fn child(&self, index: usize) -> Option<Node> {
        self.0.children.borrow().get(index).cloned()
    }

    /// How many children this node has.
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.0.children.borrow().len()
    }

    /// This node's position among its parent's children; `None` when detached.
    #[must_use]
    pub fn index_in_parent(&self) -> Option<usize> {
        let parent = self.parent()?;
        parent
            .0
            .children
            .borrow()
            .iter()
            .position(|child| child.ptr_eq(self))
    }

    /// The topmost ancestor, or this node when it is detached.
    #[must_use]
    pub fn root(&self) -> Node {
        let mut current = self.clone();
        while let Some(parent) = current.parent() {
            current = parent;
        }
        current
    }

    /// This node's ancestors, parent first, excluding self.
    pub fn ancestors(&self) -> impl Iterator<Item = Node> {
        Ancestors {
            current: self.parent(),
        }
    }

    /// This node's descendants in pre-order, excluding self.
    pub fn descendants(&self) -> impl Iterator<Item = Node> {
        let mut stack: Vec<Node> = self.0.children.borrow().clone();
        stack.reverse();
        Descendants { stack }
    }

    /// Whether `other` is inside this node's subtree (strictly below it).
    #[must_use]
    pub fn is_ancestor_of(&self, other: &Node) -> bool {
        other.ancestors().any(|ancestor| ancestor.ptr_eq(self))
    }

    /// Append `child`, reparenting it if it is attached elsewhere.
    pub fn append_child(&self, child: &Node) {
        self.insert_child(self.child_count(), child);
    }

    /// Insert `child` at `index` (clamped to the end), reparenting if needed.
    ///
    /// Inserting a node into itself or into its own descendant would build an
    /// `Rc` cycle — every tree walk would then never terminate — so it is
    /// refused and logged rather than honoured.
    pub fn insert_child(&self, index: usize, child: &Node) {
        if self.ptr_eq(child) || child.is_ancestor_of(self) {
            tracing::debug!(
                parent = self.0.name.as_str(),
                child = child.0.name.as_str(),
                "refusing to insert a node into itself or into its own descendant"
            );
            return;
        }
        child.detach();

        {
            let mut children = self.0.children.borrow_mut();
            let index = index.min(children.len());
            children.insert(index, child.clone());
        }
        *child.0.parent.borrow_mut() = Rc::downgrade(&self.0);

        let token = self.0.tree.borrow().clone();
        adopt_tree(child, &token);

        let focused = child.0.focus_count.get();
        if focused > 0 {
            self.add_focus_count(focused);
        }
        self.touch();
        child.touch();
    }

    /// Remove `child`. `false` if it was not a child of this node.
    pub fn remove_child(&self, child: &Node) -> bool {
        let position = self
            .0
            .children
            .borrow()
            .iter()
            .position(|candidate| candidate.ptr_eq(child));
        let Some(position) = position else {
            return false;
        };
        self.0.children.borrow_mut().remove(position);
        *child.0.parent.borrow_mut() = Weak::new();

        let focused = child.0.focus_count.get();
        if focused > 0 {
            self.sub_focus_count(focused);
        }

        // The removed subtree becomes a tree of its own, starting past both
        // trees' counters so no handle ever sees a generation go backwards.
        let token = Rc::new(Cell::new(self.0.tree.borrow().get() + 1));
        adopt_tree(child, &token);

        self.touch();
        child.touch();
        true
    }

    /// Remove this node from its parent, if it has one.
    pub fn detach(&self) {
        if let Some(parent) = self.parent() {
            parent.remove_child(self);
        }
    }

    /// Add `delta` focused nodes to this node and every ancestor.
    fn add_focus_count(&self, delta: u32) {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            node.0.focus_count.set(node.0.focus_count.get() + delta);
            node.touch();
            current = node.parent();
        }
    }

    /// Remove `delta` focused nodes from this node and every ancestor.
    fn sub_focus_count(&self, delta: u32) {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            node.0
                .focus_count
                .set(node.0.focus_count.get().saturating_sub(delta));
            node.touch();
            current = node.parent();
        }
    }

    /// Bump the tree generation and stamp it onto this node.
    fn touch(&self) {
        let generation = {
            let tree = self.0.tree.borrow();
            let next = tree.get() + 1;
            tree.set(next);
            next
        };
        self.0.self_generation.set(generation);
    }
}

/// Iterator over a node's ancestors, parent first.
struct Ancestors {
    current: Option<Node>,
}

impl Iterator for Ancestors {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        let node = self.current.take()?;
        self.current = node.parent();
        Some(node)
    }
}

/// Pre-order iterator over a node's descendants.
struct Descendants {
    stack: Vec<Node>,
}

impl Iterator for Descendants {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        let node = self.stack.pop()?;
        let children = node.0.children.borrow();
        for child in children.iter().rev() {
            self.stack.push(child.clone());
        }
        drop(children);
        Some(node)
    }
}

/// Move `node`'s whole subtree onto `token`, carrying the counter forward so a
/// generation is monotonic for every handle involved.
fn adopt_tree(node: &Node, token: &Rc<Cell<u64>>) {
    let previous = node.0.tree.borrow().get();
    if token.get() < previous {
        token.set(previous);
    }
    *node.0.tree.borrow_mut() = Rc::clone(token);
    for child in node.0.children.borrow().iter() {
        adopt_tree(child, token);
    }
}

#[cfg(test)]
mod tests {
    use super::{CssString, Direction, Node, PseudoStates};

    #[test]
    fn a_new_node_carries_its_name_and_classes() {
        let node = Node::with_classes("button", &["suggested-action", "text-button"]);
        assert_eq!(&*node.name(), "button");
        assert_eq!(
            node.classes()
                .iter()
                .map(CssString::as_str)
                .collect::<Vec<_>>(),
            vec!["suggested-action", "text-button"]
        );
        assert!(node.id().is_none(), "a fresh node has no id");
        let bare = Node::new("window");
        assert_eq!(&*bare.name(), "window");
        assert!(bare.classes().is_empty());
    }

    #[test]
    fn identity_mutation_is_idempotent_and_reports_change() {
        let node = Node::new("button");
        assert!(node.add_class("flat"), "adding a new class reports true");
        assert!(!node.add_class("flat"), "adding it twice reports false");
        assert!(
            node.remove_class("flat"),
            "removing a present class reports true"
        );
        assert!(
            !node.remove_class("flat"),
            "removing an absent class reports false"
        );

        node.set_classes(&["a", "b"]);
        assert_eq!(
            node.classes()
                .iter()
                .map(CssString::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        node.set_id(Some("add-color-button"));
        assert_eq!(
            node.id().as_ref().map(CssString::as_str),
            Some("add-color-button")
        );
        node.set_id(None);
        assert!(node.id().is_none());
    }

    #[test]
    fn every_identity_mutation_bumps_both_generations() {
        let node = Node::new("button");
        let mut generation = node.generation();
        let mut self_generation = node.self_generation();

        for mutate in [
            &(|n: &Node| n.set_id(Some("x"))) as &dyn Fn(&Node),
            &|n: &Node| {
                n.add_class("flat");
            },
            &|n: &Node| {
                n.remove_class("flat");
            },
            &|n: &Node| n.set_classes(&["a"]),
        ] {
            mutate(&node);
            assert!(
                node.generation() > generation,
                "tree generation must advance on every identity mutation"
            );
            assert!(
                node.self_generation() > self_generation,
                "self generation must advance on every identity mutation"
            );
            generation = node.generation();
            self_generation = node.self_generation();
        }

        // A no-op mutation must not advance anything: restyle keys off these.
        node.set_classes(&["a"]);
        assert_eq!(
            node.generation(),
            generation,
            "a no-op set_classes is not a change"
        );
        assert_eq!(node.self_generation(), self_generation);
    }

    #[test]
    fn handles_to_the_same_node_are_equal_and_share_state() {
        let node = Node::new("button");
        let handle = node.clone();
        assert!(node.ptr_eq(&handle));
        assert!(
            !node.ptr_eq(&Node::new("button")),
            "same name, different node"
        );
        handle.add_class("flat");
        assert_eq!(
            node.classes()
                .iter()
                .map(CssString::as_str)
                .collect::<Vec<_>>(),
            vec!["flat"],
            "a clone is another handle to the same node, not a copy"
        );
    }

    #[test]
    fn identity_hashes_cover_name_id_and_every_class() {
        use selectors::bloom::BLOOM_HASH_MASK;

        let node = Node::with_classes("button", &["flat", "circular"]);
        node.set_id(Some("close"));
        let mut hashes = Vec::new();
        node.for_each_identity_hash(|hash| hashes.push(hash));
        hashes.sort_unstable();

        let mut expected = vec![
            super::fnv1a(b"button") & BLOOM_HASH_MASK,
            super::fnv1a(b"close") & BLOOM_HASH_MASK,
            super::fnv1a(b"flat") & BLOOM_HASH_MASK,
            super::fnv1a(b"circular") & BLOOM_HASH_MASK,
        ];
        expected.sort_unstable();
        assert_eq!(hashes, expected);
    }

    #[test]
    fn css_string_and_direction_keep_their_m1_behaviour() {
        let text = CssString::new("suggested-action");
        assert_eq!(text.as_str(), "suggested-action");
        assert_eq!(text, CssString::new("suggested-action"));
        assert_ne!(text, CssString::new("destructive-action"));

        assert_eq!(Direction::parse("ltr"), Some(Direction::Ltr));
        assert_eq!(Direction::parse("RTL"), Some(Direction::Rtl));
        assert_eq!(Direction::parse("sideways"), None);
        assert_eq!(Direction::default(), Direction::Ltr);
        assert_eq!(PseudoStates::default(), PseudoStates::empty());
    }

    fn names(nodes: &[Node]) -> Vec<String> {
        nodes.iter().map(|n| n.name().to_string()).collect()
    }

    #[test]
    fn children_are_ordered_and_reachable_from_both_ends() {
        let box_node = Node::new("box");
        let first = Node::new("label");
        let second = Node::new("button");
        let third = Node::new("image");
        box_node.append_child(&first);
        box_node.append_child(&second);
        box_node.append_child(&third);

        assert_eq!(box_node.child_count(), 3);
        assert_eq!(
            names(&box_node.children()),
            vec!["label", "button", "image"]
        );
        assert!(box_node.child(1).expect("second child").ptr_eq(&second));
        assert!(box_node.child(3).is_none());
        assert_eq!(second.index_in_parent(), Some(1));
        assert!(second.parent().expect("parent").ptr_eq(&box_node));
        assert!(box_node.parent().is_none());
        assert_eq!(box_node.index_in_parent(), None);
    }

    #[test]
    fn insert_child_places_the_node_at_the_index() {
        let box_node = Node::new("box");
        let a = Node::new("a");
        let b = Node::new("b");
        let c = Node::new("c");
        box_node.append_child(&a);
        box_node.append_child(&c);
        box_node.insert_child(1, &b);
        assert_eq!(names(&box_node.children()), vec!["a", "b", "c"]);

        // An out-of-range index appends rather than panicking: the widget layer
        // must never be able to crash the engine with a stale index.
        let d = Node::new("d");
        box_node.insert_child(99, &d);
        assert_eq!(names(&box_node.children()), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn appending_an_attached_child_reparents_it() {
        let left = Node::new("box");
        let right = Node::new("box");
        let child = Node::new("button");
        left.append_child(&child);
        assert_eq!(left.child_count(), 1);

        right.append_child(&child);
        assert_eq!(left.child_count(), 0, "the old parent must lose the child");
        assert_eq!(right.child_count(), 1);
        assert!(child.parent().expect("parent").ptr_eq(&right));
    }

    #[test]
    fn remove_and_detach_report_and_orphan() {
        let box_node = Node::new("box");
        let child = Node::new("button");
        let stranger = Node::new("label");
        box_node.append_child(&child);

        assert!(
            !box_node.remove_child(&stranger),
            "removing a non-child reports false"
        );
        assert!(box_node.remove_child(&child));
        assert!(child.parent().is_none());
        assert_eq!(box_node.child_count(), 0);

        box_node.append_child(&child);
        child.detach();
        assert!(child.parent().is_none());
        assert_eq!(box_node.child_count(), 0);
        child.detach(); // detaching an orphan is a no-op, not a panic
    }

    #[test]
    fn ancestors_and_descendants_walk_the_whole_tree() {
        let window = Node::new("window");
        let headerbar = Node::new("headerbar");
        let box_node = Node::new("box");
        let button = Node::new("button");
        let label = Node::new("label");
        window.append_child(&headerbar);
        headerbar.append_child(&box_node);
        box_node.append_child(&button);
        button.append_child(&label);

        assert_eq!(
            names(&label.ancestors().collect::<Vec<_>>()),
            vec!["button", "box", "headerbar", "window"],
            "ancestors are parent-first and exclude self"
        );
        assert_eq!(
            names(&window.descendants().collect::<Vec<_>>()),
            vec!["headerbar", "box", "button", "label"],
            "descendants are pre-order and exclude self"
        );
        assert!(label.root().ptr_eq(&window));
        assert!(window.root().ptr_eq(&window));
        assert!(window.is_ancestor_of(&label));
        assert!(!label.is_ancestor_of(&window));
        assert!(
            !window.is_ancestor_of(&window),
            "a node is not its own ancestor"
        );
    }

    #[test]
    fn a_node_cannot_be_inserted_into_itself_or_its_own_descendant() {
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);

        button.append_child(&window); // would build an Rc cycle and hang every walk
        assert_eq!(button.child_count(), 0, "the cycle must be refused");
        assert!(window.child(0).expect("button").ptr_eq(&button));

        window.append_child(&window);
        assert_eq!(
            window.child_count(),
            1,
            "a node may not become its own child"
        );
    }

    #[test]
    fn structural_mutation_bumps_the_generation_for_every_handle() {
        let window = Node::new("window");
        let button = Node::new("button");
        let before = window.generation();
        window.append_child(&button);
        assert!(window.generation() > before);
        assert_eq!(
            button.generation(),
            window.generation(),
            "an attached node shares its tree's generation"
        );
        assert!(button.self_generation() > 0);

        let attached = window.generation();
        window.remove_child(&button);
        assert!(window.generation() > attached);
        assert!(
            button.generation() > attached,
            "a detached subtree keeps counting forward, never backwards"
        );
    }

    #[test]
    fn adopting_a_subtree_never_moves_a_generation_backwards() {
        let busy = Node::new("window");
        for index in 0..10 {
            busy.add_class(&format!("c{index}"));
        }
        let quiet = Node::new("box");
        let busy_generation = busy.generation();
        assert!(busy_generation > quiet.generation());

        quiet.append_child(&busy);
        assert!(
            busy.generation() > busy_generation,
            "the adopting tree's counter must jump past the adoptee's"
        );
        assert_eq!(quiet.generation(), busy.generation());
    }

    #[test]
    fn own_states_round_trip_and_derived_flags_are_masked_out() {
        let node = Node::new("button");
        node.set_states(PseudoStates::HOVER | PseudoStates::CHECKED);
        assert_eq!(node.states(), PseudoStates::HOVER | PseudoStates::CHECKED);

        node.set_states(PseudoStates::FOCUS_WITHIN | PseudoStates::FOCUS_VISIBLE);
        assert_eq!(
            node.states(),
            PseudoStates::empty(),
            "derived flags may not be set directly"
        );

        node.set_state(PseudoStates::DISABLED, true);
        assert!(node.states().contains(PseudoStates::DISABLED));
        node.set_state(PseudoStates::DISABLED, false);
        assert!(!node.states().contains(PseudoStates::DISABLED));
    }

    #[test]
    fn focus_derives_focus_within_and_focus_visible_up_the_chain() {
        let window = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        let sibling = Node::new("button");
        window.append_child(&box_node);
        box_node.append_child(&entry);
        window.append_child(&sibling);

        entry.set_state(PseudoStates::FOCUS, true);
        assert!(entry.states().contains(PseudoStates::FOCUS));
        for ancestor in [&entry, &box_node, &window] {
            assert!(
                ancestor.states().contains(PseudoStates::FOCUS_WITHIN),
                "focus-within must reach {:?}",
                ancestor.name()
            );
            assert!(
                ancestor.states().contains(PseudoStates::FOCUS_VISIBLE),
                "contract 3: focus-visible propagates with focus-within"
            );
        }
        assert!(
            !sibling.states().contains(PseudoStates::FOCUS_WITHIN),
            "a sibling of the focused node is not focus-within"
        );
        assert!(
            !window.states().contains(PseudoStates::FOCUS),
            "focus itself does not propagate"
        );

        entry.set_state(PseudoStates::FOCUS, false);
        for ancestor in [&entry, &box_node, &window] {
            assert!(!ancestor.states().contains(PseudoStates::FOCUS_WITHIN));
            assert!(!ancestor.states().contains(PseudoStates::FOCUS_VISIBLE));
        }
    }

    #[test]
    fn focus_within_follows_a_subtree_across_reparenting() {
        let left = Node::new("window");
        let right = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        left.append_child(&box_node);
        box_node.append_child(&entry);
        entry.set_state(PseudoStates::FOCUS, true);
        assert!(left.states().contains(PseudoStates::FOCUS_WITHIN));

        right.append_child(&box_node);
        assert!(
            !left.states().contains(PseudoStates::FOCUS_WITHIN),
            "the old ancestor chain must lose focus-within"
        );
        assert!(
            right.states().contains(PseudoStates::FOCUS_WITHIN),
            "the new ancestor chain must gain it"
        );

        box_node.detach();
        assert!(!right.states().contains(PseudoStates::FOCUS_WITHIN));
        assert!(box_node.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn two_focused_descendants_do_not_cancel_each_other() {
        // Nothing forbids the widget layer from focusing two nodes in different
        // branches; a boolean would clear the ancestor flag when the first one
        // blurs, a count does not.
        let window = Node::new("window");
        let a = Node::new("entry");
        let b = Node::new("entry");
        window.append_child(&a);
        window.append_child(&b);
        a.set_state(PseudoStates::FOCUS, true);
        b.set_state(PseudoStates::FOCUS, true);
        a.set_state(PseudoStates::FOCUS, false);
        assert!(
            window.states().contains(PseudoStates::FOCUS_WITHIN),
            "b is still focused"
        );
        b.set_state(PseudoStates::FOCUS, false);
        assert!(!window.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn a_state_change_bumps_the_ancestors_self_generation() {
        let window = Node::new("window");
        let entry = Node::new("entry");
        window.append_child(&entry);
        let window_self = window.self_generation();
        let tree = window.generation();

        entry.set_state(PseudoStates::FOCUS, true);
        assert!(
            window.self_generation() > window_self,
            "a restyle pass keys off self_generation to find the ancestors it must redo"
        );
        assert!(window.generation() > tree);

        let unchanged = window.generation();
        entry.set_state(PseudoStates::FOCUS, true);
        assert_eq!(
            unchanged,
            window.generation(),
            "setting a flag twice is not a change"
        );
    }
}
