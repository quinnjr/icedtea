//! Selector matching over the node tree, through Servo's `selectors` crate.
//!
//! GTK widgets are not a DOM, but they are a tree: [`crate::css::node::Node`]
//! has an element name, an optional id, style classes, pseudo-class state, a
//! writing direction, a parent and ordered children — and nothing else (no
//! attributes, no namespaces, no shadow roots). `Node` implements
//! [`selectors::Element`] completely over that model, so the upstream matcher
//! drives selection with no fork and no shim: combinators (descendant, `>`,
//! `+`, `~`), the whole `:nth-*` family, `:root`, `:empty`, ids, classes, and
//! GTK's pseudo-class set including `:focus-within`, `:indeterminate` and
//! `:dir()`.
//!
//! [`MatchCx`] carries the state a restyle pass reuses — the nth-index caches
//! and an ancestor bloom filter — and drops it when the tree generation moves.
//! [`RuleBuckets`] narrows a stylesheet to the rules a node could match, which
//! `selectors` itself does not provide.
//!
//! M1's `CssNode` (immutable, parent-only) is still here because the rest of the
//! crate has not been migrated onto `Node` yet; Part 3 of M2 deletes it.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use cssparser::{CowRcStr, Parser as CssParser, ParserInput, SourceLocation, ToCss};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, MatchingContext, matches_selector};
use selectors::parser::{
    AncestorHashes, Component, NonTSPseudoClass, ParseRelative,
    PseudoElement as PseudoElementTrait, Selector, SelectorParseErrorKind,
};
use selectors::{Element, OpaqueElement, SelectorImpl, SelectorList};

use crate::css::cascade::CompiledRule;
use crate::css::node::Node;
// M1's `CssNode` still uses `select::PseudoStates`, the 7-bool struct below.
// Part 3 deletes both and this alias goes with them.
use crate::css::node::PseudoStates as NodeStates;

// `CssString` and `Direction` live with the tree they describe (contract §3);
// they are re-exported here because every M1 caller reaches them through
// `css::select`, and `ui/tests/themed_button_offscreen.rs` may not be edited.
pub use crate::css::node::{CssString, Direction};

/// The pseudo-classes M1 models, plus a catch-all.
///
/// `Other` exists so a rule carrying a pseudo-class this milestone does not
/// model still *parses* -- dropping it would silently discard large parts of
/// a real theme. `Other` never matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GtkPseudoClass {
    /// `:hover`
    Hover,
    /// `:active`
    Active,
    /// `:checked`
    Checked,
    /// `:indeterminate` — a tri-state check button's middle state.
    Indeterminate,
    /// `:disabled`
    Disabled,
    /// `:focus`
    Focus,
    /// `:focus-visible`
    FocusVisible,
    /// `:focus-within` — derived by the node tree from a descendant's focus.
    /// Not built into `selectors` (unlike `:nth-child` and friends), so it is a
    /// custom pseudo-class like `:hover`.
    FocusWithin,
    /// `:backdrop`
    Backdrop,
    /// `:selected`
    Selected,
    /// `:link`. GTK never sets it on a node, so it parses and never matches.
    Link,
    /// `:visited`. GTK never sets it on a node, so it parses and never matches.
    Visited,
    /// `:dir(ltr)` / `:dir(rtl)`, matched against [`CssNode::direction`].
    Dir(Direction),
    /// `:drop(...)`. GTK sets it only mid-drag, which M1 has no notion of,
    /// so it parses and never matches.
    Drop(CssString),
    /// Any pseudo-class M1 does not model. Parses; never matches.
    Other(CssString),
    /// Any *functional* pseudo-class M1 does not model, argument text
    /// included so the selector still serializes. Parses; never matches.
    ///
    /// Crucially this keeps the rest of a comma-separated selector list
    /// alive: `colorswatch:drop(active), colorswatch` must not lose its
    /// second, perfectly valid selector.
    OtherFunctional(CssString, CssString),
}

impl ToCss for GtkPseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_char(':')?;
        match self {
            Self::Hover => dest.write_str("hover"),
            Self::Active => dest.write_str("active"),
            Self::Checked => dest.write_str("checked"),
            Self::Indeterminate => dest.write_str("indeterminate"),
            Self::Disabled => dest.write_str("disabled"),
            Self::Focus => dest.write_str("focus"),
            Self::FocusVisible => dest.write_str("focus-visible"),
            Self::FocusWithin => dest.write_str("focus-within"),
            Self::Backdrop => dest.write_str("backdrop"),
            Self::Selected => dest.write_str("selected"),
            Self::Link => dest.write_str("link"),
            Self::Visited => dest.write_str("visited"),
            Self::Dir(direction) => {
                dest.write_str("dir(")?;
                dest.write_str(direction.as_str())?;
                dest.write_char(')')
            }
            Self::Drop(argument) => {
                dest.write_str("drop(")?;
                dest.write_str(argument.as_str())?;
                dest.write_char(')')
            }
            Self::Other(name) => dest.write_str(name.as_str()),
            Self::OtherFunctional(name, argument) => {
                dest.write_str(name.as_str())?;
                dest.write_char('(')?;
                dest.write_str(argument.as_str())?;
                dest.write_char(')')
            }
        }
    }
}

impl NonTSPseudoClass for GtkPseudoClass {
    type Impl = GtkSelectorImpl;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Hover | Self::Active)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            Self::Hover | Self::Active | Self::Focus | Self::FocusVisible | Self::FocusWithin
        )
    }
}

/// A pseudo-element (`::selection`, ...). Parsed so rules using them are not
/// discarded; never matched, because M1 has no pseudo-element boxes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GtkPseudoElement(pub CssString);

impl ToCss for GtkPseudoElement {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str("::")?;
        dest.write_str(self.0.as_str())
    }
}

impl PseudoElementTrait for GtkPseudoElement {
    type Impl = GtkSelectorImpl;
}

/// The `SelectorImpl` binding GTK's node model to `selectors`.
#[derive(Clone, Debug)]
pub struct GtkSelectorImpl;

impl SelectorImpl for GtkSelectorImpl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssString;
    type LocalName = CssString;
    type NamespaceUrl = CssString;
    type NamespacePrefix = CssString;
    type BorrowedNamespaceUrl = str;
    type BorrowedLocalName = str;
    type NonTSPseudoClass = GtkPseudoClass;
    type PseudoElement = GtkPseudoElement;
}

/// The selector-syntax parser: maps `:name` onto [`GtkPseudoClass`].
pub struct GtkSelectorParser;

impl<'i> selectors::parser::Parser<'i> for GtkSelectorParser {
    type Impl = GtkSelectorImpl;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_non_ts_pseudo_class(
        &self,
        _location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<GtkPseudoClass, cssparser::ParseError<'i, Self::Error>> {
        Ok(match_pseudo_class_name(name.as_ref()))
    }

    /// Functional pseudo-classes: `:dir()` and `:drop()` are modelled;
    /// everything else parses into a never-matching
    /// [`GtkPseudoClass::OtherFunctional`] rather than erroring, so one
    /// unknown selector cannot take its whole comma list with it.
    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: CowRcStr<'i>,
        parser: &mut CssParser<'i, 't>,
        _after_part: bool,
    ) -> Result<GtkPseudoClass, cssparser::ParseError<'i, Self::Error>> {
        let argument = crate::css::tokens::serialize_remaining(parser);
        Ok(if name.eq_ignore_ascii_case("dir") {
            match Direction::parse(&argument) {
                Some(direction) => GtkPseudoClass::Dir(direction),
                // `:dir(sideways)` is well-formed syntax with a bogus
                // argument: it must parse and never match.
                None => GtkPseudoClass::OtherFunctional(
                    CssString::new("dir"),
                    CssString::new(&argument),
                ),
            }
        } else if name.eq_ignore_ascii_case("drop") {
            GtkPseudoClass::Drop(CssString::new(&argument))
        } else {
            GtkPseudoClass::OtherFunctional(
                CssString::new(&name.to_ascii_lowercase()),
                CssString::new(&argument),
            )
        })
    }

    fn parse_pseudo_element(
        &self,
        _location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<GtkPseudoElement, cssparser::ParseError<'i, Self::Error>> {
        Ok(GtkPseudoElement(CssString::new(name.as_ref())))
    }
}

