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
}
