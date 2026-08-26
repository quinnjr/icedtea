//! The cascade: which declarations a node actually gets.
//!
//! Origin is not modelled (M1 loads exactly one author sheet), so the sort
//! key is CSS's remainder: `!important` first, then selector specificity,
//! then source order. Later wins on a tie, which is what makes a theme's
//! own overrides work.

use std::collections::HashMap;

use selectors::SelectorList;

use super::colors::{ColorTable, build_color_table};
use super::parse::{Declaration, parse_stylesheet};
use super::select::{CssNode, GtkSelectorImpl, matches, parse_selector_list};

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
        let sheet = parse_stylesheet(css);
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

/// The declarations `node` wins, keyed by property name.
///
/// Specificity is taken per *selector*, not per rule: a rule whose prelude
/// is `notebook > header > tabs > arrow, button` contributes the specificity
/// of whichever of its selectors actually matched.
#[must_use]
pub fn cascade(sheet: &CompiledSheet, node: &CssNode) -> HashMap<String, String> {
    // (important, specificity, source order) -> value, per property.
    let mut winners: HashMap<String, (bool, u32, usize, String)> = HashMap::new();

    for rule in &sheet.rules {
        let Some(specificity) = rule
            .selectors
            .slice()
            .iter()
            .filter(|selector| matches(&SelectorList::from_one((*selector).clone()), node))
            .map(selectors::parser::Selector::specificity)
            .max()
        else {
            continue;
        };

        for decl in &rule.declarations {
            let key = (decl.important, specificity, rule.source_order);
            match winners.get(&decl.name) {
                // Strictly greater, not `>=`: an equal key can only come from
                // this same rule, and within one block the later declaration
                // wins.
                Some((imp, spec, order, _)) if (*imp, *spec, *order) > key => {}
                _ => {
                    winners.insert(decl.name.clone(), (key.0, key.1, key.2, decl.value.clone()));
                }
            }
        }
    }

    winners
        .into_iter()
        .map(|(name, (_, _, _, value))| (name, value))
        .collect()
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
        assert_eq!(
            cascade(&sheet, &node).get("color").map(String::as_str),
            Some("blue")
        );
    }

    #[test]
    fn equal_specificity_falls_back_to_source_order() {
        let sheet = CompiledSheet::compile("button { color: red }\nbutton { color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(
            cascade(&sheet, &node).get("color").map(String::as_str),
            Some("green")
        );
    }

    #[test]
    fn later_declaration_in_the_same_rule_wins() {
        let sheet = CompiledSheet::compile("button { color: red; color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(
            cascade(&sheet, &node).get("color").map(String::as_str),
            Some("green")
        );
    }

    #[test]
    fn important_beats_specificity() {
        let sheet = CompiledSheet::compile(
            "button { color: red !important }\n\
             window > button.suggested-action { color: blue }",
        );
        let node = button(&["suggested-action"], PseudoStates::default());
        assert_eq!(
            cascade(&sheet, &node).get("color").map(String::as_str),
            Some("red")
        );
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
        assert_eq!(normal.get("border-radius").map(String::as_str), Some("5px"));
        assert_eq!(normal.get("padding").map(String::as_str), Some("4px 9px"));
        assert_eq!(normal.get("color").map(String::as_str), Some("#2e3436"));
        assert_eq!(
            normal.get("border-color").map(String::as_str),
            Some("#cdc7c2")
        );
        assert_eq!(
            normal.get("background-image").map(String::as_str),
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
            hovered.get("background-image").map(String::as_str),
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
        assert_eq!(
            active.get("background-image").map(String::as_str),
            Some("image(#dad6d2)")
        );

        let suggested = cascade(
            &sheet,
            &button(&["suggested-action"], PseudoStates::default()),
        );
        assert_eq!(suggested.get("color").map(String::as_str), Some("white"));
        assert_eq!(
            suggested.get("border-color").map(String::as_str),
            Some("#15539e")
        );
        assert_eq!(
            suggested.get("background-image").map(String::as_str),
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
}
