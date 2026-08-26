# Pure-Rust GTK-themed UI — M2 Part 3: Registry-driven cascade, computed style, inheritance, M1 migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Date:** 2026-08-26
**Part:** 3 of 6 (`P3`) · **Branch:** `rebuild/pure-rust-gtk-m2` · **Crate:** `ui/` (`icedtea-ui`)

---

## Contract deviations

The Part 0 contract (`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`) is
binding; every signature below is copied from it verbatim. These eleven points are
where the contract is silent, self-contradictory, or factually wrong about the
existing code. Each is resolved here, in P3's favour, and the resolution is
normative for this plan.

1. **P3 must keep the tree green, so it necessarily edits three files the contract
   assigns to P4.** Deleting `ComputedStyle`'s ten scalar fields breaks
   `ui/src/layout.rs`, `ui/src/paint.rs` and `ui/tests/themed_button_offscreen.rs`,
   which P4 rewrites wholesale. P3 therefore makes *compile-preserving, number-preserving*
   mechanical edits to those three files (Task 9). P4 still replaces them; nothing
   P3 writes there is meant to survive P4.
2. **§11's P3 paragraph over-claims §10.3.** It lists "colors/shorthand/cascade/
   computed/select/button/app". By file ownership (§0) and by the P1/P2 gates in
   §11, the `colors.rs` and `shorthand.rs` rows belong to **P1** and the `select.rs`
   row belongs to **P2**. P3 owns exactly the `cascade.rs`, `computed.rs`,
   `widget/button.rs` and `app.rs` rows.
3. **§10.3's `app.rs` row says "4 of 11"; the real number is 8 of 11.** Eight tests
   call `button_style`/`headerbar_min_height` and read a `ComputedStyle` field
   (`min_height` ×5, `border_color` ×3, `padding` ×1). Only three
   (`an_empty_xdg_config_home_falls_back_to_home_dot_config`,
   `no_home_and_no_xdg_means_no_override_path`, `theme_dirs_are_searched_in_gtk_order`)
   are byte-identical. In particular `the_bundled_source_is_exactly_the_vendored_sheet`
   reads `border_color` and therefore **cannot** join §10.2's byte-identical gate;
   its `900` constant is preserved instead.
4. **`the_bundled_source_is_exactly_the_vendored_sheet` and
   `an_explicit_theme_file_is_a_whole_theme_not_an_override` change an asserted
   colour, by design.** M1's `border-color` had no initial value and defaulted to
   `Color(0x0000_0000)`. M2's registry initial for `border-*-color` is
   `currentColor` (contract §1.3), which resolves to the initial `color`,
   `Rgba(0,0,0,1)` = `Color(0xFF00_0000)`. The test's *meaning* ("Adwaita was not
   layered under the explicit file") is preserved by additionally asserting the
   colour is **not** `#cdc7c2`. Same change in `computed.rs`'s
   `defaults_apply_when_nothing_matches`.
5. **`Overrides`, `TransitionSpec` and `AnimationSpec` (§6, owned by P5) are
   *produced* by P3.** `ComputedStyle::with_overrides`, `transition_specs()` and
   `animation_specs()` are §5 (P3) but name §6 (P5) types. P3 creates
   `ui/src/anim/mod.rs` containing **only** those three types, verbatim from §6.
   P5 adds `clock.rs`, `transition.rs`, `keyframes.rs` and `AnimationState` beside
   them without touching P3's three types.
6. **`BackgroundLayer` (§8, owned by P4) is *produced* by P3.** It is defined in
   `css/computed.rs` next to `background_layers()`, its only producer; P4's
   `paint/mod.rs` re-exports it (`pub use crate::css::computed::BackgroundLayer;`)
   so the §8 signatures still read `BackgroundLayer`.
7. **`transition-property`'s parsed `Value` shape is undefined in §2.** No `Value`
   variant carries a property name. P3 rules that P1 produces
   `Value::Keyword(Keyword::All)` for `all`, `Value::Keyword(Keyword::None)` for
   `none`, and `Value::AnimationName(AnimationName::Named(name))` for a property
   name — the only string-carrying variant in the enum. `TransitionSpec::prop` is
   filled by `registry::lookup(name)`; an unknown name yields no spec.
8. **`ComputedStyle::initial(env)` seeds two rows from `env`.** §5 gives
   `ResolveEnv { dpi, root_font_size }` but §1.3 fixes `-gtk-dpi`'s initial at
   `Number(96.0)` and `font-size`'s at `Length(14px)`. P3 rules that
   `initial(env)` writes `Value::Number(env.dpi)` into `Prop::GtkDpi` and
   `Value::Length(Length::px(env.root_font_size))` into `Prop::FontSize`, so
   `ResolveEnv` actually reaches the root of an inheritance chain. With
   `ResolveEnv::default()` the two are `96.0`/`14.0` — exactly §1.3's numbers.
9. **Percentages survive computed-value time.** §5 says "`em`/`rem`/`%`/`-gtk-dpi`
   resolved here", but §5's own accessors take the basis (`padding(basis)`,
   `border_radii(w, h)`, `min_size(basis)`) and `LengthCtx::percent_basis` is
   documented as "`None` => percentages are invalid here". P3 rules: at computed
   time a value whose length tree contains a percentage is kept verbatim (it is
   *valid*, not invalid-at-computed-value-time); the accessors resolve it against
   the used basis. The one exception is `font-size`, whose basis (the parent's
   font size) is known at computed time and is resolved there.
10. **`Shadow::color: None` is resolved at computed time.** §2.6 documents `None`
    as "currentColor at used time", but §2.10 requires interpolators to see
    `ColorValue::Absolute` only. P3 resolves `None` to `Some(Absolute(current))`
    during `resolve`, so a shadow list is interpolable without a paint context.
11. **Entry conditions.** P3 assumes that at its first commit the crate compiles
    and `cargo test -p icedtea-ui` is green, with P1's registry/values/parse and
    P2's `css::node::Node` + `MatchCx` + `RuleBuckets` landed, `css::colors` and
    `css::shorthand` deleted, and every remaining `CssNode`/bool-`PseudoStates`
    call site in `src/wayland.rs`, `src/app.rs`, `src/widget/button.rs`,
    `src/paint.rs` and `ui/tests/themed_button_offscreen.rs` already mechanically
    moved to `Node`/bitflags by P2. If any of those still fails to compile,
    fix it inside the P3 task that first touches the file, without changing an
    asserted number.

---

**Goal:** Replace M1's string-keyed cascade and ten-scalar-field `ComputedStyle`
with a registry-driven cascade over `Prop` and a registry-indexed typed computed
table that implements CSS inheritance, wide keywords and invalid-at-computed-value-time,
then migrate every M1 consumer onto it with every pinned Adwaita number intact.

**Architecture:** `cascade.rs` compiles a `Stylesheet` under a `MediaEnv` into a
`CompiledSheet` (rules + rule buckets + colour table + `@keyframes`), matches it
against a `css::node::Node` through P2's `RuleBuckets`/`MatchCx`, expands
shorthands through their registry `ExpandFn`, parses each declaration with its
registry `ParseFn`, and stores every candidate — winner and runner-ups — in a
`CascadedValues` indexed by `Prop`. `computed.rs` folds that into
`ComputedStyle { values: Box<[Value; 95]> }` in a fixed order (`-gtk-dpi` →
`font-size` → `color` → the rest), applying the inheritance rule, the wide
keywords and Decision 6, then exposes typed accessors (`get::<T>(Prop)`, per-side
borders, per-corner radii, background layers, transition/animation specs) that
layout, paint, the button widget and the app wiring consume.

**Tech Stack:** Rust 2024, `cssparser` 0.37, `selectors` 0.40, `taffy` 0.14,
`skia-rs-safe` 0.4.0 (pure Rust), `wayland-client` 0.31, `tracing`.

**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
(§3 Registry/cascade/computed, §7 testing) — parent spec
`docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.
**Contract:** `docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§4 second
half, §5, §10 — binding).
**Research notes:** `.superpowers/m2-plan-notes/gtk-css-semantics.md`,
`.superpowers/m2-plan-notes/current-crate.md`.

## Global Constraints

- Crate versions are pinned and must not be bumped: `cssparser = "0.37"`,
  `selectors = "0.40"`, `taffy = "0.14"`, `skia-rs-safe = "0.4.0"`,
  `wayland-client = "0.31"`, `precomputed-hash = "0.1"`, `rustix = "1"`,
  `bitflags = "2"`, `fontconfig = "0.11"`. P1 owns every `ui/Cargo.toml` edit;
  **P3 adds no dependency and edits no manifest.**
- No `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk`, and no `smithay`. These are
  Wayland *clients*.
- `edition = 2024`, `rust-version = 1.94` (both inherited from the workspace).
- Nothing outside `css::registry` names a property string. P3 code uses `Prop::*`;
  the single permitted exception is `registry::lookup(name)` when a *value*
  (`transition-property: background-color`) names a property.
- Gates, run after every task before the commit:
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- Every commit message ends with the trailer:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **The M1 gate.** `ui/tests/themed_button_offscreen.rs` must stay green with
  every numeric constant byte-identical: height `34.0`; `y = height - 2` band
  `0xFFF6F5F4`; gutter column `bx = 4`; corner `(0,0)` transparent; border pixel
  `(cx, 0)` `0xFFCDC7C2`; hover gutter `0xFFE8E6E3`; active `0xFFDAD6D2`;
  suggested-action `#2c7fe3`→`#3584e4`, white text, border `0xFF15539E`, gutter
  within ±10/channel of `#3584e4`; empty button `36×34`; more than 20 dark label
  pixels. Only the *mechanics* of reaching those numbers may change (Task 9), and
  only because M1's `ComputedStyle` fields no longer exist. **A diff that changes
  a number in that file fails review.**
- The other pinned constants that must not move: **900** compiled Adwaita rules
  (`cascade.rs`, `app.rs`); 1941 lines / **37** `@define-color`s (`lib.rs`);
  Adwaita's base `button` values (`border-radius: 5px`, `padding: 4px 9px`,
  `border: 1px solid`, `border-color: #cdc7c2`, `color: #2e3436`,
  `min-height: 24px`, `min-width: 16px`,
  `background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa)`); layout's
  `80×34`, label `(10, 8)`, `36×34`, `220×50`, centred `label_y == 14.0`.
- Files P3 must not touch: `css/registry.rs`, `css/tokens.rs`, `css/value/**`,
  `css/parse.rs` (P1); `css/node.rs`, `css/select.rs` (P2); `src/text.rs`,
  `ui/tests/adwaita_coverage.rs`, `ui/tests/gtk4_property_reference.rs`,
  `ui/README.md` (P6); `src/shm.rs`, `ui/tests/layer_shell_screencopy.rs`
  (byte-identical gate, §10.2).
- Parts execute **in order 1 → 6 on one branch**, so P1's and P2's `Produces` are
  already on disk and may be consumed directly. P4/P5/P6 consume P3's `Produces`;
  never rename one after it ships.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/src/css/cascade.rs` | Modify (rewrite ~381 → ~520 lines) | `CompiledRule`, `CompiledSheet` (rules, colours, `@keyframes`, buckets, env), `@media` splicing, `CascadeKey`, `CascadedDecl`, `CascadedValues` keyed by `Prop`, `cascade()` |
| `ui/src/css/computed.rs` | Modify (rewrite ~858 → ~1,000 lines) | `ResolveEnv`, `FromValue`, `ComputedStyle` (registry-indexed table), inheritance + wide keywords + invalid-at-computed-value-time, unit/colour resolution, typed accessors, `BackgroundLayer` |
| `ui/src/anim/mod.rs` | Create (~90 lines) | `Overrides`, `TransitionSpec`, `AnimationSpec` — contract §6 verbatim, the three §6 types P3 produces (P5 adds the rest of `anim/`) |
| `ui/src/lib.rs` | Modify (line 8-15) | add `pub mod anim;` to the module list |
| `ui/src/widget/button.rs` | Modify (~292 → ~330 lines) | `Button` as behaviour over a `css::node::Node`, owning its `MatchCx` and `ResolveEnv`, reading typed accessors |
| `ui/src/app.rs` | Modify (tests only, lines 278-492) | test fixtures move to `Node` + typed accessors; production code unchanged |
| `ui/src/layout.rs` | Modify (lines 93-137, 170-279) | compile-preserving: read padding/border/min-size through accessors (P4 replaces this file) |
| `ui/src/paint.rs` | Modify (lines 22-139, 141-225) | compile-preserving: read style through accessors, sample the top background layer (P4 replaces this file) |
| `ui/tests/themed_button_offscreen.rs` | Modify (mechanical only) | the M1 pixel gate, re-expressed against `BackgroundLayer`/accessors with every number byte-identical |

---

## Task 1: `CompiledSheet` grows keyframes, buckets and a media environment

**Files:**
- Modify: `ui/src/css/cascade.rs:35-98` (imports, `CompiledRule`, `CompiledSheet`)
- Modify: `ui/src/css/cascade.rs:212-381` (test module: add three tests, keep the
  existing ones compiling)

**Interfaces:**
- Consumes (P1, `css::parse`): `Stylesheet { rules: Vec<StyleRule>, color_definitions:
  Vec<(String, String)>, keyframes: Vec<KeyframesRule>, media_blocks: Vec<MediaBlock> }`,
  `StyleRule { selector_text: String, declarations: Vec<Declaration>, source_order: usize }`,
  `Declaration { name: String, value: String, important: bool }`,
  `KeyframesRule { name: String, frames: Vec<(Vec<f32>, Vec<Declaration>)>, source_order: usize }`,
  `MediaBlock { query: MediaQuery, rules: Vec<StyleRule>, keyframes: Vec<KeyframesRule>, color_definitions: Vec<(String, String)> }`,
  `MediaEnv { color_scheme: ColorScheme, contrast: Contrast }` + `Default`,
  `MediaQuery::evaluate(&self, env: &MediaEnv) -> bool`,
  `parse_stylesheet(&str) -> Stylesheet`.
- Consumes (P1, `css::value`): `Keyframe { offsets: Rc<[f32]>, declarations: Rc<[(Prop, Value)]>, timing: Option<TimingFunction> }`,
  `Keyframes { name: Rc<str>, frames: Rc<[Keyframe]> }`, `Value`, `TimingFunction`.
- Consumes (P1, `css::registry`): `lookup(&str) -> Option<Prop>`, `Prop::is_longhand`,
  `Prop::expand_into`, `PropertyKind::Longhand { parse, .. }`, `Prop::def`.
- Consumes (P1, `css::value::color`): `ColorTable`, `build_color_table(&[(String, String)]) -> ColorTable`.
- Consumes (P2, `css::select`): `RuleBuckets::build(&[CompiledRule]) -> RuleBuckets`,
  `parse_selector_list(&str) -> Option<SelectorList<GtkSelectorImpl>>`.
- Produces:
  ```rust
  pub struct CompiledRule {
      pub selectors: SelectorList<GtkSelectorImpl>,
      pub declarations: Vec<Declaration>,
      pub source_order: usize,
  }
  pub struct CompiledSheet {
      pub rules: Vec<CompiledRule>,
      pub colors: ColorTable,
      pub keyframes: HashMap<Rc<str>, Rc<Keyframes>>,
      pub buckets: RuleBuckets,
      pub env: MediaEnv,
  }
  impl CompiledSheet {
      pub fn compile(css: &str) -> Self;
      pub fn from_stylesheet(sheet: Stylesheet) -> Self;
      pub fn compile_with_env(sheet: &Stylesheet, env: &MediaEnv) -> Self;
      pub fn keyframes(&self, name: &str) -> Option<&Rc<Keyframes>>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Append to `ui/src/css/cascade.rs`'s `mod tests` (after the existing tests, never
interleaved):

```rust
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
```

Add to the test module's imports:

```rust
    use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
    use crate::css::registry::Prop;
    use crate::css::value::{Length, Value};
```

Mutation checks:
- `a_matching_media_block_is_spliced_in_at_its_source_position`: appending media
  rules at the end instead of splicing (drop the `sort_by_key` on `source_order`)
  breaks the ordering assertion; evaluating the query unconditionally makes
  `light.rules.len()` 3.
- `keyframes_are_compiled_to_longhands_and_the_last_definition_wins`: using
  `entry().or_insert()` instead of `insert()` leaves the first `pulse` in place and
  the declaration assertion fails.
- `a_shorthand_inside_a_keyframe_is_expanded_to_longhands`: storing the raw
  declaration instead of calling `expand_into` yields an empty/one-entry `declared`.
- `the_rule_buckets_are_built_from_the_compiled_rules`: building the buckets from
  the *pre-compile* rule list (whose indices include the dropped, unparseable
  rules) trips the out-of-range assertion.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::cascade`
Expected: FAIL to compile with `error[E0599]: no function or associated item named
'compile_with_env' found for struct 'CompiledSheet'` and `error[E0609]: no field
'keyframes'/'buckets'/'env' on type 'CompiledSheet'`.

- [ ] **Step 3: Implement `CompiledSheet`'s new shape**

Replace `ui/src/css/cascade.rs:35-98` (the imports, `CompiledRule` and
`CompiledSheet`) with:

```rust
use std::collections::HashMap;
use std::rc::Rc;

use cssparser::{Parser, ParserInput};
use selectors::SelectorList;

