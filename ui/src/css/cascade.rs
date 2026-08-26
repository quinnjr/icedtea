//! The cascade: which declarations a node actually gets, and in what order.
//!
//! Origin is not modelled (M1 loads exactly one author sheet), so the sort
//! key is CSS's remainder: `!important` first, then selector specificity,
//! then source order, then position within the rule. Later wins on a tie,
//! which is what makes a theme's own overrides work.
//!
//! Two things make this more than a `HashMap<String, String>`:
//!
//! * **Shorthands are expanded here** (`super::shorthand`), carrying the
//!   shorthand's own key, so `border-width: 5px` and a later
//!   `border: 1px solid` are ordered by the cascade rather than by which
//!   property the consumer reads first.
//! * **Runner-ups are kept.** A theme routinely wins a property with a value
//!   this milestone cannot interpret (`border-radius: 100%`,
//!   `rgb(from currentColor ...)`). Reverting to the initial value in that
//!   case throws away a perfectly good declaration that *did* match, so
//!   `super::computed` walks down the list instead. CSS proper calls an
//!   uninterpretable computed value "invalid at computed-value time" and
//!   uses the inherited or initial value; falling back to the next
//!   applicable declaration is a deliberate M1 divergence, and the safer
//!   one while the property coverage is this narrow.
//!
//! That divergence cuts both ways, and the second direction is worth
//! stating plainly: a *later* declaration this engine cannot parse does not
//! merely fail to apply -- the property keeps the **earlier** longhand's
//! value. `background: nosuch(1)` after a working `background-image`, or
//! Adwaita:640's `cross-fade(...)`, therefore paints the older background
//! rather than nothing. Real CSS would drop the unparseable declaration at
//! parse time and land on the same place; real CSS with a value that parses
//! but computes to nothing would fall back to inherited/initial instead.
//! Every such step is logged at debug with the property, the value and its
//! rank.

use std::collections::HashMap;

use selectors::SelectorList;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{MatchingContext, matches_selector};

use super::colors::{ColorTable, build_color_table};
use super::parse::{Declaration, parse_stylesheet};
use super::select::{CssNode, GtkSelectorImpl, parse_selector_list};
use super::shorthand;

/// A rule whose prelude has been compiled to a selector list.
#[derive(Debug)]
pub struct CompiledRule {
    /// The rule's selectors.
    pub selectors: SelectorList<GtkSelectorImpl>,
    /// The rule's declarations.
    pub declarations: Vec<Declaration>,
    /// Index of this rule in the source sheet.
    pub source_order: usize,
}

/// A stylesheet ready for matching.
#[derive(Debug)]
pub struct CompiledSheet {
    /// Rules whose preludes parsed.
    pub rules: Vec<CompiledRule>,
    /// Resolved `@define-color` names.
    pub colors: ColorTable,
}

impl CompiledSheet {
    /// Parse and compile `css`.
    ///
    /// Rules whose prelude does not parse are dropped with a debug log;
    /// a real theme's unparseable corners must not take the sheet with them.
    #[must_use]
    pub fn compile(css: &str) -> Self {
        Self::from_stylesheet(parse_stylesheet(css))
    }

    /// Compile an already-parsed sheet -- the form
    /// [`super::parse::parse_stylesheet_with_base`] produces.
    #[must_use]
    pub fn from_stylesheet(sheet: super::parse::Stylesheet) -> Self {
        let colors = build_color_table(&sheet.color_definitions);
        let mut rules = Vec::with_capacity(sheet.rules.len());
        for rule in sheet.rules {
            match parse_selector_list(&rule.selector_text) {
                Some(selectors) => rules.push(CompiledRule {
                    selectors,
                    declarations: rule.declarations,
                    source_order: rule.source_order,
                }),
                None => {
                    tracing::debug!(prelude = %rule.selector_text, "unparseable selector list; dropping rule");
                }
            }
        }
        Self { rules, colors }
    }
}

