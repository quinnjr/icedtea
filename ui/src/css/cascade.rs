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
use std::rc::Rc;

use selectors::SelectorList;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{MatchingContext, matches_selector};

use super::colors::{ColorTable, build_color_table};
use super::parse::{Declaration, KeyframesRule, MediaEnv, StyleRule, Stylesheet, parse_stylesheet};
use super::select::{CssNode, GtkSelectorImpl, RuleBuckets, parse_selector_list};
use super::shorthand;
use super::value::Keyframes;

/// A rule whose prelude has been compiled to a selector list.
#[derive(Debug)]
pub struct CompiledRule {
    /// The rule's selectors.
    pub selectors: SelectorList<GtkSelectorImpl>,
    /// The rule's declarations, still as source text: the cascade parses each
    /// one through its registry `ParseFn`, which needs the property to know
    /// the grammar.
    pub declarations: Vec<Declaration>,
    /// Index of this rule in the source sheet.
    pub source_order: usize,
}

/// A stylesheet ready for matching, under one [`MediaEnv`].
///
/// `@media` blocks are kept *unevaluated* in [`Stylesheet`] so one parse can be
/// compiled under several environments -- the coverage gate compiles Adwaita's
/// light, dark and high-contrast sheets from the same parse.
#[derive(Debug)]
pub struct CompiledSheet {
    /// Rules whose preludes parsed, in source order.
    pub rules: Vec<CompiledRule>,
    /// `@define-color` names, **unresolved**: a definition may reference a name
    /// defined later, so resolution is lazy and cycle-guarded.
    ///
    /// This is M1's `super::colors::ColorTable`
    /// (`HashMap<String, skia_rs_safe::core::Color>`), not P1's
    /// `super::value::color::ColorTable`. `computed.rs`'s M1 body (still the
    /// only consumer of this field -- Task 2 of this part rewrites it) is
    /// built entirely on the old table; switching the field's type here would
    /// break that file's compile, which is outside this task's scope. Task 2
    /// swaps this field onto the registry-native table when it rewrites
    /// `computed.rs` to match.
    pub colors: ColorTable,
    /// `@keyframes` by name; the last definition of a name wins.
    pub keyframes: HashMap<Rc<str>, Rc<Keyframes>>,
    /// Rules bucketed by the rightmost compound's id/class/name.
    pub buckets: RuleBuckets,
    /// The media environment these rules were selected under.
    pub env: MediaEnv,
}

impl CompiledSheet {
    /// Parse and compile `css` under the default media environment.
    #[must_use]
    pub fn compile(css: &str) -> Self {
        Self::from_stylesheet(parse_stylesheet(css))
    }

    /// Compile an already-parsed sheet -- the form
    /// [`super::parse::parse_stylesheet_with_base`] produces.
    #[must_use]
    pub fn from_stylesheet(sheet: Stylesheet) -> Self {
        Self::compile_with_env(&sheet, &MediaEnv::default())
    }

    /// Compile `sheet` under `env`, splicing every matching `@media` block's
    /// rules, keyframes and colours in at the block's own source position.
    ///
    /// Rules whose prelude does not parse are dropped with a debug log; a real
    /// theme's unparseable corners must not take the sheet with them.
    #[must_use]
    pub fn compile_with_env(sheet: &Stylesheet, env: &MediaEnv) -> Self {
        let mut style_rules: Vec<&StyleRule> = sheet.rules.iter().collect();
        let mut keyframe_rules: Vec<&KeyframesRule> = sheet.keyframes.iter().collect();
        let mut definitions: Vec<(String, String)> = sheet.color_definitions.clone();
        for block in &sheet.media_blocks {
            if !block.query.evaluate(env) {
                tracing::debug!(query = ?block.query, "media block does not match; dropping");
                continue;
            }
            style_rules.extend(block.rules.iter());
            keyframe_rules.extend(block.keyframes.iter());
            definitions.extend(block.color_definitions.iter().cloned());
        }
        // Splice, do not append: a media rule's source order is the position it
        // occupied in the outer sheet.
        style_rules.sort_by_key(|rule| rule.source_order);
        keyframe_rules.sort_by_key(|rule| rule.source_order);

        let mut rules = Vec::with_capacity(style_rules.len());
        for rule in style_rules {
            match parse_selector_list(&rule.selector_text) {
                Some(selectors) => rules.push(CompiledRule {
                    selectors,
                    declarations: rule.declarations.clone(),
                    source_order: rule.source_order,
                }),
                None => {
                    tracing::debug!(prelude = %rule.selector_text, "unparseable selector list; dropping rule");
                }
            }
        }

        // `css::value::keyframes::Keyframes::compile` already does exactly
        // what this step needs -- expand shorthands, parse every longhand
        // through the registry, lift `animation-timing-function` out into the
        // frame's own timing, drop what does not parse. Reimplementing it here
        // would only diverge from P1's copy.
        let mut keyframes: HashMap<Rc<str>, Rc<Keyframes>> = HashMap::new();
        for rule in keyframe_rules {
            let compiled = Keyframes::compile(rule);
            // Last definition wins, per CSS Animations L1.
            keyframes.insert(Rc::clone(&compiled.name), Rc::new(compiled));
        }

        let buckets = RuleBuckets::build(&rules);
        Self {
            rules,
            colors: build_color_table(&definitions),
            keyframes,
            buckets,
            env: *env,
        }
    }