use super::parse::{Declaration, KeyframesRule, MediaEnv, StyleRule, Stylesheet, parse_stylesheet};
use super::registry::{self, Prop, PropertyKind};
use super::select::{GtkSelectorImpl, RuleBuckets, parse_selector_list};
use super::value::color::{ColorTable, build_color_table};
use super::value::{Keyframe, Keyframes, TimingFunction, Value};

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

        let mut keyframes: HashMap<Rc<str>, Rc<Keyframes>> = HashMap::new();
        for rule in keyframe_rules {
            let compiled = compile_keyframes(rule);
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

/// Compile one raw `@keyframes` rule: expand every shorthand, parse every value,
/// and lift `animation-timing-function` out into the frame's own timing.
fn compile_keyframes(rule: &KeyframesRule) -> Keyframes {
    let name: Rc<str> = Rc::from(rule.name.as_str());
    let mut frames = Vec::with_capacity(rule.frames.len());
    for (offsets, declarations) in &rule.frames {
        let mut longhands: Vec<(Prop, Value)> = Vec::with_capacity(declarations.len());
        let mut timing: Option<TimingFunction> = None;
        for declaration in declarations {
            let Some(prop) = registry::lookup(&declaration.name) else {
                tracing::debug!(property = %declaration.name, "unknown property in @keyframes");
                continue;
            };
            if prop == Prop::AnimationTimingFunction {
                let mut input = ParserInput::new(&declaration.value);
                let mut parser = Parser::new(&mut input);
                if let Ok(parsed) = TimingFunction::parse(&mut parser)
                    && parser.expect_exhausted().is_ok()
                {
                    timing = Some(parsed);
                }
                continue;
            }
            parse_declaration(prop, &declaration.value, &mut |prop, value| {
                longhands.push((prop, value));
            });
        }
        frames.push(Keyframe {
            offsets: Rc::from(offsets.as_slice()),
            declarations: Rc::from(longhands),
            timing,
        });
    }
    Keyframes {
        name,
        frames: Rc::from(frames),
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
    // sets each of the shorthand's longhands.
    if let Ok(wide) = parser.try_parse(Value::parse_wide)
        && parser.expect_exhausted().is_ok()
    {
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
        let PropertyKind::Longhand { parse, .. } = prop.def().kind else {
            unreachable!("is_longhand() and PropertyKind disagree for {}", prop.name());
        };
        match parse(&mut parser) {
            Ok(parsed) if parser.expect_exhausted().is_ok() => sink(prop, parsed),
            _ => tracing::debug!(
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
```

Leave `CascadeKey`, `CascadedDecl`, `CascadedValues` and `cascade` exactly as they
are for now; Task 2 rewrites them. Delete the now-unused
`use super::colors::{...}` line if P1 has not already.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::cascade`
Expected: PASS, including the pre-existing `every_adwaita_rule_compiles`
(`== 900`) and `compile_carries_the_color_table`.

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 6: Commit**

```bash
git add ui/src/css/cascade.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): compile @media and @keyframes into CompiledSheet

CompiledSheet now carries the sheet's @keyframes (compiled to longhand
(Prop, Value) pairs), P2's RuleBuckets, and the MediaEnv it was selected
under. compile_with_env splices a matching @media block's rules, keyframes
and colours in at the block's own source position, so one parse serves the
light, dark and high-contrast compiles the M2 coverage gate needs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `CascadedValues` keyed by `Prop`, cascade through the registry

**Files:**
- Modify: `ui/src/css/cascade.rs:100-210` (`CascadeKey`, `CascadedDecl`,
  `CascadedValues`, `cascade`)
- Modify: `ui/src/css/cascade.rs` test module (rewrite the 10 M1 tests, add 3)
- Modify: `ui/src/css/computed.rs:13-19, 180-443` (imports and the property
  readers: they now read `&Value`, not `&str`)

**Interfaces:**
- Consumes (Task 1): `CompiledSheet { rules, colors, keyframes, buckets, env }`,
  `parse_declaration(prop, value, sink)`.
- Consumes (P1, `css::registry`): `Prop`, `N_LONGHANDS = 95`, `Prop::slot`,
  `Prop::is_longhand`, `Prop::longhands`, `Prop::name`, `registry::lookup`.
- Consumes (P1, `css::value`): `Value`, `Value::parse_wide`, `Length`, `LengthCtx`,
  `Length::resolve`, `ColorValue`, `ColorCtx`, `Rgba`, `Rgba::to_color32`,
  `Image`, `Gradient`, `GradientKind`, `LinearDirection`, `SideOrCorner`,
  `ColorStop`, `Keyword`.
- Consumes (P2, `css::node`/`css::select`): `Node`, `MatchCx`, `MatchCx::new`,
  `MatchCx::seed_for`, `RuleBuckets::candidates`,
  `matches_with_specificity(&SelectorList<GtkSelectorImpl>, &Node, &mut MatchCx) -> Option<u32>`.
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
  pub struct CascadeKey {
      pub important: bool,
      pub specificity: u32,
      pub source_order: usize,
      pub declaration_order: usize,
  }
  #[derive(Clone, Debug)]
  pub struct CascadedDecl {
      pub value: Value,
      pub key: CascadeKey,
      pub from_shorthand: Option<Prop>,
  }
  pub struct CascadedValues { /* Box<[Vec<CascadedDecl>; N_LONGHANDS]> */ }
  impl CascadedValues {
      pub fn new() -> Self;
      pub fn candidates(&self, prop: Prop) -> &[CascadedDecl];
      pub fn winner(&self, prop: Prop) -> Option<&Value>;
      pub fn is_empty(&self) -> bool;
      pub fn len(&self) -> usize;
  }
  pub fn cascade(sheet: &CompiledSheet, node: &Node, cx: &mut MatchCx) -> CascadedValues;
  ```

- [ ] **Step 1: Write the failing tests**

Replace `ui/src/css/cascade.rs`'s whole `mod tests` with:

```rust
#[cfg(test)]
mod tests {
    use super::{CompiledSheet, cascade};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::color::ColorValue;
    use crate::css::value::{Length, Value};

    /// A `window.background > button` tree, the shape every M1 test used.
    fn button(classes: &[&str], states: PseudoStates) -> Node {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        button
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
        assert_eq!(
            normal.winner(Prop::BorderTopColor),
            Some(&hex(0xFFCD_C7C2))
        );
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

        let hovered = cascade(
            &sheet,
            &button(&[], PseudoStates::HOVER),
            &mut cx,
        );
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
            None
        );
    }

    #[test]
    fn a_declaration_that_is_invalid_at_parse_time_never_reaches_the_cascade() {
        // CSS drops an unparseable declaration at parse time; M1 kept the
        // string and let `computed` step past it.
        let sheet = CompiledSheet::compile(
            "button { color: red }\nbutton { color: nosuchfunction(1, 2) }",
        );
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
            for states in [PseudoStates::default(), PseudoStates::HOVER, PseudoStates::ACTIVE] {
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
}
```

Mutation checks:
- `higher_specificity_wins_over_source_order` / `equal_specificity_falls_back_to_source_order`
  / `later_declaration_in_the_same_rule_wins` / `important_beats_specificity`:
  dropping any field from `CascadeKey`'s `Ord` derive (or sorting ascending)
  flips exactly one of the four.
- `adwaita_button_cascade_resolves_the_expected_declarations`: not calling
  `expand_into` leaves every `Padding*` winner `None`.
- `runner_ups_are_kept_in_cascade_order`: keeping only the winner empties
  `candidates` past index 0.
- `a_shorthand_carries_its_own_cascade_key_into_its_longhands`: giving the
  expanded longhands a fresh key (or `declaration_order` of their own) makes
  `border-width: 5px` win.
- `a_declaration_that_is_invalid_at_parse_time_never_reaches_the_cascade`:
  pushing the declaration before checking `parse`'s result makes `len()` 2.
- `a_wide_keyword_on_a_shorthand_sets_every_one_of_its_longhands`: routing wide
  keywords through `expand_into` (which cannot parse `inherit`) drops all four.
- `bucketed_matching_agrees_with_scanning_every_rule`: any bucket that forgets a
  compound (e.g. omitting the universal bucket) changes a winner.
- `the_cascade_never_panics_on_hostile_declarations`: a `.unwrap()`/slicing panic
  anywhere in a `ParseFn` or in `cascade` aborts the run.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::cascade`
Expected: FAIL to compile with `error[E0308]: mismatched types` on
`winner(Prop::Color)` (M1's `winner` takes `&str` and returns `Option<&str>`) and
`error[E0425]: cannot find function 'cascade_every_rule'`.

- [ ] **Step 3: Implement the `Prop`-keyed cascade**

Replace `ui/src/css/cascade.rs:100-210` with:

```rust
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
fn collect_rule(
    rule: &CompiledRule,
    node: &Node,
    cx: &mut MatchCx,
    values: &mut CascadedValues,
) {
    let Some(specificity) = matches_with_specificity(&rule.selectors, node, cx) else {
        return;
    };
    for (declaration_order, declaration) in rule.declarations.iter().enumerate() {
        let key = CascadeKey {
            important: declaration.important,
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
```

Extend the file's imports (Task 1's block) with:

```rust
use super::node::Node;
use super::registry::N_LONGHANDS;
use super::select::{MatchCx, matches_with_specificity};
```

- [ ] **Step 4: Port `computed.rs`'s property readers to `Value`**

`computed.rs` still owns M1's ten scalar fields and its 22 tests -- they are the
regression bar while the cascade changes underneath. Only the readers change:
they now take `&Value` instead of `&str`. Replace `computed.rs:180-265` (the
`parse_px` / `parse_border_width` / `parse_stop` / `parse_background_image` block)
with the following, and delete the `use cssparser::{Parser, ParserInput, Token};`
and `use super::value::{comma_groups, component_values};` imports:

```rust
/// The M1 bridge: read a computed-shaped scalar out of a parsed `Value`.
///
/// Task 3 replaces every one of these with the registry-indexed table and
/// deletes this module. It exists so the cascade can move to `Prop` in one
/// commit while `ComputedStyle`'s M1 fields -- and all 22 of its tests -- stay
/// exactly as they are, which is what makes them a regression bar rather than a
/// thing to be rewritten on trust.
mod m1_bridge {
    use super::{Background, BackgroundClip, GradientStop};
    use crate::css::value::color::{ColorCtx, ColorTable, ColorValue, Rgba};
    use crate::css::value::{
        Gradient, GradientKind, Image, Keyword, Length, LengthCtx, LinearDirection, SideOrCorner,
        Value,
    };
    use skia_rs_safe::core::Color;

    /// A length context with M1's fixed assumptions: 14px font, 96 dpi, and no
    /// percentage basis unless the caller supplies one.
    pub fn ctx(font_size_px: f32, percent_basis: Option<f32>) -> LengthCtx {
        LengthCtx {
            font_size_px,
            root_font_size_px: super::ComputedStyle::DEFAULT_FONT_SIZE,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis,
        }
    }

    /// A `<length>` in px. A percentage with no basis is uninterpretable, which
    /// is what keeps `border-radius: 100%` falling through to its runner-up.
    pub fn px(value: &Value, ctx: &LengthCtx) -> Option<f32> {
        match value {
            Value::Length(length) => length.resolve(ctx).filter(|px| px.is_finite()),
            Value::Number(number) if *number == 0.0 => Some(0.0),
            _ => None,
        }
    }

    /// The horizontal half of a corner radius.
    pub fn radius(value: &Value, ctx: &LengthCtx) -> Option<f32> {
        match value {
            Value::Pair(pair) => px(&pair.0, ctx),
            other => px(other, ctx),
        }
    }

    /// A `<color>`, with `currentColor` mapped onto `current`.
    pub fn color(value: &Value, colors: &ColorTable, current: Color) -> Option<Color> {
        let Value::Color(color) = value else {
            return None;
        };
        let ctx = ColorCtx {
            table: colors,
            current: Rgba::from_color32(current),
            depth: 0,
        };
        color.resolve(&ctx).map(Rgba::to_color32)
    }

    /// `true` when this border style forces a used width of zero.
    pub fn border_style_is_none(value: &Value) -> Option<bool> {
        match value {
            Value::Keyword(Keyword::None | Keyword::Hidden) => Some(true),
            Value::Keyword(_) => Some(false),
            _ => None,
        }
    }

    /// A `background-clip` keyword. The property is comma-multiplied; M1 paints
    /// one layer, so the first entry wins.
    pub fn clip(value: &Value) -> Option<BackgroundClip> {
        match first(value) {
            Value::Keyword(Keyword::BorderBox) => Some(BackgroundClip::BorderBox),
            Value::Keyword(Keyword::PaddingBox) => Some(BackgroundClip::PaddingBox),
            Value::Keyword(Keyword::ContentBox) => Some(BackgroundClip::ContentBox),
            _ => None,
        }
    }

    /// The first entry of a comma-multiplied value, or the value itself.
    fn first(value: &Value) -> &Value {
        match value {
            Value::List(items) => items.first().unwrap_or(value),
            other => other,
        }
    }

    /// `background-image` as an M1 [`Background`].
    ///
    /// `Some(None)` means "interpretable, but paints nothing" (`none`), which
    /// must not fall through to a runner-up image.
    pub fn background(
        value: &Value,
        colors: &ColorTable,
        current: Color,
        ctx: &LengthCtx,
    ) -> Option<Option<Background>> {
        let Value::Image(image) = first(value) else {
            return None;
        };
        match image {
            Image::None => Some(None),
            Image::Solid(fill) => {
                Some(Some(Background::Solid(resolve(fill, colors, current)?)))
            }
            Image::Gradient(gradient) => {
                Some(Some(to_top(gradient, colors, current, ctx)?))
            }
            _ => None,
        }
    }

    fn resolve(value: &ColorValue, colors: &ColorTable, current: Color) -> Option<Color> {
        let ctx = ColorCtx {
            table: colors,
            current: Rgba::from_color32(current),
            depth: 0,
        };
        value.resolve(&ctx).map(Rgba::to_color32)
    }

    /// A two-stop, non-repeating `linear-gradient(to top, ...)` -- the only
    /// gradient shape M1's `Background` can hold.
    fn to_top(
        gradient: &Gradient,
        colors: &ColorTable,
        current: Color,
        ctx: &LengthCtx,
    ) -> Option<Background> {
        if gradient.repeating {
            return None;
        }
        let GradientKind::Linear {
            direction: LinearDirection::Side(SideOrCorner::Top),
        } = gradient.kind
        else {
            return None;
        };
        let [from, to] = gradient.stops.as_ref() else {
            return None;
        };
        let stop = |stop: &crate::css::value::ColorStop| -> Option<GradientStop> {
            Some(GradientStop {
                color: resolve(&stop.color, colors, current)?,
                position_px: match &stop.position {
                    Some(Length::Abs { .. } | Length::Calc(_)) => {
                        Some(stop.position.as_ref()?.resolve(ctx)?)
                    }
                    Some(_) => return None,
                    None => None,
                },
            })
        };
        Some(Background::LinearGradientToTop {
            from: stop(from)?,
            to: stop(to)?,
        })
    }
}
```

Then rewrite `from_declarations`'s body (`computed.rs:310-400`) to read through
the bridge. Every M1 rule -- the runner-up walk, the clamps, the ordering -- is
preserved verbatim; only the reader types change:

```rust
    #[must_use]
    pub fn from_declarations(
        values: &CascadedValues,
        colors: &ColorTable,
        parent: Option<&Self>,
    ) -> Self {
        let mut style = Self::default();

        let inherited_color = parent.map_or(Color::BLACK, |parent| parent.color);
        let parent_font_size =
            parent.map_or(Self::DEFAULT_FONT_SIZE, |parent| parent.font_size);
        let font_ctx = m1_bridge::ctx(parent_font_size, Some(parent_font_size));

        style.color = pick(values, Prop::Color, |value| {
            m1_bridge::color(value, colors, inherited_color)
        })
        .unwrap_or(inherited_color);
        style.font_size = pick(values, Prop::FontSize, |value| {
            m1_bridge::px(value, &font_ctx)
        })
        .map(|size| size.max(0.0))
        .unwrap_or(parent_font_size);

        let current = style.color;
        let ctx = m1_bridge::ctx(style.font_size, None);

        if let Some(color) = pick(values, Prop::BackgroundColor, |value| {
            m1_bridge::color(value, colors, current)
        }) {
            style.background = Background::Solid(color);
        }
        if let Some(background) = pick(values, Prop::BackgroundImage, |value| {
            m1_bridge::background(value, colors, current, &ctx)
        })
        .flatten()
        {
            style.background = background;
        }

        // Borders are cascaded per side; M1 paints a uniform border, so the top
        // side stands for all four. Task 3 widens `ComputedStyle` to four sides.
        if let Some(width) = pick(values, Prop::BorderTopWidth, |value| {
            m1_bridge::px(value, &ctx)
        }) {
            style.border_width = width.max(0.0);
        }
        if pick(values, Prop::BorderTopStyle, m1_bridge::border_style_is_none) == Some(true) {
            style.border_width = 0.0;
        }
        if let Some(color) = pick(values, Prop::BorderTopColor, |value| {
            m1_bridge::color(value, colors, current)
        }) {
            style.border_color = color;
        }
        if let Some(radius) = pick(values, Prop::BorderTopLeftRadius, |value| {
            m1_bridge::radius(value, &ctx)
        }) {
            style.border_radius = radius.max(0.0);
        }
        for (index, prop) in [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(padding) = pick(values, prop, |value| m1_bridge::px(value, &ctx)) {
                style.padding[index] = padding.max(0.0);
            }
        }
        if let Some(clip) = pick(values, Prop::BackgroundClip, m1_bridge::clip) {
            style.background_clip = clip;
        }
        if let Some(min_width) = pick(values, Prop::MinWidth, |value| m1_bridge::px(value, &ctx)) {
            style.min_width = min_width.max(0.0);
        }
        if let Some(min_height) = pick(values, Prop::MinHeight, |value| m1_bridge::px(value, &ctx))
        {
            style.min_height = min_height.max(0.0);
        }
        style
    }
```

and retype `pick` and the two `resolve` entry points:

```rust
fn pick<T>(
    values: &CascadedValues,
    prop: Prop,
    mut parse: impl FnMut(&Value) -> Option<T>,
) -> Option<T> {
    for (rank, candidate) in values.candidates(prop).iter().enumerate() {
        if let Some(parsed) = parse(&candidate.value) {
            return Some(parsed);
        }
        tracing::debug!(
            property = prop.name(),
            value = ?candidate.value,
            rank,
            "uninterpretable declaration; falling back to the next in cascade order"
        );
    }
    None
}
```

```rust
    #[must_use]
    pub fn resolve(sheet: &CompiledSheet, node: &Node) -> Self {
        let mut cx = MatchCx::new();
        let mut chain = vec![node.clone()];
        while let Some(parent) = chain.last().and_then(Node::parent) {
            chain.push(parent);
        }
        let mut style: Option<Self> = None;
        for ancestor in chain.iter().rev() {
            style = Some(Self::from_declarations(
                &cascade(sheet, ancestor, &mut cx),
                &sheet.colors,
                style.as_ref(),
            ));
        }
        style.unwrap_or_default()
    }

    #[must_use]
    pub fn resolve_with_parent(sheet: &CompiledSheet, node: &Node, parent: Option<&Self>) -> Self {
        let mut cx = MatchCx::new();
        Self::from_declarations(&cascade(sheet, node, &mut cx), &sheet.colors, parent)
    }
```

Finally add one bridge-fidelity test to `computed.rs`'s test module, because every
M1 byte assertion now round-trips through `Rgba`:

```rust
    #[test]
    fn adwaita_hex_bytes_round_trip_through_rgba() {
        // The M1 tests pin exact bytes; M2 resolves colours as f32 `Rgba` and
        // converts back. If that round trip were lossy, every pinned byte in
        // this file and in the offscreen gate would drift by one.
        use crate::css::value::color::Rgba;
        for declared in [
            0xFF2E_3436u32,
            0xFFCD_C7C2,
            0xFFF6_F5F4,
            0xFFFB_FAFA,
            0xFFD6_D1CD,
            0xFFE8_E6E3,
            0xFFDA_D6D2,
            0xFF2C_7FE3,
            0xFF35_84E4,
            0xFF15_539E,
            0xFF19_61B9,
        ] {
            let color = Color(declared);
            assert_eq!(Rgba::from_color32(color).to_color32(), color);
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::`
Expected: PASS. All 22 `computed.rs` tests -- including
`an_uninterpretable_winner_falls_back_to_the_runner_up` (`border-radius: 100%`
still has no percentage basis in the bridge, so it still yields to the base
rule's `5px`) -- are unchanged and green.

- [ ] **Step 6: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean, `ui/tests/themed_button_offscreen.rs` included.

- [ ] **Step 7: Commit**

```bash
git add ui/src/css/cascade.rs ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): cascade through the property registry, keyed by Prop

CascadedValues is now one slot per longhand indexed by Prop::slot(), and
every declaration is parsed by its registry ParseFn (shorthands through
their ExpandFn, wide keywords ahead of both) before it enters the cascade,
so an invalid-at-parse-time declaration is dropped exactly as CSS requires.
Matching goes through P2's RuleBuckets and one caller-owned MatchCx.

computed.rs keeps M1's ten scalar fields and all 22 of its tests as the
regression bar; only its readers move from &str to &Value, through a
short-lived m1_bridge module the next commit deletes.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: the registry-indexed computed table, inheritance, wide keywords, Decision 6

**Files:**
- Modify: `ui/src/css/computed.rs:130-443` (`ComputedStyle`, `resolve`,
  `from_declarations` → the table)
- Modify: `ui/src/css/computed.rs` test module (2 tests invert, 9 tests added)
- Modify: `ui/src/layout.rs:170-200` (test fixtures build from CSS, not from a
  struct literal with private fields)
- Modify: `ui/src/paint.rs:152-167`, `ui/src/widget/button.rs:80-95`,
  `ui/src/app.rs:291-301` (call sites of the two `resolve` entry points)

**Interfaces:**
- Consumes (Task 2): `CascadedValues`, `CascadedValues::winner/candidates`,
  `cascade(&CompiledSheet, &Node, &mut MatchCx) -> CascadedValues`, `CompiledSheet`.
- Consumes (P1): `Prop`, `N_LONGHANDS`, `registry::longhands()`, `Prop::initial`,
  `Prop::is_inherited`, `Prop::slot`, `Prop::name`; `Value`, `Wide`, `Keyword`,
  `Length`, `LengthUnit`, `LengthCtx`, `CalcNode`, `ColorValue`, `ColorCtx`,
  `Rgba`, `Image`, `Gradient`, `ColorStop`, `Position`, `BgSize`, `Shadow`,
  `TransformFn`, `FilterFn`, `LineHeight`, `FontFamily`, `FontWeight`,
  `FontStyle`, `TimingFunction`, `Time`, `AnimationName`, `IterationCount`.
- Produces:
  ```rust
  #[derive(Copy, Clone, Debug)]
  pub struct ResolveEnv { pub dpi: f32, pub root_font_size: f32 }
  impl Default for ResolveEnv { /* dpi 96.0, root_font_size 14.0 */ }

  pub trait FromValue: Sized { fn from_value(v: &Value) -> Self; }

  #[derive(Clone, Debug, PartialEq)]
  pub struct ComputedStyle { /* values: Box<[Value; N_LONGHANDS]> + M1 fields */ }
  impl ComputedStyle {
      pub fn initial(env: &ResolveEnv) -> Rc<ComputedStyle>;
      pub fn raw(&self, prop: Prop) -> &Value;
      pub fn get<T: FromValue>(&self, prop: Prop) -> T;
      pub fn resolve(sheet: &CompiledSheet, node: &Node, parent: Option<&ComputedStyle>,
                     env: &ResolveEnv, cx: &mut MatchCx) -> ComputedStyle;
      pub fn resolve_chain(sheet: &CompiledSheet, node: &Node, env: &ResolveEnv,
                           cx: &mut MatchCx) -> ComputedStyle;
  }
  ```

- [ ] **Step 1: Write the failing tests**

In `ui/src/css/computed.rs`'s test module, replace the two tests that invert and
append the nine new ones. First, the two inversions.

`an_uninterpretable_winner_falls_back_to_the_runner_up` becomes:

```rust
    #[test]
    fn an_invalid_winner_yields_the_initial_or_inherited_value() {
        // Decision 6 replaces M1's runner-up rule. Two halves:
        //
        // 1. Adwaita:1606 `button.sidebar-button { border-radius: 100% }` is
        //    *valid* now -- percentages are a first-class radius -- so the
        //    winner is kept as a percentage and resolved against the box at
        //    used-value time, not swapped for the base rule's 5px.
        // 2. A genuinely invalid winner (an unknown @name has nothing to
        //    resolve against) takes the initial value for a non-inherited
        //    property and the inherited value for an inherited one -- never
        //    the runner-up.
        let sheet = adwaita();
        let style = resolve(&sheet, &button(&["sidebar-button"], PseudoStates::default()));
        assert_eq!(
            style.raw(Prop::BorderTopLeftRadius),
            &Value::Pair(std::rc::Rc::new((
                Value::Length(Length::Percent(1.0)),
                Value::Length(Length::Percent(1.0)),
            ))),
            "the percentage radius was thrown away instead of being kept"
        );

        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n\
             button { border-top-color: #00ff00; color: #0000ff }\n\
             button.bad { border-top-color: @nosuch; color: @nosuch }",
        );
        let style = resolve(&sheet, &button(&["bad"], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000),
            "an invalid inherited property must inherit, not use the runner-up"
        );
        assert_eq!(
            style.get::<Rgba>(Prop::BorderTopColor).to_color32(),
            Color(0xFFFF_0000),
            "border-top-color's initial value is currentColor, i.e. the \
             inherited red -- not the #00ff00 runner-up"
        );
    }
```

`defaults_apply_when_nothing_matches` keeps its name; only the border colour
changes, and gains the reason:

```rust
    #[test]
    fn defaults_apply_when_nothing_matches() {
        let sheet = CompiledSheet::compile("entry { color: red }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Transparent);
        assert_eq!(s.color, Color::BLACK);
        assert_eq!(s.border_width, 0.0);
        assert_eq!(s.border_radius, 0.0);
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
        // M1 had no initial value for `border-color` and defaulted it to
        // transparent. The registry's initial is `currentColor` (contract
        // §1.3), which resolves to the initial `color`: opaque black.
        assert_eq!(s.border_color, Color(0xFF00_0000));
    }
```

Then add the nine new tests, and a shared helper at the top of the module:

```rust
    use crate::css::computed::{FromValue, ResolveEnv};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::color::Rgba;
    use crate::css::value::{Keyword, Length, Value, Wide};

    /// Resolve `node`'s whole ancestor chain under the default environment.
    fn resolve(sheet: &CompiledSheet, node: &Node) -> ComputedStyle {
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(sheet, node, &ResolveEnv::default(), &mut cx)
    }

    #[test]
    fn the_table_has_one_slot_per_longhand_and_starts_at_the_registry_initials() {
        let env = ResolveEnv::default();
        let initial = ComputedStyle::initial(&env);
        for prop in crate::css::registry::longhands() {
            // Every slot is readable and holds that row's initial value.
            let expected = match prop {
                // ResolveEnv seeds these two so it reaches the root of a chain.
                Prop::GtkDpi => Value::Number(env.dpi),
                Prop::FontSize => Value::Length(Length::px(env.root_font_size)),
                other => other.initial(),
            };
            assert_eq!(initial.raw(prop), &expected, "{}", prop.name());
        }
        let custom = ComputedStyle::initial(&ResolveEnv {
            dpi: 192.0,
            root_font_size: 16.0,
        });
        assert_eq!(custom.raw(Prop::GtkDpi), &Value::Number(192.0));
        assert_eq!(
            custom.raw(Prop::FontSize),
            &Value::Length(Length::px(16.0))
        );
    }

    #[test]
    fn get_reads_the_table_typed_and_a_mismatch_is_not_a_panic() {
        let sheet = CompiledSheet::compile(
            "button { color: #123456; opacity: 0.25; border-top-width: 3px; \
             background-repeat: no-repeat }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFF12_3456)
        );
        assert_eq!(style.get::<f32>(Prop::Opacity), 0.25);
        assert_eq!(style.get::<f32>(Prop::BorderTopWidth), 3.0);
        // Asking for the wrong type yields that type's initial-equivalent.
        assert_eq!(style.get::<Rgba>(Prop::Opacity), Rgba::TRANSPARENT);
        assert_eq!(style.get::<f32>(Prop::BackgroundRepeat), 0.0);
    }

    #[test]
    fn inherited_properties_inherit_and_non_inherited_ones_do_not() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; letter-spacing: 2px; padding: 7px; opacity: 0.5 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000)
        );
        assert_eq!(style.get::<f32>(Prop::LetterSpacing), 2.0);
        assert_eq!(style.raw(Prop::PaddingTop), &Prop::PaddingTop.initial());
        assert_eq!(style.get::<f32>(Prop::Opacity), 1.0);
    }

    #[test]
    fn the_wide_keywords_do_what_the_cascade_spec_says() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; padding: 7px }\n\
             button.a { color: initial; padding: inherit }\n\
             button.b { color: unset; padding: unset }\n\
             button.c { color: inherit }",
        );
        let a = resolve(&sheet, &button(&["a"], PseudoStates::default()));
        assert_eq!(a.get::<Rgba>(Prop::Color), Rgba::from_value(&Prop::Color.initial()));
        assert_eq!(a.get::<f32>(Prop::PaddingTop), 7.0, "inherit on a non-inherited property still inherits");

        let b = resolve(&sheet, &button(&["b"], PseudoStates::default()));
        assert_eq!(
            b.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000),
            "unset on an inherited property inherits"
        );
        assert_eq!(
            b.raw(Prop::PaddingTop),
            &Prop::PaddingTop.initial(),
            "unset on a non-inherited property is initial"
        );

        let c = resolve(&sheet, &button(&["c"], PseudoStates::default()));
        assert_eq!(
            c.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000)
        );
    }

    #[test]
    fn a_wide_keyword_at_the_root_of_the_chain_lands_on_the_initial_value() {
        let sheet = CompiledSheet::compile("window { color: inherit; padding: inherit }");
        let window = Node::with_classes("window", &["background"]);
        let style = resolve(&sheet, &window);
        assert_eq!(style.get::<Rgba>(Prop::Color), Rgba::from_value(&Prop::Color.initial()));
        assert_eq!(style.raw(Prop::PaddingTop), &Prop::PaddingTop.initial());
    }

    #[test]
    fn em_rem_and_pt_resolve_at_computed_time_against_the_right_font_size() {
        // `em` on font-size itself resolves against the *parent's* size; every
        // other property resolves against the element's own. `rem` uses the
        // initial font size, not the root node's (GTK's documented divergence).
        // `pt` converts through `-gtk-dpi`, not CSS's fixed 96.
        let sheet = CompiledSheet::compile(
            "window { font-size: 20px }\n\
             button { font-size: 2em; padding-top: 0.5em; padding-right: 1rem; \
                      padding-bottom: 12pt; -gtk-dpi: 144 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.get::<f32>(Prop::FontSize), 40.0, "2em of the parent's 20px");
        assert_eq!(style.get::<f32>(Prop::PaddingTop), 20.0, "0.5em of its own 40px");
        assert_eq!(
            style.get::<f32>(Prop::PaddingRight),
            14.0,
            "1rem is the initial 14px, not the root node's 20px"
        );
        assert_eq!(
            style.get::<f32>(Prop::PaddingBottom),
            24.0,
            "12pt at 144 dpi is 12 * 144 / 72 == 24px"
        );
    }

    #[test]
    fn a_percentage_survives_computed_time_unresolved() {
        // Only `font-size` has its basis at computed time. Everything else
        // keeps the percentage so the used-value accessors can resolve it
        // against the real box.
        let sheet = CompiledSheet::compile(
            "window { font-size: 20px }\nbutton { font-size: 150%; padding-top: 50% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.get::<f32>(Prop::FontSize), 30.0);
        assert_eq!(
            style.raw(Prop::PaddingTop),
            &Value::Length(Length::Percent(0.5))
        );
    }

    #[test]
    fn named_colours_and_current_color_are_absolute_in_the_table() {
        // Interpolators see resolved colours only (contract §2.10), so the
        // table must never hold a `@name` or `currentColor`.
        let sheet = CompiledSheet::compile(
            "@define-color accent_color #3584E4;\n\
             button { color: @accent_color; border-top-color: currentColor }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.raw(Prop::BorderTopColor),
            &Value::Color(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFF35_84E4))
            ))
        );
    }

    #[test]
    fn resolve_with_an_explicit_parent_matches_resolving_the_whole_chain() {
        let sheet = CompiledSheet::compile("window { color: #ff0000 }\nbutton { padding: 2px }");
        let window = Node::with_classes("window", &["background"]);
        let node = Node::new("button");
        window.append_child(&node);
        let mut cx = MatchCx::new();
        let env = ResolveEnv::default();
        let window_style = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);
        assert_eq!(
            ComputedStyle::resolve(&sheet, &node, Some(&window_style), &env, &mut cx),
            ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx)
        );
    }
```

Rewrite the module's existing `button` helper and drop `resolve_with_parent`'s
old test (its replacement is `resolve_with_an_explicit_parent_matches_...`):

```rust
    fn button(classes: &[&str], states: PseudoStates) -> Node {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        button
    }
```

Mutation checks:
- `the_table_has_one_slot_per_longhand_and_starts_at_the_registry_initials`:
  seeding `initial()` from the registry alone (dropping the `env` seeding) breaks
  the `dpi: 192.0` / `root_font_size: 16.0` half.
- `get_reads_the_table_typed_and_a_mismatch_is_not_a_panic`: a `FromValue` impl
  that panics or unwraps on a mismatch fails the last two assertions.
- `inherited_properties_inherit_and_non_inherited_ones_do_not`: ignoring
  `Prop::is_inherited` makes either `letter-spacing` 0 or `padding-top` 7.
- `the_wide_keywords_do_what_the_cascade_spec_says`: swapping `Initial`/`Unset`'s
  arms flips the `.a`/`.b` colour expectations.
- `a_wide_keyword_at_the_root_of_the_chain_lands_on_the_initial_value`: using
  `ComputedStyle::default()` rather than `initial(env)` as the root's parent
  changes the colour.
- `em_rem_and_pt_resolve_at_computed_time_against_the_right_font_size`: resolving
  `font-size`'s `em` against the element's own size recurses to 0/`NaN`; using 96
  dpi for `pt` yields 16, not 24; using the root node's font size for `rem` yields
  20, not 14.
- `a_percentage_survives_computed_time_unresolved`: resolving percentages with a
  `None` basis marks `padding-top` invalid and the assertion sees the initial `0`.
- `named_colours_and_current_color_are_absolute_in_the_table`: leaving
  `ColorValue::CurrentColor` in the slot fails the equality.
- `an_invalid_winner_yields_the_initial_or_inherited_value`: keeping M1's
  runner-up walk returns `#00ff00` for the border colour.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: FAIL to compile with `error[E0599]: no function or associated item
named 'initial' found for struct 'ComputedStyle'`, `no method named 'raw'`, and
`no method named 'get'`.

- [ ] **Step 3: Implement `ResolveEnv`, `FromValue` and the table**

Add to `ui/src/css/computed.rs`, replacing the `ComputedStyle` struct definition
and its `Default` impl (`computed.rs:130-178`):

```rust
/// The environment a computed style resolves against: the values GTK reads
/// from the desktop rather than from CSS.
#[derive(Copy, Clone, Debug)]
pub struct ResolveEnv {
    /// Screen resolution, seeding `-gtk-dpi`. Physical units (`pt pc in cm mm`)
    /// convert through this, **not** CSS's fixed 96.
    pub dpi: f32,
    /// The initial `font-size`, which is what `rem` resolves against in GTK --
    /// not the root node's computed size, as CSS would have it.
    pub root_font_size: f32,
}

impl Default for ResolveEnv {
    fn default() -> Self {
        Self {
            dpi: 96.0,
            root_font_size: ComputedStyle::DEFAULT_FONT_SIZE,
        }
    }
}

/// Read a computed `Value` as a concrete type.
///
/// A type mismatch is never a panic: it yields that type's initial-equivalent
/// and logs at debug. The table is indexed by the registry, so a mismatch means
/// a property's `ParseFn` and its consumer disagree -- a bug to find in the log,
/// not a reason to take the process down mid-paint.
pub trait FromValue: Sized {
    /// Read `value`, or this type's initial-equivalent if it is the wrong shape.
    fn from_value(value: &Value) -> Self;
}

/// The x-height/em ratio used before a face is resolved. The real ratio comes
/// from the matched face (P6); `ex` appears in no GTK theme this crate ships.
const EX_RATIO: f32 = 0.5;

/// Every longhand's computed value, indexed by `Prop::slot()`.
///
/// The ten M1 scalar fields are still here and still derived on every resolve;
/// Task 9 deletes them once `layout`, `paint` and the offscreen gate read the
/// typed accessors instead.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    values: Box<[Value; N_LONGHANDS]>,
    /// Resolved background (M1 view; deleted in Task 9).
    pub background: Background,
    /// Foreground (text) color (M1 view; deleted in Task 9).
    pub color: Color,
    /// Uniform border width in px (M1 view; deleted in Task 9).
    pub border_width: f32,
    /// Border color (M1 view; deleted in Task 9).
    pub border_color: Color,
    /// Uniform corner radius in px (M1 view; deleted in Task 9).
    pub border_radius: f32,
    /// `[top, right, bottom, left]` in px (M1 view; deleted in Task 9).
    pub padding: [f32; 4],
    /// `min-width` in px (M1 view; deleted in Task 9).
    pub min_width: f32,
    /// `min-height` in px (M1 view; deleted in Task 9).
    pub min_height: f32,
    /// Font size in px (M1 view; deleted in Task 9).
    pub font_size: f32,
    /// Which box the background is clipped to (M1 view; deleted in Task 9).
    pub background_clip: BackgroundClip,
}

impl ComputedStyle {
    /// The font size used when no rule sets one.
    pub const DEFAULT_FONT_SIZE: f32 = 14.0;

    /// Every longhand at its registry initial, with `-gtk-dpi` and `font-size`
    /// seeded from `env` so a chain's root inherits the environment.
    #[must_use]
    pub fn initial(env: &ResolveEnv) -> Rc<ComputedStyle> {
        let mut values: Box<[Value; N_LONGHANDS]> =
            Box::new(std::array::from_fn(|_| Value::Keyword(Keyword::None)));
        for prop in registry::longhands() {
            values[prop.slot()] = prop.initial();
        }
        values[Prop::GtkDpi.slot()] = Value::Number(env.dpi);
        values[Prop::FontSize.slot()] = Value::Length(Length::px(env.root_font_size));
        let mut style = Self {
            values,
            background: Background::Transparent,
            color: Color::BLACK,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_radius: 0.0,
            padding: [0.0; 4],
            min_width: 0.0,
            min_height: 0.0,
            font_size: env.root_font_size,
            background_clip: BackgroundClip::default(),
        };
        style.derive_m1_view();
        Rc::new(style)
    }

    /// This longhand's computed value.
    ///
    /// # Panics
    ///
    /// If `prop` is a shorthand: shorthands have no computed slot.
    #[must_use]
    pub fn raw(&self, prop: Prop) -> &Value {
        assert!(prop.is_longhand(), "{} is a shorthand", prop.name());
        &self.values[prop.slot()]
    }

    /// This longhand's computed value, read as `T`.
    #[must_use]
    pub fn get<T: FromValue>(&self, prop: Prop) -> T {
        T::from_value(self.raw(prop))
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        (*Self::initial(&ResolveEnv::default())).clone()
    }
}
```

Add the `FromValue` impls below it:

```rust
/// `f32`: numbers, percentages as their 0..1 fraction, absolute lengths in px,
/// and times in seconds. Anything else is `0.0`.
impl FromValue for f32 {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Number(number) => *number,
            Value::Percentage(fraction) => *fraction,
            Value::Angle(degrees) => *degrees,
            Value::Time(time) => time.as_secs_f32(),
            Value::Length(Length::Abs {
                value,
                unit: LengthUnit::Px,
            }) => *value,
            Value::Length(Length::Percent(fraction)) => *fraction,
            other => {
                tracing::debug!(value = ?other, "value is not a number; reading 0.0");
                0.0
            }
        }
    }
}

impl FromValue for Rgba {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Color(ColorValue::Absolute(rgba)) => *rgba,
            other => {
                tracing::debug!(value = ?other, "value is not a resolved colour; reading transparent");
                Rgba::TRANSPARENT
            }
        }
    }
}

impl FromValue for Keyword {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Keyword(keyword) => *keyword,
            Value::List(items) => match items.first() {
                Some(Value::Keyword(keyword)) => *keyword,
                _ => Keyword::None,
            },
            _ => Keyword::None,
        }
    }
}

impl FromValue for bool {
    /// `none` is false; any other keyword, and any non-zero number, is true.
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Keyword(Keyword::None) => false,
            Value::Keyword(_) => true,
            Value::Number(number) => *number != 0.0,
            _ => false,
        }
    }
}

impl FromValue for Length {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Length(length) => length.clone(),
            _ => Length::zero(),
        }
    }
}

impl FromValue for Time {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Time(time) => *time,
            _ => Time(0.0),
        }
    }
}

impl FromValue for Image {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Image(image) => image.clone(),
            Value::List(items) => match items.first() {
                Some(Value::Image(image)) => image.clone(),
                _ => Image::None,
            },
            _ => Image::None,
        }
    }
}

impl FromValue for TimingFunction {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Timing(timing) => *timing,
            _ => TimingFunction::EASE,
        }
    }
}

impl FromValue for LineHeight {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::LineHeight(line_height) => *line_height,
            _ => LineHeight::Normal,
        }
    }
}

impl FromValue for FontWeight {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontWeight(weight) => *weight,
            Value::Number(number) => FontWeight::Absolute(*number),
            _ => FontWeight::Absolute(400.0),
        }
    }
}

impl FromValue for FontStyle {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontStyle(style) => *style,
            _ => FontStyle::Normal,
        }
    }
}

impl FromValue for AnimationName {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::AnimationName(name) => name.clone(),
            _ => AnimationName::None,
        }
    }
}

impl FromValue for IterationCount {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::IterationCount(count) => *count,
            Value::Number(number) => IterationCount::Count(*number),
            _ => IterationCount::Count(1.0),
        }
    }
}

impl FromValue for Rc<[FontFamily]> {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontFamilies(families) => Rc::clone(families),
            _ => Rc::from(Vec::new()),
        }
    }
}

impl FromValue for Rc<[Shadow]> {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Shadow(shadow) => Rc::from(vec![shadow.clone()]),
            Value::List(items) => Rc::from(
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Shadow(shadow) => Some(shadow.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => Rc::from(Vec::new()),
        }
    }
}
```

Extend the file's imports:

```rust
use std::rc::Rc;

use super::cascade::{CascadedValues, CompiledSheet, cascade};
use super::node::Node;
use super::registry::{self, N_LONGHANDS, Prop};
use super::select::MatchCx;
use super::value::color::{ColorCtx, ColorTable, ColorValue, Rgba};
use super::value::{
    AnimationName, BgSize, CalcNode, ColorStop, FilterFn, FontFamily, FontStyle, FontWeight, Gradient,
    Image, IterationCount, Keyword, Length, LengthCtx, LengthUnit, LineHeight, Position, Shadow,
    Time, TimingFunction, TransformFn, Value, Wide,
};
use skia_rs_safe::core::Color;
```

- [ ] **Step 4: Implement `resolve`, `resolve_chain` and value resolution**

Replace `computed.rs`'s `resolve`/`resolve_with_parent`/`from_declarations`
(the M1 entry points, `computed.rs:267-400` after Task 2's edit) with:

```rust
impl ComputedStyle {
    /// Resolve `node`'s style. `parent` supplies inherited values and the
    /// `em`/`currentColor` bases; `None` means this node is the style root.
    ///
    /// The order is fixed: `-gtk-dpi` first (physical units convert through
    /// it), then `font-size` (whose own `em`/`%` resolve against the *parent's*
    /// size), then `color` (which `currentColor` everywhere else resolves to),
    /// then every remaining longhand in registry order.
    #[must_use]
    pub fn resolve(
        sheet: &CompiledSheet,
        node: &Node,
        parent: Option<&ComputedStyle>,
        env: &ResolveEnv,
        cx: &mut MatchCx,
    ) -> ComputedStyle {
        let root = ComputedStyle::initial(env);
        let parent = parent.unwrap_or(&root);
        let cascaded = cascade(sheet, node, cx);

        let mut values: Box<[Value; N_LONGHANDS]> =
            Box::new(std::array::from_fn(|_| Value::Keyword(Keyword::None)));

        // 1. -gtk-dpi.
        let seed_ctx = LengthCtx {
            font_size_px: parent.font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi: env.dpi,
            percent_basis: None,
        };
        let seed_colors = ColorCtx {
            table: &sheet.colors,
            current: Rgba::from_color32(parent.color),
            depth: 0,
        };
        values[Prop::GtkDpi.slot()] =
            compute_one(Prop::GtkDpi, &cascaded, parent, &seed_ctx, &seed_colors);
        let dpi = match &values[Prop::GtkDpi.slot()] {
            Value::Number(number) if number.is_finite() && *number > 0.0 => *number,
            _ => env.dpi,
        };

        // 2. font-size: its own `em`/`%` are against the parent's size.
        let font_ctx = LengthCtx {
            font_size_px: parent.font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi,
            percent_basis: Some(parent.font_size),
        };
        values[Prop::FontSize.slot()] =
            compute_one(Prop::FontSize, &cascaded, parent, &font_ctx, &seed_colors);
        let font_size = match &values[Prop::FontSize.slot()] {
            Value::Length(Length::Abs {
                value,
                unit: LengthUnit::Px,
            }) if value.is_finite() && *value >= 0.0 => *value,
            // `font-size: medium` is the initial size; every other keyword form
            // is out of the registry's grammar and is invalid at computed-value
            // time, which for an inherited property means the parent's size.
            Value::Keyword(Keyword::Medium) => env.root_font_size,
            _ => parent.font_size,
        };
        values[Prop::FontSize.slot()] = Value::Length(Length::px(font_size));

        // 3. color: `currentColor` on `color` itself is the inherited colour.
        let length_ctx = LengthCtx {
            font_size_px: font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi,
            percent_basis: None,
        };
        values[Prop::Color.slot()] =
            compute_one(Prop::Color, &cascaded, parent, &length_ctx, &seed_colors);
        let own_color = Rgba::from_value(&values[Prop::Color.slot()]);
        let color_ctx = ColorCtx {
            table: &sheet.colors,
            current: own_color,
            depth: 0,
        };

        // 4. everything else, in registry order.
        for prop in registry::longhands() {
            if matches!(prop, Prop::GtkDpi | Prop::FontSize | Prop::Color) {
                continue;
            }
            values[prop.slot()] = compute_one(prop, &cascaded, parent, &length_ctx, &color_ctx);
        }

        let mut style = ComputedStyle {
            values,
            background: Background::Transparent,
            color: own_color.to_color32(),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_radius: 0.0,
            padding: [0.0; 4],
            min_width: 0.0,
            min_height: 0.0,
            font_size,
            background_clip: BackgroundClip::default(),
        };
        style.derive_m1_view();
        style
    }

    /// Resolve `node` by walking its whole ancestor chain from the root down --
    /// the shape M1's `resolve` had, for callers that hold no parent style.
    #[must_use]
    pub fn resolve_chain(
        sheet: &CompiledSheet,
        node: &Node,
        env: &ResolveEnv,
        cx: &mut MatchCx,
    ) -> ComputedStyle {
        let mut chain = vec![node.clone()];
        while let Some(parent) = chain.last().and_then(|node| node.parent()) {
            chain.push(parent);
        }
        let mut style: Option<ComputedStyle> = None;
        for ancestor in chain.iter().rev() {
            style = Some(Self::resolve(sheet, ancestor, style.as_ref(), env, cx));
        }
        style.expect("the chain always contains the node itself")
    }
}

/// One longhand's computed value: the inheritance rule, the wide keywords and
/// invalid-at-computed-value-time (contract §5).
fn compute_one(
    prop: Prop,
    cascaded: &CascadedValues,
    parent: &ComputedStyle,
    length: &LengthCtx,
    color: &ColorCtx<'_>,
) -> Value {
    let fallback = || {
        if prop.is_inherited() {
            parent.raw(prop).clone()
        } else {
            prop.initial()
        }
    };
    let Some(declared) = cascaded.winner(prop) else {
        return fallback();
    };
    match declared {
        Value::Wide(Wide::Inherit) => return parent.raw(prop).clone(),
        Value::Wide(Wide::Initial) => return prop.initial(),
        Value::Wide(Wide::Unset) => return fallback(),
        _ => {}
    }
    match resolve_value(declared, length, color) {
        Some(value) => value,
        None => {
            // Decision 6: CSS's "invalid at computed value time". The runner-ups
            // stay in `CascadedValues` for diagnostics; they are not consulted.
            tracing::debug!(
                property = prop.name(),
                value = ?declared,
                runner_ups = cascaded.candidates(prop).len().saturating_sub(1),
                inherited = prop.is_inherited(),
                "invalid at computed-value time; using the inherited or initial value"
            );
            fallback()
        }
    }
}

/// Turn a parsed value into a computed one: absolute colours, absolute lengths.
///
/// A percentage is *kept*: it is valid, its basis is simply not known until
/// layout. `None` means invalid at computed-value time -- an unknown `@name`, a
/// colour cycle, a unit-incompatible or non-finite `calc()`.
fn resolve_value(value: &Value, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Value> {
    Some(match value {
        Value::Wide(_) => return None,
        Value::Number(number) => {
            if !number.is_finite() {
                return None;
            }
            Value::Number(*number)
        }
        Value::Percentage(fraction) => {
            if !fraction.is_finite() {
                return None;
            }
            Value::Percentage(*fraction)
        }
        Value::Angle(degrees) => {
            if !degrees.is_finite() {
                return None;
            }
            Value::Angle(*degrees)
        }
        Value::Length(inner) => Value::Length(resolve_length(inner, length)?),
        Value::Color(inner) => Value::Color(ColorValue::Absolute(inner.resolve(color)?)),
        Value::Shadow(shadow) => Value::Shadow(resolve_shadow(shadow, length, color)?),
        Value::Image(image) => Value::Image(resolve_image(image, length, color)?),
        Value::Position(position) => Value::Position(resolve_position(position, length)?),
        Value::BgSize(BgSize::Explicit(x, y)) => Value::BgSize(BgSize::Explicit(
            resolve_length(x, length)?,
            resolve_length(y, length)?,
        )),
        Value::LineHeight(LineHeight::Length(inner)) => {
            Value::LineHeight(LineHeight::Length(resolve_length(inner, length)?))
        }
        Value::Transform(list) => {
            let mut out = Vec::with_capacity(list.len());
            for function in list.iter() {
                out.push(resolve_transform(function, length)?);
            }
            Value::Transform(Rc::from(out))
        }
        Value::Filter(list) => {
            let mut out = Vec::with_capacity(list.len());
            for function in list.iter() {
                out.push(resolve_filter(function, length, color)?);
            }
            Value::Filter(Rc::from(out))
        }
        Value::IconPalette(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (name, slot) in entries.iter() {
                out.push((Rc::clone(name), ColorValue::Absolute(slot.resolve(color)?)));
            }
            Value::IconPalette(Rc::from(out))
        }
        Value::Pair(pair) => Value::Pair(Rc::new((
            resolve_value(&pair.0, length, color)?,
            resolve_value(&pair.1, length, color)?,
        ))),
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(resolve_value(item, length, color)?);
            }
            Value::List(Rc::from(out))
        }
        other => other.clone(),
    })
}

/// Absolutise a length, keeping percentages whose basis is not yet known.
fn resolve_length(length: &Length, ctx: &LengthCtx) -> Option<Length> {
    if matches!(length, Length::Auto) {
        return Some(Length::Auto);
    }
    if length_has_percent(length) && ctx.percent_basis.is_none() {
        return Some(length.clone());
    }
    let px = length.resolve(ctx)?;
    px.is_finite().then(|| Length::px(px))
}

fn length_has_percent(length: &Length) -> bool {
    match length {
        Length::Percent(_) => true,
        Length::Calc(node) => calc_has_percent(node),
        Length::Abs { .. } | Length::Auto => false,
    }
}

fn calc_has_percent(node: &CalcNode) -> bool {
    match node {
        CalcNode::Percent(_) => true,
        CalcNode::Length(length) => length_has_percent(length),
        CalcNode::Sum(a, b) | CalcNode::Difference(a, b) => calc_has_percent(a) || calc_has_percent(b),
        CalcNode::Product(a, _) | CalcNode::Quotient(a, _) => calc_has_percent(a),
        CalcNode::Min(nodes) | CalcNode::Max(nodes) => nodes.iter().any(calc_has_percent),
        CalcNode::Clamp(a, b, c) => {
            calc_has_percent(a) || calc_has_percent(b) || calc_has_percent(c)
        }
        CalcNode::Number(_) | CalcNode::Angle(_) | CalcNode::Time(_) => false,
    }
}

/// A shadow's omitted colour *is* `currentColor`; it is resolved here so an
/// interpolator never has to know about a paint context (contract §2.10).
fn resolve_shadow(shadow: &Shadow, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Shadow> {
    let resolved = match &shadow.color {
        Some(declared) => declared.resolve(color)?,
        None => color.current,
    };
    Some(Shadow {
        color: Some(ColorValue::Absolute(resolved)),
        offset_x: resolve_length(&shadow.offset_x, length)?,
        offset_y: resolve_length(&shadow.offset_y, length)?,
        blur: resolve_length(&shadow.blur, length)?,
        spread: resolve_length(&shadow.spread, length)?,
        inset: shadow.inset,
    })
}

fn resolve_position(position: &Position, length: &LengthCtx) -> Option<Position> {
    Some(Position {
        x: resolve_length(&position.x, length)?,
        y: resolve_length(&position.y, length)?,
        z: match &position.z {
            Some(z) => Some(resolve_length(z, length)?),
            None => None,
        },
    })
}

fn resolve_image(image: &Image, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Image> {
    Some(match image {
        Image::Solid(fill) => Image::Solid(ColorValue::Absolute(fill.resolve(color)?)),
        Image::Gradient(gradient) => {
            let mut stops = Vec::with_capacity(gradient.stops.len());
            for stop in gradient.stops.iter() {
                stops.push(ColorStop {
                    color: ColorValue::Absolute(stop.color.resolve(color)?),
                    position: match &stop.position {
                        Some(position) => Some(resolve_length(position, length)?),
                        None => None,
                    },
                    hint: match &stop.hint {
                        Some(hint) => Some(resolve_length(hint, length)?),
                        None => None,
                    },
                });
            }
            Image::Gradient(Rc::new(Gradient {
                kind: gradient.kind.clone(),
                repeating: gradient.repeating,
                stops: Rc::from(stops),
                interpolation: gradient.interpolation,
            }))
        }
        Image::CrossFade(layers) => {
            let mut out = Vec::with_capacity(layers.len());
            for (share, image) in layers.iter() {
                out.push((*share, resolve_image(image, length, color)?));
            }
            Image::CrossFade(Rc::from(out))
        }
        // `url()` and the `-gtk-*` icon functions carry nothing to resolve; a
        // URL that fails to decode is recorded-unresolved at paint time, not
        // invalid here.
        other => other.clone(),
    })
}

fn resolve_transform(function: &TransformFn, ctx: &LengthCtx) -> Option<TransformFn> {
    Some(match function {
        TransformFn::Translate(x, y) => {
            TransformFn::Translate(resolve_length(x, ctx)?, resolve_length(y, ctx)?)
        }
        TransformFn::TranslateX(x) => TransformFn::TranslateX(resolve_length(x, ctx)?),
        TransformFn::TranslateY(y) => TransformFn::TranslateY(resolve_length(y, ctx)?),
        TransformFn::TranslateZ(z) => TransformFn::TranslateZ(resolve_length(z, ctx)?),
        TransformFn::Translate3d(x, y, z) => TransformFn::Translate3d(
            resolve_length(x, ctx)?,
            resolve_length(y, ctx)?,
            resolve_length(z, ctx)?,
        ),
        TransformFn::Perspective(d) => TransformFn::Perspective(resolve_length(d, ctx)?),
        other => other.clone(),
    })
}

fn resolve_filter(
    function: &FilterFn,
    length: &LengthCtx,
    color: &ColorCtx<'_>,
) -> Option<FilterFn> {
    Some(match function {
        FilterFn::Blur(radius) => FilterFn::Blur(resolve_length(radius, length)?),
        FilterFn::DropShadow(shadow) => {
            FilterFn::DropShadow(resolve_shadow(shadow, length, color)?)
        }
        other => other.clone(),
    })
}
```

- [ ] **Step 5: Derive the M1 view from the table and fix the call sites**

Add to `impl ComputedStyle` (the M1 fields are now a *view*, not the storage):

```rust
    /// Fill the ten M1 scalar fields from the table.
    ///
    /// The M1 tests, `layout`, `paint` and the offscreen gate still read these;
    /// deriving them here means all 22 of `computed.rs`'s pinned Adwaita bytes
    /// are a regression bar on the new table rather than a thing to rewrite on
    /// trust. Task 9 deletes this together with the fields.
    fn derive_m1_view(&mut self) {
        let ctx = m1_bridge::ctx(self.font_size, None);
        self.color = Rgba::from_value(self.raw(Prop::Color)).to_color32();
        self.background = m1_bridge::color(
            self.raw(Prop::BackgroundColor),
            &ColorTable::new(),
            self.color,
        )
        .map_or(Background::Transparent, Background::Solid);
        if let Some(background) = m1_bridge::background(
            self.raw(Prop::BackgroundImage),
            &ColorTable::new(),
            self.color,
            &ctx,
        )
        .flatten()
        {
            self.background = background;
        }
        self.border_width = m1_bridge::px(self.raw(Prop::BorderTopWidth), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        if m1_bridge::border_style_is_none(self.raw(Prop::BorderTopStyle)) == Some(true) {
            self.border_width = 0.0;
        }
        self.border_color = m1_bridge::color(
            self.raw(Prop::BorderTopColor),
            &ColorTable::new(),
            self.color,
        )
        .unwrap_or(Color::TRANSPARENT);
        self.border_radius = m1_bridge::radius(self.raw(Prop::BorderTopLeftRadius), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        for (index, prop) in [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ]
        .into_iter()
        .enumerate()
        {
            self.padding[index] = m1_bridge::px(self.raw(prop), &ctx).unwrap_or(0.0).max(0.0);
        }
        self.background_clip = m1_bridge::clip(self.raw(Prop::BackgroundClip)).unwrap_or_default();
        self.min_width = m1_bridge::px(self.raw(Prop::MinWidth), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        self.min_height = m1_bridge::px(self.raw(Prop::MinHeight), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
    }
```

The table already holds absolute colours, so the bridge's colour readers are
handed an empty `ColorTable` -- there is nothing left to look up. Delete
`m1_bridge::border_style_is_none`'s "`None` means keep walking" comment and the
now-dead `pick` function.

Fix the four call sites of the old entry points:

- `ui/src/paint.rs:156` — `let style = ComputedStyle::resolve(&sheet, &node);` becomes
  ```rust
  let style = ComputedStyle::resolve_chain(
      &sheet,
      &node,
      &crate::css::computed::ResolveEnv::default(),
      &mut crate::css::select::MatchCx::new(),
  );
  ```
- `ui/src/widget/button.rs:86-95` — the two calls become `resolve_chain(...)` and
  `resolve(sheet, &self.node, self.parent_style.as_ref(), &ResolveEnv::default(),
  &mut MatchCx::new())`. (Task 7 gives `Button` its own long-lived `MatchCx` and
  `ResolveEnv`; this step only keeps it compiling.)
- `ui/src/app.rs:291-301` — `button_style` and `headerbar_min_height` call
  `resolve_chain(sheet, &node, &ResolveEnv::default(), &mut MatchCx::new())`.

Rewrite `ui/src/layout.rs`'s test fixture, which can no longer name
`ComputedStyle`'s private `values` field through functional-record-update syntax.
**Every number in these tests is unchanged**; only the fixture is:

```rust
#[cfg(test)]
mod tests {
    use super::layout_button;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::text::TextMetrics;

    /// The Adwaita button box, expressed as the CSS it actually comes from.
    fn style_from(css: &str) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        ComputedStyle::resolve_chain(
            &sheet,
            &button,
            &ResolveEnv::default(),
            &mut MatchCx::new(),
        )
    }

    fn adwaita_like() -> ComputedStyle {
        style_from(
            "button { background-color: #dad6d2; color: #2e3436; \
             border: 1px solid #cdc7c2; border-radius: 5px; padding: 4px 9px; \
             min-width: 16px; min-height: 24px; font-size: 14px }",
        )
    }

    fn bare() -> ComputedStyle {
        style_from(
            "button { background-color: #dad6d2; color: #2e3436; border: 0 solid #cdc7c2; \
             border-radius: 5px; padding: 0; min-width: 0; min-height: 0; font-size: 14px }",
        )
    }

    fn label(width: f32, height: f32) -> TextMetrics {
        TextMetrics {
            width,
            ascent: height * 0.8,
            descent: height * 0.2,
            line_height: height,
        }
    }
```

and in `a_borderless_paddingless_button_is_exactly_the_label`, replace the
struct-literal `style` with `let style = bare();`. The four assertions
(`42.0`, `17.0`, `0.0`, `0.0`) stay exactly as they are.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib`
Expected: PASS -- all 22 `computed.rs` tests (two now inverted), all 6 `layout.rs`
tests with their original numbers, `cascade.rs`, `paint.rs`, `button.rs`, `app.rs`.

- [ ] **Step 7: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean. `ui/tests/themed_button_offscreen.rs` still green on
the derived M1 view.

- [ ] **Step 8: Commit**

```bash
git add ui/src/css/computed.rs ui/src/layout.rs ui/src/paint.rs ui/src/widget/button.rs ui/src/app.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): registry-indexed computed style with real CSS inheritance

ComputedStyle is now Box<[Value; 95]> indexed by Prop::slot(), resolved in
the fixed order -gtk-dpi -> font-size -> color -> the rest, with CSS's
inheritance rule, the three wide keywords, and invalid-at-computed-value-time
(Decision 6) replacing M1's runner-up walk. em/rem/pt/calc absolutise here;
percentages survive to used-value time, where the basis exists. Colours
resolve to ColorValue::Absolute so interpolators never see @name or
currentColor.

M1's ten scalar fields stay as a derived view so all 22 of computed.rs's
pinned Adwaita bytes -- and the offscreen pixel gate -- remain the regression
bar for the new table.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: used-value accessors — font, colour, box model

**Files:**
- Modify: `ui/src/css/computed.rs` (append an accessor `impl ComputedStyle` block)
- Modify: `ui/src/css/computed.rs` test module (5 tests)

**Interfaces:**
- Consumes (Task 3): `ComputedStyle::raw/get`, `FromValue`, `ResolveEnv`,
  `EX_RATIO`, `resolve_length`.
- Consumes (P1): `LengthCtx`, `Length::resolve`, `Rgba`, `Prop`, `Keyword`.
- Produces:
  ```rust
  impl ComputedStyle {
      pub fn font_size_px(&self) -> f32;
      pub fn dpi(&self) -> f32;
      pub fn length_ctx(&self, env: &ResolveEnv, percent_basis: Option<f32>) -> LengthCtx;
      pub fn color(&self) -> Rgba;
      pub fn opacity(&self) -> f32;
      pub fn padding(&self, basis: f32) -> [f32; 4];
      pub fn margin(&self, basis: f32) -> [Option<f32>; 4];
      pub fn min_size(&self, basis: (f32, f32)) -> (f32, f32);
  }
  ```

- [ ] **Step 1: Write the failing tests**

Append to `computed.rs`'s test module:

```rust
    #[test]
    fn the_box_accessors_read_adwaitas_button_in_used_pixels() {
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        assert_eq!(style.font_size_px(), 14.0);
        assert_eq!(style.dpi(), 96.0);
        assert_eq!(style.color().to_color32(), Color(0xFF2E_3436));
        assert_eq!(style.opacity(), 1.0);
        assert_eq!(style.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(style.margin(0.0), [Some(0.0); 4]);
        assert_eq!(style.min_size((0.0, 0.0)), (16.0, 24.0));
    }

    #[test]
    fn percentage_box_values_resolve_against_the_used_basis() {
        let sheet = CompiledSheet::compile(
            "button { padding: 25%; min-width: 50%; min-height: 10%; margin-left: 5% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        // CSS resolves *every* padding side against the containing block's
        // *width*; the accessor takes that one basis.
        assert_eq!(style.padding(200.0), [50.0, 50.0, 50.0, 50.0]);
        assert_eq!(style.min_size((200.0, 80.0)), (100.0, 8.0));
        assert_eq!(style.margin(200.0)[3], Some(10.0));
    }

    #[test]
    fn auto_margins_are_none_and_everything_else_clamps_sanely() {
        let sheet = CompiledSheet::compile(
            "button { margin: auto; padding: -4px; min-width: -1px; min-height: -2px; \
             opacity: 4 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.margin(0.0), [None; 4]);
        assert_eq!(style.padding(0.0), [0.0; 4], "negative padding clamps to 0");
        assert_eq!(style.min_size((0.0, 0.0)), (0.0, 0.0));
        assert_eq!(style.opacity(), 1.0, "opacity clamps into 0..=1");
    }

    #[test]
    fn the_length_context_carries_the_styles_own_font_size_and_dpi() {
        let sheet = CompiledSheet::compile("button { font-size: 21px; -gtk-dpi: 192 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let ctx = style.length_ctx(&ResolveEnv::default(), Some(64.0));
        assert_eq!(ctx.font_size_px, 21.0);
        assert_eq!(ctx.dpi, 192.0);
        assert_eq!(ctx.root_font_size_px, 14.0);
        assert_eq!(ctx.percent_basis, Some(64.0));
    }

    #[test]
    fn opacity_and_colour_survive_inheritance_without_leaking() {
        let sheet = CompiledSheet::compile("window { opacity: 0.5; color: #ff0000 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.opacity(), 1.0, "opacity is not inherited");
        assert_eq!(style.color().to_color32(), Color(0xFFFF_0000));
    }
```

Mutation checks:
- `the_box_accessors_read_adwaitas_button_in_used_pixels`: reading the sides in
  any order other than TRBL swaps the `9.0`s and `4.0`s.
- `percentage_box_values_resolve_against_the_used_basis`: passing `None` as the
  basis makes every percentage invalid and the assertions read `0.0`.
- `auto_margins_are_none_and_everything_else_clamps_sanely`: dropping the `max(0.0)`
  clamp yields `-4.0`; dropping the opacity clamp yields `4.0`.
- `the_length_context_carries_the_styles_own_font_size_and_dpi`: taking `dpi` from
  `env` instead of the computed `-gtk-dpi` row yields `96.0`.
- `opacity_and_colour_survive_inheritance_without_leaking`: marking `opacity`
  inherited in the registry (or ignoring the flag here) yields `0.5`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: FAIL to compile with `error[E0599]: no method named 'font_size_px'
found for struct 'ComputedStyle'`.

- [ ] **Step 3: Implement the accessors**

Append to `ui/src/css/computed.rs`:

```rust
/// Used-value accessors: the computed table, read the way layout, paint and
/// text want it -- in device pixels, per side, with percentages resolved
/// against the basis the caller actually has.
impl ComputedStyle {
    /// The computed `font-size`, in px.
    #[must_use]
    pub fn font_size_px(&self) -> f32 {
        let size = self.get::<f32>(Prop::FontSize);
        if size.is_finite() && size >= 0.0 {
            size
        } else {
            Self::DEFAULT_FONT_SIZE
        }
    }

    /// The computed `-gtk-dpi`. Physical units convert through this.
    #[must_use]
    pub fn dpi(&self) -> f32 {
        let dpi = self.get::<f32>(Prop::GtkDpi);
        if dpi.is_finite() && dpi > 0.0 {
            dpi
        } else {
            ResolveEnv::default().dpi
        }
    }

    /// A length context for resolving whatever survived computed time.
    ///
    /// Only percentages do, so `root_font_size_px` is inert here; it is taken
    /// from `env` anyway so a caller with a non-default environment stays
    /// self-consistent.
    #[must_use]
    pub fn length_ctx(&self, env: &ResolveEnv, percent_basis: Option<f32>) -> LengthCtx {
        LengthCtx {
            font_size_px: self.font_size_px(),
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi: self.dpi(),
            percent_basis,
        }
    }

    /// The default-environment context the accessors below use.
    fn used_ctx(&self, percent_basis: Option<f32>) -> LengthCtx {
        self.length_ctx(&ResolveEnv::default(), percent_basis)
    }

    /// One longhand as a used length in px, or `None` if it is not a length.
    fn length_px(&self, prop: Prop, basis: Option<f32>) -> Option<f32> {
        match self.raw(prop) {
            Value::Length(length) => length
                .resolve(&self.used_ctx(basis))
                .filter(|px| px.is_finite()),
            Value::Number(number) if *number == 0.0 => Some(0.0),
            _ => None,
        }
    }

    /// The computed `color`.
    #[must_use]
    pub fn color(&self) -> Rgba {
        self.get::<Rgba>(Prop::Color)
    }

    /// The computed `opacity`, clamped into `0..=1` as CSS requires.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        let opacity = self.get::<f32>(Prop::Opacity);
        if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// `padding-*` in px, `[top, right, bottom, left]`. `basis` is the
    /// containing block's width: CSS resolves every padding percentage,
    /// vertical ones included, against the width.
    #[must_use]
    pub fn padding(&self, basis: f32) -> [f32; 4] {
        const SIDES: [Prop; 4] = [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ];
        let mut padding = [0.0; 4];
        for (slot, prop) in SIDES.into_iter().enumerate() {
            padding[slot] = self.length_px(prop, Some(basis)).unwrap_or(0.0).max(0.0);
        }
        padding
    }

    /// `margin-*` in px, `[top, right, bottom, left]`; `None` is `auto`.
    /// Negative margins are legal and are **not** clamped.
    #[must_use]
    pub fn margin(&self, basis: f32) -> [Option<f32>; 4] {
        const SIDES: [Prop; 4] = [
            Prop::MarginTop,
            Prop::MarginRight,
            Prop::MarginBottom,
            Prop::MarginLeft,
        ];
        let mut margin = [Some(0.0); 4];
        for (slot, prop) in SIDES.into_iter().enumerate() {
            margin[slot] = match self.raw(prop) {
                Value::Length(Length::Auto) => None,
                _ => Some(self.length_px(prop, Some(basis)).unwrap_or(0.0)),
            };
        }
        margin
    }

    /// `min-width`/`min-height` as a **content-box** minimum, in px.
    ///
    /// GTK's minimums floor the content box, not the border box -- the M1
    /// ruling that makes Adwaita's empty button 36x34 rather than 20x27.
    #[must_use]
    pub fn min_size(&self, basis: (f32, f32)) -> (f32, f32) {
        (
            self.length_px(Prop::MinWidth, Some(basis.0))
                .unwrap_or(0.0)
                .max(0.0),
            self.length_px(Prop::MinHeight, Some(basis.1))
                .unwrap_or(0.0)
                .max(0.0),
        )
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: PASS.

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 6: Commit**

```bash
git add ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): used-value accessors for font, colour and the box model

font_size_px/dpi/length_ctx/color/opacity/padding/margin/min_size read the
computed table in device pixels, resolving the percentages that survived
computed time against the basis the caller supplies. margin keeps `auto` as
None and stays unclamped; padding and min-size clamp at zero; min-size is a
content-box minimum, per GTK.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: used-value accessors — per-side borders and per-corner radii

**Files:**
- Modify: `ui/src/css/computed.rs` (extend the accessor `impl` block)
- Modify: `ui/src/css/computed.rs` test module (4 tests)

**Interfaces:**
- Consumes (Task 4): `ComputedStyle::length_px`, `used_ctx`, `get`, `raw`.
- Produces:
  ```rust
  impl ComputedStyle {
      pub fn border_widths(&self) -> [f32; 4];
      pub fn border_colors(&self) -> [Rgba; 4];
      pub fn border_styles(&self) -> [Keyword; 4];
      pub fn border_radii(&self, w: f32, h: f32) -> [[f32; 2]; 4];
  }
  ```
  All four are `[top, right, bottom, left]`; `border_radii` is
  `[top-left, top-right, bottom-right, bottom-left]` × `(rx, ry)`.

- [ ] **Step 1: Write the failing tests**

Append to `computed.rs`'s test module:

```rust
    #[test]
    fn mixed_per_side_borders_are_first_class() {
        // The headline M1 gap: `border_width`/`border_color` were one scalar
        // each, so the top side stood for all four.
        let sheet = CompiledSheet::compile(
            "button { border-width: 1px 2px 3px 4px; \
             border-color: red green blue white; \
             border-style: solid dashed dotted double }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.border_widths(), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            style.border_colors().map(Rgba::to_color32),
            [
                Color(0xFFFF_0000),
                Color(0xFF00_8000),
                Color(0xFF00_00FF),
                Color(0xFFFF_FFFF),
            ]
        );
        assert_eq!(
            style.border_styles(),
            [
                Keyword::Solid,
                Keyword::Dashed,
                Keyword::Dotted,
                Keyword::Double
            ]
        );
    }

    #[test]
    fn a_none_or_hidden_border_style_forces_a_used_width_of_zero() {
        // E1: `border: none` was a no-op, so a flat button kept a 1px border.
        let sheet = CompiledSheet::compile(
            "button { border: 5px solid red }\n\
             button.flat { border-top-style: none; border-right-style: hidden }",
        );
        let style = resolve(&sheet, &button(&["flat"], PseudoStates::default()));
        assert_eq!(style.border_widths(), [0.0, 0.0, 5.0, 5.0]);
    }

    #[test]
    fn per_corner_radii_carry_both_axes_and_percentages() {
        let sheet = CompiledSheet::compile(
            "button { border-radius: 4px 8px 12px 16px / 2px 4px 6px 8px }\n\
             button.round { border-radius: 50% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.border_radii(200.0, 100.0),
            [[4.0, 2.0], [8.0, 4.0], [12.0, 6.0], [16.0, 8.0]]
        );

        let round = resolve(&sheet, &button(&["round"], PseudoStates::default()));
        assert_eq!(
            round.border_radii(200.0, 100.0),
            [[100.0, 50.0]; 4],
            "a percentage radius resolves against the box's own width and height"
        );
    }

    #[test]
    fn overlapping_corner_radii_are_scaled_down_together() {
        // CSS Backgrounds L3 §corner overlap: if two adjacent radii exceed the
        // side they share, every radius on the box scales by the same factor.
        let sheet = CompiledSheet::compile("button { border-radius: 60px }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        // Top edge is 100 wide but wants 60 + 60: f = 100 / 120.
        assert_eq!(style.border_radii(100.0, 100.0), [[50.0, 50.0]; 4]);
        // Big enough box: no scaling at all.
        assert_eq!(style.border_radii(400.0, 400.0), [[60.0, 60.0]; 4]);
    }
```

Mutation checks:
- `mixed_per_side_borders_are_first_class`: reading `Prop::BorderTopWidth` for
  every side (M1's behaviour) yields `[1,1,1,1]`.
- `a_none_or_hidden_border_style_forces_a_used_width_of_zero`: dropping the
  `none|hidden` check yields `[5,5,5,5]`.
- `per_corner_radii_carry_both_axes_and_percentages`: resolving the vertical
  radius against the width yields `[100,100]`; ignoring the `/` split makes the
  second axis equal the first.
- `overlapping_corner_radii_are_scaled_down_together`: omitting the scale factor
  leaves `[[60,60]; 4]` in the first assertion; applying it unconditionally
  shrinks the second.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: FAIL to compile with `error[E0599]: no method named 'border_widths'
found for struct 'ComputedStyle'`.

- [ ] **Step 3: Implement the border accessors**

Append to the accessor `impl ComputedStyle` block in `ui/src/css/computed.rs`:

```rust
    /// Used border widths in px, `[top, right, bottom, left]`.
    ///
    /// A `none`/`hidden` style forces its side's used width to zero -- that is
    /// what makes `border: none` undo an earlier `border: 1px solid`.
    #[must_use]
    pub fn border_widths(&self) -> [f32; 4] {
        const WIDTHS: [Prop; 4] = [
            Prop::BorderTopWidth,
            Prop::BorderRightWidth,
            Prop::BorderBottomWidth,
            Prop::BorderLeftWidth,
        ];
        let styles = self.border_styles();
        let mut widths = [0.0; 4];
        for (slot, prop) in WIDTHS.into_iter().enumerate() {
            if matches!(styles[slot], Keyword::None | Keyword::Hidden) {
                continue;
            }
            widths[slot] = self.length_px(prop, None).unwrap_or(0.0).max(0.0);
        }
        widths
    }

    /// Border colours, `[top, right, bottom, left]`.
    #[must_use]
    pub fn border_colors(&self) -> [Rgba; 4] {
        [
            self.get::<Rgba>(Prop::BorderTopColor),
            self.get::<Rgba>(Prop::BorderRightColor),
            self.get::<Rgba>(Prop::BorderBottomColor),
            self.get::<Rgba>(Prop::BorderLeftColor),
        ]
    }

    /// Border styles, `[top, right, bottom, left]`.
    #[must_use]
    pub fn border_styles(&self) -> [Keyword; 4] {
        [
            self.get::<Keyword>(Prop::BorderTopStyle),
            self.get::<Keyword>(Prop::BorderRightStyle),
            self.get::<Keyword>(Prop::BorderBottomStyle),
            self.get::<Keyword>(Prop::BorderLeftStyle),
        ]
    }

    /// Corner radii in px, `[top-left, top-right, bottom-right, bottom-left]`,
    /// each `[rx, ry]`, already scaled by CSS's overlapping-curves rule.
    ///
    /// Horizontal radii resolve percentages against `w`, vertical ones against
    /// `h`, per CSS Backgrounds and Borders L3 §border-radius.
    #[must_use]
    pub fn border_radii(&self, w: f32, h: f32) -> [[f32; 2]; 4] {
        const CORNERS: [Prop; 4] = [
            Prop::BorderTopLeftRadius,
            Prop::BorderTopRightRadius,
            Prop::BorderBottomRightRadius,
            Prop::BorderBottomLeftRadius,
        ];
        let horizontal = self.used_ctx(Some(w));
        let vertical = self.used_ctx(Some(h));
        let mut radii = [[0.0f32; 2]; 4];
        for (slot, prop) in CORNERS.into_iter().enumerate() {
            let (rx, ry) = match self.raw(prop) {
                Value::Pair(pair) => (pair.0.clone(), pair.1.clone()),
                other => (other.clone(), other.clone()),
            };
            radii[slot][0] = used_length(&rx, &horizontal);
            radii[slot][1] = used_length(&ry, &vertical);
        }

        // f = min over the four sides of side / (sum of its two radii).
        let mut factor = f32::INFINITY;
        for (side, sum) in [
            (w, radii[0][0] + radii[1][0]),
            (h, radii[1][1] + radii[2][1]),
            (w, radii[3][0] + radii[2][0]),
            (h, radii[0][1] + radii[3][1]),
        ] {
            if sum > 0.0 {
                factor = factor.min(side / sum);
            }
        }
        if factor.is_finite() && factor < 1.0 {
            for corner in &mut radii {
                corner[0] *= factor;
                corner[1] *= factor;
            }
        }
        radii
    }
}

/// One half of a corner radius, in px. Non-lengths and non-finite results are
/// zero: a radius is never negative and never `auto`.
fn used_length(value: &Value, ctx: &LengthCtx) -> f32 {
    match value {
        Value::Length(length) => length
            .resolve(ctx)
            .filter(|px| px.is_finite())
            .unwrap_or(0.0)
            .max(0.0),
        Value::Number(number) if *number == 0.0 => 0.0,
        _ => 0.0,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: PASS.

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 6: Commit**

```bash
git add ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): per-side border and per-corner radius accessors

border_widths/colors/styles are four independent sides, and border_radii is
four corners of two axes with percentages resolved against the box and CSS's
overlapping-curves scale factor applied. This closes the M1 gap where one
scalar border width, colour and radius stood for all four sides.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `BackgroundLayer`, layered backgrounds and box shadows

**Files:**
- Modify: `ui/src/css/computed.rs` (add `BackgroundLayer`, extend the accessors)
- Modify: `ui/src/css/computed.rs` test module (4 tests)

**Interfaces:**
- Consumes (Task 4/5): the accessor block, `FromValue`, `raw`.
- Consumes (P1): `Image`, `Position`, `BgSize`, `RepeatStyle`, `Keyword`, `Shadow`.
- Produces:
  ```rust
  /// One resolved `background-*` layer, top-first (CSS: first listed is topmost).
  #[derive(Clone, Debug)]
  pub struct BackgroundLayer {
      pub image: Image,
      pub position: Position,
      pub size: BgSize,
      pub repeat: RepeatStyle,
      pub origin: Keyword,
      pub clip: Keyword,
      pub blend: Keyword,
  }
  impl ComputedStyle {
      pub fn background_layers(&self) -> Vec<BackgroundLayer>;
      pub fn box_shadows(&self) -> Rc<[Shadow]>;
  }
  ```
  P4 re-exports `BackgroundLayer` from `paint/mod.rs`, where the contract's §8
  signatures name it.

- [ ] **Step 1: Write the failing tests**

Append to `computed.rs`'s test module:

```rust
    #[test]
    fn adwaitas_button_is_one_layer_clipped_to_the_border_box() {
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let layers = style.background_layers();
        assert_eq!(layers.len(), 1);
        assert!(matches!(layers[0].image, Image::Gradient(_)));
        // Adwaita sets no `background-clip`/`-origin`, so both are at their
        // CSS initials -- and they differ, which is why a translucent border
        // shows the element's own background.
        assert_eq!(layers[0].clip, Keyword::BorderBox);
        assert_eq!(layers[0].origin, Keyword::PaddingBox);
        assert_eq!(layers[0].blend, Keyword::Normal);
    }

    #[test]
    fn multiple_layers_are_top_first_and_the_shorter_lists_cycle() {
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#ff0000), image(#00ff00), image(#0000ff); \
             background-clip: content-box, padding-box; \
             background-repeat: no-repeat }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let layers = style.background_layers();
        assert_eq!(layers.len(), 3, "the image list drives the layer count");
        assert_eq!(
            layers[0].image,
            Image::Solid(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFFFF_0000))
            )),
            "the first listed layer is the topmost"
        );
        // CSS repeats the shorter lists to the image list's length.
        assert_eq!(layers[0].clip, Keyword::ContentBox);
        assert_eq!(layers[1].clip, Keyword::PaddingBox);
        assert_eq!(layers[2].clip, Keyword::ContentBox);
        assert_eq!(layers[2].repeat.x, Keyword::NoRepeat);
    }

    #[test]
    fn a_style_with_no_background_image_still_reports_no_layers() {
        let sheet = CompiledSheet::compile("button { background-color: #112233 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(
            style.background_layers().is_empty(),
            "`background-image: none` paints no layer; the colour is not a layer"
        );
        assert_eq!(
            style.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
        );
    }

    #[test]
    fn box_shadows_keep_their_order_and_resolve_their_colours() {
        let sheet = CompiledSheet::compile(
            "button { color: #ff0000; \
             box-shadow: 1px 2px 3px 4px #00ff00, inset 5px 6px }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let shadows = style.box_shadows();
        assert_eq!(shadows.len(), 2);
        assert_eq!(shadows[0].offset_x, Length::px(1.0));
        assert_eq!(shadows[0].offset_y, Length::px(2.0));
        assert_eq!(shadows[0].blur, Length::px(3.0));
        assert_eq!(shadows[0].spread, Length::px(4.0));
        assert!(!shadows[0].inset);
        assert!(shadows[1].inset);
        // An omitted shadow colour is `currentColor`, resolved at computed time
        // so an interpolator never needs a paint context.
        assert_eq!(
            shadows[1].color,
            Some(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFFFF_0000))
            ))
        );
        assert!(
            resolve(&adwaita(), &button(&[], PseudoStates::default()))
                .box_shadows()
                .is_empty()
        );
    }
```

Mutation checks:
- `adwaitas_button_is_one_layer_clipped_to_the_border_box`: defaulting `clip` to
  `PaddingBox` (M1's paint bug) or `origin` to `BorderBox` fails outright.
- `multiple_layers_are_top_first_and_the_shorter_lists_cycle`: reversing the layer
  order puts blue on top; truncating instead of cycling makes `layers[2].clip`
  the initial `BorderBox`.
- `a_style_with_no_background_image_still_reports_no_layers`: emitting a layer for
  `Image::None` returns one layer.
- `box_shadows_keep_their_order_and_resolve_their_colours`: leaving
  `Shadow::color` as `None` fails the last colour assertion; reversing the list
  swaps `inset`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: FAIL to compile with `error[E0599]: no method named 'background_layers'
found for struct 'ComputedStyle'`.

- [ ] **Step 3: Implement `BackgroundLayer` and the two accessors**

Append to `ui/src/css/computed.rs`:

```rust
/// One resolved `background-*` layer.
///
/// Layers are **top-first**: CSS paints the first listed layer closest to the
/// viewer, with `background-color` behind all of them. Each layer carries its
/// own position/size/repeat/origin/clip/blend, shorter lists having been
/// repeated to the `background-image` list's length.
#[derive(Clone, Debug)]
pub struct BackgroundLayer {
    /// The layer's image.
    pub image: Image,
    /// `background-position`.
    pub position: Position,
    /// `background-size`.
    pub size: BgSize,
    /// `background-repeat`.
    pub repeat: RepeatStyle,
    /// `background-origin`: the box the image is positioned and sized against.
    pub origin: Keyword,
    /// `background-clip`: the box the painted result is clipped to.
    pub clip: Keyword,
    /// `background-blend-mode`, blending this layer with the ones below it.
    pub blend: Keyword,
}

impl ComputedStyle {
    /// Every painted background layer, top-first.
    ///
    /// `background-image: none` contributes no layer; `background-color` is not
    /// a layer at all -- it is one colour, painted once, behind everything.
    #[must_use]
    pub fn background_layers(&self) -> Vec<BackgroundLayer> {
        let images = comma_list(self.raw(Prop::BackgroundImage));
        let positions = comma_list(self.raw(Prop::BackgroundPosition));
        let sizes = comma_list(self.raw(Prop::BackgroundSize));
        let repeats = comma_list(self.raw(Prop::BackgroundRepeat));
        let origins = comma_list(self.raw(Prop::BackgroundOrigin));
        let clips = comma_list(self.raw(Prop::BackgroundClip));
        let blends = comma_list(self.raw(Prop::BackgroundBlendMode));

        let mut layers = Vec::with_capacity(images.len());
        for (index, image) in images.iter().enumerate() {
            let Value::Image(image) = image else {
                continue;
            };
            if matches!(image, Image::None) {
                continue;
            }
            layers.push(BackgroundLayer {
                image: image.clone(),
                position: match cycle(&positions, index) {
                    Some(Value::Position(position)) => position.clone(),
                    _ => Position::center(),
                },
                size: match cycle(&sizes, index) {
                    Some(Value::BgSize(size)) => size.clone(),
                    _ => BgSize::Auto,
                },
                repeat: match cycle(&repeats, index) {
                    Some(Value::Repeat(repeat)) => *repeat,
                    _ => RepeatStyle {
                        x: Keyword::Repeat,
                        y: Keyword::Repeat,
                    },
                },
                origin: keyword_or(cycle(&origins, index), Keyword::PaddingBox),
                clip: keyword_or(cycle(&clips, index), Keyword::BorderBox),
                blend: keyword_or(cycle(&blends, index), Keyword::Normal),
            });
        }
        layers
    }

    /// The `box-shadow` list, first listed on top. Empty for `none`.
    #[must_use]
    pub fn box_shadows(&self) -> Rc<[Shadow]> {
        self.get::<Rc<[Shadow]>>(Prop::BoxShadow)
    }
}

/// A comma-multiplied value as a slice of its entries. A non-list value is one
/// entry; `none` is still one entry, and the caller decides what that means.
fn comma_list(value: &Value) -> Vec<&Value> {
    match value {
        Value::List(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// CSS's repeat-to-longest rule: shorter `background-*` lists cycle.
fn cycle<'a>(list: &[&'a Value], index: usize) -> Option<&'a Value> {
    if list.is_empty() {
        return None;
    }
    Some(list[index % list.len()])
}

fn keyword_or(value: Option<&Value>, fallback: Keyword) -> Keyword {
    match value {
        Some(Value::Keyword(keyword)) => *keyword,
        _ => fallback,
    }
}
```

Extend the imports with `RepeatStyle` (from `super::value`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: PASS.

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 6: Commit**

```bash
git add ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): layered backgrounds and box-shadow lists

background_layers() zips the seven background-* longhands into top-first
BackgroundLayers, cycling the shorter comma lists to the image list's length
and dropping `none` layers. box_shadows() returns the shadow list with its
colours already resolved. BackgroundLayer lives beside its only producer;
P4's paint module re-exports it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `anim::Overrides`, transition/animation specs, `with_overrides`

**Files:**
- Create: `ui/src/anim/mod.rs`
- Modify: `ui/src/lib.rs:8-15` (module list)
- Modify: `ui/src/css/computed.rs` (three more accessors)
- Modify: `ui/src/css/computed.rs` test module (4 tests)

**Interfaces:**
- Consumes (Task 3-6): `ComputedStyle`, `Prop`, `Value`, `FromValue`.
- Consumes (P1): `Time`, `TimingFunction`, `AnimationName`, `IterationCount`,
  `Keyword`, `registry::lookup`.
- Produces:
  ```rust
  // ui/src/anim/mod.rs
  #[derive(Clone, Debug, Default, PartialEq)]
  pub struct Overrides { /* Vec<(Prop, Value)>, sorted by Prop */ }
  impl Overrides {
      pub fn is_empty(&self) -> bool;
      pub fn get(&self, prop: Prop) -> Option<&Value>;
      pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)>;
      pub fn set(&mut self, prop: Prop, value: Value);
  }
  #[derive(Clone, Debug, PartialEq)]
  pub struct TransitionSpec { pub prop: Option<Prop>, pub all: bool, pub duration: Time,
                              pub delay: Time, pub timing: TimingFunction }
  #[derive(Clone, Debug, PartialEq)]
  pub struct AnimationSpec { pub name: AnimationName, pub duration: Time, pub delay: Time,
                             pub timing: TimingFunction, pub iterations: IterationCount,
                             pub direction: Keyword, pub fill: Keyword, pub play_state: Keyword }

  // ui/src/css/computed.rs
  impl ComputedStyle {
      pub fn transition_specs(&self) -> Vec<TransitionSpec>;
      pub fn animation_specs(&self) -> Vec<AnimationSpec>;
      pub fn with_overrides<'a>(&'a self, overrides: &Overrides) -> Cow<'a, ComputedStyle>;
  }
  ```
  P5 adds `anim/clock.rs`, `anim/transition.rs`, `anim/keyframes.rs` and
  `AnimationState` beside these three types without changing them.

- [ ] **Step 1: Write the failing tests**

Append to `computed.rs`'s test module:

```rust
    #[test]
    fn adwaitas_button_transition_flattens_to_one_spec() {
        // Adwaita animates buttons with a plain `transition: <time>`; the
        // property defaults to `all`.
        let sheet = CompiledSheet::compile("button { transition: 200ms ease-out 50ms }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.transition_specs();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].all);
        assert_eq!(specs[0].prop, None);
        assert_eq!(specs[0].duration, Time::from_ms(200.0));
        assert_eq!(specs[0].delay, Time::from_ms(50.0));
        assert_eq!(specs[0].timing, TimingFunction::EASE_OUT);
    }

    #[test]
    fn named_transition_properties_resolve_through_the_registry() {
        let sheet = CompiledSheet::compile(
            "button { transition-property: background-color, nosuch-property, color; \
             transition-duration: 100ms, 200ms }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.transition_specs();
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.prop)
                .collect::<Vec<_>>(),
            vec![Some(Prop::BackgroundColor), Some(Prop::Color)],
            "an unknown property name contributes no spec"
        );
        // Durations cycle, and the dropped entry does not shift the pairing:
        // `color` is the third property, so it takes the first duration again.
        assert_eq!(specs[0].duration, Time::from_ms(100.0));
        assert_eq!(specs[1].duration, Time::from_ms(100.0));
        assert!(
            resolve(
                &CompiledSheet::compile("button { transition-property: none }"),
                &button(&[], PseudoStates::default())
            )
            .transition_specs()
            .is_empty()
        );
    }

    #[test]
    fn animation_specs_carry_every_animation_longhand() {
        let sheet = CompiledSheet::compile(
            "button { animation: pulse 2s linear 1s infinite alternate both paused }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.animation_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].name,
            AnimationName::Named(std::rc::Rc::from("pulse"))
        );
        assert_eq!(specs[0].duration, Time(2.0));
        assert_eq!(specs[0].delay, Time(1.0));
        assert_eq!(specs[0].timing, TimingFunction::Linear);
        assert_eq!(specs[0].iterations, IterationCount::Infinite);
        assert_eq!(specs[0].direction, Keyword::Alternate);
        assert_eq!(specs[0].fill, Keyword::Both);
        assert_eq!(specs[0].play_state, Keyword::Paused);
        assert!(
            resolve(&adwaita(), &button(&[], PseudoStates::default()))
                .animation_specs()
                .is_empty(),
            "`animation-name: none` produces no spec"
        );
    }

    #[test]
    fn overrides_layer_over_the_computed_style_without_cloning_when_empty() {
        use crate::anim::Overrides;
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let empty = Overrides::default();
        let borrowed = style.with_overrides(&empty);
        assert!(matches!(borrowed, std::borrow::Cow::Borrowed(_)));

        let mut overrides = Overrides::default();
        overrides.set(Prop::Color, Value::Color(ColorValue::Absolute(
            Rgba::from_color32(Color(0xFF00_FF00)),
        )));
        // A later `set` of the same property wins: animation over transition.
        overrides.set(Prop::Color, Value::Color(ColorValue::Absolute(
            Rgba::from_color32(Color(0xFF00_00FF)),
        )));
        let animated = style.with_overrides(&overrides);
        assert!(matches!(animated, std::borrow::Cow::Owned(_)));
        assert_eq!(animated.color().to_color32(), Color(0xFF00_00FF));
        assert_eq!(
            style.color().to_color32(),
            Color(0xFF2E_3436),
            "the base style must not be mutated"
        );
        assert_eq!(overrides.iter().count(), 1);
        assert!(overrides.get(Prop::Opacity).is_none());
    }
```

Mutation checks:
- `adwaitas_button_transition_flattens_to_one_spec`: reading the four
  `transition-*` rows in the wrong order swaps duration and delay (`200ms`/`50ms`
  are deliberately different).
- `named_transition_properties_resolve_through_the_registry`: pairing durations by
  *output* index rather than by the property's index in its own list changes
  `specs[1].duration` to `200ms`; keeping the unknown name yields three specs.
- `animation_specs_carry_every_animation_longhand`: any two of the three keyword
  rows (direction/fill/play-state) swapped fails, since the three expected
  keywords are distinct.
- `overrides_layer_over_the_computed_style_without_cloning_when_empty`: cloning
  unconditionally fails the `Cow::Borrowed` assertion; a `set` that pushes instead
  of replacing makes `iter().count()` 2 and may leave green winning.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: FAIL to compile with `error[E0432]: unresolved import 'crate::anim'`.

- [ ] **Step 3: Create `ui/src/anim/mod.rs`**

```rust
//! Transitions and animations.
//!
//! P3 lands only the three types the computed style *produces*: the per-node
//! [`Overrides`] a sample is applied through, and the flattened
//! [`TransitionSpec`]/[`AnimationSpec`] the engine is driven by. The clock, the
//! transition/animation state machines and `AnimationState` are P5's.

use crate::css::registry::Prop;
use crate::css::value::{AnimationName, IterationCount, Keyword, Time, TimingFunction, Value};

/// Per-node animation output, sorted by [`Prop`], applied over the computed
/// style.
///
/// One slot per property: a later [`set`](Self::set) of the same property
/// replaces the earlier one, which is exactly the precedence rule (animation
/// overrides transition overrides the base cascade) expressed as write order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    entries: Vec<(Prop, Value)>,
}

impl Overrides {
    /// Whether nothing is being animated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The overriding value for `prop`, if any.
    #[must_use]
    pub fn get(&self, prop: Prop) -> Option<&Value> {
        self.entries
            .binary_search_by_key(&prop, |(entry, _)| *entry)
            .ok()
            .map(|index| &self.entries[index].1)
    }

    /// Every override, in `Prop` order.
    pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)> {
        self.entries.iter().map(|(prop, value)| (*prop, value))
    }

    /// Set `prop`'s overriding value, replacing any earlier one.
    pub fn set(&mut self, prop: Prop, value: Value) {
        match self
            .entries
            .binary_search_by_key(&prop, |(entry, _)| *entry)
        {
            Ok(index) => self.entries[index].1 = value,
            Err(index) => self.entries.insert(index, (prop, value)),
        }
    }
}

/// One `transition-*` list entry, flattened from the four longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec {
    /// The property this entry transitions, if it names one.
    pub prop: Option<Prop>,
    /// Whether this entry is `transition-property: all`.
    pub all: bool,
    /// `transition-duration`.
    pub duration: Time,
    /// `transition-delay`.
    pub delay: Time,
    /// `transition-timing-function`.
    pub timing: TimingFunction,
}

/// One `animation-*` list entry, flattened from the eight longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    /// `animation-name`.
    pub name: AnimationName,
    /// `animation-duration`.
    pub duration: Time,
    /// `animation-delay`.
    pub delay: Time,
    /// `animation-timing-function`.
    pub timing: TimingFunction,
    /// `animation-iteration-count`.
    pub iterations: IterationCount,
    /// `animation-direction`: `normal`/`reverse`/`alternate`/`alternate-reverse`.
    pub direction: Keyword,
    /// `animation-fill-mode`: `none`/`forwards`/`backwards`/`both`.
    pub fill: Keyword,
    /// `animation-play-state`: `running`/`paused`.
    pub play_state: Keyword,
}
```

Add `pub mod anim;` to `ui/src/lib.rs`'s module list, in alphabetical position
(before `pub mod app;`).

- [ ] **Step 4: Implement the three accessors**

Append to `ui/src/css/computed.rs`:

```rust
impl ComputedStyle {
    /// The `transition-*` longhands, flattened into one spec per entry of
    /// `transition-property`. An entry naming an unknown property, and
    /// `transition-property: none`, contribute nothing.
    #[must_use]
    pub fn transition_specs(&self) -> Vec<TransitionSpec> {
        let properties = comma_list(self.raw(Prop::TransitionProperty));
        let durations = comma_list(self.raw(Prop::TransitionDuration));
        let timings = comma_list(self.raw(Prop::TransitionTimingFunction));
        let delays = comma_list(self.raw(Prop::TransitionDelay));

        let mut specs = Vec::with_capacity(properties.len());
        for (index, property) in properties.iter().enumerate() {
            let (prop, all) = match property {
                Value::Keyword(Keyword::All) => (None, true),
                Value::Keyword(Keyword::None) => continue,
                Value::AnimationName(AnimationName::Named(name)) => {
                    match registry::lookup(name) {
                        Some(prop) if prop.is_longhand() => (Some(prop), false),
                        _ => {
                            tracing::debug!(%name, "transition-property names no longhand");
                            continue;
                        }
                    }
                }
                _ => continue,
            };
            specs.push(TransitionSpec {
                prop,
                all,
                duration: time_at(&durations, index),
                delay: time_at(&delays, index),
                timing: timing_at(&timings, index),
            });
        }
        specs
    }

    /// The `animation-*` longhands, flattened into one spec per entry of
    /// `animation-name`. `animation-name: none` contributes nothing.
    #[must_use]
    pub fn animation_specs(&self) -> Vec<AnimationSpec> {
        let names = comma_list(self.raw(Prop::AnimationName));
        let durations = comma_list(self.raw(Prop::AnimationDuration));
        let timings = comma_list(self.raw(Prop::AnimationTimingFunction));
        let delays = comma_list(self.raw(Prop::AnimationDelay));
        let iterations = comma_list(self.raw(Prop::AnimationIterationCount));
        let directions = comma_list(self.raw(Prop::AnimationDirection));
        let fills = comma_list(self.raw(Prop::AnimationFillMode));
        let states = comma_list(self.raw(Prop::AnimationPlayState));

        let mut specs = Vec::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            let Value::AnimationName(AnimationName::Named(name)) = name else {
                continue;
            };
            specs.push(AnimationSpec {
                name: AnimationName::Named(Rc::clone(name)),
                duration: time_at(&durations, index),
                delay: time_at(&delays, index),
                timing: timing_at(&timings, index),
                iterations: match cycle(&iterations, index) {
                    Some(value) => IterationCount::from_value(value),
                    None => IterationCount::Count(1.0),
                },
                direction: keyword_or(cycle(&directions, index), Keyword::Normal),
                fill: keyword_or(cycle(&fills, index), Keyword::None),
                play_state: keyword_or(cycle(&states, index), Keyword::Running),
            });
        }
        specs
    }

    /// This style with `overrides` layered on top. Borrowed unchanged when
    /// `overrides` is empty -- the common case, every frame nothing animates.
    #[must_use]
    pub fn with_overrides<'a>(&'a self, overrides: &Overrides) -> Cow<'a, ComputedStyle> {
        if overrides.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut style = self.clone();
        for (prop, value) in overrides.iter() {
            style.values[prop.slot()] = value.clone();
        }
        style.derive_m1_view();
        Cow::Owned(style)
    }
}

fn time_at(list: &[&Value], index: usize) -> Time {
    match cycle(list, index) {
        Some(value) => Time::from_value(value),
        None => Time(0.0),
    }
}

fn timing_at(list: &[&Value], index: usize) -> TimingFunction {
    match cycle(list, index) {
        Some(value) => TimingFunction::from_value(value),
        None => TimingFunction::EASE,
    }
}
```

Extend `computed.rs`'s imports with:

```rust
use std::borrow::Cow;

use crate::anim::{AnimationSpec, Overrides, TransitionSpec};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib`
Expected: PASS.

- [ ] **Step 6: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 7: Commit**

```bash
git add ui/src/anim/mod.rs ui/src/lib.rs ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
feat(ui/anim): Overrides plus flattened transition and animation specs

The three §6 types the computed style produces land now, so P5 can build the
clock and the state machines on top without redefining them: Overrides (one
slot per Prop, later set wins, which is animation-over-transition), and
TransitionSpec/AnimationSpec flattened from the twelve transition-*/animation-*
longhands with the comma lists cycled. ComputedStyle::with_overrides borrows
unchanged when nothing is animating.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `Button` becomes behaviour over a `Node`

**Files:**
- Modify: `ui/src/widget/button.rs:1-176` (the whole widget)
- Modify: `ui/src/widget/button.rs:178-292` (its 4 tests)

**Interfaces:**
- Consumes (Task 3-6): `ComputedStyle::{resolve, resolve_chain, initial, color,
  font_size_px, border_widths, border_colors}`, `ResolveEnv`.
- Consumes (P2): `Node`, `Node::with_classes`, `Node::append_child`,
  `Node::set_states`, `Node::states`, `Node::parent`, `PseudoStates`, `MatchCx`.
- Consumes (M1, unchanged until P4): `ButtonLayout`, `Allocation`,
  `paint::paint_button`, `FontStack`, `ShapedText`.
- Produces:
  ```rust
  impl Button {
      pub fn new(label: &str, classes: &[&str], parent: Node) -> Self;
      pub fn node(&self) -> &Node;
      pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack);
      pub fn set_states(&mut self, states: PseudoStates, sheet: &CompiledSheet, fonts: &FontStack);
      pub fn set_label(&mut self, label: &str, sheet: &CompiledSheet, fonts: &FontStack);
      pub fn label(&self) -> &str;
      pub fn states(&self) -> PseudoStates;
      pub fn style(&self) -> &ComputedStyle;
      pub fn allocation(&self) -> Allocation;
      pub fn render(&self, surface: &mut Surface, origin: (f32, f32));
      pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Replace `ui/src/widget/button.rs`'s test module with the four M1 behaviours,
re-expressed against `Node` and the accessors (every asserted number unchanged),
plus one new test for the node identity:

```rust
#[cfg(test)]
mod tests {
    use super::Button;
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::css::cascade::CompiledSheet;
    use crate::css::node::{Node, PseudoStates};
    use crate::text::FontStack;
    use skia_rs_safe::core::Color;

    fn fixture(css: &str, label: &str) -> (CompiledSheet, FontStack, Button) {
        let sheet = CompiledSheet::compile(css);
        let fonts = FontStack::system().expect("system font");
        let window = Node::with_classes("window", &["background"]);
        let mut button = Button::new(label, &[], window);
        button.restyle(&sheet, &fonts);
        (sheet, fonts, button)
    }

    #[test]
    fn the_shaped_label_survives_a_state_change_but_not_a_font_size_change() {
        // A8: the label was shaped three times per paint, each with a fresh
        // font. It is now shaped once per (label, size) -- but the cache
        // must not outlive either of those changing.
        let css = "window { background-color: #fff }\n\
                   button { font-size: 14px }\n\
                   button:hover { font-size: 30px }";
        let (sheet, fonts, mut button) = fixture(css, "Click me");
        let narrow = button.allocation().width;
        assert_eq!(button.style().font_size_px(), 14.0);

        button.set_states(PseudoStates::HOVER, &sheet, &fonts);
        assert_eq!(button.style().font_size_px(), 30.0);
        assert!(
            button.allocation().width > narrow,
            "the label was not reshaped at the new font size: {} vs {narrow}",
            button.allocation().width
        );
    }

    #[test]
    fn setting_the_label_reshapes_it() {
        let (sheet, fonts, mut button) = fixture(BUNDLED_ADWAITA_LIGHT, "Click me");
        let before = button.allocation().width;

        button.set_label("Click me twice over", &sheet, &fonts);
        assert_eq!(button.label(), "Click me twice over");
        assert!(
            button.allocation().width > before,
            "the new label reused the old shaping: {} vs {before}",
            button.allocation().width
        );

        button.set_label("", &sheet, &fonts);
        // An empty Adwaita button is its content-box minimum plus its frame.
        assert_eq!(button.allocation().width, 36.0);
    }

    #[test]
    fn the_cached_parent_style_matches_a_full_ancestor_walk() {
        // The window's `color` must still reach the button through the
        // cached parent style, on the first restyle and every one after.
        let css = "window { color: #ff0000 }\nbutton:hover { border-width: 3px }";
        let (sheet, fonts, mut button) = fixture(css, "x");
        assert_eq!(button.style().color().to_color32(), Color(0xFFFF_0000));

        button.set_states(PseudoStates::HOVER, &sheet, &fonts);
        assert_eq!(
            button.style().color().to_color32(),
            Color(0xFFFF_0000),
            "inheritance was lost once the parent style came from the cache"
        );
        assert_eq!(button.style().border_widths(), [3.0; 4]);
    }

    #[test]
    fn a_different_sheet_re_derives_the_cached_parent_style() {
        // The parent-style cache used to key on nothing but "have we ever
        // computed it", so swapping in a new `CompiledSheet` (a live theme
        // reload) reused the old ancestor cascade. Key it on the sheet's
        // identity instead: a border painted with `currentColor` must pick
        // up the new sheet's `window { color: ... }`.
        let red_css = "window { color: red }\nbutton { border-color: currentColor }";
        let blue_css = "window { color: blue }\nbutton { border-color: currentColor }";
        let red_sheet = CompiledSheet::compile(red_css);
        let blue_sheet = CompiledSheet::compile(blue_css);
        let fonts = FontStack::system().expect("system font");

        let window = Node::with_classes("window", &["background"]);
        let mut button = Button::new("x", &[], window);

        button.restyle(&red_sheet, &fonts);
        let red_border = button.style().border_colors()[0];

        button.restyle(&blue_sheet, &fonts);
        let blue_border = button.style().border_colors()[0];

        assert_ne!(
            red_border, blue_border,
            "the cached parent style survived a sheet swap"
        );
    }

    #[test]
    fn the_button_is_a_real_child_of_its_parent_node() {
        // M1's `CssNode` was an immutable parent *pointer*: the window had no
        // children, so `:only-child`, `:first-child` and `:nth-child` were
        // hardcoded. The widget now attaches to a real tree.
        let (_sheet, _fonts, button) = fixture("button { color: red }", "x");
        let parent = button.node().parent().expect("the button has a parent");
        assert_eq!(parent.child_count(), 1);
        assert!(parent.child(0).expect("first child").ptr_eq(button.node()));
        assert_eq!(button.node().index_in_parent(), Some(0));
    }
}
```

Mutation checks:
- `the_shaped_label_survives_a_state_change_but_not_a_font_size_change`: dropping
  the font-size term from the shaping cache key leaves the width unchanged.
- `setting_the_label_reshapes_it`: not clearing `shaped` in `set_label` keeps the
  old width; the `36.0` pins the content-box minimum.
- `the_cached_parent_style_matches_a_full_ancestor_walk`: resolving the button
  with `parent: None` loses the inherited red.
- `a_different_sheet_re_derives_the_cached_parent_style`: dropping the
  `parent_style_sheet` identity key makes both borders red.
- `the_button_is_a_real_child_of_its_parent_node`: creating the node without
  `append_child` leaves `child_count() == 0`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib widget::button`
Expected: FAIL to compile with `error[E0599]: no method named 'node' found for
struct 'Button'` and `no method named 'font_size_px'` on the M1 field access path.

- [ ] **Step 3: Implement `Button` over `Node`**

Replace `ui/src/widget/button.rs:1-176` with:

```rust
//! The M1 widget, now a behaviour over a `css::node::Node`.
//!
//! The node is a real child of its parent, so sibling and child selectors
//! (`:first-child`, `:only-child`, `:nth-child`) see the tree GTK's own theme
//! expects. The widget owns the two things a restyle needs and a caller should
//! not have to thread: a long-lived [`MatchCx`] (which keeps the selector
//! bloom filter and nth-index caches warm across states) and the
//! [`ResolveEnv`] its styles resolve against.

use skia_rs_safe::canvas::Surface;

use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::{Node, PseudoStates};
use crate::css::select::MatchCx;
use crate::layout::{Allocation, ButtonLayout};
use crate::paint::paint_button;
use crate::text::{FontStack, ShapedText};

/// A themed button: CSS node identity, interaction state, computed style
/// and allocation, kept together and recomputed on every state change.
///
/// Three things are *cached* across restyles rather than rebuilt, because a
/// restyle happens on every `:hover`/`:active` transition:
///
/// * the shaped label, keyed by `(label, font-size, font-stack identity)`;
/// * the parent's computed style, keyed by the sheet's identity;
/// * the `taffy` tree and the `MatchCx`.
pub struct Button {
    label: String,
    node: Node,
    style: ComputedStyle,
    allocation: Allocation,
    /// The label shaped at `shaped_size`; invalidated when either changes.
    shaped: Option<ShapedText>,
    /// The font size `shaped` was shaped at.
    shaped_size: f32,
    /// Identity of the [`FontStack`] `shaped` was shaped with (its address,
    /// as `usize`), so a swapped font stack invalidates the shaping cache
    /// even if the label and font size happen to match.
    shaped_font: Option<usize>,
    /// The parent's computed style: fixed for this widget's lifetime, as long
    /// as the sheet it was cascaded from doesn't change.
    parent_style: Option<ComputedStyle>,
    /// Identity of the [`CompiledSheet`] `parent_style` was cascaded from
    /// (its address, as `usize`). A cheap key, not a hash: it only needs to
    /// notice *some other sheet is now in play*, which a swapped
    /// `CompiledSheet` (e.g. a live theme reload) always is.
    parent_style_sheet: Option<usize>,
    layout: ButtonLayout,
    /// Reused across restyles: the selector caches stay warm.
    cx: MatchCx,
    env: ResolveEnv,
}

/// The identity key stored alongside a per-sheet or per-font-stack cache:
/// the referent's address. Cheap, and sufficient to detect "a different
/// sheet/stack is now in play" — the only thing these caches need to know.
fn identity<T>(value: &T) -> usize {
    std::ptr::from_ref(value) as usize
}

impl Button {
    /// Create a `button` node with `classes` and attach it to `parent`.
    ///
    /// The style and allocation are the initial values until
    /// [`restyle`](Self::restyle) runs.
    #[must_use]
    pub fn new(label: &str, classes: &[&str], parent: Node) -> Self {
        let env = ResolveEnv::default();
        let node = Node::with_classes("button", classes);
        parent.append_child(&node);
        Self {
            label: label.to_string(),
            node,
            style: (*ComputedStyle::initial(&env)).clone(),
            allocation: Allocation {
                width: 0.0,
                height: 0.0,
                label_x: 0.0,
                label_y: 0.0,
            },
            shaped: None,
            shaped_size: f32::NAN,
            shaped_font: None,
            parent_style: None,
            parent_style_sheet: None,
            layout: ButtonLayout::new(),
            cx: MatchCx::new(),
            env,
        }
    }

    /// This widget's CSS node.
    #[must_use]
    pub fn node(&self) -> &Node {
        &self.node
    }

    /// Re-run cascade, measurement and layout for the current state.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack) {
        let sheet_id = identity(sheet);
        if self.parent_style.is_none() || self.parent_style_sheet != Some(sheet_id) {
            // The ancestors' own cascade cannot change while this widget
            // lives *and the sheet stays the same*, so resolve the chain
            // once per sheet and thread it in from here.
            self.parent_style = Some(match self.node.parent() {
                Some(parent) => {
                    ComputedStyle::resolve_chain(sheet, &parent, &self.env, &mut self.cx)
                }
                None => (*ComputedStyle::initial(&self.env)).clone(),
            });
            self.parent_style_sheet = Some(sheet_id);
        }
        self.style = ComputedStyle::resolve(
            sheet,
            &self.node,
            self.parent_style.as_ref(),
            &self.env,
            &mut self.cx,
        );

        // `!=` rather than an epsilon: the only thing that ever writes this
        // is a previous shape at exactly this size, and NAN != NAN makes the
        // first call always shape.
        let font_id = identity(fonts);
        let font_size = self.style.font_size_px();
        #[allow(clippy::float_cmp)]
        if self.shaped.is_none() || self.shaped_size != font_size || self.shaped_font != Some(font_id)
        {
            self.shaped = Some(fonts.shape(&self.label, font_size));
            self.shaped_size = font_size;
            self.shaped_font = Some(font_id);
        }
        let metrics = self.shaped.as_ref().expect("just shaped").metrics;
        self.allocation = self.layout.compute(&self.style, &metrics);
    }

    /// Replace the pseudo-class state and restyle.
    pub fn set_states(&mut self, states: PseudoStates, sheet: &CompiledSheet, fonts: &FontStack) {
        self.node.set_states(states);
        self.restyle(sheet, fonts);
    }

    /// Replace the label and restyle, reshaping it.
    pub fn set_label(&mut self, label: &str, sheet: &CompiledSheet, fonts: &FontStack) {
        if self.label == label {
            return;
        }
        self.label = label.to_string();
        self.shaped = None;
        self.restyle(sheet, fonts);
    }

    /// The current label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The current pseudo-class state.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        self.node.states()
    }

    /// The computed style from the last [`restyle`](Self::restyle).
    #[must_use]
    pub fn style(&self) -> &ComputedStyle {
        &self.style
    }

    /// The allocation from the last [`restyle`](Self::restyle).
    #[must_use]
    pub fn allocation(&self) -> Allocation {
        self.allocation
    }

    /// Paint this button at `origin` on `surface`.
    pub fn render(&self, surface: &mut Surface, origin: (f32, f32)) {
        paint_button(
            surface,
            origin,
            &self.style,
            &self.allocation,
            self.shaped.as_ref(),
        );
    }

    /// Whether surface point `(x, y)` falls inside this button's border box
    /// when the button is drawn at `origin`. Rectangular, not radius-aware:
    /// GTK's own hit testing is rectangular too.
    #[must_use]
    pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool {
        let (ox, oy) = (f64::from(origin.0), f64::from(origin.1));
        x >= ox
            && y >= oy
            && x < ox + f64::from(self.allocation.width)
            && y < oy + f64::from(self.allocation.height)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib widget::button`
Expected: PASS (5 tests).

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean, `wayland.rs`'s 11 tests and the offscreen gate
included.

- [ ] **Step 6: Commit**

```bash
git add ui/src/widget/button.rs
git commit -m "$(cat <<'EOF'
refactor(ui/widget): Button is behaviour over a real css::node::Node

The button attaches itself to its parent as a real child, so GTK's sibling
and child selectors see the tree they expect, and it owns the long-lived
MatchCx and ResolveEnv a restyle needs. Style reads go through the typed
accessors. All three M1 cache-invalidation regressions -- shaped label,
parent style, per-sheet identity -- are preserved as behaviours.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `app.rs` fixtures on the new engine

**Files:**
- Modify: `ui/src/app.rs:222-226` (`button_node`)
- Modify: `ui/src/app.rs:278-492` (the test module: 8 of 11 tests)

**Interfaces:**
- Consumes (Task 3-5, 8): `ComputedStyle::resolve_chain`, `ResolveEnv`,
  `min_size`, `border_colors`, `padding`, `Node`, `MatchCx`, `Button::new`.
- Produces: no new public API. `load_layered_stylesheet`, `compile_theme`,
  `themed_button`, `themed_button_allocation`, `run_themed_button`, `ThemeSource`
  and `ThemeEnv` keep their M1 signatures exactly.

- [ ] **Step 1: Write the failing tests**

Replace the two helpers at `ui/src/app.rs:291-301` and the eight tests that use
them. The three that touch no `ComputedStyle`
(`an_empty_xdg_config_home_falls_back_to_home_dot_config`,
`no_home_and_no_xdg_means_no_override_path`, `theme_dirs_are_searched_in_gtk_order`)
are byte-identical and must not be edited at all.

```rust
    fn button_style(sheet: &crate::css::cascade::CompiledSheet) -> ComputedStyle {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        ComputedStyle::resolve_chain(sheet, &button, &ResolveEnv::default(), &mut MatchCx::new())
    }

    fn button_min_height(sheet: &crate::css::cascade::CompiledSheet) -> f32 {
        button_style(sheet).min_size((0.0, 0.0)).1
    }

    fn headerbar_min_height(sheet: &crate::css::cascade::CompiledSheet) -> f32 {
        let window = Node::with_classes("window", &["background"]);
        let headerbar = Node::new("headerbar");
        window.append_child(&headerbar);
        ComputedStyle::resolve_chain(sheet, &headerbar, &ResolveEnv::default(), &mut MatchCx::new())
            .min_size((0.0, 0.0))
            .1
    }
```

with the imports:

```rust
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
```

Then, in the eight tests, `button_style(&sheet).min_height` becomes
`button_min_height(&sheet)` and `style.border_color` becomes
`style.border_colors()[0].to_color32()`. Every number is unchanged except the one
documented in Contract deviation 4. The two that change more than one line:

```rust
    #[test]
    fn a_user_override_layers_over_the_theme_instead_of_replacing_it() {
        // The reviewer's A1 scenario: `$XDG_CONFIG_HOME/gtk-4.0/gtk.css` is
        // GTK's priority-800 *override*, not the whole sheet. A file that
        // only restyles `headerbar` must leave Adwaita's button intact.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join("config/gtk-4.0/gtk.css"),
            "headerbar { min-height: 32px }\n",
        );
        let env = ThemeEnv {
            xdg_config_home: Some(tmp.path().join("config").display().to_string()),
            ..ThemeEnv::default()
        };

        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        let style = button_style(&sheet);
        assert_eq!(
            style.border_colors()[0].to_color32(),
            Color(0xFFCD_C7C2),
            "the override replaced the base theme instead of layering onto it"
        );
        assert_eq!(style.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(
            headerbar_min_height(&sheet),
            32.0,
            "the user's own override never reached the cascade"
        );
    }

    #[test]
    fn an_explicit_theme_file_is_a_whole_theme_not_an_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("only.css");
        std::fs::write(&path, "button { min-height: 13px }\n").expect("write");
        let sheet = compile_theme(&ThemeSource::File(path));
        assert_eq!(button_min_height(&sheet), 13.0);
        let border = button_style(&sheet).border_colors()[0].to_color32();
        // M2's initial `border-color` is `currentColor`, which resolves to the
        // initial `color` -- opaque black. (M1 had no initial and defaulted to
        // transparent.) What the test is really pinning is that Adwaita's
        // #cdc7c2 never got layered underneath.
        assert_eq!(border, Color(0xFF00_0000));
        assert_ne!(
            border,
            Color(0xFFCD_C7C2),
            "an explicit file must not be layered onto Adwaita"
        );
    }

    #[test]
    fn the_bundled_source_is_exactly_the_vendored_sheet() {
        let sheet = compile_theme(&ThemeSource::Bundled);
        assert_eq!(
            button_style(&sheet).border_colors()[0].to_color32(),
            Color(0xFFCD_C7C2)
        );
        assert_eq!(sheet.rules.len(), 900);
    }
```

The remaining five (`the_override_wins_ties_against_the_base_theme` → `41.0`,
`the_override_can_import_relative_to_its_own_directory` → `43.0`,
`a_dark_gtk_theme_picks_the_dark_variant_file` → `77.0`,
`a_light_gtk_theme_picks_the_plain_file` → `11.0`,
`a_dark_theme_without_a_dark_file_falls_back_to_gtk_css` → `11.0`) change only
`button_style(&sheet).min_height` to `button_min_height(&sheet)`.

Finally, `button_node` (production code) attaches the node:

```rust
/// The `window > button` node tree the themed-button binary styles.
fn button_node(label: &str, classes: &[&str]) -> Button {
    let window = Node::with_classes("window", &["background"]);
    Button::new(label, classes, window)
}
```

with `use crate::css::node::Node;` replacing `use crate::css::select::{CssNode,
PseudoStates};` at `ui/src/app.rs:15`.

Mutation checks:
- `a_user_override_layers_over_the_theme_instead_of_replacing_it`: replacing
  rather than layering drops `#cdc7c2` and the padding.
- `an_explicit_theme_file_is_a_whole_theme_not_an_override`: layering Adwaita
  underneath makes the border `#cdc7c2` and trips the `assert_ne!`.
- `the_bundled_source_is_exactly_the_vendored_sheet`: any regression in selector
  compilation moves `900`.
- The five `min_height` tests: reading `min-width` instead of `min-height`, or
  taking the border-box rather than the content-box minimum, changes each number.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib app`
Expected: FAIL to compile with `error[E0609]: no field 'min_height' on type
'ComputedStyle'` (once Task 10 removes it) or, before that,
`error[E0425]: cannot find function 'button_min_height'`.

- [ ] **Step 3: Apply the edits above**

There is no new implementation code in this task: `app.rs`'s production surface
is unchanged apart from `button_node`. Make the fixture edits exactly as written.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib app`
Expected: PASS (11 tests).

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 6: Commit**

```bash
git add ui/src/app.rs
git commit -m "$(cat <<'EOF'
test(ui/app): theme-stack fixtures on the M2 engine

The eight theme-stack tests that read a ComputedStyle field now go through
resolve_chain and the typed accessors; the three that only exercise path
discovery are untouched. Every number is preserved except the two border
colours, which move from M1's transparent default to M2's initial
currentColor (opaque black) -- the layering each test actually pins is
asserted explicitly instead.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: retire the M1 view — layout, paint and the offscreen gate

**Files:**
- Modify: `ui/src/css/computed.rs` (delete the ten M1 fields, `derive_m1_view`,
  `m1_bridge`, `Background`, `GradientStop`, `BackgroundClip`, `lerp_u8`; retarget
  `computed.rs`'s remaining M1 tests)
- Modify: `ui/src/layout.rs:93-137` (read the accessors)
- Modify: `ui/src/paint.rs:1-139` (read the accessors; local gradient sampler)
- Modify: `ui/tests/themed_button_offscreen.rs` (mechanical; **numbers frozen**)

**Interfaces:**
- Consumes: every accessor from Tasks 4-6, `Image`, `ColorStop`, `ColorValue`,
  `Keyword`, `Rgba`.
- Produces: `ComputedStyle` with **no** public fields — the table and its
  accessors are the whole surface. `paint::paint_button` and
  `layout::{Allocation, ButtonLayout, layout_button}` keep their M1 signatures so
  P4 replaces them on its own schedule.

- [ ] **Step 1: Write the failing tests**

Rewrite the M1 field assertions in `ui/tests/themed_button_offscreen.rs`. Replace
its header helpers (lines 10-27) with:

```rust
use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::ComputedStyle;
use icedtea_ui::css::node::{Node, PseudoStates};
use icedtea_ui::css::registry::Prop;
use icedtea_ui::css::value::color::{ColorValue, Rgba};
use icedtea_ui::css::value::{Image, Keyword, Length};
use icedtea_ui::text::FontStack;
use icedtea_ui::widget::button::Button;
use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;

const SURFACE_W: i32 = 240;
const SURFACE_H: i32 = 80;

/// The topmost background layer's gradient stops, as `(colour, position px)`.
fn gradient_stops(style: &ComputedStyle) -> Vec<(Color, Option<f32>)> {
    let layers = style.background_layers();
    let Some(Image::Gradient(gradient)) = layers.first().map(|layer| layer.image.clone()) else {
        panic!("the topmost background layer is not a gradient: {layers:?}");
    };
    gradient
        .stops
        .iter()
        .map(|stop| {
            let ColorValue::Absolute(rgba) = &stop.color else {
                panic!("a computed gradient stop must carry a resolved colour");
            };
            let position = stop.position.as_ref().map(|length| match length {
                Length::Abs { value, .. } => *value,
                other => panic!("a computed stop position must be absolute: {other:?}"),
            });
            (rgba.to_color32(), position)
        })
        .collect()
}

/// The topmost background layer as a flat fill -- GTK's `image(<color>)`.
fn solid_background(style: &ComputedStyle) -> Color {
    let layers = style.background_layers();
    let Some(Image::Solid(ColorValue::Absolute(rgba))) =
        layers.first().map(|layer| layer.image.clone())
    else {
        panic!("the topmost background layer is not a flat fill: {layers:?}");
    };
    rgba.to_color32()
}

/// A window > button node tree, a compiled Adwaita sheet and a system font.
fn labelled_fixture(label: &str, classes: &[&str]) -> (CompiledSheet, FontStack, Button) {
    let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
    let fonts =
        FontStack::system().expect("no system font found; install dejavu/liberation/noto sans");
    let window = Node::with_classes("window", &["background"]);
    let mut button = Button::new(label, classes, window);
    button.restyle(&sheet, &fonts);
    (sheet, fonts, button)
}
```

Then, in `adwaita_button_computed_style_and_pixels_match_the_theme`, replace only
the style reads. **Every literal below is copied from the M1 file unchanged:**

```rust
    let normal = button.style().clone();
    assert_eq!(
        gradient_stops(&normal),
        vec![(Color(0xFFF6_F5F4), Some(2.0)), (Color(0xFFFB_FAFA), None)],
        "Adwaita's base button background did not resolve"
    );
    assert_eq!(normal.color().to_color32(), Color(0xFF2E_3436));
    assert_eq!(normal.border_colors()[0].to_color32(), Color(0xFFCD_C7C2));
    assert_eq!(normal.border_widths(), [1.0; 4]);
    assert_eq!(normal.border_radii(78.0, 34.0), [[5.0, 5.0]; 4]);
    assert_eq!(normal.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
    assert_eq!(normal.min_size((0.0, 0.0)), (16.0, 24.0));
    assert_eq!(normal.font_size_px(), 14.0);
    // Adwaita sets no `background-clip` on `button`, so the background fills
    // the border box -- CSS's and GTK's default. The opaque #cdc7c2 border
    // is painted over it, which is why the border-pixel assertion below
    // still reads the border colour and the corner outside the radius is
    // still transparent.
    assert_eq!(normal.background_layers()[0].clip, Keyword::BorderBox);
```

The `:hover` and `:active` blocks become:

```rust
    button.set_states(PseudoStates::HOVER, &sheet, &fonts);
    let hovered = button.style().clone();
    assert_eq!(
        gradient_stops(&hovered),
        vec![(Color(0xFFD6_D1CD), None), (Color(0xFFE8_E6E3), Some(1.0))],
        "`button:hover` did not win the cascade"
    );
    assert_ne!(gradient_stops(&hovered), gradient_stops(&normal));
```

```rust
    button.set_states(PseudoStates::ACTIVE, &sheet, &fonts);
    assert_eq!(
        solid_background(button.style()),
        Color(0xFFDA_D6D2),
        "`button:active`'s image(#dad6d2) did not resolve to a flat fill"
    );
```

and `suggested_action_button_is_adwaitas_accent_blue`:

```rust
    let style = button.style().clone();
    assert_eq!(
        gradient_stops(&style),
        vec![(Color(0xFF2C_7FE3), Some(2.0)), (Color(0xFF35_84E4), None)]
    );
    assert_eq!(
        style.color().to_color32(),
        Color(0xFFFF_FFFF),
        "suggested-action text is white"
    );
    assert_eq!(style.border_colors()[0].to_color32(), Color(0xFF15_539E));
```

Every pixel assertion, every `allocation` assertion, `bx = 4`, the `± 10` close
check, `36.0`/`34.0`, and the `dark > 20` count stay **exactly** as they are.

Add one new in-crate test to `computed.rs`'s module proving the field surface is
gone:

```rust
    #[test]
    fn the_computed_style_has_no_public_fields_left() {
        // A compile-time assertion in test form: the only way in is the table.
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let cloned = style.clone();
        assert_eq!(style, cloned);
        assert_eq!(style.raw(Prop::MinHeight), &Value::Length(Length::px(24.0)));
    }
```

Mutation checks:
- `adwaita_button_computed_style_and_pixels_match_the_theme`: this is the M1 gate
  in full — any accessor that reads the wrong slot, any gradient sampled from the
  wrong edge, or any clip regression moves a pinned byte.
- `the_computed_style_has_no_public_fields_left`: re-adding a public field does
  not fail this test, but re-adding one *and* leaving it stale does — the file's
  other 25 tests all read the table.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p icedtea-ui --test themed_button_offscreen`
Expected: FAIL to compile with `error[E0432]: unresolved import
'icedtea_ui::css::computed::Background'`.

- [ ] **Step 3: Delete the M1 view from `computed.rs`**

- Delete `GradientStop`, `Background` (and its `color_at`), `BackgroundClip`,
  `lerp_u8`, the whole `m1_bridge` module and `ComputedStyle::derive_m1_view`.
- Delete the ten public fields from `ComputedStyle`, leaving
  `pub struct ComputedStyle { values: Box<[Value; N_LONGHANDS]> }`.
- `initial(env)` and `resolve(..)` construct `Self { values }` and drop the
  `derive_m1_view()` calls; `with_overrides` drops its call too.
- Retarget `computed.rs`'s remaining M1 tests onto the accessors, keeping every
  number: `s.background` → `s.background_layers()` / `s.get::<Rgba>(Prop::BackgroundColor)`,
  `s.color` → `s.color().to_color32()`, `s.border_width` → `s.border_widths()[0]`,
  `s.border_color` → `s.border_colors()[0].to_color32()`, `s.border_radius` →
  `s.border_radii(0.0, 0.0)[0][0]`, `s.padding` → `s.padding(0.0)`,
  `s.min_width`/`s.min_height` → `s.min_size((0.0, 0.0))`, `s.font_size` →
  `s.font_size_px()`, `s.background_clip` → `s.background_layers()[0].clip`.
  The gradient-sampling test (`gradient_sampling_follows_to_top_semantics`) moves
  to `paint.rs` beside the sampler it now tests, with its four expected colours
  (`0xFFD6D1CD`, `0xFFDFDCD8`, `0xFFE8E6E3`, `0xFFF6F5F4`) unchanged.

- [ ] **Step 4: Move layout onto the accessors**

In `ui/src/layout.rs`, replace the first two lines of `ButtonLayout::compute`
(`layout.rs:94-95`) with:

```rust
        let [pad_top, pad_right, pad_bottom, pad_left] = style.padding(0.0);
        // M1 paints one uniform border; the top side is the one taffy is given.
        // P4 replaces this whole module with a per-side `LayoutTree`.
        let border = style.border_widths()[0];
        let (min_width, min_height) = style.min_size((0.0, 0.0));
```

and use `min_width`/`min_height` in the `min_size` block in place of
`style.min_width`/`style.min_height`. Nothing else in the file changes.

- [ ] **Step 5: Move paint onto the accessors**

In `ui/src/paint.rs`, replace `paint_button`'s style reads (`paint.rs:29-102`)
with the block below. The band loop, the clip boxes and the border stroke are
M1's, unchanged; only where the numbers come from moves.

```rust
    let (ox, oy) = origin;
    let radius = style.border_radii(allocation.width, allocation.height)[0][0];
    let border = style.border_widths()[0];
    let fill = Fill::of(style);

    // `background-origin` stays at its CSS default (padding-box): that is
    // the box a gradient is *sized* against. `background-clip` only decides
    // how much of the result survives, so widening the clip to the border
    // box fills under the border without restretching the gradient.
    let origin_box = Rect::from_xywh(
        ox + border,
        oy + border,
        (allocation.width - border * 2.0).max(0.0),
        (allocation.height - border * 2.0).max(0.0),
    );
    let origin_height = origin_box.bottom - origin_box.top;

    let (clip, clip_radius) = match style.get::<Keyword>(Prop::BackgroundClip) {
        Keyword::PaddingBox => (origin_box, (radius - border).max(0.0)),
        Keyword::ContentBox => {
            let [top, right, bottom, left] = style.padding(allocation.width);
            (
                Rect::from_xywh(
                    origin_box.left + left,
                    origin_box.top + top,
                    (origin_box.right - origin_box.left - left - right).max(0.0),
                    (origin_box.bottom - origin_box.top - top - bottom).max(0.0),
                ),
                (radius - border - top.max(left)).max(0.0),
            )
        }
        // `border-box` and anything else: the whole element.
        _ => (
            Rect::from_xywh(ox, oy, allocation.width, allocation.height),
            radius,
        ),
    };

    match fill {
        Fill::None => {}
        Fill::Solid(color) => {
            let mut paint = Paint::new();
            paint.set_color32(color);
            paint.set_style(Style::Fill);
            paint.set_anti_alias(true);
            surface
                .canvas()
                .draw_round_rect(&clip, clip_radius, clip_radius, &paint);
        }
        Fill::ToTop { .. } => {
            let mut canvas = surface.canvas();
            let save = canvas.save();
            // Clip to the rounded background-clip box, then fill 1px bands.
            let mut path = PathBuilder::new();
            path.add_round_rect(&clip, clip_radius, clip_radius);
            canvas.clip_path(&path.build(), ClipOp::Intersect, true);
            let rows = (clip.bottom - clip.top).ceil() as i32;
            for row in 0..rows {
                let y = clip.top + row as f32;
                // Sampled against the origin box, not the clip box: rows
                // outside it clamp to the end stops, as CSS requires.
                let color = fill.color_at(origin_height, y - origin_box.top + 0.5);
                let mut paint = Paint::new();
                paint.set_color32(color);
                paint.set_style(Style::Fill);
                paint.set_anti_alias(false);
                canvas.draw_rect(
                    &Rect::new(clip.left, y, clip.right, (y + 1.0).min(clip.bottom)),
                    &paint,
                );
            }
            canvas.restore_to_count(save);
        }
    }
```

The border block reads `style.border_colors()[0].to_color32()` instead of
`style.border_color`; the label block reads `style.color().to_color32()` instead
of `style.color`. Add above `paint_button`:

```rust
/// The one background shape this module paints.
///
/// A **P3 shim**: it recognises exactly what M1's `Background` did -- a flat
/// fill and a two-stop `linear-gradient(to top, ...)` -- out of the computed
/// background layers, so the offscreen gate's pinned bytes are reproduced
/// without depending on the general gradient sampler. P4 deletes it along with
/// this whole module.
enum Fill {
    None,
    Solid(Color),
    ToTop {
        from: (Color, f32),
        to: (Color, Option<f32>),
    },
}

/// Round-to-nearest linear interpolation between two bytes.
///
/// Deliberately *not* `Color4f::lerp`: that round-trips each channel through
/// `x / 255.0` and back, so it is not bit-identical to rounding the byte-space
/// interpolation once. The offscreen gate asserts exact pixel equality against
/// values derived straight from the theme's declarations.
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = f32::from(a);
    let b = f32::from(b);
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

impl Fill {
    /// The topmost background layer, or the background colour behind it.
    fn of(style: &ComputedStyle) -> Self {
        let layers = style.background_layers();
        if let Some(layer) = layers.first() {
            match &layer.image {
                Image::Solid(ColorValue::Absolute(rgba)) => return Self::Solid(rgba.to_color32()),
                Image::Gradient(gradient) => {
                    if let Some(fill) = Self::from_gradient(gradient) {
                        return fill;
                    }
                    tracing::debug!("gradient shape is beyond M1's painter; painting nothing");
                }
                other => {
                    tracing::debug!(image = ?other, "image kind is beyond M1's painter");
                }
            }
        }
        let color = style.get::<Rgba>(Prop::BackgroundColor);
        if color.a <= 0.0 {
            Self::None
        } else {
            Self::Solid(color.to_color32())
        }
    }

    fn from_gradient(gradient: &Gradient) -> Option<Self> {
        if gradient.repeating {
            return None;
        }
        let GradientKind::Linear {
            direction: LinearDirection::Side(SideOrCorner::Top),
        } = gradient.kind
        else {
            return None;
        };
        let [from, to] = gradient.stops.as_ref() else {
            return None;
        };
        let color = |stop: &ColorStop| match &stop.color {
            ColorValue::Absolute(rgba) => Some(rgba.to_color32()),
            _ => None,
        };
        let position = |stop: &ColorStop| match &stop.position {
            Some(Length::Abs { value, .. }) => Some(Some(*value)),
            None => Some(None),
            Some(_) => None,
        };
        Some(Self::ToTop {
            from: (color(from)?, position(from)?.unwrap_or(0.0)),
            to: (color(to)?, position(to)?),
        })
    }

    /// The colour this fill paints at `y_from_top` in a box `height` tall.
    ///
    /// `to top` gradients are sampled along a line whose origin is the
    /// *bottom* edge, so this converts. Positions before the first stop and
    /// after the last clamp to that stop's colour, per CSS.
    fn color_at(&self, height: f32, y_from_top: f32) -> Color {
        match self {
            Self::None => Color::TRANSPARENT,
            Self::Solid(color) => *color,
            Self::ToTop { from, to } => {
                let line = height.max(1.0);
                let p0 = from.1;
                let p1 = to.1.unwrap_or(line);
                let y = (line - y_from_top).clamp(0.0, line);
                if y <= p0 || (p1 - p0).abs() < f32::EPSILON {
                    return from.0;
                }
                if y >= p1 {
                    return to.0;
                }
                let t = (y - p0) / (p1 - p0);
                Color::from_argb(
                    lerp_u8(from.0.alpha(), to.0.alpha(), t),
                    lerp_u8(from.0.red(), to.0.red(), t),
                    lerp_u8(from.0.green(), to.0.green(), t),
                    lerp_u8(from.0.blue(), to.0.blue(), t),
                )
            }
        }
    }
}
```

with the imports:

```rust
use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::color::{ColorValue, Rgba};
use crate::css::value::{
    ColorStop, Gradient, GradientKind, Image, Keyword, Length, LinearDirection, SideOrCorner,
};
```

`paint.rs`'s own four tests keep their CSS fixtures and every asserted pixel; only
`painted()`'s node construction changes (Task 3 already did that). Append the
gradient-sampling test moved out of `computed.rs`:

```rust
    #[test]
    fn gradient_sampling_follows_to_top_semantics() {
        // to top => the gradient line starts at the bottom edge.
        let hover = super::Fill::ToTop {
            from: (Color(0xFFD6_D1CD), 0.0),
            to: (Color(0xFFE8_E6E3), Some(1.0)),
        };
        let h = 30.0;
        // The bottom edge itself is the first stop.
        assert_eq!(hover.color_at(h, h), Color(0xFFD6_D1CD));
        // The bottom pixel row's centre sits halfway through the 1px stop
        // band, so it is the midpoint of the two stops, not either of them.
        assert_eq!(hover.color_at(h, h - 0.5), Color(0xFFDF_DCD8));
        // Anything above 1px from the bottom is past the second stop.
        assert_eq!(hover.color_at(h, h / 2.0), Color(0xFFE8_E6E3));
        assert_eq!(hover.color_at(h, 0.5), Color(0xFFE8_E6E3));

        let normal = super::Fill::ToTop {
            from: (Color(0xFFF6_F5F4), 2.0),
            to: (Color(0xFFFB_FAFA), None),
        };
        // Below the 2px first stop the fill is flat.
        assert_eq!(normal.color_at(h, h - 0.5), Color(0xFFF6_F5F4));
        assert_eq!(normal.color_at(h, h - 1.5), Color(0xFFF6_F5F4));
        // The top edge is the second stop.
        assert_eq!(normal.color_at(h, 0.0), Color(0xFFFB_FAFA));
        // The middle is strictly between the two.
        let mid = normal.color_at(h, h / 2.0);
        assert!(mid != Color(0xFFF6_F5F4) && mid != Color(0xFFFB_FAFA));
        assert!((0xF6..=0xFB).contains(&mid.red()));
        assert!((0xF5..=0xFA).contains(&mid.green()));
        assert!((0xF4..=0xFA).contains(&mid.blue()));
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
cargo test -p icedtea-ui --lib
cargo test -p icedtea-ui --test themed_button_offscreen
```
Expected: PASS. Then confirm the gate file's numbers never moved:

```bash
git diff -- ui/tests/themed_button_offscreen.rs | grep -E '^[-+].*[0-9]' | \
  grep -vE '^[-+].*(icedtea_ui|use |fn |assert_eq!\(\s*$)' | sort | uniq -c
```
Expected: every removed numeric line has an identical added counterpart apart
from the field-access spelling. A number with no partner is a review failure.

- [ ] **Step 7: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all three clean.

- [ ] **Step 8: Commit**

```bash
git add ui/src/css/computed.rs ui/src/layout.rs ui/src/paint.rs ui/tests/themed_button_offscreen.rs
git commit -m "$(cat <<'EOF'
refactor(ui): retire M1's ComputedStyle fields; the table is the surface

ComputedStyle is now Box<[Value; 95]> and nothing else: Background,
GradientStop, BackgroundClip, the ten scalar fields, the m1_bridge and the
derived view are gone. layout and paint read the typed accessors, paint
keeping a small local Fill shim so the offscreen gate's exact bytes are
reproduced without leaning on the general gradient sampler (P4 replaces both
files). The M1 pixel gate is mechanical-only: every numeric constant in
themed_button_offscreen.rs is byte-identical.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: hardening — Decision 6 both ways, never-panic, whole-Adwaita sweep

**Files:**
- Modify: `ui/src/css/computed.rs` test module (4 tests)

**Interfaces:**
- Consumes: everything P3 produced in Tasks 1-10.
- Produces: no new API. This is P3's gate (contract §11: "900 rules; every
  Adwaita byte in computed.rs's tests; Decision 6 behaviour proven both ways").

- [ ] **Step 1: Write the failing tests**

Append to `computed.rs`'s test module:

```rust
    #[test]
    fn decision_six_goes_both_ways() {
        // An invalid winner takes the *inherited* value on an inherited
        // property and the *initial* value on a non-inherited one. Never the
        // runner-up, which is what M1 did.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; letter-spacing: 3px }\n\
             button { color: #00ff00; letter-spacing: 9px; background-color: #0000ff }\n\
             button.bad { color: @nosuch; letter-spacing: calc(1px * 1s); \
                          background-color: @nosuch }",
        );
        let style = resolve(&sheet, &button(&["bad"], PseudoStates::default()));

        // inherited -> the parent's computed value
        assert_eq!(
            style.color().to_color32(),
            Color(0xFFFF_0000),
            "an invalid inherited colour must inherit"
        );
        assert_eq!(
            style.get::<f32>(Prop::LetterSpacing),
            3.0,
            "an invalid inherited length must inherit"
        );
        // not inherited -> the registry initial (transparent), not #0000ff
        assert_eq!(
            style.get::<Rgba>(Prop::BackgroundColor),
            Rgba::from_value(&Prop::BackgroundColor.initial()),
            "an invalid non-inherited colour must take its initial value"
        );
    }

    #[test]
    fn every_adwaita_node_resolves_without_panicking_and_every_accessor_reads() {
        // The whole vendored theme, across the node shapes it actually styles:
        // resolve each, then read every accessor. This is the crate-level
        // never-panic battery for computed values and used values.
        let sheet = adwaita();
        let mut cx = MatchCx::new();
        let env = ResolveEnv::default();
        for name in [
            "window", "headerbar", "button", "entry", "label", "notebook", "menuitem", "scrollbar",
            "check", "radio", "popover", "tooltip", "levelbar", "progressbar",
        ] {
            for classes in [&[][..], &["flat"][..], &["suggested-action"][..], &["osd"][..]] {
                for states in [
                    PseudoStates::default(),
                    PseudoStates::HOVER,
                    PseudoStates::ACTIVE,
                    PseudoStates::DISABLED,
                    PseudoStates::CHECKED | PseudoStates::FOCUS,
                ] {
                    let window = Node::with_classes("window", &["background"]);
                    let node = Node::with_classes(name, classes);
                    window.append_child(&node);
                    node.set_states(states);
                    let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);

                    // Every longhand slot is readable...
                    for prop in crate::css::registry::longhands() {
                        let _ = style.raw(prop);
                        let _ = style.get::<f32>(prop);
                        let _ = style.get::<Keyword>(prop);
                        let _ = style.get::<Rgba>(prop);
                    }
                    // ...and every used-value accessor, at a plausible box and
                    // at a degenerate one.
                    for (w, h) in [(0.0, 0.0), (120.0, 40.0), (f32::MAX, 1.0)] {
                        let _ = style.padding(w);
                        let _ = style.margin(w);
                        let _ = style.min_size((w, h));
                        let _ = style.border_widths();
                        let _ = style.border_colors();
                        let _ = style.border_styles();
                        let _ = style.border_radii(w, h);
                        let _ = style.background_layers();
                        let _ = style.box_shadows();
                        let _ = style.transition_specs();
                        let _ = style.animation_specs();
                        let _ = style.opacity();
                        let _ = style.color();
                        let _ = style.font_size_px();
                    }
                }
            }
        }
    }

    #[test]
    fn resolving_hostile_declarations_never_panics_and_never_invents_a_value() {
        // The cascade's own hostile battery covers parse time; this covers
        // computed and used-value time, where the failure mode is a panic in a
        // resolver or a NaN escaping into layout.
        const HOSTILE: &[&str] = &[
            "button { padding: calc(100% - 100%) }",
            "button { padding: calc(1px / 0) }",
            "button { min-width: calc(1s + 1px) }",
            "button { border-radius: calc(1px * 1e30) / 1px }",
            "button { font-size: calc(-1em) }",
            "button { font-size: 0 }",
            "button { color: color-mix(in srgb, @nosuch, red) }",
            "@define-color a @b;\n@define-color b @a;\nbutton { color: @a }",
            "button { box-shadow: 1px 2px 3px 4px @nosuch }",
            "button { background-image: linear-gradient(to top, @nosuch, red) }",
            "button { letter-spacing: 1e38em }",
            "button { -gtk-dpi: 0 }",
            "button { -gtk-dpi: -5 }",
            "button { transform: translate(50%, calc(1px + 1%)) }",
            "button { opacity: calc(1 / 0) }",
        ];
        let mut cx = MatchCx::new();
        for css in HOSTILE {
            let sheet = CompiledSheet::compile(css);
            let style = ComputedStyle::resolve_chain(
                &sheet,
                &button(&[], PseudoStates::default()),
                &ResolveEnv::default(),
                &mut cx,
            );
            assert!(
                style.font_size_px().is_finite() && style.font_size_px() >= 0.0,
                "{css} produced a non-finite font size"
            );
            assert!(style.dpi().is_finite() && style.dpi() > 0.0, "{css} broke the dpi");
            for value in style.padding(100.0) {
                assert!(value.is_finite(), "{css} produced a non-finite padding");
            }
            for corner in style.border_radii(100.0, 50.0) {
                assert!(
                    corner[0].is_finite() && corner[1].is_finite(),
                    "{css} produced a non-finite radius"
                );
            }
            assert!((0.0..=1.0).contains(&style.opacity()), "{css} broke opacity");
        }
    }

    #[test]
    fn a_restyle_pass_reuses_one_match_context_across_the_whole_tree() {
        // `MatchCx` is caller-owned precisely so a pass keeps the bloom filter
        // and nth-index caches warm; a reused context must not change answers.
        let sheet = adwaita();
        let env = ResolveEnv::default();
        let node = button(&["suggested-action"], PseudoStates::default());

        let mut shared = MatchCx::new();
        let first = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut shared);
        let hovered_node = button(&[], PseudoStates::HOVER);
        let _ = ComputedStyle::resolve_chain(&sheet, &hovered_node, &env, &mut shared);
        let again = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut shared);
        assert_eq!(first, again, "a reused MatchCx leaked state between nodes");

        let fresh = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut MatchCx::new());
        assert_eq!(first, fresh, "a warm MatchCx disagrees with a cold one");
    }