fn match_pseudo_class_name(name: &str) -> GtkPseudoClass {
    match name.to_ascii_lowercase().as_str() {
        "hover" => GtkPseudoClass::Hover,
        "active" => GtkPseudoClass::Active,
        "checked" => GtkPseudoClass::Checked,
        "indeterminate" => GtkPseudoClass::Indeterminate,
        "disabled" => GtkPseudoClass::Disabled,
        "focus" => GtkPseudoClass::Focus,
        "focus-visible" => GtkPseudoClass::FocusVisible,
        "focus-within" => GtkPseudoClass::FocusWithin,
        "backdrop" => GtkPseudoClass::Backdrop,
        "selected" => GtkPseudoClass::Selected,
        "link" => GtkPseudoClass::Link,
        "visited" => GtkPseudoClass::Visited,
        other => GtkPseudoClass::Other(CssString::new(other)),
    }
}

/// The pseudo-class state a node currently carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PseudoStates {
    /// `:hover`
    pub hover: bool,
    /// `:active`
    pub active: bool,
    /// `:checked`
    pub checked: bool,
    /// `:disabled`
    pub disabled: bool,
    /// `:focus` and `:focus-visible`
    pub focus: bool,
    /// `:backdrop`
    pub backdrop: bool,
    /// `:selected`
    pub selected: bool,
}

/// The immutable payload behind a [`CssNode`].
#[derive(Debug)]
pub struct NodeData {
    name: CssString,
    classes: Vec<CssString>,
    states: PseudoStates,
    direction: Direction,
    parent: Option<CssNode>,
}

/// A GTK CSS node: element name, style classes, pseudo-class state, parent.
///
/// Immutable and `Rc`-shared, because `Element::parent_element` must return
/// `Self` by value. Changing state means rebuilding the node (cheap: the
/// ancestors are shared), which is also exactly what a restyle is.
#[derive(Clone, Debug)]
pub struct CssNode(Rc<NodeData>);

impl CssNode {
    /// Build a node with the given identity, state and parent.
    #[must_use]
    pub fn new(
        name: &str,
        classes: &[&str],
        states: PseudoStates,
        parent: Option<CssNode>,
    ) -> Self {
        Self(Rc::new(NodeData {
            name: CssString::new(name),
            classes: classes.iter().map(|c| CssString::new(c)).collect(),
            states,
            direction: Direction::default(),
            parent,
        }))
    }

    /// A copy of this node with a different writing direction.
    #[must_use]
    pub fn with_direction(&self, direction: Direction) -> Self {
        Self(Rc::new(NodeData {
            name: self.0.name.clone(),
            classes: self.0.classes.clone(),
            states: self.0.states,
            direction,
            parent: self.0.parent.clone(),
        }))
    }

    /// This node's writing direction.
    #[must_use]
    pub fn direction(&self) -> Direction {
        self.0.direction
    }

    /// This node's parent, if any.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0.parent.clone()
    }

    /// A copy of this node with different pseudo-class state, sharing the
    /// same name, classes and ancestors.
    #[must_use]
    pub fn with_states(&self, states: PseudoStates) -> Self {
        Self(Rc::new(NodeData {
            name: self.0.name.clone(),
            classes: self.0.classes.clone(),
            states,
            direction: self.0.direction,
            parent: self.0.parent.clone(),
        }))
    }

    /// This node's pseudo-class state.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        self.0.states
    }
}

