//! GTK's CSS node model expressed through Servo's `selectors` crate.
//!
//! GTK widgets are not a DOM: nodes have an element name (`window`,
//! `button`, `label`), style classes, pseudo-class state, and a parent --
//! and nothing else. No ids, no attributes, no namespaces, no siblings that
//! M1 needs. `CssNode` models exactly that and implements
//! [`selectors::Element`], so the upstream matcher drives selection with no
//! fork and no shim.

use std::borrow::Borrow;
use std::fmt;
use std::rc::Rc;

use cssparser::{CowRcStr, Parser as CssParser, ParserInput, SourceLocation, ToCss};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, MatchingContext, matches_selector_list};
use selectors::parser::{
    NonTSPseudoClass, ParseRelative, PseudoElement as PseudoElementTrait, SelectorParseErrorKind,
};
use selectors::{Element, OpaqueElement, SelectorImpl, SelectorList};

/// An interned CSS identifier with a cached hash.
///
/// `selectors` requires `PrecomputedHash` on its `Identifier`, `LocalName`
/// and `NamespaceUrl` types; one newtype covers all of them.
#[derive(Clone, Debug)]
pub struct CssString {
    text: String,
    hash: u32,
}

/// FNV-1a, 32-bit. Cheap, stable, and adequate for the ancestor-hash
/// filtering `selectors` uses it for.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
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
    /// `:disabled`
    Disabled,
    /// `:focus`
    Focus,
    /// `:focus-visible`
    FocusVisible,
    /// `:backdrop`
    Backdrop,
    /// `:selected`
    Selected,
    /// Any pseudo-class M1 does not model. Parses; never matches.
    Other(CssString),
}

impl ToCss for GtkPseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_char(':')?;
        match self {
            Self::Hover => dest.write_str("hover"),
            Self::Active => dest.write_str("active"),
            Self::Checked => dest.write_str("checked"),
            Self::Disabled => dest.write_str("disabled"),
            Self::Focus => dest.write_str("focus"),
            Self::FocusVisible => dest.write_str("focus-visible"),
            Self::Backdrop => dest.write_str("backdrop"),
            Self::Selected => dest.write_str("selected"),
            Self::Other(name) => dest.write_str(name.as_str()),
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
            Self::Hover | Self::Active | Self::Focus | Self::FocusVisible
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
        "disabled" => GtkPseudoClass::Disabled,
        "focus" => GtkPseudoClass::Focus,
        "focus-visible" => GtkPseudoClass::FocusVisible,
        "backdrop" => GtkPseudoClass::Backdrop,
        "selected" => GtkPseudoClass::Selected,
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
            parent,
        }))
    }

    /// A copy of this node with different pseudo-class state, sharing the
    /// same name, classes and ancestors.
    #[must_use]
    pub fn with_states(&self, states: PseudoStates) -> Self {
        Self(Rc::new(NodeData {
            name: self.0.name.clone(),
            classes: self.0.classes.clone(),
            states,
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
            GtkPseudoClass::Other(_) => false,
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

/// Whether any selector in `list` matches `node`.
#[must_use]
pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &CssNode) -> bool {
    let mut caches = SelectorCaches::default();
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        &mut caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    matches_selector_list(list, node, &mut context)
}

#[cfg(test)]
mod tests {
    use super::{CssNode, PseudoStates, matches, parse_selector_list};

    fn window_button(classes: &[&str], states: PseudoStates) -> CssNode {
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        CssNode::new("button", classes, states, Some(window))
    }

    fn hits(selector: &str, node: &CssNode) -> bool {
        let list =
            parse_selector_list(selector).unwrap_or_else(|| panic!("`{selector}` failed to parse"));
        matches(&list, node)
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
        let hovered = window_button(
            &[],
            PseudoStates {
                hover: true,
                ..PseudoStates::default()
            },
        );
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
        // GTK carries pseudo-classes this milestone does not model. They must
        // not make the whole rule unparseable (which would silently drop
        // hundreds of Adwaita rules) and must not match either.
        let node = window_button(&[], PseudoStates::default());
        assert!(hits("button", &node));
        assert!(!hits("button:indeterminate", &node));
        assert!(parse_selector_list("button:indeterminate").is_some());
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
        assert!(matches(&base, &node));
    }
}