```

Mutation checks:
- `decision_six_goes_both_ways`: restoring M1's runner-up walk returns `#00ff00`,
  `9.0` and `#0000ff`; swapping the inherited/initial arms of the fallback returns
  the initial colour for `color` and blue for `background-color`.
- `every_adwaita_node_resolves_without_panicking_and_every_accessor_reads`: any
  `unwrap`/index panic in a resolver or accessor aborts; an accessor that indexes
  `values` with a shorthand's discriminant panics on the `assert!` in `raw`.
- `resolving_hostile_declarations_never_panics_and_never_invents_a_value`:
  dropping the `is_finite` filter in `resolve_length`/`length_px` lets a NaN
  radius through; dropping the `-gtk-dpi > 0` guard makes `dpi()` zero and the
  `pt` conversion infinite.
- `a_restyle_pass_reuses_one_match_context_across_the_whole_tree`: a `MatchCx`
  that does not reset its bloom filter per node (missing `seed_for`) makes the
  warm and cold results differ.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib css::computed::tests::decision_six_goes_both_ways`
Expected: FAIL — either a compile error on a missing import, or an assertion
failure if any of the four behaviours is not yet in place. If all four pass on the
first run, that is still a valid outcome for this task *only* if each mutation
check above is executed by hand and observed to fail; record which ones you ran.

- [ ] **Step 3: Fix whatever the battery exposes**

Expect one or more of these, and fix them in `computed.rs` only:
- a resolver that returns `Some(NaN)` — add the `is_finite()` filter alongside the
  existing ones in `resolve_length`/`used_length`/`length_px`;
- `dpi()` returning `0.0` for `-gtk-dpi: 0` — the guard is `is_finite() && > 0.0`;
- `resolve_value` recursing without a depth bound on a `Pair`/`List` — the value
  tree is finite by construction (P1's parsers build no cycles), so no bound is
  needed; if the battery says otherwise, the bug is in P1 and belongs in a
  contract amendment, not a local patch.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::computed`