impl Element for CssNode {
    type Impl = GtkSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // The `Rc`'s payload address: stable for the node's lifetime and
        // unique per node, which is all `OpaqueElement` needs. (Upstream's
        // constructor takes `&T`, not a raw pointer.)
        OpaqueElement::new(&*self.0)
    }

    fn parent_element(&self) -> Option<Self> {
        self.0.parent.clone()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    // M2: constant because M1's tree is a two-node chain (window > button). A
    // real tree makes this wrong in both directions (:first-child/:last-child/
    // :only-child match everything; :not(:first-child) matches nothing). 33
    // Adwaita rules depend on these.
    fn prev_sibling_element(&self) -> Option<Self> {
        None
    }

    // M2: constant because M1's tree is a two-node chain (window > button). A
    // real tree makes this wrong in both directions (:first-child/:last-child/
    // :only-child match everything; :not(:first-child) matches nothing). 33
    // Adwaita rules depend on these.
    fn next_sibling_element(&self) -> Option<Self> {
        None
    }

    // M2: constant because M1's tree is a two-node chain (window > button). A
    // real tree makes this wrong in both directions (:first-child/:last-child/
    // :only-child match everything; :not(:first-child) matches nothing). 33
    // Adwaita rules depend on these.
    fn first_element_child(&self) -> Option<Self> {
        None
    }

    fn is_html_element_in_html_document(&self) -> bool {
        false
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        self.0.name.as_str() == local_name
    }

    fn has_namespace(&self, ns: &str) -> bool {
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.0.name == other.0.name
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&CssString>,
        _local_name: &CssString,
        _operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &GtkPseudoClass,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        let s = self.0.states;
        match pc {
            GtkPseudoClass::Hover => s.hover,
            GtkPseudoClass::Active => s.active,
            GtkPseudoClass::Checked => s.checked,
            GtkPseudoClass::Disabled => s.disabled,
            GtkPseudoClass::Focus | GtkPseudoClass::FocusVisible => s.focus,
            GtkPseudoClass::Backdrop => s.backdrop,
            GtkPseudoClass::Selected => s.selected,
            GtkPseudoClass::Dir(direction) => self.0.direction == *direction,
            // M1's `CssNode` has no bit for these; `Node` matches them properly.
            // This impl disappears with `CssNode` in Part 3.
            GtkPseudoClass::Indeterminate
            | GtkPseudoClass::FocusWithin
            | GtkPseudoClass::Link
            | GtkPseudoClass::Visited
            | GtkPseudoClass::Drop(_)
            | GtkPseudoClass::Other(_)
            | GtkPseudoClass::OtherFunctional(..) => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &GtkPseudoElement,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, _id: &CssString, _case_sensitivity: CaseSensitivity) -> bool {
        false
    }

    fn has_class(&self, name: &CssString, _case_sensitivity: CaseSensitivity) -> bool {
        self.0.classes.iter().any(|c| c == name)
    }

    fn has_custom_state(&self, _name: &CssString) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssString) -> Option<CssString> {
        None
    }

    fn is_part(&self, _name: &CssString) -> bool {
        false
    }

    // M2: constant because M1's tree is a two-node chain (window > button). A
    // real tree makes this wrong in both directions (:first-child/:last-child/
    // :only-child match everything; :not(:first-child) matches nothing). 33
    // Adwaita rules depend on these.
    fn is_empty(&self) -> bool {
        true
    }

    fn is_root(&self) -> bool {
        self.0.parent.is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

/// Parse a comma-separated selector list. `None` if the whole list is invalid.
#[must_use]
pub fn parse_selector_list(text: &str) -> Option<SelectorList<GtkSelectorImpl>> {
    let mut input = ParserInput::new(text);
    let mut parser = CssParser::new(&mut input);
    SelectorList::parse(&GtkSelectorParser, &mut parser, ParseRelative::No).ok()
}

/// The matching state a whole restyle pass shares.
///
/// It carries three things `selectors` wants kept alive across calls:
/// - `SelectorCaches`, whose `nth_index` memoises `:nth-child()` counts so a wide
///   sibling list costs O(n) rather than O(n²) across a pass;
/// - a `BloomFilter` holding the identity hashes of the *ancestors* of the node
///   being matched, which fast-rejects selectors whose ancestor hashes cannot be
///   present;
/// - the tree generation and filter position both were built for, so a stale
///   cache can never answer for a mutated tree.
///
/// [`matches`] and [`matches_with_specificity`] call [`MatchCx::seed_for`]
/// themselves; a caller walking a tree depth-first can instead drive the filter
/// with [`MatchCx::push_ancestor`]/[`MatchCx::pop_ancestor`] and pay for each
/// ancestor once.
pub struct MatchCx {
    caches: SelectorCaches,
    bloom: BloomFilter,
    /// `(tree id, addr of the deepest ancestor in `bloom`, tree generation)`.
    /// `0` is the address of "no ancestors", i.e. the filter is empty and that
    /// is correct. The tree id (`Node::tree_id`) is load-bearing, not
    /// decorative: a heap address and a generation counter both restart from
    /// values a *dropped* tree could also have held, so `(addr, generation)`
    /// alone can alias a stale filter position onto an unrelated new tree
    /// (ABA). The tree id is drawn from a process-wide monotonic counter and
    /// never repeats, so including it rules that out.
    filter_top: Option<(u64, usize, u64)>,
    /// `(tree id, tree generation)` `caches` was built against.
    generation: Option<(u64, u64)>,
    /// How many ancestors are currently inserted.
    depth: usize,
}

impl Default for MatchCx {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchCx {
    /// An empty context, positioned for nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            caches: SelectorCaches::default(),
            bloom: BloomFilter::new(),
            filter_top: None,
            generation: None,
            depth: 0,
        }
    }

    /// Forget every cache and empty the filter.
    pub fn reset(&mut self) {
        self.caches = SelectorCaches::default();
        self.bloom.clear();
        self.filter_top = None;
        self.generation = None;
        self.depth = 0;
    }

    /// Position the filter for `node` by walking its ancestors root-first.
    ///
    /// A no-op when the filter already holds exactly `node`'s ancestors at
    /// `node`'s current tree generation, which is the common case when a whole
    /// stylesheet is matched against one node.
    pub fn seed_for(&mut self, node: &Node) {
        let tree_id = node.tree_id();
        let generation = node.generation();
        if self.generation != Some((tree_id, generation)) {
            // The tree changed: every memoised nth-index count is now suspect.
            self.caches = SelectorCaches::default();
            self.generation = Some((tree_id, generation));
            self.filter_top = None;
        }
        let wanted = node.parent().map_or(0, |parent| parent.addr());
        if self.filter_top == Some((tree_id, wanted, generation)) {
            return;
        }

        self.bloom.clear();
        self.depth = 0;
        let mut chain: Vec<Node> = node.ancestors().collect();
        chain.reverse();
        for ancestor in &chain {
            self.push_ancestor(ancestor);
        }
        self.filter_top = Some((tree_id, wanted, generation));
    }

    /// Insert `node`'s identity hashes, making the filter correct for `node`'s
    /// children.
    pub fn push_ancestor(&mut self, node: &Node) {
        node.for_each_identity_hash(|hash| self.bloom.insert_hash(hash));
        self.depth += 1;
        self.filter_top = Some((node.tree_id(), node.addr(), node.generation()));
    }

    /// Undo one [`MatchCx::push_ancestor`].
    pub fn pop_ancestor(&mut self, node: &Node) {
        debug_assert!(self.depth > 0, "pop_ancestor without a matching push");
        node.for_each_identity_hash(|hash| self.bloom.remove_hash(hash));
        self.depth = self.depth.saturating_sub(1);
        self.filter_top = node
            .parent()
            .map(|parent| (parent.tree_id(), parent.addr(), parent.generation()));
    }

    /// Test-only probe: whether the ancestor filter currently reports it might
    /// contain `hash` (already masked with [`selectors::bloom::BLOOM_HASH_MASK`]).
    /// Bypasses [`MatchCx::seed_for`] entirely, so a test can observe `cx`'s own
    /// filter state rather than one a fresh, hidden `MatchCx` would rebuild.
    #[cfg(test)]
    pub(crate) fn might_contain_hash(&self, hash: u32) -> bool {
        self.bloom.might_contain_hash(hash)
    }
}

/// The complete `selectors::Element` implementation over the node tree.
///
/// Everything `selectors` can ask about a GTK node is answered for real here:
/// siblings (which drive `+`, `~` and the whole `:nth-*` family), the first
/// child, emptiness, root-ness, ids, classes, the pseudo-class state, and the
/// identity hashes the ancestor bloom filter is built from.
///
/// The methods below are constant because GTK's node model genuinely has no
/// such concept — they are not stubs:
/// * `attr_matches`, `has_attr_in_no_namespace` — GTK nodes have no attributes.
/// * `has_custom_state` — no `:state()` custom states.
/// * `imported_part`, `is_part` — no shadow parts.
/// * `is_html_slot_element`, `assigned_slot`, `parent_node_is_shadow_root`,
///   `containing_shadow_host` — no shadow DOM.
/// * `is_html_element_in_html_document` — not HTML.
/// * `is_link` — `:link`/`:visited` are matched from `PseudoStates`, which GTK
///   never sets, so link-ness is meaningless here.
/// * `is_pseudo_element`, `match_pseudo_element`,
///   `pseudo_element_originating_element` — no pseudo-element boxes (M3+).
/// * `apply_selector_flags` — M2 restyles whole dirty subtrees, so there is no
///   invalidation bookkeeping to record.
impl Element for Node {
    type Impl = GtkSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // The payload address: stable for the node's lifetime and unique per
        // node. `OpaqueElement::new` takes `&T`, so the borrow must be of the
        // payload itself -- passing a raw pointer would take the address of a
        // temporary instead. Binding the `Rc` keeps the payload alive for the
        // whole expression.
        let payload = self.opaque_payload();
        OpaqueElement::new(&*payload)
    }

    fn parent_element(&self) -> Option<Self> {
        self.parent()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn pseudo_element_originating_element(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let parent = self.parent()?;
        let index = self.index_in_parent()?;
        if index == 0 {
            return None;
        }
        parent.child(index - 1)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let parent = self.parent()?;
        let index = self.index_in_parent()?;
        parent.child(index + 1)
    }

    fn first_element_child(&self) -> Option<Self> {
        self.child(0)
    }

    fn is_html_element_in_html_document(&self) -> bool {
        false
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        &*self.name() == local_name
    }

    fn has_namespace(&self, ns: &str) -> bool {
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.name() == other.name()
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&CssString>,
        _local_name: &CssString,
        _operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &GtkPseudoClass,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        let states = self.states();
        match pc {
            GtkPseudoClass::Hover => states.contains(NodeStates::HOVER),
            GtkPseudoClass::Active => states.contains(NodeStates::ACTIVE),
            GtkPseudoClass::Checked => states.contains(NodeStates::CHECKED),
            GtkPseudoClass::Indeterminate => states.contains(NodeStates::INDETERMINATE),
            GtkPseudoClass::Disabled => states.contains(NodeStates::DISABLED),
            GtkPseudoClass::Focus => states.contains(NodeStates::FOCUS),
            GtkPseudoClass::FocusVisible => states.contains(NodeStates::FOCUS_VISIBLE),
            GtkPseudoClass::FocusWithin => states.contains(NodeStates::FOCUS_WITHIN),
            GtkPseudoClass::Backdrop => states.contains(NodeStates::BACKDROP),
            GtkPseudoClass::Selected => states.contains(NodeStates::SELECTED),
            // GTK never sets these; the bits exist so the grammar is complete.
            GtkPseudoClass::Link => states.contains(NodeStates::LINK),
            GtkPseudoClass::Visited => states.contains(NodeStates::VISITED),
            GtkPseudoClass::Dir(direction) => self.direction() == *direction,
            GtkPseudoClass::Drop(argument) => {
                argument.as_str().eq_ignore_ascii_case("active")
                    && states.contains(NodeStates::DROP_ACTIVE)
            }
            GtkPseudoClass::Other(_) | GtkPseudoClass::OtherFunctional(..) => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &GtkPseudoElement,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn assigned_slot(&self) -> Option<Self> {
        None
    }

    fn has_id(&self, id: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        self.borrow_id(|own| {
            own.is_some_and(|own| {
                case_sensitivity.eq(own.as_str().as_bytes(), id.as_str().as_bytes())
            })
        })
    }

    fn has_class(&self, name: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        self.borrow_classes(|classes| {
            classes.iter().any(|class| {
                case_sensitivity.eq(class.as_str().as_bytes(), name.as_str().as_bytes())
            })
        })
    }

    fn has_custom_state(&self, _name: &CssString) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssString) -> Option<CssString> {
        None
    }

    fn is_part(&self, _name: &CssString) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.child_count() == 0
    }

    fn is_root(&self) -> bool {
        self.parent().is_none()
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        // Name, id and every class. `selectors` masks its queries with
        // `BLOOM_HASH_MASK` (matching.rs:161), so the hashes are pre-masked in
        // `Node::for_each_identity_hash` and the two sides agree exactly.
        self.for_each_identity_hash(|hash| filter.insert_hash(hash));
        true
    }
}

/// Compiled rules indexed by the rightmost compound selector's id, class or
/// element name, so a node is only offered the rules that can possibly match it.
///
/// `selectors` 0.40 has no bucketing of its own (research/cssparser-selectors.md
/// §7); this is built on `Selector::iter()`, whose first sequence is exactly the
/// rightmost compound.
#[derive(Debug, Default)]
pub struct RuleBuckets {
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
    universal: Vec<usize>,
    len: usize,
}

/// Which bucket one selector belongs in.
enum BucketKey {
    Id(String),
    Class(String),
    Name(String),
    /// The rightmost compound names no id, class or element -- `* > :first-child`,
    /// `:root`, `::selection`. These rules must be offered to every node.
    Universal,
}

/// The bucket for one selector: id, else the first class, else the element name,
/// else universal. Only the rightmost compound is considered -- `Selector::iter()`
/// stops at the first combinator, and `next_sequence()` is deliberately not
/// called.
fn bucket_key(selector: &Selector<GtkSelectorImpl>) -> BucketKey {
    let mut id = None;
    let mut class = None;
    let mut name = None;
    for component in selector.iter() {
        match component {
            Component::ID(value) => id = Some(value.as_str().to_string()),
            Component::Class(value) => {
                if class.is_none() {
                    class = Some(value.as_str().to_string());
                }
            }
            Component::LocalName(local) => name = Some(local.name.as_str().to_string()),
            _ => {}
        }
    }
    if let Some(id) = id {
        BucketKey::Id(id)
    } else if let Some(class) = class {
        BucketKey::Class(class)
    } else if let Some(name) = name {
        BucketKey::Name(name)
    } else {
        BucketKey::Universal
    }
}

impl RuleBuckets {
    /// Index `rules` by their rightmost compounds. A rule with several selectors
    /// is indexed once per selector.
    #[must_use]
    pub fn build(rules: &[CompiledRule]) -> Self {
        let mut buckets = RuleBuckets {
            len: rules.len(),
            ..RuleBuckets::default()
        };
        for (index, rule) in rules.iter().enumerate() {
            for selector in rule.selectors.slice() {
                match bucket_key(selector) {
                    BucketKey::Id(key) => buckets.by_id.entry(key).or_default().push(index),
                    BucketKey::Class(key) => buckets.by_class.entry(key).or_default().push(index),
                    BucketKey::Name(key) => buckets.by_name.entry(key).or_default().push(index),
                    BucketKey::Universal => buckets.universal.push(index),
                }
            }
        }
        for list in buckets
            .by_id
            .values_mut()
            .chain(buckets.by_class.values_mut())
            .chain(buckets.by_name.values_mut())
            .chain(std::iter::once(&mut buckets.universal))
        {
            list.dedup();
        }
        buckets
    }

    /// Every rule index that could match `node`, ascending and deduped.
    ///
    /// Sound by construction: a rule is only left out when the rightmost
    /// compound of *every* one of its selectors names an id, class or element
    /// this node does not have, which no matcher could then satisfy.
    #[must_use]
    pub fn candidates(&self, node: &Node) -> Vec<usize> {
        let mut out = self.universal.clone();
        node.borrow_id(|id| {
            if let Some(id) = id
                && let Some(list) = self.by_id.get(id.as_str())
            {
                out.extend_from_slice(list);
            }
        });
        node.borrow_classes(|classes| {
            for class in classes {
                if let Some(list) = self.by_class.get(class.as_str()) {
                    out.extend_from_slice(list);
                }
            }
        });
        if let Some(list) = self.by_name.get(&*node.name()) {
            out.extend_from_slice(list);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How many rules are indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no rules are indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Whether any selector in `list` matches `node`.
#[must_use]
pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &Node, cx: &mut MatchCx) -> bool {
    matches_with_specificity(list, node, cx).is_some()
}

/// [`matches`] with the ancestor filter switched off.
///
/// The bloom filter may only ever *reject* selectors that could not match;
/// this is the reference implementation that claim is tested against, and the
/// escape hatch for a caller that cannot position a filter.
#[must_use]
pub fn matches_unfiltered(
    list: &SelectorList<GtkSelectorImpl>,
    node: &Node,
    cx: &mut MatchCx,
) -> bool {
    let MatchCx { caches, .. } = cx;
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    list.slice()
        .iter()
        .any(|selector| matches_selector(selector, 0, None, node, &mut context))
}

/// The highest specificity among the selectors in `list` that match `node`, or
/// `None` when none of them do.
///
/// Specificity is per *selector*, not per rule: a prelude like
/// `notebook > header > tabs > arrow, button` contributes the specificity of
/// whichever of its selectors actually matched.
#[must_use]
pub fn matches_with_specificity(
    list: &SelectorList<GtkSelectorImpl>,
    node: &Node,
    cx: &mut MatchCx,
) -> Option<u32> {
    cx.seed_for(node);
    let MatchCx { caches, bloom, .. } = cx;
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        Some(&*bloom),
        caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    list.slice()
        .iter()
        .filter(|selector| {
            // `matches_selector_list` passes no hashes, so it can never use the
            // filter; going selector by selector is what turns the bloom filter
            // on (selectors-0.40.0/matching.rs:287-300).
            let hashes = AncestorHashes::new(selector, QuirksMode::NoQuirks);
            matches_selector(selector, 0, Some(&hashes), node, &mut context)
        })
        .map(Selector::specificity)
        .max()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{MatchCx, matches, matches_with_specificity, parse_selector_list};
    use crate::css::node::{Direction, Node, PseudoStates};

    thread_local! {
        /// Fixture roots. A node's parent link is `Weak` — a tree is owned by
        /// handles to its root — so a helper that hands back a *descendant*
        /// must park the root somewhere, or the ancestors it returns are freed
        /// before the assertions run.
        static FIXTURE_ROOTS: RefCell<Vec<Node>> = const { RefCell::new(Vec::new()) };
    }

    fn keep_alive(root: Node) {
        FIXTURE_ROOTS.with(|roots| roots.borrow_mut().push(root));
    }

    fn window_button(classes: &[&str], states: PseudoStates) -> Node {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        keep_alive(window);
        button
    }

    fn hits(selector: &str, node: &Node) -> bool {
        let list =
            parse_selector_list(selector).unwrap_or_else(|| panic!("`{selector}` failed to parse"));
        let mut cx = MatchCx::new();
        matches(&list, node, &mut cx)
    }

    #[test]
    fn element_name_and_child_combinator() {
        let node = window_button(&[], PseudoStates::default());
        assert!(hits("button", &node));
        assert!(hits("window > button", &node));
        assert!(hits("window button", &node));
        assert!(!hits("headerbar > button", &node));
        assert!(!hits("entry", &node));
    }

    #[test]
    fn pseudo_class_state_gates_matching() {
        let plain = window_button(&[], PseudoStates::default());
        let hovered = window_button(&[], PseudoStates::HOVER);
        assert!(!hits("button:hover", &plain));
        assert!(hits("button:hover", &hovered));
        assert!(!hits("button:active", &hovered));
        assert!(
            hits("button", &hovered),
            "state must not break the base match"
        );
    }

    #[test]
    fn style_classes_match() {
        let suggested = window_button(&["suggested-action"], PseudoStates::default());
        assert!(hits(".suggested-action", &suggested));
        assert!(hits("button.suggested-action", &suggested));
        assert!(!hits("button.destructive-action", &suggested));
        assert!(hits(
            "window.background > button.suggested-action",
            &suggested
        ));
    }

    #[test]
    fn unknown_pseudo_classes_parse_but_never_match() {
        // GTK carries pseudo-classes this engine does not model. They must not
        // make the whole rule unparseable (which would silently drop hundreds of
        // Adwaita rules) and must not match either.
        let node = window_button(&[], PseudoStates::default());
        assert!(hits("button", &node));
        assert!(!hits("button:placeholder-shown", &node));
        assert!(parse_selector_list("button:placeholder-shown").is_some());
    }

    #[test]
    fn specificity_follows_css_rules() {
        let one = parse_selector_list("button").unwrap();
        let two = parse_selector_list("button:hover").unwrap();
        let three = parse_selector_list("window > button.suggested-action:hover").unwrap();
        let spec = |l: &selectors::SelectorList<super::GtkSelectorImpl>| l.slice()[0].specificity();
        assert!(spec(&one) < spec(&two));
        assert!(spec(&two) < spec(&three));
    }

    #[test]
    fn adwaitas_real_button_preludes_parse() {
        for prelude in [
            "notebook > header > tabs > arrow, button",
            "notebook > header > tabs > arrow:hover, button:hover",
            "button.suggested-action",
            "button.suggested-action:active, button.suggested-action:checked",
            "columnview.view > header > button, treeview.view > header > button",
        ] {
            assert!(
                parse_selector_list(prelude).is_some(),
                "Adwaita prelude failed to parse: {prelude}"
            );
        }
        let base = parse_selector_list("notebook > header > tabs > arrow, button").unwrap();
        assert_eq!(base.slice().len(), 2);
        let node = window_button(&[], PseudoStates::default());
        let mut cx = MatchCx::new();
        assert!(matches(&base, &node, &mut cx));
    }

    #[test]
    fn dir_matches_the_nodes_direction() {
        let ltr = window_button(&[], PseudoStates::default());
        let rtl = window_button(&[], PseudoStates::default());
        rtl.set_direction(Some(Direction::Rtl));
        assert!(hits("button:dir(ltr)", &ltr));
        assert!(!hits("button:dir(rtl)", &ltr));
        assert!(hits("button:dir(rtl)", &rtl));
        assert!(!hits("button:dir(ltr)", &rtl));
        assert_eq!(ltr.direction(), Direction::Ltr, "default is ltr");
        // A bogus argument parses (so the rule survives) but never matches.
        assert!(!hits("button:dir(sideways)", &ltr));
    }

    #[test]
    fn drop_never_matches_but_keeps_the_rest_of_the_comma_list() {
        // E6: `colorswatch:drop(active), colorswatch` used to fail to parse
        // as a whole, taking the valid `colorswatch` selector with it.
        let node = Node::new("colorswatch");
        let list = parse_selector_list("colorswatch:drop(active), colorswatch")
            .expect("the comma list must parse");
        assert_eq!(list.slice().len(), 2);
        let mut cx = MatchCx::new();
        assert!(matches(&list, &node, &mut cx));
        assert!(!hits("colorswatch:drop(active)", &node));

        // New in M2: the node tree can actually carry the drop state.
        node.set_state(PseudoStates::DROP_ACTIVE, true);
        assert!(hits("colorswatch:drop(active)", &node));
        assert!(
            !hits("colorswatch:drop(highlight)", &node),
            "only `active` is modelled"
        );
    }

    #[test]
    fn unknown_functional_pseudo_classes_parse_but_never_match() {
        let node = window_button(&[], PseudoStates::default());
        assert!(parse_selector_list("button:lang(en), button").is_some());
        assert!(!hits("button:lang(en)", &node));
        assert!(hits("button", &node));
    }

    // ---- new in M2: the tree-structural constructs M1 could not express ----

    fn linked_box(count: usize) -> Node {
        let box_node = Node::with_classes("box", &["linked"]);
        for _ in 0..count {
            box_node.append_child(&Node::new("button"));
        }
        box_node
    }

    #[test]
    fn first_last_and_only_child_read_the_real_sibling_list() {
        let three = linked_box(3);
        let first = three.child(0).expect("first");
        let middle = three.child(1).expect("middle");
        let last = three.child(2).expect("last");

        assert!(hits("button:first-child", &first));
        assert!(!hits("button:first-child", &middle));
        assert!(!hits("button:first-child", &last));
        assert!(hits("button:last-child", &last));
        assert!(!hits("button:last-child", &middle));
        assert!(hits("button:not(:first-child)", &middle));
        assert!(!hits("button:only-child", &first));

        let alone = linked_box(1);
        let only = alone.child(0).expect("only");
        assert!(hits("button:only-child", &only));
        assert!(hits("button:first-child", &only));
        assert!(hits("button:last-child", &only));
        assert!(!hits("button:not(:first-child)", &only));
    }

    #[test]
    fn nth_child_counts_from_both_ends() {
        let five = linked_box(5);
        let nodes: Vec<Node> = five.children();
        assert!(hits("button:nth-child(2)", &nodes[1]));
        assert!(!hits("button:nth-child(2)", &nodes[2]));
        assert!(hits("button:nth-child(odd)", &nodes[0]));
        assert!(hits("button:nth-child(odd)", &nodes[2]));
        assert!(hits("button:nth-child(2n)", &nodes[1]));
        assert!(hits("button:nth-child(2n + 1)", &nodes[4]));
        assert!(hits("button:nth-last-child(1)", &nodes[4]));
        assert!(hits("button:nth-last-child(2)", &nodes[3]));
        assert!(!hits("button:nth-last-child(2)", &nodes[4]));
    }

    #[test]
    fn adjacent_and_general_sibling_combinators_match() {
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        let button = Node::new("button");
        let label = Node::new("label");
        box_node.append_child(&entry);
        box_node.append_child(&button);
        box_node.append_child(&label);

        assert!(hits("entry + button", &button));
        assert!(!hits("entry + label", &label), "+ is strictly adjacent");
        assert!(hits("entry ~ label", &label));
        assert!(hits("entry ~ button", &button));
        assert!(!hits("label ~ entry", &entry), "~ looks backwards only");
    }

    #[test]
    fn root_empty_and_id_selectors_match_the_tree() {
        let window = Node::new("window");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        window.append_child(&swatch);

        assert!(hits(":root", &window));
        assert!(!hits(":root", &swatch));
        assert!(hits("colorswatch#add-color-button", &swatch));
        assert!(!hits("colorswatch#other", &swatch));
        assert!(hits("#add-color-button:only-child", &swatch));
        assert!(hits("colorswatch:empty", &swatch), "no children == :empty");
        assert!(!hits("window:empty", &window));
    }

    #[test]
    fn focus_within_matches_the_ancestors_of_the_focused_node() {
        let window = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        window.append_child(&box_node);
        box_node.append_child(&entry);

        assert!(!hits("window:focus-within", &window));
        entry.set_state(PseudoStates::FOCUS, true);
        assert!(hits("window:focus-within", &window));
        assert!(hits("box:focus-within", &box_node));
        assert!(hits("entry:focus", &entry));
        assert!(
            !hits("window:focus", &window),
            ":focus itself does not propagate"
        );
    }

    #[test]
    fn matching_follows_the_tree_after_it_is_restructured() {
        // The nth-index cache inside `MatchCx` is per pass, not per tree: a
        // context reused across a mutation must not answer from stale counts.
        let box_node = linked_box(2);
        let first = box_node.child(0).expect("first");
        let second = box_node.child(1).expect("second");
        let list = parse_selector_list("button:last-child").expect("parses");
        let mut cx = MatchCx::new();

        assert!(!matches(&list, &first, &mut cx));
        assert!(matches(&list, &second, &mut cx));

        box_node.remove_child(&second);
        assert!(
            matches(&list, &first, &mut cx),
            "the surviving child is now the last one"
        );
    }

    #[test]
    fn matches_with_specificity_returns_the_best_matching_selector() {
        let node = window_button(&["suggested-action"], PseudoStates::HOVER);
        let mut cx = MatchCx::new();

        let list =
            parse_selector_list("button, window > button.suggested-action:hover").expect("parses");
        let best = matches_with_specificity(&list, &node, &mut cx).expect("the list matches");
        let plain = parse_selector_list("button").expect("parses");
        let plain_specificity =
            matches_with_specificity(&plain, &node, &mut cx).expect("`button` matches");
        assert!(
            best > plain_specificity,
            "the most specific matching selector wins, not the first"
        );

        let miss = parse_selector_list("entry, headerbar").expect("parses");
        assert_eq!(matches_with_specificity(&miss, &node, &mut cx), None);
    }

    #[test]
    fn nth_child_of_a_selector_list_is_not_enabled() {
        // GTK CSS has no `:nth-child(An+B of S)`; `GtkSelectorParser` keeps
        // `parse_nth_child_of`'s `false` default, so the form is rejected.
        assert!(parse_selector_list("button:nth-child(2n of .flat)").is_none());
        assert!(parse_selector_list("button:nth-child(2n)").is_some());
    }

    #[test]
    fn selector_parsing_never_panics_on_odd_input() {
        // Theme files are third-party text; a panic here takes the compositor
        // down. Every one of these must return Some or None, never unwind.
        for text in [
            "",
            " ",
            ",",
            "button,",
            ",button",
            "button >",
            "> button",
            "button ~~ label",
            "button::",
            "button::selection::selection",
            "button:",
            "button:()",
            "button:dir()",
            "button:dir(",
            "button:drop()",
            "button:nth-child()",
            "button:nth-child(+)",
            "button:nth-child(2n+)",
            "button:not()",
            "button:not(:not(:not(button)))",
            "#",
            ".",
            "#.",
            "[",
            "[attr=]",
            "button[",
            "\u{0}",
            "🙂",
            "button/*unterminated",
            "button{",
            "*|*",
            "|button",
            "button:is(,)",
            "button:where()",
            &"button ".repeat(200),
            &":not(".repeat(50),
        ] {
            let _ = parse_selector_list(text);
        }
        assert!(
            parse_selector_list("button").is_some(),
            "the parser still works"
        );
    }

    #[test]
    fn the_gtk_pseudo_class_set_is_modelled_not_swallowed() {
        use super::GtkPseudoClass;

        // Every pseudo-class GTK 4.22 sets on a node must reach the matcher as a
        // modelled variant. Anything landing in `Other` matches nothing, which is
        // how M1 silently dropped :focus-within, :indeterminate, :link, :visited.
        for (text, expected) in [
            ("hover", GtkPseudoClass::Hover),
            ("active", GtkPseudoClass::Active),
            ("checked", GtkPseudoClass::Checked),
            ("indeterminate", GtkPseudoClass::Indeterminate),
            ("disabled", GtkPseudoClass::Disabled),
            ("focus", GtkPseudoClass::Focus),
            ("focus-visible", GtkPseudoClass::FocusVisible),
            ("focus-within", GtkPseudoClass::FocusWithin),
            ("backdrop", GtkPseudoClass::Backdrop),
            ("selected", GtkPseudoClass::Selected),
            ("link", GtkPseudoClass::Link),
            ("visited", GtkPseudoClass::Visited),
            ("FOCUS-WITHIN", GtkPseudoClass::FocusWithin),
        ] {
            assert_eq!(
                super::match_pseudo_class_name(text),
                expected,
                "`:{text}` must be modelled"
            );
        }
        assert!(matches!(
            super::match_pseudo_class_name("hover-ish"),
            GtkPseudoClass::Other(_)
        ));
    }

    #[test]
    fn unique_hashes_are_inserted_and_reported() {
        use selectors::Element;
        use selectors::bloom::{BLOOM_HASH_MASK, BloomFilter};

        let node = Node::with_classes("button", &["flat"]);
        node.set_id(Some("close"));
        let mut filter = BloomFilter::new();
        assert!(
            node.add_element_unique_hashes(&mut filter),
            "returning false makes the ancestor filter a permanent no-op"
        );
        for text in ["button", "flat", "close"] {
            assert!(
                filter
                    .might_contain_hash(crate::css::node::fnv1a(text.as_bytes()) & BLOOM_HASH_MASK),
                "`{text}` must be in the filter"
            );
        }
    }

    #[test]
    fn push_and_pop_ancestor_are_exact_inverses() {
        // `hits()` builds its own fresh `MatchCx` (via `matches` -> `seed_for`),
        // so asserting through it never observes whatever `cx.push_ancestor`/
        // `cx.pop_ancestor` actually did to `cx`'s filter -- the test would stay
        // green even if `pop_ancestor` were gutted into a no-op. Assert against
        // `cx`'s own filter state directly instead, bypassing `seed_for`.
        use selectors::bloom::BLOOM_HASH_MASK;

        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);

        let background_hash = crate::css::node::fnv1a(b"background") & BLOOM_HASH_MASK;

        let mut cx = MatchCx::new();
        assert!(
            !cx.might_contain_hash(background_hash),
            "a fresh filter must not already report an unrelated hash"
        );

        cx.push_ancestor(&window);
        assert!(
            cx.might_contain_hash(background_hash),
            "push_ancestor(&window) must insert window's own hashes into cx's filter"
        );

        cx.pop_ancestor(&window);
        assert!(
            !cx.might_contain_hash(background_hash),
            "pop_ancestor(&window) must undo exactly what push_ancestor(&window) did \
             (mutation check: gutting pop_ancestor into a no-op fails this assertion)"
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "pop_ancestor without a matching push")
    )]
    fn pop_ancestor_without_a_push_is_a_bug() {
        // `depth` exists to catch a caller popping more than it pushed -- a bug
        // that would otherwise sail through silently (the saturating decrement
        // just clamps at zero). In debug builds this must panic; in a release
        // build (`debug_assert!` compiled out) the saturating decrement is the
        // only guard left, so the call is harmless there and this test is
        // skipped rather than asserting a panic that cannot happen.
        let window = Node::with_classes("window", &["background"]);
        let mut cx = MatchCx::new();
        cx.pop_ancestor(&window);
    }

    #[test]
    fn seed_for_reseeds_on_a_tree_id_mismatch_even_when_addr_and_generation_collide() {
        // Regression test for the ABA hazard in `MatchCx`'s cache key. It used
        // to be `(parent addr, tree generation)` alone: a tree that gets
        // dropped can free memory a later, wholly unrelated tree's root then
        // gets allocated at, and if that new tree also happens to read the
        // same generation counter value (every freshly built, untouched tree
        // starts at generation 1, so this is not exotic), the old key would
        // read back a hit for the wrong tree -- `seed_for` would skip
        // reseeding and leave a stale bloom filter in place, silently
        // rejecting real matches. `tree_id` closes the hole because it is
        // drawn from a process-wide monotonic counter that never repeats.
        //
        // This test forges that exact collision by hand -- an address match
        // real allocator reuse *would* produce, but that a test cannot force
        // -- to prove the fix without depending on allocator behaviour.
        use selectors::bloom::BLOOM_HASH_MASK;

        let tree_a = Node::with_classes("window", &["danger"]);
        let child_a = Node::new("button");
        tree_a.append_child(&child_a);
        let danger_hash = crate::css::node::fnv1a(b"danger") & BLOOM_HASH_MASK;

        let mut cx = MatchCx::new();
        cx.seed_for(&child_a);
        assert!(
            cx.might_contain_hash(danger_hash),
            "sanity: a real seed_for must populate the filter with the ancestor's hashes"
        );

        // Forge `cx` into the exact state an `(addr, generation)`-only cache
        // could reach after a dropped tree's address was reused: the addr and
        // generation on file are `child_a`'s real, current ones (so an
        // addr/generation-only key would call this "already seeded"), but the
        // tree id on file belongs to a different tree, and the bloom filter
        // holds that other tree's (empty, here) ancestor hashes rather than
        // `child_a`'s real ancestor's.
        let real_addr = tree_a.addr();
        let real_generation = child_a.generation();
        let other_tree_id = tree_a.tree_id() ^ 1; // guaranteed different from tree_a's real id
        cx.filter_top = Some((other_tree_id, real_addr, real_generation));
        cx.generation = Some((other_tree_id, real_generation));
        cx.bloom.clear();

        cx.seed_for(&child_a);
        assert!(
            cx.might_contain_hash(danger_hash),
            "seed_for must reseed when the cached tree id doesn't match `child_a`'s, \
             even though the cached (addr, generation) exactly matches -- without \
             tree_id in the key this would wrongly be treated as an up-to-date filter"
        );
    }

    #[test]
    fn the_bloom_filter_never_changes_an_answer() {
        // The filter may only *fast-reject* selectors that could not match. Any
        // rule it rejects that actually matches is a silently lost style.
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let window = Node::with_classes("window", &["background", "csd"]);
        let headerbar = Node::with_classes("headerbar", &["titlebar"]);
        // The bundled Adwaita subset only reaches a bare `box` through
        // `headerbar > windowhandle > box` (adwaita-light.css:620); there is no
        // rule with `box` as a rightmost compound one level under `headerbar`
        // directly, so `windowhandle` is threaded in to give `box_node` a real
        // rule to match, matching upstream GTK's own headerbar structure.
        let windowhandle = Node::new("windowhandle");
        let box_node = Node::with_classes("box", &["linked", "horizontal"]);
        let button = Node::with_classes("button", &["suggested-action", "text-button"]);
        let label = Node::new("label");
        window.append_child(&headerbar);
        headerbar.append_child(&windowhandle);
        windowhandle.append_child(&box_node);
        box_node.append_child(&Node::new("entry"));
        box_node.append_child(&button);
        button.append_child(&label);
        button.set_states(PseudoStates::HOVER);

        let mut with_filter = MatchCx::new();
        for node in [&window, &headerbar, &box_node, &button, &label] {
            let mut filtered = Vec::new();
            let mut unfiltered = Vec::new();
            for (index, rule) in sheet.rules.iter().enumerate() {
                if matches(&rule.selectors, node, &mut with_filter) {
                    filtered.push(index);
                }
                // A fresh context that has never been seeded still holds an empty
                // filter, so re-seed it per node to compare like with like: the
                // reference answer comes from matching every selector directly.
                let mut plain = MatchCx::new();
                plain.seed_for(node);
                if super::matches_unfiltered(&rule.selectors, node, &mut plain) {
                    unfiltered.push(index);
                }
            }
            assert_eq!(
                filtered,
                unfiltered,
                "the bloom filter changed the answer for {:?}",
                node.name()
            );
            assert!(
                !filtered.is_empty(),
                "{:?} must match something",
                node.name()
            );
        }
    }

    #[test]
    fn every_pseudo_class_serialises_back_to_its_own_syntax() {
        use super::GtkPseudoClass;
        use cssparser::ToCss;

        let mut out = String::new();
        for (pseudo, expected) in [
            (GtkPseudoClass::FocusWithin, ":focus-within"),
            (GtkPseudoClass::Indeterminate, ":indeterminate"),
            (GtkPseudoClass::Link, ":link"),
            (GtkPseudoClass::Visited, ":visited"),
        ] {
            out.clear();
            pseudo
                .to_css(&mut out)
                .expect("serialising to a String cannot fail");
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn buckets_index_by_the_rightmost_compound() {
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        // id beats class beats name; a rightmost compound with none of the three
        // (`* > :first-child`) must land in the universal bucket.
        let sheet = CompiledSheet::compile(concat!(
            "button { color: red; }\n",
            ".suggested-action { color: green; }\n",
            "#close { color: blue; }\n",
            "window > * { color: black; }\n",
            "box > :first-child { color: white; }\n",
        ));
        assert_eq!(sheet.rules.len(), 5, "the fixture must compile whole");
        let buckets = RuleBuckets::build(&sheet.rules);
        assert_eq!(buckets.len(), 5);

        let window = Node::new("window");
        let button = Node::with_classes("button", &["suggested-action"]);
        window.append_child(&button);
        let candidates = buckets.candidates(&button);
        // button (name) + .suggested-action (class) + the two universals.
        assert_eq!(candidates, vec![0, 1, 3, 4]);
        assert!(
            !candidates.contains(&2),
            "a rule bucketed by id must not reach a node without that id"
        );

        button.set_id(Some("close"));
        assert_eq!(buckets.candidates(&button), vec![0, 1, 2, 3, 4]);

        let entry = Node::new("entry");
        window.append_child(&entry);
        assert_eq!(
            buckets.candidates(&entry),
            vec![3, 4],
            "only the universal bucket applies"
        );
    }

    #[test]
    fn a_rule_lands_in_a_bucket_for_every_selector_it_has() {
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        let sheet =
            CompiledSheet::compile("notebook > header > tabs > arrow, button { color: red; }");
        let buckets = RuleBuckets::build(&sheet.rules);
        assert_eq!(buckets.candidates(&Node::new("arrow")), vec![0]);
        assert_eq!(buckets.candidates(&Node::new("button")), vec![0]);
        assert!(buckets.candidates(&Node::new("entry")).is_empty());
    }

    #[test]
    fn every_matching_adwaita_rule_is_offered_as_a_candidate() {
        // Bucketing is only sound if it never hides a rule that would match.
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let buckets = RuleBuckets::build(&sheet.rules);

        let window = Node::with_classes("window", &["background", "csd"]);
        let headerbar = Node::with_classes("headerbar", &["titlebar"]);
        // As in `the_bloom_filter_never_changes_an_answer` above: the bundled
        // Adwaita subset only reaches a bare `box` through
        // `headerbar > windowhandle > box` (adwaita-light.css:620), so
        // `windowhandle` is threaded in to give `box_node` a real rule to
        // match, mirroring upstream GTK's own headerbar structure.
        let windowhandle = Node::new("windowhandle");
        let box_node = Node::with_classes("box", &["linked"]);
        let button = Node::with_classes("button", &["suggested-action"]);
        let label = Node::new("label");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        window.append_child(&headerbar);
        headerbar.append_child(&windowhandle);
        windowhandle.append_child(&box_node);
        box_node.append_child(&button);
        button.append_child(&label);
        headerbar.append_child(&swatch);
        button.set_states(PseudoStates::HOVER);

        let mut cx = MatchCx::new();
        for node in [&window, &headerbar, &box_node, &button, &label, &swatch] {
            let candidates = buckets.candidates(node);
            let mut matched = 0usize;
            for (index, rule) in sheet.rules.iter().enumerate() {
                if matches(&rule.selectors, node, &mut cx) {
                    matched += 1;
                    assert!(
                        candidates.binary_search(&index).is_ok(),
                        "rule {index} matches {:?} but was not a candidate",
                        node.name()
                    );
                }
            }
            assert!(matched > 0, "{:?} must match something", node.name());
            assert!(
                candidates.len() < sheet.rules.len(),
                "bucketing must actually narrow {} rules for {:?}",
                sheet.rules.len(),
                node.name()
            );
        }
    }

    /// The rules M1 could not match: a two-node chain made `:first-child`,
    /// `:last-child` and `:only-child` match everything and `:not(:first-child)`
    /// match nothing. Derived from the vendored sheet with
    /// `grep -c -E ':(first|last|only)-child' ui/themes/adwaita-light.css`.
    const ADWAITA_TREE_STRUCTURAL_RULES: usize = 44;

    #[test]
    fn adwaitas_tree_structural_rules_are_all_present_and_parse() {
        use crate::css::parse::parse_stylesheet;

        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        let structural: Vec<&str> = sheet
            .rules
            .iter()
            .map(|rule| rule.selector_text.as_str())
            .filter(|text| {
                text.contains(":first-child")
                    || text.contains(":last-child")
                    || text.contains(":only-child")
            })
            .collect();
        assert_eq!(
            structural.len(),
            ADWAITA_TREE_STRUCTURAL_RULES,
            "the vendored Adwaita sheet's tree-structural rule count moved"
        );
        for text in &structural {
            assert!(
                parse_selector_list(text).is_some(),
                "a tree-structural prelude must survive the selector parser: {text}"
            );
        }
    }

    #[test]
    fn adwaitas_tree_structural_selectors_match_a_real_tree() {
        // Each of these is a verbatim selector from the vendored sheet. Under
        // M1's constant-stub Element they all answered wrongly.
        let list_box = Node::with_classes("box", &["boxed-list"]);
        let first = Node::new("row");
        let middle = Node::new("row");
        let last = Node::new("row");
        for row in [&first, &middle, &last] {
            list_box.append_child(row);
        }
        assert!(hits(".boxed-list > row:first-child", &first));
        assert!(!hits(".boxed-list > row:first-child", &middle));
        assert!(hits(".boxed-list > row:last-child", &last));
        assert!(!hits(".boxed-list > row:last-child", &middle));

        // `.linked.vertical > button:not(:first-child)`
        let linked = Node::with_classes("box", &["linked", "vertical"]);
        let top = Node::new("button");
        let bottom = Node::new("button");
        linked.append_child(&top);
        linked.append_child(&bottom);
        assert!(!hits(".linked.vertical > button:not(:first-child)", &top));
        assert!(hits(".linked.vertical > button:not(:first-child)", &bottom));
        assert!(hits(".linked.vertical > button:not(:last-child)", &top));

        // `notebook > header.top > tabs:not(:only-child)`
        let notebook = Node::new("notebook");
        let header = Node::with_classes("header", &["top"]);
        let tabs = Node::new("tabs");
        let arrow = Node::new("arrow");
        notebook.append_child(&header);
        header.append_child(&tabs);
        assert!(
            !hits("notebook > header.top > tabs:not(:only-child)", &tabs),
            "an only child must not match :not(:only-child)"
        );
        header.append_child(&arrow);
        assert!(hits("notebook > header.top > tabs:not(:only-child)", &tabs));
        assert!(hits(
            "notebook > header.top > tabs:not(:only-child):first-child",
            &tabs
        ));

        // `dropdown.linked button:nth-child(2):dir(ltr)`
        let dropdown = Node::with_classes("dropdown", &["linked"]);
        let one = Node::new("button");
        let two = Node::new("button");
        dropdown.append_child(&one);
        dropdown.append_child(&two);
        assert!(!hits("dropdown.linked button:nth-child(2):dir(ltr)", &one));
        assert!(hits("dropdown.linked button:nth-child(2):dir(ltr)", &two));
        dropdown.set_direction(Some(Direction::Rtl));
        assert!(!hits("dropdown.linked button:nth-child(2):dir(ltr)", &two));
        assert!(hits("dropdown.linked button:nth-child(2):dir(rtl)", &two));

        // `.linked.vertical > entry:drop(active):not(:only-child) + button`
        let vertical = Node::with_classes("box", &["linked", "vertical"]);
        let entry = Node::new("entry");
        let button = Node::new("button");
        vertical.append_child(&entry);
        vertical.append_child(&button);
        assert!(!hits(
            ".linked.vertical > entry:drop(active):not(:only-child) + button",
            &button
        ));
        entry.set_state(PseudoStates::DROP_ACTIVE, true);
        assert!(hits(
            ".linked.vertical > entry:drop(active):not(:only-child) + button",
            &button
        ));

        // `colorswatch#add-color-button:only-child`
        let grid = Node::new("colorchooser");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        grid.append_child(&swatch);
        assert!(hits("colorswatch#add-color-button:only-child", &swatch));
        grid.append_child(&Node::new("colorswatch"));
        assert!(!hits("colorswatch#add-color-button:only-child", &swatch));

        // `textview > text > selection:focus-within`
        let textview = Node::new("textview");
        let text = Node::new("text");
        let selection = Node::new("selection");
        let caret = Node::new("cursor");
        textview.append_child(&text);
        text.append_child(&selection);
        selection.append_child(&caret);
        assert!(!hits(
            "textview > text > selection:focus-within",
            &selection
        ));
        caret.set_state(PseudoStates::FOCUS, true);
        assert!(hits("textview > text > selection:focus-within", &selection));
    }

    #[test]
    fn a_real_adwaita_tree_matches_more_rules_than_a_flat_one() {
        // The end-to-end proof that the tree is load-bearing: the same button
        // node, once given real siblings and ancestors, matches a strictly
        // larger set of Adwaita rules than it does standing alone.
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let mut cx = MatchCx::new();

        let lonely = Node::new("button");
        let lonely_hits = sheet
            .rules
            .iter()
            .filter(|rule| matches(&rule.selectors, &lonely, &mut cx))
            .count();

        let window = Node::with_classes("window", &["background"]);
        let box_node = Node::with_classes("box", &["linked"]);
        let sibling = Node::new("button");
        let button = Node::new("button");
        window.append_child(&box_node);
        box_node.append_child(&sibling);
        box_node.append_child(&button);
        let embedded_hits = sheet
            .rules
            .iter()
            .filter(|rule| matches(&rule.selectors, &button, &mut cx))
            .count();

        assert!(
            embedded_hits > lonely_hits,
            "a button inside a linked box must match more rules ({embedded_hits}) \
             than a detached one ({lonely_hits})"
        );
    }
}
