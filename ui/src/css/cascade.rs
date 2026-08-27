//! The cascade: which declarations a node actually gets, and in what order.
//!
//! Origin is not modelled (M1 loads exactly one author sheet), so the sort
//! key is CSS's remainder: `!important` first, then selector specificity,
//! then source order, then position within the rule. Later wins on a tie,
//! which is what makes a theme's own overrides work.
//!
//! Two things make this more than a `HashMap<String, String>`:
//!
//! * **Shorthands are expanded here**, through their registry `ExpandFn`,
//!   carrying the shorthand's own key, so `border-width: 5px` and a later
//!   `border: 1px solid` are ordered by the cascade rather than by which
//!   property the consumer reads first. Every declaration is parsed by its
//!   registry `ParseFn` before it enters the cascade, so a declaration that
//!   is invalid at parse time is dropped exactly as CSS requires.
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

use cssparser::{Parser, ParserInput};
use selectors::SelectorList;

use super::node::Node;
use super::parse::{Declaration, KeyframesRule, MediaEnv, StyleRule, Stylesheet, parse_stylesheet};
use super::registry::{self, N_LONGHANDS, Prop};
use super::select::{
    GtkSelectorImpl, MatchCx, RuleBuckets, matches_with_specificity, parse_selector_list,
};
use super::value::color::{ColorTable, build_color_table};
use super::value::{Keyframes, Value};

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
    /// Which layer the rule came from; see [`CascadeKey::origin`].
    pub origin: u8,
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
    /// P1's registry-native `super::value::color::ColorTable`
    /// (`HashMap<String, ColorValue>`), resolved against a
    /// [`super::value::color::ColorCtx`] at computed-value time.
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
        // `(source_order, name, value)`, so a definition inside a matching
        // `@media` block is *spliced* at the block's own position rather
        // than appended after every top-level one -- otherwise
        // `@media (...) { @define-color bg black }` always beat a later
        // top-level `@define-color bg white`, where GTK's last-wins rule
        // gives the top-level one.
        let mut definitions: Vec<(usize, &String, &String)> = sheet
            .color_definitions
            .iter()
            .zip(sheet.color_definition_orders.iter().copied())
            .map(|((name, value), order)| (order, name, value))
            .collect();
        for block in &sheet.media_blocks {
            if !block.query.evaluate(env) {
                tracing::debug!(query = ?block.query, "media block does not match; dropping");
                continue;
            }
            style_rules.extend(block.rules.iter());
            keyframe_rules.extend(block.keyframes.iter());
            definitions.extend(
                block
                    .color_definitions
                    .iter()
                    .zip(block.color_definition_orders.iter().copied())
                    .map(|((name, value), order)| (order, name, value)),
            );
        }
        definitions.sort_by_key(|(order, _, _)| *order);
        let definitions: Vec<(String, String)> = definitions
            .into_iter()
            .map(|(_, name, value)| (name.clone(), value.clone()))
            .collect();
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
                    origin: rule.origin,
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

/// Parse one declaration into longhand `(Prop, Value)` pairs, expanding a
/// shorthand through its registry `ExpandFn`.
///
/// A declaration that is invalid at parse time is dropped and logged, exactly as
/// CSS requires -- it never reaches the cascade.
fn parse_declaration(prop: Prop, value: &str, sink: &mut dyn FnMut(Prop, Value)) {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);

    // A wide keyword is legal on every property, shorthands included, where it
    // sets each of the shorthand's longhands. `Value::parse_wide` already
    // requires the whole value to be the keyword.
    if let Ok(wide) = parser.try_parse(Value::parse_wide) {
        if prop.is_longhand() {
            sink(prop, wide);
        } else {
            for longhand in prop.longhands() {
                sink(*longhand, wide.clone());
            }
        }
        return;
    }

    if prop.is_longhand() {
        match registry::parse_declaration_value(prop, value) {
            Ok(parsed) => sink(prop, parsed),
            Err(()) => tracing::debug!(
                property = prop.name(),
                value,
                "declaration is invalid at parse time; dropping"
            ),
        }
        return;
    }

    let mut expanded: Vec<(Prop, Value)> = Vec::new();
    let outcome = prop.expand_into(&mut parser, &mut |longhand, value| {
        expanded.push((longhand, value));
    });
    if outcome.is_err() || parser.expect_exhausted().is_err() {
        tracing::debug!(
            property = prop.name(),
            value,
            "shorthand is invalid at parse time; dropping"
        );
        return;
    }
    for (longhand, value) in expanded {
        sink(longhand, value);
    }
}