Expected: PASS (30 tests).

- [ ] **Step 5: Run the full gates**

Run:
```bash
cargo test -p icedtea-ui
cargo test --workspace
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean. `ui/tests/layer_shell_screencopy.rs` needs a compositor; run
it as the workspace's other Wayland tests are run.

- [ ] **Step 6: Commit**

```bash
git add ui/src/css/computed.rs
git commit -m "$(cat <<'EOF'
test(ui/css): P3's gate -- Decision 6 both ways, never-panic, Adwaita sweep

Proves an invalid winner inherits on an inherited property and takes the
initial on a non-inherited one; resolves every Adwaita node shape across
five state sets and reads every longhand slot and every used-value accessor
at three box sizes; runs a hostile-declaration battery through computed and
used-value time asserting nothing panics and no NaN escapes into layout; and
pins that a warm, reused MatchCx answers exactly like a cold one.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Task |
|---|---|
| Spec §1: cascade and computed style are driven by the declarative registry, nothing outside `registry.rs` names a property string | 2, 3 |
| Spec §3 "Cascade": M1's mechanism (important → specificity → source order → declaration order) iterating the registry | 2 |
| Spec §3: shorthands expanded at cascade time carrying the shorthand's key | 2 |
| Spec §3: `CascadedValues` keeps runner-ups for diagnostics | 2, 3 (logged, never consulted) |
| Spec §3: wide keywords and invalid-at-computed-value-time resolve in `computed.rs` (Decision 6) | 3, 11 |
| Spec §3: `@define-color` resolves lazily and cycle-guarded (definitions may reference later names) | 1 (`build_color_table` over the spliced definition list), 11 (cycle battery) |
| Spec §3: `ComputedStyle { values: Box<[Value; N]> }` indexed by `Prop`, typed `get::<T>` | 3 |
| Spec §3: inheritance copies the parent's computed value | 3 |
| Spec §3: `em`/`rem`/`%`/`-gtk-dpi` resolved in `computed.rs` | 3 (`em`/`rem`/`pt`/`-gtk-dpi`), 4-5 (`%`, against the used basis — deviation 9) |
| Spec §3: per-side borders and per-corner radii are first-class | 5 |
| Spec §5: `AnimationState::sample -> Overrides` layered onto the computed style | 7 (`Overrides`, `with_overrides`; the state machine is P5) |
| Spec §5: `transition-*`/`animation-*` values come from the new style | 7 (`transition_specs`, `animation_specs`) |
| Spec §7: engine unit tests for cascade, inheritance, invalid-at-computed-value-time | 2, 3, 11 |
| Spec §7: never-panic fuzz batteries | 2 (parse time, through every registry `ParseFn`), 11 (computed and used-value time) |
| Spec §7: mutation discipline — every load-bearing test records a mutation check | every task |
| Spec §7 / Decision 3: the M1 button gate stays unchanged | 10 (mechanical only, numbers frozen), gate-checked in every task |
| Contract §4: `CompiledSheet` gains `keyframes`, `buckets`, `env`, `compile_with_env`, `keyframes(name)`; `@media` spliced at source position | 1 |
| Contract §5: `CascadeKey`, `CascadedDecl`, `CascadedValues`, `cascade(sheet, node, cx)` | 2 |
| Contract §5: `ResolveEnv`, `FromValue`, `ComputedStyle::{initial, raw, get, resolve, resolve_chain, with_overrides}` | 3, 7 |
| Contract §5: the fixed resolution order `-gtk-dpi` → `font-size` → `color` → the rest | 3 |
| Contract §5: the five-step inheritance rule | 3 |
| Contract §5: typed accessors `font_size_px`, `length_ctx`, `color`, `opacity`, `padding`, `margin`, `border_widths/colors/styles/radii`, `min_size`, `background_layers`, `box_shadows`, `transition_specs`, `animation_specs` | 4, 5, 6, 7 |
| Contract §8: `BackgroundLayer` | 6 (defined; P4 re-exports — deviation 6) |
| Contract §10.1: every "Replaced types" row P3 owns | 2 (`CascadedValues`, `cascade`), 3 (`ComputedStyle`, `resolve`), 6 (`Background` → layers), 10 (deletions) |
| Contract §10.3: `cascade.rs`'s 10 tests rewritten, `900` and every declared value kept | 2 |
| Contract §10.3: `computed.rs`'s 22 tests, every pinned Adwaita byte kept, the runner-up test inverted | 3 (inverted, kept green through the derived view), 10 (retargeted onto accessors) |
| Contract §10.3: `widget/button.rs`'s 4 tests preserved as behaviours | 8 |
| Contract §10.3: `app.rs`'s fixture migration | 9 |
| Contract §10.3: `tests/themed_button_offscreen.rs` rewritten mechanically only | 10 |
| Contract §11 P3 gate: 900 rules; every Adwaita byte; Decision 6 both ways | 1, 2, 10, 11 |