/// Where a declaration sits in the cascade. Ordered worst-to-best, so
/// `max` is the winner and a descending sort puts the winner first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CascadeKey {
    /// Whether the declaration carried `!important`.
    pub important: bool,
    /// The specificity of the selector that actually matched.
    pub specificity: u32,
    /// The rule's index in the sheet.
    pub source_order: usize,
    /// The declaration's index within its rule -- the last tiebreak, so the
    /// later of two declarations in one block wins.
    pub declaration_order: usize,
}

/// One declaration that applied to a node, with its place in the cascade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadedDecl {
    /// The declaration's value (a longhand value: shorthands are expanded).
    pub value: String,
    /// Where it sits in the cascade.
    pub key: CascadeKey,
}

/// Every declaration that applied to a node, per longhand, best first.
#[derive(Clone, Debug, Default)]
pub struct CascadedValues {
    /// Longhand property name -> its declarations, sorted best-first.
    pub map: HashMap<String, Vec<CascadedDecl>>,
}

impl CascadedValues {
    /// Every declaration for `name`, winner first.
    #[must_use]
    pub fn candidates(&self, name: &str) -> &[CascadedDecl] {
        self.map.get(name).map_or(&[], Vec::as_slice)
    }

    /// The winning value for `name`, ignoring runner-ups.
    #[must_use]
    pub fn winner(&self, name: &str) -> Option<&str> {
        self.candidates(name)
            .first()
            .map(|decl| decl.value.as_str())
    }

    /// Whether no declaration applied at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// How many distinct longhands applied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// The declarations `node` wins, keyed by longhand property name.
///
/// Specificity is taken per *selector*, not per rule: a rule whose prelude
/// is `notebook > header > tabs > arrow, button` contributes the specificity
/// of whichever of its selectors actually matched.
#[must_use]
pub fn cascade(sheet: &CompiledSheet, node: &CssNode) -> CascadedValues {
    // One matching context for the whole sheet: the previous code cloned a
    // `SelectorList` and built fresh `SelectorCaches` per selector per rule.
    let mut caches = SelectorCaches::default();
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        &mut caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );

    let mut map: HashMap<String, Vec<CascadedDecl>> = HashMap::new();
    for rule in &sheet.rules {
        let Some(specificity) = rule
            .selectors
            .slice()
            .iter()
            .filter(|selector| matches_selector(selector, 0, None, node, &mut context))
            .map(selectors::parser::Selector::specificity)
            .max()
        else {
            continue;
        };

        for (declaration_order, decl) in rule.declarations.iter().enumerate() {
            let key = CascadeKey {
                important: decl.important,
                specificity,
                source_order: rule.source_order,
                declaration_order,
            };
            for (name, value) in shorthand::expand(&decl.name, &decl.value) {
                map.entry(name)
                    .or_default()
                    .push(CascadedDecl { value, key });
            }
        }
    }

    for candidates in map.values_mut() {
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.key));
    }
    CascadedValues { map }
}

#[cfg(test)]
mod tests {
    use super::{CompiledSheet, cascade};
    use crate::css::select::{CssNode, PseudoStates};

    fn button(classes: &[&str], states: PseudoStates) -> CssNode {
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        CssNode::new("button", classes, states, Some(window))
    }

    #[test]
    fn higher_specificity_wins_over_source_order() {
        let sheet = CompiledSheet::compile(
            "button.suggested-action { color: blue }\n\
             button { color: red }",
        );
        let node = button(&["suggested-action"], PseudoStates::default());
        assert_eq!(cascade(&sheet, &node).winner("color"), Some("blue"));
    }