/// Where a declaration sits in the cascade. Ordered worst-to-best, so
/// `max` is the winner and a descending sort puts the winner first.
///
/// Field order *is* the sort order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CascadeKey {
    /// Whether the declaration carried `!important`.
    pub important: bool,
    /// Which layer the declaration came from: 0 is the base sheet, and each
    /// [`super::parse::Stylesheet::append_layer`] adds one.
    ///
    /// This outranks specificity, which is the whole point. GTK loads the
    /// user's `gtk.css` at `GTK_STYLE_PROVIDER_PRIORITY_USER` (800) over the
    /// theme's 200, and a higher-priority provider wins *regardless* of
    /// specificity. Folding the layer into `source_order` -- which ranks
    /// below specificity -- meant a user `button { background-color: #f00 }`
    /// (0,0,1) lost to Adwaita's `button:hover` (0,1,1), so the override
    /// applied only in the base state and `!important` had no origin to
    /// reverse against.
    ///
    /// This is GTK's model, not CSS's: CSS ranks the user origin *below* the
    /// author's for normal declarations and reverses origin order for
    /// `!important`. GTK does neither, so `important` stays the outermost
    /// key and origins never reverse.
    pub origin: u8,
    /// The specificity of the selector that actually matched.
    pub specificity: u32,
    /// The rule's index in the sheet.
    pub source_order: usize,
    /// The declaration's index within its rule -- the last tiebreak, so the
    /// later of two declarations in one block wins.
    pub declaration_order: usize,
}

/// One longhand declaration that applied to a node, with its place in the
/// cascade and the shorthand (if any) that set it.
#[derive(Clone, Debug)]
pub struct CascadedDecl {
    /// The parsed longhand value.
    pub value: Value,
    /// Where it sits in the cascade.
    pub key: CascadeKey,
    /// The shorthand this longhand was expanded out of, if it was.
    pub from_shorthand: Option<Prop>,
}

/// Every declaration that applied to a node, per longhand, best first.
///
/// One slot per longhand, indexed by `Prop::slot()`: the registry's row order
/// *is* the storage order, so a lookup is an index, not a hash.
///
/// Runner-ups are kept for **diagnostics only**. M1 walked them when the winner
/// could not be interpreted; M2 follows CSS instead (contract §5, Decision 6):
/// an invalid winner yields the inherited or initial value.
#[derive(Clone, Debug)]
pub struct CascadedValues {
    by_prop: Box<[Vec<CascadedDecl>; N_LONGHANDS]>,
}

impl Default for CascadedValues {
    fn default() -> Self {
        Self::new()
    }
}

impl CascadedValues {
    /// An empty set: no declaration applied to any longhand.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_prop: Box::new(std::array::from_fn(|_| Vec::new())),
        }
    }

    /// Every declaration for `prop`, winner first.
    ///
    /// # Panics
    ///
    /// If `prop` is a shorthand: shorthands have no cascade slot of their own,
    /// they are expanded into their longhands before storage.
    #[must_use]
    pub fn candidates(&self, prop: Prop) -> &[CascadedDecl] {
        assert!(prop.is_longhand(), "{} is a shorthand", prop.name());
        &self.by_prop[prop.slot()]
    }

    /// The winning value for `prop`, ignoring runner-ups.
    #[must_use]
    pub fn winner(&self, prop: Prop) -> Option<&Value> {
        self.candidates(prop).first().map(|decl| &decl.value)
    }

    /// Whether no declaration applied at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_prop.iter().all(Vec::is_empty)
    }

    /// How many distinct longhands applied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_prop
            .iter()
            .filter(|candidates| !candidates.is_empty())
            .count()
    }

    fn push(&mut self, prop: Prop, decl: CascadedDecl) {
        debug_assert!(prop.is_longhand(), "{} is a shorthand", prop.name());
        self.by_prop[prop.slot()].push(decl);
    }

    fn sort(&mut self) {
        for candidates in self.by_prop.iter_mut() {
            candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.key));
        }
    }
}

/// The declarations `node` wins, keyed by longhand.
///
/// Specificity is taken per *selector*, not per rule: a rule whose prelude
/// is `notebook > header > tabs > arrow, button` contributes the specificity
/// of whichever of its selectors actually matched.
///
/// `cx` is caller-owned and reused across a whole restyle pass, which keeps the
/// bloom filter and the nth-index caches warm.
#[must_use]
pub fn cascade(sheet: &CompiledSheet, node: &Node, cx: &mut MatchCx) -> CascadedValues {
    cx.seed_for(node);
    let mut values = CascadedValues::new();
    for index in sheet.buckets.candidates(node) {
        collect_rule(&sheet.rules[index], node, cx, &mut values);
    }
    values.sort();
    values
}