Not in P3, by ownership: the `colors.rs` and `shorthand.rs` rows of §10.3 (P1),
the `select.rs` row (P2), `layout.rs`/`paint/**` proper (P4), the animation state
machine (P5), `text.rs` and the two coverage-gate test files (P6). See Contract
deviations 1 and 2.

### 2. Placeholder scan

- No "TBD", "TODO", "implement later", "fill in details", "add appropriate error
  handling", "handle edge cases" or "write tests for the above" appears in any
  task. Grep the plan for `TBD|TODO|XXX|FIXME|similar to Task` before executing:
  the only hits should be inside quoted *test data* (there are none) — a hit in
  prose is a plan bug.
- Every step that changes code carries the code. Task 9's Step 3 is deliberately
  edit-only and says so, with the exact replacement text given in Step 1.
- "Similar to Task N" never appears: Task 8's and Task 10's overlapping fixture
  code is written out in full in both places, as is `lerp_u8` (moved, not
  referenced) and the gradient-sampling test (relocated verbatim).
- Every type named is defined: in this plan (`CompiledSheet`, `CascadeKey`,
  `CascadedDecl`, `CascadedValues`, `ResolveEnv`, `FromValue`, `ComputedStyle`,
  `BackgroundLayer`, `Overrides`, `TransitionSpec`, `AnimationSpec`, `Fill`), in
  the contract §1-§3 (P1's registry/values, P2's `Node`/`MatchCx`/`RuleBuckets`),
  or in M1 and unchanged (`Allocation`, `ButtonLayout`, `FontStack`, `ShapedText`,
  `Button`, `ThemeSource`, `ThemeEnv`).