    /// The `@keyframes` rule named `name`, if the sheet defines one.
    #[must_use]
    pub fn keyframes(&self, name: &str) -> Option<&Rc<Keyframes>> {
        self.keyframes.get(name)
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
    use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
    use crate::css::registry::Prop;
    use crate::css::select::{CssNode, PseudoStates};
    use crate::css::value::{Length, Value};

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

    #[test]
    fn a_matching_media_block_is_spliced_in_at_its_source_position() {
        // GTK 4.20+ media queries: the same parsed sheet compiles under
        // several envs, which is what lets the coverage gate compile
        // light/dark/hc from one parse.
        let css = "button { color: red }\n\
                   @media (prefers-color-scheme: dark) { button { color: blue } }\n\
                   button { border-width: 1px }";
        let parsed = crate::css::parse::parse_stylesheet(css);

        let light = CompiledSheet::compile_with_env(&parsed, &MediaEnv::default());
        assert_eq!(light.rules.len(), 2, "the dark block must be dropped");
        assert_eq!(light.env, MediaEnv::default());

        let dark = CompiledSheet::compile_with_env(
            &parsed,
            &MediaEnv {
                color_scheme: ColorScheme::Dark,
                contrast: Contrast::NoPreference,
            },
        );
        assert_eq!(dark.rules.len(), 3);
        // Spliced at its own source position, not appended at the end.
        assert!(
            dark.rules[0].source_order < dark.rules[1].source_order
                && dark.rules[1].source_order < dark.rules[2].source_order,
            "media rules were not spliced in source order: {:?}",
            dark.rules
                .iter()
                .map(|rule| rule.source_order)
                .collect::<Vec<_>>()
        );
        assert_eq!(dark.rules[1].declarations[0].value, "blue");
    }

    #[test]
    fn keyframes_are_compiled_to_longhands_and_the_last_definition_wins() {
        let sheet = CompiledSheet::compile(
            "@keyframes pulse { from { padding: 1px } to { padding: 2px } }\n\
             @keyframes pulse { from { opacity: 0 } to { opacity: 1 } }",
        );
        let frames = sheet.keyframes("pulse").expect("@keyframes pulse");
        assert_eq!(&*frames.name, "pulse");
        assert_eq!(frames.frames.len(), 2);
        assert_eq!(&*frames.frames[0].offsets, &[0.0]);
        assert_eq!(&*frames.frames[1].offsets, &[1.0]);
        assert_eq!(
            frames.frames[0].declarations.as_ref(),
            &[(Prop::Opacity, Value::Number(0.0))],
            "the later @keyframes of the same name must win outright"
        );
        assert!(sheet.keyframes("nosuch").is_none());
    }

    #[test]
    fn a_shorthand_inside_a_keyframe_is_expanded_to_longhands() {
        let sheet = CompiledSheet::compile("@keyframes grow { to { padding: 4px 9px } }");
        let frames = sheet.keyframes("grow").expect("@keyframes grow");
        let declared: Vec<Prop> = frames.frames[0]
            .declarations
            .iter()
            .map(|(prop, _)| *prop)
            .collect();
        assert_eq!(
            declared,
            vec![
                Prop::PaddingTop,
                Prop::PaddingRight,
                Prop::PaddingBottom,
                Prop::PaddingLeft
            ]
        );
        assert_eq!(
            frames.frames[0].declarations[1].1,
            Value::Length(Length::px(9.0))
        );
    }

    #[test]
    fn the_rule_buckets_are_built_from_the_compiled_rules() {
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(sheet.rules.len(), 900);
        // `RuleBuckets::candidates` matches against `css::node::Node`, not
        // this module's `CssNode` (M1's immutable, parent-only stand-in that
        // Part 3 deletes); the plan's own `button()` fixture builds a
        // `CssNode`, so a detached `Node` is built directly here instead. Its
        // id/classes/name are all `RuleBuckets::candidates` reads.
        let node = crate::css::node::Node::new("button");
        let candidates = sheet.buckets.candidates(&node);
        assert!(
            !candidates.is_empty(),
            "the buckets returned no candidate rules for a plain Adwaita button"
        );
        assert!(
            candidates.iter().all(|index| *index < sheet.rules.len()),
            "a bucket handed back an out-of-range rule index"
        );
        assert!(
            candidates.windows(2).all(|pair| pair[0] < pair[1]),
            "candidates must be in source order and deduped"
        );
    }
}