    #[test]
    fn equal_specificity_falls_back_to_source_order() {
        let sheet = CompiledSheet::compile("button { color: red }\nbutton { color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(cascade(&sheet, &node).winner("color"), Some("green"));
    }

    #[test]
    fn later_declaration_in_the_same_rule_wins() {
        let sheet = CompiledSheet::compile("button { color: red; color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(cascade(&sheet, &node).winner("color"), Some("green"));
    }

    #[test]
    fn important_beats_specificity() {
        let sheet = CompiledSheet::compile(
            "button { color: red !important }\n\
             window > button.suggested-action { color: blue }",
        );
        let node = button(&["suggested-action"], PseudoStates::default());
        assert_eq!(cascade(&sheet, &node).winner("color"), Some("red"));
    }

    #[test]
    fn non_matching_rules_contribute_nothing() {
        let sheet = CompiledSheet::compile("entry { color: red }\nbutton:hover { color: blue }");
        let node = button(&[], PseudoStates::default());
        assert!(cascade(&sheet, &node).is_empty());
    }

    #[test]
    fn adwaita_button_cascade_resolves_the_expected_declarations() {
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let normal = cascade(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(normal.winner("border-radius"), Some("5px"));
        // The `padding: 4px 9px` shorthand entered the cascade as longhands.
        assert_eq!(normal.winner("padding-top"), Some("4px"));
        assert_eq!(normal.winner("padding-right"), Some("9px"));
        assert_eq!(normal.winner("padding-bottom"), Some("4px"));
        assert_eq!(normal.winner("padding-left"), Some("9px"));
        assert_eq!(
            normal.winner("padding"),
            None,
            "the shorthand itself is gone"
        );
        assert_eq!(normal.winner("color"), Some("#2e3436"));
        assert_eq!(normal.winner("border-top-color"), Some("#cdc7c2"));
        assert_eq!(normal.winner("border-top-width"), Some("1px"));
        assert_eq!(normal.winner("border-top-style"), Some("solid"));
        assert_eq!(
            normal.winner("background-image"),
            Some("linear-gradient(to top, #f6f5f4 2px, #fbfafa)")
        );

        let hovered = cascade(
            &sheet,
            &button(
                &[],
                PseudoStates {
                    hover: true,
                    ..PseudoStates::default()
                },
            ),
        );
        assert_eq!(
            hovered.winner("background-image"),
            Some("linear-gradient(to top, #d6d1cd, #e8e6e3 1px)")
        );

        let active = cascade(
            &sheet,
            &button(
                &[],
                PseudoStates {
                    active: true,
                    ..PseudoStates::default()
                },
            ),
        );
        assert_eq!(active.winner("background-image"), Some("image(#dad6d2)"));

        let suggested = cascade(
            &sheet,
            &button(&["suggested-action"], PseudoStates::default()),
        );
        assert_eq!(suggested.winner("color"), Some("white"));
        assert_eq!(suggested.winner("border-top-color"), Some("#15539e"));
        assert_eq!(
            suggested.winner("background-image"),
            Some("linear-gradient(to top, #2c7fe3 2px, #3584e4)")
        );
    }

    #[test]
    fn compile_carries_the_color_table() {
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(
            sheet.colors.get("accent_color"),
            Some(&skia_rs_safe::core::Color(0xFF35_84E4))
        );
    }

    #[test]
    fn every_adwaita_rule_compiles() {
        // E6: the two functional pseudo-classes GTK uses (`:dir()`,
        // `:drop()`) used to fail selector parsing, dropping 62 rules.
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(sheet.rules.len(), 900);
    }

    #[test]
    fn runner_ups_are_kept_in_cascade_order() {
        // E3: the loser of a cascade used to be discarded outright, so an
        // uninterpretable winner left `computed` with nothing to fall back on.
        let sheet = CompiledSheet::compile(
            "button { color: red }\n             button.x { color: green }\n             button { color: blue !important }",
        );
        let values = cascade(&sheet, &button(&["x"], PseudoStates::default()));
        let candidates = values.candidates("color");
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["blue", "green", "red"],
            "candidates must be sorted best-first"
        );
        assert!(candidates[0].key.important);
        assert!(candidates[1].key.specificity > candidates[2].key.specificity);
        assert_eq!(values.winner("color"), Some("blue"));
        assert!(values.candidates("nonesuch").is_empty());
    }

    #[test]
    fn a_shorthand_carries_its_own_cascade_key_into_its_longhands() {
        // E1: `border` used to be applied before `border-width` regardless of
        // which one actually won the cascade.
        let sheet = CompiledSheet::compile(
            "button { border-width: 5px }\nbutton { border: 1px solid red }",
        );
        let values = cascade(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(values.winner("border-top-width"), Some("1px"));
        assert_eq!(
            values.candidates("border-top-width")[1].value,
            "5px",
            "the losing longhand is still available as a runner-up"
        );
    }
}