### 3. Type consistency vs the contract

- `cascade(sheet: &CompiledSheet, node: &Node, cx: &mut MatchCx) -> CascadedValues`
  — contract §5, verbatim, and used with exactly that shape in Tasks 3, 8, 9, 11.
- `CascadedValues::{candidates(Prop) -> &[CascadedDecl], winner(Prop) ->
  Option<&Value>, is_empty, len}` — contract §5, verbatim. `new()` is added
  (private construction would be impossible for the `#[cfg(test)]` reference
  implementation); adding a constructor is permitted by the contract's preamble.
- `CascadedDecl { value: Value, key: CascadeKey, from_shorthand: Option<Prop> }`
  — contract §5, verbatim, including `from_shorthand`, which Task 2 asserts.
- `ComputedStyle::{initial(&ResolveEnv) -> Rc<ComputedStyle>, raw(Prop) -> &Value,
  get::<T>(Prop) -> T, resolve(sheet, node, parent, env, cx), resolve_chain(sheet,
  node, env, cx), with_overrides(&Overrides) -> Cow<'_, ComputedStyle>}` —
  contract §5, verbatim. Every accessor's name, parameter list and return type
  matches §5's list, `border_radii(w, h) -> [[f32; 2]; 4]` and
  `margin(basis) -> [Option<f32>; 4]` included.