/// The bucket-free form: scan every rule in the sheet. Kept as the reference
/// implementation the bucketed path is tested against.
#[cfg(test)]
fn cascade_every_rule(sheet: &CompiledSheet, node: &Node, cx: &mut MatchCx) -> CascadedValues {
    cx.seed_for(node);
    let mut values = CascadedValues::new();
    for rule in &sheet.rules {
        collect_rule(rule, node, cx, &mut values);
    }
    values.sort();
    values
}

/// Match one rule and, if it applies, push every declaration it carries.
fn collect_rule(rule: &CompiledRule, node: &Node, cx: &mut MatchCx, values: &mut CascadedValues) {
    let Some(specificity) = matches_with_specificity(&rule.selectors, node, cx) else {
        return;
    };
    for (declaration_order, declaration) in rule.declarations.iter().enumerate() {
        let key = CascadeKey {
            important: declaration.important,
            origin: rule.origin,
            specificity,
            source_order: rule.source_order,
            declaration_order,
        };
        let Some(prop) = registry::lookup(&declaration.name) else {
            tracing::debug!(property = %declaration.name, "unknown property; dropping declaration");
            continue;
        };
        let from_shorthand = (!prop.is_longhand()).then_some(prop);
        parse_declaration(prop, &declaration.value, &mut |longhand, value| {
            values.push(
                longhand,
                CascadedDecl {
                    value,
                    key,
                    from_shorthand,
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{CompiledSheet, cascade};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::color::ColorValue;
    use crate::css::value::{Length, Value};

    /// A `window.background > button` tree.
    ///
    /// A [`Node`]'s parent link is a `Weak`, so the fixture has to keep the
    /// window alive for as long as the button is used; it derefs to the button
    /// so every call site reads as if it were the bare node.
    struct ButtonTree {
        window: Node,
        button: Node,
    }

    impl std::ops::Deref for ButtonTree {
        type Target = Node;

        fn deref(&self) -> &Node {
            &self.button
        }
    }

    impl std::fmt::Debug for ButtonTree {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.button.fmt(f)
        }
    }

    /// A `window.background > button` tree, the shape every M1 test used.
    fn button(classes: &[&str], states: PseudoStates) -> ButtonTree {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        ButtonTree { window, button }
    }

    fn winner(sheet: &CompiledSheet, node: &Node, prop: Prop) -> Option<Value> {
        let mut cx = MatchCx::new();
        cascade(sheet, node, &mut cx).winner(prop).cloned()
    }

    fn hex(value: u32) -> Value {
        Value::Color(ColorValue::Absolute(
            crate::css::value::color::Rgba::from_color32(skia_rs_safe::core::Color(value)),
        ))
    }

    #[test]
    fn higher_specificity_wins_over_source_order() {
        let sheet = CompiledSheet::compile(
            "button.suggested-action { color: blue }\n\
             button { color: red }",
        );
        let node = button(&["suggested-action"], PseudoStates::default());
        assert_eq!(winner(&sheet, &node, Prop::Color), Some(hex(0xFF00_00FF)));
    }

    #[test]
    fn equal_specificity_falls_back_to_source_order() {
        let sheet = CompiledSheet::compile("button { color: red }\nbutton { color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(winner(&sheet, &node, Prop::Color), Some(hex(0xFF00_8000)));
    }

    #[test]
    fn later_declaration_in_the_same_rule_wins() {
        let sheet = CompiledSheet::compile("button { color: red; color: green }");
        let node = button(&[], PseudoStates::default());
        assert_eq!(winner(&sheet, &node, Prop::Color), Some(hex(0xFF00_8000)));
    }

    #[test]
    fn important_beats_specificity() {
        let sheet = CompiledSheet::compile(
            "button { color: red !important }\n\
             window > button.suggested-action { color: blue }",
        );
        let node = button(&["suggested-action"], PseudoStates::default());
        assert_eq!(winner(&sheet, &node, Prop::Color), Some(hex(0xFFFF_0000)));
    }

    #[test]
    fn non_matching_rules_contribute_nothing() {
        let sheet = CompiledSheet::compile("entry { color: red }\nbutton:hover { color: blue }");
        let node = button(&[], PseudoStates::default());
        let mut cx = MatchCx::new();
        assert!(cascade(&sheet, &node, &mut cx).is_empty());
    }

    #[test]
    fn adwaita_button_cascade_resolves_the_expected_declarations() {
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let mut cx = MatchCx::new();
        let node = button(&[], PseudoStates::default());
        let normal = cascade(&sheet, &node, &mut cx);
        assert_eq!(
            normal.winner(Prop::BorderTopLeftRadius),
            Some(&Value::Pair(std::rc::Rc::new((
                Value::Length(Length::px(5.0)),
                Value::Length(Length::px(5.0)),
            ))))
        );
        // The `padding: 4px 9px` shorthand entered the cascade as longhands.
        assert_eq!(
            normal.winner(Prop::PaddingTop),
            Some(&Value::Length(Length::px(4.0)))
        );
        assert_eq!(
            normal.winner(Prop::PaddingRight),
            Some(&Value::Length(Length::px(9.0)))
        );
        assert_eq!(
            normal.winner(Prop::PaddingBottom),
            Some(&Value::Length(Length::px(4.0)))
        );
        assert_eq!(
            normal.winner(Prop::PaddingLeft),
            Some(&Value::Length(Length::px(9.0)))
        );
        assert_eq!(normal.winner(Prop::Color), Some(&hex(0xFF2E_3436)));
        assert_eq!(normal.winner(Prop::BorderTopColor), Some(&hex(0xFFCD_C7C2)));
        assert_eq!(
            normal.winner(Prop::BorderTopWidth),
            Some(&Value::Length(Length::px(1.0)))
        );
        assert_eq!(
            normal.winner(Prop::BorderTopStyle),
            Some(&Value::Keyword(crate::css::value::Keyword::Solid))
        );
        assert!(
            matches!(normal.winner(Prop::BackgroundImage), Some(Value::List(_))),
            "background-image is comma-multiplied, so it cascades as a list"
        );

        let hovered = cascade(&sheet, &button(&[], PseudoStates::HOVER), &mut cx);
        assert_ne!(
            hovered.winner(Prop::BackgroundImage),
            normal.winner(Prop::BackgroundImage),
            "`button:hover` did not win the cascade"
        );

        let suggested = cascade(
            &sheet,
            &button(&["suggested-action"], PseudoStates::default()),
            &mut cx,
        );
        assert_eq!(suggested.winner(Prop::Color), Some(&hex(0xFFFF_FFFF)));
        assert_eq!(
            suggested.winner(Prop::BorderTopColor),
            Some(&hex(0xFF15_539E))
        );
    }

    #[test]
    fn compile_carries_the_color_table() {
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(
            sheet.colors.get("accent_color"),
            Some(&ColorValue::Absolute(
                crate::css::value::color::Rgba::from_color32(skia_rs_safe::core::Color(
                    0xFF35_84E4
                ))
            ))
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
        // E3: the loser of a cascade used to be discarded outright. M2 keeps
        // runner-ups for diagnostics only -- `computed` no longer walks them.
        let sheet = CompiledSheet::compile(
            "button { color: red }\nbutton.x { color: green }\nbutton { color: blue !important }",
        );
        let mut cx = MatchCx::new();
        let values = cascade(&sheet, &button(&["x"], PseudoStates::default()), &mut cx);
        let candidates = values.candidates(Prop::Color);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.value.clone())
                .collect::<Vec<_>>(),
            vec![hex(0xFF00_00FF), hex(0xFF00_8000), hex(0xFFFF_0000)],
            "candidates must be sorted best-first"
        );
        assert!(candidates[0].key.important);
        assert!(candidates[1].key.specificity > candidates[2].key.specificity);
        assert_eq!(values.winner(Prop::Color), Some(&hex(0xFF00_00FF)));
        assert!(values.candidates(Prop::Opacity).is_empty());
    }

    #[test]
    fn a_shorthand_carries_its_own_cascade_key_into_its_longhands() {
        // E1: `border` used to be applied before `border-width` regardless of
        // which one actually won the cascade.
        let sheet = CompiledSheet::compile(
            "button { border-width: 5px }\nbutton { border: 1px solid red }",
        );
        let mut cx = MatchCx::new();
        let values = cascade(&sheet, &button(&[], PseudoStates::default()), &mut cx);
        assert_eq!(
            values.winner(Prop::BorderTopWidth),
            Some(&Value::Length(Length::px(1.0)))
        );
        assert_eq!(
            values.candidates(Prop::BorderTopWidth)[0].from_shorthand,
            Some(Prop::Border),
            "the winner must record which shorthand set it"
        );
        assert_eq!(
            values.candidates(Prop::BorderTopWidth)[1].value,
            Value::Length(Length::px(5.0)),
            "the losing longhand is still available as a runner-up"
        );
        assert_eq!(
            values.candidates(Prop::BorderTopWidth)[1].from_shorthand,
            Some(Prop::BorderWidth),
            "and the runner-up records the shorthand that set *it*"
        );
    }

    #[test]
    fn a_declaration_that_is_invalid_at_parse_time_never_reaches_the_cascade() {
        // CSS drops an unparseable declaration at parse time; M1 kept the
        // string and let `computed` step past it.
        let sheet =
            CompiledSheet::compile("button { color: red }\nbutton { color: nosuchfunction(1, 2) }");
        let mut cx = MatchCx::new();
        let values = cascade(&sheet, &button(&[], PseudoStates::default()), &mut cx);
        assert_eq!(values.candidates(Prop::Color).len(), 1);
        assert_eq!(values.winner(Prop::Color), Some(&hex(0xFFFF_0000)));
    }

    #[test]
    fn a_wide_keyword_on_a_shorthand_sets_every_one_of_its_longhands() {
        let sheet = CompiledSheet::compile("button { padding: inherit }");
        let mut cx = MatchCx::new();
        let values = cascade(&sheet, &button(&[], PseudoStates::default()), &mut cx);
        for prop in [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ] {
            assert_eq!(
                values.winner(prop),
                Some(&Value::Wide(crate::css::value::Wide::Inherit)),
                "{} did not receive the shorthand's wide keyword",
                prop.name()
            );
        }
    }

    #[test]
    fn bucketed_matching_agrees_with_scanning_every_rule() {
        // The buckets are an optimisation, not a semantic: for every Adwaita
        // node shape, the bucketed cascade must equal a brute-force scan.
        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let mut cx = MatchCx::new();
        for classes in [
            &[][..],
            &["suggested-action"][..],
            &["flat"][..],
            &["image-button"][..],
            &["text-button"][..],
        ] {
            for states in [
                PseudoStates::default(),
                PseudoStates::HOVER,
                PseudoStates::ACTIVE,
            ] {
                let node = button(classes, states);
                let bucketed = cascade(&sheet, &node, &mut cx);
                let scanned = super::cascade_every_rule(&sheet, &node, &mut cx);
                for prop in crate::css::registry::longhands() {
                    assert_eq!(
                        bucketed.winner(prop),
                        scanned.winner(prop),
                        "bucketing changed the winner of {} for {classes:?} {states:?}",
                        prop.name()
                    );
                }
            }
        }
    }

    #[test]
    fn the_cascade_never_panics_on_hostile_declarations() {
        // Every registry ParseFn is reached through `cascade`, so this is the
        // crate-level never-panic battery for the whole property table.
        const HOSTILE: &[&str] = &[
            "button { color: }",
            "button { color: #zzz }",
            "button { color: rgb( }",
            "button { color: rgb(from) }",
            "button { padding: calc(1px + ) }",
            "button { padding: calc(((((1px)))) }",
            "button { margin: 1e40px }",
            "button { margin: NaNpx }",
            "button { min-width: infpx }",
            "button { border-radius: 1px / }",
            "button { border: 1px solid solid solid }",
            "button { background: url( }",
            "button { background-image: linear-gradient() }",
            "button { background-image: linear-gradient(to nowhere, red) }",
            "button { box-shadow: inset inset 1px }",
            "button { transition: 1s 2s 3s 4s }",
            "button { animation: none none none }",
            "button { transform: matrix(1) }",
            "button { filter: blur(-1px) blur() }",
            "button { font: 🙂 }",
            "button { font-family: \"unterminated }",
            "button { -gtk-icon-palette: success }",
            "button { -gtk-dpi: -0 }",
            "button { letter-spacing: 99999999999999999999999px }",
            "button { text-shadow: 1px 2px 3px 4px red }",
            "button { opacity: 1e999 }",
            "button { transition-property: 🙂 }",
            "button { border-image: fill }",
            "button { outline: 🙂 dashed }",
            "button { color: red !!important }",
        ];
        let mut cx = MatchCx::new();
        for css in HOSTILE {
            let sheet = CompiledSheet::compile(css);
            let node = button(&[], PseudoStates::default());
            let values = cascade(&sheet, &node, &mut cx);
            // Reading every slot exercises the whole table, winners included.
            for prop in crate::css::registry::longhands() {
                let _ = values.winner(prop);
                let _ = values.candidates(prop);
            }
        }
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
        let node = button(&[], PseudoStates::default());
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

    #[test]
    fn the_fixture_keeps_the_window_parent_alive() {
        // `Node::parent` is a `Weak`: a fixture that dropped the window would
        // silently turn every descendant selector in this module into a
        // non-match, and the tests would still pass for the wrong reason.
        let node = button(&[], PseudoStates::default());
        let parent = node.parent().expect("the window parent is still alive");
        assert!(parent.ptr_eq(&node.window));
        assert_eq!(&*parent.name(), "window");
    }
}

#[cfg(test)]
mod review_tests {
    use super::{CompiledSheet, cascade};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::parse::{ColorScheme, MediaEnv, parse_stylesheet};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::Value;
    use crate::css::value::color::{ColorCtx, ColorValue, Rgba};

    fn hovered_button() -> Node {
        let button = Node::new("button");
        button.set_states(PseudoStates::HOVER);
        button
    }

    fn winning_color(sheet: &CompiledSheet, node: &Node, prop: Prop) -> Option<Rgba> {
        let mut cx = MatchCx::new();
        let values = cascade(sheet, node, &mut cx);
        let ctx = ColorCtx {
            table: &sheet.colors,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        match values.winner(prop)? {
            Value::Color(color) => color.resolve(&ctx),
            _ => None,
        }
    }

    // F4: a user `gtk.css` layer is a higher cascade *origin*, which outranks
    // any specificity -- GTK's USER (800) over THEME (200) rule. Folding the
    // layer into `source_order`, which ranks below specificity, let a
    // higher-specificity theme rule keep winning.
    #[test]
    fn a_later_layer_beats_a_higher_specificity_rule_from_an_earlier_one() {
        let mut sheet = parse_stylesheet("button:hover { color: #00ff00 }");
        sheet.append_layer(parse_stylesheet("button { color: #ff0000 }"));
        let compiled = CompiledSheet::from_stylesheet(sheet);
        assert_eq!(
            winning_color(&compiled, &hovered_button(), Prop::Color),
            Some(Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0
            }),
            "the user layer's low-specificity rule must still win"
        );

        // Within one layer, specificity still decides.
        let one_layer =
            CompiledSheet::compile("button:hover { color: #00ff00 } button { color: #ff0000 }");
        assert_eq!(
            winning_color(&one_layer, &hovered_button(), Prop::Color),
            Some(Rgba {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0
            }),
            "specificity must still decide inside a layer"
        );
    }

    // `!important` stays the outermost key: it is not reversed by origin,
    // because GTK does not reverse provider priority for it either.
    #[test]
    fn important_still_outranks_a_later_layer() {
        let mut sheet = parse_stylesheet("button { color: #00ff00 !important }");
        sheet.append_layer(parse_stylesheet("button { color: #ff0000 }"));
        let compiled = CompiledSheet::from_stylesheet(sheet);
        assert_eq!(
            winning_color(&compiled, &Node::new("button"), Prop::Color),
            Some(Rgba {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0
            })
        );
    }

    // F3: a `@define-color` inside a matching `@media` block is spliced at
    // the block's own source position, so a *later* top-level definition of
    // the same name still wins, as it does in GTK.
    #[test]
    fn a_media_block_define_color_is_spliced_not_appended() {
        let css = "@media (prefers-color-scheme: dark) { @define-color bg #000000; }\n\
                   @define-color bg #ffffff;\n\
                   button { color: @bg }";
        let dark = MediaEnv {
            color_scheme: ColorScheme::Dark,
            ..MediaEnv::default()
        };
        let sheet = CompiledSheet::compile_with_env(&parse_stylesheet(css), &dark);
        assert_eq!(
            sheet.colors.get("bg"),
            Some(&ColorValue::Absolute(Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0
            })),
            "the later top-level definition must win"
        );

        // The other order still lets the media block win, which is the whole
        // reason the block's definitions are collected at all.
        let reversed = "@define-color bg #ffffff;\n\
                        @media (prefers-color-scheme: dark) { @define-color bg #000000; }";
        let sheet = CompiledSheet::compile_with_env(&parse_stylesheet(reversed), &dark);
        assert_eq!(
            sheet.colors.get("bg"),
            Some(&ColorValue::Absolute(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0
            }))
        );
    }
}
