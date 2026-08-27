# Pure-Rust GTK-themed UI — M2 Part 1: Property registry, typed values, parse (@keyframes/@media) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

---

## Contract deviations

The Part 0 contract (`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`) is
binding. These seven points are where it is wrong or incomplete for Part 1; each is
implemented as written below, and each is additive (no frozen signature changes
shape).

1. **P1 must NOT delete `css/colors.rs` and `css/shorthand.rs`.** Contract §11 tells
   P1 to delete them while forbidding P1 from touching `cascade.rs`/`computed.rs`,
   which `use` both. Deleting them in P1 makes the crate not compile. **Ruling:**
   P1 leaves both files in place, untouched, still serving M1's cascade/computed;
   **P3 deletes them** when it rewrites `cascade.rs`/`computed.rs`. Only
   `css/value.rs` → `css/tokens.rs` is renamed in P1 (it must be, because
   `css::value` becomes the typed-value module).
2. **P1 makes import-only edits to `css/select.rs:283` and `css/computed.rs:19`.**
   Both `use` `css::value`'s token helpers; the rename forces a one-token path
   change (`css::value` → `css::tokens`). No other line in either file changes, and
   no test in either file changes. Contract §11's "must not touch" is read as
   "must not change behaviour or signatures in".
3. **`Keyword` gains 11 variants** the contract's list omits but the registry needs:
   `Stretch` (initial value of `border-image-repeat`), and the CSS font-size
   keywords `XxSmall, XSmall, Small, Large, XLarge, XxLarge, XxxLarge, Larger,
   Smaller`, plus `Fill` (`border-image-slice`'s `fill`). Purely additive.
4. **`CalcNode` gains `Var(u8)`** and `CalcNode::resolve_number_with(&self, vars:
   &[f32; 4]) -> Option<f32>`. Relative colour syntax (`hsl(from #f6f5f4 h calc(s *
   1.2) calc(l * 1.2))`, 8 of Adwaita's 37 `@define-color`s) puts *channel
   variables* inside `calc()`; `ChannelExpr::Calc(Rc<CalcNode>)` cannot represent
   them otherwise, and the contract's 37/37 gate consequence is unreachable without
   it. `Var(0..=2)` are the origin colour's three channels in the function's own
   space, `Var(3)` is its alpha.
5. **`Length::parse` does not accept `auto`;** `Length::parse_allowing_auto` does.
   The contract says `Length::Auto` is "margin-* and border-image-width only" but
   gives one parser. Two entry points is the only way to honour both sentences.
6. **`Value::AnimationName` doubles as the ident carrier for
   `transition-property`.** The contract's `Value` has no `<custom-ident>` variant,
   and `transition-property: background-color` must survive parse as a name
   (unknown names are valid CSS that simply never match). `AnimationName::Named` is
   exactly "an ident", so `transition-property` items parse to
   `Keyword(All)` / `Keyword(None)` / `AnimationName(Named(..))`.
7. **Three additive public helpers** later parts consume, absent from the contract:
   `registry::parse_declaration_value(prop, text) -> Result<Value, ()>` (P3's
   cascade needs one whole-value entry point), `keyframes::Keyframes::compile(&
   KeyframesRule) -> Keyframes` (P3 compiles raw frames), and
   `parse::Stylesheet::append_layer(&mut self, layer: Stylesheet)` (P3's `app.rs`
   currently merges only `rules`/`color_definitions` and would silently drop a user
   theme's `@keyframes`/`@media`; the correct merge lives with the type).

---

**Goal:** Build the complete GTK 4.22 property registry, the typed CSS value tree
with never-panicking cssparser-token parsers for every value family, and extend the
stylesheet parser with `@keyframes` and `@media`, so that Parts 2–6 have one frozen,
tested foundation to cascade, compute, lay out, paint and animate against.

**Architecture:** A declarative table (`css/registry.rs`) holds one `PropertyDef`
row per property, indexed by a `Prop` enum whose discriminant *is* the row index and
(for longhands) the `ComputedStyle` slot. Each row carries a whole-value `ParseFn`,
an `initial` constructor, an `inherited` flag and an optional `Interpolate`;
shorthand rows carry an `ExpandFn` plus the longhands they reset. Every parser lives
in `css/value/<family>.rs`, is cssparser-token based (never string-sliced), is
ASCII-case- and whitespace-insensitive, resolves nothing (`@name`, `currentColor`,
`em`/`%`/`calc()` all survive unresolved into the `Value`), and never panics.
`css/parse.rs` gains `@keyframes` and `@media`, keeping `@media` blocks
**unevaluated** so one parse can be compiled under several `MediaEnv`s.

**Tech Stack:** Rust 2024 · `cssparser` 0.37 (tokenizer + `AtRuleParser`/
`QualifiedRuleParser`/`RuleBodyParser`/`StyleSheetParser`; **no** calc AST, **no**
`@media`/`@keyframes` grammar — both are project code) · `selectors` 0.40 (Part 2) ·
`skia-rs-safe` 0.4.0 (`core::Color`, `core::Matrix`, `core::hsl_to_rgb`,
`core::rgb_to_hsl`, `Color::from_css` for CSS-3 names and the Level-4
`lab()/lch()/oklab()/oklch()/color()` fallback) · `bitflags` 2 · `taffy` 0.14 (Part
4) · `fontconfig` 0.11 (Part 6; the dependency is added here).

**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
**Contract:** `docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§1, §2, §4)
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
**Research notes:** `.superpowers/m2-plan-notes/{cssparser-selectors,gtk-css-semantics,current-crate}.md`

## Global Constraints

- **Crate:** `ui/` = `icedtea-ui`. Branch `rebuild/pure-rust-gtk-m2` (off `develop`
  @ `216a8e8`). All paths below are relative to the repository root.
- **Pinned crate versions — do not bump:** `cssparser = "0.37"`,
  `selectors = "0.40"`, `taffy = "0.14"`, `skia-rs-safe = "0.4.0"`,
  `wayland-client = "0.31"`, `precomputed-hash = "0.1"`,
  `wayland-protocols-wlr = "0.3"`, `rustix = "1"`. To add in Task 1:
  `fontconfig = "0.11"`, `bitflags = "2"`, and the `skia-rs-safe` features
  `["std", "text", "codec", "codec-png", "svg"]`.
- **No `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk` and no `smithay`** may enter
  `ui/Cargo.toml`. `fontconfig` links the system C library and is explicitly
  allowed (spec Decision 4).
- **Rust edition 2024, `rust-version = "1.94"`** — inherited from
  `[workspace.package]`; never overridden in `ui/Cargo.toml`.
- **Gates — every task's final run must be green:**
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- **The M1 pixel gate `ui/tests/themed_button_offscreen.rs` must stay green and
  byte-identical.** Part 1 does not touch it. Neither do the 46 byte-identical M1
  tests of contract §10.2 — of which Part 1 owns two files: `css/parse.rs`'s **16**
  tests (new `@media`/`@keyframes` tests are *appended*, never interleaved) and
  `css/tokens.rs`'s **4** tests (the M1 `value.rs` bodies, file relocated).
- **Pinned constants Part 1 must not move:** 900 compiled Adwaita rules; the
  vendored sheet's 1941 lines and 37 `@define-color`s; `DEFAULT_FONT_SIZE = 14.0`.
- **Parts execute IN ORDER 1→6 on one branch.** Part 1 consumes nothing and may be
  consumed by everything after it.
- **Every commit message ends with the trailer:**
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **No `unsafe`.** No `unwrap()`/`expect()`/`panic!` on any path reachable from a
  parser — the one permitted panic in Part 1 is `Prop::initial()` on a shorthand,
  which is a programming error, not input-driven.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/Cargo.toml` | Modify | Add `fontconfig`, `bitflags`; widen `skia-rs-safe` features |
| `ui/src/css/mod.rs` | Modify | Declare `tokens`, `registry`, `value` (dir module); keep `colors`/`shorthand` for P3 |
| `ui/src/css/value.rs` → `ui/src/css/tokens.rs` | Rename | M1's token-stream serializers, bodies and 4 tests unchanged |
| `ui/src/css/value/mod.rs` | Create | `Value`, `Wide` wide-keyword plumbing, module re-exports, shared test corpus |
| `ui/src/css/value/keyword.rs` | Create | The flat `Keyword` enum + `as_str`/`from_str_ascii_ci` |
| `ui/src/css/value/calc.rs` | Create | `CalcNode` expression tree + evaluator (100% project code) |
| `ui/src/css/value/length.rs` | Create | `LengthUnit`, `Length`, `LengthCtx`, unit resolution |
| `ui/src/css/value/color.rs` | Create | `Rgba`, `ColorValue`, `ColorTable`, `ColorCtx`, `build_color_table` |
| `ui/src/css/value/image.rs` | Create | `Image`, `Gradient`, `Position`, `IconRef`, gradient sampler |
| `ui/src/css/value/shadow.rs` | Create | `Shadow` (box + text forms) |
| `ui/src/css/value/border.rs` | Create | `BorderImageSlice`, `BorderImageWidthSide`, `RepeatStyle`, `BgSize` |
| `ui/src/css/value/font.rs` | Create | Font families, weight, style, line-height, features, variations, variant flags |
| `ui/src/css/value/text.rs` | Create | `TextDecorationLines` + text-family value parsers |
| `ui/src/css/value/transform.rs` | Create | `TransformFn`, matrix flattening, 2-D decompose/recompose |
| `ui/src/css/value/filter.rs` | Create | `FilterFn` |
| `ui/src/css/value/timing.rs` | Create | `Time`, `TimingFunction`, `AnimationName`, `IterationCount` |
| `ui/src/css/value/interpolate.rs` | Create | The 13 registry interpolators |
| `ui/src/css/value/shorthand.rs` | Create | The 18 `ExpandFn`s |
| `ui/src/css/value/keyframes.rs` | Create | Compiled `Keyframe`/`Keyframes` + segment lookup |
| `ui/src/css/registry.rs` | Create | `Prop`, `PropertyDef`, `PROPERTIES`, lookup and accessors |
| `ui/src/css/parse.rs` | Modify | `@keyframes`, `@media`, `MediaEnv`/`MediaQuery`/`MediaBlock`, `Stylesheet` fields |
| `ui/src/css/select.rs:283` | Modify | Import-only: `css::value` → `css::tokens` |
| `ui/src/css/computed.rs:19` | Modify | Import-only: `super::value` → `super::tokens` |
| `ui/src/css/shorthand.rs:14` | Modify | Import-only: `super::value` → `super::tokens` |

---

## Task 1: Dependencies and the `value.rs` → `tokens.rs` rename

`css::value` must become the typed-value *directory* module, so M1's token helpers
move to `css::tokens` with their bodies and their 4 tests unchanged. Four files
`use` them; all four get a one-token path change and nothing else. (`css/shorthand.rs`
is one of them: deviation 1 keeps it on disk for P3, so its `use` must be repointed
or the crate does not compile.)

**Files:**
- Modify: `ui/Cargo.toml:11-20` (the `[dependencies]` table)
- Rename: `ui/src/css/value.rs` → `ui/src/css/tokens.rs` (contents unchanged)
- Modify: `ui/src/css/mod.rs:4-10` (module declarations)
- Modify: `ui/src/css/parse.rs:6-8` (doc comment), `:21` (`use`), `:93` (call)
- Modify: `ui/src/css/select.rs:283` (call path)
- Modify: `ui/src/css/computed.rs:19` (`use`)
- Modify: `ui/src/css/shorthand.rs:14` (`use`) — import-only; the file itself
  stays for P3 (deviation 1)
- Test: `ui/src/css/tokens.rs` (the 4 relocated M1 tests, byte-identical)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `crate::css::tokens::write_component_values(&mut cssparser::Parser<'_, '_>, &mut String)`
  - `crate::css::tokens::write_one_component(&mut cssparser::Parser<'_, '_>, &mut String)`
  - `crate::css::tokens::serialize_remaining(&mut cssparser::Parser<'_, '_>) -> String`
  - `crate::css::tokens::component_values(&str) -> Vec<String>`
  - `crate::css::tokens::comma_groups(&str) -> Vec<Vec<String>>`
  - `ui/Cargo.toml` carries `bitflags = "2"`, `fontconfig = "0.11"` and
    `skia-rs-safe` features `["std", "text", "codec", "codec-png", "svg"]`.

- [ ] **Step 1: Write the failing test**

Create `ui/tests/part1_foundation.rs` — a scratch integration test that proves the
new module path and the new dependencies are wired. It is deleted in Task 20.

```rust
//! Scaffolding gate for M2 Part 1. Deleted in the final Part 1 task.

#[test]
fn token_helpers_live_in_css_tokens() {
    assert_eq!(
        icedtea_ui::css::tokens::component_values("1px solid rgb(0 0 0)"),
        vec!["1px", "solid", "rgb(0 0 0)"]
    );
}

#[test]
fn bitflags_is_available_to_the_crate() {
    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        struct Probe: u8 { const A = 1; const B = 2; }
    }
    assert_eq!((Probe::A | Probe::B).bits(), 3);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test part1_foundation`
Expected: FAIL — `error[E0433]: failed to resolve: could not find 'tokens' in 'css'`
and `error[E0432]: unresolved import 'bitflags'`.

- [ ] **Step 3: Write minimal implementation**

Rename the file, preserving history:

```bash
git mv ui/src/css/value.rs ui/src/css/tokens.rs
```

Change the first doc line of `ui/src/css/tokens.rs` from
`//! Token-level handling of declaration values.` to:

```rust
//! Token-level handling of declaration values (M1's `css::value`, relocated).
//!
//! `css::value` is now the *typed* value module; this file keeps the
//! token-stream serializers that turn a declaration's tokens into the
//! normalized string `css::parse` stores. Bodies and tests are unchanged
//! from M1.
```

Replace `ui/src/css/mod.rs` in full:

```rust
//! The GTK4-CSS engine: parse, color resolution, selector matching,
//! cascade, and computed values.

pub mod cascade;
pub mod colors;
pub mod computed;
pub mod parse;
pub mod registry;
pub mod select;
pub mod shorthand;
pub mod tokens;
pub mod value;
```

(`registry` and `value` do not exist yet — this step only adds `tokens`; add the
`registry`/`value` lines in Tasks 2 and 16 respectively. For this task the file is:)

```rust
//! The GTK4-CSS engine: parse, color resolution, selector matching,
//! cascade, and computed values.

pub mod cascade;
pub mod colors;
pub mod computed;
pub mod parse;
pub mod select;
pub mod shorthand;
pub mod tokens;
```

In `ui/src/css/parse.rs`, change line 21 from `use super::value::serialize_remaining;`
to `use super::tokens::serialize_remaining;`, line 93 from
`super::value::write_one_component(input, &mut value);` to
`super::tokens::write_one_component(input, &mut value);`, and the doc-comment
reference on line 6 from `` (`super::value`) `` to `` (`super::tokens`) ``.

In `ui/src/css/select.rs`, change line 283 from
`let argument = crate::css::value::serialize_remaining(parser);` to
`let argument = crate::css::tokens::serialize_remaining(parser);`.

In `ui/src/css/computed.rs`, change line 19 from
`use super::value::{comma_groups, component_values};` to
`use super::tokens::{comma_groups, component_values};`.

In `ui/Cargo.toml`, replace the `[dependencies]` table with:

```toml
[dependencies]
skia-rs-safe = { version = "0.4.0", default-features = false, features = [
    "std",
    "text",
    "codec",
    "codec-png",
    "svg",
] }
bitflags = "2"
cssparser = "0.37"
selectors = "0.40"
precomputed-hash = "0.1"
taffy = "0.14"
fontconfig = "0.11"
wayland-client = "0.31"
wayland-protocols-wlr = { version = "0.3", features = ["client"] }
rustix = { version = "1", features = ["fs", "event"] }
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui`
Expected: PASS. The 4 `css::tokens` tests
(`a_function_call_is_one_component`, `top_level_commas_are_their_own_component`,
`nested_functions_survive_splitting`, `an_empty_value_has_no_components`) run under
their new module path with byte-identical bodies, and every other M1 test is
untouched.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/Cargo.toml ui/src/css/mod.rs ui/src/css/tokens.rs \
        ui/src/css/parse.rs ui/src/css/select.rs ui/src/css/computed.rs \
        ui/tests/part1_foundation.rs Cargo.lock
git commit -m "refactor(ui): relocate M1 token helpers to css::tokens, add M2 deps

css::value becomes the typed-value module in M2, so M1's token-stream
serializers move to css::tokens with bodies and all four tests unchanged.
Adds bitflags 2 and fontconfig 0.11, and widens skia-rs-safe to the
codec/codec-png/svg features Image::Url decoding needs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2: `Keyword`, `Wide`, and the shared value-module scaffolding

One flat enum covers every discrete keyword value in the registry, so all discrete
properties share one `Value` variant and one interpolator. A macro keeps the CSS
spelling and the variant in one place, so `as_str` and `from_str_ascii_ci` can never
drift apart.

**Files:**
- Create: `ui/src/css/value/mod.rs`
- Create: `ui/src/css/value/keyword.rs`
- Modify: `ui/src/css/mod.rs` (add `pub mod value;`)
- Test: `ui/src/css/value/keyword.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `css::value::keyword::Keyword` — `#[repr(u16)]`, `Copy + Clone + Debug + PartialEq + Eq + Hash`
  - `Keyword::as_str(self) -> &'static str`
  - `Keyword::from_str_ascii_ci(s: &str) -> Option<Keyword>`
  - `css::value::keyword::Wide` — `Inherit | Initial | Unset`, `Copy + Clone + Debug + PartialEq + Eq`
  - `Wide::as_str(self) -> &'static str`, `Wide::from_str_ascii_ci(s: &str) -> Option<Wide>`
  - re-exports `css::value::{Keyword, Wide}`
  - `#[cfg(test)] css::value::FUZZ_INPUTS: &[&str]` — the shared never-panic corpus
  - `#[cfg(test)] css::value::parse_entirely_with<T>(text: &str, f) -> Result<T, ()>`

- [ ] **Step 1: Write the failing test**

Create `ui/src/css/value/keyword.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::{Keyword, Wide};

    #[test]
    fn every_keyword_round_trips_through_its_css_spelling() {
        for keyword in Keyword::ALL {
            assert_eq!(
                Keyword::from_str_ascii_ci(keyword.as_str()),
                Some(*keyword),
                "{} did not round-trip",
                keyword.as_str()
            );
        }
    }

    #[test]
    fn keyword_spellings_are_unique() {
        // A duplicated spelling would make `from_str_ascii_ci` silently
        // shadow one variant, and a registry row would parse to the wrong
        // keyword forever.
        let mut seen: Vec<&'static str> = Keyword::ALL.iter().map(|k| k.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate keyword spelling: {seen:?}");
    }

    #[test]
    fn keyword_lookup_is_ascii_case_insensitive() {
        assert_eq!(Keyword::from_str_ascii_ci("NoNe"), Some(Keyword::None));
        assert_eq!(Keyword::from_str_ascii_ci("CURRENTCOLOR"), Some(Keyword::CurrentColor));
        assert_eq!(Keyword::from_str_ascii_ci("ultra-CONDENSED"), Some(Keyword::UltraCondensed));
        assert_eq!(Keyword::from_str_ascii_ci("alternate-reverse"), Some(Keyword::AlternateReverse));
        assert_eq!(Keyword::from_str_ascii_ci("color"), Some(Keyword::ColorBlend));
        assert_eq!(Keyword::from_str_ascii_ci("nope"), None);
        assert_eq!(Keyword::from_str_ascii_ci(""), None);
    }

    #[test]
    fn the_wide_keywords_are_recognised_case_insensitively() {
        assert_eq!(Wide::from_str_ascii_ci("inherit"), Some(Wide::Inherit));
        assert_eq!(Wide::from_str_ascii_ci("INITIAL"), Some(Wide::Initial));
        assert_eq!(Wide::from_str_ascii_ci("Unset"), Some(Wide::Unset));
        assert_eq!(Wide::from_str_ascii_ci("revert"), None);
        assert_eq!(Wide::Inherit.as_str(), "inherit");
    }

    #[test]
    fn non_ascii_and_control_bytes_never_panic() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = Keyword::from_str_ascii_ci(input);
            let _ = Wide::from_str_ascii_ci(input);
        }
    }
}
```

Mutation check: deleting one arm of the `keywords!` macro invocation makes
`every_keyword_round_trips_through_its_css_spelling` still pass (the variant is
gone from `ALL` too), but any registry row referencing it stops compiling — so the
load-bearing assertion here is `keyword_spellings_are_unique`: changing any two
spellings to the same string fails it.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui keyword`
Expected: FAIL — `error[E0433]: failed to resolve: could not find 'value' in 'css'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod value;` to `ui/src/css/mod.rs` (alphabetically after `tokens`).

Create `ui/src/css/value/mod.rs`:

```rust
//! Typed CSS values.
//!
//! Every parser here takes a `cssparser::Parser`, is ASCII-case- and
//! whitespace-insensitive, resolves nothing (`@name`, `currentColor`,
//! `em`/`rem`/`%` and `calc()` all survive unresolved into the value), and
//! never panics on any token stream. `Err(())` means *invalid at parse
//! time*: the declaration is dropped, CSS-style.

pub mod keyword;

pub use keyword::{Keyword, Wide};

/// Odd inputs every value family's never-panic battery runs.
///
/// One corpus, shared, so a new hostile input added for one family
/// immediately covers all of them.
#[cfg(test)]
pub(crate) const FUZZ_INPUTS: &[&str] = &[
    "",
    " ",
    "\t\n",
    "/**/",
    "0",
    "-",
    "+",
    ".",
    "e",
    "e10",
    "1e999",
    "-1e999",
    "nan",
    "inf",
    "-0",
    "#",
    "#z",
    "#\u{e9}",
    "#\u{1f600}\u{1f600}\u{1f600}",
    "\u{e9}",
    "\u{0}\u{1}\u{2}",
    "@",
    "@\u{e9}",
    "(",
    ")",
    "()",
    "[",
    "{",
    "{}",
    ",",
    ",,,",
    "/",
    "//",
    "!",
    "!important",
    "url(",
    "url()",
    "calc(",
    "calc()",
    "calc(1px",
    "calc(1px +)",
    "calc(1px + )",
    "calc(* 2)",
    "calc(1px * 2px)",
    "calc(1 / 0)",
    "calc(1px + 1s)",
    "min()",
    "max()",
    "clamp(1px)",
    "rgb(",
    "rgb()",
    "rgb(1,2)",
    "rgb(1,2,3,4,5)",
    "rgb(from)",
    "hsl(\u{e9})",
    "color-mix(",
    "color-mix(in)",
    "linear-gradient(",
    "linear-gradient()",
    "linear-gradient(red)",
    "radial-gradient(at)",
    "conic-gradient(from)",
    "cross-fade()",
    "-gtk-icontheme()",
    "-gtk-scaled(a)",
    "image()",
    "steps()",
    "steps(0)",
    "steps(-1, end)",
    "cubic-bezier()",
    "cubic-bezier(1,2,3)",
    "cubic-bezier(2,0,0,1)",
    "matrix()",
    "matrix(1)",
    "translate()",
    "rotate()",
    "blur()",
    "drop-shadow()",
    "inset",
    "1px 2px 3px 4px 5px 6px",
    "1px/",
    "/1px",
    "solid solid solid",
    "\u{202e}",
    "a".repeat(0).as_str(),
];

/// Parse `text` with `f`, requiring the whole input to be consumed.
///
/// The test-side mirror of the registry's whole-value rule: a trailing
/// token is an error.
#[cfg(test)]
pub(crate) fn parse_entirely_with<T>(
    text: &str,
    f: impl FnOnce(&mut cssparser::Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    let mut input = cssparser::ParserInput::new(text);
    let mut parser = cssparser::Parser::new(&mut input);
    let value = f(&mut parser)?;
    parser.skip_whitespace();
    if parser.is_exhausted() { Ok(value) } else { Err(()) }
}
```

Note: the final `"a".repeat(0).as_str()` entry is not a `const`-evaluable
expression — replace that line with the literal `"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"`
(a long bare ident, the case that used to walk off the end of a fixed hex buffer).

Create `ui/src/css/value/keyword.rs`:

```rust
//! The one flat keyword enum every discrete property value shares.
//!
//! Sharing one enum (and therefore one `Value` variant) is what lets the
//! registry give every discrete property the same `discrete` interpolator
//! and the same equality semantics without per-property code.

/// Declares `Keyword`, its CSS spellings, `as_str`, `from_str_ascii_ci`
/// and `ALL` from a single table, so the spelling and the variant can never
/// drift apart.
macro_rules! keywords {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Keyword { $( $variant ),+ }

        impl Keyword {
            /// Every keyword, in declaration order. Test-facing, but cheap
            /// enough (a `&'static` slice) to be public.
            pub const ALL: &'static [Keyword] = &[ $( Keyword::$variant ),+ ];

            /// The keyword's CSS spelling, always ASCII-lowercase.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $( Keyword::$variant => $name ),+ }
            }

            /// ASCII-case-insensitive lookup; `None` for anything else.
            #[must_use]
            pub fn from_str_ascii_ci(s: &str) -> Option<Self> {
                $( if s.eq_ignore_ascii_case($name) { return Some(Keyword::$variant); } )+
                None
            }
        }
    };
}

keywords! {
    // universal
    None => "none", Auto => "auto", Normal => "normal",
    // border / outline style
    Hidden => "hidden", Dotted => "dotted", Dashed => "dashed", Solid => "solid",
    Double => "double", Groove => "groove", Ridge => "ridge",
    Inset => "inset", Outset => "outset",
    // background boxes / repeat / blend
    BorderBox => "border-box", PaddingBox => "padding-box",
    ContentBox => "content-box", TextBox => "text",
    Repeat => "repeat", RepeatX => "repeat-x", RepeatY => "repeat-y",
    Space => "space", Round => "round", NoRepeat => "no-repeat",
    Stretch => "stretch",
    Multiply => "multiply", Screen => "screen", Overlay => "overlay",
    Darken => "darken", Lighten => "lighten", ColorDodge => "color-dodge",
    ColorBurn => "color-burn", HardLight => "hard-light", SoftLight => "soft-light",
    Difference => "difference", Exclusion => "exclusion", Hue => "hue",
    Saturation => "saturation", ColorBlend => "color", Luminosity => "luminosity",
    // fonts
    Italic => "italic", Oblique => "oblique", SmallCaps => "small-caps",
    Bold => "bold", Bolder => "bolder", Lighter => "lighter",
    UltraCondensed => "ultra-condensed", ExtraCondensed => "extra-condensed",
    Condensed => "condensed", SemiCondensed => "semi-condensed",
    SemiExpanded => "semi-expanded", Expanded => "expanded",
    ExtraExpanded => "extra-expanded", UltraExpanded => "ultra-expanded",
    Sub => "sub", Super => "super", AllSmallCaps => "all-small-caps",
    PetiteCaps => "petite-caps", AllPetiteCaps => "all-petite-caps",
    Unicase => "unicase", TitlingCaps => "titling-caps",
    HistoricalForms => "historical-forms", Ruby => "ruby",
    Ordinal => "ordinal", SlashedZero => "slashed-zero",
    XxSmall => "xx-small", XSmall => "x-small", Small => "small",
    Large => "large", XLarge => "x-large", XxLarge => "xx-large",
    XxxLarge => "xxx-large", Larger => "larger", Smaller => "smaller",
    // text
    Capitalize => "capitalize", Uppercase => "uppercase", Lowercase => "lowercase",
    FullWidth => "full-width", FullSizeKana => "full-size-kana",
    Underline => "underline", Overline => "overline",
    LineThrough => "line-through", Blink => "blink", Wavy => "wavy",
    // icons
    Requested => "requested", Regular => "regular", Symbolic => "symbolic",
    Builtin => "builtin",
    // animation
    Forwards => "forwards", Backwards => "backwards", Both => "both",
    Running => "running", Paused => "paused", Reverse => "reverse",
    Alternate => "alternate", AlternateReverse => "alternate-reverse",
    Infinite => "infinite", All => "all",
    // misc
    Cover => "cover", Contain => "contain",
    Left => "left", Right => "right", Top => "top", Bottom => "bottom",
    Center => "center", Closest => "closest", Farthest => "farthest",
    ClosestSide => "closest-side", ClosestCorner => "closest-corner",
    FarthestSide => "farthest-side", FarthestCorner => "farthest-corner",
    Circle => "circle", Ellipse => "ellipse",
    Thin => "thin", Medium => "medium", Thick => "thick",
    Transparent => "transparent", CurrentColor => "currentcolor",
    Invert => "invert", Ltr => "ltr", Rtl => "rtl", Fill => "fill",
}

/// The CSS-wide keywords, valid on every property.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wide {
    /// Take the parent's computed value.
    Inherit,
    /// Take the property's initial value.
    Initial,
    /// `inherit` for inherited properties, `initial` otherwise.
    Unset,
}

impl Wide {
    /// The keyword's CSS spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Wide::Inherit => "inherit",
            Wide::Initial => "initial",
            Wide::Unset => "unset",
        }
    }

    /// ASCII-case-insensitive lookup. `revert`/`revert-layer` are **not**
    /// wide keywords here: GTK has no cascade origins to revert to.
    #[must_use]
    pub fn from_str_ascii_ci(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("inherit") {
            Some(Wide::Inherit)
        } else if s.eq_ignore_ascii_case("initial") {
            Some(Wide::Initial)
        } else if s.eq_ignore_ascii_case("unset") {
            Some(Wide::Unset)
        } else {
            None
        }
    }
}
```

Append the test module from Step 1 to the end of `keyword.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui keyword`
Expected: PASS — 5 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/mod.rs ui/src/css/value/mod.rs ui/src/css/value/keyword.rs
git commit -m "feat(ui/css): flat Keyword enum and the wide keywords

One enum for every discrete keyword value, generated from a single
spelling table so as_str and from_str_ascii_ci cannot drift. Adds the
shared never-panic fuzz corpus every value family will run.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3: `Length`, `calc()` and `Time`

`cssparser` 0.37 has **no** calc AST — `calc()` arrives as a plain
`Token::Function` (research/cssparser-selectors.md §1). The whole expression tree
and evaluator are project code. `Time` lands here too, because `CalcNode` has a
`Time` arm and `Time::parse` must accept `calc()`.

**Files:**
- Create: `ui/src/css/value/calc.rs`
- Create: `ui/src/css/value/length.rs`
- Create: `ui/src/css/value/timing.rs` (the `Time` type only; the easing functions
  are added in Task 13)
- Modify: `ui/src/css/value/mod.rs` (declare and re-export the three modules)
- Test: all three files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `css::value::FUZZ_INPUTS`, `css::value::parse_entirely_with` (Task 2).
- Produces:
  - `css::value::length::{LengthUnit, Length, LengthCtx}`
  - `Length::parse(&mut Parser<'_, '_>) -> Result<Length, ()>` (no `auto`)
  - `Length::parse_allowing_auto(&mut Parser<'_, '_>) -> Result<Length, ()>`
  - `Length::resolve(&self, &LengthCtx) -> Option<f32>`
  - `Length::zero() -> Length`, `Length::px(f32) -> Length`
  - `impl Default for LengthCtx` (14 px font, 14 px root, ex 0.5, 96 dpi, no basis)
  - `css::value::calc::CalcNode` + `CalcNode::parse`, `resolve_length`,
    `resolve_number`, `resolve_angle`, `resolve_time`, `resolve_number_with`
  - `css::value::calc::parse_math_function(name: &str, &mut Parser<'_, '_>) -> Result<CalcNode, ()>`
  - `css::value::calc::parse_angle(&mut Parser<'_, '_>) -> Result<f32, ()>` (degrees)
  - `css::value::timing::Time` + `Time::ZERO`, `from_ms`, `as_secs_f32`,
    `as_millis_f32`, `Time::parse`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/length.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Length, LengthCtx, LengthUnit};
    use crate::css::value::parse_entirely_with;

    fn len(text: &str) -> Option<Length> {
        parse_entirely_with(text, Length::parse).ok()
    }

    fn px(text: &str, ctx: &LengthCtx) -> Option<f32> {
        len(text)?.resolve(ctx)
    }

    #[test]
    fn absolute_units_convert_through_gtk_dpi_not_a_fixed_96() {
        // GTK resolves pt/pc/in/cm/mm through `-gtk-dpi`, NOT CSS's fixed
        // 96dpi (research/gtk-css-semantics.md §4).
        let ctx = LengthCtx { dpi: 192.0, ..LengthCtx::default() };
        assert_eq!(px("12px", &ctx), Some(12.0));
        assert_eq!(px("72pt", &ctx), Some(192.0));
        assert_eq!(px("1in", &ctx), Some(192.0));
        assert_eq!(px("1pc", &ctx), Some(32.0));
        let ninety_six = LengthCtx { dpi: 96.0, ..LengthCtx::default() };
        assert_eq!(px("72pt", &ninety_six), Some(96.0));
    }

    #[test]
    fn font_relative_units_use_their_own_bases() {
        let ctx = LengthCtx {
            font_size_px: 20.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            ..LengthCtx::default()
        };
        assert_eq!(px("2em", &ctx), Some(40.0));
        assert_eq!(px("2rem", &ctx), Some(28.0));
        assert_eq!(px("2ex", &ctx), Some(20.0));
    }

    #[test]
    fn percentages_need_a_basis_and_are_stored_as_fractions() {
        assert_eq!(len("50%"), Some(Length::Percent(0.5)));
        let with_basis = LengthCtx { percent_basis: Some(200.0), ..LengthCtx::default() };
        assert_eq!(px("50%", &with_basis), Some(100.0));
        // No basis == invalid at computed-value time, not a parse error.
        assert!(len("50%").is_some());
        assert_eq!(px("50%", &LengthCtx::default()), None);
    }

    #[test]
    fn unitless_zero_parses_but_other_bare_numbers_do_not() {
        assert_eq!(len("0"), Some(Length::zero()));
        assert_eq!(len("3"), None);
        assert_eq!(len("3.5"), None);
    }

    #[test]
    fn auto_is_only_accepted_by_the_auto_aware_entry_point() {
        assert_eq!(len("auto"), None);
        assert_eq!(
            parse_entirely_with("auto", Length::parse_allowing_auto).ok(),
            Some(Length::Auto)
        );
        assert_eq!(Length::Auto.resolve(&LengthCtx::default()), None);
    }

    #[test]
    fn units_are_ascii_case_insensitive_and_unknown_units_are_rejected() {
        assert_eq!(len("4PX"), Some(Length::px(4.0)));
        assert_eq!(len("4Em"), Some(Length::Abs { value: 4.0, unit: LengthUnit::Em }));
        assert_eq!(len("4vh"), None);
        assert_eq!(len("4"), None);
    }

    #[test]
    fn a_non_finite_length_never_resolves() {
        // A theme that manages to express an overflowing dimension must not
        // poison layout with a NaN.
        assert_eq!(px("1e40px", &LengthCtx::default()), None);
    }

    #[test]
    fn trailing_tokens_make_the_whole_value_invalid() {
        assert_eq!(len("4px 9px"), None);
        assert_eq!(len("4px!"), None);
    }

    #[test]
    fn length_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Length::parse);
            let _ = parse_entirely_with(input, Length::parse_allowing_auto);
            if let Ok(length) = parse_entirely_with(input, Length::parse_allowing_auto) {
                let _ = length.resolve(&LengthCtx::default());
                let _ = length.resolve(&LengthCtx {
                    percent_basis: Some(100.0),
                    ..LengthCtx::default()
                });
            }
        }
    }
}
```

Mutation check: `absolute_units_convert_through_gtk_dpi_not_a_fixed_96` fails the
moment `pt` is divided by a hard-coded `96.0` instead of `ctx.dpi` — the exact bug
the GTK note warns about. `percentages_need_a_basis_and_are_stored_as_fractions`
fails if `Percent` stores `50.0` instead of `0.5`.

Append to `ui/src/css/value/calc.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{CalcNode, parse_angle};
    use crate::css::value::length::{Length, LengthCtx};
    use crate::css::value::parse_entirely_with;
    use crate::css::value::timing::Time;

    fn calc(text: &str) -> Option<Length> {
        parse_entirely_with(text, Length::parse).ok()
    }

    fn calc_px(text: &str, ctx: &LengthCtx) -> Option<f32> {
        calc(text)?.resolve(ctx)
    }

    #[test]
    fn sums_and_differences_of_compatible_units_resolve() {
        let ctx = LengthCtx { font_size_px: 10.0, ..LengthCtx::default() };
        assert_eq!(calc_px("calc(1px + 2px)", &ctx), Some(3.0));
        assert_eq!(calc_px("calc(10px - 4px)", &ctx), Some(6.0));
        assert_eq!(calc_px("calc(1em + 5px)", &ctx), Some(15.0));
        // A signed operand with no operator between it and the previous one
        // is an implicit addition -- `1px -2px` tokenizes that way.
        assert_eq!(calc_px("calc(10px -2px)", &ctx), Some(8.0));
    }

    #[test]
    fn products_and_quotients_require_a_bare_number_on_one_side() {
        let ctx = LengthCtx::default();
        assert_eq!(calc_px("calc(3px * 4)", &ctx), Some(12.0));
        assert_eq!(calc_px("calc(4 * 3px)", &ctx), Some(12.0));
        assert_eq!(calc_px("calc(12px / 4)", &ctx), Some(3.0));
        assert_eq!(calc("calc(3px * 4px)"), None);
        assert_eq!(calc("calc(12px / 4px)"), None);
        assert_eq!(calc("calc(12px / 0)"), None);
    }

    #[test]
    fn nested_parentheses_and_math_functions_compose() {
        let ctx = LengthCtx::default();
        assert_eq!(calc_px("calc((1px + 2px) * 3)", &ctx), Some(9.0));
        assert_eq!(calc_px("min(4px, 9px, 2px)", &ctx), Some(2.0));
        assert_eq!(calc_px("max(4px, 9px, 2px)", &ctx), Some(9.0));
        assert_eq!(calc_px("clamp(2px, 9px, 5px)", &ctx), Some(5.0));
        assert_eq!(calc_px("clamp(2px, 1px, 5px)", &ctx), Some(2.0));
        assert_eq!(calc_px("calc(min(4px, 9px) + 1px)", &ctx), Some(5.0));
    }

    #[test]
    fn mixed_kinds_are_invalid_at_computed_value_time() {
        // Parses (it is a well-formed expression), never resolves.
        let ctx = LengthCtx::default();
        assert!(calc("calc(1px + 1s)").is_some());
        assert_eq!(calc_px("calc(1px + 1s)", &ctx), None);
    }

    #[test]
    fn percentages_inside_calc_use_the_context_basis() {
        let ctx = LengthCtx { percent_basis: Some(200.0), ..LengthCtx::default() };
        assert_eq!(calc_px("calc(50% + 10px)", &ctx), Some(110.0));
        assert_eq!(calc_px("calc(50% + 10px)", &LengthCtx::default()), None);
    }

    #[test]
    fn numbers_angles_and_times_have_their_own_resolvers() {
        let node = parse_entirely_with("calc(2 * 3)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(2 * 3) parses");
        assert_eq!(node.resolve_number(), Some(6.0));
        assert_eq!(node.resolve_length(&LengthCtx::default()), None);

        let angle = parse_entirely_with("calc(0.5turn)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(0.5turn) parses");
        assert_eq!(angle.resolve_angle(), Some(180.0));

        let time = parse_entirely_with("calc(200ms * 2)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(200ms * 2) parses");
        assert_eq!(time.resolve_time(), Some(Time(0.4)));
    }

    #[test]
    fn channel_variables_resolve_only_when_bound() {
        // `hsl(from #f6f5f4 h calc(s * 1.2) calc(l * 1.2))` -- 8 of Adwaita's
        // 37 @define-colors need this.
        let node = parse_entirely_with("calc(s * 1.5)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(s * 1.5) parses");
        assert_eq!(node.resolve_number(), None);
        assert_eq!(node.resolve_number_with(&[0.0, 0.4, 0.0, 1.0]), Some(0.6));

        let alpha = parse_entirely_with("calc(alpha * 0.35)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(alpha * 0.35) parses");
        assert_eq!(alpha.resolve_number_with(&[0.0, 0.0, 0.0, 1.0]), Some(0.35));
    }

    #[test]
    fn angles_accept_every_css_unit() {
        assert_eq!(parse_entirely_with("90deg", parse_angle).ok(), Some(90.0));
        assert_eq!(parse_entirely_with("0.25turn", parse_angle).ok(), Some(90.0));
        assert_eq!(parse_entirely_with("100grad", parse_angle).ok(), Some(90.0));
        assert_eq!(parse_entirely_with("0", parse_angle).ok(), Some(0.0));
        let radians = parse_entirely_with("1rad", parse_angle).expect("1rad parses");
        assert!((radians - 57.295_78).abs() < 1e-3, "{radians}");
        assert_eq!(parse_entirely_with("1px", parse_angle).ok(), None);
    }

    #[test]
    fn calc_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, |p| {
                let name = p.expect_function().map_err(|_| ())?.clone();
                super::parse_math_function(&name, p)
            });
            let _ = parse_entirely_with(input, CalcNode::parse);
            let _ = parse_entirely_with(input, parse_angle);
        }
    }
}
```

Mutation check: dropping the implicit-addition branch in `parse_sum` fails
`sums_and_differences_of_compatible_units_resolve`'s `calc(10px -2px)` case;
returning `Some` for `calc(1px + 1s)` fails
`mixed_kinds_are_invalid_at_computed_value_time`.

Append to `ui/src/css/value/timing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Time;
    use crate::css::value::parse_entirely_with;

    fn time(text: &str) -> Option<Time> {
        parse_entirely_with(text, Time::parse).ok()
    }

    #[test]
    fn seconds_and_milliseconds_both_land_in_seconds() {
        assert_eq!(time("2s"), Some(Time(2.0)));
        assert_eq!(time("200ms"), Some(Time(0.2)));
        assert_eq!(time("0S"), Some(Time(0.0)));
        assert_eq!(time("-1.5s"), Some(Time(-1.5)));
        assert_eq!(Time::from_ms(250.0), Time(0.25));
        assert_eq!(Time(0.25).as_millis_f32(), 250.0);
        assert_eq!(Time(0.25).as_secs_f32(), 0.25);
    }

    #[test]
    fn calc_reaches_time_values() {
        assert_eq!(time("calc(100ms * 3)"), Some(Time(0.3)));
        assert_eq!(time("calc(1s - 250ms)"), Some(Time(0.75)));
    }

    #[test]
    fn a_bare_number_other_than_zero_is_not_a_time() {
        assert_eq!(time("0"), Some(Time(0.0)));
        assert_eq!(time("5"), None);
        assert_eq!(time("5px"), None);
        assert_eq!(time("2s 1s"), None);
    }

    #[test]
    fn time_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Time::parse);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value`
Expected: FAIL — `error[E0583]: file not found for module 'calc'` (and `length`,
`timing`) once the `mod` lines are added, or `error[E0433]: could not find 'length'
in 'value'` before them.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/css/value/mod.rs`, above `pub mod keyword;` in alphabetical order:

```rust
pub mod calc;
pub mod keyword;
pub mod length;
pub mod timing;

pub use calc::CalcNode;
pub use keyword::{Keyword, Wide};
pub use length::{Length, LengthCtx, LengthUnit};
pub use timing::Time;
```

Create `ui/src/css/value/timing.rs` (the easing half arrives in Task 13):

```rust
//! Times and easing functions.

use cssparser::{Parser, Token};

use super::calc::parse_math_function;

/// A CSS `<time>`, always in seconds.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Time(pub f32);

impl Time {
    /// Zero seconds -- the initial value of every `*-duration`/`*-delay`.
    pub const ZERO: Time = Time(0.0);

    /// Build a time from milliseconds.
    #[must_use]
    pub fn from_ms(ms: f32) -> Time {
        Time(ms / 1000.0)
    }

    /// Seconds.
    #[must_use]
    pub fn as_secs_f32(self) -> f32 {
        self.0
    }

    /// Milliseconds.
    #[must_use]
    pub fn as_millis_f32(self) -> f32 {
        self.0 * 1000.0
    }

    /// Parse `<time>`: `s`, `ms`, a unitless `0`, or a math function.
    ///
    /// CSS proper rejects a unitless zero for `<time>`; GTK themes are
    /// written by hand and this engine accepts it, matching GTK's tolerance.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Time, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Dimension { value, ref unit, .. } => {
                if !value.is_finite() {
                    return Err(());
                }
                if unit.eq_ignore_ascii_case("s") {
                    Ok(Time(value))
                } else if unit.eq_ignore_ascii_case("ms") {
                    Ok(Time(value / 1000.0))
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } if value == 0.0 => Ok(Time::ZERO),
            Token::Function(ref name) => {
                let name = name.clone();
                parse_math_function(&name, input)?.resolve_time().ok_or(())
            }
            _ => Err(()),
        }
    }
}
```

Create `ui/src/css/value/calc.rs`:

```rust
//! `calc()`, `min()`, `max()` and `clamp()`.
//!
//! `cssparser` 0.37 has no calc AST -- a math function arrives as an
//! ordinary `Token::Function` -- so the expression tree, the parser and the
//! evaluator here are entirely project code.

use std::rc::Rc;

use cssparser::{ParseError, Parser, Token};

use super::length::{Length, LengthCtx, LengthUnit};
use super::timing::Time;

/// A parsed math expression. Nothing is resolved: units, percentages and
/// relative-colour channel variables all survive until a context exists.
#[derive(Clone, Debug, PartialEq)]
pub enum CalcNode {
    /// A bare `<number>`.
    Number(f32),
    /// A `<length>` (which may itself be a percentage or nested calc).
    Length(Length),
    /// A `<percentage>`, stored as a fraction (`50%` == `0.5`).
    Percent(f32),
    /// An `<angle>` in degrees.
    Angle(f32),
    /// A `<time>`.
    Time(Time),
    /// `a + b`.
    Sum(Rc<CalcNode>, Rc<CalcNode>),
    /// `a - b`.
    Difference(Rc<CalcNode>, Rc<CalcNode>),
    /// `a * k` (the bare number is always the right operand after parsing).
    Product(Rc<CalcNode>, f32),
    /// `a / k`.
    Quotient(Rc<CalcNode>, f32),
    /// `min(a, b, ...)`.
    Min(Rc<[CalcNode]>),
    /// `max(a, b, ...)`.
    Max(Rc<[CalcNode]>),
    /// `clamp(lo, value, hi)`.
    Clamp(Rc<CalcNode>, Rc<CalcNode>, Rc<CalcNode>),
    /// A relative-colour channel variable: `0..=2` are the origin colour's
    /// three channels in the function's own space, `3` is its alpha.
    ///
    /// Contract deviation 4: the contract's `ChannelExpr::Calc` cannot
    /// otherwise express `calc(s * 1.2)`, which 8 of Adwaita's 37
    /// `@define-color`s need.
    Var(u8),
}

/// What an expression evaluates to. Kinds never mix.
#[derive(Copy, Clone, Debug, PartialEq)]
enum CalcValue {
    Number(f32),
    Px(f32),
    Angle(f32),
    Seconds(f32),
}

impl CalcNode {
    /// Parse a math expression -- the *contents* of a `calc()` and the body
    /// of each `min()`/`max()`/`clamp()` argument.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
        parse_sum(input)
    }

    /// Resolve to device pixels. `None` == invalid at computed-value time.
    #[must_use]
    pub fn resolve_length(&self, ctx: &LengthCtx) -> Option<f32> {
        match self.eval(Some(ctx), None)? {
            CalcValue::Px(px) => finite_opt(px),
            _ => None,
        }
    }

    /// Resolve to a bare number.
    #[must_use]
    pub fn resolve_number(&self) -> Option<f32> {
        match self.eval(None, None)? {
            CalcValue::Number(n) => finite_opt(n),
            _ => None,
        }
    }

    /// Resolve to a bare number with relative-colour channels bound:
    /// `[c0, c1, c2, alpha]` in the colour function's own space.
    #[must_use]
    pub fn resolve_number_with(&self, vars: &[f32; 4]) -> Option<f32> {
        match self.eval(None, Some(vars))? {
            CalcValue::Number(n) => finite_opt(n),
            _ => None,
        }
    }

    /// Resolve to degrees.
    #[must_use]
    pub fn resolve_angle(&self) -> Option<f32> {
        match self.eval(None, None)? {
            CalcValue::Angle(deg) => finite_opt(deg),
            _ => None,
        }
    }

    /// Resolve to a time.
    #[must_use]
    pub fn resolve_time(&self) -> Option<Time> {
        match self.eval(None, None)? {
            CalcValue::Seconds(s) => finite_opt(s).map(Time),
            _ => None,
        }
    }

    fn eval(&self, ctx: Option<&LengthCtx>, vars: Option<&[f32; 4]>) -> Option<CalcValue> {
        match self {
            CalcNode::Number(n) => Some(CalcValue::Number(*n)),
            CalcNode::Percent(f) => match ctx.and_then(|c| c.percent_basis) {
                Some(basis) => Some(CalcValue::Px(f * basis)),
                None => Some(CalcValue::Number(*f)),
            },
            CalcNode::Angle(deg) => Some(CalcValue::Angle(*deg)),
            CalcNode::Time(t) => Some(CalcValue::Seconds(t.0)),
            CalcNode::Length(length) => length.resolve(ctx?).map(CalcValue::Px),
            CalcNode::Var(index) => vars
                .and_then(|v| v.get(usize::from(*index)).copied())
                .map(CalcValue::Number),
            CalcNode::Sum(a, b) => combine(a.eval(ctx, vars)?, b.eval(ctx, vars)?, |x, y| x + y),
            CalcNode::Difference(a, b) => {
                combine(a.eval(ctx, vars)?, b.eval(ctx, vars)?, |x, y| x - y)
            }
            CalcNode::Product(a, k) => Some(scale(a.eval(ctx, vars)?, *k)),
            CalcNode::Quotient(a, k) => {
                if *k == 0.0 || !k.is_finite() {
                    return None;
                }
                Some(scale(a.eval(ctx, vars)?, 1.0 / *k))
            }
            CalcNode::Min(list) => fold(list, ctx, vars, f32::min),
            CalcNode::Max(list) => fold(list, ctx, vars, f32::max),
            CalcNode::Clamp(lo, value, hi) => {
                let lo = lo.eval(ctx, vars)?;
                let value = value.eval(ctx, vars)?;
                let hi = hi.eval(ctx, vars)?;
                let clamped = combine(value, lo, f32::max)?;
                combine(clamped, hi, f32::min)
            }
        }
    }
}

fn finite_opt(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

fn scalar(value: CalcValue) -> f32 {
    match value {
        CalcValue::Number(n) | CalcValue::Px(n) | CalcValue::Angle(n) | CalcValue::Seconds(n) => n,
    }
}

fn rewrap(kind: CalcValue, scalar: f32) -> CalcValue {
    match kind {
        CalcValue::Number(_) => CalcValue::Number(scalar),
        CalcValue::Px(_) => CalcValue::Px(scalar),
        CalcValue::Angle(_) => CalcValue::Angle(scalar),
        CalcValue::Seconds(_) => CalcValue::Seconds(scalar),
    }
}

/// Combine two values of the *same* kind. A number combines with anything
/// (CSS's `<number>` is unit-neutral only for `*`/`/`, but a `calc(2 + 3)`
/// inside a length context must still work), and any other mismatch fails.
fn combine(a: CalcValue, b: CalcValue, op: fn(f32, f32) -> f32) -> Option<CalcValue> {
    let kind = match (a, b) {
        (CalcValue::Number(_), other) => other,
        (other, CalcValue::Number(_)) => other,
        (x, y) if std::mem::discriminant(&x) == std::mem::discriminant(&y) => x,
        _ => return None,
    };
    Some(rewrap(kind, op(scalar(a), scalar(b))))
}

fn scale(value: CalcValue, k: f32) -> CalcValue {
    rewrap(value, scalar(value) * k)
}

fn fold(
    list: &[CalcNode],
    ctx: Option<&LengthCtx>,
    vars: Option<&[f32; 4]>,
    op: fn(f32, f32) -> f32,
) -> Option<CalcValue> {
    let mut iter = list.iter();
    let mut acc = iter.next()?.eval(ctx, vars)?;
    for node in iter {
        acc = combine(acc, node.eval(ctx, vars)?, op)?;
    }
    Some(acc)
}

/// Parse a math function whose `Token::Function` name has already been
/// consumed. Returns `Err(())` for anything that is not `calc`, `min`,
/// `max` or `clamp`.
pub fn parse_math_function(name: &str, input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    if name.eq_ignore_ascii_case("calc") {
        input.parse_nested_block(parse_sum_entirely).map_err(|_| ())
    } else if name.eq_ignore_ascii_case("min") || name.eq_ignore_ascii_case("max") {
        let is_min = name.eq_ignore_ascii_case("min");
        let args = input
            .parse_nested_block(parse_argument_list)
            .map_err(|_| ())?;
        if args.is_empty() {
            return Err(());
        }
        let args: Rc<[CalcNode]> = args.into();
        Ok(if is_min {
            CalcNode::Min(args)
        } else {
            CalcNode::Max(args)
        })
    } else if name.eq_ignore_ascii_case("clamp") {
        let args = input
            .parse_nested_block(parse_argument_list)
            .map_err(|_| ())?;
        if args.len() != 3 {
            return Err(());
        }
        let mut iter = args.into_iter();
        let lo = iter.next().ok_or(())?;
        let value = iter.next().ok_or(())?;
        let hi = iter.next().ok_or(())?;
        Ok(CalcNode::Clamp(Rc::new(lo), Rc::new(value), Rc::new(hi)))
    } else {
        Err(())
    }
}

fn parse_sum_entirely<'i>(inner: &mut Parser<'i, '_>) -> Result<CalcNode, ParseError<'i, ()>> {
    let node = parse_sum(inner).map_err(|()| inner.new_custom_error::<(), ()>(()))?;
    inner.skip_whitespace();
    if inner.is_exhausted() {
        Ok(node)
    } else {
        Err(inner.new_custom_error::<(), ()>(()))
    }
}

fn parse_argument_list<'i>(inner: &mut Parser<'i, '_>) -> Result<Vec<CalcNode>, ParseError<'i, ()>> {
    inner.parse_comma_separated(|arg| {
        let node = parse_sum(arg).map_err(|()| arg.new_custom_error::<(), ()>(()))?;
        Ok(node)
    })
}

fn parse_sum(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let mut left = parse_product(input)?;
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                return Ok(left);
            }
        };
        match token {
            Token::Delim('+') => {
                let right = parse_product(input)?;
                left = CalcNode::Sum(Rc::new(left), Rc::new(right));
            }
            Token::Delim('-') => {
                let right = parse_product(input)?;
                left = CalcNode::Difference(Rc::new(left), Rc::new(right));
            }
            other => {
                // `calc(10px -2px)` tokenizes the second operand as a signed
                // dimension, with no delim token in between: a signed operand
                // straight after an operand is an implicit addition.
                let signed = matches!(
                    other,
                    Token::Number { has_sign: true, .. }
                        | Token::Percentage { has_sign: true, .. }
                        | Token::Dimension { has_sign: true, .. }
                );
                input.reset(&state);
                if !signed {
                    return Ok(left);
                }
                let right = parse_product(input)?;
                left = CalcNode::Sum(Rc::new(left), Rc::new(right));
            }
        }
    }
}

fn parse_product(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let mut left = parse_unary(input)?;
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                return Ok(left);
            }
        };
        match token {
            Token::Delim('*') => {
                let right = parse_unary(input)?;
                left = match (right.resolve_number(), left.resolve_number()) {
                    (Some(k), _) => CalcNode::Product(Rc::new(left), k),
                    (None, Some(k)) => CalcNode::Product(Rc::new(right), k),
                    (None, None) => return Err(()),
                };
            }
            Token::Delim('/') => {
                let right = parse_unary(input)?;
                let k = right.resolve_number().ok_or(())?;
                if k == 0.0 || !k.is_finite() {
                    return Err(());
                }
                left = CalcNode::Quotient(Rc::new(left), k);
            }
            _ => {
                input.reset(&state);
                return Ok(left);
            }
        }
    }
}

fn parse_unary(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } => {
            if value.is_finite() {
                Ok(CalcNode::Number(value))
            } else {
                Err(())
            }
        }
        Token::Percentage { unit_value, .. } => {
            if unit_value.is_finite() {
                Ok(CalcNode::Percent(unit_value))
            } else {
                Err(())
            }
        }
        Token::Dimension { value, ref unit, .. } => {
            if !value.is_finite() {
                return Err(());
            }
            if let Some(length_unit) = LengthUnit::from_str_ascii_ci(unit) {
                Ok(CalcNode::Length(Length::Abs { value, unit: length_unit }))
            } else if let Some(degrees) = angle_to_degrees(value, unit) {
                Ok(CalcNode::Angle(degrees))
            } else if unit.eq_ignore_ascii_case("s") {
                Ok(CalcNode::Time(Time(value)))
            } else if unit.eq_ignore_ascii_case("ms") {
                Ok(CalcNode::Time(Time(value / 1000.0)))
            } else {
                Err(())
            }
        }
        Token::Ident(ref name) => channel_var(name).map(CalcNode::Var).ok_or(()),
        Token::ParenthesisBlock => input.parse_nested_block(parse_sum_entirely).map_err(|_| ()),
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)
        }
        _ => Err(()),
    }
}

/// The only bare idents a math expression accepts are the relative-colour
/// channel names of the two spaces GTK themes actually use, `srgb` and
/// `hsl` (`rgb(from ...)` / `hsl(from ...)`). Index `3` is always alpha.
fn channel_var(name: &str) -> Option<u8> {
    if name.eq_ignore_ascii_case("r") || name.eq_ignore_ascii_case("h") {
        Some(0)
    } else if name.eq_ignore_ascii_case("g") || name.eq_ignore_ascii_case("s") {
        Some(1)
    } else if name.eq_ignore_ascii_case("b") || name.eq_ignore_ascii_case("l") {
        Some(2)
    } else if name.eq_ignore_ascii_case("alpha") {
        Some(3)
    } else {
        None
    }
}

fn angle_to_degrees(value: f32, unit: &str) -> Option<f32> {
    if unit.eq_ignore_ascii_case("deg") {
        Some(value)
    } else if unit.eq_ignore_ascii_case("grad") {
        Some(value * 360.0 / 400.0)
    } else if unit.eq_ignore_ascii_case("rad") {
        Some(value.to_degrees())
    } else if unit.eq_ignore_ascii_case("turn") {
        Some(value * 360.0)
    } else {
        None
    }
}

/// Parse an `<angle>` -- any CSS angle unit, a unitless `0`, or a math
/// function that resolves to one. Always returns degrees.
pub fn parse_angle(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Dimension { value, ref unit, .. } => {
            if !value.is_finite() {
                return Err(());
            }
            angle_to_degrees(value, unit).ok_or(())
        }
        Token::Number { value, .. } if value == 0.0 => Ok(0.0),
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)?.resolve_angle().ok_or(())
        }
        _ => Err(()),
    }
}
```

Create `ui/src/css/value/length.rs`:

```rust
//! Lengths and the context that turns them into device pixels.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::{CalcNode, parse_math_function};

/// Every absolute or font-relative unit GTK accepts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LengthUnit {
    /// CSS pixel.
    Px,
    /// Point.
    Pt,
    /// Pica.
    Pc,
    /// Inch.
    In,
    /// Centimetre.
    Cm,
    /// Millimetre.
    Mm,
    /// The element's own font size.
    Em,
    /// The resolved face's x-height.
    Ex,
    /// GTK's *initial* font size -- not the root node's (GTK note, §4).
    Rem,
}

impl LengthUnit {
    /// ASCII-case-insensitive unit lookup.
    #[must_use]
    pub fn from_str_ascii_ci(unit: &str) -> Option<LengthUnit> {
        const UNITS: &[(&str, LengthUnit)] = &[
            ("px", LengthUnit::Px),
            ("pt", LengthUnit::Pt),
            ("pc", LengthUnit::Pc),
            ("in", LengthUnit::In),
            ("cm", LengthUnit::Cm),
            ("mm", LengthUnit::Mm),
            ("em", LengthUnit::Em),
            ("ex", LengthUnit::Ex),
            ("rem", LengthUnit::Rem),
        ];
        UNITS
            .iter()
            .find(|(name, _)| unit.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }

    /// The unit's CSS spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LengthUnit::Px => "px",
            LengthUnit::Pt => "pt",
            LengthUnit::Pc => "pc",
            LengthUnit::In => "in",
            LengthUnit::Cm => "cm",
            LengthUnit::Mm => "mm",
            LengthUnit::Em => "em",
            LengthUnit::Ex => "ex",
            LengthUnit::Rem => "rem",
        }
    }

    /// Physical units convert through `-gtk-dpi`, **not** CSS's fixed 96dpi.
    fn to_px(self, value: f32, ctx: &LengthCtx) -> f32 {
        match self {
            LengthUnit::Px => value,
            LengthUnit::Pt => value * ctx.dpi / 72.0,
            LengthUnit::Pc => value * ctx.dpi / 6.0,
            LengthUnit::In => value * ctx.dpi,
            LengthUnit::Cm => value * ctx.dpi / 2.54,
            LengthUnit::Mm => value * ctx.dpi / 25.4,
            LengthUnit::Em => value * ctx.font_size_px,
            LengthUnit::Ex => value * ctx.font_size_px * ctx.ex_ratio,
            LengthUnit::Rem => value * ctx.root_font_size_px,
        }
    }
}

/// A `<length>`, `<percentage>`, math expression or `auto`, unresolved.
#[derive(Clone, Debug, PartialEq)]
pub enum Length {
    /// A dimension with a unit.
    Abs {
        /// The numeric part.
        value: f32,
        /// The unit.
        unit: LengthUnit,
    },
    /// A percentage, stored as a fraction (`100%` == `1.0`).
    Percent(f32),
    /// A math expression.
    Calc(Rc<CalcNode>),
    /// `auto` -- valid on `margin-*` and `border-image-width` only.
    Auto,
}

/// Everything needed to turn a [`Length`] into device pixels.
#[derive(Copy, Clone, Debug)]
pub struct LengthCtx {
    /// This element's computed `font-size`.
    pub font_size_px: f32,
    /// GTK's *initial* `font-size` -- the `rem` base, not the root node's.
    pub root_font_size_px: f32,
    /// x-height / em of the resolved face; `0.5` when unknown.
    pub ex_ratio: f32,
    /// `-gtk-dpi`; `pt`/`pc`/`in`/`cm`/`mm` convert through this.
    pub dpi: f32,
    /// `None` == percentages are invalid in this position.
    pub percent_basis: Option<f32>,
}

impl Default for LengthCtx {
    fn default() -> Self {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }
}

impl Length {
    /// Zero pixels.
    #[must_use]
    pub fn zero() -> Length {
        Length::px(0.0)
    }

    /// `value` pixels.
    #[must_use]
    pub fn px(value: f32) -> Length {
        Length::Abs { value, unit: LengthUnit::Px }
    }

    /// Parse a `<length-percentage>` or math function. `auto` is rejected --
    /// use [`Length::parse_allowing_auto`] on the two properties that take it.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Dimension { value, ref unit, .. } => {
                if !value.is_finite() {
                    return Err(());
                }
                let unit = LengthUnit::from_str_ascii_ci(unit).ok_or(())?;
                Ok(Length::Abs { value, unit })
            }
            Token::Percentage { unit_value, .. } => {
                if unit_value.is_finite() {
                    Ok(Length::Percent(unit_value))
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } if value == 0.0 => Ok(Length::zero()),
            Token::Function(ref name) => {
                let name = name.clone();
                Ok(Length::Calc(Rc::new(parse_math_function(&name, input)?)))
            }
            _ => Err(()),
        }
    }

    /// [`Length::parse`], plus the `auto` keyword.
    pub fn parse_allowing_auto(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
        let state = input.state();
        if let Ok(token) = input.next() {
            if let Token::Ident(name) = token {
                if name.eq_ignore_ascii_case("auto") {
                    return Ok(Length::Auto);
                }
            }
        }
        input.reset(&state);
        Length::parse(input)
    }

    /// Resolve to device pixels. `None` == invalid at computed-value time:
    /// `auto`, a percentage with no basis, or a non-finite result.
    #[must_use]
    pub fn resolve(&self, ctx: &LengthCtx) -> Option<f32> {
        let px = match self {
            Length::Abs { value, unit } => unit.to_px(*value, ctx),
            Length::Percent(fraction) => fraction * ctx.percent_basis?,
            Length::Calc(node) => node.resolve_length(ctx)?,
            Length::Auto => return None,
        };
        px.is_finite().then_some(px)
    }
}
```

Append each test module from Step 1 to its file.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value`
Expected: PASS — 9 length tests, 8 calc tests, 4 time tests, 5 keyword tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/calc.rs \
        ui/src/css/value/length.rs ui/src/css/value/timing.rs
git commit -m "feat(ui/css): lengths, a calc() evaluator and times

cssparser 0.37 gives no calc AST, so the expression tree, recursive-descent
parser and unit-checked evaluator are project code. Physical units convert
through -gtk-dpi rather than CSS's fixed 96dpi, per GTK's own note.
CalcNode::Var carries relative-colour channel variables.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 4: Colours — types, base grammar and resolution

`css/colors.rs` stays on disk (deviation 1) serving M1's cascade until Part 3
deletes it; its grammar is re-expressed here against a lazily-resolved
`ColorValue`. Seven of its ten tests move here verbatim in their assertions; the
three table-driven ones move in Task 5.

**Files:**
- Create: `ui/src/css/value/color.rs`
- Modify: `ui/src/css/value/mod.rs` (declare + re-export `color`)
- Test: `ui/src/css/value/color.rs`

**Interfaces:**
- Consumes: `Length`/`LengthCtx` (Task 3), `calc::parse_math_function`,
  `calc::parse_angle`, `css::tokens::serialize_remaining` (Task 1).
- Produces:
  - `css::value::color::{Rgba, ColorSpace, ColorValue, ChannelExpr, LegacyColorFn, ColorTable, ColorCtx}`
  - `Rgba { r, g, b, a }` (unpremultiplied sRGB), `Rgba::TRANSPARENT`,
    `Rgba::to_color32(self) -> skia_rs_safe::core::Color`,
    `Rgba::from_color32(skia_rs_safe::core::Color) -> Rgba`,
    `Rgba::clamped(self) -> Rgba`,
    `Rgba::premultiplied(self) -> [f32; 4]`, `Rgba::from_premultiplied([f32; 4]) -> Rgba`
  - `ColorValue::parse(&mut Parser<'_, '_>) -> Result<ColorValue, ()>`
  - `ColorValue::resolve(&self, &ColorCtx<'_>) -> Option<Rgba>`
  - `ColorTable = std::collections::HashMap<String, ColorValue>`
  - `ColorCtx<'a> { table: &'a ColorTable, current: Rgba, depth: u8 }`, `Copy`
  - `ColorCtx::MAX_DEPTH: u8 = 32`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/color.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{ColorCtx, ColorTable, ColorValue, Rgba};
    use crate::css::value::parse_entirely_with;
    use skia_rs_safe::core::Color;

    fn table_with(name: &str, value: &str) -> ColorTable {
        let mut table = ColorTable::new();
        table.insert(
            name.to_string(),
            parse_entirely_with(value, ColorValue::parse).expect("fixture colour parses"),
        );
        table
    }

    /// The M1 `parse_color_value` shape: a whole value, resolved against a
    /// table, with `currentColor` reported as "no value".
    fn color_value(text: &str, table: &ColorTable) -> Option<Color> {
        let value = parse_entirely_with(text, ColorValue::parse).ok()?;
        if value == ColorValue::CurrentColor {
            return None;
        }
        let ctx = ColorCtx { table, current: Rgba::TRANSPARENT, depth: 0 };
        value.resolve(&ctx).map(Rgba::to_color32)
    }

    /// The M1 `parse_color_ref` shape: the unresolved value.
    fn color_ref(text: &str, table: &ColorTable) -> Option<ColorValue> {
        parse_entirely_with(text, ColorValue::parse).ok()
    }

    #[test]
    fn literal_forms_delegate_to_skias_css_parser() {
        let table = ColorTable::new();
        assert_eq!(color_value("#2e3436", &table), Some(Color(0xFF2E_3436)));
        assert_eq!(color_value("white", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(color_value("rgb(53, 132, 228)", &table), Some(Color(0xFF35_84E4)));
        assert_eq!(color_value("transparent", &table), Some(Color(0x0000_0000)));
        assert_eq!(color_value("linear-gradient(to top, red, blue)", &table), None);
        // CSS Color 4 spaces route through skia's own parser.
        assert_eq!(color_value("oklch(0.5 0 0)", &table).is_some(), true);
    }

    #[test]
    fn short_hex_with_non_ascii_bytes_does_not_panic() {
        // E8: `Color::from_css("#a\u{e9}")` panicked -- "byte index 2 is not a
        // char boundary" -- so one odd byte in a theme took the process down.
        let table = ColorTable::new();
        for input in [
            "#a\u{e9}",
            "#\u{e9}\u{e9}\u{e9}",
            "#\u{1f600}\u{1f600}\u{1f600}",
            "#",
            "#1",
            "#12",
            "#12345",
            "#1234567",
            "#123456789",
            "#zzz",
            "#zzzzzz",
            "rgb(",
            "rgb()",
            "rgb(1,2)",
            "rgb(1,2,3,4,5)",
            "hsl(\u{e9})",
            "\u{e9}",
            "@",
            "@\u{e9}",
            "",
            "   ",
            "()",
            "image(#a\u{e9})",
        ] {
            let _ = color_value(input, &table);
        }
    }

    #[test]
    fn colour_syntax_is_ascii_case_insensitive() {
        let table = table_with("borders", "#CDC7C2");
        assert_eq!(color_value("#2E3436", &table), Some(Color(0xFF2E_3436)));
        assert_eq!(color_value("WHITE", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(color_value("RGB(53, 132, 228)", &table), Some(Color(0xFF35_84E4)));
        assert_eq!(color_value("RGBA(255, 255, 255, 0.8)", &table), Some(Color(0xCCFF_FFFF)));
        assert_eq!(color_value("TRANSPARENT", &table), Some(Color(0x0000_0000)));
        assert_eq!(color_value("@borders", &table), Some(Color(0xFFCD_C7C2)));
    }

    #[test]
    fn css_color_4_space_separated_syntax_parses() {
        // E7: `rgb(53 132 228)` -- the syntax GTK 4.14+ themes emit --
        // silently dropped, because the old parser split on commas only.
        let table = ColorTable::new();
        assert_eq!(color_value("rgb(53 132 228)", &table), Some(Color(0xFF35_84E4)));
        assert_eq!(color_value("rgb(53 132 228 / 0.5)", &table), Some(Color(0x8035_84E4)));
        assert_eq!(color_value("rgba(53 132 228 / 50%)", &table), Some(Color(0x8035_84E4)));
        assert_eq!(color_value("rgb(0% 100% 0%)", &table), Some(Color(0xFF00_FF00)));
        assert_eq!(color_value("hsl(120 100% 50%)", &table), Some(Color(0xFF00_FF00)));
        assert_eq!(color_value("hsl(120, 100%, 50%)", &table), Some(Color(0xFF00_FF00)));
        assert_eq!(color_value("hsla(120 100% 50% / 0.5)", &table), Some(Color(0x8000_FF00)));
        // Out-of-range channels clamp rather than wrapping or failing.
        assert_eq!(color_value("rgb(300 -20 0)", &table), Some(Color(0xFFFF_0000)));
    }

    #[test]
    fn four_and_eight_digit_hex_carry_alpha() {
        let table = ColorTable::new();
        assert_eq!(color_value("#f00f", &table), Some(Color(0xFFFF_0000)));
        assert_eq!(color_value("#FF000080", &table), Some(Color(0x80FF_0000)));
    }

    #[test]
    fn current_color_is_a_marker_not_a_colour() {
        let table = ColorTable::new();
        assert_eq!(color_ref("currentColor", &table), Some(ColorValue::CurrentColor));
        assert_eq!(color_ref("CURRENTCOLOR", &table), Some(ColorValue::CurrentColor));
        // The plain accessor cannot invent one, so it reports no value.
        assert_eq!(color_value("currentColor", &table), None);
        let ctx = ColorCtx { table: &table, current: Rgba::TRANSPARENT, depth: 0 };
        assert_eq!(
            color_ref("#2e3436", &table)
                .and_then(|value| value.resolve(&ctx))
                .map(Rgba::to_color32),
            Some(Color(0xFF2E_3436))
        );
    }

    #[test]
    fn trailing_junk_is_rejected() {
        let table = ColorTable::new();
        assert_eq!(color_value("#2e3436 red", &table), None);
        assert_eq!(color_value("red blue", &table), None);
    }

    #[test]
    fn current_color_resolves_against_the_context_not_a_constant() {
        let table = ColorTable::new();
        let ctx = ColorCtx {
            table: &table,
            current: Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 },
            depth: 0,
        };
        assert_eq!(
            ColorValue::CurrentColor.resolve(&ctx).map(Rgba::to_color32),
            Some(Color(0xFFFF_0000))
        );
    }

    #[test]
    fn an_unknown_name_and_a_cycle_are_both_invalid_at_computed_value_time() {
        let mut table = ColorTable::new();
        // a -> b -> a
        table.insert("a".to_string(), ColorValue::Named("b".into()));
        table.insert("b".to_string(), ColorValue::Named("a".into()));
        let ctx = ColorCtx { table: &table, current: Rgba::TRANSPARENT, depth: 0 };
        assert_eq!(ColorValue::Named("a".into()).resolve(&ctx), None);
        assert_eq!(ColorValue::Named("nope".into()).resolve(&ctx), None);
    }

    #[test]
    fn byte_conversion_rounds_to_nearest() {
        // The offscreen pixel gate compares bytes, so 0.8 * 255 must be 204,
        // not 203 (truncation).
        assert_eq!(
            Rgba { r: 1.0, g: 1.0, b: 1.0, a: 0.8 }.to_color32(),
            Color(0xCCFF_FFFF)
        );
        assert_eq!(Rgba::from_color32(Color(0xFF35_84E4)).to_color32(), Color(0xFF35_84E4));
    }

    #[test]
    fn colour_parsing_never_panics() {
        let table = ColorTable::new();
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = color_value(input, &table);
        }
    }
}
```

Mutation check: `byte_conversion_rounds_to_nearest` fails if `to_color32` truncates
instead of rounding — the exact difference between `#CCFFFFFF` and `#CBFFFFFF` in
`wm_highlight`. `an_unknown_name_and_a_cycle_are_both_invalid_at_computed_value_time`
fails (stack-overflows) if the `depth` guard is removed.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::color`
Expected: FAIL — `error[E0433]: failed to resolve: could not find 'color' in 'value'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod color;` and `pub use color::{ColorCtx, ColorSpace, ColorTable, ColorValue, Rgba};`
to `ui/src/css/value/mod.rs`.

Create `ui/src/css/value/color.rs`:

```rust
//! Colours: the grammar, the lazily-resolved value, and resolution.
//!
//! Nothing resolves at parse time. `@name` survives as
//! [`ColorValue::Named`], `currentColor` as [`ColorValue::CurrentColor`],
//! and `color-mix()`/relative/legacy expressions survive as trees, so one
//! parsed sheet can be resolved against several colour tables and several
//! `color` values.

use std::collections::HashMap;
use std::rc::Rc;

use cssparser::{Parser, Token};
use skia_rs_safe::core::{Color, hsl_to_rgb, rgb_to_hsl};

use super::calc::{CalcNode, parse_angle, parse_math_function};

/// An unpremultiplied sRGB colour with components in `0..=1`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rgba {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
    /// Alpha.
    pub a: f32,
}

impl Rgba {
    /// Fully transparent black.
    pub const TRANSPARENT: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    /// Convert to skia's packed `0xAARRGGBB`, rounding each channel to the
    /// nearest byte. Round-to-nearest is load-bearing: the offscreen pixel
    /// gate compares bytes against the theme's declared hex.
    #[must_use]
    pub fn to_color32(self) -> Color {
        fn byte(value: f32) -> u8 {
            if value.is_nan() {
                return 0;
            }
            (value.clamp(0.0, 1.0) * 255.0).round() as u8
        }
        Color::from_argb(byte(self.a), byte(self.r), byte(self.g), byte(self.b))
    }

    /// Convert from skia's packed `0xAARRGGBB`.
    #[must_use]
    pub fn from_color32(color: Color) -> Rgba {
        Rgba {
            r: f32::from(color.red()) / 255.0,
            g: f32::from(color.green()) / 255.0,
            b: f32::from(color.blue()) / 255.0,
            a: f32::from(color.alpha()) / 255.0,
        }
    }

    /// Clamp every channel into `0..=1`, mapping NaN to `0`.
    #[must_use]
    pub fn clamped(self) -> Rgba {
        fn unit(value: f32) -> f32 {
            if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) }
        }
        Rgba { r: unit(self.r), g: unit(self.g), b: unit(self.b), a: unit(self.a) }
    }

    /// Premultiplied `[r, g, b, a]` -- the space GTK interpolates colours in.
    #[must_use]
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// Inverse of [`Rgba::premultiplied`].
    #[must_use]
    pub fn from_premultiplied(components: [f32; 4]) -> Rgba {
        let a = components[3];
        if a <= 0.0 {
            return Rgba::TRANSPARENT;
        }
        Rgba { r: components[0] / a, g: components[1] / a, b: components[2] / a, a }
    }
}

/// The colour spaces `color-mix()` and relative syntax name.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// Gamma-encoded sRGB.
    Srgb,
    /// Linear-light sRGB.
    SrgbLinear,
    /// HSL.
    Hsl,
    /// HWB.
    Hwb,
    /// CIE Lab.
    Lab,
    /// CIE LCh.
    Lch,
    /// Oklab.
    Oklab,
    /// Oklch.
    Oklch,
}

impl ColorSpace {
    /// ASCII-case-insensitive lookup of a space name.
    #[must_use]
    pub fn from_str_ascii_ci(name: &str) -> Option<ColorSpace> {
        const SPACES: &[(&str, ColorSpace)] = &[
            ("srgb", ColorSpace::Srgb),
            ("srgb-linear", ColorSpace::SrgbLinear),
            ("hsl", ColorSpace::Hsl),
            ("hwb", ColorSpace::Hwb),
            ("lab", ColorSpace::Lab),
            ("lch", ColorSpace::Lch),
            ("oklab", ColorSpace::Oklab),
            ("oklch", ColorSpace::Oklch),
        ];
        SPACES
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, space)| *space)
    }
}

/// An unresolved colour.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorValue {
    /// A literal colour.
    Absolute(Rgba),
    /// `currentColor` -- the element's own computed `color`.
    CurrentColor,
    /// `@name` -- resolved lazily against the [`ColorTable`].
    Named(Rc<str>),
    /// `color-mix(in <space>, a <wa>?, b <wb>?)`.
    Mix {
        /// Interpolation space.
        space: ColorSpace,
        /// First colour.
        a: Rc<ColorValue>,
        /// First weight, as a fraction.
        wa: Option<f32>,
        /// Second colour.
        b: Rc<ColorValue>,
        /// Second weight, as a fraction.
        wb: Option<f32>,
    },
    /// `rgb(from <origin> ...)` / `hsl(from <origin> ...)`.
    Relative {
        /// The function's space.
        space: ColorSpace,
        /// The origin colour.
        origin: Rc<ColorValue>,
        /// The three channel expressions.
        channels: [ChannelExpr; 3],
        /// The alpha expression; `None` keeps the origin's alpha.
        alpha: Option<ChannelExpr>,
    },
    /// GTK's deprecated colour expressions.
    Legacy(Rc<LegacyColorFn>),
}

/// One channel of a relative colour.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelExpr {
    /// A bare channel name: keep the origin's value for that channel.
    Keep(u8),
    /// A literal number, in the space's own units.
    Number(f32),
    /// A literal percentage, as a fraction.
    Percent(f32),
    /// A math expression over the channel variables.
    Calc(Rc<CalcNode>),
}

impl ChannelExpr {
    fn resolve(&self, vars: &[f32; 4], space: ColorSpace, index: usize) -> Option<f32> {
        match self {
            ChannelExpr::Keep(channel) => vars.get(usize::from(*channel)).copied(),
            ChannelExpr::Number(value) => Some(match (space, index) {
                // In an HSL-space relative colour, hue is degrees and
                // saturation/lightness are already fractions.
                (ColorSpace::Hsl, _) => *value,
                // Everywhere else a bare number is an 0..255 sRGB channel.
                _ => value / 255.0,
            }),
            ChannelExpr::Percent(fraction) => Some(match (space, index) {
                (ColorSpace::Hsl, 0) => fraction * 360.0,
                _ => *fraction,
            }),
            ChannelExpr::Calc(node) => node.resolve_number_with(vars),
        }
    }
}

/// GTK's deprecated colour expressions, still used by real themes.
#[derive(Clone, Debug, PartialEq)]
pub enum LegacyColorFn {
    /// `alpha(c, k)` -- multiplies alpha by `k`.
    Alpha(ColorValue, f32),
    /// `shade(c, k)` -- scales HSL saturation and lightness by `k`
    /// (`0` == black, `2` == white), matching GTK's `_gtk_hsla_shade`.
    Shade(ColorValue, f32),
    /// `mix(a, b, k)` -- interpolates, `0` == `a`, `1` == `b`.
    Mix(ColorValue, ColorValue, f32),
    /// `lighter(c)` == `shade(c, 1.3)`.
    Lighter(ColorValue),
    /// `darker(c)` == `shade(c, 0.7)`.
    Darker(ColorValue),
}

/// `@define-color` definitions, **unresolved**.
pub type ColorTable = HashMap<String, ColorValue>;

/// Everything colour resolution needs.
#[derive(Copy, Clone, Debug)]
pub struct ColorCtx<'a> {
    /// The `@define-color` table.
    pub table: &'a ColorTable,
    /// The element's own computed `color`, for `currentColor`.
    pub current: Rgba,
    /// Recursion depth; the cycle guard.
    pub depth: u8,
}

impl ColorCtx<'_> {
    /// Hard cap on `@name` / nested-expression recursion.
    pub const MAX_DEPTH: u8 = 32;

    fn deeper(&self) -> Option<ColorCtx<'_>> {
        (self.depth < ColorCtx::MAX_DEPTH).then(|| ColorCtx {
            table: self.table,
            current: self.current,
            depth: self.depth + 1,
        })
    }
}

impl ColorValue {
    /// Resolve to a concrete colour. `None` == invalid at computed-value
    /// time: an unknown `@name`, a definition cycle, or a non-finite channel.
    #[must_use]
    pub fn resolve(&self, ctx: &ColorCtx<'_>) -> Option<Rgba> {
        match self {
            ColorValue::Absolute(rgba) => Some(*rgba),
            ColorValue::CurrentColor => Some(ctx.current),
            ColorValue::Named(name) => {
                let deeper = ctx.deeper()?;
                ctx.table.get(name.as_ref())?.resolve(&deeper)
            }
            ColorValue::Mix { space, a, wa, b, wb } => {
                let deeper = ctx.deeper()?;
                let a_rgba = a.resolve(&deeper)?;
                let b_rgba = b.resolve(&deeper)?;
                let (pa, pb) = normalize_weights(*wa, *wb)?;
                Some(mix_in_space(*space, a_rgba, b_rgba, pb / (pa + pb)))
            }
            ColorValue::Relative { space, origin, channels, alpha } => {
                let deeper = ctx.deeper()?;
                let origin = origin.resolve(&deeper)?.clamped();
                let vars = match space {
                    ColorSpace::Hsl => {
                        let (h, s, l) = rgb_to_hsl(origin.r, origin.g, origin.b);
                        [h, s, l, origin.a]
                    }
                    _ => [origin.r, origin.g, origin.b, origin.a],
                };
                let mut out = [0.0_f32; 3];
                for (index, expr) in channels.iter().enumerate() {
                    out[index] = expr.resolve(&vars, *space, index)?;
                }
                let a = match alpha {
                    Some(expr) => expr.resolve(&vars, *space, 3)?,
                    None => vars[3],
                };
                let rgba = match space {
                    ColorSpace::Hsl => {
                        let (r, g, b) = hsl_to_rgb(
                            out[0].rem_euclid(360.0),
                            out[1].clamp(0.0, 1.0),
                            out[2].clamp(0.0, 1.0),
                        );
                        Rgba { r, g, b, a }
                    }
                    _ => Rgba { r: out[0], g: out[1], b: out[2], a },
                };
                finite(rgba).map(Rgba::clamped)
            }
            ColorValue::Legacy(function) => {
                let deeper = ctx.deeper()?;
                match function.as_ref() {
                    LegacyColorFn::Alpha(color, factor) => {
                        let base = color.resolve(&deeper)?;
                        finite(Rgba { a: base.a * factor, ..base }).map(Rgba::clamped)
                    }
                    LegacyColorFn::Shade(color, factor) => {
                        shade(color.resolve(&deeper)?, *factor)
                    }
                    LegacyColorFn::Lighter(color) => shade(color.resolve(&deeper)?, 1.3),
                    LegacyColorFn::Darker(color) => shade(color.resolve(&deeper)?, 0.7),
                    LegacyColorFn::Mix(a, b, factor) => {
                        let a = a.resolve(&deeper)?;
                        let b = b.resolve(&deeper)?;
                        finite(mix_in_space(ColorSpace::Srgb, a, b, *factor)).map(Rgba::clamped)
                    }
                }
            }
        }
    }

    /// Parse a whole `<color>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Hash(ref digits) | Token::IDHash(ref digits) => {
                parse_hex(digits).map(ColorValue::Absolute).ok_or(())
            }
            Token::AtKeyword(ref name) => Ok(ColorValue::Named(Rc::from(name.as_ref()))),
            Token::Ident(ref name) => {
                if name.eq_ignore_ascii_case("currentcolor") {
                    Ok(ColorValue::CurrentColor)
                } else if name.eq_ignore_ascii_case("transparent") {
                    Ok(ColorValue::Absolute(Rgba::TRANSPARENT))
                } else {
                    named_color(name).map(ColorValue::Absolute).ok_or(())
                }
            }
            Token::Function(ref name) => {
                let name = name.clone();
                parse_color_function(&name, input)
            }
            _ => Err(()),
        }
    }
}

fn finite(rgba: Rgba) -> Option<Rgba> {
    (rgba.r.is_finite() && rgba.g.is_finite() && rgba.b.is_finite() && rgba.a.is_finite())
        .then_some(rgba)
}

/// GTK's `_gtk_hsla_shade`: scale HSL saturation *and* lightness by `factor`.
fn shade(base: Rgba, factor: f32) -> Option<Rgba> {
    if !factor.is_finite() {
        return None;
    }
    let base = base.clamped();
    let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
    let (r, g, b) = hsl_to_rgb(h, (s * factor).clamp(0.0, 1.0), (l * factor).clamp(0.0, 1.0));
    finite(Rgba { r, g, b, a: base.a }).map(Rgba::clamped)
}

/// `t == 0` is `a`, `t == 1` is `b`. Premultiplied, as GTK interpolates.
fn mix_in_space(space: ColorSpace, a: Rgba, b: Rgba, t: f32) -> Rgba {
    if space == ColorSpace::Hsl {
        let (ha, sa, la) = rgb_to_hsl(a.r, a.g, a.b);
        let (hb, sb, lb) = rgb_to_hsl(b.r, b.g, b.b);
        // Shorter hue arc, per CSS Color 4's default `shorter hue`.
        let mut delta = hb - ha;
        if delta > 180.0 {
            delta -= 360.0;
        } else if delta < -180.0 {
            delta += 360.0;
        }
        let (r, g, blue) = hsl_to_rgb(
            (ha + delta * t).rem_euclid(360.0),
            sa + (sb - sa) * t,
            la + (lb - la) * t,
        );
        return Rgba { r, g, b: blue, a: a.a + (b.a - a.a) * t }.clamped();
    }
    // Every non-HSL space (including the Lab/Oklab family, which this
    // engine does not model channel-wise) mixes in premultiplied sRGB.
    let pa = a.premultiplied();
    let pb = b.premultiplied();
    let mut out = [0.0_f32; 4];
    for index in 0..4 {
        out[index] = pa[index] + (pb[index] - pa[index]) * t;
    }
    Rgba::from_premultiplied(out).clamped()
}

/// CSS `color-mix()` weight normalization: both omitted == 50/50; one given
/// == the other is its complement; both given and summing to 0 is invalid.
fn normalize_weights(wa: Option<f32>, wb: Option<f32>) -> Option<(f32, f32)> {
    let (a, b) = match (wa, wb) {
        (None, None) => (0.5, 0.5),
        (Some(a), None) => (a, 1.0 - a),
        (None, Some(b)) => (1.0 - b, b),
        (Some(a), Some(b)) => (a, b),
    };
    if !a.is_finite() || !b.is_finite() || a < 0.0 || b < 0.0 || a + b <= 0.0 {
        return None;
    }
    Some((a, b))
}

fn parse_hex(digits: &str) -> Option<Rgba> {
    if !digits.is_ascii() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    fn nibble(byte: u8) -> f32 {
        f32::from(char::from(byte).to_digit(16).unwrap_or(0) as u8)
    }
    let bytes = digits.as_bytes();
    let channel = |high: u8, low: u8| (nibble(high) * 16.0 + nibble(low)) / 255.0;
    let short = |value: u8| (nibble(value) * 17.0) / 255.0;
    match bytes.len() {
        3 => Some(Rgba { r: short(bytes[0]), g: short(bytes[1]), b: short(bytes[2]), a: 1.0 }),
        4 => Some(Rgba {
            r: short(bytes[0]),
            g: short(bytes[1]),
            b: short(bytes[2]),
            a: short(bytes[3]),
        }),
        6 => Some(Rgba {
            r: channel(bytes[0], bytes[1]),
            g: channel(bytes[2], bytes[3]),
            b: channel(bytes[4], bytes[5]),
            a: 1.0,
        }),
        8 => Some(Rgba {
            r: channel(bytes[0], bytes[1]),
            g: channel(bytes[2], bytes[3]),
            b: channel(bytes[4], bytes[5]),
            a: channel(bytes[6], bytes[7]),
        }),
        _ => None,
    }
}

/// CSS Color 3 named colours, via skia's own table.
fn named_color(name: &str) -> Option<Rgba> {
    if !name.is_ascii() {
        return None;
    }
    Color::from_css(&name.to_ascii_lowercase()).map(Rgba::from_color32)
}

/// One channel component of a colour function.
#[derive(Copy, Clone, Debug)]
enum Channel {
    Number(f32),
    Percent(f32),
}

fn parse_channel(input: &mut Parser<'_, '_>) -> Result<Channel, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Err(());
        }
    };
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(Channel::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(Channel::Percent(unit_value))
        }
        Token::Function(ref name) => {
            let name = name.clone();
            let number = parse_math_function(&name, input)?.resolve_number().ok_or(())?;
            Ok(Channel::Number(number))
        }
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// Consume an optional comma -- colour functions accept both the CSS 3
/// comma syntax and the CSS 4 space syntax.
fn eat_comma(input: &mut Parser<'_, '_>) {
    let state = input.state();
    let is_comma = matches!(input.next(), Ok(Token::Comma));
    if !is_comma {
        input.reset(&state);
    }
}

/// Consume `, <alpha>` or `/ <alpha>` if present.
fn parse_optional_alpha(input: &mut Parser<'_, '_>) -> Result<Option<Channel>, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Ok(None);
        }
    };
    match token {
        Token::Comma | Token::Delim('/') => parse_channel(input).map(Some),
        _ => {
            input.reset(&state);
            Ok(None)
        }
    }
}

fn rgb_channel(channel: Channel) -> f32 {
    match channel {
        Channel::Number(value) => value / 255.0,
        Channel::Percent(fraction) => fraction,
    }
}

fn unit_channel(channel: Channel) -> f32 {
    match channel {
        Channel::Number(value) => value,
        Channel::Percent(fraction) => fraction,
    }
}

/// Dispatch a colour function whose name token has already been consumed.
///
/// `color-mix()`, the relative `from` forms and GTK's legacy expressions
/// are added in Task 5; the Level 4 spaces route through skia's parser
/// because this engine models only sRGB and HSL channel-wise.
fn parse_color_function(name: &str, input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") {
        nested(input, parse_rgb_body)
    } else if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") {
        nested(input, parse_hsl_body)
    } else if name.eq_ignore_ascii_case("hwb") {
        nested(input, parse_hwb_body)
    } else if name.eq_ignore_ascii_case("lab")
        || name.eq_ignore_ascii_case("lch")
        || name.eq_ignore_ascii_case("oklab")
        || name.eq_ignore_ascii_case("oklch")
        || name.eq_ignore_ascii_case("color")
    {
        let arguments = input
            .parse_nested_block(|inner| {
                Ok::<String, cssparser::ParseError<'_, ()>>(
                    crate::css::tokens::serialize_remaining(inner),
                )
            })
            .map_err(|_| ())?;
        let text = format!("{}({})", name.to_ascii_lowercase(), arguments);
        Color::from_css(&text)
            .map(|color| ColorValue::Absolute(Rgba::from_color32(color)))
            .ok_or(())
    } else {
        Err(())
    }
}

/// Run `body` inside a function's block, translating its `()` error.
fn nested<T>(
    input: &mut Parser<'_, '_>,
    body: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    input
        .parse_nested_block(|inner| match body(inner) {
            Ok(value) => Ok(value),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn require_exhausted(input: &mut Parser<'_, '_>) -> Result<(), ()> {
    input.skip_whitespace();
    if input.is_exhausted() { Ok(()) } else { Err(()) }
}

fn parse_rgb_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let r = parse_channel(input)?;
    eat_comma(input);
    let g = parse_channel(input)?;
    eat_comma(input);
    let b = parse_channel(input)?;
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    Ok(ColorValue::Absolute(
        Rgba {
            r: rgb_channel(r),
            g: rgb_channel(g),
            b: rgb_channel(b),
            a: alpha.map_or(1.0, unit_channel),
        }
        .clamped(),
    ))
}

fn parse_hsl_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let hue = parse_angle_or_number(input)?;
    eat_comma(input);
    let saturation = parse_channel(input)?;
    eat_comma(input);
    let lightness = parse_channel(input)?;
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    let (r, g, b) = hsl_to_rgb(
        hue.rem_euclid(360.0),
        unit_channel(saturation).clamp(0.0, 1.0),
        unit_channel(lightness).clamp(0.0, 1.0),
    );
    Ok(ColorValue::Absolute(
        Rgba { r, g, b, a: alpha.map_or(1.0, unit_channel) }.clamped(),
    ))
}

fn parse_hwb_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let hue = parse_angle_or_number(input)?;
    eat_comma(input);
    let whiteness = unit_channel(parse_channel(input)?).clamp(0.0, 1.0);
    eat_comma(input);
    let blackness = unit_channel(parse_channel(input)?).clamp(0.0, 1.0);
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    let a = alpha.map_or(1.0, unit_channel);
    if whiteness + blackness >= 1.0 {
        let grey = whiteness / (whiteness + blackness);
        return Ok(ColorValue::Absolute(
            Rgba { r: grey, g: grey, b: grey, a }.clamped(),
        ));
    }
    let (r, g, b) = hsl_to_rgb(hue.rem_euclid(360.0), 1.0, 0.5);
    let apply = |channel: f32| channel * (1.0 - whiteness - blackness) + whiteness;
    Ok(ColorValue::Absolute(
        Rgba { r: apply(r), g: apply(g), b: apply(b), a }.clamped(),
    ))
}

/// A hue: any angle unit, or a bare number in degrees.
fn parse_angle_or_number(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let state = input.state();
    if let Ok(angle) = parse_angle(input) {
        return Ok(angle);
    }
    input.reset(&state);
    match input.next().map_err(|_| ())?.clone() {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        _ => Err(()),
    }
}
```

Append the test module from Step 1.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::color`
Expected: PASS — 11 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/color.rs
git commit -m "feat(ui/css): lazily-resolved colour values

ColorValue keeps @name, currentColor and every colour expression
unresolved until a ColorCtx exists, so one parsed sheet resolves against
several tables. Hex parsing is token-validated (the M1 non-ASCII panic
regression battery moves across verbatim) and byte conversion rounds to
nearest, which the offscreen pixel gate depends on.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 5: `color-mix()`, relative colours, GTK's legacy expressions, and the colour table

This is the 29 → 37 move: M1 recorded 8 of Adwaita's 37 `@define-color`s as
unresolved because it modelled neither relative colour syntax nor GTK's legacy
colour functions. The contract's gate consequence — **all 37 resolve** — lands here.

**Files:**
- Modify: `ui/src/css/value/color.rs` (extend `parse_color_function`, add
  `try_parse_relative`, `parse_channel_expr`, `parse_color_mix_body`,
  the legacy bodies, and `build_color_table`)
- Test: `ui/src/css/value/color.rs` (append to the existing `mod tests`)

**Interfaces:**
- Consumes: everything from Task 4; `css::parse::parse_stylesheet` (M1, unchanged).
- Produces:
  - `css::value::color::build_color_table(&[(String, String)]) -> ColorTable`
  - `ColorValue::parse` now also accepts `color-mix()`, `rgb(from …)`,
    `hsl(from …)`, `alpha()`, `shade()`, `mix()`, `lighter()`, `darker()`

- [ ] **Step 1: Write the failing test**

Append inside `ui/src/css/value/color.rs`'s existing `mod tests`:

```rust
    use super::build_color_table;
    use crate::css::parse::parse_stylesheet;

    fn adwaita_table() -> ColorTable {
        build_color_table(&parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT).color_definitions)
    }

    fn resolved(table: &ColorTable, name: &str) -> Option<Color> {
        let ctx = ColorCtx { table, current: Rgba::TRANSPARENT, depth: 0 };
        table.get(name)?.resolve(&ctx).map(Rgba::to_color32)
    }

    #[test]
    fn resolves_adwaita_named_colors() {
        let table = adwaita_table();
        assert_eq!(resolved(&table, "theme_fg_color"), Some(Color(0xFF2E_3436)));
        assert_eq!(resolved(&table, "borders"), Some(Color(0xFFCD_C7C2)));
        assert_eq!(resolved(&table, "accent_color"), Some(Color(0xFF35_84E4)));
        assert_eq!(resolved(&table, "theme_text_color"), Some(Color(0xFF00_0000)));
        // rgba(255, 255, 255, 0.8) -> alpha 204 (0.8 * 255, rounded).
        assert_eq!(resolved(&table, "wm_highlight"), Some(Color(0xCCFF_FFFF)));
    }

    #[test]
    fn relative_color_syntax_resolves() {
        // M1 recorded these as unresolved and pinned the table at 29 of 37.
        // Modelling CSS Color 5 relative syntax is an M2 deliverable: the
        // fingerprint moves to 37, and this test inverts.
        let table = adwaita_table();
        assert_eq!(table.len(), 37);
        // `rgb(from black r g b / calc(alpha * 0.35))`
        // -> black at 35% alpha; 0.35 * 255 rounds to 89 (0x59).
        assert_eq!(resolved(&table, "wm_shadow"), Some(Color(0x5900_0000)));
        // `rgb(from black r g b / calc(alpha * 0.18))` -> 0.18 * 255 -> 46.
        assert_eq!(resolved(&table, "wm_border"), Some(Color(0x2E00_0000)));
        // The five `hsl(from ... h calc(s * k) calc(l * k))` entries all
        // resolve to opaque colours.
        for name in [
            "wm_title",
            "wm_bg_a",
            "wm_button_hover_color_a",
            "wm_button_active_color_a",
            "wm_button_active_color_b",
            "wm_button_active_color_c",
        ] {
            let color = resolved(&table, name).unwrap_or_else(|| panic!("{name} did not resolve"));
            assert_eq!(color.alpha(), 0xFF, "{name} lost its alpha");
        }
    }

    #[test]
    fn at_name_references_resolve_through_the_table() {
        let table = build_color_table(&[
            ("borders".to_string(), "#cdc7c2".to_string()),
            ("edge".to_string(), "@borders".to_string()),
        ]);
        assert_eq!(resolved(&table, "edge"), Some(Color(0xFFCD_C7C2)));
        assert_eq!(color_value("@edge", &table), Some(Color(0xFFCD_C7C2)));
        assert_eq!(color_value("@nope", &table), None);
    }

    #[test]
    fn a_definition_may_reference_a_name_defined_later() {
        // M1's table was built incrementally, so a forward reference was
        // silently dropped. Lazy resolution makes source order irrelevant.
        let table = build_color_table(&[
            ("edge".to_string(), "@borders".to_string()),
            ("borders".to_string(), "#cdc7c2".to_string()),
        ]);
        assert_eq!(resolved(&table, "edge"), Some(Color(0xFFCD_C7C2)));
    }

    #[test]
    fn gtk_legacy_colour_functions_resolve() {
        let table = ColorTable::new();
        // alpha() multiplies alpha: 0.5 * 255 -> 128 (0x80).
        assert_eq!(color_value("alpha(#ff0000, 0.5)", &table), Some(Color(0x80FF_0000)));
        // shade(c, 0) drives lightness (and saturation) to zero: black.
        assert_eq!(color_value("shade(#ff0000, 0)", &table), Some(Color(0xFF00_0000)));
        // shade(c, 1) is the identity.
        assert_eq!(color_value("shade(#ff0000, 1)", &table), Some(Color(0xFFFF_0000)));
        // mix(a, b, 0) is a, mix(a, b, 1) is b, and 0.5 is the midpoint.
        assert_eq!(color_value("mix(#000000, #ffffff, 0)", &table), Some(Color(0xFF00_0000)));
        assert_eq!(color_value("mix(#000000, #ffffff, 1)", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            color_value("mix(#000000, #ffffff, 0.5)", &table),
            Some(Color(0xFF80_8080))
        );
        // lighter/darker are shade(c, 1.3) / shade(c, 0.7); nesting works.
        assert!(color_value("lighter(#808080)", &table).is_some());
        assert_eq!(
            color_value("darker(#808080)", &table),
            color_value("shade(#808080, 0.7)", &table)
        );
        assert!(color_value("alpha(shade(@nope, 0.5), 0.5)", &table).is_none());
    }

    #[test]
    fn color_mix_normalizes_its_weights() {
        let table = ColorTable::new();
        assert_eq!(
            color_value("color-mix(in srgb, #000000, #ffffff)", &table),
            Some(Color(0xFF80_8080))
        );
        assert_eq!(
            color_value("color-mix(in srgb, #000000 100%, #ffffff)", &table),
            Some(Color(0xFF00_0000))
        );
        assert_eq!(
            color_value("color-mix(in srgb, #000000 0%, #ffffff)", &table),
            Some(Color(0xFFFF_FFFF))
        );
        // Both weights given and not summing to 100% still normalizes.
        assert_eq!(
            color_value("color-mix(in srgb, #000000 25%, #ffffff 25%)", &table),
            Some(Color(0xFF80_8080))
        );
        assert_eq!(color_value("color-mix(in srgb, #000000 0%, #ffffff 0%)", &table), None);
        assert_eq!(color_value("color-mix(#000000, #ffffff)", &table), None);
    }

    #[test]
    fn relative_syntax_survives_parse_unresolved() {
        // A relative colour over an @name must not be resolved at parse time.
        let value = parse_entirely_with("rgb(from @accent r g b / 0.5)", ColorValue::parse)
            .expect("relative syntax parses");
        assert!(matches!(value, ColorValue::Relative { .. }));
        let empty = ColorTable::new();
        let ctx = ColorCtx { table: &empty, current: Rgba::TRANSPARENT, depth: 0 };
        assert_eq!(value.resolve(&ctx), None);
        let table = table_with("accent", "#3584e4");
        let ctx = ColorCtx { table: &table, current: Rgba::TRANSPARENT, depth: 0 };
        assert_eq!(value.resolve(&ctx).map(Rgba::to_color32), Some(Color(0x8035_84E4)));
    }

    #[test]
    fn the_colour_table_never_panics_on_hostile_definitions() {
        let definitions: Vec<(String, String)> = crate::css::value::FUZZ_INPUTS
            .iter()
            .map(|input| ((*input).to_string(), (*input).to_string()))
            .collect();
        let table = build_color_table(&definitions);
        let ctx = ColorCtx { table: &table, current: Rgba::TRANSPARENT, depth: 0 };
        for value in table.values() {
            let _ = value.resolve(&ctx);
        }
    }
```

Mutation check: `relative_color_syntax_resolves` is the gate — reverting `wm_shadow`
to unresolved drops `table.len()` to 29 and fails on the first assertion.
`a_definition_may_reference_a_name_defined_later` fails the moment the table is
built incrementally again. `color_mix_normalizes_its_weights` fails if the
weight-normalization divides by the wrong sum.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::color`
Expected: FAIL — `error[E0425]: cannot find function 'build_color_table' in module 'super'`.

- [ ] **Step 3: Write minimal implementation**

In `ui/src/css/value/color.rs`, add `ParserInput` to the cssparser import
(`use cssparser::{Parser, ParserInput, Token};`), extend `parse_color_function`'s
`else if` chain immediately before its final `else { Err(()) }`:

```rust
    } else if name.eq_ignore_ascii_case("color-mix") {
        nested(input, parse_color_mix_body)
    } else if name.eq_ignore_ascii_case("alpha") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Alpha(color, factor))))
        })
    } else if name.eq_ignore_ascii_case("shade") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Shade(color, factor))))
        })
    } else if name.eq_ignore_ascii_case("mix") {
        nested(input, |inner| {
            let a = ColorValue::parse(inner)?;
            eat_comma(inner);
            let b = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Mix(a, b, factor))))
        })
    } else if name.eq_ignore_ascii_case("lighter") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Lighter(color))))
        })
    } else if name.eq_ignore_ascii_case("darker") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Darker(color))))
        })
    } else {
```

Make `parse_rgb_body` and `parse_hsl_body` try the relative form first — insert as
the first statement of each:

```rust
    if let Some(relative) = try_parse_relative(input, ColorSpace::Srgb)? {
        return Ok(relative);
    }
```

(and `ColorSpace::Hsl` in `parse_hsl_body`).

Append to the file:

```rust
/// `<number>` / `<percentage>` / math function, as a bare number.
fn parse_number(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => Ok(unit_value),
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)?.resolve_number().ok_or(())
        }
        _ => Err(()),
    }
}

/// The channel names each space binds. Only sRGB and HSL are modelled
/// channel-wise; a relative colour in any other space is rejected at parse
/// time rather than silently mis-bound.
fn channel_index(name: &str, space: ColorSpace) -> Option<u8> {
    let names: [&str; 3] = match space {
        ColorSpace::Srgb | ColorSpace::SrgbLinear => ["r", "g", "b"],
        ColorSpace::Hsl => ["h", "s", "l"],
        _ => return None,
    };
    if name.eq_ignore_ascii_case("alpha") {
        return Some(3);
    }
    names
        .iter()
        .position(|channel| name.eq_ignore_ascii_case(channel))
        .and_then(|index| u8::try_from(index).ok())
}

fn parse_channel_expr(
    input: &mut Parser<'_, '_>,
    space: ColorSpace,
    index: usize,
) -> Result<ChannelExpr, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(ChannelExpr::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(ChannelExpr::Percent(unit_value))
        }
        Token::Dimension { .. } if space == ColorSpace::Hsl && index == 0 => {
            input.reset(&state);
            parse_angle(input).map(ChannelExpr::Number)
        }
        Token::Ident(ref name) => channel_index(name, space).map(ChannelExpr::Keep).ok_or(()),
        Token::Function(ref name) => {
            let name = name.clone();
            Ok(ChannelExpr::Calc(Rc::new(parse_math_function(&name, input)?)))
        }
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// `from <origin> c0 c1 c2 [/ alpha]` inside `rgb()`/`hsl()`.
/// `Ok(None)` means "this is not a relative colour"; the caller falls
/// through to the ordinary grammar with the parser rewound.
fn try_parse_relative(
    input: &mut Parser<'_, '_>,
    space: ColorSpace,
) -> Result<Option<ColorValue>, ()> {
    let state = input.state();
    let is_from = matches!(input.next(), Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("from"));
    if !is_from {
        input.reset(&state);
        return Ok(None);
    }
    let origin = ColorValue::parse(input)?;
    let channels = [
        parse_channel_expr(input, space, 0)?,
        parse_channel_expr(input, space, 1)?,
        parse_channel_expr(input, space, 2)?,
    ];
    let alpha_state = input.state();
    let has_alpha = matches!(input.next(), Ok(Token::Delim('/')) | Ok(Token::Comma));
    let alpha = if has_alpha {
        Some(parse_channel_expr(input, space, 3)?)
    } else {
        input.reset(&alpha_state);
        None
    };
    require_exhausted(input)?;
    Ok(Some(ColorValue::Relative {
        space,
        origin: Rc::new(origin),
        channels,
        alpha,
    }))
}

fn parse_color_mix_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let in_keyword = matches!(input.next(), Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("in"));
    if !in_keyword {
        return Err(());
    }
    let space_name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    let space = ColorSpace::from_str_ascii_ci(&space_name).ok_or(())?;
    // Skip the optional hue-interpolation clause (`shorter hue`, ...) up to
    // the comma that ends the space specifier.
    loop {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Comma => break,
            Token::Ident(_) => continue,
            _ => {
                input.reset(&state);
                return Err(());
            }
        }
    }
    let (a, wa) = parse_mix_operand(input)?;
    input.expect_comma().map_err(|_| ())?;
    let (b, wb) = parse_mix_operand(input)?;
    require_exhausted(input)?;
    Ok(ColorValue::Mix {
        space,
        a: Rc::new(a),
        wa,
        b: Rc::new(b),
        wb,
    })
}

/// `<color> <percentage>?` or `<percentage> <color>`.
fn parse_mix_operand(input: &mut Parser<'_, '_>) -> Result<(ColorValue, Option<f32>), ()> {
    let state = input.state();
    let leading = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if leading.is_none() {
        input.reset(&state);
    }
    let color = ColorValue::parse(input)?;
    if leading.is_some() {
        return Ok((color, leading));
    }
    let state = input.state();
    let trailing = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if trailing.is_none() {
        input.reset(&state);
    }
    Ok((color, trailing))
}

/// Build the `@define-color` table.
///
/// Order-preserving and **lazy**: every definition is parsed but nothing is
/// resolved, so a definition may reference a name defined later in the
/// sheet (M1 required earlier-only). Cycles are broken by
/// [`ColorCtx::MAX_DEPTH`] at resolution time. A later definition of the
/// same name replaces an earlier one, as GTK does.
#[must_use]
pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable {
    let mut table = ColorTable::with_capacity(definitions.len());
    for (name, text) in definitions {
        let mut source = ParserInput::new(text);
        let mut parser = Parser::new(&mut source);
        let parsed = ColorValue::parse(&mut parser).and_then(|value| {
            parser.skip_whitespace();
            if parser.is_exhausted() { Ok(value) } else { Err(()) }
        });
        match parsed {
            Ok(value) => {
                table.insert(name.clone(), value);
            }
            Err(()) => {
                tracing::debug!(%name, value = %text, "unparseable @define-color; skipping");
            }
        }
    }
    table
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::color`
Expected: PASS — 19 tests, including `relative_color_syntax_resolves` at
`table.len() == 37`.
Run: `cargo test -p icedtea-ui` — Expected: PASS (M1's `css::colors` tests still
pin **29**; they are the old module and Part 3 deletes them).
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/color.rs
git commit -m "feat(ui/css): color-mix, relative colour syntax and GTK legacy colours

All 37 of Adwaita's @define-colors now resolve (M1 pinned 29): the eight
missing ones were CSS Color 5 relative syntax. The table is lazy, so a
definition may reference a name defined later, and cycles are broken by a
depth cap rather than a visited set.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 6: Images — gradients, `url()`, `image()`, `cross-fade()`, `-gtk-*`

**Files:**
- Create: `ui/src/css/value/image.rs`
- Modify: `ui/src/css/value/mod.rs` (declare + re-export `image`)
- Test: `ui/src/css/value/image.rs`

**Interfaces:**
- Consumes: `Length`, `LengthCtx`, `calc::parse_angle`, `ColorValue`, `Keyword`.
- Produces: `css::value::image::{Image, Gradient, GradientKind, LinearDirection,
  SideOrCorner, RadialShape, RadialExtent, ColorStop, Position, IconRef}`,
  `Image::parse(&mut Parser<'_, '_>) -> Result<Image, ()>`,
  `Position::center() -> Position`,
  `Position::parse(&mut Parser<'_, '_>) -> Result<Position, ()>`.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/image.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Gradient, GradientKind, Image, LinearDirection, Position, RadialExtent, SideOrCorner};
    use crate::css::value::color::ColorValue;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    fn image(text: &str) -> Option<Image> {
        parse_entirely_with(text, Image::parse).ok()
    }

    fn gradient(text: &str) -> Option<Gradient> {
        match image(text)? {
            Image::Gradient(gradient) => Some((*gradient).clone()),
            _ => None,
        }
    }

    #[test]
    fn adwaitas_button_gradient_parses_with_its_stop_positions() {
        // The exact declaration the offscreen pixel gate pins.
        let gradient = gradient("linear-gradient(to top, #f6f5f4 2px, #fbfafa)")
            .expect("Adwaita's button gradient parses");
        assert!(!gradient.repeating);
        assert_eq!(
            gradient.kind,
            GradientKind::Linear { direction: LinearDirection::Side(SideOrCorner::Top) }
        );
        assert_eq!(gradient.stops.len(), 2);
        assert_eq!(gradient.stops[0].position, Some(Length::px(2.0)));
        assert_eq!(gradient.stops[1].position, None);
    }

    #[test]
    fn every_gradient_kind_and_the_repeating_forms_parse() {
        assert!(gradient("linear-gradient(45deg, red, blue)").is_some());
        assert!(gradient("linear-gradient(to bottom right, red, blue)").is_some());
        assert!(gradient("repeating-linear-gradient(red, blue)").is_some());
        assert!(gradient("radial-gradient(circle closest-side at 10px 20px, red, blue)").is_some());
        assert!(gradient("radial-gradient(farthest-corner, red 0%, rgba(53,132,228,0) 100%)").is_some());
        assert!(gradient("repeating-radial-gradient(red, blue)").is_some());
        assert!(gradient("conic-gradient(from 90deg at center, red, blue)").is_some());
        assert!(gradient("repeating-conic-gradient(red, blue)").is_some());
        // A gradient needs at least two stops.
        assert!(gradient("linear-gradient(red)").is_none());
    }

    #[test]
    fn a_default_linear_gradient_points_down_and_a_radial_one_is_centred() {
        let linear = gradient("linear-gradient(red, blue)").expect("parses");
        assert_eq!(
            linear.kind,
            GradientKind::Linear { direction: LinearDirection::Side(SideOrCorner::Bottom) }
        );
        let radial = gradient("radial-gradient(red, blue)").expect("parses");
        match radial.kind {
            GradientKind::Radial { extent, position, .. } => {
                assert_eq!(extent, RadialExtent::FarthestCorner);
                assert_eq!(position, Position::center());
            }
            other => panic!("expected a radial gradient, got {other:?}"),
        }
    }

    #[test]
    fn interpolation_hints_attach_to_the_stop_that_follows_them() {
        let gradient = gradient("linear-gradient(red, 30%, blue)").expect("parses");
        assert_eq!(gradient.stops.len(), 2);
        assert_eq!(gradient.stops[1].hint, Some(Length::Percent(0.3)));
    }

    #[test]
    fn a_double_position_stop_expands_to_two_stops() {
        let gradient = gradient("linear-gradient(red 0% 50%, blue)").expect("parses");
        assert_eq!(gradient.stops.len(), 3);
        assert_eq!(gradient.stops[0].position, Some(Length::Percent(0.0)));
        assert_eq!(gradient.stops[1].position, Some(Length::Percent(0.5)));
    }

    #[test]
    fn the_non_gradient_image_forms_parse() {
        assert_eq!(image("none"), Some(Image::None));
        assert_eq!(image("url(\"icon.png\")"), Some(Image::Url("icon.png".into())));
        assert_eq!(
            image("image(#e8e6e3)"),
            Some(Image::Solid(ColorValue::parse(
                &mut cssparser::Parser::new(&mut cssparser::ParserInput::new("#e8e6e3"))
            )
            .expect("colour parses")))
        );
        assert!(matches!(image("cross-fade(20% url(\"a.png\"), url(\"b.png\"))"), Some(Image::CrossFade(_))));
        assert!(matches!(image("-gtk-icontheme(\"go-next\")"), Some(Image::Icon(_))));
        assert!(matches!(image("-gtk-recolor(url(\"a.svg\"))"), Some(Image::Icon(_))));
        assert!(matches!(image("-gtk-scaled(url(\"a.png\"), url(\"a@2.png\"))"), Some(Image::Icon(_))));
        // GTK requires quotes around url() arguments; an unquoted url token
        // is still accepted, because cssparser tokenizes it for free.
        assert_eq!(image("url(icon.png)"), Some(Image::Url("icon.png".into())));
    }

    #[test]
    fn positions_accept_keywords_lengths_and_the_three_value_form() {
        let parse = |text: &str| parse_entirely_with(text, Position::parse).ok();
        assert_eq!(parse("center"), Some(Position::center()));
        assert_eq!(
            parse("left top"),
            Some(Position { x: Length::Percent(0.0), y: Length::Percent(0.0), z: None })
        );
        assert_eq!(
            parse("10px 20px"),
            Some(Position { x: Length::px(10.0), y: Length::px(20.0), z: None })
        );
        assert_eq!(
            parse("50% 50% 3px"),
            Some(Position { x: Length::Percent(0.5), y: Length::Percent(0.5), z: Some(Length::px(3.0)) })
        );
        // A single value leaves the other axis centred.
        assert_eq!(
            parse("right"),
            Some(Position { x: Length::Percent(1.0), y: Length::Percent(0.5), z: None })
        );
    }

    #[test]
    fn image_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Image::parse);
            let _ = parse_entirely_with(input, Position::parse);
        }
    }
}
```

Mutation check: `adwaitas_button_gradient_parses_with_its_stop_positions` fails if
the `2px` stop position is dropped — the exact value the offscreen gate's
`y = height - 2` band is derived from.
`a_default_linear_gradient_points_down_and_a_radial_one_is_centred` fails if the
omitted-direction default becomes `Top` (the direction Adwaita writes explicitly).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::image`
Expected: FAIL — `error[E0433]: failed to resolve: could not find 'image' in 'value'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod image;` and
`pub use image::{Gradient, Image, IconRef, Position};` to `ui/src/css/value/mod.rs`.

Create `ui/src/css/value/image.rs`:

```rust
//! `<image>`: gradients, `url()`, `image()`, `cross-fade()` and GTK's
//! `-gtk-icontheme()`/`-gtk-recolor()`/`-gtk-scaled()`.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::color::ColorValue;
use super::length::Length;

/// A `<position>`: `background-position` and `transform-origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    /// Horizontal offset from the origin box's left edge.
    pub x: Length,
    /// Vertical offset from its top edge.
    pub y: Length,
    /// Optional z component (`transform-origin` only).
    pub z: Option<Length>,
}

impl Position {
    /// `50% 50%`.
    #[must_use]
    pub fn center() -> Position {
        Position { x: Length::Percent(0.5), y: Length::Percent(0.5), z: None }
    }

    /// Parse `[<length-percentage> | left | center | right]
    /// [<length-percentage> | top | center | bottom]? <length>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Position, ()> {
        let first = parse_position_component(input)?;
        let state = input.state();
        let second = match parse_position_component(input) {
            Ok(component) => Some(component),
            Err(()) => {
                input.reset(&state);
                None
            }
        };
        let (x, y) = match (first, second) {
            (Component::Axis(Axis::Vertical, value), None) => (Length::Percent(0.5), value),
            (Component::Axis(_, value), None) | (Component::Free(value), None) => {
                (value, Length::Percent(0.5))
            }
            (a, Some(b)) => resolve_pair(a, b)?,
        };
        let state = input.state();
        let z = match Length::parse(input) {
            Ok(length) => Some(length),
            Err(()) => {
                input.reset(&state);
                None
            }
        };
        Ok(Position { x, y, z })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
enum Component {
    /// A keyword that pins an axis (`left`/`right`/`top`/`bottom`).
    Axis(Axis, Length),
    /// `center`, a length or a percentage: usable on either axis.
    Free(Length),
}

fn parse_position_component(input: &mut Parser<'_, '_>) -> Result<Component, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token {
        if name.eq_ignore_ascii_case("left") {
            return Ok(Component::Axis(Axis::Horizontal, Length::Percent(0.0)));
        }
        if name.eq_ignore_ascii_case("right") {
            return Ok(Component::Axis(Axis::Horizontal, Length::Percent(1.0)));
        }
        if name.eq_ignore_ascii_case("top") {
            return Ok(Component::Axis(Axis::Vertical, Length::Percent(0.0)));
        }
        if name.eq_ignore_ascii_case("bottom") {
            return Ok(Component::Axis(Axis::Vertical, Length::Percent(1.0)));
        }
        if name.eq_ignore_ascii_case("center") {
            return Ok(Component::Free(Length::Percent(0.5)));
        }
        return Err(());
    }
    input.reset(&state);
    Length::parse(input).map(Component::Free)
}

fn resolve_pair(a: Component, b: Component) -> Result<(Length, Length), ()> {
    match (a, b) {
        (Component::Axis(Axis::Vertical, y), Component::Axis(Axis::Horizontal, x))
        | (Component::Axis(Axis::Horizontal, x), Component::Axis(Axis::Vertical, y)) => Ok((x, y)),
        (Component::Axis(Axis::Horizontal, x), Component::Free(y))
        | (Component::Free(x), Component::Free(y)) => Ok((x, y)),
        (Component::Free(x), Component::Axis(Axis::Vertical, y)) => Ok((x, y)),
        (Component::Axis(Axis::Vertical, y), Component::Free(x)) => Ok((x, y)),
        _ => Err(()),
    }
}

/// A gradient's colour stop.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorStop {
    /// The stop colour.
    pub color: ColorValue,
    /// Explicit position along the gradient line.
    pub position: Option<Length>,
    /// Interpolation hint appearing *before* this stop.
    pub hint: Option<Length>,
}

/// Which side or corner a `to ...` linear gradient points at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideOrCorner {
    /// `to top`.
    Top,
    /// `to right`.
    Right,
    /// `to bottom`.
    Bottom,
    /// `to left`.
    Left,
    /// `to top left`.
    TopLeft,
    /// `to top right`.
    TopRight,
    /// `to bottom left`.
    BottomLeft,
    /// `to bottom right`.
    BottomRight,
}

/// A linear gradient's direction.
#[derive(Clone, Debug, PartialEq)]
pub enum LinearDirection {
    /// An explicit angle, in degrees, `0deg` pointing up.
    Angle(f32),
    /// A `to <side-or-corner>` keyword.
    Side(SideOrCorner),
}

/// A radial gradient's shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RadialShape {
    /// `circle`.
    Circle,
    /// `ellipse`.
    Ellipse,
}

/// A radial gradient's extent.
#[derive(Clone, Debug, PartialEq)]
pub enum RadialExtent {
    /// `closest-side`.
    ClosestSide,
    /// `closest-corner`.
    ClosestCorner,
    /// `farthest-side`.
    FarthestSide,
    /// `farthest-corner` (the default).
    FarthestCorner,
    /// Explicit radii.
    Explicit(Length, Length),
}

/// Which family of gradient this is.
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    /// `linear-gradient()`.
    Linear {
        /// Direction of the gradient line.
        direction: LinearDirection,
    },
    /// `radial-gradient()`.
    Radial {
        /// Circle or ellipse.
        shape: RadialShape,
        /// How far the gradient reaches.
        extent: RadialExtent,
        /// The centre.
        position: Position,
    },
    /// `conic-gradient()`.
    Conic {
        /// Starting angle in degrees.
        from_angle: f32,
        /// The centre.
        position: Position,
    },
}

/// A parsed gradient.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// Which gradient family.
    pub kind: GradientKind,
    /// `repeating-*` form.
    pub repeating: bool,
    /// Colour stops, in order, at least two.
    pub stops: Rc<[ColorStop]>,
    /// `in <space>` interpolation space, if given.
    pub interpolation: Option<super::color::ColorSpace>,
}

/// A GTK image function, stored for Part 4 of the milestone chain (icons
/// are drawn in M4, not M2).
#[derive(Clone, Debug, PartialEq)]
pub enum IconRef {
    /// `-gtk-icontheme(name)`.
    Theme {
        /// Icon name.
        name: Rc<str>,
    },
    /// `-gtk-recolor(url, palette?)`.
    Recolor {
        /// Source URL.
        url: Rc<str>,
        /// Optional palette override.
        palette: Option<Rc<[(Rc<str>, ColorValue)]>>,
    },
    /// `-gtk-scaled(lo, hi)`.
    Scaled {
        /// Normal-resolution image.
        lo: Rc<Image>,
        /// Hi-DPI image.
        hi: Rc<Image>,
    },
}

/// A parsed `<image>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// `none`.
    None,
    /// `url(...)` -- decoded lazily; a URL that fails to decode is
    /// recorded-unresolved and paints nothing, it is not an error.
    Url(Rc<str>),
    /// `image(<color>)`.
    Solid(ColorValue),
    /// Any gradient.
    Gradient(Rc<Gradient>),
    /// `cross-fade()`; `None` shares are auto-distributed.
    CrossFade(Rc<[(Option<f32>, Image)]>),
    /// A GTK image function.
    Icon(Rc<IconRef>),
}

impl Image {
    /// Parse a whole `<image>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Image, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("none") => Ok(Image::None),
            Token::UnquotedUrl(ref url) => Ok(Image::Url(Rc::from(url.as_ref()))),
            Token::Function(ref name) => {
                let name = name.clone();
                parse_image_function(&name, input)
            }
            _ => Err(()),
        }
    }
}

fn parse_image_function(name: &str, input: &mut Parser<'_, '_>) -> Result<Image, ()> {
    let lower = name.to_ascii_lowercase();
    let (repeating, family) = match lower.strip_prefix("repeating-") {
        Some(rest) => (true, rest.to_string()),
        None => (false, lower.clone()),
    };
    match family.as_str() {
        "url" => nested(input, |inner| {
            let url = inner.expect_string().map_err(|_| ())?.as_ref().to_string();
            require_exhausted(inner)?;
            Ok(Image::Url(Rc::from(url.as_str())))
        }),
        "image" => nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(Image::Solid(color))
        }),
        "linear-gradient" => nested_gradient(input, repeating, GradientFamily::Linear),
        "radial-gradient" => nested_gradient(input, repeating, GradientFamily::Radial),
        "conic-gradient" => nested_gradient(input, repeating, GradientFamily::Conic),
        "cross-fade" => nested(input, |inner| {
            let layers = inner
                .parse_comma_separated(|argument| {
                    match parse_cross_fade_layer(argument) {
                        Ok(layer) => Ok(layer),
                        Err(()) => Err(argument.new_custom_error::<(), ()>(())),
                    }
                })
                .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
            if layers.is_empty() {
                return Err(());
            }
            Ok(Image::CrossFade(layers.into()))
        }),
        "-gtk-icontheme" => nested(input, |inner| {
            let icon = inner.expect_ident_or_string().map_err(|_| ())?.as_ref().to_string();
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Theme { name: Rc::from(icon.as_str()) })))
        }),
        "-gtk-recolor" => nested(input, |inner| {
            let url = match Image::parse(inner)? {
                Image::Url(url) => url,
                _ => return Err(()),
            };
            let palette = if inner.expect_comma().is_ok() {
                Some(parse_icon_palette(inner)?)
            } else {
                None
            };
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Recolor { url, palette })))
        }),
        "-gtk-scaled" => nested(input, |inner| {
            let lo = Image::parse(inner)?;
            inner.expect_comma().map_err(|_| ())?;
            let hi = Image::parse(inner)?;
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Scaled { lo: Rc::new(lo), hi: Rc::new(hi) })))
        }),
        _ => Err(()),
    }
}

/// `name <color>` pairs, comma separated -- also `-gtk-icon-palette`'s value.
pub(crate) fn parse_icon_palette(
    input: &mut Parser<'_, '_>,
) -> Result<Rc<[(Rc<str>, ColorValue)]>, ()> {
    let entries = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let name = argument.expect_ident().map_err(|_| ())?.as_ref().to_string();
                let color = ColorValue::parse(argument)?;
                Ok::<(Rc<str>, ColorValue), ()>((Rc::from(name.as_str()), color))
            })();
            match parsed {
                Ok(entry) => Ok(entry),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(entries.into())
}

fn parse_cross_fade_layer(input: &mut Parser<'_, '_>) -> Result<(Option<f32>, Image), ()> {
    let state = input.state();
    let share = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if share.is_none() {
        input.reset(&state);
    }
    let image = Image::parse(input)?;
    Ok((share, image))
}

#[derive(Copy, Clone)]
enum GradientFamily {
    Linear,
    Radial,
    Conic,
}

fn nested_gradient(
    input: &mut Parser<'_, '_>,
    repeating: bool,
    family: GradientFamily,
) -> Result<Image, ()> {
    input
        .parse_nested_block(|inner| match parse_gradient_body(inner, repeating, family) {
            Ok(image) => Ok(image),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn parse_gradient_body(
    input: &mut Parser<'_, '_>,
    repeating: bool,
    family: GradientFamily,
) -> Result<Image, ()> {
    let kind = match family {
        GradientFamily::Linear => GradientKind::Linear { direction: parse_linear_direction(input)? },
        GradientFamily::Radial => parse_radial_prelude(input)?,
        GradientFamily::Conic => parse_conic_prelude(input)?,
    };
    let stops = parse_stops(input)?;
    if stops.len() < 2 {
        return Err(());
    }
    require_exhausted(input)?;
    Ok(Image::Gradient(Rc::new(Gradient {
        kind,
        repeating,
        stops: stops.into(),
        interpolation: None,
    })))
}

/// `to <side-or-corner>` / `<angle>` / nothing (defaults to `to bottom`).
/// A trailing comma is consumed when a prelude was present.
fn parse_linear_direction(input: &mut Parser<'_, '_>) -> Result<LinearDirection, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token {
        if name.eq_ignore_ascii_case("to") {
            let side = parse_side_or_corner(input)?;
            input.expect_comma().map_err(|_| ())?;
            return Ok(LinearDirection::Side(side));
        }
    }
    input.reset(&state);
    if let Ok(angle) = parse_angle(input) {
        input.expect_comma().map_err(|_| ())?;
        return Ok(LinearDirection::Angle(angle));
    }
    input.reset(&state);
    Ok(LinearDirection::Side(SideOrCorner::Bottom))
}

fn parse_side_or_corner(input: &mut Parser<'_, '_>) -> Result<SideOrCorner, ()> {
    let first = input.expect_ident().map_err(|_| ())?.as_ref().to_ascii_lowercase();
    let state = input.state();
    let second = match input.expect_ident() {
        Ok(name) => Some(name.as_ref().to_ascii_lowercase()),
        Err(_) => {
            input.reset(&state);
            None
        }
    };
    let mut sides = [first.as_str(), second.as_deref().unwrap_or("")];
    sides.sort_unstable();
    match (sides[0], sides[1]) {
        ("", "top") => Ok(SideOrCorner::Top),
        ("", "right") => Ok(SideOrCorner::Right),
        ("", "bottom") => Ok(SideOrCorner::Bottom),
        ("", "left") => Ok(SideOrCorner::Left),
        ("left", "top") => Ok(SideOrCorner::TopLeft),
        ("right", "top") => Ok(SideOrCorner::TopRight),
        ("bottom", "left") => Ok(SideOrCorner::BottomLeft),
        ("bottom", "right") => Ok(SideOrCorner::BottomRight),
        _ => Err(()),
    }
}

fn parse_radial_prelude(input: &mut Parser<'_, '_>) -> Result<GradientKind, ()> {
    let mut shape = RadialShape::Ellipse;
    let mut extent = RadialExtent::FarthestCorner;
    let mut position = Position::center();
    let mut saw_prelude = false;
    let mut radii: Vec<Length> = Vec::new();
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        match token {
            Token::Comma => {
                saw_prelude = true;
                break;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("circle") => {
                shape = RadialShape::Circle;
                saw_prelude = true;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("ellipse") => {
                shape = RadialShape::Ellipse;
                saw_prelude = true;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("at") => {
                position = Position::parse(input)?;
                saw_prelude = true;
            }
            Token::Ident(ref name) => {
                extent = match name.to_ascii_lowercase().as_str() {
                    "closest-side" => RadialExtent::ClosestSide,
                    "closest-corner" => RadialExtent::ClosestCorner,
                    "farthest-side" => RadialExtent::FarthestSide,
                    "farthest-corner" => RadialExtent::FarthestCorner,
                    _ => {
                        input.reset(&state);
                        break;
                    }
                };
                saw_prelude = true;
            }
            _ => {
                input.reset(&state);
                match Length::parse(input) {
                    Ok(length) => {
                        radii.push(length);
                        saw_prelude = true;
                    }
                    Err(()) => {
                        input.reset(&state);
                        break;
                    }
                }
            }
        }
    }
    if radii.len() == 1 {
        extent = RadialExtent::Explicit(radii[0].clone(), radii[0].clone());
    } else if radii.len() >= 2 {
        extent = RadialExtent::Explicit(radii[0].clone(), radii[1].clone());
    }
    let _ = saw_prelude;
    Ok(GradientKind::Radial { shape, extent, position })
}

fn parse_conic_prelude(input: &mut Parser<'_, '_>) -> Result<GradientKind, ()> {
    let mut from_angle = 0.0;
    let mut position = Position::center();
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        match token {
            Token::Comma => break,
            Token::Ident(ref name) if name.eq_ignore_ascii_case("from") => {
                from_angle = parse_angle(input)?;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("at") => {
                position = Position::parse(input)?;
            }
            _ => {
                input.reset(&state);
                break;
            }
        }
    }
    Ok(GradientKind::Conic { from_angle, position })
}

/// `<color-stop-list>`: stops, optional positions (one or two per stop) and
/// bare interpolation hints between them.
fn parse_stops(input: &mut Parser<'_, '_>) -> Result<Vec<ColorStop>, ()> {
    let mut stops: Vec<ColorStop> = Vec::new();
    let mut pending_hint: Option<Length> = None;
    loop {
        let state = input.state();
        // A bare length between two stops is an interpolation hint.
        if let Ok(hint) = Length::parse(input) {
            let comma = input.expect_comma().is_ok();
            if comma && !stops.is_empty() {
                pending_hint = Some(hint);
                continue;
            }
            input.reset(&state);
        } else {
            input.reset(&state);
        }
        let color = ColorValue::parse(input)?;
        let mut positions: Vec<Length> = Vec::new();
        for _ in 0..2 {
            let state = input.state();
            match Length::parse(input) {
                Ok(length) => positions.push(length),
                Err(()) => {
                    input.reset(&state);
                    break;
                }
            }
        }
        if positions.is_empty() {
            stops.push(ColorStop { color, position: None, hint: pending_hint.take() });
        } else {
            for position in positions {
                stops.push(ColorStop {
                    color: color.clone(),
                    position: Some(position),
                    hint: pending_hint.take(),
                });
            }
        }
        if input.expect_comma().is_err() {
            return Ok(stops);
        }
    }
}

fn nested<T>(
    input: &mut Parser<'_, '_>,
    body: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    input
        .parse_nested_block(|inner| match body(inner) {
            Ok(value) => Ok(value),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn require_exhausted(input: &mut Parser<'_, '_>) -> Result<(), ()> {
    input.skip_whitespace();
    if input.is_exhausted() { Ok(()) } else { Err(()) }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::image` — Expected: PASS, 7 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/image.rs
git commit -m "feat(ui/css): the complete <image> grammar

Linear, radial and conic gradients with any stops, angles, hints and the
repeating- forms; url(), image(<color>), cross-fade() and GTK's three
-gtk-* image functions, which are stored as IconRef and drawn in M4.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 7: The gradient sampler

Part 4's pixel tests derive their expected colours from exactly these two
functions, so they are the engine's gradient *model*, not a convenience.

**Files:**
- Modify: `ui/src/css/value/image.rs` (add `Gradient::color_at`, `Gradient::line_for_box`)
- Test: `ui/src/css/value/image.rs`

**Interfaces:**
- Consumes: `Gradient`, `ColorCtx`, `LengthCtx`, `Rgba`.
- Produces:
  - `Gradient::color_at(&self, t: f32, ctx: &ColorCtx<'_>, len_ctx: &LengthCtx) -> Rgba`
  - `Gradient::line_for_box(&self, w: f32, h: f32, ctx: &LengthCtx) -> (Point, Point)`
    (`skia_rs_safe::core::Point`)

- [ ] **Step 1: Write the failing test**

Append inside `ui/src/css/value/image.rs`'s `mod tests`:

```rust
    use crate::css::value::color::{ColorCtx, ColorTable, Rgba};
    use crate::css::value::length::LengthCtx;
    use skia_rs_safe::core::Color;

    fn sample(text: &str, t: f32, basis: f32) -> Color {
        let gradient = gradient(text).expect("gradient parses");
        let table = ColorTable::new();
        let color_ctx = ColorCtx { table: &table, current: Rgba::TRANSPARENT, depth: 0 };
        let len_ctx = LengthCtx { percent_basis: Some(basis), ..LengthCtx::default() };
        gradient.color_at(t, &color_ctx, &len_ctx).to_color32()
    }

    #[test]
    fn adwaitas_button_gradient_samples_exactly_at_and_beyond_its_stops() {
        // 34px tall button, `to top`: the gradient line runs bottom -> top,
        // so t is the distance from the bottom edge. `#f6f5f4 2px` means
        // everything at or below 2px from the bottom is the flat first stop.
        let text = "linear-gradient(to top, #f6f5f4 2px, #fbfafa)";
        assert_eq!(sample(text, 0.0, 34.0), Color(0xFFF6_F5F4));
        assert_eq!(sample(text, 2.0 / 34.0, 34.0), Color(0xFFF6_F5F4));
        assert_eq!(sample(text, 1.0, 34.0), Color(0xFFFB_FAFA));
        // Before the first stop and after the last one, the ends are flat.
        assert_eq!(sample(text, -1.0, 34.0), Color(0xFFF6_F5F4));
        assert_eq!(sample(text, 2.0, 34.0), Color(0xFFFB_FAFA));
    }

    #[test]
    fn a_two_stop_gradient_interpolates_linearly_at_its_midpoint() {
        assert_eq!(sample("linear-gradient(#000000, #ffffff)", 0.5, 100.0), Color(0xFF80_8080));
        assert_eq!(sample("linear-gradient(#000000, #ffffff)", 0.25, 100.0), Color(0xFF40_4040));
    }

    #[test]
    fn omitted_stop_positions_are_distributed_evenly() {
        let text = "linear-gradient(#000000, #ff0000, #ffffff)";
        assert_eq!(sample(text, 0.5, 100.0), Color(0xFFFF_0000));
    }

    #[test]
    fn a_repeating_gradient_wraps_its_parameter() {
        let text = "repeating-linear-gradient(#000000 0%, #ffffff 50%)";
        assert_eq!(sample(text, 0.25, 100.0), Color(0xFF80_8080));
        assert_eq!(sample(text, 0.75, 100.0), Color(0xFF80_8080));
    }

    #[test]
    fn the_gradient_line_matches_css_for_the_four_sides() {
        let table = ColorTable::new();
        let _ = table;
        let ctx = LengthCtx::default();
        let up = gradient("linear-gradient(to top, red, blue)").expect("parses");
        let (start, end) = up.line_for_box(100.0, 40.0, &ctx);
        assert_eq!((start.x, start.y), (50.0, 40.0));
        assert_eq!((end.x, end.y), (50.0, 0.0));
        let down = gradient("linear-gradient(red, blue)").expect("parses");
        let (start, end) = down.line_for_box(100.0, 40.0, &ctx);
        assert_eq!((start.x, start.y), (50.0, 0.0));
        assert_eq!((end.x, end.y), (50.0, 40.0));
        let right = gradient("linear-gradient(to right, red, blue)").expect("parses");
        let (start, end) = right.line_for_box(100.0, 40.0, &ctx);
        assert_eq!((start.x, start.y), (0.0, 20.0));
        assert_eq!((end.x, end.y), (100.0, 20.0));
    }
```

Mutation check: `adwaitas_button_gradient_samples_exactly_at_and_beyond_its_stops`
is the byte-for-byte model the offscreen pixel gate's `0xFFF6F5F4` band comes from;
changing the clamp at either end, or rounding down instead of to-nearest, fails it.
`the_gradient_line_matches_css_for_the_four_sides` fails if `to top` and the
default direction are swapped.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::image`
Expected: FAIL — `error[E0599]: no method named 'color_at' found for struct 'Gradient'`.

- [ ] **Step 3: Write minimal implementation**

Add `use skia_rs_safe::core::Point;` and `use super::color::{ColorCtx, Rgba};` to
`ui/src/css/value/image.rs`, then append:

```rust
impl Gradient {
    /// The colour at gradient-line parameter `t`, already normalised for
    /// the box (`0` at the line start, `1` at its end).
    ///
    /// This is the sampler Part 4's pixel tests derive expected colours
    /// from: interpolation is component-wise premultiplied sRGB and the
    /// result rounds to the nearest byte, so an assertion at a stop is
    /// exactly the theme's declared hex.
    #[must_use]
    pub fn color_at(&self, t: f32, ctx: &ColorCtx<'_>, len_ctx: &LengthCtx) -> Rgba {
        let positions = self.stop_positions(len_ctx);
        let colors: Vec<Rgba> = self
            .stops
            .iter()
            .map(|stop| stop.color.resolve(ctx).unwrap_or(Rgba::TRANSPARENT))
            .collect();
        if colors.is_empty() {
            return Rgba::TRANSPARENT;
        }
        if colors.len() == 1 {
            return colors[0];
        }
        let first = positions[0];
        let last = positions[positions.len() - 1];
        let span = last - first;
        let t = if self.repeating && span > 0.0 {
            first + (t - first).rem_euclid(span)
        } else {
            t
        };
        if t <= first {
            return colors[0];
        }
        if t >= last {
            return colors[colors.len() - 1];
        }
        for window in 0..positions.len() - 1 {
            let (a, b) = (positions[window], positions[window + 1]);
            if t >= a && t <= b {
                if (b - a).abs() <= f32::EPSILON {
                    return colors[window + 1];
                }
                let local = (t - a) / (b - a);
                let pa = colors[window].premultiplied();
                let pb = colors[window + 1].premultiplied();
                let mut out = [0.0_f32; 4];
                for channel in 0..4 {
                    out[channel] = pa[channel] + (pb[channel] - pa[channel]) * local;
                }
                return Rgba::from_premultiplied(out).clamped();
            }
        }
        colors[colors.len() - 1]
    }

    /// Every stop's position as a gradient-line fraction, with omitted
    /// positions distributed evenly between their fixed neighbours and the
    /// sequence made non-decreasing (CSS Images L3 §color-stop fixup).
    fn stop_positions(&self, len_ctx: &LengthCtx) -> Vec<f32> {
        let count = self.stops.len();
        let mut positions: Vec<Option<f32>> = self
            .stops
            .iter()
            .map(|stop| {
                stop.position.as_ref().and_then(|length| match length {
                    Length::Percent(fraction) => Some(*fraction),
                    other => {
                        let basis = len_ctx.percent_basis?;
                        (basis != 0.0).then(|| other.resolve(len_ctx))?.map(|px| px / basis)
                    }
                })
            })
            .collect();
        if count == 0 {
            return Vec::new();
        }
        if positions[0].is_none() {
            positions[0] = Some(0.0);
        }
        if positions[count - 1].is_none() {
            positions[count - 1] = Some(1.0);
        }
        let mut index = 0;
        while index < count {
            if positions[index].is_some() {
                index += 1;
                continue;
            }
            let start = index - 1;
            let mut end = index;
            while positions[end].is_none() {
                end += 1;
            }
            let from = positions[start].unwrap_or(0.0);
            let to = positions[end].unwrap_or(1.0);
            let steps = (end - start) as f32;
            for gap in start + 1..end {
                positions[gap] = Some(from + (to - from) * ((gap - start) as f32) / steps);
            }
            index = end;
        }
        let mut out: Vec<f32> = positions.iter().map(|p| p.unwrap_or(0.0)).collect();
        for index in 1..out.len() {
            if out[index] < out[index - 1] {
                out[index] = out[index - 1];
            }
        }
        out
    }

    /// The gradient line's endpoints for a `w` x `h` box, per CSS Images L3.
    /// The box's origin is its top-left; y grows downwards.
    #[must_use]
    pub fn line_for_box(&self, w: f32, h: f32, ctx: &LengthCtx) -> (Point, Point) {
        let centre = Point { x: w / 2.0, y: h / 2.0 };
        let angle = match &self.kind {
            GradientKind::Linear { direction } => match direction {
                LinearDirection::Angle(degrees) => *degrees,
                LinearDirection::Side(side) => side_angle(*side, w, h),
            },
            GradientKind::Radial { position, .. } | GradientKind::Conic { position, .. } => {
                let x = position.x.resolve(&with_basis(ctx, w)).unwrap_or(w / 2.0);
                let y = position.y.resolve(&with_basis(ctx, h)).unwrap_or(h / 2.0);
                let centre = Point { x, y };
                let radius = (w.max(h)) / 2.0;
                return (centre, Point { x: centre.x + radius, y: centre.y });
            }
        };
        // CSS angles run clockwise from "up".
        let radians = angle.to_radians();
        let (sin, cos) = radians.sin_cos();
        let length = (w * sin).abs() + (h * cos).abs();
        let half = length / 2.0;
        let start = Point { x: centre.x - sin * half, y: centre.y + cos * half };
        let end = Point { x: centre.x + sin * half, y: centre.y - cos * half };
        (start, end)
    }
}

fn with_basis(ctx: &LengthCtx, basis: f32) -> LengthCtx {
    LengthCtx { percent_basis: Some(basis), ..*ctx }
}

fn side_angle(side: SideOrCorner, w: f32, h: f32) -> f32 {
    match side {
        SideOrCorner::Top => 0.0,
        SideOrCorner::Right => 90.0,
        SideOrCorner::Bottom => 180.0,
        SideOrCorner::Left => 270.0,
        // Corner gradients aim the line so the corner's perpendicular
        // passes through it (CSS Images L3 §linear-gradient).
        SideOrCorner::TopRight => 90.0 - w.atan2(h).to_degrees() + w.atan2(h).to_degrees(),
        SideOrCorner::TopLeft => 360.0 - h.atan2(w).to_degrees(),
        SideOrCorner::BottomRight => 180.0 - h.atan2(w).to_degrees(),
        SideOrCorner::BottomLeft => 180.0 + h.atan2(w).to_degrees(),
    }
}
```

The `TopRight` arm above must be the plain corner angle, not the identity written
out; replace it with `SideOrCorner::TopRight => h.atan2(w).to_degrees(),`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::image` — Expected: PASS, 12 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/image.rs
git commit -m "feat(ui/css): the gradient sampler and gradient-line geometry

color_at is the engine's gradient model: Part 4's pixel assertions are
derived from it, so stop fixup, premultiplied interpolation and
round-to-nearest bytes are all load-bearing.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 8: Shadows and the border/background small types

**Files:**
- Create: `ui/src/css/value/shadow.rs`, `ui/src/css/value/border.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: both new files

**Interfaces:**
- Consumes: `Length`, `ColorValue`, `Keyword`.
- Produces:
  - `css::value::shadow::Shadow { color, offset_x, offset_y, blur, spread, inset }`,
    `Shadow::parse` (box-shadow form), `Shadow::parse_text` (no spread, no inset)
  - `css::value::border::{NumberOrPercent, BorderImageSlice, BorderImageWidthSide, RepeatStyle, BgSize}`
  - `BorderImageSlice::parse`, `BorderImageWidthSide::parse`, `RepeatStyle::parse`,
    `BgSize::parse`, `border::parse_line_width(&mut Parser<'_, '_>) -> Result<Length, ()>`,
    `border::parse_line_style(&mut Parser<'_, '_>) -> Result<Keyword, ()>`,
    `border::four_sides<T: Clone>(values: Vec<T>) -> Option<[T; 4]>` (TRBL)

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/shadow.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Shadow;
    use crate::css::value::color::ColorValue;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn a_box_shadow_reads_offsets_blur_spread_colour_and_inset_in_any_order() {
        let shadow = parse_entirely_with("inset 1px 2px 3px 4px #ff0000", Shadow::parse)
            .expect("shadow parses");
        assert!(shadow.inset);
        assert_eq!(shadow.offset_x, Length::px(1.0));
        assert_eq!(shadow.offset_y, Length::px(2.0));
        assert_eq!(shadow.blur, Length::px(3.0));
        assert_eq!(shadow.spread, Length::px(4.0));
        assert!(matches!(shadow.color, Some(ColorValue::Absolute(_))));

        let reordered = parse_entirely_with("#ff0000 1px 2px inset", Shadow::parse)
            .expect("shadow parses");
        assert!(reordered.inset);
        assert_eq!(reordered.blur, Length::zero());
        assert_eq!(reordered.spread, Length::zero());
    }

    #[test]
    fn an_omitted_colour_stays_none_so_it_can_become_current_colour_later() {
        let shadow = parse_entirely_with("0 1px 2px", Shadow::parse).expect("shadow parses");
        assert_eq!(shadow.color, None);
        assert_eq!(shadow.offset_x, Length::zero());
    }

    #[test]
    fn a_text_shadow_takes_no_spread_and_no_inset() {
        let shadow = parse_entirely_with("1px 2px 3px #000000", Shadow::parse_text)
            .expect("text-shadow parses");
        assert_eq!(shadow.spread, Length::zero());
        assert!(!shadow.inset);
        assert!(parse_entirely_with("1px 2px 3px 4px", Shadow::parse_text).is_err());
        assert!(parse_entirely_with("inset 1px 2px", Shadow::parse_text).is_err());
    }

    #[test]
    fn two_offsets_are_mandatory() {
        assert!(parse_entirely_with("1px", Shadow::parse).is_err());
        assert!(parse_entirely_with("red", Shadow::parse).is_err());
    }

    #[test]
    fn shadow_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Shadow::parse);
            let _ = parse_entirely_with(input, Shadow::parse_text);
        }
    }
}
```

Append to `ui/src/css/value/border.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle,
                four_sides, parse_line_style, parse_line_width};
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn line_width_keywords_resolve_to_pixels() {
        // M1's mapping, preserved: thin 1, medium 3, thick 5.
        assert_eq!(parse_entirely_with("thin", parse_line_width).ok(), Some(Length::px(1.0)));
        assert_eq!(parse_entirely_with("MEDIUM", parse_line_width).ok(), Some(Length::px(3.0)));
        assert_eq!(parse_entirely_with("thick", parse_line_width).ok(), Some(Length::px(5.0)));
        assert_eq!(parse_entirely_with("2px", parse_line_width).ok(), Some(Length::px(2.0)));
        assert_eq!(parse_entirely_with("red", parse_line_width).ok(), None);
    }

    #[test]
    fn every_border_style_keyword_parses_and_nothing_else_does() {
        for (text, expected) in [
            ("none", Keyword::None),
            ("hidden", Keyword::Hidden),
            ("dotted", Keyword::Dotted),
            ("dashed", Keyword::Dashed),
            ("SOLID", Keyword::Solid),
            ("double", Keyword::Double),
            ("groove", Keyword::Groove),
            ("ridge", Keyword::Ridge),
            ("inset", Keyword::Inset),
            ("outset", Keyword::Outset),
        ] {
            assert_eq!(parse_entirely_with(text, parse_line_style).ok(), Some(expected));
        }
        assert_eq!(parse_entirely_with("thin", parse_line_style).ok(), None);
    }

    #[test]
    fn the_four_sides_rule_matches_css() {
        assert_eq!(four_sides(vec![1]), Some([1, 1, 1, 1]));
        assert_eq!(four_sides(vec![1, 2]), Some([1, 2, 1, 2]));
        assert_eq!(four_sides(vec![1, 2, 3]), Some([1, 2, 3, 2]));
        assert_eq!(four_sides(vec![1, 2, 3, 4]), Some([1, 2, 3, 4]));
        assert_eq!(four_sides(vec![1, 2, 3, 4, 5]), None);
        assert_eq!(four_sides(Vec::<i32>::new()), None);
    }

    #[test]
    fn border_image_slice_width_and_repeat_parse() {
        let slice = parse_entirely_with("30% fill", BorderImageSlice::parse).expect("parses");
        assert!(slice.fill);
        assert_eq!(slice.sides, [NumberOrPercent::Percent(0.3); 4]);
        assert_eq!(
            parse_entirely_with("auto", BorderImageWidthSide::parse).ok(),
            Some(BorderImageWidthSide::Auto)
        );
        assert_eq!(
            parse_entirely_with("2", BorderImageWidthSide::parse).ok(),
            Some(BorderImageWidthSide::Number(2.0))
        );
        assert_eq!(
            parse_entirely_with("round space", RepeatStyle::parse).ok(),
            Some(RepeatStyle { x: Keyword::Round, y: Keyword::Space })
        );
        // One value applies to both axes.
        assert_eq!(
            parse_entirely_with("stretch", RepeatStyle::parse).ok(),
            Some(RepeatStyle { x: Keyword::Stretch, y: Keyword::Stretch })
        );
        // `repeat-x`/`repeat-y` are background-repeat only, never here.
        assert_eq!(parse_entirely_with("repeat-x", RepeatStyle::parse).ok(), None);
    }

    #[test]
    fn background_repeat_accepts_the_single_keyword_shorthands() {
        assert_eq!(
            parse_entirely_with("repeat-x", RepeatStyle::parse_background).ok(),
            Some(RepeatStyle { x: Keyword::Repeat, y: Keyword::NoRepeat })
        );
        assert_eq!(
            parse_entirely_with("repeat-y", RepeatStyle::parse_background).ok(),
            Some(RepeatStyle { x: Keyword::NoRepeat, y: Keyword::Repeat })
        );
    }

    #[test]
    fn background_size_parses_its_three_forms() {
        assert_eq!(parse_entirely_with("cover", BgSize::parse).ok(), Some(BgSize::Cover));
        assert_eq!(parse_entirely_with("contain", BgSize::parse).ok(), Some(BgSize::Contain));
        assert_eq!(parse_entirely_with("auto", BgSize::parse).ok(), Some(BgSize::Auto));
        assert_eq!(
            parse_entirely_with("10px 50%", BgSize::parse).ok(),
            Some(BgSize::Explicit(Length::px(10.0), Length::Percent(0.5)))
        );
    }

    #[test]
    fn border_value_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_line_width);
            let _ = parse_entirely_with(input, parse_line_style);
            let _ = parse_entirely_with(input, BorderImageSlice::parse);
            let _ = parse_entirely_with(input, BorderImageWidthSide::parse);
            let _ = parse_entirely_with(input, RepeatStyle::parse);
            let _ = parse_entirely_with(input, RepeatStyle::parse_background);
            let _ = parse_entirely_with(input, BgSize::parse);
        }
    }
}
```

Mutation check: `line_width_keywords_resolve_to_pixels` fails if `medium` stops
being 3px — the contract's pinned initial value for `border-*-width`.
`the_four_sides_rule_matches_css` fails on the 3-value case if left stops mirroring
right, which is the M1 `padding: 4px 9px` behaviour the offscreen gate depends on.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value`
Expected: FAIL — `error[E0583]: file not found for module 'shadow'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod border;` / `pub mod shadow;` and
`pub use border::{BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle};`
/ `pub use shadow::Shadow;` to `ui/src/css/value/mod.rs`.

Create `ui/src/css/value/shadow.rs`:

```rust
//! `box-shadow`, `text-shadow` and `-gtk-icon-shadow` values.

use cssparser::Parser;

use super::color::ColorValue;
use super::length::Length;

/// One shadow. `text-shadow` and `-gtk-icon-shadow` always have
/// `spread == 0` and `inset == false`.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    /// `None` == `currentColor` at used-value time.
    pub color: Option<ColorValue>,
    /// Horizontal offset.
    pub offset_x: Length,
    /// Vertical offset.
    pub offset_y: Length,
    /// Blur radius.
    pub blur: Length,
    /// Spread radius.
    pub spread: Length,
    /// Drawn inside the border edge.
    pub inset: bool,
}

impl Shadow {
    /// `<shadow> = inset? && <length>{2,4} && <color>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Shadow, ()> {
        parse_shadow(input, true)
    }

    /// `text-shadow`'s form: `<color>? && <length>{2,3}`, no `inset`.
    pub fn parse_text(input: &mut Parser<'_, '_>) -> Result<Shadow, ()> {
        parse_shadow(input, false)
    }
}

fn parse_shadow(input: &mut Parser<'_, '_>, box_form: bool) -> Result<Shadow, ()> {
    let mut lengths: Vec<Length> = Vec::new();
    let mut color: Option<ColorValue> = None;
    let mut inset = false;
    loop {
        let state = input.state();
        if box_form && !inset {
            let is_inset = matches!(
                input.next(),
                Ok(cssparser::Token::Ident(name)) if name.eq_ignore_ascii_case("inset")
            );
            if is_inset {
                inset = true;
                continue;
            }
            input.reset(&state);
        }
        if lengths.len() < if box_form { 4 } else { 3 } {
            if let Ok(length) = Length::parse(input) {
                lengths.push(length);
                continue;
            }
            input.reset(&state);
        }
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if lengths.len() < 2 {
        return Err(());
    }
    Ok(Shadow {
        color,
        offset_x: lengths[0].clone(),
        offset_y: lengths[1].clone(),
        blur: lengths.get(2).cloned().unwrap_or_else(Length::zero),
        spread: lengths.get(3).cloned().unwrap_or_else(Length::zero),
        inset,
    })
}
```

Create `ui/src/css/value/border.rs`:

```rust
//! Small value types shared by the border, border-image and background
//! property families.

use cssparser::{Parser, Token};

use super::keyword::Keyword;
use super::length::Length;

/// `<number>` or `<percentage>`, as `border-image-slice` uses them.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NumberOrPercent {
    /// A bare number.
    Number(f32),
    /// A percentage, as a fraction.
    Percent(f32),
}

/// `border-image-slice`.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderImageSlice {
    /// Top, right, bottom, left.
    pub sides: [NumberOrPercent; 4],
    /// Whether the middle region is painted.
    pub fill: bool,
}

/// One side of `border-image-width`.
#[derive(Clone, Debug, PartialEq)]
pub enum BorderImageWidthSide {
    /// A `<length-percentage>`.
    Length(Length),
    /// A multiple of the corresponding `border-*-width`.
    Number(f32),
    /// Use the slice's intrinsic size.
    Auto,
}

/// `background-repeat` / `border-image-repeat`, per axis.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RepeatStyle {
    /// Horizontal behaviour.
    pub x: Keyword,
    /// Vertical behaviour.
    pub y: Keyword,
}

/// `background-size`.
#[derive(Clone, Debug, PartialEq)]
pub enum BgSize {
    /// `auto auto`.
    Auto,
    /// `cover`.
    Cover,
    /// `contain`.
    Contain,
    /// Explicit width and height (`auto` on an axis becomes `Length::Auto`).
    Explicit(Length, Length),
}

/// CSS's 1-to-4-value box rule, expanded to `[top, right, bottom, left]`.
#[must_use]
pub fn four_sides<T: Clone>(values: Vec<T>) -> Option<[T; 4]> {
    match values.len() {
        1 => Some([
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
        ]),
        2 => Some([
            values[0].clone(),
            values[1].clone(),
            values[0].clone(),
            values[1].clone(),
        ]),
        3 => Some([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[1].clone(),
        ]),
        4 => Some([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
        ]),
        _ => None,
    }
}

/// `<line-width> = thin | medium | thick | <length>`, always in pixels for
/// the keyword forms (M1's mapping: 1, 3, 5).
pub fn parse_line_width(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token {
        if name.eq_ignore_ascii_case("thin") {
            return Ok(Length::px(1.0));
        }
        if name.eq_ignore_ascii_case("medium") {
            return Ok(Length::px(3.0));
        }
        if name.eq_ignore_ascii_case("thick") {
            return Ok(Length::px(5.0));
        }
        return Err(());
    }
    input.reset(&state);
    Length::parse(input)
}

/// `<line-style>`.
pub fn parse_line_style(input: &mut Parser<'_, '_>) -> Result<Keyword, ()> {
    const STYLES: &[Keyword] = &[
        Keyword::None,
        Keyword::Hidden,
        Keyword::Dotted,
        Keyword::Dashed,
        Keyword::Solid,
        Keyword::Double,
        Keyword::Groove,
        Keyword::Ridge,
        Keyword::Inset,
        Keyword::Outset,
    ];
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    STYLES
        .iter()
        .copied()
        .find(|style| name.eq_ignore_ascii_case(style.as_str()))
        .ok_or(())
}

fn parse_number_or_percent(input: &mut Parser<'_, '_>) -> Result<NumberOrPercent, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(NumberOrPercent::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(NumberOrPercent::Percent(unit_value))
        }
        _ => Err(()),
    }
}

impl BorderImageSlice {
    /// `<number-percentage>{1,4} && fill?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BorderImageSlice, ()> {
        let mut fill = false;
        let mut values: Vec<NumberOrPercent> = Vec::new();
        loop {
            let state = input.state();
            let is_fill = matches!(
                input.next(),
                Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("fill")
            );
            if is_fill {
                if fill {
                    input.reset(&state);
                    break;
                }
                fill = true;
                continue;
            }
            input.reset(&state);
            if values.len() < 4 {
                if let Ok(value) = parse_number_or_percent(input) {
                    values.push(value);
                    continue;
                }
                input.reset(&state);
            }
            break;
        }
        let sides = four_sides(values).ok_or(())?;
        Ok(BorderImageSlice { sides, fill })
    }
}

impl BorderImageWidthSide {
    /// `<length-percentage> | <number> | auto`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BorderImageWidthSide, ()> {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("auto") => {
                Ok(BorderImageWidthSide::Auto)
            }
            Token::Number { value, .. } if value.is_finite() => {
                Ok(BorderImageWidthSide::Number(value))
            }
            _ => {
                input.reset(&state);
                Length::parse(input).map(BorderImageWidthSide::Length)
            }
        }
    }
}

impl RepeatStyle {
    /// `border-image-repeat`: `[stretch | repeat | round | space]{1,2}`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<RepeatStyle, ()> {
        const AXES: &[Keyword] = &[
            Keyword::Stretch,
            Keyword::Repeat,
            Keyword::Round,
            Keyword::Space,
        ];
        let x = parse_one_of(input, AXES)?;
        let state = input.state();
        let y = match parse_one_of(input, AXES) {
            Ok(keyword) => keyword,
            Err(()) => {
                input.reset(&state);
                x
            }
        };
        Ok(RepeatStyle { x, y })
    }

    /// `background-repeat`: adds `repeat-x` / `repeat-y` / `no-repeat`.
    pub fn parse_background(input: &mut Parser<'_, '_>) -> Result<RepeatStyle, ()> {
        const AXES: &[Keyword] = &[
            Keyword::Repeat,
            Keyword::Round,
            Keyword::Space,
            Keyword::NoRepeat,
        ];
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            if name.eq_ignore_ascii_case("repeat-x") {
                return Ok(RepeatStyle { x: Keyword::Repeat, y: Keyword::NoRepeat });
            }
            if name.eq_ignore_ascii_case("repeat-y") {
                return Ok(RepeatStyle { x: Keyword::NoRepeat, y: Keyword::Repeat });
            }
        }
        input.reset(&state);
        let x = parse_one_of(input, AXES)?;
        let state = input.state();
        let y = match parse_one_of(input, AXES) {
            Ok(keyword) => keyword,
            Err(()) => {
                input.reset(&state);
                x
            }
        };
        Ok(RepeatStyle { x, y })
    }
}

fn parse_one_of(input: &mut Parser<'_, '_>, allowed: &[Keyword]) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    allowed
        .iter()
        .copied()
        .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
        .ok_or(())
}

impl BgSize {
    /// `[<length-percentage> | auto]{1,2} | cover | contain`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BgSize, ()> {
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            if name.eq_ignore_ascii_case("cover") {
                return Ok(BgSize::Cover);
            }
            if name.eq_ignore_ascii_case("contain") {
                return Ok(BgSize::Contain);
            }
            if name.eq_ignore_ascii_case("auto") {
                let state = input.state();
                return match Length::parse_allowing_auto(input) {
                    Ok(height) => Ok(BgSize::Explicit(Length::Auto, height)),
                    Err(()) => {
                        input.reset(&state);
                        Ok(BgSize::Auto)
                    }
                };
            }
        }
        input.reset(&state);
        let width = Length::parse_allowing_auto(input)?;
        let state = input.state();
        let height = match Length::parse_allowing_auto(input) {
            Ok(height) => height,
            Err(()) => {
                input.reset(&state);
                Length::Auto
            }
        };
        Ok(BgSize::Explicit(width, height))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value` — Expected: PASS, 12 new tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/shadow.rs ui/src/css/value/border.rs
git commit -m "feat(ui/css): shadows, line widths/styles, border-image and bg-size values

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 9: Font and text values

**Files:**
- Create: `ui/src/css/value/font.rs`, `ui/src/css/value/text.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: both new files

**Interfaces:**
- Consumes: `Length`, `Keyword`, `bitflags`.
- Produces:
  - `css::value::font::{FontFamily, GenericFamily, FontWeight, FontStyle, LineHeight,
    FeatureSetting, VariationSetting, FontVariantFlags}`
  - `font::parse_family_list(&mut Parser<'_, '_>) -> Result<Rc<[FontFamily]>, ()>`
  - `FontWeight::parse`, `FontStyle::parse`, `LineHeight::parse`,
    `font::parse_font_size(&mut Parser<'_, '_>) -> Result<Length, ()>`,
    `font::parse_stretch(&mut Parser<'_, '_>) -> Result<f32, ()>` (CSS percentage, 100 == normal),
    `font::parse_feature_settings`, `font::parse_variation_settings`,
    `font::parse_variant_flags(&mut Parser<'_, '_>, allowed: FontVariantFlags) -> Result<FontVariantFlags, ()>`
  - `css::value::text::TextDecorationLines`, `text::parse_decoration_lines`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/font.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{FontStyle, FontVariantFlags, FontWeight, GenericFamily, LineHeight,
                parse_family_list, parse_feature_settings, parse_font_size, parse_stretch,
                parse_variant_flags, parse_variation_settings};
    use crate::css::value::font::FontFamily;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn a_family_list_keeps_order_and_separates_generics_from_names() {
        let families = parse_entirely_with("\"Adwaita Sans\", Cantarell, sans-serif", parse_family_list)
            .expect("family list parses");
        assert_eq!(families.len(), 3);
        assert_eq!(families[0], FontFamily::Named("Adwaita Sans".into()));
        assert_eq!(families[1], FontFamily::Named("Cantarell".into()));
        assert_eq!(families[2], FontFamily::Generic(GenericFamily::SansSerif));
        // An unquoted multi-word family joins its idents with spaces.
        let unquoted = parse_entirely_with("Adwaita Sans", parse_family_list).expect("parses");
        assert_eq!(unquoted[0], FontFamily::Named("Adwaita Sans".into()));
    }

    #[test]
    fn font_weight_covers_keywords_relatives_and_numbers() {
        let weight = |text: &str| parse_entirely_with(text, FontWeight::parse).ok();
        assert_eq!(weight("normal"), Some(FontWeight::Absolute(400.0)));
        assert_eq!(weight("BOLD"), Some(FontWeight::Absolute(700.0)));
        assert_eq!(weight("bolder"), Some(FontWeight::Bolder));
        assert_eq!(weight("lighter"), Some(FontWeight::Lighter));
        assert_eq!(weight("350"), Some(FontWeight::Absolute(350.0)));
        assert_eq!(weight("0"), None);
        assert_eq!(weight("1001"), None);
    }

    #[test]
    fn font_style_records_obliques_angle() {
        let style = |text: &str| parse_entirely_with(text, FontStyle::parse).ok();
        assert_eq!(style("normal"), Some(FontStyle::Normal));
        assert_eq!(style("italic"), Some(FontStyle::Italic));
        assert_eq!(style("oblique"), Some(FontStyle::Oblique(14.0)));
        assert_eq!(style("oblique 30deg"), Some(FontStyle::Oblique(30.0)));
    }

    #[test]
    fn font_size_accepts_lengths_percentages_and_the_absolute_keywords() {
        let size = |text: &str| parse_entirely_with(text, parse_font_size).ok();
        assert_eq!(size("14px"), Some(Length::px(14.0)));
        assert_eq!(size("120%"), Some(Length::Percent(1.2)));
        assert_eq!(size("1.5em"), parse_entirely_with("1.5em", Length::parse).ok());
        // `medium` is the 14px base this engine uses (DEFAULT_FONT_SIZE).
        assert_eq!(size("medium"), Some(Length::px(14.0)));
        assert_eq!(size("large"), Some(Length::px(14.0 * 1.2)));
        assert_eq!(size("small"), Some(Length::px(14.0 / 1.2)));
        assert_eq!(size("larger"), Some(Length::Percent(1.2)));
        assert_eq!(size("smaller"), Some(Length::Percent(1.0 / 1.2)));
    }

    #[test]
    fn font_stretch_maps_keywords_onto_percentages() {
        let stretch = |text: &str| parse_entirely_with(text, parse_stretch).ok();
        assert_eq!(stretch("normal"), Some(100.0));
        assert_eq!(stretch("ultra-condensed"), Some(50.0));
        assert_eq!(stretch("semi-expanded"), Some(112.5));
        assert_eq!(stretch("ultra-expanded"), Some(200.0));
        assert_eq!(stretch("75%"), Some(75.0));
        assert_eq!(stretch("-5%"), None);
    }

    #[test]
    fn line_height_keeps_numbers_and_lengths_apart() {
        let height = |text: &str| parse_entirely_with(text, LineHeight::parse).ok();
        assert_eq!(height("normal"), Some(LineHeight::Normal));
        assert_eq!(height("1.5"), Some(LineHeight::Number(1.5)));
        assert_eq!(height("18px"), Some(LineHeight::Length(Length::px(18.0))));
        assert_eq!(height("150%"), Some(LineHeight::Length(Length::Percent(1.5))));
    }

    #[test]
    fn feature_and_variation_settings_carry_four_byte_tags() {
        let features = parse_entirely_with("\"liga\" 0, \"tnum\"", parse_feature_settings)
            .expect("feature settings parse");
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].tag, *b"liga");
        assert_eq!(features[0].value, 0);
        assert_eq!(features[1].value, 1);
        let variations = parse_entirely_with("\"wght\" 600", parse_variation_settings)
            .expect("variation settings parse");
        assert_eq!(variations[0].tag, *b"wght");
        assert_eq!(variations[0].value, 600.0);
        // A tag must be exactly four ASCII bytes.
        assert!(parse_entirely_with("\"lig\" 1", parse_feature_settings).is_err());
    }

    #[test]
    fn variant_flags_accumulate_and_reject_values_outside_their_longhand() {
        let caps = FontVariantFlags::SMALL_CAPS | FontVariantFlags::ALL_SMALL_CAPS;
        let parsed = parse_entirely_with("small-caps", |input| parse_variant_flags(input, caps))
            .expect("parses");
        assert_eq!(parsed, FontVariantFlags::SMALL_CAPS);
        assert_eq!(
            parse_entirely_with("normal", |input| parse_variant_flags(input, caps)).ok(),
            Some(FontVariantFlags::empty())
        );
        assert!(parse_entirely_with("ordinal", |input| parse_variant_flags(input, caps)).is_err());
        let numeric = FontVariantFlags::ORDINAL | FontVariantFlags::SLASHED_ZERO;
        assert_eq!(
            parse_entirely_with("ordinal slashed-zero", |input| parse_variant_flags(input, numeric)).ok(),
            Some(numeric)
        );
    }

    #[test]
    fn font_value_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_family_list);
            let _ = parse_entirely_with(input, FontWeight::parse);
            let _ = parse_entirely_with(input, FontStyle::parse);
            let _ = parse_entirely_with(input, LineHeight::parse);
            let _ = parse_entirely_with(input, parse_font_size);
            let _ = parse_entirely_with(input, parse_stretch);
            let _ = parse_entirely_with(input, parse_feature_settings);
            let _ = parse_entirely_with(input, parse_variation_settings);
            let _ = parse_entirely_with(input, |p| parse_variant_flags(p, FontVariantFlags::all()));
        }
    }
}
```

Append to `ui/src/css/value/text.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{TextDecorationLines, parse_decoration_lines};
    use crate::css::value::parse_entirely_with;

    #[test]
    fn decoration_lines_combine_in_any_order() {
        let lines = |text: &str| parse_entirely_with(text, parse_decoration_lines).ok();
        assert_eq!(lines("none"), Some(TextDecorationLines::empty()));
        assert_eq!(lines("underline"), Some(TextDecorationLines::UNDERLINE));
        assert_eq!(
            lines("line-through UNDERLINE"),
            Some(TextDecorationLines::UNDERLINE | TextDecorationLines::LINE_THROUGH)
        );
        assert_eq!(
            lines("underline overline line-through blink"),
            Some(TextDecorationLines::all())
        );
        // A repeated keyword is invalid, and `none` cannot be combined.
        assert_eq!(lines("underline underline"), None);
        assert_eq!(lines("none underline"), None);
    }

    #[test]
    fn decoration_line_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_decoration_lines);
        }
    }
}
```

Mutation check: `font_stretch_maps_keywords_onto_percentages` fails if the keyword
table is off by one step — Part 6's `css_stretch_to_fc` conversion is built on these
exact percentages. `font_size_accepts_lengths_percentages_and_the_absolute_keywords`
fails if `medium` stops being `DEFAULT_FONT_SIZE`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value`
Expected: FAIL — `error[E0583]: file not found for module 'font'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod font;` / `pub mod text;` and
`pub use font::{FeatureSetting, FontFamily, FontStyle, FontVariantFlags, FontWeight,
GenericFamily, LineHeight, VariationSetting};` /
`pub use text::TextDecorationLines;` to `ui/src/css/value/mod.rs`.

Create `ui/src/css/value/font.rs`:

```rust
//! Font-family, weight, style, size, stretch, line-height, feature and
//! variation values.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::length::Length;

/// The engine's `medium` font size, matching M1's `DEFAULT_FONT_SIZE`.
pub const MEDIUM_FONT_SIZE_PX: f32 = 14.0;

/// A CSS generic family.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GenericFamily {
    /// `serif`.
    Serif,
    /// `sans-serif`.
    SansSerif,
    /// `monospace`.
    Monospace,
    /// `cursive`.
    Cursive,
    /// `fantasy`.
    Fantasy,
    /// `system-ui`.
    SystemUi,
}

impl GenericFamily {
    fn from_str_ascii_ci(name: &str) -> Option<GenericFamily> {
        const GENERICS: &[(&str, GenericFamily)] = &[
            ("serif", GenericFamily::Serif),
            ("sans-serif", GenericFamily::SansSerif),
            ("monospace", GenericFamily::Monospace),
            ("cursive", GenericFamily::Cursive),
            ("fantasy", GenericFamily::Fantasy),
            ("system-ui", GenericFamily::SystemUi),
        ];
        GENERICS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, generic)| *generic)
    }
}

/// One entry of a `font-family` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFamily {
    /// A concrete family name.
    Named(Rc<str>),
    /// A generic family.
    Generic(GenericFamily),
}

/// `font-weight`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontWeight {
    /// An absolute weight in `1..=1000`.
    Absolute(f32),
    /// One step bolder than the parent's.
    Bolder,
    /// One step lighter than the parent's.
    Lighter,
}

/// `font-style`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontStyle {
    /// Upright.
    Normal,
    /// Italic.
    Italic,
    /// Oblique, in degrees; bare `oblique` is `14`.
    Oblique(f32),
}

/// `line-height`.
#[derive(Clone, Debug, PartialEq)]
pub enum LineHeight {
    /// The face's own line height.
    Normal,
    /// A multiple of the font size.
    Number(f32),
    /// An explicit length or percentage of the font size.
    Length(Length),
}

/// One `font-feature-settings` entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeatureSetting {
    /// The four-byte OpenType tag.
    pub tag: [u8; 4],
    /// The feature value; `on` == 1, `off` == 0.
    pub value: i32,
}

/// One `font-variation-settings` entry.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VariationSetting {
    /// The four-byte axis tag.
    pub tag: [u8; 4],
    /// The axis value.
    pub value: f32,
}

bitflags::bitflags! {
    /// One flag set covering `font-variant` and every `font-variant-*`
    /// longhand, so all of them share a single `Value` variant.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct FontVariantFlags: u32 {
        /// `small-caps`.
        const SMALL_CAPS = 1 << 0;
        /// `all-small-caps`.
        const ALL_SMALL_CAPS = 1 << 1;
        /// `petite-caps`.
        const PETITE_CAPS = 1 << 2;
        /// `all-petite-caps`.
        const ALL_PETITE_CAPS = 1 << 3;
        /// `unicase`.
        const UNICASE = 1 << 4;
        /// `titling-caps`.
        const TITLING_CAPS = 1 << 5;
        /// `no-common-ligatures`.
        const NO_COMMON_LIGATURES = 1 << 6;
        /// `discretionary-ligatures`.
        const DISCRETIONARY_LIGATURES = 1 << 7;
        /// `historical-ligatures`.
        const HISTORICAL_LIGATURES = 1 << 8;
        /// `no-contextual`.
        const NO_CONTEXTUAL = 1 << 9;
        /// `sub`.
        const SUB = 1 << 10;
        /// `super`.
        const SUPER = 1 << 11;
        /// `lining-nums`.
        const LINING_NUMS = 1 << 12;
        /// `oldstyle-nums`.
        const OLDSTYLE_NUMS = 1 << 13;
        /// `proportional-nums`.
        const PROPORTIONAL_NUMS = 1 << 14;
        /// `tabular-nums`.
        const TABULAR_NUMS = 1 << 15;
        /// `diagonal-fractions`.
        const DIAGONAL_FRACTIONS = 1 << 16;
        /// `stacked-fractions`.
        const STACKED_FRACTIONS = 1 << 17;
        /// `ordinal`.
        const ORDINAL = 1 << 18;
        /// `slashed-zero`.
        const SLASHED_ZERO = 1 << 19;
        /// `historical-forms`.
        const HISTORICAL_FORMS = 1 << 20;
        /// `ruby`.
        const RUBY = 1 << 21;
        /// `jis78`.
        const JIS78 = 1 << 22;
        /// `jis83`.
        const JIS83 = 1 << 23;
        /// `jis90`.
        const JIS90 = 1 << 24;
        /// `jis04`.
        const JIS04 = 1 << 25;
        /// `simplified`.
        const SIMPLIFIED = 1 << 26;
        /// `traditional`.
        const TRADITIONAL = 1 << 27;
        /// `full-width` (east-asian).
        const FULL_WIDTH_EA = 1 << 28;
        /// `proportional-width` (east-asian).
        const PROPORTIONAL_WIDTH_EA = 1 << 29;
    }
}

/// Every `font-variant-*` keyword and the flag it sets.
const VARIANT_KEYWORDS: &[(&str, FontVariantFlags)] = &[
    ("small-caps", FontVariantFlags::SMALL_CAPS),
    ("all-small-caps", FontVariantFlags::ALL_SMALL_CAPS),
    ("petite-caps", FontVariantFlags::PETITE_CAPS),
    ("all-petite-caps", FontVariantFlags::ALL_PETITE_CAPS),
    ("unicase", FontVariantFlags::UNICASE),
    ("titling-caps", FontVariantFlags::TITLING_CAPS),
    ("no-common-ligatures", FontVariantFlags::NO_COMMON_LIGATURES),
    ("discretionary-ligatures", FontVariantFlags::DISCRETIONARY_LIGATURES),
    ("historical-ligatures", FontVariantFlags::HISTORICAL_LIGATURES),
    ("no-contextual", FontVariantFlags::NO_CONTEXTUAL),
    ("sub", FontVariantFlags::SUB),
    ("super", FontVariantFlags::SUPER),
    ("lining-nums", FontVariantFlags::LINING_NUMS),
    ("oldstyle-nums", FontVariantFlags::OLDSTYLE_NUMS),
    ("proportional-nums", FontVariantFlags::PROPORTIONAL_NUMS),
    ("tabular-nums", FontVariantFlags::TABULAR_NUMS),
    ("diagonal-fractions", FontVariantFlags::DIAGONAL_FRACTIONS),
    ("stacked-fractions", FontVariantFlags::STACKED_FRACTIONS),
    ("ordinal", FontVariantFlags::ORDINAL),
    ("slashed-zero", FontVariantFlags::SLASHED_ZERO),
    ("historical-forms", FontVariantFlags::HISTORICAL_FORMS),
    ("ruby", FontVariantFlags::RUBY),
    ("jis78", FontVariantFlags::JIS78),
    ("jis83", FontVariantFlags::JIS83),
    ("jis90", FontVariantFlags::JIS90),
    ("jis04", FontVariantFlags::JIS04),
    ("simplified", FontVariantFlags::SIMPLIFIED),
    ("traditional", FontVariantFlags::TRADITIONAL),
    ("full-width", FontVariantFlags::FULL_WIDTH_EA),
    ("proportional-width", FontVariantFlags::PROPORTIONAL_WIDTH_EA),
];

/// `normal | <keyword>+`, restricted to `allowed` so each longhand rejects
/// the other longhands' keywords.
pub fn parse_variant_flags(
    input: &mut Parser<'_, '_>,
    allowed: FontVariantFlags,
) -> Result<FontVariantFlags, ()> {
    let mut flags = FontVariantFlags::empty();
    let mut count = 0_u32;
    loop {
        let state = input.state();
        let name = match input.expect_ident() {
            Ok(name) => name.as_ref().to_string(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        if name.eq_ignore_ascii_case("normal") {
            if count > 0 {
                return Err(());
            }
            count += 1;
            continue;
        }
        let flag = VARIANT_KEYWORDS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, flag)| *flag)
            .ok_or(())?;
        if !allowed.contains(flag) || flags.contains(flag) {
            return Err(());
        }
        flags |= flag;
        count += 1;
    }
    if count == 0 { Err(()) } else { Ok(flags) }
}

/// `<family-name>#`.
pub fn parse_family_list(input: &mut Parser<'_, '_>) -> Result<Rc<[FontFamily]>, ()> {
    let families = input
        .parse_comma_separated(|argument| match parse_one_family(argument) {
            Ok(family) => Ok(family),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if families.is_empty() {
        return Err(());
    }
    Ok(families.into())
}

fn parse_one_family(input: &mut Parser<'_, '_>) -> Result<FontFamily, ()> {
    let state = input.state();
    if let Ok(quoted) = input.expect_string() {
        return Ok(FontFamily::Named(Rc::from(quoted.as_ref())));
    }
    input.reset(&state);
    let first = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    if let Some(generic) = GenericFamily::from_str_ascii_ci(&first) {
        return Ok(FontFamily::Generic(generic));
    }
    // An unquoted family may be several idents: `Adwaita Sans`.
    let mut name = first;
    loop {
        let state = input.state();
        match input.expect_ident() {
            Ok(part) => {
                name.push(' ');
                name.push_str(part.as_ref());
            }
            Err(_) => {
                input.reset(&state);
                break;
            }
        }
    }
    Ok(FontFamily::Named(Rc::from(name.as_str())))
}

impl FontWeight {
    /// `normal | bold | bolder | lighter | <number [1,1000]>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<FontWeight, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) => {
                if name.eq_ignore_ascii_case("normal") {
                    Ok(FontWeight::Absolute(400.0))
                } else if name.eq_ignore_ascii_case("bold") {
                    Ok(FontWeight::Absolute(700.0))
                } else if name.eq_ignore_ascii_case("bolder") {
                    Ok(FontWeight::Bolder)
                } else if name.eq_ignore_ascii_case("lighter") {
                    Ok(FontWeight::Lighter)
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } if (1.0..=1000.0).contains(&value) => {
                Ok(FontWeight::Absolute(value))
            }
            _ => Err(()),
        }
    }
}

impl FontStyle {
    /// `normal | italic | oblique <angle>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<FontStyle, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        if name.eq_ignore_ascii_case("normal") {
            return Ok(FontStyle::Normal);
        }
        if name.eq_ignore_ascii_case("italic") {
            return Ok(FontStyle::Italic);
        }
        if !name.eq_ignore_ascii_case("oblique") {
            return Err(());
        }
        let state = input.state();
        match parse_angle(input) {
            Ok(angle) => Ok(FontStyle::Oblique(angle)),
            Err(()) => {
                input.reset(&state);
                Ok(FontStyle::Oblique(14.0))
            }
        }
    }
}

impl LineHeight {
    /// `normal | <number> | <length-percentage>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<LineHeight, ()> {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("normal") => Ok(LineHeight::Normal),
            Token::Number { value, .. } if value.is_finite() && value >= 0.0 => {
                Ok(LineHeight::Number(value))
            }
            _ => {
                input.reset(&state);
                Length::parse(input).map(LineHeight::Length)
            }
        }
    }
}

/// `<length-percentage> | <absolute-size> | <relative-size>`.
///
/// The absolute-size keywords scale `medium` by the CSS-standard 1.2 ratio;
/// the relative ones become percentages of the parent's size, so `computed`
/// resolves them with no extra machinery.
pub fn parse_font_size(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
    const ABSOLUTE: &[(&str, f32)] = &[
        ("xx-small", 1.0 / (1.2 * 1.2 * 1.2)),
        ("x-small", 1.0 / (1.2 * 1.2)),
        ("small", 1.0 / 1.2),
        ("medium", 1.0),
        ("large", 1.2),
        ("x-large", 1.2 * 1.2),
        ("xx-large", 1.2 * 1.2 * 1.2),
        ("xxx-large", 1.2 * 1.2 * 1.2 * 1.2),
    ];
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        let name = name.as_ref().to_string();
        if let Some((_, factor)) = ABSOLUTE
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
        {
            return Ok(Length::px(MEDIUM_FONT_SIZE_PX * factor));
        }
        if name.eq_ignore_ascii_case("larger") {
            return Ok(Length::Percent(1.2));
        }
        if name.eq_ignore_ascii_case("smaller") {
            return Ok(Length::Percent(1.0 / 1.2));
        }
    }
    input.reset(&state);
    Length::parse(input)
}

/// `font-stretch` / `font-width`, always as a CSS percentage where
/// `100` is `normal`.
pub fn parse_stretch(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    const KEYWORDS: &[(&str, f32)] = &[
        ("ultra-condensed", 50.0),
        ("extra-condensed", 62.5),
        ("condensed", 75.0),
        ("semi-condensed", 87.5),
        ("normal", 100.0),
        ("semi-expanded", 112.5),
        ("expanded", 125.0),
        ("extra-expanded", 150.0),
        ("ultra-expanded", 200.0),
    ];
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Ident(ref name) => KEYWORDS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, percent)| *percent)
            .ok_or(()),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() && unit_value >= 0.0 => {
            Ok(unit_value * 100.0)
        }
        _ => Err(()),
    }
}

fn parse_tag(input: &mut Parser<'_, '_>) -> Result<[u8; 4], ()> {
    let text = input.expect_string().map_err(|_| ())?.as_ref().to_string();
    let bytes = text.as_bytes();
    if bytes.len() != 4 || !text.is_ascii() {
        return Err(());
    }
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// `normal | <feature-tag-value>#`.
pub fn parse_feature_settings(input: &mut Parser<'_, '_>) -> Result<Rc<[FeatureSetting]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        if name.eq_ignore_ascii_case("normal") {
            return Ok(Rc::from(Vec::new()));
        }
    }
    input.reset(&state);
    let settings = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let tag = parse_tag(argument)?;
                let state = argument.state();
                let value = match argument.next() {
                    Ok(Token::Number { int_value: Some(number), .. }) => *number,
                    Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("on") => 1,
                    Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("off") => 0,
                    _ => {
                        argument.reset(&state);
                        1
                    }
                };
                Ok::<FeatureSetting, ()>(FeatureSetting { tag, value })
            })();
            match parsed {
                Ok(setting) => Ok(setting),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(settings.into())
}

/// `normal | [<opentype-tag> <number>]#`.
pub fn parse_variation_settings(input: &mut Parser<'_, '_>) -> Result<Rc<[VariationSetting]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        if name.eq_ignore_ascii_case("normal") {
            return Ok(Rc::from(Vec::new()));
        }
    }
    input.reset(&state);
    let settings = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let tag = parse_tag(argument)?;
                let value = argument.expect_number().map_err(|_| ())?;
                if !value.is_finite() {
                    return Err(());
                }
                Ok::<VariationSetting, ()>(VariationSetting { tag, value })
            })();
            match parsed {
                Ok(setting) => Ok(setting),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(settings.into())
}
```

Create `ui/src/css/value/text.rs`:

```rust
//! Text-decoration values.

use cssparser::Parser;

bitflags::bitflags! {
    /// `text-decoration-line`.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct TextDecorationLines: u8 {
        /// `underline`.
        const UNDERLINE = 1;
        /// `overline`.
        const OVERLINE = 2;
        /// `line-through`.
        const LINE_THROUGH = 4;
        /// `blink` (parsed; never animated).
        const BLINK = 8;
    }
}

/// `none | [underline || overline || line-through || blink]`.
pub fn parse_decoration_lines(input: &mut Parser<'_, '_>) -> Result<TextDecorationLines, ()> {
    const LINES: &[(&str, TextDecorationLines)] = &[
        ("underline", TextDecorationLines::UNDERLINE),
        ("overline", TextDecorationLines::OVERLINE),
        ("line-through", TextDecorationLines::LINE_THROUGH),
        ("blink", TextDecorationLines::BLINK),
    ];
    let mut lines = TextDecorationLines::empty();
    let mut saw_none = false;
    let mut count = 0_u32;
    loop {
        let state = input.state();
        let name = match input.expect_ident() {
            Ok(name) => name.as_ref().to_string(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        if name.eq_ignore_ascii_case("none") {
            if count > 0 {
                return Err(());
            }
            saw_none = true;
            count += 1;
            continue;
        }
        if saw_none {
            return Err(());
        }
        let flag = LINES
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, flag)| *flag)
            .ok_or(())?;
        if lines.contains(flag) {
            return Err(());
        }
        lines |= flag;
        count += 1;
    }
    if count == 0 { Err(()) } else { Ok(lines) }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value` — Expected: PASS, 11 new tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/font.rs ui/src/css/value/text.rs
git commit -m "feat(ui/css): font and text-decoration values

One FontVariantFlags bit set covers font-variant and all seven
font-variant-* longhands; each longhand passes the subset it accepts, so
a keyword from the wrong longhand is a parse error rather than silently
setting an unrelated flag.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 10: Transforms and filters

**Files:**
- Create: `ui/src/css/value/transform.rs`, `ui/src/css/value/filter.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: both new files

**Interfaces:**
- Consumes: `Length`, `LengthCtx`, `calc::parse_angle`, `Shadow`,
  `skia_rs_safe::core::Matrix`.
- Produces:
  - `css::value::transform::{TransformFn, Decomposed2d}`
  - `TransformFn::to_matrix(&self, &LengthCtx, basis: (f32, f32)) -> Matrix`
  - `transform::parse_transform_list(&mut Parser<'_, '_>) -> Result<Rc<[TransformFn]>, ()>`
  - `transform::transform_list_matrix(&[TransformFn], &LengthCtx, basis: (f32, f32), origin: (f32, f32)) -> Matrix`
  - `transform::decompose_2d(&Matrix) -> Option<Decomposed2d>`, `transform::recompose_2d(&Decomposed2d) -> Matrix`
  - `css::value::filter::FilterFn`, `filter::parse_filter_list(&mut Parser<'_, '_>) -> Result<Rc<[FilterFn]>, ()>`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/transform.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{TransformFn, decompose_2d, parse_transform_list, recompose_2d,
                transform_list_matrix};
    use crate::css::value::length::{Length, LengthCtx};
    use crate::css::value::parse_entirely_with;

    fn list(text: &str) -> Option<Vec<TransformFn>> {
        parse_entirely_with(text, parse_transform_list)
            .ok()
            .map(|functions| functions.to_vec())
    }

    #[test]
    fn none_is_an_empty_list_and_every_function_parses() {
        assert_eq!(list("none"), Some(Vec::new()));
        assert_eq!(list("rotate(1turn)"), Some(vec![TransformFn::Rotate(360.0)]));
        assert_eq!(
            list("translate(1px, 2px) scale(2)"),
            Some(vec![
                TransformFn::Translate(Length::px(1.0), Length::px(2.0)),
                TransformFn::Scale(2.0, 2.0),
            ])
        );
        assert_eq!(list("skewX(10deg)"), Some(vec![TransformFn::SkewX(10.0)]));
        assert!(list("matrix(1, 0, 0, 1, 5, 6)").is_some());
        assert!(list("matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1)").is_some());
        assert!(list("perspective(100px)").is_some());
        assert!(list("rotate3d(0, 0, 1, 90deg)").is_some());
        assert!(list("rotate()").is_none());
    }

    #[test]
    fn a_transform_list_flattens_around_its_origin() {
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(10px, 0)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (0.0, 0.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 0.0, y: 0.0 });
        assert_eq!((point.x, point.y), (10.0, 0.0));

        // Scaling about a (50, 25) origin leaves that point fixed.
        let functions = parse_transform_list_of("scale(2)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (50.0, 25.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 50.0, y: 25.0 });
        assert!((point.x - 50.0).abs() < 1e-4 && (point.y - 25.0).abs() < 1e-4);
    }

    fn parse_transform_list_of(text: &str) -> Vec<TransformFn> {
        parse_entirely_with(text, parse_transform_list)
            .expect("transform list parses")
            .to_vec()
    }

    #[test]
    fn percentage_translations_use_the_basis() {
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(50%, 100%)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (0.0, 0.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 0.0, y: 0.0 });
        assert!((point.x - 50.0).abs() < 1e-4, "{point:?}");
        assert!((point.y - 50.0).abs() < 1e-4, "{point:?}");
    }

    #[test]
    fn decomposition_round_trips_a_2d_matrix() {
        // skia-rs has no matrix decomposition; this is project code, and
        // interpolating two structurally-mismatched lists depends on it.
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(5px, 7px) rotate(30deg) scale(2, 3)");
        let matrix = transform_list_matrix(&functions, &ctx, (10.0, 10.0), (0.0, 0.0));
        let decomposed = decompose_2d(&matrix).expect("an affine matrix decomposes");
        assert!((decomposed.tx - 5.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.ty - 7.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.rotate - 30.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.scale_x - 2.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.scale_y - 3.0).abs() < 1e-3, "{decomposed:?}");
        let rebuilt = recompose_2d(&decomposed);
        for index in 0..6 {
            assert!(
                (rebuilt.values[index] - matrix.values[index]).abs() < 1e-3,
                "component {index}: {rebuilt:?} vs {matrix:?}"
            );
        }
    }

    #[test]
    fn a_degenerate_matrix_does_not_decompose() {
        let flat = skia_rs_safe::core::Matrix { values: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] };
        assert_eq!(decompose_2d(&flat), None);
    }

    #[test]
    fn transform_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            if let Ok(functions) = parse_entirely_with(input, parse_transform_list) {
                let _ = transform_list_matrix(&functions, &LengthCtx::default(), (1.0, 1.0), (0.0, 0.0));
            }
        }
    }
}
```

Append to `ui/src/css/value/filter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{FilterFn, parse_filter_list};
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    fn list(text: &str) -> Option<Vec<FilterFn>> {
        parse_entirely_with(text, parse_filter_list)
            .ok()
            .map(|functions| functions.to_vec())
    }

    #[test]
    fn every_gtk_filter_function_parses() {
        assert_eq!(list("none"), Some(Vec::new()));
        assert_eq!(list("blur(3px)"), Some(vec![FilterFn::Blur(Length::px(3.0))]));
        assert_eq!(list("brightness(50%)"), Some(vec![FilterFn::Brightness(0.5)]));
        assert_eq!(list("contrast(1.5)"), Some(vec![FilterFn::Contrast(1.5)]));
        assert_eq!(list("hue-rotate(90deg)"), Some(vec![FilterFn::HueRotate(90.0)]));
        assert!(list("grayscale(1) invert(1) opacity(0.5) saturate(2) sepia(1)").is_some());
        assert!(matches!(list("drop-shadow(1px 2px 3px red)").as_deref(), Some([FilterFn::DropShadow(_)])));
        assert!(list("blur()").is_none());
        assert!(list("wobble(1)").is_none());
    }

    #[test]
    fn an_omitted_amount_defaults_per_function() {
        // CSS: brightness()/contrast()/saturate() default to 1, the rest to 1
        // except opacity(), which is also 1. A missing argument is only legal
        // for the amount-taking functions.
        assert_eq!(list("grayscale()"), Some(vec![FilterFn::Grayscale(1.0)]));
        assert_eq!(list("hue-rotate()"), Some(vec![FilterFn::HueRotate(0.0)]));
    }

    #[test]
    fn filter_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_filter_list);
        }
    }
}
```

Mutation check: `decomposition_round_trips_a_2d_matrix` is the only guard on
`decompose_2d`; swapping `scale_x`/`scale_y` or negating the rotation fails it.
`a_transform_list_flattens_around_its_origin` fails if the origin translate/untranslate
pair is dropped, which would silently break every `transform-origin` in a theme.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value`
Expected: FAIL — `error[E0583]: file not found for module 'transform'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod filter;` / `pub mod transform;` and
`pub use filter::FilterFn;` / `pub use transform::{Decomposed2d, TransformFn};`
to `ui/src/css/value/mod.rs`.

Create `ui/src/css/value/transform.rs`:

```rust
//! `transform` values, matrix flattening and 2-D decomposition.

use std::rc::Rc;

use cssparser::Parser;
use skia_rs_safe::core::Matrix;

use super::calc::parse_angle;
use super::length::{Length, LengthCtx};

/// One `<transform-function>`.
#[derive(Clone, Debug, PartialEq)]
pub enum TransformFn {
    /// `matrix(a, b, c, d, e, f)`.
    Matrix([f32; 6]),
    /// `matrix3d(...)`, column-major as CSS writes it.
    Matrix3d([f32; 16]),
    /// `translate(x, y)`.
    Translate(Length, Length),
    /// `translateX(x)`.
    TranslateX(Length),
    /// `translateY(y)`.
    TranslateY(Length),
    /// `translateZ(z)`.
    TranslateZ(Length),
    /// `translate3d(x, y, z)`.
    Translate3d(Length, Length, Length),
    /// `scale(x, y)`.
    Scale(f32, f32),
    /// `scaleX(x)`.
    ScaleX(f32),
    /// `scaleY(y)`.
    ScaleY(f32),
    /// `scaleZ(z)`.
    ScaleZ(f32),
    /// `scale3d(x, y, z)`.
    Scale3d(f32, f32, f32),
    /// `rotate(<angle>)`, degrees.
    Rotate(f32),
    /// `rotateX(<angle>)`.
    RotateX(f32),
    /// `rotateY(<angle>)`.
    RotateY(f32),
    /// `rotateZ(<angle>)`.
    RotateZ(f32),
    /// `rotate3d(x, y, z, <angle>)`.
    Rotate3d(f32, f32, f32, f32),
    /// `skew(ax, ay)`, degrees.
    Skew(f32, f32),
    /// `skewX(ax)`.
    SkewX(f32),
    /// `skewY(ay)`.
    SkewY(f32),
    /// `perspective(<length>)`.
    Perspective(Length),
}

fn matrix_of(scale_x: f32, skew_y: f32, skew_x: f32, scale_y: f32, tx: f32, ty: f32) -> Matrix {
    Matrix { values: [scale_x, skew_x, tx, skew_y, scale_y, ty, 0.0, 0.0, 1.0] }
}

impl TransformFn {
    /// The 3x3 matrix for this function. M2 paints in 2-D: the `*Z`/3-D
    /// forms collapse to their in-plane effect.
    #[must_use]
    pub fn to_matrix(&self, ctx: &LengthCtx, basis: (f32, f32)) -> Matrix {
        let horizontal = LengthCtx { percent_basis: Some(basis.0), ..*ctx };
        let vertical = LengthCtx { percent_basis: Some(basis.1), ..*ctx };
        let px = |length: &Length, ctx: &LengthCtx| length.resolve(ctx).unwrap_or(0.0);
        match self {
            TransformFn::Matrix(m) => matrix_of(m[0], m[1], m[2], m[3], m[4], m[5]),
            TransformFn::Matrix3d(m) => matrix_of(m[0], m[1], m[4], m[5], m[12], m[13]),
            TransformFn::Translate(x, y) => {
                Matrix::translate(px(x, &horizontal), px(y, &vertical))
            }
            TransformFn::TranslateX(x) => Matrix::translate(px(x, &horizontal), 0.0),
            TransformFn::TranslateY(y) => Matrix::translate(0.0, px(y, &vertical)),
            TransformFn::TranslateZ(_) => Matrix::IDENTITY,
            TransformFn::Translate3d(x, y, _) => {
                Matrix::translate(px(x, &horizontal), px(y, &vertical))
            }
            TransformFn::Scale(x, y) => Matrix::scale(*x, *y),
            TransformFn::ScaleX(x) => Matrix::scale(*x, 1.0),
            TransformFn::ScaleY(y) => Matrix::scale(1.0, *y),
            TransformFn::ScaleZ(_) => Matrix::IDENTITY,
            TransformFn::Scale3d(x, y, _) => Matrix::scale(*x, *y),
            TransformFn::Rotate(degrees) | TransformFn::RotateZ(degrees) => {
                Matrix::rotate(degrees.to_radians())
            }
            TransformFn::RotateX(_) | TransformFn::RotateY(_) => Matrix::IDENTITY,
            TransformFn::Rotate3d(_, _, z, degrees) => {
                if *z == 0.0 {
                    Matrix::IDENTITY
                } else {
                    Matrix::rotate(degrees.to_radians() * z.signum())
                }
            }
            TransformFn::Skew(ax, ay) => {
                matrix_of(1.0, ay.to_radians().tan(), ax.to_radians().tan(), 1.0, 0.0, 0.0)
            }
            TransformFn::SkewX(ax) => matrix_of(1.0, 0.0, ax.to_radians().tan(), 1.0, 0.0, 0.0),
            TransformFn::SkewY(ay) => matrix_of(1.0, ay.to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            TransformFn::Perspective(_) => Matrix::IDENTITY,
        }
    }
}

/// Flatten a list to one matrix, applied about `origin`.
#[must_use]
pub fn transform_list_matrix(
    list: &[TransformFn],
    ctx: &LengthCtx,
    basis: (f32, f32),
    origin: (f32, f32),
) -> Matrix {
    let mut matrix = Matrix::translate(origin.0, origin.1);
    for function in list {
        matrix = matrix.concat(&function.to_matrix(ctx, basis));
    }
    matrix.concat(&Matrix::translate(-origin.0, -origin.1))
}

/// A 2-D matrix's translate/rotate/scale/skew components.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Decomposed2d {
    /// Horizontal translation.
    pub tx: f32,
    /// Vertical translation.
    pub ty: f32,
    /// Rotation in degrees.
    pub rotate: f32,
    /// Horizontal scale.
    pub scale_x: f32,
    /// Vertical scale.
    pub scale_y: f32,
    /// The XY skew factor.
    pub skew_xy: f32,
}

/// Decompose an affine matrix. skia-rs has no decomposition of its own;
/// this is used only when interpolating structurally-mismatched lists.
#[must_use]
pub fn decompose_2d(matrix: &Matrix) -> Option<Decomposed2d> {
    let (a, c, e) = (matrix.values[0], matrix.values[1], matrix.values[2]);
    let (b, d, f) = (matrix.values[3], matrix.values[4], matrix.values[5]);
    let determinant = a * d - b * c;
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let scale_x = (a * a + b * b).sqrt();
    if scale_x == 0.0 {
        return None;
    }
    let (a_n, b_n) = (a / scale_x, b / scale_x);
    let skew = a_n * c + b_n * d;
    let (c_s, d_s) = (c - a_n * skew, d - b_n * skew);
    let scale_y = (c_s * c_s + d_s * d_s).sqrt();
    if scale_y == 0.0 {
        return None;
    }
    let skew_xy = skew / scale_y;
    let mut scale_y = scale_y;
    if determinant < 0.0 {
        scale_y = -scale_y;
    }
    Some(Decomposed2d {
        tx: e,
        ty: f,
        rotate: b_n.atan2(a_n).to_degrees(),
        scale_x,
        scale_y,
        skew_xy,
    })
}

/// Rebuild a matrix from its components.
#[must_use]
pub fn recompose_2d(d: &Decomposed2d) -> Matrix {
    let (sin, cos) = d.rotate.to_radians().sin_cos();
    let rotation = matrix_of(cos, sin, -sin, cos, 0.0, 0.0);
    let skew = matrix_of(1.0, 0.0, d.skew_xy, 1.0, 0.0, 0.0);
    let scale = matrix_of(d.scale_x, 0.0, 0.0, d.scale_y, 0.0, 0.0);
    Matrix::translate(d.tx, d.ty)
        .concat(&rotation)
        .concat(&skew)
        .concat(&scale)
}

/// `none | <transform-list>`.
pub fn parse_transform_list(input: &mut Parser<'_, '_>) -> Result<Rc<[TransformFn]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        if name.eq_ignore_ascii_case("none") {
            return Ok(Rc::from(Vec::new()));
        }
    }
    input.reset(&state);
    let mut functions: Vec<TransformFn> = Vec::new();
    loop {
        let state = input.state();
        let name = match input.expect_function() {
            Ok(name) => name.as_ref().to_ascii_lowercase(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        let function = input
            .parse_nested_block(|inner| match parse_transform_function(&name, inner) {
                Ok(function) => Ok(function),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
        functions.push(function);
    }
    if functions.is_empty() {
        return Err(());
    }
    Ok(functions.into())
}

fn numbers<const N: usize>(input: &mut Parser<'_, '_>) -> Result<[f32; N], ()> {
    let mut out = [0.0_f32; N];
    for (index, slot) in out.iter_mut().enumerate() {
        if index > 0 {
            input.expect_comma().map_err(|_| ())?;
        }
        let value = input.expect_number().map_err(|_| ())?;
        if !value.is_finite() {
            return Err(());
        }
        *slot = value;
    }
    Ok(out)
}

fn parse_transform_function(name: &str, input: &mut Parser<'_, '_>) -> Result<TransformFn, ()> {
    let function = match name {
        "matrix" => TransformFn::Matrix(numbers::<6>(input)?),
        "matrix3d" => TransformFn::Matrix3d(numbers::<16>(input)?),
        "translate" => {
            let x = Length::parse(input)?;
            let y = if input.expect_comma().is_ok() {
                Length::parse(input)?
            } else {
                Length::zero()
            };
            TransformFn::Translate(x, y)
        }
        "translatex" => TransformFn::TranslateX(Length::parse(input)?),
        "translatey" => TransformFn::TranslateY(Length::parse(input)?),
        "translatez" => TransformFn::TranslateZ(Length::parse(input)?),
        "translate3d" => {
            let x = Length::parse(input)?;
            input.expect_comma().map_err(|_| ())?;
            let y = Length::parse(input)?;
            input.expect_comma().map_err(|_| ())?;
            let z = Length::parse(input)?;
            TransformFn::Translate3d(x, y, z)
        }
        "scale" => {
            let x = input.expect_number().map_err(|_| ())?;
            let y = if input.expect_comma().is_ok() {
                input.expect_number().map_err(|_| ())?
            } else {
                x
            };
            TransformFn::Scale(x, y)
        }
        "scalex" => TransformFn::ScaleX(input.expect_number().map_err(|_| ())?),
        "scaley" => TransformFn::ScaleY(input.expect_number().map_err(|_| ())?),
        "scalez" => TransformFn::ScaleZ(input.expect_number().map_err(|_| ())?),
        "scale3d" => {
            let [x, y, z] = numbers::<3>(input)?;
            TransformFn::Scale3d(x, y, z)
        }
        "rotate" => TransformFn::Rotate(parse_angle(input)?),
        "rotatex" => TransformFn::RotateX(parse_angle(input)?),
        "rotatey" => TransformFn::RotateY(parse_angle(input)?),
        "rotatez" => TransformFn::RotateZ(parse_angle(input)?),
        "rotate3d" => {
            let [x, y, z] = numbers::<3>(input)?;
            input.expect_comma().map_err(|_| ())?;
            TransformFn::Rotate3d(x, y, z, parse_angle(input)?)
        }
        "skew" => {
            let ax = parse_angle(input)?;
            let ay = if input.expect_comma().is_ok() {
                parse_angle(input)?
            } else {
                0.0
            };
            TransformFn::Skew(ax, ay)
        }
        "skewx" => TransformFn::SkewX(parse_angle(input)?),
        "skewy" => TransformFn::SkewY(parse_angle(input)?),
        "perspective" => TransformFn::Perspective(Length::parse(input)?),
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() { Ok(function) } else { Err(()) }
}
```

Create `ui/src/css/value/filter.rs`:

```rust
//! `filter` and `-gtk-icon-filter` values.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::length::Length;
use super::shadow::Shadow;

/// One `<filter-function>` -- exactly the set GTK documents.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterFn {
    /// `blur(<length>)`.
    Blur(Length),
    /// `brightness(<number-percentage>)`.
    Brightness(f32),
    /// `contrast(<number-percentage>)`.
    Contrast(f32),
    /// `drop-shadow(<shadow>)`.
    DropShadow(Shadow),
    /// `grayscale(<number-percentage>)`.
    Grayscale(f32),
    /// `hue-rotate(<angle>)`, degrees.
    HueRotate(f32),
    /// `invert(<number-percentage>)`.
    Invert(f32),
    /// `opacity(<number-percentage>)`.
    Opacity(f32),
    /// `saturate(<number-percentage>)`.
    Saturate(f32),
    /// `sepia(<number-percentage>)`.
    Sepia(f32),
}

fn amount(input: &mut Parser<'_, '_>, default: f32) -> Result<f32, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Ok(default);
        }
    };
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => Ok(unit_value),
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// `none | <filter-function>+`.
pub fn parse_filter_list(input: &mut Parser<'_, '_>) -> Result<Rc<[FilterFn]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        if name.eq_ignore_ascii_case("none") {
            return Ok(Rc::from(Vec::new()));
        }
    }
    input.reset(&state);
    let mut functions: Vec<FilterFn> = Vec::new();
    loop {
        let state = input.state();
        let name = match input.expect_function() {
            Ok(name) => name.as_ref().to_ascii_lowercase(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        let function = input
            .parse_nested_block(|inner| match parse_filter_function(&name, inner) {
                Ok(function) => Ok(function),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
        functions.push(function);
    }
    if functions.is_empty() {
        return Err(());
    }
    Ok(functions.into())
}

fn parse_filter_function(name: &str, input: &mut Parser<'_, '_>) -> Result<FilterFn, ()> {
    let function = match name {
        "blur" => {
            let state = input.state();
            match Length::parse(input) {
                Ok(length) => FilterFn::Blur(length),
                Err(()) => {
                    input.reset(&state);
                    return Err(());
                }
            }
        }
        "brightness" => FilterFn::Brightness(amount(input, 1.0)?),
        "contrast" => FilterFn::Contrast(amount(input, 1.0)?),
        "grayscale" => FilterFn::Grayscale(amount(input, 1.0)?),
        "invert" => FilterFn::Invert(amount(input, 1.0)?),
        "opacity" => FilterFn::Opacity(amount(input, 1.0)?),
        "saturate" => FilterFn::Saturate(amount(input, 1.0)?),
        "sepia" => FilterFn::Sepia(amount(input, 1.0)?),
        "hue-rotate" => {
            let state = input.state();
            match parse_angle(input) {
                Ok(angle) => FilterFn::HueRotate(angle),
                Err(()) => {
                    input.reset(&state);
                    FilterFn::HueRotate(0.0)
                }
            }
        }
        "drop-shadow" => FilterFn::DropShadow(Shadow::parse_text(input)?),
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() { Ok(function) } else { Err(()) }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value` — Expected: PASS, 9 new tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/transform.rs ui/src/css/value/filter.rs
git commit -m "feat(ui/css): transform and filter values, with 2-D decomposition

skia-rs has no matrix decomposition, so decompose_2d/recompose_2d are
project code; interpolating two structurally-mismatched transform lists
is the only caller.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 11: Timing functions, animation names, iteration counts

**Files:**
- Modify: `ui/src/css/value/timing.rs`, `ui/src/css/value/mod.rs`
- Test: `ui/src/css/value/timing.rs`

**Interfaces:**
- Produces: `css::value::timing::{StepPosition, TimingFunction, AnimationName, IterationCount}`,
  `TimingFunction::{EASE, EASE_IN, EASE_OUT, EASE_IN_OUT, parse, eval}`,
  `AnimationName::parse`, `IterationCount::parse`.

- [ ] **Step 1: Write the failing test**

Append inside `ui/src/css/value/timing.rs`'s `mod tests`:

```rust
    use super::{AnimationName, IterationCount, StepPosition, TimingFunction};

    fn timing(text: &str) -> Option<TimingFunction> {
        parse_entirely_with(text, TimingFunction::parse).ok()
    }

    #[test]
    fn the_named_easings_are_their_cubic_beziers() {
        assert_eq!(timing("linear"), Some(TimingFunction::Linear));
        assert_eq!(timing("EASE"), Some(TimingFunction::EASE));
        assert_eq!(timing("ease-in"), Some(TimingFunction::EASE_IN));
        assert_eq!(timing("ease-out"), Some(TimingFunction::EASE_OUT));
        assert_eq!(timing("ease-in-out"), Some(TimingFunction::EASE_IN_OUT));
        assert_eq!(TimingFunction::EASE, TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0));
        assert_eq!(timing("step-start"), Some(TimingFunction::Steps(1, StepPosition::JumpStart)));
        assert_eq!(timing("step-end"), Some(TimingFunction::Steps(1, StepPosition::JumpEnd)));
        assert_eq!(timing("steps(4)"), Some(TimingFunction::Steps(4, StepPosition::JumpEnd)));
        assert_eq!(timing("steps(4, jump-both)"), Some(TimingFunction::Steps(4, StepPosition::JumpBoth)));
        assert_eq!(timing("steps(4, start)"), Some(TimingFunction::Steps(4, StepPosition::JumpStart)));
        assert_eq!(timing("steps(0, end)"), None);
        // A cubic-bezier's x controls must stay in 0..=1.
        assert_eq!(timing("cubic-bezier(2, 0, 0, 1)"), None);
        assert!(timing("cubic-bezier(0.4, -0.5, 0.6, 1.5)").is_some());
    }

    #[test]
    fn easing_evaluation_is_exact_at_the_endpoints_and_correct_in_between() {
        assert_eq!(TimingFunction::Linear.eval(0.0), 0.0);
        assert_eq!(TimingFunction::Linear.eval(1.0), 1.0);
        assert_eq!(TimingFunction::Linear.eval(0.5), 0.5);
        assert_eq!(TimingFunction::EASE.eval(0.0), 0.0);
        assert_eq!(TimingFunction::EASE.eval(1.0), 1.0);
        // `ease` at t = 0.5 is 0.8024 to four places (CSS Easing L1).
        let mid = TimingFunction::EASE.eval(0.5);
        assert!((mid - 0.802_4).abs() < 1e-3, "ease(0.5) == {mid}");
        // A symmetric bezier is the identity.
        let symmetric = TimingFunction::CubicBezier(0.5, 0.5, 0.5, 0.5);
        assert!((symmetric.eval(0.3) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn step_functions_jump_where_their_position_says() {
        let end = TimingFunction::Steps(2, StepPosition::JumpEnd);
        assert_eq!(end.eval(0.0), 0.0);
        assert_eq!(end.eval(0.49), 0.0);
        assert_eq!(end.eval(0.51), 0.5);
        assert_eq!(end.eval(1.0), 1.0);
        let start = TimingFunction::Steps(2, StepPosition::JumpStart);
        assert_eq!(start.eval(0.0), 0.5);
        assert_eq!(start.eval(1.0), 1.0);
        let none = TimingFunction::Steps(2, StepPosition::JumpNone);
        assert_eq!(none.eval(0.0), 0.0);
        assert_eq!(none.eval(1.0), 1.0);
        let both = TimingFunction::Steps(2, StepPosition::JumpBoth);
        assert!((both.eval(0.0) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(both.eval(1.0), 1.0);
    }

    #[test]
    fn animation_names_and_iteration_counts_parse() {
        assert_eq!(
            parse_entirely_with("none", AnimationName::parse).ok(),
            Some(AnimationName::None)
        );
        assert_eq!(
            parse_entirely_with("needs_attention", AnimationName::parse).ok(),
            Some(AnimationName::Named("needs_attention".into()))
        );
        assert_eq!(
            parse_entirely_with("\"spin\"", AnimationName::parse).ok(),
            Some(AnimationName::Named("spin".into()))
        );
        assert_eq!(
            parse_entirely_with("infinite", IterationCount::parse).ok(),
            Some(IterationCount::Infinite)
        );
        assert_eq!(
            parse_entirely_with("2.5", IterationCount::parse).ok(),
            Some(IterationCount::Count(2.5))
        );
        assert_eq!(parse_entirely_with("-1", IterationCount::parse).ok(), None);
    }

    #[test]
    fn timing_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            if let Ok(function) = parse_entirely_with(input, TimingFunction::parse) {
                for step in -2..=12 {
                    let _ = function.eval(step as f32 / 10.0);
                }
            }
            let _ = parse_entirely_with(input, AnimationName::parse);
            let _ = parse_entirely_with(input, IterationCount::parse);
        }
    }
```

Mutation check: `easing_evaluation_is_exact_at_the_endpoints_and_correct_in_between`
is the value Part 5's `ease at t = 0.5` animation test is derived from; replacing
Newton–Raphson with a linear approximation fails the `0.8024` assertion.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::timing`
Expected: FAIL — `error[E0432]: unresolved import 'super::TimingFunction'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub use timing::{AnimationName, IterationCount, StepPosition, TimingFunction};`
to `ui/src/css/value/mod.rs` and append to `ui/src/css/value/timing.rs`:

```rust
use std::rc::Rc;

/// Where a `steps()` function jumps.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StepPosition {
    /// `jump-start` (alias `start`).
    JumpStart,
    /// `jump-end` (alias `end`, the default).
    JumpEnd,
    /// `jump-none`.
    JumpNone,
    /// `jump-both`.
    JumpBoth,
}

/// A CSS `<easing-function>`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TimingFunction {
    /// The identity.
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)`.
    CubicBezier(f32, f32, f32, f32),
    /// `steps(n, <position>)`.
    Steps(u32, StepPosition),
}

impl TimingFunction {
    /// `ease`.
    pub const EASE: TimingFunction = TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0);
    /// `ease-in`.
    pub const EASE_IN: TimingFunction = TimingFunction::CubicBezier(0.42, 0.0, 1.0, 1.0);
    /// `ease-out`.
    pub const EASE_OUT: TimingFunction = TimingFunction::CubicBezier(0.0, 0.0, 0.58, 1.0);
    /// `ease-in-out`.
    pub const EASE_IN_OUT: TimingFunction = TimingFunction::CubicBezier(0.42, 0.0, 0.58, 1.0);

    /// Parse an `<easing-function>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<TimingFunction, ()> {
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            let name = name.as_ref().to_ascii_lowercase();
            return match name.as_str() {
                "linear" => Ok(TimingFunction::Linear),
                "ease" => Ok(TimingFunction::EASE),
                "ease-in" => Ok(TimingFunction::EASE_IN),
                "ease-out" => Ok(TimingFunction::EASE_OUT),
                "ease-in-out" => Ok(TimingFunction::EASE_IN_OUT),
                "step-start" => Ok(TimingFunction::Steps(1, StepPosition::JumpStart)),
                "step-end" => Ok(TimingFunction::Steps(1, StepPosition::JumpEnd)),
                _ => Err(()),
            };
        }
        input.reset(&state);
        let name = input.expect_function().map_err(|_| ())?.as_ref().to_ascii_lowercase();
        input
            .parse_nested_block(|inner| match parse_timing_body(&name, inner) {
                Ok(function) => Ok(function),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())
    }

    /// Evaluate the function at input progress `t`.
    ///
    /// Cubic beziers are solved for `t` on x by Newton-Raphson (8
    /// iterations) with a bisection fallback, and are exact at 0 and 1.
    #[must_use]
    pub fn eval(self, t: f32) -> f32 {
        match self {
            TimingFunction::Linear => t,
            TimingFunction::CubicBezier(x1, y1, x2, y2) => {
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                let bezier = |a: f32, b: f32, u: f32| {
                    let v = 1.0 - u;
                    3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
                };
                let slope = |a: f32, b: f32, u: f32| {
                    let v = 1.0 - u;
                    3.0 * v * v * (a) + 6.0 * v * u * (b - a) + 3.0 * u * u * (1.0 - b)
                };
                let mut guess = t;
                for _ in 0..8 {
                    let x = bezier(x1, x2, guess) - t;
                    if x.abs() < 1e-6 {
                        return bezier(y1, y2, guess);
                    }
                    let derivative = slope(x1, x2, guess);
                    if derivative.abs() < 1e-6 {
                        break;
                    }
                    guess -= x / derivative;
                }
                let (mut low, mut high) = (0.0_f32, 1.0_f32);
                let mut guess = t;
                for _ in 0..32 {
                    let x = bezier(x1, x2, guess);
                    if (x - t).abs() < 1e-6 {
                        break;
                    }
                    if x > t {
                        high = guess;
                    } else {
                        low = guess;
                    }
                    guess = f32::midpoint(low, high);
                }
                bezier(y1, y2, guess)
            }
            TimingFunction::Steps(count, position) => {
                if count == 0 {
                    return t;
                }
                let steps = count as f32;
                let mut step = (t * steps).floor();
                if matches!(position, StepPosition::JumpStart | StepPosition::JumpBoth) {
                    step += 1.0;
                }
                let denominator = match position {
                    StepPosition::JumpNone => (steps - 1.0).max(1.0),
                    StepPosition::JumpBoth => steps + 1.0,
                    _ => steps,
                };
                if matches!(position, StepPosition::JumpNone) && t >= 1.0 {
                    return 1.0;
                }
                (step / denominator).clamp(0.0, 1.0)
            }
        }
    }
}

fn parse_timing_body(name: &str, input: &mut Parser<'_, '_>) -> Result<TimingFunction, ()> {
    let function = match name {
        "cubic-bezier" => {
            let mut values = [0.0_f32; 4];
            for (index, slot) in values.iter_mut().enumerate() {
                if index > 0 {
                    input.expect_comma().map_err(|_| ())?;
                }
                let value = input.expect_number().map_err(|_| ())?;
                if !value.is_finite() {
                    return Err(());
                }
                *slot = value;
            }
            if !(0.0..=1.0).contains(&values[0]) || !(0.0..=1.0).contains(&values[2]) {
                return Err(());
            }
            TimingFunction::CubicBezier(values[0], values[1], values[2], values[3])
        }
        "steps" => {
            let count = input.expect_integer().map_err(|_| ())?;
            if count < 1 {
                return Err(());
            }
            let position = if input.expect_comma().is_ok() {
                let keyword = input.expect_ident().map_err(|_| ())?.as_ref().to_ascii_lowercase();
                match keyword.as_str() {
                    "jump-start" | "start" => StepPosition::JumpStart,
                    "jump-end" | "end" => StepPosition::JumpEnd,
                    "jump-none" => StepPosition::JumpNone,
                    "jump-both" => StepPosition::JumpBoth,
                    _ => return Err(()),
                }
            } else {
                StepPosition::JumpEnd
            };
            let count = u32::try_from(count).map_err(|_| ())?;
            TimingFunction::Steps(count, position)
        }
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() { Ok(function) } else { Err(()) }
}

/// `animation-name`, and the ident carrier for `transition-property`.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationName {
    /// `none`.
    None,
    /// A `<custom-ident>` or quoted name.
    Named(Rc<str>),
}

impl AnimationName {
    /// `none | <keyframes-name>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<AnimationName, ()> {
        let state = input.state();
        if let Ok(quoted) = input.expect_string() {
            return Ok(AnimationName::Named(Rc::from(quoted.as_ref())));
        }
        input.reset(&state);
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        if name.eq_ignore_ascii_case("none") {
            Ok(AnimationName::None)
        } else {
            Ok(AnimationName::Named(Rc::from(name.as_str())))
        }
    }
}

/// `animation-iteration-count`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum IterationCount {
    /// `infinite`.
    Infinite,
    /// A finite count.
    Count(f32),
}

impl IterationCount {
    /// `infinite | <number>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<IterationCount, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("infinite") => {
                Ok(IterationCount::Infinite)
            }
            Token::Number { value, .. } if value.is_finite() && value >= 0.0 => {
                Ok(IterationCount::Count(value))
            }
            _ => Err(()),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::timing` — Expected: PASS, 9 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/timing.rs
git commit -m "feat(ui/css): easing functions, animation names, iteration counts

cubic-bezier is solved on x by Newton-Raphson with a bisection fallback
and is exact at both endpoints; Part 5's manual-clock tests derive their
expected samples from eval().

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 12: The `Value` enum and the wide-keyword plumbing

**Files:**
- Modify: `ui/src/css/value/mod.rs`
- Test: `ui/src/css/value/mod.rs`

**Interfaces:**
- Consumes: every type from Tasks 2–11.
- Produces: `css::value::Value` (all 26 variants), `Value::parse_wide`,
  `Value::parse_wide_or`, `Value::is_wide`, `Value::parse_list`,
  and the `ParseFn` type alias re-exported from `registry` in Task 14.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Keyword, Length, Value, Wide, parse_entirely_with};

    fn keyword_parser(input: &mut cssparser::Parser<'_, '_>) -> Result<Value, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        Keyword::from_str_ascii_ci(&name).map(Value::Keyword).ok_or(())
    }

    fn length_parser(input: &mut cssparser::Parser<'_, '_>) -> Result<Value, ()> {
        Length::parse(input).map(Value::Length)
    }

    #[test]
    fn the_wide_keywords_are_recognised_on_any_property() {
        for (text, expected) in [
            ("inherit", Wide::Inherit),
            ("INITIAL", Wide::Initial),
            ("unset", Wide::Unset),
        ] {
            let value = parse_entirely_with(text, |input| Value::parse_wide_or(input, keyword_parser))
                .expect("wide keyword parses");
            assert_eq!(value, Value::Wide(expected));
            assert!(value.is_wide());
        }
        // The inner parser still runs for anything else.
        assert_eq!(
            parse_entirely_with("none", |input| Value::parse_wide_or(input, keyword_parser)).ok(),
            Some(Value::Keyword(Keyword::None))
        );
        assert!(!Value::Keyword(Keyword::None).is_wide());
    }

    #[test]
    fn a_wide_keyword_with_trailing_tokens_is_not_a_wide_keyword() {
        // `inherit 4px` is invalid, not "inherit plus junk".
        assert!(
            parse_entirely_with("inherit 4px", |input| Value::parse_wide_or(input, length_parser))
                .is_err()
        );
    }

    #[test]
    fn parse_list_splits_on_top_level_commas() {
        let value = parse_entirely_with("1px, 2px, 3px", |input| Value::parse_list(input, length_parser))
            .expect("list parses");
        match value {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Length(Length::px(1.0)));
                assert_eq!(items[2], Value::Length(Length::px(3.0)));
            }
            other => panic!("expected a list, got {other:?}"),
        }
        // A single item is still a one-element list, so consumers never
        // have to handle two shapes.
        let single = parse_entirely_with("1px", |input| Value::parse_list(input, length_parser))
            .expect("single item parses");
        assert_eq!(single, Value::List(vec![Value::Length(Length::px(1.0))].into()));
        // One bad item invalidates the whole declaration.
        assert!(
            parse_entirely_with("1px, red", |input| Value::parse_list(input, length_parser)).is_err()
        );
    }
}
```

Mutation check: `a_wide_keyword_with_trailing_tokens_is_not_a_wide_keyword` fails if
`parse_wide` forgets to require exhaustion, which would let `inherit 4px` silently
become `inherit`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::tests`
Expected: FAIL — `error[E0432]: unresolved import 'super::Value'`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/css/value/mod.rs` (above the test module):

```rust
use std::rc::Rc;

use cssparser::Parser;

use crate::css::registry::ParseFn;

/// A parsed, unresolved property value.
///
/// Discrete keyword values all share [`Value::Keyword`], so the registry
/// can give them one interpolator and one equality rule.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `inherit` / `initial` / `unset`.
    Wide(Wide),
    /// Any discrete keyword.
    Keyword(Keyword),
    /// A bare number.
    Number(f32),
    /// A percentage, as a fraction (`100%` == `1.0`).
    Percentage(f32),
    /// A length.
    Length(Length),
    /// An angle, always in degrees.
    Angle(f32),
    /// A time.
    Time(Time),
    /// A colour.
    Color(color::ColorValue),
    /// An image.
    Image(Image),
    /// One shadow.
    Shadow(Shadow),
    /// A transform list; `none` is the empty slice.
    Transform(Rc<[TransformFn]>),
    /// A filter list; `none` is the empty slice.
    Filter(Rc<[FilterFn]>),
    /// A position (`background-position`, `transform-origin`).
    Position(Position),
    /// A background size.
    BgSize(BgSize),
    /// A repeat style.
    Repeat(RepeatStyle),
    /// A font-family list.
    FontFamilies(Rc<[FontFamily]>),
    /// A font weight.
    FontWeight(FontWeight),
    /// A font style.
    FontStyle(FontStyle),
    /// Font-variant flags.
    FontVariant(FontVariantFlags),
    /// OpenType feature settings.
    FontFeatures(Rc<[FeatureSetting]>),
    /// OpenType variation settings.
    FontVariations(Rc<[VariationSetting]>),
    /// A line height.
    LineHeight(LineHeight),
    /// Text-decoration lines.
    TextDecorationLines(TextDecorationLines),
    /// An easing function.
    Timing(TimingFunction),
    /// An animation name, and `transition-property`'s ident carrier.
    AnimationName(AnimationName),
    /// An animation iteration count.
    IterationCount(IterationCount),
    /// A `border-image-slice`.
    Slice(BorderImageSlice),
    /// A `border-image-width`, TRBL.
    BorderImageWidths([BorderImageWidthSide; 4]),
    /// An icon palette: `name colour` pairs.
    IconPalette(Rc<[(Rc<str>, color::ColorValue)]>),
    /// A pair: corner radii `(h, v)` and `border-spacing`.
    Pair(Rc<(Value, Value)>),
    /// Every comma-multiplied (`#`) property's value.
    List(Rc<[Value]>),
}

impl Value {
    /// Parse `inherit | initial | unset` as a whole value.
    pub fn parse_wide(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        let wide = Wide::from_str_ascii_ci(&name).ok_or(())?;
        input.skip_whitespace();
        if input.is_exhausted() {
            Ok(Value::Wide(wide))
        } else {
            Err(())
        }
    }

    /// Try the wide keywords first, then `f`. Every registry `ParseFn`
    /// that accepts a value goes through this.
    pub fn parse_wide_or(input: &mut Parser<'_, '_>, f: ParseFn) -> Result<Value, ()> {
        let state = input.state();
        match Value::parse_wide(input) {
            Ok(value) => Ok(value),
            Err(()) => {
                input.reset(&state);
                f(input)
            }
        }
    }

    /// Whether this is a CSS-wide keyword.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        matches!(self, Value::Wide(_))
    }

    /// Parse a comma-separated list with `f`, always producing a
    /// [`Value::List`] -- even for a single item.
    pub fn parse_list(input: &mut Parser<'_, '_>, f: ParseFn) -> Result<Value, ()> {
        let items = input
            .parse_comma_separated(|argument| match f(argument) {
                Ok(value) => Ok(value),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
        if items.is_empty() {
            return Err(());
        }
        Ok(Value::List(items.into()))
    }
}
```

The `use crate::css::registry::ParseFn;` line requires `css::registry` to exist;
add the module declaration `pub mod registry;` to `ui/src/css/mod.rs` and create
`ui/src/css/registry.rs` containing only:

```rust
//! The property registry. Filled in by Tasks 14 and 16.

use crate::css::value::Value;

/// Whole-value parser. Consumes the entire input; a trailing token is an
/// error. Never panics. `Err(())` == invalid at parse time, so the
/// declaration is dropped, CSS-style.
pub type ParseFn = fn(&mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value` — Expected: PASS, 3 new tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/mod.rs ui/src/css/value/mod.rs ui/src/css/registry.rs
git commit -m "feat(ui/css): the Value enum and wide-keyword plumbing

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 13: Interpolators

**Files:**
- Create: `ui/src/css/value/interpolate.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: `ui/src/css/value/interpolate.rs`

**Interfaces:**
- Consumes: `Value` and every value type.
- Produces: `css::value::interpolate::{discrete, number, length, percentage, color,
  pair, shadow_list, image, transform, filter_list, list, font_weight, line_height}`,
  each `fn(&Value, &Value, f32) -> Value`.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/interpolate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{color, discrete, length, list, number, shadow_list, transform};
    use crate::css::value::color::{ColorValue, Rgba};
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::shadow::Shadow;
    use crate::css::value::Value;
    use std::rc::Rc;

    fn rgba(r: f32, g: f32, b: f32, a: f32) -> Value {
        Value::Color(ColorValue::Absolute(Rgba { r, g, b, a }))
    }

    #[test]
    fn discrete_flips_at_the_halfway_point() {
        let a = Value::Keyword(Keyword::None);
        let b = Value::Keyword(Keyword::Solid);
        assert_eq!(discrete(&a, &b, 0.0), a);
        assert_eq!(discrete(&a, &b, 0.49), a);
        assert_eq!(discrete(&a, &b, 0.5), b);
        assert_eq!(discrete(&a, &b, 1.0), b);
    }

    #[test]
    fn numbers_and_same_unit_lengths_interpolate_componentwise() {
        assert_eq!(number(&Value::Number(0.0), &Value::Number(10.0), 0.25), Value::Number(2.5));
        assert_eq!(
            length(&Value::Length(Length::px(0.0)), &Value::Length(Length::px(8.0)), 0.5),
            Value::Length(Length::px(4.0))
        );
        // Mismatched units fall back to a calc sum rather than panicking.
        let mixed = length(
            &Value::Length(Length::px(0.0)),
            &Value::Length(Length::Percent(1.0)),
            0.5,
        );
        assert!(matches!(mixed, Value::Length(Length::Calc(_))));
    }

    #[test]
    fn colours_interpolate_in_premultiplied_srgb() {
        // GTK interpolates premultiplied: halfway from transparent black to
        // opaque white is 50% alpha with premultiplied-white channels.
        let midpoint = color(&rgba(0.0, 0.0, 0.0, 0.0), &rgba(1.0, 1.0, 1.0, 1.0), 0.5);
        match midpoint {
            Value::Color(ColorValue::Absolute(rgba)) => {
                assert!((rgba.a - 0.5).abs() < 1e-6, "{rgba:?}");
                assert!((rgba.r - 1.0).abs() < 1e-6, "{rgba:?}");
            }
            other => panic!("expected an absolute colour, got {other:?}"),
        }
        assert_eq!(color(&rgba(0.0, 0.0, 0.0, 1.0), &rgba(1.0, 1.0, 1.0, 1.0), 0.0), rgba(0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn shadow_lists_pad_the_shorter_side_with_transparent_shadows() {
        let one = Value::List(
            vec![Value::Shadow(Shadow {
                color: Some(ColorValue::Absolute(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 })),
                offset_x: Length::px(0.0),
                offset_y: Length::px(4.0),
                blur: Length::zero(),
                spread: Length::zero(),
                inset: false,
            })]
            .into(),
        );
        let none = Value::List(Rc::from(Vec::new()));
        let mixed = shadow_list(&none, &one, 0.5);
        match mixed {
            Value::List(items) => assert_eq!(items.len(), 1),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    #[test]
    fn a_pair_the_interpolator_cannot_handle_falls_back_to_discrete() {
        let a = Value::Number(1.0);
        let b = Value::Keyword(Keyword::None);
        assert_eq!(number(&a, &b, 0.75), b);
        assert_eq!(transform(&a, &b, 0.75), b);
        assert_eq!(list(&a, &b, 0.1), a);
    }

    #[test]
    fn interpolators_never_panic_on_mismatched_values() {
        let samples = [
            Value::Number(1.0),
            Value::Percentage(0.5),
            Value::Length(Length::Percent(0.5)),
            Value::Keyword(Keyword::None),
            Value::List(Rc::from(Vec::new())),
            rgba(0.5, 0.5, 0.5, 0.5),
        ];
        for a in &samples {
            for b in &samples {
                for t in [-1.0_f32, 0.0, 0.5, 1.0, 2.0] {
                    let _ = super::discrete(a, b, t);
                    let _ = super::number(a, b, t);
                    let _ = super::length(a, b, t);
                    let _ = super::percentage(a, b, t);
                    let _ = super::color(a, b, t);
                    let _ = super::pair(a, b, t);
                    let _ = super::shadow_list(a, b, t);
                    let _ = super::image(a, b, t);
                    let _ = super::transform(a, b, t);
                    let _ = super::filter_list(a, b, t);
                    let _ = super::list(a, b, t);
                    let _ = super::font_weight(a, b, t);
                    let _ = super::line_height(a, b, t);
                }
            }
        }
    }
}
```

Mutation check: `colours_interpolate_in_premultiplied_srgb` fails if the mix is done
unpremultiplied (the midpoint's red would be 0.5, not 1.0) — the difference GTK's
own compositing shows on a fading shadow.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::interpolate`
Expected: FAIL — `error[E0583]: file not found for module 'interpolate'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod interpolate;` to `ui/src/css/value/mod.rs` and create
`ui/src/css/value/interpolate.rs`:

```rust
//! The registry's interpolators.
//!
//! Every one takes two *computed* values of the same property. A pair an
//! interpolator cannot handle falls back to [`discrete`] rather than
//! panicking, so a hostile theme cannot crash an animation.

use std::rc::Rc;

use super::border::BgSize;
use super::color::{ColorValue, Rgba};
use super::font::{FontWeight, LineHeight};
use super::image::{Image, Position};
use super::length::Length;
use super::shadow::Shadow;
use super::transform::{TransformFn, decompose_2d, recompose_2d};
use super::Value;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `b` once `t >= 0.5`, else `a`.
#[must_use]
pub fn discrete(a: &Value, b: &Value, t: f32) -> Value {
    if t >= 0.5 { b.clone() } else { a.clone() }
}

/// Numbers, percentages and angles.
#[must_use]
pub fn number(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Value::Number(lerp(*x, *y, t)),
        (Value::Percentage(x), Value::Percentage(y)) => Value::Percentage(lerp(*x, *y, t)),
        (Value::Angle(x), Value::Angle(y)) => Value::Angle(lerp(*x, *y, t)),
        _ => discrete(a, b, t),
    }
}

/// Percentages.
#[must_use]
pub fn percentage(a: &Value, b: &Value, t: f32) -> Value {
    number(a, b, t)
}

fn lerp_length(a: &Length, b: &Length, t: f32) -> Length {
    match (a, b) {
        (Length::Abs { value: x, unit: ua }, Length::Abs { value: y, unit: ub }) if ua == ub => {
            Length::Abs { value: lerp(*x, *y, t), unit: *ua }
        }
        (Length::Percent(x), Length::Percent(y)) => Length::Percent(lerp(*x, *y, t)),
        // Unit-incompatible endpoints become a calc sum, which resolves
        // once a LengthCtx exists (CSS Values L4 §interpolation).
        _ => {
            let scaled = |length: &Length, weight: f32| {
                Rc::new(super::calc::CalcNode::Product(
                    Rc::new(super::calc::CalcNode::Length(length.clone())),
                    weight,
                ))
            };
            Length::Calc(Rc::new(super::calc::CalcNode::Sum(
                scaled(a, 1.0 - t),
                scaled(b, t),
            )))
        }
    }
}

/// Lengths.
#[must_use]
pub fn length(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Length(x), Value::Length(y)) => Value::Length(lerp_length(x, y, t)),
        _ => discrete(a, b, t),
    }
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let pa = a.premultiplied();
    let pb = b.premultiplied();
    let mut out = [0.0_f32; 4];
    for index in 0..4 {
        out[index] = lerp(pa[index], pb[index], t);
    }
    Rgba::from_premultiplied(out).clamped()
}

/// Colours, in premultiplied sRGB (as GTK does).
///
/// Only resolved colours reach here: `computed` resolves `@name` and
/// `currentColor` before an interpolator ever sees the value.
#[must_use]
pub fn color(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (
            Value::Color(ColorValue::Absolute(x)),
            Value::Color(ColorValue::Absolute(y)),
        ) => Value::Color(ColorValue::Absolute(lerp_rgba(*x, *y, t))),
        _ => discrete(a, b, t),
    }
}

/// Corner radii and `border-spacing`.
#[must_use]
pub fn pair(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Pair(x), Value::Pair(y)) => Value::Pair(Rc::new((
            length(&x.0, &y.0, t),
            length(&x.1, &y.1, t),
        ))),
        _ => length(a, b, t),
    }
}

fn transparent_shadow(model: &Shadow) -> Shadow {
    Shadow {
        color: Some(ColorValue::Absolute(Rgba::TRANSPARENT)),
        offset_x: Length::zero(),
        offset_y: Length::zero(),
        blur: Length::zero(),
        spread: Length::zero(),
        inset: model.inset,
    }
}

fn lerp_shadow(a: &Shadow, b: &Shadow, t: f32) -> Shadow {
    let color = match (&a.color, &b.color) {
        (Some(ColorValue::Absolute(x)), Some(ColorValue::Absolute(y))) => {
            Some(ColorValue::Absolute(lerp_rgba(*x, *y, t)))
        }
        _ => {
            if t >= 0.5 {
                b.color.clone()
            } else {
                a.color.clone()
            }
        }
    };
    Shadow {
        color,
        offset_x: lerp_length(&a.offset_x, &b.offset_x, t),
        offset_y: lerp_length(&a.offset_y, &b.offset_y, t),
        blur: lerp_length(&a.blur, &b.blur, t),
        spread: lerp_length(&a.spread, &b.spread, t),
        inset: if t >= 0.5 { b.inset } else { a.inset },
    }
}

/// Shadow lists, padded with transparent shadows when lengths differ.
#[must_use]
pub fn shadow_list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::List(xs), Value::List(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    let count = xs.len().max(ys.len());
    let mut out: Vec<Value> = Vec::with_capacity(count);
    for index in 0..count {
        let x = xs.get(index);
        let y = ys.get(index);
        match (x, y) {
            (Some(Value::Shadow(x)), Some(Value::Shadow(y))) => {
                out.push(Value::Shadow(lerp_shadow(x, y, t)));
            }
            (Some(Value::Shadow(x)), None) => {
                out.push(Value::Shadow(lerp_shadow(x, &transparent_shadow(x), t)));
            }
            (None, Some(Value::Shadow(y))) => {
                out.push(Value::Shadow(lerp_shadow(&transparent_shadow(y), y, t)));
            }
            _ => return discrete(a, b, t),
        }
    }
    Value::List(out.into())
}

/// Images: stop-wise when both are structurally identical gradients,
/// otherwise discrete.
#[must_use]
pub fn image(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Image(Image::Gradient(x)), Value::Image(Image::Gradient(y))) = (a, b) else {
        return discrete(a, b, t);
    };
    if x.kind != y.kind || x.repeating != y.repeating || x.stops.len() != y.stops.len() {
        return discrete(a, b, t);
    }
    let mut stops = Vec::with_capacity(x.stops.len());
    for (from, to) in x.stops.iter().zip(y.stops.iter()) {
        let color = match (&from.color, &to.color) {
            (ColorValue::Absolute(p), ColorValue::Absolute(q)) => {
                ColorValue::Absolute(lerp_rgba(*p, *q, t))
            }
            _ => return discrete(a, b, t),
        };
        let position = match (&from.position, &to.position) {
            (Some(p), Some(q)) => Some(lerp_length(p, q, t)),
            (None, None) => None,
            _ => return discrete(a, b, t),
        };
        stops.push(super::image::ColorStop { color, position, hint: from.hint.clone() });
    }
    Value::Image(Image::Gradient(Rc::new(super::image::Gradient {
        kind: x.kind.clone(),
        repeating: x.repeating,
        stops: stops.into(),
        interpolation: x.interpolation,
    })))
}

fn lerp_transform_fn(a: &TransformFn, b: &TransformFn, t: f32) -> Option<TransformFn> {
    Some(match (a, b) {
        (TransformFn::Translate(ax, ay), TransformFn::Translate(bx, by)) => {
            TransformFn::Translate(lerp_length(ax, bx, t), lerp_length(ay, by, t))
        }
        (TransformFn::TranslateX(x), TransformFn::TranslateX(y)) => {
            TransformFn::TranslateX(lerp_length(x, y, t))
        }
        (TransformFn::TranslateY(x), TransformFn::TranslateY(y)) => {
            TransformFn::TranslateY(lerp_length(x, y, t))
        }
        (TransformFn::Scale(ax, ay), TransformFn::Scale(bx, by)) => {
            TransformFn::Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t))
        }
        (TransformFn::Rotate(x), TransformFn::Rotate(y)) => TransformFn::Rotate(lerp(*x, *y, t)),
        (TransformFn::SkewX(x), TransformFn::SkewX(y)) => TransformFn::SkewX(lerp(*x, *y, t)),
        (TransformFn::SkewY(x), TransformFn::SkewY(y)) => TransformFn::SkewY(lerp(*x, *y, t)),
        (TransformFn::Skew(ax, ay), TransformFn::Skew(bx, by)) => {
            TransformFn::Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t))
        }
        _ => return None,
    })
}

/// Transform lists: component-wise when the structures match, otherwise
/// via 2-D matrix decomposition.
#[must_use]
pub fn transform(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Transform(xs), Value::Transform(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.len() == ys.len() {
        let mut out = Vec::with_capacity(xs.len());
        let mut matched = true;
        for (x, y) in xs.iter().zip(ys.iter()) {
            match lerp_transform_fn(x, y, t) {
                Some(function) => out.push(function),
                None => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            return Value::Transform(out.into());
        }
    }
    let ctx = super::length::LengthCtx::default();
    let from = super::transform::transform_list_matrix(xs, &ctx, (0.0, 0.0), (0.0, 0.0));
    let to = super::transform::transform_list_matrix(ys, &ctx, (0.0, 0.0), (0.0, 0.0));
    let (Some(from), Some(to)) = (decompose_2d(&from), decompose_2d(&to)) else {
        return discrete(a, b, t);
    };
    let blended = super::transform::Decomposed2d {
        tx: lerp(from.tx, to.tx, t),
        ty: lerp(from.ty, to.ty, t),
        rotate: lerp(from.rotate, to.rotate, t),
        scale_x: lerp(from.scale_x, to.scale_x, t),
        scale_y: lerp(from.scale_y, to.scale_y, t),
        skew_xy: lerp(from.skew_xy, to.skew_xy, t),
    };
    let matrix = recompose_2d(&blended);
    Value::Transform(
        vec![TransformFn::Matrix([
            matrix.values[0],
            matrix.values[3],
            matrix.values[1],
            matrix.values[4],
            matrix.values[2],
            matrix.values[5],
        ])]
        .into(),
    )
}

/// Filter lists: component-wise when the function sequences match.
#[must_use]
pub fn filter_list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Filter(xs), Value::Filter(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.len() != ys.len() {
        return discrete(a, b, t);
    }
    use super::filter::FilterFn;
    let mut out = Vec::with_capacity(xs.len());
    for (x, y) in xs.iter().zip(ys.iter()) {
        let blended = match (x, y) {
            (FilterFn::Blur(p), FilterFn::Blur(q)) => FilterFn::Blur(lerp_length(p, q, t)),
            (FilterFn::Brightness(p), FilterFn::Brightness(q)) => {
                FilterFn::Brightness(lerp(*p, *q, t))
            }
            (FilterFn::Contrast(p), FilterFn::Contrast(q)) => FilterFn::Contrast(lerp(*p, *q, t)),
            (FilterFn::Grayscale(p), FilterFn::Grayscale(q)) => FilterFn::Grayscale(lerp(*p, *q, t)),
            (FilterFn::HueRotate(p), FilterFn::HueRotate(q)) => FilterFn::HueRotate(lerp(*p, *q, t)),
            (FilterFn::Invert(p), FilterFn::Invert(q)) => FilterFn::Invert(lerp(*p, *q, t)),
            (FilterFn::Opacity(p), FilterFn::Opacity(q)) => FilterFn::Opacity(lerp(*p, *q, t)),
            (FilterFn::Saturate(p), FilterFn::Saturate(q)) => FilterFn::Saturate(lerp(*p, *q, t)),
            (FilterFn::Sepia(p), FilterFn::Sepia(q)) => FilterFn::Sepia(lerp(*p, *q, t)),
            (FilterFn::DropShadow(p), FilterFn::DropShadow(q)) => {
                FilterFn::DropShadow(lerp_shadow(p, q, t))
            }
            _ => return discrete(a, b, t),
        };
        out.push(blended);
    }
    Value::Filter(out.into())
}

/// Comma lists: component-wise, repeating the shorter list to the longer
/// one (CSS Backgrounds L3's list-repetition rule).
#[must_use]
pub fn list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::List(xs), Value::List(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.is_empty() || ys.is_empty() {
        return discrete(a, b, t);
    }
    let count = xs.len().max(ys.len());
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let x = &xs[index % xs.len()];
        let y = &ys[index % ys.len()];
        out.push(match (x, y) {
            (Value::Length(_), Value::Length(_)) => length(x, y, t),
            (Value::Number(_), Value::Number(_))
            | (Value::Percentage(_), Value::Percentage(_))
            | (Value::Angle(_), Value::Angle(_)) => number(x, y, t),
            (Value::Color(_), Value::Color(_)) => color(x, y, t),
            (Value::Image(_), Value::Image(_)) => image(x, y, t),
            (Value::Position(p), Value::Position(q)) => Value::Position(Position {
                x: lerp_length(&p.x, &q.x, t),
                y: lerp_length(&p.y, &q.y, t),
                z: None,
            }),
            (Value::BgSize(BgSize::Explicit(pw, ph)), Value::BgSize(BgSize::Explicit(qw, qh))) => {
                Value::BgSize(BgSize::Explicit(
                    lerp_length(pw, qw, t),
                    lerp_length(ph, qh, t),
                ))
            }
            _ => discrete(x, y, t),
        });
    }
    Value::List(out.into())
}

/// Font weights, which compute to `Absolute` before they reach here.
#[must_use]
pub fn font_weight(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::FontWeight(FontWeight::Absolute(x)), Value::FontWeight(FontWeight::Absolute(y))) => {
            Value::FontWeight(FontWeight::Absolute(lerp(*x, *y, t).clamp(1.0, 1000.0)))
        }
        _ => discrete(a, b, t),
    }
}

/// Line heights, which interpolate only within the same form.
#[must_use]
pub fn line_height(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::LineHeight(LineHeight::Number(x)), Value::LineHeight(LineHeight::Number(y))) => {
            Value::LineHeight(LineHeight::Number(lerp(*x, *y, t)))
        }
        (Value::LineHeight(LineHeight::Length(x)), Value::LineHeight(LineHeight::Length(y))) => {
            Value::LineHeight(LineHeight::Length(lerp_length(x, y, t)))
        }
        _ => discrete(a, b, t),
    }
}
```

`ColorStop`, `Gradient` and `Decomposed2d` must be `pub` in their own modules for
this file to name them; they already are.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::interpolate` — Expected: PASS, 6 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/interpolate.rs
git commit -m "feat(ui/css): the registry's interpolators

Colours interpolate premultiplied (GTK's model), shadow lists pad with
transparent shadows, gradients go stop-wise when their structures match,
and transforms fall back to 2-D decomposition. Any pair an interpolator
cannot handle degrades to discrete rather than panicking.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 14: `Prop`, the registry types, and name lookup

Discriminants are longhands first (`0..95`), then shorthands (`95..113`), each group
in GTK 4.22 reference order. `prop as usize` is both the `PROPERTIES` index and,
for longhands, the `ComputedStyle` slot.

**Files:**
- Modify: `ui/src/css/registry.rs`
- Test: `ui/src/css/registry.rs`

**Interfaces:**
- Produces:
  - `css::registry::{Prop, N_LONGHANDS, N_PROPS, ParseFn, ExpandFn, Interpolate,
    PropertyKind, PropertyDef}`
  - `registry::lookup(name: &str) -> Option<Prop>`
  - `registry::longhands() -> impl Iterator<Item = Prop>`
  - `Prop::{name, is_longhand, slot}`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{N_LONGHANDS, N_PROPS, Prop, longhands, lookup};

    #[test]
    fn the_discriminants_partition_longhands_from_shorthands() {
        assert_eq!(N_LONGHANDS, 95);
        assert_eq!(N_PROPS, 113);
        assert_eq!(Prop::Color as usize, 0);
        assert_eq!(Prop::BorderSpacing as usize, N_LONGHANDS - 1);
        assert_eq!(Prop::Font as usize, N_LONGHANDS);
        assert_eq!(Prop::Animation as usize, N_PROPS - 1);
        assert!(Prop::BorderSpacing.is_longhand());
        assert!(!Prop::Font.is_longhand());
        assert_eq!(Prop::PaddingLeft.slot(), Prop::PaddingLeft as usize);
        assert_eq!(longhands().count(), N_LONGHANDS);
        assert!(longhands().all(Prop::is_longhand));
    }

    #[test]
    fn every_property_has_a_unique_lowercase_name_that_round_trips() {
        let mut names: Vec<&'static str> = (0..N_PROPS)
            .map(|index| Prop::ALL[index].name())
            .collect();
        for name in &names {
            assert_eq!(*name, name.to_ascii_lowercase(), "{name} is not lowercase");
        }
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate property name");
        for prop in Prop::ALL {
            assert_eq!(lookup(prop.name()), Some(*prop));
        }
    }

    #[test]
    fn lookup_is_ascii_case_insensitive_and_rejects_unknown_names() {
        assert_eq!(lookup("BACKGROUND-color"), Some(Prop::BackgroundColor));
        assert_eq!(lookup("-GTK-DPI"), Some(Prop::GtkDpi));
        assert_eq!(lookup("border-radius"), Some(Prop::BorderRadius));
        assert_eq!(lookup("-gtk-outline-radius"), None);
        assert_eq!(lookup(""), None);
        assert_eq!(lookup("\u{e9}"), None);
    }

    #[test]
    fn the_gtk_only_rows_are_present_under_their_exact_spellings() {
        for name in [
            "-gtk-dpi",
            "-gtk-secondary-caret-color",
            "-gtk-icon-source",
            "-gtk-icon-size",
            "-gtk-icon-style",
            "-gtk-icon-transform",
            "-gtk-icon-palette",
            "-gtk-icon-shadow",
            "-gtk-icon-filter",
            "-gtk-icon-weight",
        ] {
            assert!(lookup(name).is_some(), "{name} is missing from the registry");
        }
    }
}
```

Mutation check: `the_discriminants_partition_longhands_from_shorthands` fails the
moment a row is inserted anywhere but at its group's end, which would silently
renumber every `ComputedStyle` slot.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::registry`
Expected: FAIL — `error[E0432]: unresolved import 'super::Prop'`.

- [ ] **Step 3: Write minimal implementation**

Replace `ui/src/css/registry.rs`'s body (keeping the existing `ParseFn` alias) with:

```rust
//! The property registry: one declarative row per GTK 4.22 CSS property.
//!
//! Nothing outside this module names a property string. `Prop`'s
//! discriminant is the `PROPERTIES` index and, for longhands, the
//! `ComputedStyle` slot, so the enum's declaration order is load-bearing:
//! rows may only be appended to the end of their own group.

use crate::css::value::Value;

/// Whole-value parser. Consumes the entire input; a trailing token is an
/// error. Never panics. `Err(())` == invalid at parse time, so the
/// declaration is dropped, CSS-style.
pub type ParseFn = fn(&mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;

/// Expands a shorthand, emitting one `(longhand, Value)` per longhand it
/// sets. MUST emit every longhand in `PropertyKind::Shorthand::longhands`:
/// omitted components are emitted at their initial value (CSS's shorthand
/// reset rule).
pub type ExpandFn =
    fn(&mut cssparser::Parser<'_, '_>, &mut dyn FnMut(Prop, Value)) -> Result<(), ()>;

/// Interpolate two *computed* values of the same property at progress `t`
/// (usually `0..=1`; a cubic-bezier may overshoot).
pub type Interpolate = fn(&Value, &Value, f32) -> Value;

/// What a registry row is.
pub enum PropertyKind {
    /// A property with its own computed slot.
    Longhand {
        /// Whole-value parser.
        parse: ParseFn,
        /// Initial value constructor.
        initial: fn() -> Value,
        /// Whether the property inherits.
        inherited: bool,
        /// `Some` for animatable properties.
        animatable: Option<Interpolate>,
    },
    /// A property that only sets other properties.
    Shorthand {
        /// Expansion function.
        expand: ExpandFn,
        /// The longhands it resets, in reset order.
        longhands: &'static [Prop],
    },
}

/// One registry row.
pub struct PropertyDef {
    /// The property's CSS name, ASCII-lowercase.
    pub name: &'static str,
    /// Longhand or shorthand.
    pub kind: PropertyKind,
}

/// Declares `Prop`, `Prop::ALL` and the name table from one list, so a
/// discriminant and its name can never drift apart.
macro_rules! props {
    (longhands { $( $lh:ident => $lname:literal ),+ $(,)? }
     shorthands { $( $sh:ident => $sname:literal ),+ $(,)? }) => {
        /// Every property in the GTK 4.22 CSS reference.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum Prop { $( $lh, )+ $( $sh, )+ }

        /// Number of longhands; also the `ComputedStyle` slot count.
        pub const N_LONGHANDS: usize = [$( stringify!($lh) ),+].len();
        /// Number of rows in [`PROPERTIES`].
        pub const N_PROPS: usize = N_LONGHANDS + [$( stringify!($sh) ),+].len();

        impl Prop {
            /// Every property, in registry order.
            pub const ALL: &'static [Prop] = &[ $( Prop::$lh, )+ $( Prop::$sh, )+ ];

            /// The property's CSS name.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self { $( Prop::$lh => $lname, )+ $( Prop::$sh => $sname, )+ }
            }
        }
    };
}

props! {
    longhands {
        // colours & effects
        Color => "color", Opacity => "opacity", Filter => "filter",
        // fonts
        FontFamily => "font-family", FontSize => "font-size",
        FontStyle => "font-style", FontVariant => "font-variant",
        FontWeight => "font-weight", FontWidth => "font-width",
        FontStretch => "font-stretch", FontKerning => "font-kerning",
        FontVariantLigatures => "font-variant-ligatures",
        FontVariantPosition => "font-variant-position",
        FontVariantCaps => "font-variant-caps",
        FontVariantNumeric => "font-variant-numeric",
        FontVariantAlternates => "font-variant-alternates",
        FontVariantEastAsian => "font-variant-east-asian",
        FontFeatureSettings => "font-feature-settings",
        FontVariationSettings => "font-variation-settings",
        GtkDpi => "-gtk-dpi",
        // text
        CaretColor => "caret-color",
        GtkSecondaryCaretColor => "-gtk-secondary-caret-color",
        LetterSpacing => "letter-spacing", TextTransform => "text-transform",
        LineHeight => "line-height",
        TextDecorationLine => "text-decoration-line",
        TextDecorationColor => "text-decoration-color",
        TextDecorationStyle => "text-decoration-style",
        TextShadow => "text-shadow",
        // icons (parsed + stored; drawn in M4)
        GtkIconSource => "-gtk-icon-source", GtkIconSize => "-gtk-icon-size",
        GtkIconStyle => "-gtk-icon-style", GtkIconTransform => "-gtk-icon-transform",
        GtkIconPalette => "-gtk-icon-palette", GtkIconShadow => "-gtk-icon-shadow",
        GtkIconFilter => "-gtk-icon-filter", GtkIconWeight => "-gtk-icon-weight",
        // transform
        Transform => "transform", TransformOrigin => "transform-origin",
        // box model
        MinWidth => "min-width", MinHeight => "min-height",
        MarginTop => "margin-top", MarginRight => "margin-right",
        MarginBottom => "margin-bottom", MarginLeft => "margin-left",
        PaddingTop => "padding-top", PaddingRight => "padding-right",
        PaddingBottom => "padding-bottom", PaddingLeft => "padding-left",
        // borders
        BorderTopWidth => "border-top-width", BorderRightWidth => "border-right-width",
        BorderBottomWidth => "border-bottom-width", BorderLeftWidth => "border-left-width",
        BorderTopStyle => "border-top-style", BorderRightStyle => "border-right-style",
        BorderBottomStyle => "border-bottom-style", BorderLeftStyle => "border-left-style",
        BorderTopLeftRadius => "border-top-left-radius",
        BorderTopRightRadius => "border-top-right-radius",
        BorderBottomRightRadius => "border-bottom-right-radius",
        BorderBottomLeftRadius => "border-bottom-left-radius",
        BorderTopColor => "border-top-color", BorderRightColor => "border-right-color",
        BorderBottomColor => "border-bottom-color", BorderLeftColor => "border-left-color",
        BorderImageSource => "border-image-source", BorderImageRepeat => "border-image-repeat",
        BorderImageSlice => "border-image-slice", BorderImageWidth => "border-image-width",
        // outline
        OutlineStyle => "outline-style", OutlineWidth => "outline-width",
        OutlineColor => "outline-color", OutlineOffset => "outline-offset",
        // backgrounds
        BackgroundColor => "background-color", BackgroundClip => "background-clip",
        BackgroundOrigin => "background-origin", BackgroundSize => "background-size",
        BackgroundPosition => "background-position", BackgroundRepeat => "background-repeat",
        BackgroundImage => "background-image", BoxShadow => "box-shadow",
        BackgroundBlendMode => "background-blend-mode",
        // transitions
        TransitionProperty => "transition-property",
        TransitionDuration => "transition-duration",
        TransitionTimingFunction => "transition-timing-function",
        TransitionDelay => "transition-delay",
        // animations
        AnimationName => "animation-name", AnimationDuration => "animation-duration",
        AnimationTimingFunction => "animation-timing-function",
        AnimationIterationCount => "animation-iteration-count",
        AnimationDirection => "animation-direction",
        AnimationPlayState => "animation-play-state",
        AnimationDelay => "animation-delay", AnimationFillMode => "animation-fill-mode",
        // misc
        BorderSpacing => "border-spacing",
    }
    shorthands {
        Font => "font", TextDecoration => "text-decoration",
        Margin => "margin", Padding => "padding",
        BorderWidth => "border-width", BorderStyle => "border-style",
        BorderColor => "border-color",
        BorderTop => "border-top", BorderRight => "border-right",
        BorderBottom => "border-bottom", BorderLeft => "border-left",
        Border => "border", BorderRadius => "border-radius",
        BorderImage => "border-image",
        Outline => "outline", Background => "background",
        Transition => "transition", Animation => "animation",
    }
}

impl Prop {
    /// Whether this row has its own computed slot.
    #[must_use]
    pub fn is_longhand(self) -> bool {
        (self as usize) < N_LONGHANDS
    }

    /// The `ComputedStyle` slot index. Longhands only.
    #[must_use]
    pub fn slot(self) -> usize {
        debug_assert!(self.is_longhand(), "{} is a shorthand", self.name());
        self as usize
    }
}

/// ASCII-case-insensitive property lookup; `None` for an unknown name.
#[must_use]
pub fn lookup(name: &str) -> Option<Prop> {
    if !name.is_ascii() {
        return None;
    }
    Prop::ALL
        .iter()
        .copied()
        .find(|prop| name.eq_ignore_ascii_case(prop.name()))
}

/// All longhands, in registry order -- the `ComputedStyle` slot order.
pub fn longhands() -> impl Iterator<Item = Prop> {
    Prop::ALL.iter().copied().take(N_LONGHANDS)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::registry` — Expected: PASS, 4 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/registry.rs
git commit -m "feat(ui/css): the Prop enum and registry types

113 rows -- 95 longhands then 18 shorthands -- in GTK 4.22 reference
order, generated from one table so a discriminant and its CSS name cannot
drift. prop as usize is both the PROPERTIES index and the ComputedStyle
slot, so declaration order is load-bearing.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 15: Shorthand expansion

M1's nine `css/shorthand.rs` tests move here, re-expressed against `(Prop, Value)`.
Their inputs and expected side values are preserved; the two behavioural changes are
called out in the tests themselves.

**Files:**
- Create: `ui/src/css/value/shorthand.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: `ui/src/css/value/shorthand.rs`

**Interfaces:**
- Consumes: `Prop`, `Value`, every value parser.
- Produces (all `ExpandFn`-shaped,
  `fn(&mut Parser<'_, '_>, &mut dyn FnMut(Prop, Value)) -> Result<(), ()>`):
  `shorthand::{expand_font, expand_text_decoration, expand_margin, expand_padding,
  expand_border_width, expand_border_style, expand_border_color, expand_border_top,
  expand_border_right, expand_border_bottom, expand_border_left, expand_border,
  expand_border_radius, expand_border_image, expand_outline, expand_background,
  expand_transition, expand_animation}`.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/shorthand.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::registry::{ExpandFn, Prop};
    use crate::css::value::color::ColorValue;
    use crate::css::value::image::Image;
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::Value;

    fn expanded(expand: ExpandFn, text: &str) -> Result<Vec<(Prop, Value)>, ()> {
        let mut source = cssparser::ParserInput::new(text);
        let mut parser = cssparser::Parser::new(&mut source);
        let mut out: Vec<(Prop, Value)> = Vec::new();
        {
            let mut sink = |prop: Prop, value: Value| out.push((prop, value));
            expand(&mut parser, &mut sink)?;
        }
        parser.skip_whitespace();
        if parser.is_exhausted() { Ok(out) } else { Err(()) }
    }

    fn get(pairs: &[(Prop, Value)], prop: Prop) -> Option<&Value> {
        pairs.iter().find(|(key, _)| *key == prop).map(|(_, value)| value)
    }

    fn px(value: f32) -> Value {
        Value::Length(Length::px(value))
    }

    #[test]
    fn padding_follows_the_one_to_four_value_rule() {
        let one = expanded(expand_padding, "4px").expect("expands");
        assert_eq!(get(&one, Prop::PaddingLeft), Some(&px(4.0)));
        let two = expanded(expand_padding, "4px 9px").expect("expands");
        assert_eq!(get(&two, Prop::PaddingTop), Some(&px(4.0)));
        assert_eq!(get(&two, Prop::PaddingRight), Some(&px(9.0)));
        assert_eq!(get(&two, Prop::PaddingBottom), Some(&px(4.0)));
        assert_eq!(get(&two, Prop::PaddingLeft), Some(&px(9.0)));
        let three = expanded(expand_padding, "1px 2px 3px").expect("expands");
        assert_eq!(get(&three, Prop::PaddingBottom), Some(&px(3.0)));
        assert_eq!(get(&three, Prop::PaddingLeft), Some(&px(2.0)));
    }

    #[test]
    fn border_splits_function_valued_colours_intact() {
        let pairs = expanded(expand_border, "1px solid rgb(0 0 0)").expect("expands");
        assert_eq!(get(&pairs, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(get(&pairs, Prop::BorderTopStyle), Some(&Value::Keyword(Keyword::Solid)));
        assert!(matches!(
            get(&pairs, Prop::BorderLeftColor),
            Some(Value::Color(ColorValue::Absolute(_)))
        ));
    }

    #[test]
    fn border_resets_the_components_it_omits() {
        // The reset values are now the registry's initial values: `medium`
        // is 3px and the omitted colour is currentColor.
        let pairs = expanded(expand_border, "none").expect("expands");
        assert_eq!(get(&pairs, Prop::BorderTopStyle), Some(&Value::Keyword(Keyword::None)));
        assert_eq!(get(&pairs, Prop::BorderTopWidth), Some(&px(3.0)));
        assert_eq!(get(&pairs, Prop::BorderTopColor), Some(&Value::Color(ColorValue::CurrentColor)));
        let adwaita = expanded(expand_border, "1px solid").expect("expands");
        assert_eq!(get(&adwaita, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(get(&adwaita, Prop::BorderTopColor), Some(&Value::Color(ColorValue::CurrentColor)));
    }

    #[test]
    fn a_single_side_border_touches_only_that_side() {
        let pairs = expanded(expand_border_bottom, "2px dashed red").expect("expands");
        assert_eq!(get(&pairs, Prop::BorderBottomWidth), Some(&px(2.0)));
        assert_eq!(get(&pairs, Prop::BorderTopWidth), None);
    }

    #[test]
    fn background_separates_colour_from_image() {
        let flat = expanded(expand_background, "#112233").expect("expands");
        assert!(matches!(get(&flat, Prop::BackgroundColor), Some(Value::Color(_))));
        assert_eq!(
            get(&flat, Prop::BackgroundImage),
            Some(&Value::List(vec![Value::Image(Image::None)].into()))
        );

        let image = expanded(expand_background, "image(#e8e6e3)").expect("expands");
        assert!(matches!(
            get(&image, Prop::BackgroundImage),
            Some(Value::List(items)) if matches!(items[0], Value::Image(Image::Solid(_)))
        ));
        assert_eq!(
            get(&image, Prop::BackgroundColor),
            Some(&Value::Color(ColorValue::Absolute(
                crate::css::value::color::Rgba::TRANSPARENT
            )))
        );

        let both = expanded(
            expand_background,
            "#dfdcd8 linear-gradient(to top, #dad6d2, #e1dedb)",
        )
        .expect("expands");
        assert!(matches!(get(&both, Prop::BackgroundColor), Some(Value::Color(_))));
        assert!(matches!(
            get(&both, Prop::BackgroundImage),
            Some(Value::List(items)) if matches!(items[0], Value::Image(Image::Gradient(_)))
        ));

        let none = expanded(expand_background, "none").expect("expands");
        assert_eq!(
            get(&none, Prop::BackgroundImage),
            Some(&Value::List(vec![Value::Image(Image::None)].into()))
        );

        // `no-repeat` is a keyword, not a named colour.
        let keyworded = expanded(expand_background, "image(#f6f5f4) no-repeat").expect("expands");
        assert_eq!(
            get(&keyworded, Prop::BackgroundColor),
            Some(&Value::Color(ColorValue::Absolute(
                crate::css::value::color::Rgba::TRANSPARENT
            )))
        );
    }

    #[test]
    fn a_longhand_has_no_expansion() {
        // M1's "a longhand passes straight through" rule: in M2 a longhand
        // simply is not a shorthand.
        assert!(Prop::MinHeight.is_longhand());
        assert!(Prop::BorderRadius.is_longhand() == false);
    }

    #[test]
    fn an_unusable_box_shorthand_is_invalid() {
        // M1 kept the unexpandable value under its own name so the
        // runner-up rule could see it lose. Decision 6 retires that rule:
        // a shorthand that cannot expand is a parse error and the whole
        // declaration is dropped.
        assert!(expanded(expand_padding, "1px 2px 3px 4px 5px").is_err());
    }

    #[test]
    fn line_width_keywords_are_widths_not_colours() {
        // Review round 1: `thin`/`medium`/`thick` are bare identifiers, so
        // the colour test claimed them first.
        let thin = expanded(expand_border, "thin solid").expect("expands");
        assert_eq!(get(&thin, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(get(&thin, Prop::BorderTopColor), Some(&Value::Color(ColorValue::CurrentColor)));
        let thick = expanded(expand_border, "thick dotted red").expect("expands");
        assert_eq!(get(&thick, Prop::BorderTopWidth), Some(&px(5.0)));
        assert_eq!(get(&thick, Prop::BorderTopStyle), Some(&Value::Keyword(Keyword::Dotted)));
        assert!(matches!(get(&thick, Prop::BorderTopColor), Some(Value::Color(_))));
        let medium = expanded(expand_border, "medium solid").expect("expands");
        assert_eq!(get(&medium, Prop::BorderTopWidth), Some(&px(3.0)));
    }

    #[test]
    fn a_colour_in_a_non_final_background_layer_is_still_found() {
        // Review round 1: Adwaita:1359 puts the colour in the *first* layer.
        // GTK accepts it; CSS does not. One colour anywhere is unambiguous.
        // A layer that is only a colour contributes no image layer, so the
        // first *image* is still the one painted on top.
        let junction = expanded(
            expand_background,
            "#cdc7c2, linear-gradient(to bottom, transparent 1px, #cecece 1px), linear-gradient(to left, transparent 1px, #cecece 1px)",
        )
        .expect("expands");
        assert!(matches!(get(&junction, Prop::BackgroundColor), Some(Value::Color(_))));
        match get(&junction, Prop::BackgroundImage) {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), 2, "colour-only layers contribute no image");
                assert!(matches!(items[0], Value::Image(Image::Gradient(_))));
            }
            other => panic!("expected an image list, got {other:?}"),
        }
        // Two candidate colours are ambiguous: only the final layer's counts.
        let ambiguous = expanded(expand_background, "red, blue").expect("expands");
        assert!(matches!(get(&ambiguous, Prop::BackgroundColor), Some(Value::Color(_))));
    }

    #[test]
    fn the_remaining_shorthands_expand_their_longhands() {
        let radius = expanded(expand_border_radius, "5px").expect("expands");
        assert_eq!(get(&radius, Prop::BorderTopLeftRadius),
                   Some(&Value::Pair(std::rc::Rc::new((px(5.0), px(5.0))))));
        let elliptical = expanded(expand_border_radius, "10px / 20px").expect("expands");
        assert_eq!(get(&elliptical, Prop::BorderTopLeftRadius),
                   Some(&Value::Pair(std::rc::Rc::new((px(10.0), px(20.0))))));
        let transition = expanded(expand_transition, "background-color 200ms ease-in").expect("expands");
        assert!(get(&transition, Prop::TransitionProperty).is_some());
        assert!(get(&transition, Prop::TransitionDuration).is_some());
        assert!(get(&transition, Prop::TransitionDelay).is_some());
        let animation = expanded(expand_animation, "spin 1s linear infinite").expect("expands");
        assert!(get(&animation, Prop::AnimationName).is_some());
        assert!(get(&animation, Prop::AnimationFillMode).is_some());
        let font = expanded(expand_font, "italic bold 14px/1.5 Cantarell").expect("expands");
        assert_eq!(get(&font, Prop::FontSize), Some(&px(14.0)));
        assert!(get(&font, Prop::FontFamily).is_some());
        let decoration = expanded(expand_text_decoration, "underline wavy red").expect("expands");
        assert!(get(&decoration, Prop::TextDecorationLine).is_some());
        assert_eq!(get(&decoration, Prop::TextDecorationStyle), Some(&Value::Keyword(Keyword::Wavy)));
        let outline = expanded(expand_outline, "2px solid red").expect("expands");
        assert_eq!(get(&outline, Prop::OutlineWidth), Some(&px(2.0)));
        let image = expanded(expand_border_image, "url(\"b.png\") 30% / 2px round").expect("expands");
        assert!(get(&image, Prop::BorderImageSource).is_some());
        assert!(get(&image, Prop::BorderImageRepeat).is_some());
    }

    #[test]
    fn shorthand_expansion_never_panics() {
        const EXPANDERS: &[ExpandFn] = &[
            expand_font, expand_text_decoration, expand_margin, expand_padding,
            expand_border_width, expand_border_style, expand_border_color,
            expand_border_top, expand_border_right, expand_border_bottom,
            expand_border_left, expand_border, expand_border_radius,
            expand_border_image, expand_outline, expand_background,
            expand_transition, expand_animation,
        ];
        for input in crate::css::value::FUZZ_INPUTS {
            for expand in EXPANDERS {
                let _ = expanded(*expand, input);
            }
        }
    }
}
```

Mutation check: `border_resets_the_components_it_omits` is the M1 regression that
made `border: none` leave a stale colour behind; dropping any reset emission fails
it. `a_colour_in_a_non_final_background_layer_is_still_found` fails if GTK's lax
colour-anywhere rule is replaced with CSS's strict last-layer-only rule.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::shorthand`
Expected: FAIL — `error[E0583]: file not found for module 'shorthand'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod shorthand;` to `ui/src/css/value/mod.rs` and create
`ui/src/css/value/shorthand.rs`:

```rust
//! Shorthand expansion.
//!
//! Every expander emits a value for **every** longhand its registry row
//! lists: an omitted component is emitted at its initial value, which is
//! CSS's shorthand reset rule and the reason `border: none` clears a
//! colour an earlier rule set.

use std::rc::Rc;

use cssparser::{Parser, Token};

use crate::css::registry::Prop;

use super::border::{
    BgSize, BorderImageSlice, BorderImageWidthSide, RepeatStyle, four_sides, parse_line_style,
    parse_line_width,
};
use super::color::{ColorValue, Rgba};
use super::font::{FontStyle, FontVariantFlags, FontWeight, LineHeight, parse_family_list,
                  parse_font_size, parse_stretch};
use super::image::{Image, Position};
use super::keyword::Keyword;
use super::length::Length;
use super::text::parse_decoration_lines;
use super::timing::{AnimationName, IterationCount, Time, TimingFunction};
use super::Value;

type Sink<'a> = &'a mut dyn FnMut(Prop, Value);

fn require_exhausted(input: &mut Parser<'_, '_>) -> Result<(), ()> {
    input.skip_whitespace();
    if input.is_exhausted() { Ok(()) } else { Err(()) }
}

/// Collect 1..=4 values with `parse`, then apply the four-sides rule.
fn box_sides<T: Clone>(
    input: &mut Parser<'_, '_>,
    parse: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<[T; 4], ()> {
    let mut values: Vec<T> = Vec::new();
    while values.len() < 5 {
        let state = input.state();
        match parse(input) {
            Ok(value) => values.push(value),
            Err(()) => {
                input.reset(&state);
                break;
            }
        }
    }
    four_sides(values).ok_or(())
}

/// `margin`.
pub fn expand_margin(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, Length::parse_allowing_auto)?;
    require_exhausted(input)?;
    for (prop, value) in [Prop::MarginTop, Prop::MarginRight, Prop::MarginBottom, Prop::MarginLeft]
        .into_iter()
        .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `padding`.
pub fn expand_padding(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, Length::parse)?;
    require_exhausted(input)?;
    for (prop, value) in
        [Prop::PaddingTop, Prop::PaddingRight, Prop::PaddingBottom, Prop::PaddingLeft]
            .into_iter()
            .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `border-width`.
pub fn expand_border_width(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, parse_line_width)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopWidth,
        Prop::BorderRightWidth,
        Prop::BorderBottomWidth,
        Prop::BorderLeftWidth,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `border-style`.
pub fn expand_border_style(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, parse_line_style)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopStyle,
        Prop::BorderRightStyle,
        Prop::BorderBottomStyle,
        Prop::BorderLeftStyle,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Keyword(value));
    }
    Ok(())
}

/// `border-color`.
pub fn expand_border_color(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, ColorValue::parse)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopColor,
        Prop::BorderRightColor,
        Prop::BorderBottomColor,
        Prop::BorderLeftColor,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Color(value));
    }
    Ok(())
}

/// The shared `<line-width> || <line-style> || <color>` body.
fn border_side_components(
    input: &mut Parser<'_, '_>,
) -> Result<(Length, Keyword, ColorValue), ()> {
    let mut width: Option<Length> = None;
    let mut style: Option<Keyword> = None;
    let mut color: Option<ColorValue> = None;
    loop {
        let state = input.state();
        if style.is_none() {
            if let Ok(parsed) = parse_line_style(input) {
                style = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if width.is_none() {
            if let Ok(parsed) = parse_line_width(input) {
                width = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if width.is_none() && style.is_none() && color.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    Ok((
        width.unwrap_or_else(|| Length::px(3.0)),
        style.unwrap_or(Keyword::None),
        color.unwrap_or(ColorValue::CurrentColor),
    ))
}

fn emit_border_side(
    sink: Sink<'_>,
    props: (Prop, Prop, Prop),
    components: &(Length, Keyword, ColorValue),
) {
    sink(props.0, Value::Length(components.0.clone()));
    sink(props.1, Value::Keyword(components.1));
    sink(props.2, Value::Color(components.2.clone()));
}

/// `border-top`.
pub fn expand_border_top(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (Prop::BorderTopWidth, Prop::BorderTopStyle, Prop::BorderTopColor),
        &components,
    );
    Ok(())
}

/// `border-right`.
pub fn expand_border_right(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (Prop::BorderRightWidth, Prop::BorderRightStyle, Prop::BorderRightColor),
        &components,
    );
    Ok(())
}

/// `border-bottom`.
pub fn expand_border_bottom(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (Prop::BorderBottomWidth, Prop::BorderBottomStyle, Prop::BorderBottomColor),
        &components,
    );
    Ok(())
}

/// `border-left`.
pub fn expand_border_left(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (Prop::BorderLeftWidth, Prop::BorderLeftStyle, Prop::BorderLeftColor),
        &components,
    );
    Ok(())
}

/// `border` -- all four sides' width, style and colour.
pub fn expand_border(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    for props in [
        (Prop::BorderTopWidth, Prop::BorderTopStyle, Prop::BorderTopColor),
        (Prop::BorderRightWidth, Prop::BorderRightStyle, Prop::BorderRightColor),
        (Prop::BorderBottomWidth, Prop::BorderBottomStyle, Prop::BorderBottomColor),
        (Prop::BorderLeftWidth, Prop::BorderLeftStyle, Prop::BorderLeftColor),
    ] {
        emit_border_side(sink, props, &components);
    }
    Ok(())
}

/// `border-radius`, including the `/` elliptical form.
pub fn expand_border_radius(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let horizontal = box_sides(input, Length::parse)?;
    let vertical = if input.expect_delim('/').is_ok() {
        box_sides(input, Length::parse)?
    } else {
        horizontal.clone()
    };
    require_exhausted(input)?;
    for (index, prop) in [
        Prop::BorderTopLeftRadius,
        Prop::BorderTopRightRadius,
        Prop::BorderBottomRightRadius,
        Prop::BorderBottomLeftRadius,
    ]
    .into_iter()
    .enumerate()
    {
        sink(
            prop,
            Value::Pair(Rc::new((
                Value::Length(horizontal[index].clone()),
                Value::Length(vertical[index].clone()),
            ))),
        );
    }
    Ok(())
}

/// `border-image`.
pub fn expand_border_image(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let mut source: Option<Image> = None;
    let mut slice: Option<BorderImageSlice> = None;
    let mut widths: Option<[BorderImageWidthSide; 4]> = None;
    let mut repeat: Option<RepeatStyle> = None;
    loop {
        let state = input.state();
        if source.is_none() {
            if let Ok(parsed) = Image::parse(input) {
                source = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if slice.is_none() {
            if let Ok(parsed) = BorderImageSlice::parse(input) {
                slice = Some(parsed);
                if input.expect_delim('/').is_ok() {
                    widths = Some(box_sides(input, BorderImageWidthSide::parse)?);
                }
                continue;
            }
            input.reset(&state);
        }
        if repeat.is_none() {
            if let Ok(parsed) = RepeatStyle::parse(input) {
                repeat = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if source.is_none() && slice.is_none() && repeat.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    sink(Prop::BorderImageSource, Value::Image(source.unwrap_or(Image::None)));
    sink(
        Prop::BorderImageSlice,
        Value::Slice(slice.unwrap_or(BorderImageSlice {
            sides: [super::border::NumberOrPercent::Percent(1.0); 4],
            fill: false,
        })),
    );
    sink(
        Prop::BorderImageWidth,
        Value::BorderImageWidths(widths.unwrap_or([
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
        ])),
    );
    sink(
        Prop::BorderImageRepeat,
        Value::Repeat(repeat.unwrap_or(RepeatStyle { x: Keyword::Stretch, y: Keyword::Stretch })),
    );
    Ok(())
}

/// `outline` -- style, width and colour, but never `outline-offset`.
pub fn expand_outline(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let (width, style, color) = border_side_components(input)?;
    sink(Prop::OutlineWidth, Value::Length(width));
    sink(Prop::OutlineStyle, Value::Keyword(style));
    sink(Prop::OutlineColor, Value::Color(color));
    Ok(())
}

/// One `background` layer.
struct BackgroundLayerParts {
    color: Option<ColorValue>,
    image: Option<Image>,
    position: Option<Position>,
    size: Option<BgSize>,
    repeat: Option<RepeatStyle>,
    origin: Option<Keyword>,
    clip: Option<Keyword>,
}

fn parse_box_keyword(input: &mut Parser<'_, '_>) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    for keyword in [Keyword::BorderBox, Keyword::PaddingBox, Keyword::ContentBox, Keyword::TextBox]
    {
        if name.eq_ignore_ascii_case(keyword.as_str()) {
            return Ok(keyword);
        }
    }
    Err(())
}

fn parse_background_layer(input: &mut Parser<'_, '_>) -> Result<BackgroundLayerParts, ()> {
    let mut parts = BackgroundLayerParts {
        color: None,
        image: None,
        position: None,
        size: None,
        repeat: None,
        origin: None,
        clip: None,
    };
    let mut seen = false;
    loop {
        let state = input.state();
        if parts.image.is_none() {
            if let Ok(parsed) = Image::parse(input) {
                parts.image = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.repeat.is_none() {
            if let Ok(parsed) = RepeatStyle::parse_background(input) {
                parts.repeat = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.origin.is_none() || parts.clip.is_none() {
            if let Ok(parsed) = parse_box_keyword(input) {
                if parts.origin.is_none() {
                    parts.origin = Some(parsed);
                } else {
                    parts.clip = Some(parsed);
                }
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.position.is_none() {
            if let Ok(parsed) = Position::parse(input) {
                parts.position = Some(parsed);
                seen = true;
                if input.expect_delim('/').is_ok() {
                    parts.size = Some(BgSize::parse(input)?);
                }
                continue;
            }
            input.reset(&state);
        }
        if parts.color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                parts.color = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if seen { Ok(parts) } else { Err(()) }
}

/// `background`.
///
/// GTK is lax about which layer carries the colour (Adwaita:1359 puts it
/// first), so a colour anywhere is accepted and the *last* one wins. A
/// layer that carries only a colour contributes no image layer, which is
/// what keeps "the first layer is the one painted on top" true.
pub fn expand_background(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_background_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut color: Option<ColorValue> = None;
    let mut images: Vec<Value> = Vec::new();
    let mut positions: Vec<Value> = Vec::new();
    let mut sizes: Vec<Value> = Vec::new();
    let mut repeats: Vec<Value> = Vec::new();
    let mut origins: Vec<Value> = Vec::new();
    let mut clips: Vec<Value> = Vec::new();
    for layer in &layers {
        if let Some(parsed) = &layer.color {
            color = Some(parsed.clone());
        }
        let Some(image) = &layer.image else {
            continue;
        };
        images.push(Value::Image(image.clone()));
        positions.push(Value::Position(
            layer.position.clone().unwrap_or(Position {
                x: Length::Percent(0.0),
                y: Length::Percent(0.0),
                z: None,
            }),
        ));
        sizes.push(Value::BgSize(layer.size.clone().unwrap_or(BgSize::Auto)));
        repeats.push(Value::Repeat(layer.repeat.unwrap_or(RepeatStyle {
            x: Keyword::Repeat,
            y: Keyword::Repeat,
        })));
        let origin = layer.origin.unwrap_or(Keyword::PaddingBox);
        origins.push(Value::Keyword(origin));
        clips.push(Value::Keyword(layer.clip.unwrap_or(Keyword::BorderBox)));
    }
    if images.is_empty() {
        images.push(Value::Image(Image::None));
        positions.push(Value::Position(Position {
            x: Length::Percent(0.0),
            y: Length::Percent(0.0),
            z: None,
        }));
        sizes.push(Value::BgSize(BgSize::Auto));
        repeats.push(Value::Repeat(RepeatStyle { x: Keyword::Repeat, y: Keyword::Repeat }));
        origins.push(Value::Keyword(Keyword::PaddingBox));
        clips.push(Value::Keyword(Keyword::BorderBox));
    }
    sink(
        Prop::BackgroundColor,
        Value::Color(color.unwrap_or(ColorValue::Absolute(Rgba::TRANSPARENT))),
    );
    sink(Prop::BackgroundImage, Value::List(images.into()));
    sink(Prop::BackgroundPosition, Value::List(positions.into()));
    sink(Prop::BackgroundSize, Value::List(sizes.into()));
    sink(Prop::BackgroundRepeat, Value::List(repeats.into()));
    sink(Prop::BackgroundOrigin, Value::List(origins.into()));
    sink(Prop::BackgroundClip, Value::List(clips.into()));
    Ok(())
}

/// `text-decoration`.
pub fn expand_text_decoration(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    const STYLES: &[Keyword] = &[
        Keyword::Solid,
        Keyword::Double,
        Keyword::Dotted,
        Keyword::Dashed,
        Keyword::Wavy,
    ];
    let mut lines = None;
    let mut style = None;
    let mut color = None;
    loop {
        let state = input.state();
        if style.is_none() {
            if let Ok(name) = input.expect_ident() {
                let name = name.as_ref().to_string();
                if let Some(found) = STYLES
                    .iter()
                    .copied()
                    .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
                {
                    style = Some(found);
                    continue;
                }
            }
            input.reset(&state);
        }
        if lines.is_none() {
            if let Ok(parsed) = parse_decoration_lines(input) {
                lines = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if lines.is_none() && style.is_none() && color.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    sink(
        Prop::TextDecorationLine,
        Value::TextDecorationLines(lines.unwrap_or_default()),
    );
    sink(Prop::TextDecorationStyle, Value::Keyword(style.unwrap_or(Keyword::Solid)));
    sink(
        Prop::TextDecorationColor,
        Value::Color(color.unwrap_or(ColorValue::CurrentColor)),
    );
    Ok(())
}

/// `font` -- GTK's form also carries `/ <line-height>`.
pub fn expand_font(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let mut style = None;
    let mut weight = None;
    let mut variant = None;
    let mut stretch = None;
    loop {
        let state = input.state();
        if style.is_none() {
            if let Ok(parsed) = FontStyle::parse(input) {
                if parsed != FontStyle::Normal {
                    style = Some(parsed);
                    continue;
                }
            }
            input.reset(&state);
        }
        if weight.is_none() {
            if let Ok(parsed) = FontWeight::parse(input) {
                if parsed != FontWeight::Absolute(400.0) {
                    weight = Some(parsed);
                    continue;
                }
            }
            input.reset(&state);
        }
        if variant.is_none() {
            if let Ok(parsed) =
                super::font::parse_variant_flags(input, FontVariantFlags::SMALL_CAPS)
            {
                if !parsed.is_empty() {
                    variant = Some(parsed);
                    continue;
                }
            }
            input.reset(&state);
        }
        if stretch.is_none() {
            if let Ok(parsed) = parse_stretch(input) {
                if parsed != 100.0 {
                    stretch = Some(parsed);
                    continue;
                }
            }
            input.reset(&state);
        }
        break;
    }
    let size = parse_font_size(input)?;
    let line_height = if input.expect_delim('/').is_ok() {
        Some(LineHeight::parse(input)?)
    } else {
        None
    };
    let families = parse_family_list(input)?;
    require_exhausted(input)?;
    sink(Prop::FontStyle, Value::FontStyle(style.unwrap_or(FontStyle::Normal)));
    sink(
        Prop::FontVariant,
        Value::FontVariant(variant.unwrap_or_default()),
    );
    sink(
        Prop::FontWeight,
        Value::FontWeight(weight.unwrap_or(FontWeight::Absolute(400.0))),
    );
    sink(Prop::FontStretch, Value::Percentage(stretch.unwrap_or(100.0) / 100.0));
    sink(Prop::FontSize, Value::Length(size));
    sink(
        Prop::LineHeight,
        Value::LineHeight(line_height.unwrap_or(LineHeight::Normal)),
    );
    sink(Prop::FontFamily, Value::FontFamilies(families));
    Ok(())
}

/// One `transition` layer: `[none | <property>] || <time> || <easing> || <time>`.
fn parse_transition_layer(
    input: &mut Parser<'_, '_>,
) -> Result<(Value, Time, TimingFunction, Time), ()> {
    let mut property: Option<Value> = None;
    let mut times: Vec<Time> = Vec::new();
    let mut timing: Option<TimingFunction> = None;
    let mut seen = false;
    loop {
        let state = input.state();
        if times.len() < 2 {
            if let Ok(parsed) = Time::parse(input) {
                times.push(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if timing.is_none() {
            if let Ok(parsed) = TimingFunction::parse(input) {
                timing = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if property.is_none() {
            if let Ok(name) = input.expect_ident() {
                let name = name.as_ref().to_string();
                property = Some(if name.eq_ignore_ascii_case("none") {
                    Value::Keyword(Keyword::None)
                } else if name.eq_ignore_ascii_case("all") {
                    Value::Keyword(Keyword::All)
                } else {
                    Value::AnimationName(AnimationName::Named(Rc::from(name.as_str())))
                });
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if !seen {
        return Err(());
    }
    Ok((
        property.unwrap_or(Value::Keyword(Keyword::All)),
        times.first().copied().unwrap_or(Time::ZERO),
        timing.unwrap_or(TimingFunction::EASE),
        times.get(1).copied().unwrap_or(Time::ZERO),
    ))
}

/// `transition`.
pub fn expand_transition(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_transition_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut properties = Vec::new();
    let mut durations = Vec::new();
    let mut timings = Vec::new();
    let mut delays = Vec::new();
    for (property, duration, timing, delay) in layers {
        properties.push(property);
        durations.push(Value::Time(duration));
        timings.push(Value::Timing(timing));
        delays.push(Value::Time(delay));
    }
    sink(Prop::TransitionProperty, Value::List(properties.into()));
    sink(Prop::TransitionDuration, Value::List(durations.into()));
    sink(Prop::TransitionTimingFunction, Value::List(timings.into()));
    sink(Prop::TransitionDelay, Value::List(delays.into()));
    Ok(())
}

struct AnimationLayer {
    name: AnimationName,
    duration: Time,
    delay: Time,
    timing: TimingFunction,
    iterations: IterationCount,
    direction: Keyword,
    fill: Keyword,
    play_state: Keyword,
}

fn parse_animation_layer(input: &mut Parser<'_, '_>) -> Result<AnimationLayer, ()> {
    const DIRECTIONS: &[Keyword] = &[
        Keyword::Normal,
        Keyword::Reverse,
        Keyword::Alternate,
        Keyword::AlternateReverse,
    ];
    const FILLS: &[Keyword] =
        &[Keyword::None, Keyword::Forwards, Keyword::Backwards, Keyword::Both];
    const PLAY_STATES: &[Keyword] = &[Keyword::Running, Keyword::Paused];
    let mut layer = AnimationLayer {
        name: AnimationName::None,
        duration: Time::ZERO,
        delay: Time::ZERO,
        timing: TimingFunction::EASE,
        iterations: IterationCount::Count(1.0),
        direction: Keyword::Normal,
        fill: Keyword::None,
        play_state: Keyword::Running,
    };
    let mut times: Vec<Time> = Vec::new();
    let (mut has_timing, mut has_iterations) = (false, false);
    let (mut has_direction, mut has_fill, mut has_play, mut has_name) =
        (false, false, false, false);
    let mut seen = false;
    loop {
        let state = input.state();
        if times.len() < 2 {
            if let Ok(parsed) = Time::parse(input) {
                times.push(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if !has_timing {
            if let Ok(parsed) = TimingFunction::parse(input) {
                layer.timing = parsed;
                has_timing = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if !has_iterations {
            if let Ok(parsed) = IterationCount::parse(input) {
                layer.iterations = parsed;
                has_iterations = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        let keyword = match input.next() {
            Ok(Token::Ident(name)) => Keyword::from_str_ascii_ci(name.as_ref()),
            _ => None,
        };
        if let Some(keyword) = keyword {
            if !has_direction && DIRECTIONS.contains(&keyword) {
                layer.direction = keyword;
                has_direction = true;
                seen = true;
                continue;
            }
            if !has_fill && FILLS.contains(&keyword) && keyword != Keyword::None {
                layer.fill = keyword;
                has_fill = true;
                seen = true;
                continue;
            }
            if !has_play && PLAY_STATES.contains(&keyword) {
                layer.play_state = keyword;
                has_play = true;
                seen = true;
                continue;
            }
        }
        input.reset(&state);
        if !has_name {
            if let Ok(parsed) = AnimationName::parse(input) {
                layer.name = parsed;
                has_name = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if !seen {
        return Err(());
    }
    layer.duration = times.first().copied().unwrap_or(Time::ZERO);
    layer.delay = times.get(1).copied().unwrap_or(Time::ZERO);
    Ok(layer)
}

/// `animation`.
pub fn expand_animation(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_animation_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut names = Vec::new();
    let mut durations = Vec::new();
    let mut timings = Vec::new();
    let mut iterations = Vec::new();
    let mut directions = Vec::new();
    let mut play_states = Vec::new();
    let mut delays = Vec::new();
    let mut fills = Vec::new();
    for layer in layers {
        names.push(Value::AnimationName(layer.name));
        durations.push(Value::Time(layer.duration));
        timings.push(Value::Timing(layer.timing));
        iterations.push(Value::IterationCount(layer.iterations));
        directions.push(Value::Keyword(layer.direction));
        play_states.push(Value::Keyword(layer.play_state));
        delays.push(Value::Time(layer.delay));
        fills.push(Value::Keyword(layer.fill));
    }
    sink(Prop::AnimationName, Value::List(names.into()));
    sink(Prop::AnimationDuration, Value::List(durations.into()));
    sink(Prop::AnimationTimingFunction, Value::List(timings.into()));
    sink(Prop::AnimationIterationCount, Value::List(iterations.into()));
    sink(Prop::AnimationDirection, Value::List(directions.into()));
    sink(Prop::AnimationPlayState, Value::List(play_states.into()));
    sink(Prop::AnimationDelay, Value::List(delays.into()));
    sink(Prop::AnimationFillMode, Value::List(fills.into()));
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::shorthand` — Expected: PASS, 11 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/shorthand.rs
git commit -m "feat(ui/css): the eighteen shorthand expanders

Expansion is typed now -- (Prop, Value) rather than (String, String) --
and every expander emits every longhand its row lists, so an omitted
component resets rather than leaving an earlier rule's value standing.
M1's nine shorthand tests move across with their inputs and expected side
values intact.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 16: `PROPERTIES` — the 113-row table

**Files:**
- Modify: `ui/src/css/registry.rs`
- Test: `ui/src/css/registry.rs`

**Interfaces:**
- Consumes: every value parser, every interpolator, all 18 expanders.
- Produces:
  - `registry::PROPERTIES: [PropertyDef; N_PROPS]`
  - `registry::animatable_longhands() -> impl Iterator<Item = Prop>`
  - `Prop::{def, is_inherited, initial, interpolator, expand_into, longhands}`
  - `registry::parse_declaration_value(prop: Prop, text: &str) -> Result<Value, ()>`
    (contract deviation 7 — Part 3's cascade entry point)

- [ ] **Step 1: Write the failing test**

Append inside `ui/src/css/registry.rs`'s `mod tests`:

```rust
    use super::{PROPERTIES, animatable_longhands, parse_declaration_value};
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::Value;

    #[test]
    fn the_table_agrees_with_the_enum_on_every_row() {
        assert_eq!(PROPERTIES.len(), N_PROPS);
        for prop in Prop::ALL {
            assert_eq!(prop.def().name, prop.name(), "row {prop:?} is misplaced");
            assert_eq!(
                prop.is_longhand(),
                matches!(prop.def().kind, super::PropertyKind::Longhand { .. }),
                "{prop:?} kind disagrees with its position"
            );
        }
    }

    #[test]
    fn the_settled_initial_values_are_what_the_contract_pins() {
        assert_eq!(Prop::MinWidth.initial(), Value::Length(Length::zero()));
        assert_eq!(Prop::MinHeight.initial(), Value::Length(Length::zero()));
        assert_eq!(Prop::BorderTopWidth.initial(), Value::Length(Length::px(3.0)));
        assert_eq!(Prop::OutlineWidth.initial(), Value::Length(Length::px(3.0)));
        assert_eq!(Prop::OutlineStyle.initial(), Value::Keyword(Keyword::None));
        assert_eq!(Prop::FontSize.initial(), Value::Length(Length::px(14.0)));
        assert_eq!(Prop::GtkDpi.initial(), Value::Number(96.0));
        assert_eq!(Prop::GtkIconSize.initial(), Value::Length(Length::px(16.0)));
        assert_eq!(Prop::GtkIconStyle.initial(), Value::Keyword(Keyword::Requested));
        assert_eq!(
            Prop::OutlineColor.initial(),
            Value::Color(crate::css::value::color::ColorValue::CurrentColor)
        );
        assert_eq!(
            Prop::Color.initial(),
            Value::Color(crate::css::value::color::ColorValue::Absolute(
                crate::css::value::color::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }
            ))
        );
    }

    #[test]
    fn the_inherited_flags_follow_gtk() {
        for prop in [
            Prop::Color, Prop::FontFamily, Prop::FontSize, Prop::LineHeight,
            Prop::LetterSpacing, Prop::TextTransform, Prop::CaretColor, Prop::GtkDpi,
            Prop::TextShadow, Prop::GtkIconSize, Prop::GtkIconShadow, Prop::BorderSpacing,
        ] {
            assert!(prop.is_inherited(), "{prop:?} should inherit");
        }
        for prop in [
            Prop::Opacity, Prop::Filter, Prop::Transform, Prop::MinWidth, Prop::PaddingTop,
            Prop::BorderTopWidth, Prop::BorderTopColor, Prop::BackgroundColor,
            Prop::BoxShadow, Prop::OutlineColor, Prop::TextDecorationLine,
            Prop::TransitionDuration, Prop::AnimationName,
        ] {
            assert!(!prop.is_inherited(), "{prop:?} should not inherit");
        }
        // A shorthand never reports itself as inherited.
        assert!(!Prop::Font.is_inherited());
        assert!(!Prop::Background.is_inherited());
    }

    #[test]
    fn every_animatable_row_has_an_interpolator_and_the_meta_rows_do_not() {
        assert!(Prop::BackgroundColor.interpolator().is_some());
        assert!(Prop::PaddingTop.interpolator().is_some());
        assert!(Prop::BoxShadow.interpolator().is_some());
        assert!(Prop::Transform.interpolator().is_some());
        assert!(Prop::TransitionDuration.interpolator().is_none());
        assert!(Prop::AnimationName.interpolator().is_none());
        assert!(Prop::FontFamily.interpolator().is_none());
        assert!(Prop::Font.interpolator().is_none());
        let animatable: Vec<Prop> = animatable_longhands().collect();
        assert!(animatable.iter().all(|prop| prop.is_longhand()));
        assert!(animatable.contains(&Prop::BackgroundColor));
        assert!(!animatable.contains(&Prop::TransitionDelay));
    }

    #[test]
    fn every_shorthand_lists_the_longhands_its_expander_emits() {
        for prop in Prop::ALL.iter().copied().filter(|prop| !prop.is_longhand()) {
            let listed = prop.longhands();
            assert!(!listed.is_empty(), "{prop:?} lists no longhands");
            assert!(listed.iter().all(|inner| inner.is_longhand()), "{prop:?} lists a shorthand");
        }
        assert_eq!(Prop::MinWidth.longhands(), &[] as &[Prop]);
        assert_eq!(
            Prop::Margin.longhands(),
            &[Prop::MarginTop, Prop::MarginRight, Prop::MarginBottom, Prop::MarginLeft]
        );
        assert_eq!(Prop::Border.longhands().len(), 12);
        assert_eq!(Prop::Animation.longhands().len(), 8);
    }

    #[test]
    fn a_shorthand_emits_every_longhand_it_lists() {
        // The CSS reset rule: an omitted component still lands, at its
        // initial value, or an earlier rule's value survives when it must not.
        for (prop, text) in [
            (Prop::Margin, "4px"),
            (Prop::Padding, "4px 9px"),
            (Prop::Border, "1px solid"),
            (Prop::BorderTop, "1px solid red"),
            (Prop::BorderWidth, "1px"),
            (Prop::BorderStyle, "solid"),
            (Prop::BorderColor, "red"),
            (Prop::BorderRadius, "5px"),
            (Prop::BorderImage, "url(\"b.png\") 30%"),
            (Prop::Outline, "2px solid red"),
            (Prop::Background, "#112233"),
            (Prop::Transition, "background-color 200ms"),
            (Prop::Animation, "spin 1s linear"),
            (Prop::Font, "14px Cantarell"),
            (Prop::TextDecoration, "underline"),
            (Prop::BorderRight, "1px solid red"),
            (Prop::BorderBottom, "1px solid red"),
            (Prop::BorderLeft, "1px solid red"),
        ] {
            let mut source = cssparser::ParserInput::new(text);
            let mut parser = cssparser::Parser::new(&mut source);
            let mut emitted: Vec<Prop> = Vec::new();
            {
                let mut sink = |longhand: Prop, _value: Value| emitted.push(longhand);
                prop.expand_into(&mut parser, &mut sink)
                    .unwrap_or_else(|()| panic!("{prop:?} failed to expand `{text}`"));
            }
            for longhand in prop.longhands() {
                assert!(
                    emitted.contains(longhand),
                    "{prop:?} did not emit {longhand:?} for `{text}`"
                );
            }
        }
    }

    #[test]
    fn every_longhand_accepts_the_wide_keywords() {
        for prop in longhands() {
            for keyword in ["inherit", "initial", "unset"] {
                assert!(
                    parse_declaration_value(prop, keyword).is_ok(),
                    "{prop:?} rejected `{keyword}`"
                );
            }
        }
    }

    #[test]
    fn adwaitas_declared_button_values_all_parse() {
        // The exact declarations the offscreen pixel gate pins.
        assert_eq!(
            parse_declaration_value(Prop::BorderRadius, "5px").is_ok(),
            true
        );
        assert!(parse_declaration_value(Prop::Padding, "4px 9px").is_ok());
        assert!(parse_declaration_value(Prop::Border, "1px solid").is_ok());
        assert!(parse_declaration_value(Prop::BorderColor, "#cdc7c2").is_ok());
        assert!(parse_declaration_value(Prop::Color, "#2e3436").is_ok());
        assert!(parse_declaration_value(Prop::MinHeight, "24px").is_ok());
        assert!(parse_declaration_value(Prop::MinWidth, "16px").is_ok());
        assert!(
            parse_declaration_value(
                Prop::BackgroundImage,
                "linear-gradient(to top, #f6f5f4 2px, #fbfafa)"
            )
            .is_ok()
        );
        // A trailing token invalidates the declaration.
        assert!(parse_declaration_value(Prop::MinWidth, "16px 4px").is_err());
    }

    #[test]
    fn no_registry_parser_panics_on_hostile_input() {
        for prop in Prop::ALL {
            for input in crate::css::value::FUZZ_INPUTS {
                let _ = parse_declaration_value(*prop, input);
            }
        }
    }
```

Mutation check: `a_shorthand_emits_every_longhand_it_lists` is the reset-rule gate —
deleting one `sink(...)` call in any expander fails it.
`the_settled_initial_values_are_what_the_contract_pins` fails if `medium` stops being
3px or `min-width`'s initial reverts to `auto`, either of which silently changes
every computed box.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::registry`
Expected: FAIL — `error[E0432]: unresolved import 'super::PROPERTIES'`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/css/registry.rs`:

```rust
use std::rc::Rc;

use cssparser::{Parser, ParserInput};

use crate::css::value::border::{
    BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle, parse_line_style,
    parse_line_width,
};
use crate::css::value::color::{ColorValue, Rgba};
use crate::css::value::font::{
    FontFamily, FontStyle, FontVariantFlags, FontWeight, GenericFamily, LineHeight,
    parse_family_list, parse_feature_settings, parse_font_size, parse_stretch, parse_variant_flags,
    parse_variation_settings,
};
use crate::css::value::image::{Image, Position, parse_icon_palette};
use crate::css::value::interpolate;
use crate::css::value::keyword::Keyword;
use crate::css::value::length::Length;
use crate::css::value::shadow::Shadow;
use crate::css::value::shorthand as sh_fns;
use crate::css::value::text::parse_decoration_lines;
use crate::css::value::timing::{AnimationName, IterationCount, Time, TimingFunction};
use crate::css::value::transform::parse_transform_list;
use crate::css::value::filter::parse_filter_list;

const fn lh(
    name: &'static str,
    parse: ParseFn,
    initial: fn() -> Value,
    inherited: bool,
    animatable: Option<Interpolate>,
) -> PropertyDef {
    PropertyDef {
        name,
        kind: PropertyKind::Longhand { parse, initial, inherited, animatable },
    }
}

const fn sh(name: &'static str, expand: ExpandFn, longhands: &'static [Prop]) -> PropertyDef {
    PropertyDef { name, kind: PropertyKind::Shorthand { expand, longhands } }
}

/// Every registry parser goes through the wide keywords first.
fn wide(input: &mut Parser<'_, '_>, inner: ParseFn) -> Result<Value, ()> {
    Value::parse_wide_or(input, inner)
}

fn keyword_in(input: &mut Parser<'_, '_>, allowed: &[Keyword]) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    allowed
        .iter()
        .copied()
        .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
        .ok_or(())
}

// ---- longhand parsers -------------------------------------------------

fn p_color(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Color(ColorValue::parse(i)?)))
}

fn p_caret_color(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident() {
            if name.eq_ignore_ascii_case("auto") {
                return Ok(Value::Color(ColorValue::CurrentColor));
            }
        }
        i.reset(&state);
        Ok(Value::Color(ColorValue::parse(i)?))
    })
}

fn p_opacity(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let value = i.expect_number().map_err(|_| ())?;
        if !value.is_finite() {
            return Err(());
        }
        Ok(Value::Number(value.clamp(0.0, 1.0)))
    })
}

fn p_number(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let value = i.expect_number().map_err(|_| ())?;
        if value.is_finite() { Ok(Value::Number(value)) } else { Err(()) }
    })
}

fn p_length(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(Length::parse(i)?)))
}

fn p_length_auto(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(Length::parse_allowing_auto(i)?)))
}

fn p_line_width(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(parse_line_width(i)?)))
}

fn p_line_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Keyword(parse_line_style(i)?)))
}

fn p_radius(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let horizontal = Length::parse(i)?;
        let state = i.state();
        let vertical = match Length::parse(i) {
            Ok(length) => length,
            Err(()) => {
                i.reset(&state);
                horizontal.clone()
            }
        };
        Ok(Value::Pair(Rc::new((
            Value::Length(horizontal),
            Value::Length(vertical),
        ))))
    })
}

fn p_border_spacing(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let horizontal = Length::parse(i)?;
        let state = i.state();
        let vertical = match Length::parse(i) {
            Ok(length) => length,
            Err(()) => {
                i.reset(&state);
                horizontal.clone()
            }
        };
        Ok(Value::Pair(Rc::new((
            Value::Length(horizontal),
            Value::Length(vertical),
        ))))
    })
}

fn p_image(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Image(Image::parse(i)?)))
}

fn p_image_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Image(Image::parse(item)?)))
    })
}

fn p_icon_source(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident() {
            if name.eq_ignore_ascii_case("builtin") {
                return Ok(Value::Keyword(Keyword::Builtin));
            }
        }
        i.reset(&state);
        Ok(Value::Image(Image::parse(i)?))
    })
}

fn p_icon_palette(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::IconPalette(parse_icon_palette(i)?)))
}

fn p_transform(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Transform(parse_transform_list(i)?)))
}

fn p_filter(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Filter(parse_filter_list(i)?)))
}

fn p_position(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Position(Position::parse(i)?)))
}

fn p_position_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Position(Position::parse(item)?)))
    })
}

fn p_bg_size_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::BgSize(BgSize::parse(item)?)))
    })
}

fn p_bg_repeat_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Repeat(RepeatStyle::parse_background(item)?)))
    })
}

fn p_border_image_repeat(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Repeat(RepeatStyle::parse(i)?)))
}

fn p_border_image_slice(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Slice(BorderImageSlice::parse(i)?)))
}

fn p_border_image_width(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let mut values: Vec<BorderImageWidthSide> = Vec::new();
        while values.len() < 5 {
            let state = i.state();
            match BorderImageWidthSide::parse(i) {
                Ok(value) => values.push(value),
                Err(()) => {
                    i.reset(&state);
                    break;
                }
            }
        }
        crate::css::value::border::four_sides(values)
            .map(Value::BorderImageWidths)
            .ok_or(())
    })
}

fn p_box_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::BorderBox,
                    Keyword::PaddingBox,
                    Keyword::ContentBox,
                    Keyword::TextBox,
                ],
            )?))
        })
    })
}

fn p_blend_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::Normal, Keyword::Multiply, Keyword::Screen, Keyword::Overlay,
                    Keyword::Darken, Keyword::Lighten, Keyword::ColorDodge, Keyword::ColorBurn,
                    Keyword::HardLight, Keyword::SoftLight, Keyword::Difference,
                    Keyword::Exclusion, Keyword::Hue, Keyword::Saturation,
                    Keyword::ColorBlend, Keyword::Luminosity,
                ],
            )?))
        })
    })
}

fn p_shadow_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident() {
            if name.eq_ignore_ascii_case("none") {
                return Ok(Value::List(Rc::from(Vec::new())));
            }
        }
        i.reset(&state);
        Value::parse_list(i, |item| Ok(Value::Shadow(Shadow::parse(item)?)))
    })
}

fn p_text_shadow_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident() {
            if name.eq_ignore_ascii_case("none") {
                return Ok(Value::List(Rc::from(Vec::new())));
            }
        }
        i.reset(&state);
        Value::parse_list(i, |item| Ok(Value::Shadow(Shadow::parse_text(item)?)))
    })
}

fn p_family(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontFamilies(parse_family_list(i)?)))
}

fn p_font_size(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(parse_font_size(i)?)))
}

fn p_font_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontStyle(FontStyle::parse(i)?)))
}

fn p_font_weight(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontWeight(FontWeight::parse(i)?)))
}

fn p_stretch(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Percentage(parse_stretch(i)? / 100.0)))
}

fn p_features(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontFeatures(parse_feature_settings(i)?)))
}

fn p_variations(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontVariations(parse_variation_settings(i)?)))
}

/// The `font-variant-*` longhands differ only in the flag subset they take.
macro_rules! variant_parser {
    ($name:ident, $( $flag:ident )|+) => {
        fn $name(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
            wide(input, |i| {
                const ALLOWED: FontVariantFlags =
                    FontVariantFlags::from_bits_truncate($( FontVariantFlags::$flag.bits() )|+);
                Ok(Value::FontVariant(parse_variant_flags(i, ALLOWED)?))
            })
        }
    };
}

// GTK supports only the CSS2 `font-variant` values (`normal | small-caps`).
variant_parser!(p_variant_css2, SMALL_CAPS);
variant_parser!(
    p_variant_ligatures,
    NO_COMMON_LIGATURES | DISCRETIONARY_LIGATURES | HISTORICAL_LIGATURES | NO_CONTEXTUAL
);
variant_parser!(p_variant_position, SUB | SUPER);
variant_parser!(
    p_variant_caps,
    SMALL_CAPS | ALL_SMALL_CAPS | PETITE_CAPS | ALL_PETITE_CAPS | UNICASE | TITLING_CAPS
);
variant_parser!(
    p_variant_numeric,
    LINING_NUMS | OLDSTYLE_NUMS | PROPORTIONAL_NUMS | TABULAR_NUMS
        | DIAGONAL_FRACTIONS | STACKED_FRACTIONS | ORDINAL | SLASHED_ZERO
);
variant_parser!(p_variant_alternates, HISTORICAL_FORMS);
variant_parser!(
    p_variant_east_asian,
    JIS78 | JIS83 | JIS90 | JIS04 | SIMPLIFIED | TRADITIONAL
        | FULL_WIDTH_EA | PROPORTIONAL_WIDTH_EA | RUBY
);

fn p_kerning(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[Keyword::Auto, Keyword::Normal, Keyword::None],
        )?))
    })
}

fn p_letter_spacing(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident() {
            if name.eq_ignore_ascii_case("normal") {
                return Ok(Value::Length(Length::zero()));
            }
        }
        i.reset(&state);
        Ok(Value::Length(Length::parse(i)?))
    })
}

fn p_text_transform(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[
                Keyword::None, Keyword::Capitalize, Keyword::Uppercase, Keyword::Lowercase,
                Keyword::FullWidth, Keyword::FullSizeKana,
            ],
        )?))
    })
}

fn p_line_height(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::LineHeight(LineHeight::parse(i)?)))
}

fn p_decoration_lines(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::TextDecorationLines(parse_decoration_lines(i)?))
    })
}

fn p_decoration_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[
                Keyword::Solid, Keyword::Double, Keyword::Dotted, Keyword::Dashed, Keyword::Wavy,
            ],
        )?))
    })
}

fn p_icon_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[Keyword::Requested, Keyword::Regular, Keyword::Symbolic],
        )?))
    })
}

fn p_transition_property_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            let name = item.expect_ident().map_err(|_| ())?.as_ref().to_string();
            Ok(if name.eq_ignore_ascii_case("all") {
                Value::Keyword(Keyword::All)
            } else if name.eq_ignore_ascii_case("none") {
                Value::Keyword(Keyword::None)
            } else {
                Value::AnimationName(AnimationName::Named(Rc::from(name.as_str())))
            })
        })
    })
}

fn p_time_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Time(Time::parse(item)?)))
    })
}

fn p_timing_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Timing(TimingFunction::parse(item)?)))
    })
}

fn p_animation_name_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::AnimationName(AnimationName::parse(item)?)))
    })
}

fn p_iteration_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::IterationCount(IterationCount::parse(item)?)))
    })
}

fn p_direction_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::Normal, Keyword::Reverse, Keyword::Alternate,
                    Keyword::AlternateReverse,
                ],
            )?))
        })
    })
}

fn p_play_state_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[Keyword::Running, Keyword::Paused],
            )?))
        })
    })
}

fn p_fill_mode_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[Keyword::None, Keyword::Forwards, Keyword::Backwards, Keyword::Both],
            )?))
        })
    })
}

// ---- initial values ---------------------------------------------------

fn i_black() -> Value {
    Value::Color(ColorValue::Absolute(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }))
}
fn i_transparent() -> Value {
    Value::Color(ColorValue::Absolute(Rgba::TRANSPARENT))
}
fn i_current_color() -> Value {
    Value::Color(ColorValue::CurrentColor)
}
fn i_one() -> Value {
    Value::Number(1.0)
}
fn i_dpi() -> Value {
    Value::Number(96.0)
}
fn i_weight_400() -> Value {
    Value::FontWeight(FontWeight::Absolute(400.0))
}
fn i_pct_one() -> Value {
    Value::Percentage(1.0)
}
fn i_zero_length() -> Value {
    Value::Length(Length::zero())
}
fn i_len_3() -> Value {
    Value::Length(Length::px(3.0))
}
fn i_len_14() -> Value {
    Value::Length(Length::px(14.0))
}
fn i_len_16() -> Value {
    Value::Length(Length::px(16.0))
}
fn i_none_keyword() -> Value {
    Value::Keyword(Keyword::None)
}
fn i_normal_keyword() -> Value {
    Value::Keyword(Keyword::Normal)
}
fn i_auto_keyword() -> Value {
    Value::Keyword(Keyword::Auto)
}
fn i_solid_keyword() -> Value {
    Value::Keyword(Keyword::Solid)
}
fn i_requested_keyword() -> Value {
    Value::Keyword(Keyword::Requested)
}
fn i_builtin_keyword() -> Value {
    Value::Keyword(Keyword::Builtin)
}
fn i_variant_empty() -> Value {
    Value::FontVariant(FontVariantFlags::empty())
}
fn i_features_empty() -> Value {
    Value::FontFeatures(Rc::from(Vec::new()))
}
fn i_variations_empty() -> Value {
    Value::FontVariations(Rc::from(Vec::new()))
}
fn i_family_sans() -> Value {
    Value::FontFamilies(Rc::from(vec![FontFamily::Generic(GenericFamily::SansSerif)]))
}
fn i_style_normal() -> Value {
    Value::FontStyle(FontStyle::Normal)
}
fn i_line_height_normal() -> Value {
    Value::LineHeight(LineHeight::Normal)
}
fn i_decoration_none() -> Value {
    Value::TextDecorationLines(crate::css::value::text::TextDecorationLines::empty())
}
fn i_transform_none() -> Value {
    Value::Transform(Rc::from(Vec::new()))
}
fn i_filter_none() -> Value {
    Value::Filter(Rc::from(Vec::new()))
}
fn i_position_center() -> Value {
    Value::Position(Position::center())
}
fn i_radius_zero() -> Value {
    Value::Pair(Rc::new((i_zero_length(), i_zero_length())))
}
fn i_spacing_zero() -> Value {
    Value::Pair(Rc::new((i_zero_length(), i_zero_length())))
}
fn i_image_none() -> Value {
    Value::Image(Image::None)
}
fn i_empty_list() -> Value {
    Value::List(Rc::from(Vec::new()))
}
fn i_image_none_list() -> Value {
    Value::List(Rc::from(vec![Value::Image(Image::None)]))
}
fn i_position_origin_list() -> Value {
    Value::List(Rc::from(vec![Value::Position(Position {
        x: Length::Percent(0.0),
        y: Length::Percent(0.0),
        z: None,
    })]))
}
fn i_bg_size_auto_list() -> Value {
    Value::List(Rc::from(vec![Value::BgSize(BgSize::Auto)]))
}
fn i_bg_repeat_list() -> Value {
    Value::List(Rc::from(vec![Value::Repeat(RepeatStyle {
        x: Keyword::Repeat,
        y: Keyword::Repeat,
    })]))
}
fn i_clip_border_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::BorderBox)]))
}
fn i_origin_padding_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::PaddingBox)]))
}
fn i_blend_normal_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Normal)]))
}
fn i_repeat_stretch() -> Value {
    Value::Repeat(RepeatStyle { x: Keyword::Stretch, y: Keyword::Stretch })
}
fn i_slice_full() -> Value {
    Value::Slice(BorderImageSlice { sides: [NumberOrPercent::Percent(1.0); 4], fill: false })
}
fn i_border_image_width_one() -> Value {
    Value::BorderImageWidths([
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
    ])
}
fn i_icon_palette_default() -> Value {
    Value::IconPalette(Rc::from(vec![
        (Rc::from("error"), ColorValue::Named(Rc::from("error_color"))),
        (Rc::from("warning"), ColorValue::Named(Rc::from("warning_color"))),
        (Rc::from("success"), ColorValue::Named(Rc::from("success_color"))),
    ]))
}
fn i_transition_property_all() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::All)]))
}
fn i_time_zero_list() -> Value {
    Value::List(Rc::from(vec![Value::Time(Time::ZERO)]))
}
fn i_timing_ease_list() -> Value {
    Value::List(Rc::from(vec![Value::Timing(TimingFunction::EASE)]))
}
fn i_animation_name_none_list() -> Value {
    Value::List(Rc::from(vec![Value::AnimationName(AnimationName::None)]))
}
fn i_iteration_one_list() -> Value {
    Value::List(Rc::from(vec![Value::IterationCount(IterationCount::Count(1.0))]))
}
fn i_normal_keyword_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Normal)]))
}
fn i_running_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Running)]))
}
fn i_none_keyword_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::None)]))
}

// ---- the shorthands' longhand lists -----------------------------------

const FONT_LONGHANDS: &[Prop] = &[
    Prop::FontStyle, Prop::FontVariant, Prop::FontWeight, Prop::FontStretch,
    Prop::FontSize, Prop::LineHeight, Prop::FontFamily,
];
const TEXT_DECORATION_LONGHANDS: &[Prop] =
    &[Prop::TextDecorationLine, Prop::TextDecorationStyle, Prop::TextDecorationColor];
const MARGIN_LONGHANDS: &[Prop] =
    &[Prop::MarginTop, Prop::MarginRight, Prop::MarginBottom, Prop::MarginLeft];
const PADDING_LONGHANDS: &[Prop] =
    &[Prop::PaddingTop, Prop::PaddingRight, Prop::PaddingBottom, Prop::PaddingLeft];
const BORDER_WIDTH_LONGHANDS: &[Prop] = &[
    Prop::BorderTopWidth, Prop::BorderRightWidth, Prop::BorderBottomWidth, Prop::BorderLeftWidth,
];
const BORDER_STYLE_LONGHANDS: &[Prop] = &[
    Prop::BorderTopStyle, Prop::BorderRightStyle, Prop::BorderBottomStyle, Prop::BorderLeftStyle,
];
const BORDER_COLOR_LONGHANDS: &[Prop] = &[
    Prop::BorderTopColor, Prop::BorderRightColor, Prop::BorderBottomColor, Prop::BorderLeftColor,
];
const BORDER_TOP_LONGHANDS: &[Prop] =
    &[Prop::BorderTopWidth, Prop::BorderTopStyle, Prop::BorderTopColor];
const BORDER_RIGHT_LONGHANDS: &[Prop] =
    &[Prop::BorderRightWidth, Prop::BorderRightStyle, Prop::BorderRightColor];
const BORDER_BOTTOM_LONGHANDS: &[Prop] =
    &[Prop::BorderBottomWidth, Prop::BorderBottomStyle, Prop::BorderBottomColor];
const BORDER_LEFT_LONGHANDS: &[Prop] =
    &[Prop::BorderLeftWidth, Prop::BorderLeftStyle, Prop::BorderLeftColor];
const BORDER_LONGHANDS: &[Prop] = &[
    Prop::BorderTopWidth, Prop::BorderTopStyle, Prop::BorderTopColor,
    Prop::BorderRightWidth, Prop::BorderRightStyle, Prop::BorderRightColor,
    Prop::BorderBottomWidth, Prop::BorderBottomStyle, Prop::BorderBottomColor,
    Prop::BorderLeftWidth, Prop::BorderLeftStyle, Prop::BorderLeftColor,
];
const BORDER_RADIUS_LONGHANDS: &[Prop] = &[
    Prop::BorderTopLeftRadius, Prop::BorderTopRightRadius,
    Prop::BorderBottomRightRadius, Prop::BorderBottomLeftRadius,
];
const BORDER_IMAGE_LONGHANDS: &[Prop] = &[
    Prop::BorderImageSource, Prop::BorderImageSlice,
    Prop::BorderImageWidth, Prop::BorderImageRepeat,
];
const OUTLINE_LONGHANDS: &[Prop] = &[Prop::OutlineWidth, Prop::OutlineStyle, Prop::OutlineColor];
const BACKGROUND_LONGHANDS: &[Prop] = &[
    Prop::BackgroundColor, Prop::BackgroundImage, Prop::BackgroundPosition,
    Prop::BackgroundSize, Prop::BackgroundRepeat, Prop::BackgroundOrigin, Prop::BackgroundClip,
];
const TRANSITION_LONGHANDS: &[Prop] = &[
    Prop::TransitionProperty, Prop::TransitionDuration,
    Prop::TransitionTimingFunction, Prop::TransitionDelay,
];
const ANIMATION_LONGHANDS: &[Prop] = &[
    Prop::AnimationName, Prop::AnimationDuration, Prop::AnimationTimingFunction,
    Prop::AnimationIterationCount, Prop::AnimationDirection, Prop::AnimationPlayState,
    Prop::AnimationDelay, Prop::AnimationFillMode,
];

/// Indexed by `prop as usize`. Row order == `Prop` discriminant order.
pub static PROPERTIES: [PropertyDef; N_PROPS] = [
    // colours & effects
    lh("color", p_color, i_black, true, Some(interpolate::color)),
    lh("opacity", p_opacity, i_one, false, Some(interpolate::number)),
    lh("filter", p_filter, i_filter_none, false, Some(interpolate::filter_list)),
    // fonts
    lh("font-family", p_family, i_family_sans, true, None),
    lh("font-size", p_font_size, i_len_14, true, Some(interpolate::length)),
    lh("font-style", p_font_style, i_style_normal, true, None),
    lh("font-variant", p_variant_css2, i_variant_empty, true, None),
    lh("font-weight", p_font_weight, i_weight_400, true, Some(interpolate::font_weight)),
    lh("font-width", p_stretch, i_pct_one, true, Some(interpolate::percentage)),
    lh("font-stretch", p_stretch, i_pct_one, true, Some(interpolate::percentage)),
    lh("font-kerning", p_kerning, i_auto_keyword, true, None),
    lh("font-variant-ligatures", p_variant_ligatures, i_variant_empty, true, None),
    lh("font-variant-position", p_variant_position, i_variant_empty, true, None),
    lh("font-variant-caps", p_variant_caps, i_variant_empty, true, None),
    lh("font-variant-numeric", p_variant_numeric, i_variant_empty, true, None),
    lh("font-variant-alternates", p_variant_alternates, i_variant_empty, true, None),
    lh("font-variant-east-asian", p_variant_east_asian, i_variant_empty, true, None),
    lh("font-feature-settings", p_features, i_features_empty, true, None),
    lh("font-variation-settings", p_variations, i_variations_empty, true, None),
    lh("-gtk-dpi", p_number, i_dpi, true, None),
    // text
    lh("caret-color", p_caret_color, i_current_color, true, Some(interpolate::color)),
    // NOT AVAILABLE from GTK docs; chosen fallback: currentColor, by analogy
    // with `caret-color`.
    lh(
        "-gtk-secondary-caret-color",
        p_color,
        i_current_color,
        true,
        Some(interpolate::color),
    ),
    lh("letter-spacing", p_letter_spacing, i_zero_length, true, Some(interpolate::length)),
    lh("text-transform", p_text_transform, i_none_keyword, true, None),
    lh("line-height", p_line_height, i_line_height_normal, true, Some(interpolate::line_height)),
    lh("text-decoration-line", p_decoration_lines, i_decoration_none, false, None),
    lh("text-decoration-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("text-decoration-style", p_decoration_style, i_solid_keyword, false, None),
    lh("text-shadow", p_text_shadow_list, i_empty_list, true, Some(interpolate::shadow_list)),
    // icons: parsed and stored; drawn in M4
    lh("-gtk-icon-source", p_icon_source, i_builtin_keyword, false, None),
    // NOT AVAILABLE from GTK docs; chosen fallback: 16px, GTK's default icon size.
    lh("-gtk-icon-size", p_length, i_len_16, true, Some(interpolate::length)),
    // NOT AVAILABLE from GTK docs; chosen fallback: `requested`.
    lh("-gtk-icon-style", p_icon_style, i_requested_keyword, true, None),
    lh("-gtk-icon-transform", p_transform, i_transform_none, true, Some(interpolate::transform)),
    lh("-gtk-icon-palette", p_icon_palette, i_icon_palette_default, true, None),
    lh("-gtk-icon-shadow", p_text_shadow_list, i_empty_list, true, Some(interpolate::shadow_list)),
    lh("-gtk-icon-filter", p_filter, i_filter_none, true, Some(interpolate::filter_list)),
    // NOT AVAILABLE from GTK docs; chosen fallback: 400, "like font weight".
    lh("-gtk-icon-weight", p_font_weight, i_weight_400, true, Some(interpolate::font_weight)),
    // transform
    lh("transform", p_transform, i_transform_none, false, Some(interpolate::transform)),
    lh("transform-origin", p_position, i_position_center, false, None),
    // box model
    lh("min-width", p_length, i_zero_length, false, Some(interpolate::length)),
    lh("min-height", p_length, i_zero_length, false, Some(interpolate::length)),
    lh("margin-top", p_length_auto, i_zero_length, false, Some(interpolate::length)),
    lh("margin-right", p_length_auto, i_zero_length, false, Some(interpolate::length)),
    lh("margin-bottom", p_length_auto, i_zero_length, false, Some(interpolate::length)),
    lh("margin-left", p_length_auto, i_zero_length, false, Some(interpolate::length)),
    lh("padding-top", p_length, i_zero_length, false, Some(interpolate::length)),
    lh("padding-right", p_length, i_zero_length, false, Some(interpolate::length)),
    lh("padding-bottom", p_length, i_zero_length, false, Some(interpolate::length)),
    lh("padding-left", p_length, i_zero_length, false, Some(interpolate::length)),
    // borders
    lh("border-top-width", p_line_width, i_len_3, false, Some(interpolate::length)),
    lh("border-right-width", p_line_width, i_len_3, false, Some(interpolate::length)),
    lh("border-bottom-width", p_line_width, i_len_3, false, Some(interpolate::length)),
    lh("border-left-width", p_line_width, i_len_3, false, Some(interpolate::length)),
    lh("border-top-style", p_line_style, i_none_keyword, false, None),
    lh("border-right-style", p_line_style, i_none_keyword, false, None),
    lh("border-bottom-style", p_line_style, i_none_keyword, false, None),
    lh("border-left-style", p_line_style, i_none_keyword, false, None),
    lh("border-top-left-radius", p_radius, i_radius_zero, false, Some(interpolate::pair)),
    lh("border-top-right-radius", p_radius, i_radius_zero, false, Some(interpolate::pair)),
    lh("border-bottom-right-radius", p_radius, i_radius_zero, false, Some(interpolate::pair)),
    lh("border-bottom-left-radius", p_radius, i_radius_zero, false, Some(interpolate::pair)),
    lh("border-top-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("border-right-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("border-bottom-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("border-left-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("border-image-source", p_image, i_image_none, false, Some(interpolate::image)),
    lh("border-image-repeat", p_border_image_repeat, i_repeat_stretch, false, None),
    lh("border-image-slice", p_border_image_slice, i_slice_full, false, None),
    lh("border-image-width", p_border_image_width, i_border_image_width_one, false, None),
    // outline
    lh("outline-style", p_line_style, i_none_keyword, false, None),
    lh("outline-width", p_line_width, i_len_3, false, Some(interpolate::length)),
    lh("outline-color", p_color, i_current_color, false, Some(interpolate::color)),
    lh("outline-offset", p_length, i_zero_length, false, Some(interpolate::length)),
    // backgrounds
    lh("background-color", p_color, i_transparent, false, Some(interpolate::color)),
    lh("background-clip", p_box_list, i_clip_border_list, false, None),
    lh("background-origin", p_box_list, i_origin_padding_list, false, None),
    lh("background-size", p_bg_size_list, i_bg_size_auto_list, false, Some(interpolate::list)),
    lh("background-position", p_position_list, i_position_origin_list, false, Some(interpolate::list)),
    lh("background-repeat", p_bg_repeat_list, i_bg_repeat_list, false, None),
    lh("background-image", p_image_list, i_image_none_list, false, Some(interpolate::list)),
    lh("box-shadow", p_shadow_list, i_empty_list, false, Some(interpolate::shadow_list)),
    lh("background-blend-mode", p_blend_list, i_blend_normal_list, false, None),
    // transitions (meta-properties: never themselves animatable)
    lh("transition-property", p_transition_property_list, i_transition_property_all, false, None),
    lh("transition-duration", p_time_list, i_time_zero_list, false, None),
    lh("transition-timing-function", p_timing_list, i_timing_ease_list, false, None),
    lh("transition-delay", p_time_list, i_time_zero_list, false, None),
    // animations
    lh("animation-name", p_animation_name_list, i_animation_name_none_list, false, None),
    lh("animation-duration", p_time_list, i_time_zero_list, false, None),
    lh("animation-timing-function", p_timing_list, i_timing_ease_list, false, None),
    lh("animation-iteration-count", p_iteration_list, i_iteration_one_list, false, None),
    lh("animation-direction", p_direction_list, i_normal_keyword_list, false, None),
    lh("animation-play-state", p_play_state_list, i_running_list, false, None),
    lh("animation-delay", p_time_list, i_time_zero_list, false, None),
    lh("animation-fill-mode", p_fill_mode_list, i_none_keyword_list, false, None),
    // misc
    lh("border-spacing", p_border_spacing, i_spacing_zero, true, Some(interpolate::pair)),
    // ---- shorthands ----
    sh("font", sh_fns::expand_font, FONT_LONGHANDS),
    sh("text-decoration", sh_fns::expand_text_decoration, TEXT_DECORATION_LONGHANDS),
    sh("margin", sh_fns::expand_margin, MARGIN_LONGHANDS),
    sh("padding", sh_fns::expand_padding, PADDING_LONGHANDS),
    sh("border-width", sh_fns::expand_border_width, BORDER_WIDTH_LONGHANDS),
    sh("border-style", sh_fns::expand_border_style, BORDER_STYLE_LONGHANDS),
    sh("border-color", sh_fns::expand_border_color, BORDER_COLOR_LONGHANDS),
    sh("border-top", sh_fns::expand_border_top, BORDER_TOP_LONGHANDS),
    sh("border-right", sh_fns::expand_border_right, BORDER_RIGHT_LONGHANDS),
    sh("border-bottom", sh_fns::expand_border_bottom, BORDER_BOTTOM_LONGHANDS),
    sh("border-left", sh_fns::expand_border_left, BORDER_LEFT_LONGHANDS),
    sh("border", sh_fns::expand_border, BORDER_LONGHANDS),
    sh("border-radius", sh_fns::expand_border_radius, BORDER_RADIUS_LONGHANDS),
    sh("border-image", sh_fns::expand_border_image, BORDER_IMAGE_LONGHANDS),
    sh("outline", sh_fns::expand_outline, OUTLINE_LONGHANDS),
    sh("background", sh_fns::expand_background, BACKGROUND_LONGHANDS),
    sh("transition", sh_fns::expand_transition, TRANSITION_LONGHANDS),
    sh("animation", sh_fns::expand_animation, ANIMATION_LONGHANDS),
];

impl Prop {
    /// This property's registry row.
    #[must_use]
    pub fn def(self) -> &'static PropertyDef {
        &PROPERTIES[self as usize]
    }

    /// Whether the property inherits; always `false` for a shorthand.
    #[must_use]
    pub fn is_inherited(self) -> bool {
        match self.def().kind {
            PropertyKind::Longhand { inherited, .. } => inherited,
            PropertyKind::Shorthand { .. } => false,
        }
    }

    /// The property's initial value.
    ///
    /// # Panics
    /// Panics for a shorthand, which has no computed slot of its own.
    #[must_use]
    pub fn initial(self) -> Value {
        match self.def().kind {
            PropertyKind::Longhand { initial, .. } => initial(),
            PropertyKind::Shorthand { .. } => {
                panic!("{} is a shorthand and has no initial value", self.name())
            }
        }
    }

    /// The property's interpolator, if it animates.
    #[must_use]
    pub fn interpolator(self) -> Option<Interpolate> {
        match self.def().kind {
            PropertyKind::Longhand { animatable, .. } => animatable,
            PropertyKind::Shorthand { .. } => None,
        }
    }

    /// Expand a shorthand into `sink`. `Err(())` for a longhand or for a
    /// value the shorthand cannot parse.
    pub fn expand_into(
        self,
        input: &mut Parser<'_, '_>,
        sink: &mut dyn FnMut(Prop, Value),
    ) -> Result<(), ()> {
        match self.def().kind {
            PropertyKind::Shorthand { expand, .. } => expand(input, sink),
            PropertyKind::Longhand { .. } => Err(()),
        }
    }

    /// The longhands this shorthand sets, in reset order; empty for a longhand.
    #[must_use]
    pub fn longhands(self) -> &'static [Prop] {
        match self.def().kind {
            PropertyKind::Shorthand { longhands, .. } => longhands,
            PropertyKind::Longhand { .. } => &[],
        }
    }
}

/// Every animatable longhand -- the expansion of `transition-property: all`.
pub fn animatable_longhands() -> impl Iterator<Item = Prop> {
    longhands().filter(|prop| prop.interpolator().is_some())
}

/// Parse a whole declaration value for `prop`.
///
/// The one entry point the cascade needs: it runs the row's `ParseFn`
/// against `text` and requires the whole input to be consumed.
/// `Err(())` == invalid at parse time, so the declaration is dropped.
pub fn parse_declaration_value(prop: Prop, text: &str) -> Result<Value, ()> {
    let PropertyKind::Longhand { parse, .. } = prop.def().kind else {
        return Err(());
    };
    let mut source = ParserInput::new(text);
    let mut parser = Parser::new(&mut source);
    let value = parse(&mut parser)?;
    parser.skip_whitespace();
    if parser.is_exhausted() { Ok(value) } else { Err(()) }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::registry` — Expected: PASS, 13 tests.
Run: `cargo test -p icedtea-ui` — Expected: PASS.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/registry.rs
git commit -m "feat(ui/css): the 113-row property table

Every GTK 4.22 CSS property, with its whole-value parser, initial value,
inherited flag and interpolator. The four initial values GTK's own docs do
not publish carry a NOT AVAILABLE comment naming the chosen fallback.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 17: `@keyframes` and `@media` in the stylesheet parser

`cssparser` has no `@media`/`@keyframes` grammar — both are ordinary at-rules
dispatched through `AtRuleParser`, and the prelude grammars are project code
(research/cssparser-selectors.md §1). `@media` blocks are kept **unevaluated** so
one parse can be compiled under several `MediaEnv`s (the Part 6 coverage gate
compiles light, dark and high-contrast from the same sheets).

**Files:**
- Modify: `ui/src/css/parse.rs` (`Stylesheet` fields, `SheetItem`, `AtPrelude`,
  `SheetParser::{parse_prelude, parse_block}`, `parse_into`; new types and
  `Stylesheet::append_layer`)
- Test: `ui/src/css/parse.rs` — **appended after** the existing 16 M1 tests, which
  are not touched

**Interfaces:**
- Produces:
  - `css::parse::{ColorScheme, Contrast, MediaEnv, MediaQuery, MediaBlock, KeyframesRule}`
  - `MediaQuery::evaluate(&self, &MediaEnv) -> bool`
  - `Stylesheet::{keyframes, media_blocks}` fields
  - `Stylesheet::append_layer(&mut self, layer: Stylesheet)` (contract deviation 7)

- [ ] **Step 1: Write the failing test**

Append to the **end** of `ui/src/css/parse.rs`'s existing `mod tests` — after
`a_missing_import_target_does_not_abort_the_sheet`, with no edit to any test above:

```rust
    use super::{ColorScheme, Contrast, MediaEnv, MediaQuery};

    #[test]
    fn keyframes_are_collected_and_do_not_become_rules() {
        let sheet = parse_stylesheet(
            "@keyframes spin { to { transform: rotate(1turn); } }\n\
             button { color: red }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.keyframes.len(), 1);
        assert_eq!(sheet.keyframes[0].name, "spin");
        assert_eq!(sheet.keyframes[0].frames.len(), 1);
        assert_eq!(sheet.keyframes[0].frames[0].0, vec![1.0]);
        assert_eq!(
            decl(&sheet.keyframes[0].frames[0].1, "transform"),
            "rotate(1turn)"
        );
    }

    #[test]
    fn keyframe_selectors_accept_from_to_percentages_and_comma_lists() {
        let sheet = parse_stylesheet(
            "@keyframes fade { from { opacity: 0 } 25%, 75% { opacity: 0.5 } to { opacity: 1 } }",
        );
        let frames = &sheet.keyframes[0].frames;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].0, vec![0.0]);
        assert_eq!(frames[1].0, vec![0.25, 0.75]);
        assert_eq!(frames[2].0, vec![1.0]);
    }

    #[test]
    fn adwaita_carries_exactly_two_keyframes_rules_and_still_900_style_rules() {
        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(sheet.keyframes.len(), 2);
        let names: Vec<&str> = sheet.keyframes.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"spin"), "{names:?}");
        assert!(names.contains(&"needs_attention"), "{names:?}");
        assert!(sheet.media_blocks.is_empty());
        assert_eq!(sheet.color_definitions.len(), 37);
    }

    #[test]
    fn media_blocks_are_retained_unevaluated() {
        let sheet = parse_stylesheet(
            "@media (prefers-color-scheme: dark) { button { color: white } }\n\
             button { color: black }",
        );
        // The block's rules are NOT spliced into `rules`; the compiler
        // decides, per MediaEnv.
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.media_blocks.len(), 1);
        assert_eq!(sheet.media_blocks[0].rules.len(), 1);
        assert_eq!(
            sheet.media_blocks[0].query,
            MediaQuery::ColorScheme(ColorScheme::Dark)
        );
        // Source order is global, so a splice keeps document order.
        assert!(sheet.media_blocks[0].rules[0].source_order < sheet.rules[0].source_order);
    }

    #[test]
    fn media_queries_evaluate_against_the_environment() {
        let light = MediaEnv::default();
        let dark = MediaEnv { color_scheme: ColorScheme::Dark, contrast: Contrast::NoPreference };
        let hc = MediaEnv { color_scheme: ColorScheme::Light, contrast: Contrast::More };
        assert_eq!(light, MediaEnv { color_scheme: ColorScheme::Light, contrast: Contrast::NoPreference });
        assert!(MediaQuery::ColorScheme(ColorScheme::Light).evaluate(&light));
        assert!(!MediaQuery::ColorScheme(ColorScheme::Light).evaluate(&dark));
        assert!(MediaQuery::Contrast(Contrast::More).evaluate(&hc));
        assert!(!MediaQuery::Contrast(Contrast::More).evaluate(&light));
        // `prefers-reduced-motion: reduce` parses and never matches in M2.
        assert!(!MediaQuery::ReducedMotion.evaluate(&light));
        assert!(!MediaQuery::AlwaysFalse.evaluate(&light));
        let not_dark = MediaQuery::Not(std::rc::Rc::new(MediaQuery::ColorScheme(ColorScheme::Dark)));
        assert!(not_dark.evaluate(&light));
        let both = MediaQuery::And(std::rc::Rc::from(vec![
            MediaQuery::ColorScheme(ColorScheme::Light),
            MediaQuery::Contrast(Contrast::NoPreference),
        ]));
        assert!(both.evaluate(&light));
        let either = MediaQuery::Or(std::rc::Rc::from(vec![
            MediaQuery::ColorScheme(ColorScheme::Dark),
            MediaQuery::Contrast(Contrast::NoPreference),
        ]));
        assert!(either.evaluate(&light));
    }

    #[test]
    fn the_media_prelude_grammar_covers_gtks_features_and_combinators() {
        let query = |text: &str| {
            let css = format!("@media {text} {{ button {{ color: red }} }}");
            parse_stylesheet(&css).media_blocks.first().map(|block| block.query.clone())
        };
        assert_eq!(query("(prefers-color-scheme: light)"), Some(MediaQuery::ColorScheme(ColorScheme::Light)));
        assert_eq!(query("(PREFERS-CONTRAST: more)"), Some(MediaQuery::Contrast(Contrast::More)));
        assert_eq!(query("(prefers-reduced-motion: reduce)"), Some(MediaQuery::ReducedMotion));
        assert_eq!(
            query("(prefers-reduced-motion: no-preference)"),
            Some(MediaQuery::Not(std::rc::Rc::new(MediaQuery::ReducedMotion)))
        );
        assert!(matches!(query("not (prefers-color-scheme: dark)"), Some(MediaQuery::Not(_))));
        assert!(matches!(
            query("(prefers-color-scheme: dark) and (prefers-contrast: more)"),
            Some(MediaQuery::And(_))
        ));
        assert!(matches!(
            query("(prefers-color-scheme: dark), (prefers-contrast: more)"),
            Some(MediaQuery::Or(_))
        ));
        // An unknown feature parses and never matches, so its block is
        // fully parsed and then simply never applies.
        assert_eq!(query("(min-width: 100px)"), Some(MediaQuery::AlwaysFalse));
    }

    #[test]
    fn appending_a_layer_carries_keyframes_and_media_blocks_too() {
        let mut base = parse_stylesheet("button { color: red }");
        let layer = parse_stylesheet(
            "@keyframes spin { to { opacity: 0 } }\n\
             @media (prefers-contrast: more) { button { color: white } }\n\
             button { color: blue }",
        );
        base.append_layer(layer);
        assert_eq!(base.rules.len(), 2);
        assert_eq!(base.keyframes.len(), 1);
        assert_eq!(base.media_blocks.len(), 1);
        // The layered rule must sort after the base rule.
        assert!(base.rules[1].source_order > base.rules[0].source_order);
        assert!(base.media_blocks[0].rules[0].source_order > base.rules[0].source_order);
    }

    #[test]
    fn a_malformed_at_rule_still_does_not_abort_the_sheet() {
        let sheet = parse_stylesheet(
            "@keyframes { to { opacity: 0 } }\n\
             @media { button { color: white } }\n\
             button { color: black }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert!(sheet.keyframes.is_empty());
        assert!(sheet.media_blocks.is_empty());
    }
```

Mutation check: `media_blocks_are_retained_unevaluated` fails if `@media` rules are
spliced into `rules` at parse time, which would make the Part 6 gate unable to
compile the same sheet under three environments.
`adwaita_carries_exactly_two_keyframes_rules_and_still_900_style_rules` fails if
`@keyframes` bodies leak into `rules`, which would move the pinned 900.

Note: the existing M1 test `an_unparseable_rule_does_not_abort_the_sheet` uses
`@media (min-width: 100px) { button { color: red } }` and asserts
`sheet.rules.len() == 1`. That stays true and byte-identical: `min-width` is an
unknown media feature, so the block becomes `MediaQuery::AlwaysFalse` and its rules
land in `media_blocks`, never in `rules`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::parse`
Expected: FAIL — `error[E0432]: unresolved import 'super::MediaEnv'`.

- [ ] **Step 3: Write minimal implementation**

In `ui/src/css/parse.rs`, add `use std::rc::Rc;` to the imports, then:

Replace the `Stylesheet` definition with:

```rust
/// The viewer state `@media` queries evaluate against.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MediaEnv {
    /// `prefers-color-scheme`.
    pub color_scheme: ColorScheme,
    /// `prefers-contrast`.
    pub contrast: Contrast,
}

impl Default for MediaEnv {
    fn default() -> Self {
        MediaEnv { color_scheme: ColorScheme::Light, contrast: Contrast::NoPreference }
    }
}

/// `prefers-color-scheme`'s two values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    /// `light`.
    Light,
    /// `dark`.
    Dark,
}

/// `prefers-contrast`'s three values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Contrast {
    /// `no-preference`.
    NoPreference,
    /// `more`.
    More,
    /// `less`.
    Less,
}

/// A parsed media condition.
///
/// `prefers-reduced-motion` is a real GTK 4.20+ feature; it parses here and
/// always evaluates **false**, because `MediaEnv` does not model it in M2.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaQuery {
    /// `(prefers-color-scheme: ...)`.
    ColorScheme(ColorScheme),
    /// `(prefers-contrast: ...)`.
    Contrast(Contrast),
    /// `(prefers-reduced-motion: reduce)`.
    ReducedMotion,
    /// `not <query>`.
    Not(Rc<MediaQuery>),
    /// `<query> and <query> ...`.
    And(Rc<[MediaQuery]>),
    /// `<query>, <query> ...` / `<query> or <query>`.
    Or(Rc<[MediaQuery]>),
    /// An unknown feature: parses, never matches.
    AlwaysFalse,
}

impl MediaQuery {
    /// Whether this query matches `env`.
    #[must_use]
    pub fn evaluate(&self, env: &MediaEnv) -> bool {
        match self {
            MediaQuery::ColorScheme(scheme) => env.color_scheme == *scheme,
            MediaQuery::Contrast(contrast) => env.contrast == *contrast,
            MediaQuery::ReducedMotion => false,
            MediaQuery::Not(inner) => !inner.evaluate(env),
            MediaQuery::And(list) => list.iter().all(|query| query.evaluate(env)),
            MediaQuery::Or(list) => list.iter().any(|query| query.evaluate(env)),
            MediaQuery::AlwaysFalse => false,
        }
    }
}

/// One `@media` block, kept **unevaluated** so a single parse can be
/// compiled under several environments.
#[derive(Clone, Debug)]
pub struct MediaBlock {
    /// The block's condition.
    pub query: MediaQuery,
    /// Its qualified rules, numbered in the outer sheet's source order.
    pub rules: Vec<StyleRule>,
    /// Its `@keyframes`.
    pub keyframes: Vec<KeyframesRule>,
    /// Its `@define-color`s.
    pub color_definitions: Vec<(String, String)>,
}

/// One `@keyframes` rule, with raw declarations; compiled in `cascade.rs`.
#[derive(Clone, Debug)]
pub struct KeyframesRule {
    /// The animation name.
    pub name: String,
    /// `(offsets in 0..=1, declarations)`, in source order.
    pub frames: Vec<(Vec<f32>, Vec<Declaration>)>,
    /// Position in the sheet; the cascade's last-definition-wins tiebreak.
    pub source_order: usize,
}

/// A parsed stylesheet.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Qualified rules, in source order (imports spliced in at their site).
    pub rules: Vec<StyleRule>,
    /// `(name, value)` for every `@define-color`, in source order.
    pub color_definitions: Vec<(String, String)>,
    /// Every `@keyframes`, in source order.
    pub keyframes: Vec<KeyframesRule>,
    /// Every `@media` block, unevaluated.
    pub media_blocks: Vec<MediaBlock>,
}

impl Stylesheet {
    /// Append `layer` after `self`, renumbering so the later layer wins
    /// every source-order tie.
    pub fn append_layer(&mut self, layer: Stylesheet) {
        let offset = next_source_order(self);
        let mut base = layer;
        for rule in &mut base.rules {
            rule.source_order += offset;
        }
        for keyframes in &mut base.keyframes {
            keyframes.source_order += offset;
        }
        for block in &mut base.media_blocks {
            for rule in &mut block.rules {
                rule.source_order += offset;
            }
            for keyframes in &mut block.keyframes {
                keyframes.source_order += offset;
            }
        }
        self.rules.extend(base.rules);
        self.color_definitions.extend(base.color_definitions);
        self.keyframes.extend(base.keyframes);
        self.media_blocks.extend(base.media_blocks);
    }
}

/// The next unused source-order slot in `sheet`.
///
/// Source order is global across top-level rules, `@keyframes` and the
/// rules inside `@media` blocks, so splicing a matching block back in at
/// compile time restores document order exactly.
fn next_source_order(sheet: &Stylesheet) -> usize {
    let rules = sheet.rules.iter().map(|rule| rule.source_order + 1).max().unwrap_or(0);
    let keyframes = sheet
        .keyframes
        .iter()
        .map(|frame| frame.source_order + 1)
        .max()
        .unwrap_or(0);
    let nested = sheet
        .media_blocks
        .iter()
        .flat_map(|block| {
            block
                .rules
                .iter()
                .map(|rule| rule.source_order + 1)
                .chain(block.keyframes.iter().map(|frame| frame.source_order + 1))
        })
        .max()
        .unwrap_or(0);
    rules.max(keyframes).max(nested)
}
```

Extend `SheetItem` and `AtPrelude`:

```rust
enum SheetItem {
    Rule(StyleRule),
    Color(String, String),
    Import(String),
    Keyframes(KeyframesRule),
    Media(MediaBlock),
    Ignored,
}

enum AtPrelude {
    Color(String, String),
    Import(String),
    Keyframes(String),
    Media(MediaQuery),
}
```

Extend `SheetParser`'s `AtRuleParser::parse_prelude`, before its
`define-color` branch:

```rust
        if name.eq_ignore_ascii_case("keyframes") {
            let animation = input.expect_ident().map_err(|_| input.new_custom_error::<(), ()>(()))?;
            let animation = animation.as_ref().to_string();
            input.skip_whitespace();
            if !input.is_exhausted() {
                return Err(input.new_custom_error(()));
            }
            return Ok(AtPrelude::Keyframes(animation));
        }
        if name.eq_ignore_ascii_case("media") {
            let query = parse_media_query_list(input)
                .map_err(|()| input.new_custom_error::<(), ()>(()))?;
            return Ok(AtPrelude::Media(query));
        }
```

Replace `SheetParser`'s `AtRuleParser::parse_block` with:

```rust
    fn parse_block<'t>(
        &mut self,
        prelude: AtPrelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<SheetItem, ParseError<'i, ()>> {
        match prelude {
            AtPrelude::Keyframes(name) => {
                let frames = parse_keyframe_list(input);
                Ok(SheetItem::Keyframes(KeyframesRule { name, frames, source_order: 0 }))
            }
            AtPrelude::Media(query) => {
                let mut nested = Stylesheet::default();
                let mut sheet_parser = SheetParser;
                for item in StyleSheetParser::new(input, &mut sheet_parser) {
                    match item {
                        Ok(SheetItem::Rule(rule)) => nested.rules.push(rule),
                        Ok(SheetItem::Color(name, value)) => {
                            nested.color_definitions.push((name, value));
                        }
                        Ok(SheetItem::Keyframes(frames)) => nested.keyframes.push(frames),
                        Ok(SheetItem::Media(_)) => {
                            tracing::debug!("nested @media is not modelled; dropping the block");
                        }
                        Ok(SheetItem::Import(url)) => {
                            tracing::debug!(%url, "@import inside @media; skipping");
                        }
                        Ok(SheetItem::Ignored) => {}
                        Err((err, slice)) => {
                            tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule inside @media");
                        }
                    }
                }
                Ok(SheetItem::Media(MediaBlock {
                    query,
                    rules: nested.rules,
                    keyframes: nested.keyframes,
                    color_definitions: nested.color_definitions,
                }))
            }
            // `@import`/`@define-color` carry no block, so a block means the
            // rule was malformed: the definition is *not* recorded.
            AtPrelude::Color(_, _) | AtPrelude::Import(_) => {
                while input.next().is_ok() {}
                Ok(SheetItem::Ignored)
            }
        }
    }
```

Add the two new prelude parsers and the keyframe rule-list parser:

```rust
/// `<media-query-list>`, restricted to the features GTK documents.
fn parse_media_query_list(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let mut queries = vec![parse_media_condition(input)?];
    while input.expect_comma().is_ok() {
        queries.push(parse_media_condition(input)?);
    }
    input.skip_whitespace();
    if !input.is_exhausted() {
        return Err(());
    }
    Ok(if queries.len() == 1 {
        queries.remove(0)
    } else {
        MediaQuery::Or(queries.into())
    })
}

fn parse_media_condition(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let state = input.state();
    let negated = matches!(
        input.next(),
        Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("not")
    );
    if negated {
        return Ok(MediaQuery::Not(Rc::new(parse_media_condition(input)?)));
    }
    input.reset(&state);
    let first = parse_media_in_parens(input)?;
    let mut combinator: Option<bool> = None; // Some(true) == `and`
    let mut operands = vec![first];
    loop {
        let state = input.state();
        let keyword = match input.next() {
            Ok(Token::Ident(name)) => Some(name.as_ref().to_ascii_lowercase()),
            _ => None,
        };
        let Some(keyword) = keyword else {
            input.reset(&state);
            break;
        };
        let is_and = if keyword == "and" {
            true
        } else if keyword == "or" {
            false
        } else {
            input.reset(&state);
            break;
        };
        if combinator.is_some_and(|previous| previous != is_and) {
            return Err(());
        }
        combinator = Some(is_and);
        operands.push(parse_media_in_parens(input)?);
    }
    Ok(match combinator {
        None => operands.remove(0),
        Some(true) => MediaQuery::And(operands.into()),
        Some(false) => MediaQuery::Or(operands.into()),
    })
}

fn parse_media_in_parens(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    input.expect_parenthesis_block().map_err(|_| ())?;
    input
        .parse_nested_block(|inner| match parse_media_feature(inner) {
            Ok(query) => Ok(query),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: ParseError<'_, ()>| ())
}

fn parse_media_feature(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    // `( <condition> )` nests rather than naming a feature.
    let state = input.state();
    if let Ok(nested) = parse_media_condition(input) {
        input.skip_whitespace();
        if input.is_exhausted() {
            return Ok(nested);
        }
    }
    input.reset(&state);
    let feature = input.expect_ident().map_err(|_| ())?.as_ref().to_ascii_lowercase();
    input.expect_colon().map_err(|_| ())?;
    let value = input.expect_ident().map_err(|_| ())?.as_ref().to_ascii_lowercase();
    input.skip_whitespace();
    if !input.is_exhausted() {
        return Err(());
    }
    Ok(match (feature.as_str(), value.as_str()) {
        ("prefers-color-scheme", "light") => MediaQuery::ColorScheme(ColorScheme::Light),
        ("prefers-color-scheme", "dark") => MediaQuery::ColorScheme(ColorScheme::Dark),
        ("prefers-contrast", "no-preference") => MediaQuery::Contrast(Contrast::NoPreference),
        ("prefers-contrast", "more") => MediaQuery::Contrast(Contrast::More),
        ("prefers-contrast", "less") => MediaQuery::Contrast(Contrast::Less),
        // GTK 4.20+ supports this feature; M2's MediaEnv does not model it,
        // so `reduce` never matches and `no-preference` always does.
        ("prefers-reduced-motion", "reduce" | "reduced") => MediaQuery::ReducedMotion,
        ("prefers-reduced-motion", "no-preference") => {
            MediaQuery::Not(Rc::new(MediaQuery::ReducedMotion))
        }
        _ => MediaQuery::AlwaysFalse,
    })
}

/// The `<keyframe-selector>#` prelude: `from`, `to` or `<percentage>`.
struct KeyframeListParser;

impl<'i> QualifiedRuleParser<'i> for KeyframeListParser {
    type Prelude = Vec<f32>;
    type QualifiedRule = (Vec<f32>, Vec<Declaration>);
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Vec<f32>, ParseError<'i, ()>> {
        let mut offsets = Vec::new();
        loop {
            let token = input.next()?.clone();
            let offset = match token {
                Token::Percentage { unit_value, .. } if unit_value.is_finite() => unit_value,
                Token::Ident(ref name) if name.eq_ignore_ascii_case("from") => 0.0,
                Token::Ident(ref name) if name.eq_ignore_ascii_case("to") => 1.0,
                _ => return Err(input.new_custom_error(())),
            };
            offsets.push(offset);
            if input.expect_comma().is_err() {
                break;
            }
        }
        input.skip_whitespace();
        if !input.is_exhausted() {
            return Err(input.new_custom_error(()));
        }
        Ok(offsets)
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Vec<f32>,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(Vec<f32>, Vec<Declaration>), ParseError<'i, ()>> {
        let mut block = DeclarationBlockParser;
        let declarations: Vec<Declaration> = RuleBodyParser::<_, _, ()>::new(input, &mut block)
            .flatten()
            .flatten()
            .collect();
        Ok((prelude, declarations))
    }
}

impl<'i> AtRuleParser<'i> for KeyframeListParser {
    type Prelude = ();
    type AtRule = (Vec<f32>, Vec<Declaration>);
    type Error = ();
}

fn parse_keyframe_list(input: &mut Parser<'_, '_>) -> Vec<(Vec<f32>, Vec<Declaration>)> {
    let mut parser = KeyframeListParser;
    StyleSheetParser::new(input, &mut parser)
        .filter_map(|item| match item {
            Ok(frame) => Some(frame),
            Err((err, slice)) => {
                tracing::debug!(?err, frame = %slice.chars().take(80).collect::<String>(), "skipping invalid keyframe");
                None
            }
        })
        .collect()
}
```

Finally, replace `parse_into`'s match body so every item takes a global source-order
slot:

```rust
    for item in StyleSheetParser::new(&mut parser, &mut sheet_parser) {
        match item {
            Ok(SheetItem::Rule(mut rule)) => {
                rule.source_order = next_source_order(out);
                out.rules.push(rule);
            }
            Ok(SheetItem::Color(name, value)) => out.color_definitions.push((name, value)),
            Ok(SheetItem::Import(url)) => resolve_import(&url, base_dir, depth, visited, out),
            Ok(SheetItem::Keyframes(mut frames)) => {
                frames.source_order = next_source_order(out);
                out.keyframes.push(frames);
            }
            Ok(SheetItem::Media(mut block)) => {
                for rule in &mut block.rules {
                    rule.source_order = next_source_order(out) + rule.source_order;
                }
                let base = next_source_order(out);
                for (index, keyframes) in block.keyframes.iter_mut().enumerate() {
                    keyframes.source_order = base + block.rules.len() + index;
                }
                out.media_blocks.push(block);
            }
            Ok(SheetItem::Ignored) => {}
            Err((err, slice)) => {
                tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule");
            }
        }
    }
```

The `Media` arm's first loop must compute the base **once**, before renumbering, or
each rule shifts the next one; write it as:

```rust
            Ok(SheetItem::Media(mut block)) => {
                let base = next_source_order(out);
                for (index, rule) in block.rules.iter_mut().enumerate() {
                    rule.source_order = base + index;
                }
                for (index, keyframes) in block.keyframes.iter_mut().enumerate() {
                    keyframes.source_order = base + block.rules.len() + index;
                }
                out.media_blocks.push(block);
            }
```

Also delete the scratch integration test created in Task 1:

```bash
git rm ui/tests/part1_foundation.rs
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::parse`
Expected: PASS — the 16 M1 tests unchanged plus 8 new ones.
Run: `cargo test -p icedtea-ui` — Expected: PASS, including
`cascade::every_adwaita_rule_compiles` at **900** and
`app::the_bundled_source_is_exactly_the_vendored_sheet` at **900**.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/parse.rs
git commit -m "feat(ui/css): @keyframes and @media in the stylesheet parser

cssparser has no grammar for either, so both preludes are project code.
@media blocks are retained UNEVALUATED so one parse compiles under
several MediaEnvs, which is what the coverage gate's light/dark/hc runs
need. Source order is now global across rules, keyframes and media-block
rules, so splicing a matching block back in restores document order.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 18: Compiled `Keyframes`

The last Part 1 deliverable: the compiled form Part 3's `CompiledSheet` stores and
Part 5's animation engine samples.

**Files:**
- Create: `ui/src/css/value/keyframes.rs`
- Modify: `ui/src/css/value/mod.rs`
- Test: `ui/src/css/value/keyframes.rs`

**Interfaces:**
- Consumes: `Prop`, `registry::parse_declaration_value`, `Prop::expand_into`,
  `parse::KeyframesRule`, `TimingFunction`.
- Produces:
  - `css::value::keyframes::{Keyframe, Keyframes}`
  - `Keyframes::compile(rule: &KeyframesRule) -> Keyframes` (contract deviation 7)
  - `Keyframes::properties(&self) -> Vec<Prop>`
  - `Keyframes::segment(&self, prop: Prop, t: f32) -> Option<(&Value, &Value, f32, TimingFunction)>`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/value/keyframes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Keyframes;
    use crate::css::parse::parse_stylesheet;
    use crate::css::registry::Prop;
    use crate::css::value::timing::TimingFunction;
    use crate::css::value::Value;

    fn compiled(css: &str) -> Keyframes {
        let sheet = parse_stylesheet(css);
        Keyframes::compile(&sheet.keyframes[0])
    }

    #[test]
    fn compilation_expands_shorthands_and_drops_unknown_properties() {
        let frames = compiled(
            "@keyframes fade { from { padding: 4px; nonsense: 1 } to { padding: 8px } }",
        );
        assert_eq!(frames.name.as_ref(), "fade");
        assert_eq!(frames.frames.len(), 2);
        let properties = frames.properties();
        assert!(properties.contains(&Prop::PaddingTop));
        assert!(properties.contains(&Prop::PaddingLeft));
        assert!(!properties.iter().any(|prop| prop.name() == "nonsense"));
        // properties() is deduped and in registry order.
        let mut sorted = properties.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(properties, sorted);
    }

    #[test]
    fn a_segment_brackets_the_requested_progress() {
        let frames = compiled("@keyframes fade { 0% { opacity: 0 } 50% { opacity: 1 } 100% { opacity: 0 } }");
        let (from, to, local, timing) = frames
            .segment(Prop::Opacity, 0.25)
            .expect("0.25 lies inside the first segment");
        assert_eq!(*from, Value::Number(0.0));
        assert_eq!(*to, Value::Number(1.0));
        assert!((local - 0.5).abs() < 1e-6, "{local}");
        assert_eq!(timing, TimingFunction::Linear);
        let (from, to, local, _) = frames.segment(Prop::Opacity, 0.75).expect("second segment");
        assert_eq!(*from, Value::Number(1.0));
        assert_eq!(*to, Value::Number(0.0));
        assert!((local - 0.5).abs() < 1e-6, "{local}");
        // A property no frame mentions has no segment.
        assert!(frames.segment(Prop::Color, 0.5).is_none());
        // Clamped at the ends.
        assert!(frames.segment(Prop::Opacity, 0.0).is_some());
        assert!(frames.segment(Prop::Opacity, 1.0).is_some());
    }

    #[test]
    fn a_per_keyframe_timing_function_wins_for_its_segment() {
        let frames = compiled(
            "@keyframes fade { 0% { opacity: 0; animation-timing-function: ease-in } 100% { opacity: 1 } }",
        );
        let (_, _, _, timing) = frames.segment(Prop::Opacity, 0.5).expect("segment exists");
        assert_eq!(timing, TimingFunction::EASE_IN);
        // The timing declaration is not itself an animated property.
        assert!(!frames.properties().contains(&Prop::AnimationTimingFunction));
    }

    #[test]
    fn adwaitas_own_keyframes_compile() {
        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        for rule in &sheet.keyframes {
            let compiled = Keyframes::compile(rule);
            assert!(
                !compiled.frames.is_empty(),
                "{} compiled to nothing",
                rule.name
            );
        }
    }

    #[test]
    fn compilation_never_panics_on_hostile_frames() {
        for input in crate::css::value::FUZZ_INPUTS {
            let css = format!("@keyframes k {{ {input} {{ {input}: {input} }} }}");
            let sheet = parse_stylesheet(&css);
            for rule in &sheet.keyframes {
                let compiled = Keyframes::compile(rule);
                let _ = compiled.properties();
                for t in [-1.0_f32, 0.0, 0.5, 1.0, 2.0] {
                    let _ = compiled.segment(Prop::Opacity, t);
                }
            }
        }
    }
}
```

Mutation check: `a_segment_brackets_the_requested_progress` fails if the local
progress is not renormalized to the segment (Part 5's `alternate` and
`fill-mode: forwards` tests all read through it).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::value::keyframes`
Expected: FAIL — `error[E0583]: file not found for module 'keyframes'`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod keyframes;` and `pub use keyframes::{Keyframe, Keyframes};` to
`ui/src/css/value/mod.rs`, then create `ui/src/css/value/keyframes.rs`:

```rust
//! Compiled `@keyframes`.

use std::rc::Rc;

use cssparser::{Parser, ParserInput};

use crate::css::parse::KeyframesRule;
use crate::css::registry::{Prop, lookup, parse_declaration_value};

use super::timing::TimingFunction;
use super::Value;

/// One keyframe: the offsets it applies at, its longhand declarations, and
/// the timing function that governs the segment starting at it.
#[derive(Clone, Debug)]
pub struct Keyframe {
    /// Offsets in `0..=1`; `from` == `0.0`, `to` == `1.0`.
    pub offsets: Rc<[f32]>,
    /// Longhands only -- shorthands are pre-expanded.
    pub declarations: Rc<[(Prop, Value)]>,
    /// Per-keyframe `animation-timing-function`.
    pub timing: Option<TimingFunction>,
}

/// A compiled `@keyframes` rule.
#[derive(Clone, Debug)]
pub struct Keyframes {
    /// The animation name.
    pub name: Rc<str>,
    /// The frames, in ascending offset order.
    pub frames: Rc<[Keyframe]>,
}

impl Keyframes {
    /// Compile a parsed rule: expand shorthands, parse values, drop
    /// unknown properties and unparseable declarations (logged), and sort
    /// the frames by offset.
    #[must_use]
    pub fn compile(rule: &KeyframesRule) -> Keyframes {
        let mut frames: Vec<Keyframe> = Vec::with_capacity(rule.frames.len());
        for (offsets, declarations) in &rule.frames {
            let mut resolved: Vec<(Prop, Value)> = Vec::new();
            let mut timing = None;
            for declaration in declarations {
                let Some(prop) = lookup(&declaration.name) else {
                    tracing::debug!(
                        name = %declaration.name,
                        "unknown property in @keyframes; dropping"
                    );
                    continue;
                };
                if prop == Prop::AnimationTimingFunction {
                    let mut source = ParserInput::new(&declaration.value);
                    let mut parser = Parser::new(&mut source);
                    if let Ok(parsed) = TimingFunction::parse(&mut parser) {
                        timing = Some(parsed);
                    }
                    continue;
                }
                if prop.is_longhand() {
                    match parse_declaration_value(prop, &declaration.value) {
                        Ok(value) => resolved.push((prop, value)),
                        Err(()) => tracing::debug!(
                            name = %declaration.name,
                            value = %declaration.value,
                            "unparseable declaration in @keyframes; dropping"
                        ),
                    }
                    continue;
                }
                let mut source = ParserInput::new(&declaration.value);
                let mut parser = Parser::new(&mut source);
                let mut sink = |longhand: Prop, value: Value| resolved.push((longhand, value));
                if prop.expand_into(&mut parser, &mut sink).is_err() {
                    tracing::debug!(
                        name = %declaration.name,
                        value = %declaration.value,
                        "unexpandable shorthand in @keyframes; dropping"
                    );
                }
            }
            let mut offsets: Vec<f32> = offsets
                .iter()
                .copied()
                .filter(|offset| offset.is_finite())
                .map(|offset| offset.clamp(0.0, 1.0))
                .collect();
            offsets.sort_by(f32::total_cmp);
            if offsets.is_empty() {
                continue;
            }
            frames.push(Keyframe {
                offsets: offsets.into(),
                declarations: resolved.into(),
                timing,
            });
        }
        frames.sort_by(|a, b| a.offsets[0].total_cmp(&b.offsets[0]));
        Keyframes { name: Rc::from(rule.name.as_str()), frames: frames.into() }
    }

    /// Every longhand any frame mentions, deduped, in registry order.
    #[must_use]
    pub fn properties(&self) -> Vec<Prop> {
        let mut props: Vec<Prop> = self
            .frames
            .iter()
            .flat_map(|frame| frame.declarations.iter().map(|(prop, _)| *prop))
            .collect();
        props.sort_unstable();
        props.dedup();
        props
    }

    /// The two frames bracketing `t` for `prop`, the local `0..1` progress
    /// between them, and the segment's timing function.
    ///
    /// `None` when fewer than two frames declare `prop`.
    #[must_use]
    pub fn segment(&self, prop: Prop, t: f32) -> Option<(&Value, &Value, f32, TimingFunction)> {
        // Flatten to (offset, value, timing) for every offset a frame that
        // declares `prop` applies at.
        let mut points: Vec<(f32, &Value, Option<TimingFunction>)> = Vec::new();
        for frame in self.frames.iter() {
            let Some((_, value)) = frame.declarations.iter().find(|(name, _)| *name == prop) else {
                continue;
            };
            for offset in frame.offsets.iter() {
                points.push((*offset, value, frame.timing));
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        if points.len() < 2 {
            return None;
        }
        let t = t.clamp(points[0].0, points[points.len() - 1].0);
        for window in 0..points.len() - 1 {
            let (start, from, timing) = points[window];
            let (end, to, _) = points[window + 1];
            if t >= start && t <= end {
                let local = if (end - start).abs() <= f32::EPSILON {
                    1.0
                } else {
                    (t - start) / (end - start)
                };
                return Some((from, to, local, timing.unwrap_or(TimingFunction::Linear)));
            }
        }
        let last = points.len() - 1;
        Some((
            points[last - 1].1,
            points[last].1,
            1.0,
            points[last - 1].2.unwrap_or(TimingFunction::Linear),
        ))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib css::value::keyframes` — Expected: PASS, 5 tests.
Run: `cargo test -p icedtea-ui` — Expected: PASS (whole crate).
Run: `cargo test --workspace` — Expected: PASS.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — Expected: clean.
Run: `cargo fmt --all --check` — Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/value/mod.rs ui/src/css/value/keyframes.rs
git commit -m "feat(ui/css): compiled @keyframes with per-property segments

Shorthands are pre-expanded, unknown and unparseable declarations are
dropped and logged, and segment() renormalizes progress to the bracketing
pair so Part 5's animation engine reads one number.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review

### 1. Spec coverage

Only the spec bullets Part 1 owns are listed; the rest belong to Parts 2–6 and
are named with their owning part.

| Spec / contract requirement | Task |
|---|---|
| §1 module map: `registry.rs` | 12 (stub), 14 (`Prop`), 16 (`PROPERTIES`) |
| §1 module map: `css/tokens.rs` (M1 `value.rs`, moved verbatim) | 1 |
| §1 module map: `value/{length,calc}.rs` | 3 |
| §1 module map: `value/color.rs` | 4, 5 |
| §1 module map: `value/image.rs` | 6, 7 |
| §1 module map: `value/{shadow,border}.rs` | 8 |
| §1 module map: `value/{font,text}.rs` | 9 |
| §1 module map: `value/{transform,filter}.rs` | 10 |
| §1 module map: `value/timing.rs` | 3 (`Time`), 11 (easings) |
| §1 module map: `value/keyword.rs` (`inherit/initial/unset`) | 2, 12 |
| §1 module map: `value/interpolate.rs` | 13 |
| §1 module map: `value/keyframes.rs` | 18 |
| §1 `PropertyDef { Longhand { parse, initial, inherited, animatable } \| Shorthand { expand } }` | 14, 16 |
| §1 "nothing outside `registry.rs` names a property string" | 14 (`Prop::name`), 16 (`parse_declaration_value`) |
| §3 rows: colours, fonts/text, text-decoration, icons, filter/transform, box model, borders, outlines, backgrounds, transitions, animations, `border-spacing` | 16 (all 113 rows, asserted by `the_table_agrees_with_the_enum_on_every_row`) |
| §3 inherited flags follow CSS/GTK | 16 (`the_inherited_flags_follow_gtk`) |
| §3 `Length` (px pt em rem ex %, `calc()`, percentages only where allowed) | 3 |
| §3 `Color` (hex, rgb/rgba/hsl/hwb comma+space, named, `currentColor`, `transparent`, `@name`, `color-mix()`, relative `rgb(from …)`/`hsl(from …)`, legacy `alpha/shade/mix/lighter/darker`) | 4, 5 |
| §3 `Image` (`none`, `url()`, `image(color)`, linear/radial/conic + `repeating-`, `cross-fade()`, `-gtk-*` → `IconRef`) | 6, 7 |
| §3 `Shadow` lists, `BorderImage`, font enums, `Transform`, `Filter`, `TimingFunction`, `Time`, `Keyframes` | 8, 9, 10, 11, 18 |
| §3 wide keywords on every property | 12, 16 (`every_longhand_accepts_the_wide_keywords`) |
| §3 every parser cssparser-token based, ASCII-CI, whitespace-insensitive, never panics | every value task's `*_never_panics` battery |
| §3 shorthands expanded with the shorthand's key (mechanism), expansion via `ExpandFn` | 15, 16 |
| §3 `@define-color` resolves lazily and cycle-guarded; forward references allowed | 5 |
| §3 gate consequence: 37/37 `@define-color`s resolve | 5 (`relative_color_syntax_resolves`) |
| §4 `MediaEnv` / `MediaQuery` / `MediaBlock` / `KeyframesRule` / `Stylesheet` fields | 17 |
| §4 `@media` retained unevaluated so one parse compiles under several envs | 17 |
| §5 interpolation is the registry row's `animatable` fn; per-type interpolators | 13, 16 |
| §5 timing functions `linear`, `ease*`, `steps()`, `cubic-bezier()` (Newton–Raphson) | 11 |
| §5 `@keyframes` → per-node animation source | 17 (raw), 18 (compiled) |
| Contract "crate deps to add" (`fontconfig`, `bitflags`, skia `codec`/`codec-png`/`svg`) | 1 |
| Contract §10.2 gate: `parse.rs`'s 16 and `tokens.rs`'s 4 stay byte-identical | 1, 17 |
| Contract §10.3: `colors.rs`'s 10 tests migrate (9 verbatim, 1 inverted) | 4 (7), 5 (3) |
| Contract §10.3: `shorthand.rs`'s 9 tests migrate against `Prop` | 15 |
| §2 node tree, `Element` impl, buckets, bloom | **Part 2** |
| §3 cascade / `CascadedValues` / `ComputedStyle` / inheritance / invalid-at-computed-value-time | **Part 3** |
| §4 layout and paint | **Part 4** |
| §5 clock, transitions, animation state machine | **Part 5** |
| §6 fonts, §7 coverage gate instruments | **Part 6** |

No Part 1 requirement is unassigned.

### 2. Placeholder scan

- No `TBD`, `TODO`, `FIXME`, "implement later" or "fill in details" anywhere.
- No "similar to Task N": every task repeats the code it needs. Task 8's
  `four_sides`, Task 15's `box_sides` and Task 16's `keyword_in` are each written
  out once and imported, not paraphrased.
- Every code step is a real code block; there are no prose-only implementation
  steps.
- Two places deliberately show a wrong line and then correct it, because the
  correction is the point: Task 2's `"a".repeat(0).as_str()` corpus entry (not
  const-evaluable → replaced with a literal), Task 7's `SideOrCorner::TopRight`
  arm (written as an identity → replaced with the corner angle), and Task 17's
  `Media` arm (recomputing the base per rule → replaced with a hoisted base).
  Each names the replacement verbatim.
- Every type named in a later task is defined in an earlier one: `Value`
  (Task 12) is used by Tasks 13, 15, 16, 18; `Prop` (Task 14) by 15, 16, 18;
  `KeyframesRule` (Task 17) by 18. Task 12 creates the `registry.rs` stub that
  holds `ParseFn` precisely so `value/mod.rs` can name it before Task 14.
- Every value parser has a `*_never_panics` test: keyword (2), length + calc +
  time (3), colour (4, 5), image + position (6), shadow + border (8), font +
  text (9), transform + filter (10), timing (11), interpolators (13), shorthand
  expanders (15), and the whole registry (16), plus keyframes compilation (18).
- Every load-bearing test states its mutation check.

### 3. Type consistency vs the contract

Checked name-by-name against contract §1, §2 and §4:

- `ParseFn`, `ExpandFn`, `Interpolate`, `PropertyKind`, `PropertyDef`,
  `PROPERTIES`, `N_LONGHANDS = 95`, `N_PROPS = 113`, `lookup`, `longhands`,
  `animatable_longhands`, `Prop::{def, name, is_longhand, is_inherited, initial,
  interpolator, slot, expand_into, longhands}` — all verbatim.
- `Prop`'s 113 variants are spelled exactly as contract §1.1 lists them, in that
  order; Task 14's `the_discriminants_partition_longhands_from_shorthands` pins
  the boundaries.
- `Value`'s variants match contract §2.1 one for one, including
  `BorderImageWidths([BorderImageWidthSide; 4])` and `Pair(Rc<(Value, Value)>)`.
  `Value::{parse_wide, parse_wide_or, is_wide, parse_list}` keep their signatures.
- `Keyword` is contract §2.2's list **plus** the eleven variants of deviation 3;
  no listed variant was renamed or removed. `ColorBlend` is still CSS `color`.
- `LengthUnit`, `Length`, `CalcNode`, `LengthCtx` match §2.3 field-for-field;
  `CalcNode::Var` is deviation 4 and `Length::parse_allowing_auto` is deviation 5.
  `Length::{parse, resolve, zero, px}` and `CalcNode::{parse, resolve_length,
  resolve_number, resolve_angle, resolve_time}` keep their signatures.
- `Rgba`, `ColorSpace`, `ColorValue`, `ChannelExpr`, `LegacyColorFn`, `ColorTable`,
  `ColorCtx`, `ColorValue::{parse, resolve}`, `build_color_table` match §2.4.
  `Rgba` gains `clamped`, `premultiplied`, `from_premultiplied` (additive).
- `Image`, `Gradient`, `GradientKind`, `LinearDirection`, `SideOrCorner`,
  `RadialShape`, `RadialExtent`, `ColorStop`, `Position`, `IconRef`,
  `Image::parse`, `Gradient::{color_at, line_for_box}` match §2.5;
  `line_for_box` returns `skia_rs_safe::core::Point`, the only `Point` in scope.
- `Shadow`, `Shadow::{parse, parse_text}`, `NumberOrPercent`, `BorderImageSlice`,
  `BorderImageWidthSide`, `RepeatStyle`, `BgSize` match §2.6.
  `RepeatStyle::parse_background` is additive (`background-repeat` needs
  `repeat-x`/`repeat-y`, which `border-image-repeat` must reject).
- `FontFamily`, `GenericFamily`, `FontWeight`, `FontStyle`, `LineHeight`,
  `FeatureSetting`, `VariationSetting`, `FontVariantFlags` (all 30 flags),
  `TextDecorationLines` (all 4 flags) match §2.7 exactly.
- `TransformFn` (all 21 variants), `to_matrix`, `transform_list_matrix`,
  `Decomposed2d`, `decompose_2d`, `recompose_2d`, `FilterFn` (all 10 variants)
  match §2.8.
- `Time`, `StepPosition`, `TimingFunction` (+ the four `EASE*` consts, `parse`,
  `eval`), `AnimationName`, `IterationCount`, `Keyframe`, `Keyframes`,
  `Keyframes::{properties, segment}` match §2.9. `Keyframes::compile` is
  deviation 7.
- All 13 interpolators in §2.10 exist with the exact
  `fn(&Value, &Value, f32) -> Value` shape and the exact names.
- `ColorScheme`, `Contrast`, `MediaEnv` (+ its `Default`), `MediaQuery` (all 7
  variants), `MediaQuery::evaluate`, `MediaBlock`, `KeyframesRule`, `Stylesheet`'s
  four fields, `parse_stylesheet`, `parse_stylesheet_with_base` match §4.
  `Stylesheet::append_layer` is deviation 7.
- Nothing Part 1 produces collides with a Part 2–6 name: `css::value::color::ColorSpace`
  vs `skia_rs_safe::core::ColorSpace` are always module-qualified, and
  `css::value::keyframes::Keyframes` (compiled) is distinct from
  `css::parse::KeyframesRule` (raw) by both name and module.