- `dpi()` and `used_ctx()` are additions, not replacements: `length_ctx(env,
  percent_basis)` keeps the contract's signature and is the public form.
- `Overrides`/`TransitionSpec`/`AnimationSpec` are copied field-for-field from
  contract §6, so P5's `AnimationState::{restyle, sample}` compiles against them
  unchanged.
- `BackgroundLayer`'s seven fields match contract §8 field-for-field
  (`image, position, size, repeat, origin, clip, blend`), so P4's
  `paint_backgrounds(..., layers: &[BackgroundLayer], ...)` compiles unchanged.
- Naming is stable across tasks: `resolve_chain` (never `resolve_all`),
  `background_layers` (never `layers`), `border_widths` (never `borders`),
  `font_size_px` (never `font_size`), `box_shadows` (never `shadows`). Task 4
  defines `length_px`; Tasks 5-6 call `length_px`/`used_length`, both defined in
  Task 4/5 respectively and never renamed.
- `Prop` variants used in this plan (`Color`, `Opacity`, `FontSize`, `GtkDpi`,
  `LetterSpacing`, `MinWidth`, `MinHeight`, `Padding*`, `Margin*`,
  `BorderTop/Right/Bottom/Left{Width,Style,Color}`, the four radius corners,
  `Background{Color,Image,Position,Size,Repeat,Origin,Clip,BlendMode}`,
  `BoxShadow`, `Transition*`, `Animation*`, `Border`) are all in contract §1.1's
  enum, spelled exactly as listed there.
- `Keyword` variants used (`None`, `Hidden`, `Solid`, `Dashed`, `Dotted`,
  `Double`, `Normal`, `All`, `Alternate`, `Both`, `Paused`, `Running`,
  `BorderBox`, `PaddingBox`, `ContentBox`, `Repeat`, `NoRepeat`, `Medium`) are all
  in contract §2.2's enum.
