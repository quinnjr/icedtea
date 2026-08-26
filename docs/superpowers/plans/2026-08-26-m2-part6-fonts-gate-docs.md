# Pure-Rust GTK-themed UI — M2 Part 6: fontconfig FontDatabase, text properties, the coverage gate, README/spec status — Implementation Plan

> **Note for agentic workers:** this plan is written to be executed with the
> `superpowers:subagent-driven-development` skill — one subagent per task, each
> task self-contained (files, interfaces, failing test, implementation, gate
> command, commit). Do not batch tasks; do not skip the failing-test step. Every
> task ends on a green `cargo test -p icedtea-ui` and a commit.

## Contract deviations

The binding contract is
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`. Every signature in
this plan is copied from it verbatim except for the items below.

1. **`text.rs` may already be non-M1 when P6 starts.** The contract makes P6 the
   owner of `ui/src/text.rs` (§11), but P3 (`widget/button.rs`) and P4
   (`paint/text.rs`) both *consume* `FontDatabase`/`ShapedText` and run
   **before** P6 on the same branch (§11 dependency order, "may stub against
   `probe_only()` until P6 lands"). So by the time this plan runs, `text.rs` is
   either still M1's `FontStack` or an earlier part's minimal placeholder. Every
   task below is written as **Modify** and says "replace whatever body is there",
   and every public signature it produces is contract-exact, so P3/P4 callers
   keep compiling either way. This is a sequencing note, not a signature change.
2. **Additive public items not in the contract** (permitted by the contract's
   "a part may add private items freely"; listed here because they are `pub`):
   `text::TextStyle` (+ `from_computed`, `query`, `shape_key`, `line_height_px`),
   `text::apply_text_transform`, `text::FontDatabase::ex_ratio`, and M1's
   `text::FONT_CANDIDATES` (kept, now used only by `probe_only()`). Nothing in
   the contract is renamed or re-typed by them.
3. **`ui/src/lib.rs` is NOT touched.** The contract's §10.2 gate pins lib.rs's
   single test byte-identical; rather than add `BUNDLED_ADWAITA_DARK`/`_HC`
   consts there, the new vendored sheets are pulled into the gate test with
   `include_str!("../themes/adwaita-dark.css")` from `ui/tests/`. No lib.rs edit
   means no gate risk.
4. **`css::value` re-export assumption.** This plan's `use` lines name value
   types at the module root (`crate::css::value::{Value, Keyword, FontFamily,
   …}`), which assumes P1's `css/value/mod.rs` re-exports its submodules. If P1
   filed them without re-exports, fix only the `use` paths (e.g.
   `crate::css::value::font::FontFamily`) — the type names and shapes are
   unchanged.

## Goal

Close M2's last three obligations:

- **Fonts (spec §6).** Replace M1's fixed 8-path probe with a real
  `text::FontDatabase` over the `fontconfig` 0.11 crate: `fc-match` parity for
  `font-family` lists, weights, styles and widths, with per-pattern,
  per-`(path, index)` and per-shape caches, and `probe_only()` as the
  no-fontconfig fallback.
- **Text properties (spec §6).** Turn a `ComputedStyle` into a font query and a
  shaping key: family list, size, weight, style, width/stretch, letter-spacing,
  `text-transform` (including `full-width`/`full-size-kana`), line-height, and
  the feature/variation settings that this shaper stack cannot honour (warned
  once, never silently dropped).
- **The M2 gate + docs (spec §7).** `tests/adwaita_coverage.rs` — the gate:
  Adwaita 4.22 light **and** dark **and** high-contrast walked declaration by
  declaration through the registry with **0 unparseable declarations, 0 unknown
  properties, 37/37 `@define-color`s resolved** — plus
  `tests/gtk4_property_reference.rs` proving the registry is the GTK 4.22
  property table, and the `ui/README.md` / spec status rewrite that says so.

## Architecture

```
ui/src/text.rs                       [this part, rewritten]
  FONT_CANDIDATES                    M1's 8 paths, now only probe_only()'s list
  css_weight_to_fc / css_stretch_to_fc / css_style_to_fc_slant
                                     CSS <-> fontconfig scale tables (NOT casts)
  FontFace { path, index, family }   what a match resolves to
  FontQuery<'a>                      what a match asks for
  FontDatabase                       Option<Fontconfig> + Shaper + 3 caches
    new / probe_only / has_fontconfig
    match_face  -> FontFace          FcPattern + FcFontMatch (or the probe list)
    typeface / font / ex_ratio       (path,index) -> Arc<Typeface> -> Font
    shape(&ShapeKey) -> Rc<ShapedText>
  apply_text_transform               applied BEFORE shaping
  TextStyle                          ComputedStyle -> FontQuery + ShapeKey

ui/themes/adwaita-dark.css           vendored GTK 4.22 Default-dark.css
ui/themes/adwaita-hc.css             vendored GTK 4.22 Default-hc.css

ui/tests/fixtures/gtk4.22-css-properties.txt   113 rows: name|kind|inherited
ui/tests/gtk4_property_reference.rs  registry == the GTK 4.22 reference table
ui/tests/adwaita_coverage.rs         THE M2 GATE: 0 / 0 / 37 on three sheets
```

The gate instrument is deliberately a *test*, not library code: it uses only
public API (`parse_stylesheet`, `registry::lookup`, `PropertyKind`,
`Prop::expand_into`, `build_color_table`, `ColorValue::resolve`), so a
regression in any of P1–P5 surfaces here as a named unparseable declaration
rather than as a silent fallback.

## Tech Stack

| Concern | Crate / API |
|---|---|
| Font discovery & matching | `fontconfig` 0.11 (`Fontconfig::new`, `Pattern::{new,add_string,add_integer,font_match,filename,face_index,get_string}`, `FC_FAMILY`/`FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/`FC_PIXEL_SIZE` re-exported from the crate root) |
| Typeface / shaping / blobs | `skia-rs-safe` 0.4.0 `text`: `Typeface::from_data`, `Font::new(Arc<Typeface>, size)`, `Font::metrics`, `Shaper::shape_auto`, `TextBlobBuilder::add_positioned_run` |
| CSS values | P1's `css::value` (`Value`, `Keyword`, `FontFamily`, `FontStyle`, `FontWeight`, `LineHeight`, `FeatureSetting`, `VariationSetting`, `Length`, `LengthUnit`) |
| Registry | P1's `css::registry` (`Prop`, `PROPERTIES`, `PropertyKind`, `lookup`, `N_LONGHANDS`, `N_PROPS`) |
| Sheets & colours | P1's `css::parse::{parse_stylesheet, Stylesheet, StyleRule, Declaration, KeyframesRule, MediaBlock}`, `css::value::color::{build_color_table, ColorCtx, ColorValue, Rgba}` |
| Test-side CSS re-parse | `cssparser` 0.37 (`ParserInput`, `Parser`, `Parser::is_exhausted`) — a `[dependencies]` entry, usable from `ui/tests/` |

**Researched facts this part depends on** (from
`.superpowers/m2-plan-notes/`, re-verified on this machine 2026-08-26):

- `fontconfig::Pattern::font_match(&mut self)` calls `FcConfigSubstitute` +
  `FcDefaultSubstitute` itself; callers must not. It returns a `Pattern<'_>`
  borrowing the query pattern, so path/index/family must be copied out before
  either is dropped (taffy-fontconfig.md §2.2).
- Generic families (`sans-serif`, `serif`, `monospace`, …) need **no** special
  API — they resolve through the same `FC_FAMILY` + substitution path
  (taffy-fontconfig.md §2.6). Verified: `fc-match system-ui` → Noto Sans.
- fontconfig's weight/width scales are **not** CSS's; the mapping is a table
  with piecewise-linear interpolation (taffy-fontconfig.md §2.4 & §3.3).
  Verified: `fc-match "DejaVu Sans:weight=200"` → DejaVu Sans **Bold**,
  `weight=80` → Book.
- `probe_only()` has no fontconfig-side equivalent — it is 100% project code
  (taffy-fontconfig.md §2.8).
- `Typeface::from_data(Vec<u8>) -> Option<Typeface>` takes **no face index**:
  a `.ttc` collection index from fontconfig cannot be honoured (skia-rs.md §8
  neighbourhood; re-verified in `~/Projects/skia-rs` at `f839e19`). Keep the
  index in `FontFace` (it is part of the cache identity and of `fc-match`
  parity) and warn once when it is non-zero.
- `font-feature-settings` / `font-variation-settings` cannot reach rustybuzz
  through `skia-rs-text` 0.4.0 (`Shaper::shape` hardcodes an empty feature
  slice; no variation-axis API) — skia-rs.md §8.
- `Font::metrics() -> FontMetrics` carries `x_height` (verified in
  `crates/skia-rs-text/src/font.rs:72`), which is what `LengthCtx::ex_ratio`
  wants.

## Spec

- Design spec:
  `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
  (§6 Fonts & text, §7 Testing strategy & the M2 gate).
- Parent spec:
  `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.
- Binding interface contract:
  `docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§9 Fonts & text,
  §10.2/§10.3 migration gate, §11 P6 boundaries).

## Global Constraints

- **Branch:** `rebuild/pure-rust-gtk-m2`. Commit after every task. Never push,
  never merge, never open a PR — the owner takes that decision.
- **Crate pins (do not bump):** `cssparser 0.37`, `selectors 0.40`, `taffy
  0.14`, `skia-rs-safe 0.4.0`, `wayland-client 0.31`, `fontconfig 0.11`,
  `bitflags 2`. `fontconfig` and the `skia-rs-safe` feature additions are **P1's
  `ui/Cargo.toml` edit**; Task 1 verifies it rather than re-making it.
- **No `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk`, no `smithay`.** Linking system
  `libfontconfig` is explicitly in scope (spec Decision 4).
- **Edition 2024, `rust-version = 1.94`** (workspace-inherited).
- **Gates (all three must pass before every commit):**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
- **Commit trailer** on every commit:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **The M1 pixel gate `ui/tests/themed_button_offscreen.rs` must stay green and
  byte-identical** to what P4 left it as: this part changes no number in it and
  no line of it. The same applies to every file in the contract's §10.2 list
  (`css/parse.rs`'s 16 tests, `css/tokens.rs`'s 4, `shm.rs`'s 11,
  `wayland.rs`'s 11, `lib.rs`'s 1, `tests/layer_shell_screencopy.rs`'s 3, and
  `app.rs`'s **3** byte-identical ones — P3 deviation 3 corrects the contract's
  "4 of 11 rewritten / 7 byte-identical" to **8 of 11 rewritten / 3 byte-identical**;
  see the contract's Execution notes E7). Verify with
  `git diff --stat <base> -- ui/tests/themed_button_offscreen.rs` at Task 12.
- **Execution order.** Parts run **in order 1 → 6 on one branch**, so this part
  may consume everything P1–P5 produced (`css::registry`, `css::value`,
  `css::node`, `css::select`, `css::cascade`, `css::computed`, `layout`,
  `paint`, `anim`). It must not modify files owned by P1–P5.
- **`text.rs` is single-threaded by construction.** `FontDatabase` holds
  `Rc<ShapedText>` values, so it is `!Send`/`!Sync` — matching the contract's
  "single-threaded, !Send" and the one-`Fontconfig`-per-process rule
  (taffy-fontconfig.md §3.5). Never construct a second `FontDatabase::new()` in
  a loop; tests that need a fresh one use `probe_only()`.
- **Every parser/total-function this part adds gets a never-panic battery**
  (`css_weight_to_fc`, `css_stretch_to_fc`, `css_style_to_fc_slant`,
  `apply_text_transform`, `FontDatabase::shape`, the fixture line parser).
- **Every load-bearing test states its mutation check** — the one-line edit to
  the implementation that must turn it red.

## File Structure

| Path | Action | Contents |
|---|---|---|
| `ui/Cargo.toml` | **Verify only** (P1 owns) | `fontconfig = "0.11"` present |
| `ui/src/text.rs` | Modify (full rewrite) | scale tables, `FontFace`, `FontQuery`, `FontDatabase`, `apply_text_transform`, `ShapeKey`, `ShapedText`, `TextStyle` |
| `ui/themes/adwaita-dark.css` | Create | vendored GTK 4.22 `Default-dark.css` (1,929 lines, 37 `@define-color`s) |
| `ui/themes/adwaita-hc.css` | Create | vendored GTK 4.22 `Default-hc.css` (1,944 lines, 37 `@define-color`s) |
| `ui/themes/README.md` | Modify | provenance + LGPL note for the two new files |
| `ui/tests/fixtures/gtk4.22-css-properties.txt` | Create | 113 rows `name\|L\|S\|inherited` + doc URL header |
| `ui/tests/gtk4_property_reference.rs` | Create | `every_gtk4_property_is_registered` + 3 supporting tests |
| `ui/tests/adwaita_coverage.rs` | Create | THE M2 GATE: 0 unparseable / 0 unknown / 37 colours × 3 sheets |
| `ui/README.md` | Modify | M2 rewrite: engine breadth, fonts, gate, what M2 still does not cover |
| `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md` | Modify | `**Status:**` line + implementation-status block |

---

## Task 1 — CSS ↔ fontconfig scale conversions

The one place a naive port silently produces wrong weights
(taffy-fontconfig.md §3.3). Pure functions, no I/O, no fontconfig — testable
everywhere.

**Files**

- Verify: `ui/Cargo.toml` — `fontconfig = "0.11"` must be in `[dependencies]`
  (P1's edit).
- Modify: `ui/src/text.rs` — add the conversion block **above** whatever
  `FontStack`/`FontDatabase` body is currently in the file (roughly after the
  `FONT_CANDIDATES` const, M1 line ~31); leave the rest of the file alone in
  this task.

**Interfaces**

- Consumes (P1, `css::value`):
  ```rust
  pub enum FontStyle { Normal, Italic, Oblique(f32) }   // degrees; bare `oblique` == 14.0
  ```
- Produces:
  ```rust
  pub fn css_weight_to_fc(w: f32) -> i32;
  pub fn css_stretch_to_fc(pct: f32) -> i32;
  pub fn css_style_to_fc_slant(s: FontStyle) -> i32;
  ```

**Step 1 — preflight (not a test).**

```bash
grep -n 'fontconfig' /home/joseph/Projects/icedtea/ui/Cargo.toml
```

Expected: `fontconfig = "0.11"`. If it is missing, P1's Cargo.toml edit
regressed; restore exactly that one line under `[dependencies]` (the contract's
"Crate deps to add" block) and note it in the task's commit body. Then:

```bash
cargo build -p icedtea-ui 2>&1 | tail -5
```

must succeed — a link error here means system `libfontconfig` is absent, which
is a machine problem, not a code problem (`fc-match --version` to confirm).

**Step 2 — failing test.** Append to `ui/src/text.rs`'s `mod tests`:

```rust
    use super::{css_stretch_to_fc, css_style_to_fc_slant, css_weight_to_fc};
    use crate::css::value::FontStyle;

    #[test]
    fn css_weights_map_onto_fontconfigs_own_scale() {
        // fontconfig's FcWeightFromOpenType table, the mapping `fc-match` itself
        // uses. Verified on this machine: `fc-match "DejaVu Sans:weight=200"`
        // resolves to DejaVu Sans Bold, `weight=80` to Book.
        assert_eq!(css_weight_to_fc(100.0), 0, "thin");
        assert_eq!(css_weight_to_fc(200.0), 40, "extra-light");
        assert_eq!(css_weight_to_fc(300.0), 50, "light");
        assert_eq!(css_weight_to_fc(350.0), 55, "semi-light");
        assert_eq!(css_weight_to_fc(380.0), 75, "book");
        assert_eq!(css_weight_to_fc(400.0), 80, "regular");
        assert_eq!(css_weight_to_fc(500.0), 100, "medium");
        assert_eq!(css_weight_to_fc(600.0), 180, "semi-bold");
        assert_eq!(css_weight_to_fc(700.0), 200, "bold");
        assert_eq!(css_weight_to_fc(800.0), 205, "extra-bold");
        assert_eq!(css_weight_to_fc(900.0), 210, "black");
        assert_eq!(css_weight_to_fc(1000.0), 215, "extra-black");
    }

    #[test]
    fn css_weights_between_table_rows_interpolate_and_clamp() {
        // 450 is halfway between 400 (fc 80) and 500 (fc 100).
        assert_eq!(css_weight_to_fc(450.0), 90);
        // 650 is halfway between 600 (fc 180) and 700 (fc 200).
        assert_eq!(css_weight_to_fc(650.0), 190);
        // Outside the table the ends hold, they do not extrapolate.
        assert_eq!(css_weight_to_fc(0.0), 0);
        assert_eq!(css_weight_to_fc(-500.0), 0);
        assert_eq!(css_weight_to_fc(5000.0), 215);
    }

    #[test]
    fn css_stretch_percentages_map_onto_fc_width() {
        assert_eq!(css_stretch_to_fc(50.0), 50, "ultra-condensed");
        assert_eq!(css_stretch_to_fc(62.5), 63, "extra-condensed");
        assert_eq!(css_stretch_to_fc(75.0), 75, "condensed");
        assert_eq!(css_stretch_to_fc(87.5), 87, "semi-condensed");
        assert_eq!(css_stretch_to_fc(100.0), 100, "normal");
        assert_eq!(css_stretch_to_fc(112.5), 113, "semi-expanded");
        assert_eq!(css_stretch_to_fc(125.0), 125, "expanded");
        assert_eq!(css_stretch_to_fc(150.0), 150, "extra-expanded");
        assert_eq!(css_stretch_to_fc(200.0), 200, "ultra-expanded");
        // The two scales are not proportional: 68.75% sits between
        // extra-condensed (63) and condensed (75), i.e. 69, not 68.
        assert_eq!(css_stretch_to_fc(68.75), 69);
        assert_eq!(css_stretch_to_fc(10.0), 50, "clamped at the ultra-condensed end");
        assert_eq!(css_stretch_to_fc(900.0), 200, "clamped at the ultra-expanded end");
    }

    #[test]
    fn css_styles_map_onto_fc_slant() {
        assert_eq!(css_style_to_fc_slant(FontStyle::Normal), 0);
        assert_eq!(css_style_to_fc_slant(FontStyle::Italic), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(14.0)), 110);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(-14.0)), 110);
        // `oblique 0deg` is upright, per CSS Fonts 4.
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(0.0)), 0);
    }

    #[test]
    fn the_scale_conversions_never_panic_on_hostile_numbers() {
        for value in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            -0.0,
            0.0,
            1e30,
            -1e30,
        ] {
            let _ = css_weight_to_fc(value);
            let _ = css_stretch_to_fc(value);
            let _ = css_style_to_fc_slant(FontStyle::Oblique(value));
        }
        // A non-finite input must land on the neutral row, never on 0 or a
        // saturated cast.
        assert_eq!(css_weight_to_fc(f32::NAN), 80);
        assert_eq!(css_stretch_to_fc(f32::NAN), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(f32::NAN)), 110);
    }
```

**Step 3 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import `super::css_weight_to_fc`` (and the
two siblings) — the functions do not exist yet.

**Step 4 — implement.** Insert into `ui/src/text.rs` after `FONT_CANDIDATES`:

```rust
/// CSS `font-weight` (1..1000) → fontconfig `FC_WEIGHT`.
///
/// This is a table lookup with piecewise-linear interpolation, **not** a cast:
/// the two scales are not proportional (fontconfig's own
/// `FcWeightFromOpenType` table). `fc-match "DejaVu Sans:weight=200"` picks the
/// Bold face, which is CSS 700 — passing 700 straight through would ask for a
/// weight past extra-black.
const CSS_TO_FC_WEIGHT: &[(f32, f32)] = &[
    (100.0, 0.0),
    (200.0, 40.0),
    (300.0, 50.0),
    (350.0, 55.0),
    (380.0, 75.0),
    (400.0, 80.0),
    (500.0, 100.0),
    (600.0, 180.0),
    (700.0, 200.0),
    (800.0, 205.0),
    (900.0, 210.0),
    (1000.0, 215.0),
];

/// CSS `font-width`/`font-stretch` percentage → fontconfig `FC_WIDTH`.
const CSS_TO_FC_WIDTH: &[(f32, f32)] = &[
    (50.0, 50.0),
    (62.5, 63.0),
    (75.0, 75.0),
    (87.5, 87.0),
    (100.0, 100.0),
    (112.5, 113.0),
    (125.0, 125.0),
    (150.0, 150.0),
    (200.0, 200.0),
];

/// Piecewise-linear lookup over an ascending `(input, output)` table. Values
/// outside the table clamp to its ends rather than extrapolating.
fn interpolate_table(table: &[(f32, f32)], x: f32) -> f32 {
    let first = table[0];
    let last = table[table.len() - 1];
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for pair in table.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        if x <= x1 {
            let span = x1 - x0;
            if span <= 0.0 {
                return y1;
            }
            return y0 + (y1 - y0) * ((x - x0) / span);
        }
    }
    last.1
}

/// CSS `font-weight` → `FC_WEIGHT`. Non-finite input resolves to regular (80).
#[must_use]
pub fn css_weight_to_fc(w: f32) -> i32 {
    if !w.is_finite() {
        return 80;
    }
    interpolate_table(CSS_TO_FC_WEIGHT, w).round().clamp(0.0, 215.0) as i32
}

/// CSS `font-width`/`font-stretch` percentage → `FC_WIDTH`. Non-finite input
/// resolves to normal (100).
#[must_use]
pub fn css_stretch_to_fc(pct: f32) -> i32 {
    if !pct.is_finite() {
        return 100;
    }
    interpolate_table(CSS_TO_FC_WIDTH, pct)
        .round()
        .clamp(1.0, 400.0) as i32
}

/// CSS `font-style` → `FC_SLANT` (`ROMAN` 0, `ITALIC` 100, `OBLIQUE` 110).
///
/// `oblique 0deg` is upright text, so it maps to `ROMAN`; every other angle,
/// including a non-finite one, is oblique.
#[must_use]
pub fn css_style_to_fc_slant(s: FontStyle) -> i32 {
    match s {
        FontStyle::Normal => 0,
        FontStyle::Italic => 100,
        FontStyle::Oblique(degrees) if degrees.is_finite() && degrees.abs() < 0.5 => 0,
        FontStyle::Oblique(_) => 110,
    }
}
```

Add to the file's `use` block:

```rust
use crate::css::value::FontStyle;
```

**Step 5 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Change `(600.0, 180.0)` to `(600.0, 190.0)` in
`CSS_TO_FC_WEIGHT`: `css_weights_map_onto_fontconfigs_own_scale` and
`css_weights_between_table_rows_interpolate_and_clamp` both fail. Delete the
`!w.is_finite()` guard: `the_scale_conversions_never_panic_on_hostile_numbers`
fails on the `NAN` assertion (the cast saturates to 0, not 80).

**Step 6 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs ui/Cargo.toml
git commit -m "feat(ui/text): CSS<->fontconfig weight, width and slant scales

The two scales are not proportional, so these are table lookups with
piecewise-linear interpolation rather than casts: CSS 700 is FC_WEIGHT 200,
CSS 400 is 80. Non-finite input lands on the neutral row instead of a
saturated cast.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2 — `FontFace`, `FontQuery`, and `FontDatabase::probe_only()`

The fontconfig-free half of the database: identity types, the typeface/font
caches, and M1's probe list preserved as the CI fallback.

**Files**

- Modify: `ui/src/text.rs` — replace the `FontStack` struct and its `impl`
  (M1 lines ~60–160), or the placeholder an earlier part left, with the types
  below. Keep `FONT_CANDIDATES`, `TextMetrics` and Task 1's block. `ShapedText`
  gains two fields in this task; `shape`/`blob`/`measure` are rewritten in
  Task 5, so leave the M1 bodies in place only if they still compile — otherwise
  comment nothing out, port them as instructed below.

**Interfaces**

- Consumes: `skia_rs_safe::text::{Font, Shaper, Typeface}`; Task 1's
  conversions.
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq, Hash)]
  pub struct FontFace { pub path: PathBuf, pub index: i32, pub family: String }

  #[derive(Copy, Clone, Debug, PartialEq)]
  pub struct FontQuery<'a> {
      pub families: &'a [FontFamily],
      pub weight: f32,
      pub style: FontStyle,
      pub stretch: f32,
      pub size_px: f32,
  }

  pub struct FontDatabase { /* Option<Fontconfig> + caches; !Send */ }
  impl FontDatabase {
      pub fn probe_only() -> FontDatabase;
      pub fn has_fontconfig(&self) -> bool;
      pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>>;
      pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font>;
      pub fn ex_ratio(&mut self, face: &FontFace) -> f32;
      pub fn clear_caches(&mut self);
  }
  ```

**Step 1 — failing test.** In `ui/src/text.rs`'s `mod tests`:

```rust
    use super::{FontDatabase, FontQuery};
    use crate::css::value::{FontFamily, FontStyle, GenericFamily};

    fn sans() -> [FontFamily; 1] {
        [FontFamily::Generic(GenericFamily::SansSerif)]
    }

    fn query<'a>(families: &'a [FontFamily]) -> FontQuery<'a> {
        FontQuery {
            families,
            weight: 400.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        }
    }

    #[test]
    fn the_probe_only_database_resolves_a_face_without_fontconfig() {
        let mut db = FontDatabase::probe_only();
        assert!(!db.has_fontconfig(), "probe_only must not initialise fontconfig");
        let families = sans();
        let face = db.match_face(&query(&families)).expect(
            "no font found among FONT_CANDIDATES; install adwaita-fonts/dejavu/liberation/noto",
        );
        assert!(face.path.is_file(), "{} is not a readable file", face.path.display());
        assert!(!face.family.is_empty(), "the probed face reported no family name");
        assert_eq!(face.index, 0, "the probe list only ever loads face 0");
    }

    #[test]
    fn a_typeface_is_loaded_once_per_face_and_cached() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let first = db.typeface(&face).expect("typeface");
        let second = db.typeface(&face).expect("typeface");
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "the typeface cache handed out two different allocations for one face"
        );
        assert_eq!(first.family_name(), face.family);

        db.clear_caches();
        let third = db.typeface(&face).expect("typeface");
        assert!(
            !std::sync::Arc::ptr_eq(&first, &third),
            "clear_caches did not drop the typeface cache"
        );
    }

    #[test]
    fn a_font_carries_the_requested_size_and_an_x_height_ratio() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let font = db.font(&face, 28.0).expect("font");
        let metrics = font.metrics();
        assert!(
            metrics.line_height() > 14.0,
            "28px face reported a {}px line height",
            metrics.line_height()
        );
        let ratio = db.ex_ratio(&face);
        assert!(
            (0.2..=0.9).contains(&ratio),
            "x-height/em ratio {ratio} is outside every real UI face's range"
        );
    }

    #[test]
    fn an_unreadable_face_resolves_to_nothing_rather_than_panicking() {
        let mut db = FontDatabase::probe_only();
        let missing = super::FontFace {
            path: std::path::PathBuf::from("/nonexistent/font/does-not-exist.ttf"),
            index: 0,
            family: String::from("Nothing"),
        };
        assert!(db.typeface(&missing).is_none());
        assert!(db.font(&missing, 14.0).is_none());
        assert_eq!(db.ex_ratio(&missing), 0.5, "the documented ex-ratio fallback");
    }
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import `super::FontDatabase`` (and
`FontQuery`).

**Step 3 — implement.** Rewrite the head of `ui/src/text.rs` (module doc + uses)
and replace `FontStack`:

```rust
//! The text stack: fontconfig discovery over `skia-rs-text` shaping.
//!
//! Font *matching* is real fontconfig — one `FcPattern` per query carrying the
//! whole `font-family` list plus `FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/
//! `FC_PIXEL_SIZE`, then `FcFontMatch` — so user aliases, generic families and
//! `~/.config/fontconfig` rules resolve exactly as `fc-match` resolves them.
//! Leaving the GNOME toolkit means leaving `pango`/`gtk`, not purging native
//! libraries: `wlroots` is already C, and so is `libfontconfig`.
//!
//! Font *rendering* stays `skia-rs-text`: `Typeface::from_data` (`ttf_parser`),
//! `rustybuzz` shaping, `Canvas::draw_text_blob`.
//!
//! Caches, all invalidated together by [`FontDatabase::clear_caches`]:
//! query → [`FontFace`], `(path, index)` → `Typeface`, [`ShapeKey`] →
//! [`ShapedText`]. `FontDatabase` is single-threaded by construction (it hands
//! out `Rc`s and owns the process's one `Fontconfig`); do not send it anywhere.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use skia_rs_safe::core::Point;
use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{
    FeatureSetting, FontFamily, FontStyle, FontWeight, GenericFamily, Keyword, Length, LengthUnit,
    LineHeight, Value, VariationSetting,
};
```

(Task 2 only needs `HashMap`, `Path`, `PathBuf`, `Arc`, `Font`, `Shaper`,
`Typeface`, `FontFamily`, `FontStyle`, `GenericFamily`; the rest land in Tasks
4–6. Add them as those tasks arrive rather than importing unused names — clippy
runs with `-D warnings`.)

```rust
/// A resolved face: the file fontconfig chose, the face inside it, and the
/// family name it reported.
///
/// `index` is kept even though `skia-rs-text` 0.4.0's `Typeface::from_data`
/// cannot select a face inside a collection: it is part of the cache identity
/// and of `fc-match` parity, and [`FontDatabase::typeface`] warns once when it
/// is non-zero rather than pretending the right face was loaded.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFace {
    /// Absolute path of the font file.
    pub path: PathBuf,
    /// Face index within the file (0 for a plain TTF/OTF).
    pub index: i32,
    /// The family name fontconfig reported for the match.
    pub family: String,
}

/// What a caller asks a [`FontDatabase`] for: the computed font properties of
/// one element.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontQuery<'a> {
    /// `font-family`, in priority order; generics included as-is.
    pub families: &'a [FontFamily],
    /// CSS `font-weight`, 1..=1000.
    pub weight: f32,
    /// CSS `font-style`.
    pub style: FontStyle,
    /// CSS `font-width`/`font-stretch` as a percentage; 100.0 is normal.
    pub stretch: f32,
    /// Computed `font-size` in device pixels.
    pub size_px: f32,
}

/// Font discovery, loading and shaping, with the caches that make a restyle
/// cheap.
pub struct FontDatabase {
    fontconfig: Option<fontconfig::Fontconfig>,
    shaper: Shaper,
    matches: HashMap<String, Option<FontFace>>,
    typefaces: HashMap<FontFace, Option<Arc<Typeface>>>,
    shapes: HashMap<String, Rc<ShapedText>>,
    warned_collection_index: bool,
    warned_unsupported_shaping: bool,
}

impl FontDatabase {
    fn empty(fontconfig: Option<fontconfig::Fontconfig>) -> FontDatabase {
        FontDatabase {
            fontconfig,
            shaper: Shaper::new(),
            matches: HashMap::new(),
            typefaces: HashMap::new(),
            shapes: HashMap::new(),
            warned_collection_index: false,
            warned_unsupported_shaping: false,
        }
    }

    /// The fontconfig-free database: M1's fixed [`FONT_CANDIDATES`] probe.
    ///
    /// This is the CI / stripped-container fallback, and what
    /// [`FontDatabase::new`] degrades to when `FcInit` fails.
    #[must_use]
    pub fn probe_only() -> FontDatabase {
        FontDatabase::empty(None)
    }

    /// Whether real fontconfig matching is available.
    #[must_use]
    pub fn has_fontconfig(&self) -> bool {
        self.fontconfig.is_some()
    }

    /// The loaded typeface for `face`, cached per `(path, index)`.
    pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>> {
        if let Some(cached) = self.typefaces.get(face) {
            return cached.clone();
        }
        if face.index != 0 && !self.warned_collection_index {
            self.warned_collection_index = true;
            tracing::warn!(
                path = %face.path.display(),
                index = face.index,
                "skia-rs-text 0.4.0 cannot select a face index inside a font collection; \
                 loading face 0 instead"
            );
        }
        let loaded = std::fs::read(&face.path)
            .ok()
            .and_then(Typeface::from_data)
            .map(Arc::new);
        if loaded.is_none() {
            tracing::debug!(path = %face.path.display(), "font file is unreadable or unparseable");
        }
        self.typefaces.insert(face.clone(), loaded.clone());
        loaded
    }

    /// A [`Font`] for `face` at `size_px`.
    pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font> {
        Some(Font::new(self.typeface(face)?, size_px))
    }

    /// x-height ÷ em for `face`, the `ex` unit's basis.
    ///
    /// Falls back to the CSS-recommended `0.5` when the face has no usable
    /// `x_height` (`LengthCtx::ex_ratio`'s documented default).
    pub fn ex_ratio(&mut self, face: &FontFace) -> f32 {
        const PROBE_SIZE: f32 = 100.0;
        let Some(font) = self.font(face, PROBE_SIZE) else {
            return 0.5;
        };
        let x_height = font.metrics().x_height;
        if x_height.is_finite() && x_height > 0.0 {
            x_height / PROBE_SIZE
        } else {
            0.5
        }
    }

    /// Drop every cache. Callers use this when the theme or the font
    /// configuration changed underneath them.
    pub fn clear_caches(&mut self) {
        self.matches.clear();
        self.typefaces.clear();
        self.shapes.clear();
    }
}
```

Plus the probe-side matching used by `match_face` (Task 3 wires fontconfig in
front of it; this task lands the whole `match_face` so the tests above can run):

```rust
impl FontDatabase {
    /// Resolve `query` to a face, with `fc-match` parity when fontconfig is
    /// available and the [`FONT_CANDIDATES`] probe otherwise.
    ///
    /// Cached per query; a query that resolves to nothing is cached as nothing,
    /// so a missing font costs one lookup, not one per restyle.
    pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace> {
        let key = match_cache_key(query);
        if let Some(cached) = self.matches.get(&key) {
            return cached.clone();
        }
        let face = probe_face(query);
        self.matches.insert(key, face.clone());
        face
    }
}

/// A stable string identity for a query — `FontFamily` is not `Hash` and the
/// numeric fields are floats, so the cache is keyed by this rather than by the
/// query itself.
fn match_cache_key(query: &FontQuery<'_>) -> String {
    let mut key = String::new();
    for family in query.families {
        for name in family_names(family) {
            key.push_str(name);
            key.push('\u{1}');
        }
    }
    key.push_str(&format!(
        "\u{2}{}\u{2}{}\u{2}{}\u{2}{}",
        query.weight.to_bits(),
        css_style_to_fc_slant(query.style),
        query.stretch.to_bits(),
        query.size_px.to_bits()
    ));
    key
}

/// The fontconfig family strings for one CSS family, in the order they should
/// be offered. `system-ui` is offered with `sans-serif` behind it because not
/// every fontconfig configuration aliases it.
fn family_names(family: &FontFamily) -> Vec<&str> {
    match family {
        FontFamily::Named(name) => vec![name.as_ref()],
        FontFamily::Generic(GenericFamily::Serif) => vec!["serif"],
        FontFamily::Generic(GenericFamily::SansSerif) => vec!["sans-serif"],
        FontFamily::Generic(GenericFamily::Monospace) => vec!["monospace"],
        FontFamily::Generic(GenericFamily::Cursive) => vec!["cursive"],
        FontFamily::Generic(GenericFamily::Fantasy) => vec!["fantasy"],
        FontFamily::Generic(GenericFamily::SystemUi) => vec!["system-ui", "sans-serif"],
    }
}

/// The no-fontconfig path: a named family that is itself a readable font file
/// wins (so tests can name a file), then M1's fixed candidate list.
fn probe_face(query: &FontQuery<'_>) -> Option<FontFace> {
    for family in query.families {
        if let FontFamily::Named(name) = family {
            let path = Path::new(name.as_ref());
            if path.is_file() {
                if let Some(face) = face_from_file(path) {
                    return Some(face);
                }
            }
        }
    }
    for candidate in FONT_CANDIDATES {
        if let Some(face) = face_from_file(Path::new(candidate)) {
            tracing::debug!(font = candidate, "probed UI typeface");
            return Some(face);
        }
    }
    tracing::warn!(
        candidates = FONT_CANDIDATES.len(),
        "no UI typeface found in the probe list"
    );
    None
}

fn face_from_file(path: &Path) -> Option<FontFace> {
    let data = std::fs::read(path).ok()?;
    let typeface = Typeface::from_data(data)?;
    Some(FontFace {
        path: path.to_path_buf(),
        index: 0,
        family: typeface.family_name().to_string(),
    })
}
```

Also update `ShapedText` to the contract's four fields (its `blob`/`metrics`
producers are rewritten in Task 5; for now construct the two new fields
wherever an existing body builds a `ShapedText`):

```rust
/// A shaped label: its blob (empty text shapes to `None`), its extents, and
/// the face and size it was shaped with.
pub struct ShapedText {
    /// The positioned glyph run, relative to the baseline origin `(0, 0)`.
    /// `None` for text that produced no glyphs, including `""`.
    pub blob: Option<TextBlob>,
    /// The measured extents that layout sizes the label against.
    pub metrics: TextMetrics,
    /// The face this run was shaped with.
    pub face: FontFace,
    /// The size, in px, this run was shaped at.
    pub size_px: f32,
}
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Make `typeface` skip the cache insert (`let loaded = …;
return loaded;` with no `self.typefaces.insert`):
`a_typeface_is_loaded_once_per_face_and_cached` fails on the `Arc::ptr_eq`
assertion. Make `ex_ratio` return `x_height / PROBE_SIZE` unconditionally:
`an_unreadable_face_resolves_to_nothing_rather_than_panicking` fails (it panics
on the `None` font instead of returning 0.5).

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs
git commit -m "feat(ui/text): FontFace, FontQuery and the probe-only FontDatabase

Replaces M1's FontStack with the contract's database shape: query -> FontFace,
(path,index) -> Typeface and an ex-height ratio for the `ex` unit, all cached
and all droppable with clear_caches. M1's FONT_CANDIDATES survives as
probe_only()'s list, the fallback for machines with no fontconfig.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3 — `FontDatabase::new()` and real `fc-match` parity

**Files**

- Modify: `ui/src/text.rs` — add `FontDatabase::new`, the `fc_match` free
  function, and route `match_face` through fontconfig before the probe.

**Interfaces**

- Consumes: `fontconfig::{Fontconfig, Pattern, FC_FAMILY, FC_WEIGHT, FC_SLANT,
  FC_WIDTH, FC_PIXEL_SIZE}` (the sys constants are re-exported from the crate
  root: `pub use sys::constants::*;`), Task 1's conversions, Task 2's
  `family_names`/`probe_face`/`match_cache_key`.
- Produces:
  ```rust
  impl FontDatabase {
      pub fn new() -> FontDatabase;   // FcInit once; falls back to probe_only()
      // match_face gains the fontconfig branch (signature unchanged)
  }
  ```

**Step 1 — failing test.** In `ui/src/text.rs`'s `mod tests` (the helpers
`sans()`/`query()` from Task 2 are reused):

```rust
    use std::process::Command;

    /// `fc-match`'s answer for a pattern, or `None` when fc-match is absent.
    fn fc_match_file(pattern: &str) -> Option<String> {
        let output = Command::new("fc-match")
            .arg("--format=%{file}")
            .arg(pattern)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let file = String::from_utf8(output.stdout).ok()?;
        (!file.trim().is_empty()).then(|| file.trim().to_string())
    }

    #[test]
    fn a_generic_family_resolves_to_something() {
        let mut db = FontDatabase::new();
        let families = sans();
        let face = db
            .match_face(&query(&families))
            .expect("sans-serif resolved to no face at all");
        // Deliberately NOT a fixed family: which face `sans-serif` means is the
        // machine's business (spec section 6).
        assert!(face.path.is_file(), "{} is not readable", face.path.display());
        assert!(!face.family.is_empty());
    }

    #[test]
    fn the_query_matches_what_fc_match_would_pick() {
        let mut db = FontDatabase::new();
        if !db.has_fontconfig() {
            eprintln!("fontconfig unavailable; parity check skipped");
            return;
        }
        let Some(expected) = fc_match_file("sans-serif:weight=200:slant=0:width=100:pixelsize=14")
        else {
            eprintln!("fc-match unavailable; parity check skipped");
            return;
        };
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let bold = FontQuery {
            families: &families,
            weight: 700.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        };
        let face = db.match_face(&bold).expect("bold sans-serif");
        assert_eq!(
            face.path.to_string_lossy(),
            expected,
            "our FcPattern disagrees with fc-match for bold sans-serif"
        );
    }

    #[test]
    fn a_named_family_is_offered_before_the_generic_fallback() {
        let mut db = FontDatabase::new();
        if !db.has_fontconfig() {
            eprintln!("fontconfig unavailable; family-order check skipped");
            return;
        }
        let Some(expected) = fc_match_file("monospace") else {
            eprintln!("fc-match unavailable; family-order check skipped");
            return;
        };
        let families = [
            FontFamily::Named("Definitely Not An Installed Family".into()),
            FontFamily::Generic(GenericFamily::Monospace),
        ];
        let face = db.match_face(&query(&families)).expect("monospace");
        assert_eq!(
            face.path.to_string_lossy(),
            expected,
            "an unknown first family must fall through to the next one, not abort the match"
        );
    }

    #[test]
    fn matches_are_cached_per_query() {
        let mut db = FontDatabase::new();
        let families = sans();
        let first = db.match_face(&query(&families));
        let second = db.match_face(&query(&families));
        assert_eq!(first, second, "two identical queries disagreed");

        let mut heavier = query(&families);
        heavier.weight = 900.0;
        // A different query must not be served from the first one's slot; it
        // may legitimately resolve to the same file on a one-weight system, so
        // assert on the cache size, not on the face.
        let _ = db.match_face(&heavier);
        assert_eq!(db.match_cache_len(), 2, "the weight is not part of the cache key");
    }
```

`match_cache_len` is a test-only accessor; add it under `#[cfg(test)]` inside
`impl FontDatabase`:

```rust
    #[cfg(test)]
    fn match_cache_len(&self) -> usize {
        self.matches.len()
    }
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0599]: no function or associated item named `new` found for
struct `FontDatabase`` (and `match_cache_len`).

**Step 3 — implement.** Add to `ui/src/text.rs`:

```rust
impl FontDatabase {
    /// Initialise fontconfig (`FcInit`) once for this process, falling back to
    /// [`FontDatabase::probe_only`] when it fails.
    ///
    /// The handle is never dropped-and-recreated: the `fontconfig` crate
    /// deliberately never calls `FcFini`, so one database should live for the
    /// lifetime of the UI thread.
    #[must_use]
    pub fn new() -> FontDatabase {
        match fontconfig::Fontconfig::new() {
            Some(fc) => {
                tracing::debug!("fontconfig initialised");
                FontDatabase::empty(Some(fc))
            }
            None => {
                tracing::warn!(
                    "FcInit failed; falling back to the FONT_CANDIDATES probe list"
                );
                FontDatabase::probe_only()
            }
        }
    }
}

impl Default for FontDatabase {
    fn default() -> Self {
        FontDatabase::new()
    }
}
```

Replace `match_face`'s body's middle line with the fontconfig branch:

```rust
        let face = match self.fontconfig.as_ref() {
            // A fontconfig miss still falls through to the probe list: a
            // configured-but-empty fontconfig must not leave the UI textless.
            Some(fc) => fc_match(fc, query).or_else(|| probe_face(query)),
            None => probe_face(query),
        };
```

And add the matcher:

```rust
/// One `FcPattern` carrying the whole family list in priority order plus
/// `FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/`FC_PIXEL_SIZE`, then `FcFontMatch` — the
/// same path `fc-match` takes, so aliases, generic families and user rules in
/// `~/.config/fontconfig` are honoured without any special-casing here.
fn fc_match(fc: &fontconfig::Fontconfig, query: &FontQuery<'_>) -> Option<FontFace> {
    let mut pattern = fontconfig::Pattern::new(fc).ok()?;
    for family in query.families {
        for name in family_names(family) {
            // A family with an embedded NUL cannot reach fontconfig; skip that
            // one name rather than abandoning the whole query.
            match CString::new(name) {
                Ok(value) => {
                    if let Err(error) = pattern.add_string(fontconfig::FC_FAMILY, &value) {
                        tracing::debug!(family = name, ?error, "FcPatternAddString failed");
                    }
                }
                Err(_) => tracing::debug!(family = name, "font family contains a NUL byte"),
            }
        }
    }
    let _ = pattern.add_integer(fontconfig::FC_WEIGHT, css_weight_to_fc(query.weight));
    let _ = pattern.add_integer(fontconfig::FC_SLANT, css_style_to_fc_slant(query.style));
    let _ = pattern.add_integer(fontconfig::FC_WIDTH, css_stretch_to_fc(query.stretch));
    if query.size_px.is_finite() && query.size_px > 0.0 {
        let _ = pattern.add_integer(
            fontconfig::FC_PIXEL_SIZE,
            query.size_px.round().clamp(1.0, 4096.0) as i32,
        );
    }

    // `font_match` runs FcConfigSubstitute + FcDefaultSubstitute itself, and
    // the pattern it returns borrows this one -- copy everything out here.
    let matched = match pattern.font_match() {
        Ok(matched) => matched,
        Err(error) => {
            tracing::debug!(?error, "FcFontMatch found nothing");
            return None;
        }
    };
    let path = PathBuf::from(matched.filename().ok()?);
    let index = matched.face_index().unwrap_or(0);
    let family = matched
        .get_string(fontconfig::FC_FAMILY)
        .or_else(|_| matched.name())
        .map(str::to_owned)
        .unwrap_or_default();
    Some(FontFace { path, index, family })
}
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Replace `css_weight_to_fc(query.weight)` with
`query.weight as i32`: `the_query_matches_what_fc_match_would_pick` fails —
`FC_WEIGHT 700` is past extra-black, so fontconfig returns a different file than
`fc-match … weight=200`. Drop `query.weight.to_bits()` from `match_cache_key`:
`matches_are_cached_per_query` fails on the cache length.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs
git commit -m "feat(ui/text): fc-match parity for family lists, weight, slant and width

FontDatabase::new runs FcInit once and degrades to probe_only when it fails.
Matching builds one FcPattern with every family in priority order plus the
converted FC_WEIGHT/FC_SLANT/FC_WIDTH/FC_PIXEL_SIZE and calls FcFontMatch, so
generic families and user aliases resolve through fontconfig's own
substitution. Proven against the fc-match binary where it is installed.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 4 — `text-transform`, applied before shaping

CSS `text-transform` changes the *characters*, so it must run before the shaper
sees them — GTK does the same. Includes `full-width` and `full-size-kana`
(GTK 4.6+, spec §6).

**Files**

- Modify: `ui/src/text.rs` — add `apply_text_transform` and its tables after
  `face_from_file`.

**Interfaces**

- Consumes: `crate::css::value::Keyword` (`None`, `Capitalize`, `Uppercase`,
  `Lowercase`, `FullWidth`, `FullSizeKana`).
- Produces:
  ```rust
  pub fn apply_text_transform(text: &str, transform: Keyword) -> Cow<'_, str>;
  ```

**Step 1 — failing test.** In `mod tests`:

```rust
    use super::apply_text_transform;
    use crate::css::value::Keyword;

    #[test]
    fn text_transform_none_borrows_the_input_unchanged() {
        let out = apply_text_transform("Click me", Keyword::None);
        assert_eq!(out, "Click me");
        assert!(
            matches!(out, std::borrow::Cow::Borrowed(_)),
            "`none` must not allocate"
        );
        // Any keyword that is not a text-transform value is also a no-op.
        assert_eq!(apply_text_transform("Click me", Keyword::Solid), "Click me");
    }

    #[test]
    fn text_transform_cases_follow_unicode_not_ascii() {
        assert_eq!(apply_text_transform("straße", Keyword::Uppercase), "STRASSE");
        assert_eq!(apply_text_transform("ÅNGSTRÖM", Keyword::Lowercase), "ångström");
        assert_eq!(apply_text_transform("ábc déf", Keyword::Capitalize), "Ábc Déf");
    }

    #[test]
    fn capitalize_starts_a_word_after_any_whitespace_or_punctuation_run() {
        assert_eq!(apply_text_transform("click me now", Keyword::Capitalize), "Click Me Now");
        assert_eq!(apply_text_transform("  leading", Keyword::Capitalize), "  Leading");
        assert_eq!(apply_text_transform("multi\tword\nlines", Keyword::Capitalize), "Multi\tWord\nLines");
        assert_eq!(
            apply_text_transform("ALREADY UP", Keyword::Capitalize),
            "ALREADY UP",
            "capitalize only touches the first letter of each word"
        );
    }

    #[test]
    fn full_width_maps_ascii_into_the_fullwidth_block() {
        assert_eq!(apply_text_transform("AB1!", Keyword::FullWidth), "ＡＢ１！");
        assert_eq!(apply_text_transform(" ", Keyword::FullWidth), "\u{3000}");
        assert_eq!(
            apply_text_transform("あ", Keyword::FullWidth),
            "あ",
            "a character that is already full width is left alone"
        );
    }

    #[test]
    fn full_size_kana_promotes_the_small_kana() {
        assert_eq!(apply_text_transform("ぁぃっゃ", Keyword::FullSizeKana), "あいつや");
        assert_eq!(apply_text_transform("ァィッャ", Keyword::FullSizeKana), "アイツヤ");
        assert_eq!(
            apply_text_transform("あア", Keyword::FullSizeKana),
            "あア",
            "full-size kana are already full size"
        );
    }

    #[test]
    fn text_transform_never_panics_on_hostile_text() {
        let hostile = [
            "",
            "\u{0}",
            "\u{7}\u{1b}\u{7f}",
            "\u{200b}\u{200e}\u{feff}",
            "e\u{301}\u{301}\u{301}",
            "🇯🇵👩‍👩‍👧‍👦",
            "\u{10FFFF}",
            "ﬁﬂﬀ",
        ];
        for transform in [
            Keyword::None,
            Keyword::Capitalize,
            Keyword::Uppercase,
            Keyword::Lowercase,
            Keyword::FullWidth,
            Keyword::FullSizeKana,
        ] {
            for text in hostile {
                let _ = apply_text_transform(text, transform);
            }
            let long = "aあ ".repeat(5_000);
            let _ = apply_text_transform(&long, transform);
        }
    }
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import `super::apply_text_transform``.

**Step 3 — implement.**

```rust
/// Small kana → their full-size form, for `text-transform: full-size-kana`.
///
/// The hiragana and katakana small letters GTK's own `full-size-kana` covers;
/// the half-width katakana block is left alone (it is a *width* transform, not
/// a size one, and `full-width` handles it).
const FULL_SIZE_KANA: &[(char, char)] = &[
    ('ぁ', 'あ'), ('ぃ', 'い'), ('ぅ', 'う'), ('ぇ', 'え'), ('ぉ', 'お'),
    ('っ', 'つ'), ('ゃ', 'や'), ('ゅ', 'ゆ'), ('ょ', 'よ'), ('ゎ', 'わ'),
    ('ゕ', 'か'), ('ゖ', 'け'),
    ('ァ', 'ア'), ('ィ', 'イ'), ('ゥ', 'ウ'), ('ェ', 'エ'), ('ォ', 'オ'),
    ('ッ', 'ツ'), ('ャ', 'ヤ'), ('ュ', 'ユ'), ('ョ', 'ヨ'), ('ヮ', 'ワ'),
    ('ヵ', 'カ'), ('ヶ', 'ケ'),
];

/// Apply CSS `text-transform` to `text`.
///
/// This runs *before* shaping (as it does in GTK): the transform changes which
/// characters exist, so the shaper must see the transformed string, and the
/// result is what [`FontDatabase::shape`] caches.
///
/// A keyword that is not a `text-transform` value — including
/// [`Keyword::None`] — borrows the input unchanged.
#[must_use]
pub fn apply_text_transform(text: &str, transform: Keyword) -> Cow<'_, str> {
    match transform {
        Keyword::Uppercase => Cow::Owned(text.to_uppercase()),
        Keyword::Lowercase => Cow::Owned(text.to_lowercase()),
        Keyword::Capitalize => Cow::Owned(capitalize(text)),
        Keyword::FullWidth => Cow::Owned(text.chars().map(to_full_width).collect()),
        Keyword::FullSizeKana => Cow::Owned(text.chars().map(to_full_size_kana).collect()),
        _ => Cow::Borrowed(text),
    }
}

/// Upper-case the first letter of every whitespace-delimited word.
fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            at_word_start = true;
            out.push(ch);
        } else if at_word_start {
            at_word_start = false;
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// ASCII → the Halfwidth and Fullwidth Forms block; space → ideographic space.
fn to_full_width(ch: char) -> char {
    match ch {
        ' ' => '\u{3000}',
        '!'..='~' => char::from_u32(ch as u32 + 0xFEE0).unwrap_or(ch),
        _ => ch,
    }
}

fn to_full_size_kana(ch: char) -> char {
    FULL_SIZE_KANA
        .iter()
        .find_map(|&(small, full)| (small == ch).then_some(full))
        .unwrap_or(ch)
}
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Change `capitalize`'s `out.extend(ch.to_uppercase())` to
`out.push(ch.to_ascii_uppercase())`:
`text_transform_cases_follow_unicode_not_ascii` fails on `"ábc déf"`. Change
`to_full_width`'s range to `'a'..='z'`: `full_width_maps_ascii_into_the_fullwidth_block`
fails on `"AB1!"`.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs
git commit -m "feat(ui/text): text-transform, applied before shaping

case, capitalize, full-width and full-size-kana, Unicode-correct (straße
upper-cases to STRASSE) and non-allocating for the `none` case. The transform
changes which characters exist, so it runs ahead of the shaper and is part of
the shaping cache key.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 5 — `ShapeKey`, `FontDatabase::shape`, letter-spacing and the shaping cache

Ports M1's shape/measure/blob path onto `FontFace` + `ShapeKey`, adds
`letter-spacing`, applies Task 4's transform, and caches the result. This is
where the contract's §10.3 rewrite of `text.rs`'s five M1 tests lands: the same
properties are asserted, with "resolves to *something*" replacing any fixed
family.

**Files**

- Modify: `ui/src/text.rs` — replace M1's `measure`/`shape`/`blob` methods with
  `shape`, `measure_metrics` and `build_blob`; delete `FontStack` entirely if an
  earlier part left it behind.

**Interfaces**

- Consumes: Task 2's `FontDatabase::font`, Task 4's `apply_text_transform`,
  `skia_rs_safe::text::{TextBlob, TextBlobBuilder}`, `skia_rs_safe::core::Point`.
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub struct ShapeKey<'a> {
      pub text: &'a str, pub face: &'a FontFace, pub size_px: f32,
      pub letter_spacing_px: f32, pub features: &'a [FeatureSetting],
      pub variations: &'a [VariationSetting], pub transform: Keyword,
  }
  impl FontDatabase { pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText>; }
  ```

**Step 1 — failing test.** In `mod tests`:

```rust
    use super::{FontFace, ShapeKey, ShapedText};
    use std::rc::Rc;

    fn key<'a>(text: &'a str, face: &'a FontFace, size_px: f32) -> ShapeKey<'a> {
        ShapeKey {
            text,
            face,
            size_px,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        }
    }

    fn shaped(db: &mut FontDatabase, text: &str, size_px: f32) -> Rc<ShapedText> {
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        db.shape(&key(text, &face, size_px))
    }

    #[test]
    fn measurement_scales_with_size_and_length() {
        let mut db = FontDatabase::probe_only();
        let small = shaped(&mut db, "Click me", 14.0);
        let large = shaped(&mut db, "Click me", 28.0);
        assert!(
            small.metrics.width > 0.0,
            "zero-width measurement means no hmtx data reached us"
        );
        assert!(
            large.metrics.width > small.metrics.width * 1.8,
            "28px measured {} vs 14px {}: advances are not scaling with size",
            large.metrics.width,
            small.metrics.width
        );
        let longer = shaped(&mut db, "Click me twice", 14.0);
        assert!(longer.metrics.width > small.metrics.width);

        assert!(small.metrics.ascent > 0.0 && small.metrics.descent > 0.0);
        assert!(small.metrics.line_height >= small.metrics.ascent + small.metrics.descent);
    }

    #[test]
    fn shaping_produces_one_positioned_glyph_per_character() {
        let mut db = FontDatabase::probe_only();
        let run = shaped(&mut db, "Click me", 14.0);
        let blob = run.blob.as_ref().expect("shaping produced no runs");
        let glyphs: usize = blob.runs().iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(
            glyphs,
            "Click me".chars().count(),
            "Latin text with no ligatures should shape 1:1"
        );
        for run in blob.runs() {
            assert_eq!(run.glyphs.len(), run.positions.len());
        }
        let first = &blob.runs()[0];
        for pair in first.positions.windows(2) {
            assert!(pair[1].x > pair[0].x, "glyph positions did not advance");
        }
    }

    #[test]
    fn shaping_returns_the_blob_the_metrics_and_the_face_together() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let run = db.shape(&key("Click me", &face, 14.0));
        assert!(run.blob.is_some());
        assert_eq!(run.face, face);
        assert_eq!(run.size_px, 14.0);

        let empty = db.shape(&key("", &face, 14.0));
        assert!(empty.blob.is_none(), "empty text must not build a blob");
        assert_eq!(empty.metrics.width, 0.0);
        assert!(
            empty.metrics.line_height > 0.0,
            "an empty label still occupies a line"
        );
    }

    #[test]
    fn the_last_glyph_origin_sits_inside_the_measured_width() {
        let mut db = FontDatabase::probe_only();
        let run = shaped(&mut db, "Click me", 14.0);
        let blob = run.blob.as_ref().expect("blob");
        let last = blob.runs()[0].positions.last().copied().expect("positions");
        assert!(
            last.x < run.metrics.width && last.x > run.metrics.width * 0.5,
            "last glyph origin {} is not inside the measured width {}",
            last.x,
            run.metrics.width
        );
    }

    #[test]
    fn letter_spacing_widens_the_run_by_one_step_per_glyph() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let tight = db.shape(&key("Click me", &face, 14.0));
        let mut spaced_key = key("Click me", &face, 14.0);
        spaced_key.letter_spacing_px = 3.0;
        let spaced = db.shape(&spaced_key);

        let glyphs = "Click me".chars().count() as f32;
        assert!(
            (spaced.metrics.width - (tight.metrics.width + 3.0 * glyphs)).abs() < 0.01,
            "letter-spacing 3px over {glyphs} glyphs widened {} to {}",
            tight.metrics.width,
            spaced.metrics.width
        );

        let tight_positions = &tight.blob.as_ref().expect("blob").runs()[0].positions;
        let spaced_positions = &spaced.blob.as_ref().expect("blob").runs()[0].positions;
        // The nth glyph has n spacing steps in front of it; the first has none.
        assert!((spaced_positions[0].x - tight_positions[0].x).abs() < 0.01);
        assert!((spaced_positions[3].x - (tight_positions[3].x + 9.0)).abs() < 0.01);
    }

    #[test]
    fn a_transform_changes_the_glyphs_and_the_cache_key() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let plain = db.shape(&key("click", &face, 14.0));
        let mut upper_key = key("click", &face, 14.0);
        upper_key.transform = Keyword::Uppercase;
        let upper = db.shape(&upper_key);
        assert!(
            upper.metrics.width > plain.metrics.width,
            "CLICK ({}) is not wider than click ({}) — was the transform applied?",
            upper.metrics.width,
            plain.metrics.width
        );
        assert_eq!(db.shape_cache_len(), 2, "the transform is not part of the cache key");
    }

    #[test]
    fn identical_keys_are_served_from_the_shaping_cache() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let first = db.shape(&key("Click me", &face, 14.0));
        let second = db.shape(&key("Click me", &face, 14.0));
        assert!(Rc::ptr_eq(&first, &second), "the same key reshaped");
        assert_eq!(db.shape_cache_len(), 1);

        db.clear_caches();
        let third = db.shape(&key("Click me", &face, 14.0));
        assert!(!Rc::ptr_eq(&first, &third), "clear_caches did not drop the shaping cache");
    }

    #[test]
    fn shaping_never_panics_on_hostile_text() {
        let mut db = FontDatabase::probe_only();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        for text in [
            "",
            " ",
            "\u{0}",
            "\u{200b}\u{feff}",
            "مرحبا بالعالم",
            "🇯🇵👩‍👩‍👧‍👦",
            "e\u{301}\u{301}",
            "\u{10FFFF}",
        ] {
            for size in [0.0_f32, -12.0, 1.0, 4096.0, f32::NAN] {
                let mut hostile = key(text, &face, size);
                hostile.letter_spacing_px = if size.is_nan() { f32::INFINITY } else { -100.0 };
                let _ = db.shape(&hostile);
            }
        }
        let long = "Click me ".repeat(2_000);
        let _ = db.shape(&key(&long, &face, 14.0));
    }
```

Add the test-only accessor beside `match_cache_len`:

```rust
    #[cfg(test)]
    fn shape_cache_len(&self) -> usize {
        self.shapes.len()
    }
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import `super::ShapeKey`` and
`error[E0599]: no method named `shape` found for struct `FontDatabase``.

**Step 3 — implement.**

```rust
/// Everything that changes a shaped run: the text, the face, the size, the
/// letter spacing, the OpenType settings and the `text-transform`.
///
/// `features`/`variations` are part of the key even though this shaper stack
/// cannot honour them — when it can, the cache must not be serving runs shaped
/// without them.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeKey<'a> {
    /// The untransformed text.
    pub text: &'a str,
    /// The face resolved for this element.
    pub face: &'a FontFace,
    /// Computed `font-size`, in device pixels.
    pub size_px: f32,
    /// Computed `letter-spacing`, in device pixels.
    pub letter_spacing_px: f32,
    /// Computed `font-feature-settings`.
    pub features: &'a [FeatureSetting],
    /// Computed `font-variation-settings`.
    pub variations: &'a [VariationSetting],
    /// Computed `text-transform`.
    pub transform: Keyword,
}

fn shape_cache_key(key: &ShapeKey<'_>) -> String {
    let mut out = String::with_capacity(key.text.len() + 96);
    out.push_str(key.text);
    out.push('\u{1}');
    out.push_str(&key.face.path.to_string_lossy());
    out.push_str(&format!(
        "\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}",
        key.face.index,
        key.size_px.to_bits(),
        key.letter_spacing_px.to_bits(),
        key.transform.as_str()
    ));
    for feature in key.features {
        out.push_str(&format!(
            "{}{}\u{2}",
            String::from_utf8_lossy(&feature.tag),
            feature.value
        ));
    }
    out.push('\u{1}');
    for variation in key.variations {
        out.push_str(&format!(
            "{}{}\u{2}",
            String::from_utf8_lossy(&variation.tag),
            variation.value.to_bits()
        ));
    }
    out
}

impl FontDatabase {
    /// Shape and measure one run, cached by [`ShapeKey`].
    ///
    /// `text-transform` is applied first (it changes which characters exist),
    /// then the shaper runs, then `letter-spacing` is added after every glyph
    /// — CSS 2.1's rule, so a trailing step is part of the measured width.
    ///
    /// A face that will not load yields an empty run rather than `None`: a
    /// missing font must not take the widget tree down with it.
    pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText> {
        let cache_key = shape_cache_key(key);
        if let Some(cached) = self.shapes.get(&cache_key) {
            return Rc::clone(cached);
        }
        if !(key.features.is_empty() && key.variations.is_empty())
            && !self.warned_unsupported_shaping
        {
            self.warned_unsupported_shaping = true;
            tracing::warn!(
                features = key.features.len(),
                variations = key.variations.len(),
                "font-feature-settings and font-variation-settings cannot reach rustybuzz \
                 through skia-rs-text 0.4.0 (Shaper::shape passes an empty feature slice and \
                 exposes no variation axes); they are dropped at the shaper"
            );
        }
        let text = apply_text_transform(key.text, key.transform);
        let shaped = Rc::new(self.shape_transformed(&text, key));
        self.shapes.insert(cache_key, Rc::clone(&shaped));
        shaped
    }

    fn shape_transformed(&mut self, text: &str, key: &ShapeKey<'_>) -> ShapedText {
        let size_px = if key.size_px.is_finite() && key.size_px > 0.0 {
            key.size_px
        } else {
            0.0
        };
        let spacing = if key.letter_spacing_px.is_finite() {
            key.letter_spacing_px
        } else {
            0.0
        };
        let Some(font) = self.font(key.face, size_px) else {
            return ShapedText {
                blob: None,
                metrics: TextMetrics {
                    width: 0.0,
                    ascent: 0.0,
                    descent: 0.0,
                    line_height: 0.0,
                },
                face: key.face.clone(),
                size_px,
            };
        };
        let font_metrics = font.metrics();
        let runs = if text.is_empty() {
            None
        } else {
            self.shaper.shape_auto(text, &font)
        };

        let mut builder = TextBlobBuilder::new();
        let mut pen_x = 0.0f32;
        let mut any = false;
        if let Some(runs) = runs.as_ref() {
            for run in runs {
                let mut glyphs = Vec::with_capacity(run.glyphs.len());
                let mut positions = Vec::with_capacity(run.glyphs.len());
                for glyph in &run.glyphs {
                    glyphs.push(glyph.glyph_id.0);
                    positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                    pen_x += glyph.x_advance + spacing;
                }
                if !glyphs.is_empty() {
                    builder.add_positioned_run(&run.font, &glyphs, &positions);
                    any = true;
                }
            }
        }

        // Width comes from the shaper's advances when shaping succeeded (so
        // kerning and ligatures count), and from `Font::measure_text`'s
        // per-glyph `hmtx` sum otherwise -- M1's rule, unchanged.
        let width = if any {
            pen_x
        } else if text.is_empty() {
            0.0
        } else {
            font.measure_text(text) + spacing * text.chars().count() as f32
        };

        ShapedText {
            blob: if any { builder.build() } else { None },
            metrics: TextMetrics {
                width,
                // `FontMetrics::ascent` is negative (above the baseline), matching Skia.
                ascent: -font_metrics.ascent,
                descent: font_metrics.descent,
                line_height: font_metrics.line_height(),
            },
            face: key.face.clone(),
            size_px,
        }
    }
}
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo test -p icedtea-ui 2>&1 | tail -30
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

The second command must show the M1 gate `tests/themed_button_offscreen.rs`
still green: this task changed how the text stack is *reached*, not what it
measures.

**Mutation check.** Move `pen_x += glyph.x_advance + spacing;` to
`pen_x += glyph.x_advance;`:
`letter_spacing_widens_the_run_by_one_step_per_glyph` fails on both the width
and the position assertions. Drop `key.transform` from `shape_cache_key`:
`a_transform_changes_the_glyphs_and_the_cache_key` fails (the upper-case run is
served from the lower-case slot, so the widths match and the cache holds 1).

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs
git commit -m "feat(ui/text): ShapeKey-keyed shaping with letter-spacing and a run cache

Shaping now runs text-transform first, then rustybuzz, then adds letter-spacing
after every glyph (CSS 2.1), and caches the run by every input that can change
it. A face that will not load yields an empty run rather than None. M1's five
text tests survive as behaviour, with 'resolves to something' replacing the
fixed-family assertions, and font-feature/variation settings are warned about
once instead of being silently dropped.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 6 — `TextStyle`: computed text properties → query and key

The bridge the paint and layout layers use: one struct read out of a
`ComputedStyle`, holding everything the font stack needs.

**Files**

- Modify: `ui/src/text.rs` — append `TextStyle` and its private value readers.

**Interfaces**

- Consumes (P1/P3): `ComputedStyle::{raw, font_size_px}`, `Prop::{FontFamily,
  FontWeight, FontStyle, FontWidth, FontStretch, LetterSpacing,
  FontFeatureSettings, FontVariationSettings, TextTransform, LineHeight}`,
  `Value::{FontFamilies, FontWeight, FontStyle, Percentage, Number, Keyword,
  Length, FontFeatures, FontVariations, LineHeight}`, `Length::Abs`,
  `LengthUnit::Px`.
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub struct TextStyle {
      pub families: Rc<[FontFamily]>, pub weight: f32, pub style: FontStyle,
      pub stretch: f32, pub size_px: f32, pub letter_spacing_px: f32,
      pub features: Rc<[FeatureSetting]>, pub variations: Rc<[VariationSetting]>,
      pub transform: Keyword, pub line_height: LineHeight,
  }
  impl TextStyle {
      pub fn from_computed(style: &ComputedStyle) -> TextStyle;
      pub fn query(&self) -> FontQuery<'_>;
      pub fn shape_key<'a>(&'a self, text: &'a str, face: &'a FontFace) -> ShapeKey<'a>;
      pub fn line_height_px(&self, metrics: &TextMetrics) -> f32;
  }
  ```

**Step 1 — failing test.** In `mod tests`:

```rust
    use super::TextStyle;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::LineHeight;

    fn style_for(css: &str) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("label");
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(&sheet, &node, &ResolveEnv::default(), &mut cx)
    }

    #[test]
    fn the_text_style_reads_every_font_property_off_the_computed_style() {
        let style = style_for(
            "label { font-family: \"Adwaita Sans\", sans-serif; font-size: 20px; \
             font-weight: bold; font-style: italic; font-width: 75%; \
             letter-spacing: 2px; text-transform: uppercase; line-height: 1.5; }",
        );
        let text = TextStyle::from_computed(&style);
        assert_eq!(
            text.families.as_ref(),
            &[
                FontFamily::Named("Adwaita Sans".into()),
                FontFamily::Generic(GenericFamily::SansSerif),
            ]
        );
        assert_eq!(text.size_px, 20.0);
        assert_eq!(text.weight, 700.0);
        assert_eq!(text.style, FontStyle::Italic);
        assert_eq!(text.stretch, 75.0);
        assert_eq!(text.letter_spacing_px, 2.0);
        assert_eq!(text.transform, Keyword::Uppercase);
        assert_eq!(text.line_height, LineHeight::Number(1.5));
    }

    #[test]
    fn an_unstyled_node_gets_the_registrys_initial_font() {
        let text = TextStyle::from_computed(&style_for("other { color: red; }"));
        assert_eq!(
            text.families.as_ref(),
            &[FontFamily::Generic(GenericFamily::SansSerif)]
        );
        assert_eq!(text.weight, 400.0);
        assert_eq!(text.style, FontStyle::Normal);
        assert_eq!(text.stretch, 100.0);
        assert_eq!(text.letter_spacing_px, 0.0);
        assert_eq!(text.transform, Keyword::None);
        assert_eq!(text.line_height, LineHeight::Normal);
        assert!(text.features.is_empty() && text.variations.is_empty());
    }

    #[test]
    fn font_stretch_is_read_only_when_font_width_is_normal() {
        // GTK 4.22 registers both rows; font-width wins where it is set.
        let both = TextStyle::from_computed(&style_for(
            "label { font-width: 125%; font-stretch: condensed; }",
        ));
        assert_eq!(both.stretch, 125.0, "font-width must win over font-stretch");

        let stretch_only =
            TextStyle::from_computed(&style_for("label { font-stretch: condensed; }"));
        assert_eq!(stretch_only.stretch, 75.0, "the condensed keyword is 75%");
    }

    #[test]
    fn the_text_style_hands_the_database_a_query_and_a_key() {
        let style = style_for("label { font-size: 18px; text-transform: lowercase; }");
        let text = TextStyle::from_computed(&style);
        let mut db = FontDatabase::probe_only();
        let face = db.match_face(&text.query()).expect("system font");
        let run = db.shape(&text.shape_key("ABC", &face));
        assert_eq!(run.size_px, 18.0);
        assert!(run.blob.is_some());

        let untransformed = db.shape(&text.shape_key("abc", &face));
        assert!(
            (run.metrics.width - untransformed.metrics.width).abs() < 0.01,
            "text-transform: lowercase did not reach the shaper through shape_key"
        );
    }

    #[test]
    fn line_height_resolves_against_the_faces_metrics() {
        let metrics = TextMetrics {
            width: 0.0,
            ascent: 12.0,
            descent: 4.0,
            line_height: 18.0,
        };
        let normal = TextStyle::from_computed(&style_for("label { font-size: 20px; }"));
        assert_eq!(normal.line_height_px(&metrics), 18.0, "`normal` is the face's own");

        let numeric =
            TextStyle::from_computed(&style_for("label { font-size: 20px; line-height: 1.5; }"));
        assert_eq!(numeric.line_height_px(&metrics), 30.0);

        let absolute =
            TextStyle::from_computed(&style_for("label { font-size: 20px; line-height: 26px; }"));
        assert_eq!(absolute.line_height_px(&metrics), 26.0);
    }
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import `super::TextStyle``.

**Step 3 — implement.**

```rust
/// The computed text properties of one element, in the shape the font stack
/// consumes them.
///
/// Read once per restyle: [`TextStyle::query`] resolves the face and
/// [`TextStyle::shape_key`] shapes a label with it.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `font-family`, in priority order.
    pub families: Rc<[FontFamily]>,
    /// `font-weight`, computed to an absolute 1..=1000.
    pub weight: f32,
    /// `font-style`.
    pub style: FontStyle,
    /// `font-width`, falling back to `font-stretch`, as a percentage.
    pub stretch: f32,
    /// `font-size`, in device pixels.
    pub size_px: f32,
    /// `letter-spacing`, in device pixels (`normal` is 0).
    pub letter_spacing_px: f32,
    /// `font-feature-settings`.
    pub features: Rc<[FeatureSetting]>,
    /// `font-variation-settings`.
    pub variations: Rc<[VariationSetting]>,
    /// `text-transform`.
    pub transform: Keyword,
    /// `line-height`, unresolved (it needs the face's metrics — see
    /// [`TextStyle::line_height_px`]).
    pub line_height: LineHeight,
}

impl TextStyle {
    /// Read the text properties out of a computed style.
    ///
    /// Values are read raw and matched on their variant rather than coerced,
    /// so a property that somehow holds the wrong shape falls back to its
    /// initial value instead of to a coerced nonsense number.
    #[must_use]
    pub fn from_computed(style: &ComputedStyle) -> TextStyle {
        let families = match style.raw(Prop::FontFamily) {
            Value::FontFamilies(list) => Rc::clone(list),
            _ => Rc::from(vec![FontFamily::Generic(GenericFamily::SansSerif)]),
        };
        let weight = match style.raw(Prop::FontWeight) {
            Value::FontWeight(FontWeight::Absolute(w)) if w.is_finite() => w.clamp(1.0, 1000.0),
            _ => 400.0,
        };
        let font_style = match style.raw(Prop::FontStyle) {
            Value::FontStyle(s) => *s,
            _ => FontStyle::Normal,
        };
        // GTK 4.22 registers `font-width` and `font-stretch` as two rows with
        // one grammar; the L4 name wins where it is set.
        let width = stretch_percent(style.raw(Prop::FontWidth));
        let stretch = if (width - 100.0).abs() < f32::EPSILON {
            stretch_percent(style.raw(Prop::FontStretch))
        } else {
            width
        };
        let features = match style.raw(Prop::FontFeatureSettings) {
            Value::FontFeatures(list) => Rc::clone(list),
            _ => Rc::from(Vec::<FeatureSetting>::new()),
        };
        let variations = match style.raw(Prop::FontVariationSettings) {
            Value::FontVariations(list) => Rc::clone(list),
            _ => Rc::from(Vec::<VariationSetting>::new()),
        };
        TextStyle {
            families,
            weight,
            style: font_style,
            stretch,
            size_px: style.font_size_px(),
            letter_spacing_px: computed_px(style.raw(Prop::LetterSpacing)),
            features,
            variations,
            transform: match style.raw(Prop::TextTransform) {
                Value::Keyword(keyword) => *keyword,
                _ => Keyword::None,
            },
            line_height: match style.raw(Prop::LineHeight) {
                Value::LineHeight(line_height) => *line_height,
                _ => LineHeight::Normal,
            },
        }
    }

    /// The font query for these properties.
    #[must_use]
    pub fn query(&self) -> FontQuery<'_> {
        FontQuery {
            families: &self.families,
            weight: self.weight,
            style: self.style,
            stretch: self.stretch,
            size_px: self.size_px,
        }
    }

    /// The shaping key for `text` on `face`.
    #[must_use]
    pub fn shape_key<'a>(&'a self, text: &'a str, face: &'a FontFace) -> ShapeKey<'a> {
        ShapeKey {
            text,
            face,
            size_px: self.size_px,
            letter_spacing_px: self.letter_spacing_px,
            features: &self.features,
            variations: &self.variations,
            transform: self.transform,
        }
    }

    /// `line-height` in device pixels. `normal` is the face's own line height;
    /// a number multiplies the computed `font-size`.
    #[must_use]
    pub fn line_height_px(&self, metrics: &TextMetrics) -> f32 {
        match self.line_height {
            LineHeight::Normal => metrics.line_height,
            LineHeight::Number(factor) if factor.is_finite() && factor >= 0.0 => {
                factor * self.size_px
            }
            LineHeight::Number(_) => metrics.line_height,
            LineHeight::Length(ref length) => absolute_px(length).unwrap_or(metrics.line_height),
        }
    }
}

/// A computed length as device pixels. Computed values are already resolved to
/// `px` (the contract's resolution order), so anything else is a bug upstream
/// and reads as 0 rather than as a guess.
fn computed_px(value: &Value) -> f32 {
    match value {
        Value::Length(length) => absolute_px(length).unwrap_or(0.0),
        Value::Number(number) if number.is_finite() => *number,
        _ => 0.0,
    }
}

fn absolute_px(length: &Length) -> Option<f32> {
    match length {
        Length::Abs {
            value,
            unit: LengthUnit::Px,
        } if value.is_finite() => Some(*value),
        _ => None,
    }
}

/// A computed `font-width`/`font-stretch` as a percentage. `Value::Percentage`
/// is a fraction (1.0 == 100%), per the value contract.
fn stretch_percent(value: &Value) -> f32 {
    match value {
        Value::Percentage(fraction) if fraction.is_finite() => fraction * 100.0,
        Value::Number(number) if number.is_finite() => *number,
        Value::Keyword(keyword) => match keyword {
            Keyword::UltraCondensed => 50.0,
            Keyword::ExtraCondensed => 62.5,
            Keyword::Condensed => 75.0,
            Keyword::SemiCondensed => 87.5,
            Keyword::SemiExpanded => 112.5,
            Keyword::Expanded => 125.0,
            Keyword::ExtraExpanded => 150.0,
            Keyword::UltraExpanded => 200.0,
            _ => 100.0,
        },
        _ => 100.0,
    }
}
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --lib text::tests 2>&1 | tail -20
cargo test -p icedtea-ui 2>&1 | tail -30
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Swap `from_computed`'s width/stretch preference (read
`FontStretch` first, fall back to `FontWidth`):
`font_stretch_is_read_only_when_font_width_is_normal` fails on the first
assertion (125 → 75). Make `line_height_px` return `metrics.line_height`
unconditionally: `line_height_resolves_against_the_faces_metrics` fails on both
the numeric and the absolute cases.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/src/text.rs
git commit -m "feat(ui/text): TextStyle bridges computed properties to the font stack

One read of the computed style yields the family list, size, weight, style,
width (font-width first, font-stretch behind it), letter-spacing, feature and
variation settings, text-transform and line-height, and hands out the
FontQuery and ShapeKey the database consumes. Values are matched on their
variant rather than coerced, so a wrong-shaped value lands on its initial.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 7 — Vendor Adwaita dark and high-contrast

The gate compiles three sheets, so three sheets must be in the tree. Same
provenance rule as M1's light sheet: verbatim `gresource` extracts, LGPL note,
hermetic.

**Files**

- Create: `ui/themes/adwaita-dark.css` — GTK 4.22 `Default-dark.css`, verbatim.
- Create: `ui/themes/adwaita-hc.css` — GTK 4.22 `Default-hc.css`, verbatim.
- Modify: `ui/themes/README.md` — one section per new file, plus the shared
  provenance/licence note.

**Interfaces** — none (data files).

**Step 1 — failing test.** Create `ui/tests/adwaita_coverage.rs` with only the
vendoring assertions for now (Tasks 9 and 10 fill the same file with the gate):

```rust
//! THE M2 GATE — every declaration of GTK 4.22's Adwaita, through the registry.
//!
//! Light, dark and high-contrast are all walked: 0 unparseable declarations,
//! 0 unknown properties, 37/37 `@define-color`s resolved. See
//! `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
//! section 7.

/// GTK 4.22 `Default-dark.css`, vendored so this gate is hermetic.
const ADWAITA_DARK: &str = include_str!("../themes/adwaita-dark.css");
/// GTK 4.22 `Default-hc.css`, vendored so this gate is hermetic.
const ADWAITA_HC: &str = include_str!("../themes/adwaita-hc.css");

#[test]
fn the_vendored_sheets_are_the_extracted_gtk4_themes() {
    assert_eq!(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT.lines().count(),
        1941,
        "vendored Adwaita light is not GTK 4.22's 1,941-line Default-light.css"
    );
    assert_eq!(
        ADWAITA_DARK.lines().count(),
        1929,
        "vendored Adwaita dark is not GTK 4.22's 1,929-line Default-dark.css"
    );
    assert_eq!(
        ADWAITA_HC.lines().count(),
        1944,
        "vendored Adwaita high-contrast is not GTK 4.22's 1,944-line Default-hc.css"
    );
    for (name, sheet) in [
        ("light", icedtea_ui::BUNDLED_ADWAITA_LIGHT),
        ("dark", ADWAITA_DARK),
        ("hc", ADWAITA_HC),
    ] {
        assert_eq!(
            sheet.matches("@define-color").count(),
            37,
            "vendored Adwaita {name} does not carry GTK 4.22's 37 @define-color declarations"
        );
    }
}

#[test]
fn the_three_sheets_are_three_different_themes() {
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_DARK);
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_HC);
    assert_ne!(ADWAITA_DARK, ADWAITA_HC);
    // The colour that separates them, declared in each sheet's own words.
    assert!(icedtea_ui::BUNDLED_ADWAITA_LIGHT.contains("@define-color theme_bg_color #f6f5f4"));
    assert!(ADWAITA_DARK.contains("@define-color theme_bg_color #353535"));
    assert!(ADWAITA_HC.contains("@define-color theme_bg_color #fdfdfc"));
}
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -20
```

Expected: `error: couldn't read ui/tests/../themes/adwaita-dark.css: No such
file or directory`.

**Step 3 — implement.** Extract both sheets from the installed GTK 4.22
(`gtk4 4.22.4` on this machine, `/usr/lib/libgtk-4.so.1`):

```bash
cd /home/joseph/Projects/icedtea
gresource extract /usr/lib/libgtk-4.so.1 \
    /org/gtk/libgtk/theme/Default/Default-dark.css > ui/themes/adwaita-dark.css
gresource extract /usr/lib/libgtk-4.so.1 \
    /org/gtk/libgtk/theme/Default/Default-hc.css > ui/themes/adwaita-hc.css
wc -l ui/themes/adwaita-dark.css ui/themes/adwaita-hc.css
grep -c '@define-color' ui/themes/adwaita-dark.css ui/themes/adwaita-hc.css
```

Expected: `1929` / `1944` lines and `37` / `37` colours. Confirm the light sheet
in the tree is still bit-identical to the same GTK build, so all three come from
one source:

```bash
gresource extract /usr/lib/libgtk-4.so.1 \
    /org/gtk/libgtk/theme/Default/Default-light.css | diff - ui/themes/adwaita-light.css \
    && echo "light sheet unchanged"
```

Then append to `ui/themes/README.md`, after the existing `adwaita-light.css`
section and before the licence paragraph it shares:

```markdown
## `adwaita-dark.css`

Verbatim extract of GTK 4's default dark theme:

    gresource extract /usr/lib/libgtk-4.so.1 \
        /org/gtk/libgtk/theme/Default/Default-dark.css

Extracted 2026-08-26 from GTK 4.22.4 as packaged on Arch Linux (1,929 lines,
37 `@define-color` declarations).

## `adwaita-hc.css`

Verbatim extract of GTK 4's default high-contrast theme:

    gresource extract /usr/lib/libgtk-4.so.1 \
        /org/gtk/libgtk/theme/Default/Default-hc.css

Extracted 2026-08-26 from GTK 4.22.4 as packaged on Arch Linux (1,944 lines,
37 `@define-color` declarations).

Both are vendored for the same reason the light sheet is, and carry the same
LGPL-2.1-or-later terms: `tests/adwaita_coverage.rs` — the M2 gate — walks every
declaration of all three through the property registry, and it must assert
against a fixed, known theme rather than whatever GTK happens to be installed.
Neither sheet is a runtime default: `icedtea-ui` still loads the user's own
theme first and falls back to `adwaita-light.css`.
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Delete one line from `ui/themes/adwaita-dark.css`:
`the_vendored_sheets_are_the_extracted_gtk4_themes` fails on the 1,929-line
assertion. Copy the light sheet over the dark one:
`the_three_sheets_are_three_different_themes` fails on both the `assert_ne!` and
the `theme_bg_color` assertion.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/themes/adwaita-dark.css ui/themes/adwaita-hc.css ui/themes/README.md \
        ui/tests/adwaita_coverage.rs
git commit -m "test(ui): vendor GTK 4.22 Adwaita dark and high-contrast

The M2 coverage gate compiles three sheets, so all three are in the tree,
extracted verbatim from the same GTK 4.22.4 gresource as the light sheet and
carrying the same LGPL-2.1-or-later note. 1,929 and 1,944 lines, 37
@define-colors each, pinned by the test that reads them.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 8 — `tests/gtk4_property_reference.rs`: the registry *is* the GTK 4.22 table

Reference conformance (spec §7): a vendored fixture of the GTK 4.22 property
names, with kind and inherited flag, must match `PROPERTIES` exactly — same
names, same longhand/shorthand split, same inheritance, same order.

**Files**

- Create: `ui/tests/fixtures/gtk4.22-css-properties.txt` — 113 data rows.
- Create: `ui/tests/gtk4_property_reference.rs`.

**Interfaces**

- Consumes: `icedtea_ui::css::registry::{lookup, longhands, Prop, PropertyKind,
  PROPERTIES, N_LONGHANDS, N_PROPS}`, `Prop::{name, is_longhand, is_inherited}`.
- Produces: no library API — a gate test.

**Step 1 — failing test.** Create `ui/tests/gtk4_property_reference.rs`:

```rust
//! Reference conformance: the property registry is GTK 4.22's CSS property
//! table, not a subset of it and not a superset.
//!
//! The fixture is vendored text rather than a live fetch so the gate is
//! hermetic and reviewable in a diff.

use icedtea_ui::css::registry::{lookup, PropertyKind, N_LONGHANDS, N_PROPS, PROPERTIES};

const REFERENCE: &str = include_str!("fixtures/gtk4.22-css-properties.txt");

#[derive(Debug, PartialEq, Eq)]
struct Row {
    name: String,
    longhand: bool,
    inherited: bool,
}

/// `name|L|yes` / `name|S|no`; `#` comments and blank lines are skipped.
/// A malformed row is a test failure, never a silently dropped line.
fn reference_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (number, line) in REFERENCE.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('|').collect();
        assert_eq!(
            fields.len(),
            3,
            "fixture line {} is not `name|kind|inherited`: {line:?}",
            number + 1
        );
        let longhand = match fields[1] {
            "L" => true,
            "S" => false,
            other => panic!("fixture line {} has kind {other:?}, not L or S", number + 1),
        };
        let inherited = match fields[2] {
            "yes" => true,
            "no" => false,
            other => panic!(
                "fixture line {} has inherited {other:?}, not yes or no",
                number + 1
            ),
        };
        rows.push(Row {
            name: fields[0].to_string(),
            longhand,
            inherited,
        });
    }
    rows
}

#[test]
fn every_gtk4_property_is_registered() {
    let rows = reference_rows();
    let mut missing = Vec::new();
    let mut wrong_kind = Vec::new();
    let mut wrong_inheritance = Vec::new();

    for row in &rows {
        let Some(prop) = lookup(&row.name) else {
            missing.push(row.name.clone());
            continue;
        };
        if prop.is_longhand() != row.longhand {
            wrong_kind.push(format!(
                "{}: registry says {}, GTK 4.22 says {}",
                row.name,
                if prop.is_longhand() { "longhand" } else { "shorthand" },
                if row.longhand { "longhand" } else { "shorthand" },
            ));
        }
        if prop.is_inherited() != row.inherited {
            wrong_inheritance.push(format!(
                "{}: registry says inherited={}, GTK 4.22 says {}",
                row.name,
                prop.is_inherited(),
                row.inherited
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "{} GTK 4.22 properties are not in the registry: {missing:?}",
        missing.len()
    );
    assert!(wrong_kind.is_empty(), "wrong longhand/shorthand kind: {wrong_kind:?}");
    assert!(
        wrong_inheritance.is_empty(),
        "wrong inherited flag: {wrong_inheritance:?}"
    );
}

#[test]
fn the_registry_holds_nothing_gtk_does_not() {
    let rows = reference_rows();
    let extra: Vec<&str> = PROPERTIES
        .iter()
        .map(|def| def.name)
        .filter(|name| !rows.iter().any(|row| row.name == *name))
        .collect();
    assert!(
        extra.is_empty(),
        "the registry invents {} properties GTK 4.22 does not have: {extra:?}",
        extra.len()
    );
}

#[test]
fn the_registry_row_order_is_the_reference_order() {
    let rows = reference_rows();
    assert_eq!(
        rows.len(),
        N_PROPS,
        "the fixture has {} rows, the registry has {N_PROPS}",
        rows.len()
    );
    assert_eq!(
        rows.iter().filter(|row| row.longhand).count(),
        N_LONGHANDS,
        "longhand count disagrees with N_LONGHANDS"
    );
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            PROPERTIES[index].name, row.name,
            "registry slot {index} is {:?}, the reference says {:?}",
            PROPERTIES[index].name, row.name
        );
    }
    // Longhands occupy 0..N_LONGHANDS, shorthands the rest: the ComputedStyle
    // slot rule depends on it.
    for (index, def) in PROPERTIES.iter().enumerate() {
        let is_longhand = matches!(def.kind, PropertyKind::Longhand { .. });
        assert_eq!(
            is_longhand,
            index < N_LONGHANDS,
            "{} sits at slot {index}, on the wrong side of N_LONGHANDS",
            def.name
        );
    }
}

#[test]
fn property_lookup_is_ascii_case_insensitive() {
    for row in reference_rows() {
        let upper = row.name.to_ascii_uppercase();
        assert_eq!(
            lookup(&upper),
            lookup(&row.name),
            "{upper} and {} resolve differently",
            row.name
        );
    }
    assert_eq!(lookup("nosuchproperty"), None);
    assert_eq!(lookup(""), None);
    assert_eq!(lookup("-gtk-"), None);
}
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --test gtk4_property_reference 2>&1 | tail -20
```

Expected: `error: couldn't read ui/tests/fixtures/gtk4.22-css-properties.txt:
No such file or directory`.

**Step 3 — implement.** Create
`ui/tests/fixtures/gtk4.22-css-properties.txt` with exactly this content:

```text
# GTK 4.22 CSS property reference — vendored fixture for
# tests/gtk4_property_reference.rs.
#
# Source: https://docs.gtk.org/gtk4/css-properties.html, checked against the
# GTK 4.22.4 build installed on this machine (/usr/lib/libgtk-4.so.1).
# Captured 2026-08-26.
#
# The GTK page publishes each property's reference standard and GTK's
# divergences from it, but not the inherited flag; that column comes from the
# cited W3C spec, with GTK's notes applied as overrides (see
# .superpowers/m2-plan-notes/gtk-css-semantics.md section 1).
#
# Format: <property>|<L longhand | S shorthand>|<inherited: yes|no>
# Order is the registry's row order: longhands first, then shorthands.
# Shorthands have no inherited flag of their own (they inherit per longhand),
# so they are recorded as `no`, matching `Prop::is_inherited`.
color|L|yes
opacity|L|no
filter|L|no
font-family|L|yes
font-size|L|yes
font-style|L|yes
font-variant|L|yes
font-weight|L|yes
font-width|L|yes
font-stretch|L|yes
font-kerning|L|yes
font-variant-ligatures|L|yes
font-variant-position|L|yes
font-variant-caps|L|yes
font-variant-numeric|L|yes
font-variant-alternates|L|yes
font-variant-east-asian|L|yes
font-feature-settings|L|yes
font-variation-settings|L|yes
-gtk-dpi|L|yes
caret-color|L|yes
-gtk-secondary-caret-color|L|yes
letter-spacing|L|yes
text-transform|L|yes
line-height|L|yes
text-decoration-line|L|no
text-decoration-color|L|no
text-decoration-style|L|no
text-shadow|L|yes
-gtk-icon-source|L|no
-gtk-icon-size|L|yes
-gtk-icon-style|L|yes
-gtk-icon-transform|L|yes
-gtk-icon-palette|L|yes
-gtk-icon-shadow|L|yes
-gtk-icon-filter|L|yes
-gtk-icon-weight|L|yes
transform|L|no
transform-origin|L|no
min-width|L|no
min-height|L|no
margin-top|L|no
margin-right|L|no
margin-bottom|L|no
margin-left|L|no
padding-top|L|no
padding-right|L|no
padding-bottom|L|no
padding-left|L|no
border-top-width|L|no
border-right-width|L|no
border-bottom-width|L|no
border-left-width|L|no
border-top-style|L|no
border-right-style|L|no
border-bottom-style|L|no
border-left-style|L|no
border-top-left-radius|L|no
border-top-right-radius|L|no
border-bottom-right-radius|L|no
border-bottom-left-radius|L|no
border-top-color|L|no
border-right-color|L|no
border-bottom-color|L|no
border-left-color|L|no
border-image-source|L|no
border-image-repeat|L|no
border-image-slice|L|no
border-image-width|L|no
outline-style|L|no
outline-width|L|no
outline-color|L|no
outline-offset|L|no
background-color|L|no
background-clip|L|no
background-origin|L|no
background-size|L|no
background-position|L|no
background-repeat|L|no
background-image|L|no
box-shadow|L|no
background-blend-mode|L|no
transition-property|L|no
transition-duration|L|no
transition-timing-function|L|no
transition-delay|L|no
animation-name|L|no
animation-duration|L|no
animation-timing-function|L|no
animation-iteration-count|L|no
animation-direction|L|no
animation-play-state|L|no
animation-delay|L|no
animation-fill-mode|L|no
border-spacing|L|yes
font|S|no
text-decoration|S|no
margin|S|no
padding|S|no
border-width|S|no
border-style|S|no
border-color|S|no
border-top|S|no
border-right|S|no
border-bottom|S|no
border-left|S|no
border|S|no
border-radius|S|no
border-image|S|no
outline|S|no
background|S|no
transition|S|no
animation|S|no
```

Verify the shape of the fixture before running the test:

```bash
cd /home/joseph/Projects/icedtea
grep -vc '^#' ui/tests/fixtures/gtk4.22-css-properties.txt          # expect 113
grep -c '|L|' ui/tests/fixtures/gtk4.22-css-properties.txt          # expect 95
grep -c '|S|' ui/tests/fixtures/gtk4.22-css-properties.txt          # expect 18
cut -d'|' -f1 ui/tests/fixtures/gtk4.22-css-properties.txt | grep -v '^#' | sort | uniq -d
                                                                     # expect no output
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --test gtk4_property_reference 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

If `every_gtk4_property_is_registered` fails here, the defect is in **P1's
registry**, not in this fixture: the fixture is the specification. Report the
named properties rather than editing the fixture to agree with the code — the
only legitimate fixture edit is one backed by a fresh reading of
`docs.gtk.org/gtk4/css-properties.html`, noted in the commit body.

**Mutation check.** Flip `border-spacing`'s registry row to `inherited: false`:
`every_gtk4_property_is_registered` fails naming `border-spacing`. Swap two
adjacent rows in `PROPERTIES` (e.g. `font-width` and `font-stretch`):
`the_registry_row_order_is_the_reference_order` fails at that slot. Register an
invented `-gtk-nonsense` longhand: `the_registry_holds_nothing_gtk_does_not`
fails.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/tests/fixtures/gtk4.22-css-properties.txt ui/tests/gtk4_property_reference.rs
git commit -m "test(ui): the registry is GTK 4.22's CSS property table

113 vendored rows — name, longhand/shorthand kind, inherited flag, in registry
order — asserted both ways: nothing in the GTK 4.22 reference is missing and
nothing invented is present. Also pins the longhands-first slot rule that
ComputedStyle indexes by, and that lookup is ASCII-case-insensitive.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 9 — The coverage instrument, and Adwaita light at 0 unknown / 0 unparseable

THE M2 GATE, first half. The instrument walks a sheet's every rule ×
declaration through the registry and reports, by name, anything the engine
cannot read. A negative control proves the instrument can fail.

**Files**

- Modify: `ui/tests/adwaita_coverage.rs` — add the instrument and the light-sheet
  assertions above Task 7's vendoring tests.

**Interfaces**

- Consumes: `icedtea_ui::css::parse::{parse_stylesheet, Declaration,
  KeyframesRule, StyleRule, Stylesheet}`, `icedtea_ui::css::registry::{lookup,
  PropertyKind}`, `Prop::{def, expand_into}`, `cssparser::{Parser,
  ParserInput}`, `Parser::is_exhausted`.
- Produces: no library API — a gate test.

**Step 1 — failing test.** Insert into `ui/tests/adwaita_coverage.rs`, after the
two `const` declarations:

```rust
use cssparser::{Parser, ParserInput};
use icedtea_ui::css::parse::{parse_stylesheet, Declaration, KeyframesRule, StyleRule, Stylesheet};
use icedtea_ui::css::registry::{lookup, PropertyKind};

/// What one sheet's walk found.
#[derive(Default)]
struct Coverage {
    rules: usize,
    declarations: usize,
    /// Property names the registry does not know, in source order.
    unknown: Vec<String>,
    /// `name: value` for declarations the registry knows but cannot parse.
    unparseable: Vec<String>,
}

impl Coverage {
    /// Walk one sheet: its rules, its `@keyframes`, and every `@media` block's
    /// rules and keyframes. `@media` blocks are kept unevaluated by the parser,
    /// so a declaration inside one is still the engine's to read.
    fn walk(css: &str) -> (Coverage, Stylesheet) {
        let sheet = parse_stylesheet(css);
        let mut coverage = Coverage::default();
        coverage.rules(&sheet.rules);
        coverage.keyframes(&sheet.keyframes);
        for block in &sheet.media_blocks {
            coverage.rules(&block.rules);
            coverage.keyframes(&block.keyframes);
        }
        (coverage, sheet)
    }

    fn rules(&mut self, rules: &[StyleRule]) {
        for rule in rules {
            self.rules += 1;
            for declaration in &rule.declarations {
                self.declaration(declaration);
            }
        }
    }

    fn keyframes(&mut self, keyframes: &[KeyframesRule]) {
        for rule in keyframes {
            for (_offsets, declarations) in &rule.frames {
                for declaration in declarations {
                    self.declaration(declaration);
                }
            }
        }
    }

    fn declaration(&mut self, declaration: &Declaration) {
        self.declarations += 1;
        let Some(prop) = lookup(&declaration.name) else {
            self.unknown.push(declaration.name.clone());
            return;
        };
        let mut input = ParserInput::new(&declaration.value);
        let mut parser = Parser::new(&mut input);
        let parsed = match &prop.def().kind {
            PropertyKind::Longhand { parse, .. } => {
                let parse = *parse;
                parse(&mut parser).is_ok()
            }
            PropertyKind::Shorthand { .. } => prop
                .expand_into(&mut parser, &mut |_prop, _value| {})
                .is_ok(),
        };
        // A parser that stopped early left tokens behind: that is a partial
        // read, which is a failure, not a success.
        if !(parsed && parser.is_exhausted()) {
            self.unparseable
                .push(format!("{}: {}", declaration.name, declaration.value));
        }
    }

    /// Fail with the offending declarations named, not with a bare count.
    fn assert_complete(&self, sheet: &str) {
        let mut unknown = self.unknown.clone();
        unknown.sort();
        unknown.dedup();
        assert!(
            self.unknown.is_empty(),
            "{sheet}: {} declarations use {} property names the registry does not know: {unknown:#?}",
            self.unknown.len(),
            unknown.len()
        );
        assert!(
            self.unparseable.is_empty(),
            "{sheet}: {} declarations the registry knows but cannot parse:\n{}",
            self.unparseable.len(),
            self.unparseable.join("\n")
        );
    }
}

#[test]
fn adwaita_light_has_no_unknown_properties_and_no_unparseable_declarations() {
    let (coverage, _sheet) = Coverage::walk(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    assert!(
        coverage.rules > 500,
        "only {} rules walked — the sheet did not parse",
        coverage.rules
    );
    assert!(
        coverage.declarations > 850,
        "only {} declarations walked — the sheet did not parse",
        coverage.declarations
    );
    coverage.assert_complete("Adwaita light");
}

#[test]
fn the_instrument_reports_what_it_cannot_read() {
    // Negative control: without this, a walk that silently accepted everything
    // would still pass the gate.
    let (coverage, _sheet) = Coverage::walk(
        "button { -gtk-not-a-property: 1px; color: nosuchfunction(1); \
         border: 1px solid #cdc7c2; }",
    );
    assert_eq!(coverage.rules, 1);
    assert_eq!(coverage.declarations, 3);
    assert_eq!(coverage.unknown, vec!["-gtk-not-a-property".to_string()]);
    assert_eq!(
        coverage.unparseable,
        vec!["color: nosuchfunction(1)".to_string()],
        "an uninterpretable value must be reported, not counted as read"
    );
}

#[test]
fn a_trailing_token_is_a_failed_read_not_a_partial_one() {
    // `parse_entirely`'s rule, asserted directly: a parser that stops early has
    // not read the declaration.
    let (coverage, _sheet) = Coverage::walk("button { opacity: 0.5 0.5; }");
    assert_eq!(
        coverage.unparseable,
        vec!["opacity: 0.5 0.5".to_string()],
        "a value with a trailing token was accepted"
    );
}

#[test]
fn keyframe_and_media_declarations_are_walked_too() {
    let (coverage, sheet) = Coverage::walk(
        "@keyframes spin { to { transform: rotate(1turn); } } \
         @media (prefers-color-scheme: dark) { button { color: white; } }",
    );
    assert_eq!(sheet.keyframes.len(), 1, "the @keyframes rule was dropped");
    assert_eq!(sheet.media_blocks.len(), 1, "the @media block was dropped");
    assert_eq!(
        coverage.declarations, 2,
        "a declaration inside @keyframes or @media was not walked"
    );
    coverage.assert_complete("synthetic");
}
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -40
```

Expected, in order of likelihood: a real list of Adwaita declarations the
engine cannot read (the gate doing its job — e.g.
`transform: rotate(1turn)` if P1's angle parser skipped `turn`,
`-gtk-icon-filter: brightness(1.2)`, `cross-fade(...)`,
`background-image: -gtk-icontheme(...)`), each named in the panic message.
Every failure is a **P1 registry/value defect**, not a defect in this test: fix
it in `css/registry.rs` or `css/value/**` and re-run. Do not narrow the gate.

**Step 3 — implement.** There is nothing to implement in this part: the
instrument *is* the deliverable, and the code above is complete. Work the
failure list until it is empty, one property family at a time, in
`ui/src/css/registry.rs` / `ui/src/css/value/**` (P1's files — this is the one
sanctioned reason for P6 to touch them, and every such edit is its own commit
naming the declaration that forced it):

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 \
  | sed -n '/cannot parse/,/^$/p' | head -40
```

If a declaration is genuinely not GTK 4.22 CSS, the answer is still not to
weaken the gate: check it against `docs.gtk.org/gtk4/css-properties.html`,
record the finding in the commit body, and add the row to the registry.

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Drop the `&& parser.is_exhausted()` clause:
`a_trailing_token_is_a_failed_read_not_a_partial_one` fails. Make
`declaration()` return early for unknown names without pushing:
`the_instrument_reports_what_it_cannot_read` fails on the `unknown` assertion.
Remove the `media_blocks` loop from `walk`:
`keyframe_and_media_declarations_are_walked_too` fails on the declaration count.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/tests/adwaita_coverage.rs
git commit -m "test(ui): the M2 coverage instrument, and Adwaita light at 0/0

Walks every rule, @keyframes frame and @media block declaration of a sheet
through the registry and names what it cannot read, rather than counting.
A trailing token is a failed read, not a partial one. Three negative controls
prove the instrument can fail, so a green light sheet means something.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 10 — The gate: dark, high-contrast, and 37/37 `@define-color`s

THE M2 GATE, second half — the spec's headline number, and the one that inverts
M1's ruling: M1 pinned **29** of 37 colours resolved, M2 requires **37**
(relative colour syntax included).

**Files**

- Modify: `ui/tests/adwaita_coverage.rs` — add the colour resolution walk and
  the dark/high-contrast sheet tests.

**Interfaces**

- Consumes: `icedtea_ui::css::value::color::{build_color_table, ColorCtx,
  ColorValue, Rgba}`, `Stylesheet::color_definitions`,
  `MediaBlock::color_definitions`.
- Produces: no library API — a gate test.

**Step 1 — failing test.** Append to `ui/tests/adwaita_coverage.rs`:

```rust
use icedtea_ui::css::value::color::{build_color_table, ColorCtx, Rgba};

/// Resolve every `@define-color` in a sheet, returning the names that did not
/// resolve. Definitions inside `@media` blocks count too.
fn unresolved_colors(sheet: &Stylesheet) -> Vec<String> {
    let mut definitions = sheet.color_definitions.clone();
    for block in &sheet.media_blocks {
        definitions.extend(block.color_definitions.iter().cloned());
    }
    let table = build_color_table(&definitions);
    let ctx = ColorCtx {
        table: &table,
        // `@define-color` bodies are resolved outside any element, so
        // `currentColor` has nothing to be but the initial colour.
        current: Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
        depth: 0,
    };
    let mut unresolved = Vec::new();
    for (name, _source) in &definitions {
        match table.get(name) {
            Some(value) => {
                if value.resolve(&ctx).is_none() {
                    unresolved.push(name.clone());
                }
            }
            None => unresolved.push(name.clone()),
        }
    }
    unresolved
}

/// One sheet, walked end to end: the whole gate for that sheet.
fn assert_sheet_is_fully_covered(name: &str, css: &str) {
    let (coverage, sheet) = Coverage::walk(css);
    assert!(
        coverage.rules > 500 && coverage.declarations > 850,
        "{name}: only {} rules / {} declarations walked — the sheet did not parse",
        coverage.rules,
        coverage.declarations
    );
    coverage.assert_complete(name);

    let definitions = sheet.color_definitions.len();
    assert_eq!(
        definitions, 37,
        "{name}: {definitions} @define-color declarations reached the sheet, GTK 4.22 has 37"
    );
    let unresolved = unresolved_colors(&sheet);
    assert!(
        unresolved.is_empty(),
        "{name}: {}/37 @define-colors resolved; these did not: {unresolved:#?}",
        37 - unresolved.len()
    );
}

#[test]
fn adwaita_light_resolves_all_37_define_colors() {
    // M1 pinned 29/37: the eight relative-colour definitions
    // (`hsl(from … calc(s * 1.8) …)`, `rgb(from black r g b / calc(alpha * .35))`)
    // were recorded unresolved rather than fabricated. Resolving all 37 is an
    // M2 deliverable (spec Decision 6 / contract section 2.4), not a regression.
    let sheet = parse_stylesheet(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    assert_eq!(sheet.color_definitions.len(), 37);
    assert!(
        sheet
            .color_definitions
            .iter()
            .any(|(name, value)| name == "wm_title" && value.contains("hsl(from")),
        "the relative-colour definitions are not in the sheet this test claims to cover"
    );
    assert!(unresolved_colors(&sheet).is_empty());
}

#[test]
fn a_definition_may_reference_a_name_defined_later() {
    // Lazy, order-independent resolution — M1 required earlier-only.
    let sheet = parse_stylesheet(
        "@define-color a alpha(@b, 0.5); @define-color b #3584e4; button { color: @a; }",
    );
    assert!(
        unresolved_colors(&sheet).is_empty(),
        "a forward reference did not resolve"
    );
}

#[test]
fn a_colour_cycle_is_broken_not_hung() {
    let sheet = parse_stylesheet("@define-color a shade(@b, 1.1); @define-color b shade(@a, 1.1);");
    // The point is that this returns at all; both names are legitimately
    // unresolvable, and neither may hang or overflow the stack.
    assert_eq!(unresolved_colors(&sheet).len(), 2);
}

#[test]
fn the_m2_gate_adwaita_light_dark_and_high_contrast() {
    assert_sheet_is_fully_covered("Adwaita light", icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    assert_sheet_is_fully_covered("Adwaita dark", ADWAITA_DARK);
    assert_sheet_is_fully_covered("Adwaita high-contrast", ADWAITA_HC);
}
```

**Step 2 — run (expect failure).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -40
```

Expected: either `unresolved_colors` naming the eight relative-colour
definitions (a P1 `value/color.rs` gap), or the dark/high-contrast sheets naming
declarations the light sheet never exercised. Both are real gaps; fix them
upstream, one commit per family, and re-run.

**Step 3 — implement.** As in Task 9, the test *is* the deliverable. Work the
list until all three sheets are clean. Useful narrowing while doing so:

```bash
cargo test -p icedtea-ui --test adwaita_coverage the_m2_gate 2>&1 | head -60
```

**Step 4 — run (expect pass).**

```bash
cargo test -p icedtea-ui --test adwaita_coverage 2>&1 | tail -20
cargo test -p icedtea-ui 2>&1 | tail -30
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Make `unresolved_colors` skip definitions whose source
contains `from` (i.e. quietly excuse relative colours):
`adwaita_light_resolves_all_37_define_colors` still passes but
`a_colour_cycle_is_broken_not_hung` fails on the count — and re-adding the
excuse to that test too is caught by review, which is why the 37 assertion names
the failures rather than counting them. Directly: change `assert_eq!(definitions,
37)` to `>= 29` and the gate stops being the gate — that line is the M2
deliverable and must not move.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/tests/adwaita_coverage.rs
git commit -m "test(ui): THE M2 GATE — light, dark and high-contrast at 0/0/37

Every rule, keyframe and media-block declaration of all three vendored GTK 4.22
Adwaita sheets goes through the registry with nothing unknown and nothing
unparseable, and all 37 @define-colors resolve — including the eight relative
colour definitions M1 recorded as unresolved (29/37). Forward references and
cycles are covered too: lazy, order-independent, depth-guarded.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 11 — `ui/README.md` for M2

The README still describes an M1 crate that paints a uniform border, falls back
to runner-ups, and probes eight font paths. Rewrite the parts M2 invalidated;
keep everything M2 did not change (theme stack, Wayland behaviour, vendored
files) as it stands.

**Files**

- Modify: `ui/README.md` — replace the "What M1 covers", "CSS engine behaviour"
  (three bullets), "Tests" and "Deliberately not covered by M1" sections; add a
  "Fonts and text" section; leave "Running it", "The theme stack", the Wayland
  bullets and "Vendored files" intact except where a named number changed.

**Interfaces** — none (documentation).

**Step 1 — verification, not a test.** A README claim that is not true of the
tree is a defect. Before editing, collect the numbers the new text will state:

```bash
cd /home/joseph/Projects/icedtea
cargo test -p icedtea-ui 2>&1 | grep -E '^test result|running [0-9]+ tests'
grep -c '|' ui/tests/fixtures/gtk4.22-css-properties.txt
```

Every number written below must come from that output or from an assertion in
the tree. Do not write a number the tests do not pin.

**Step 2 — edit.** Replace the section headed `## What M1 covers` with:

```markdown
## What this crate covers

M1 proved one themed `button` end to end. M2 widened the CSS layer to the whole
GTK 4.22 property table: every property in GTK's CSS reference parses, cascades,
inherits, computes, paints and animates on a widget-independent styled-node
tree.

| Layer | Crate |
|---|---|
| Wayland + layer shell | `wayland-client`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
| 2D paint | `skia-rs-safe` (pure Rust) |
| Text shaping | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` (**not** `cosmic-text`) |
| Font discovery | `fontconfig` — real `fc-match` parity |
| Layout | `taffy` |
| CSS parse / match | Servo's `cssparser` + `selectors` |
| Widget | bespoke — a `css::node::Node` tree wearing GTK's node identity |

The property registry (`css/registry.rs`) is the spine: one static table of
113 rows — 95 longhands, 18 shorthands — drives parsing, shorthand expansion,
inheritance, computed values and interpolation. Nothing outside that module
names a property string.
```

Replace the three now-false bullets under `## CSS engine behaviour`:

- The **"Shorthands are expanded at cascade time"** bullet keeps its first half;
  delete its last sentence ("`ComputedStyle` still paints a *uniform* border…")
  and write instead: *"Per-side borders and per-corner radii are first-class:
  `ComputedStyle` carries all four sides and all four corners, and the painter
  fills each side as a path between the outer and inner rounded rects, so mixed
  widths and colours join the way GTK's do."*
- Delete the **"Multi-layer `background` keeps only the first image"** bullet
  and write: *"`background` is a full layer list. Every comma-separated layer
  carries its own image, position, size, repeat, origin, clip and blend mode;
  layers paint top-first, as CSS requires, and the colour comes from the last
  layer."*
- Replace the whole **"Runner-ups are kept"** bullet with:

```markdown
- **Invalid at computed-value time follows CSS.** `cascade` still returns
  `CascadedValues` — per longhand, every declaration that applied, sorted
  best-first — but the runner-ups are now diagnostics only. When the winning
  declaration cannot be interpreted (an unknown `@name`, a colour cycle, a
  percentage with no basis, a non-finite `calc()`), the property takes the
  inherited value if it is an inherited property and its initial value
  otherwise. M1 fell back to the runner-up; that divergence is closed.
```

Append after the `background-clip` bullet:

```markdown
- **All 37 `@define-color`s resolve.** The colour table is built unresolved and
  resolved lazily with a depth guard, so a definition may reference a name
  defined later and a cycle terminates instead of hanging. GTK's legacy
  `alpha()`/`shade()`/`mix()`/`lighter()`/`darker()` and CSS Color 5 relative
  syntax (`hsl(from … calc(s * 1.8) …)`) are all understood; M1 resolved 29 of
  37 and recorded the rest as absent.
- **`@media` blocks are parsed once and evaluated per environment.**
  `prefers-color-scheme` and `prefers-contrast` are modelled; an unknown feature
  parses and never matches; `prefers-reduced-motion` parses and always evaluates
  false. One parse can therefore be compiled under several environments, which
  is how the coverage gate compiles light, dark and high-contrast.
```

Insert a new section before `## Layout, paint and Wayland behaviour`:

```markdown
## Fonts and text

- **Font discovery is real fontconfig.** `text::FontDatabase` builds one
  `FcPattern` per query carrying the whole `font-family` list in priority order
  plus `FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/`FC_PIXEL_SIZE`, and calls
  `FcFontMatch` — so generic families, user aliases and everything in
  `~/.config/fontconfig` resolve exactly as `fc-match` resolves them. The tests
  assert that `sans-serif` resolves to *something*, never to a fixed family, and
  compare against the `fc-match` binary where it is installed.
- **The CSS and fontconfig scales are not the same scale.** `font-weight` 700 is
  `FC_WEIGHT` 200 and 400 is 80; `font-stretch: 87.5%` is `FC_WIDTH` 87. The
  conversions are table lookups with piecewise-linear interpolation
  (`css_weight_to_fc`, `css_stretch_to_fc`, `css_style_to_fc_slant`), not casts.
- **`FontDatabase::probe_only()` is the fallback**, and the whole of what M1
  had: the fixed `FONT_CANDIDATES` path list. It is used when `FcInit` fails
  (a stripped container, a machine with no fontconfig), so the UI still has a
  face.
- **Three caches, dropped together by `clear_caches`:** query → `FontFace`,
  `(path, index)` → `Typeface`, and `ShapeKey` → `ShapedText`. A shaping key
  covers the text, face, size, letter-spacing, feature and variation settings
  and `text-transform` — every input that can change the run.
- **`text-transform` runs before shaping**, as it does in GTK, including
  `full-width` and `full-size-kana`. `letter-spacing` is added after every
  glyph (CSS 2.1's rule, so the trailing step is part of the measured width) via
  `TextBlobBuilder::add_positioned_run`.
- **Known limits of this shaper stack**, warned once at runtime rather than
  silently dropped: `font-feature-settings` and `font-variation-settings` cannot
  reach `rustybuzz` through `skia-rs-text` 0.4.0 (its `Shaper` passes an empty
  feature slice and exposes no variation axes), and a face index inside a font
  collection cannot be selected (`Typeface::from_data` takes no index). Both
  values still parse, compute and enter the shaping key, so the day the shaper
  grows the API the cache is already keyed correctly.
```

Replace the `## Tests` bullet list's tail with the gate description, keeping the
two existing bullets about `themed_button_offscreen.rs` and
`layer_shell_screencopy.rs`:

```markdown
- `tests/adwaita_coverage.rs` is the **M2 gate**: GTK 4.22's Adwaita light, dark
  and high-contrast, walked declaration by declaration — including the ones
  inside `@keyframes` and `@media` — through the property registry, asserting
  **0 unknown properties, 0 unparseable declarations and 37/37 `@define-color`s
  resolved** on each sheet. It names what it cannot read rather than counting,
  and carries negative controls so that a green run means something.
- `tests/gtk4_property_reference.rs` pins the registry against a vendored
  fixture of the GTK 4.22 property table: 113 rows of name, longhand/shorthand
  kind and inherited flag, asserted both ways — nothing missing, nothing
  invented — in registry order.
```

Replace the whole `## Deliberately not covered by M1` section with:

```markdown
## Deliberately not covered by M2

Icon *drawing* — every `-gtk-icon-*` value parses, computes and is stored, but
nothing rasterizes an icon yet (M4). Widgets beyond the button behaviour that
carries the M1 gate, the focus/event model, grid and centre layouts, and text
editing (M3). Accessibility, input methods, drag and drop (M6). `url()` images
beyond PNG, and SVG only where `skia-rs-svg` decodes it — anything else is
recorded unresolved and paints nothing rather than erroring. `@media` features
beyond `prefers-color-scheme` and `prefers-contrast`. Fractional scale and
surface resize. `font-feature-settings`/`font-variation-settings` reaching the
shaper, and font-collection face indices (see **Fonts and text** above). See the
spec's M3–M6.
```

Finally, extend `## Vendored files`:

```markdown
`themes/adwaita-light.css`, `themes/adwaita-dark.css` and
`themes/adwaita-hc.css` are GTK 4's default light, dark and high-contrast
themes, redistributed under the LGPL-2.1-or-later. See
[`themes/README.md`](themes/README.md). `tests/fixtures/gtk4.22-css-properties.txt`
is a transcription of GTK 4.22's CSS property reference, with the doc URL in its
header.
```

**Step 3 — verify the README against the tree.**

```bash
cd /home/joseph/Projects/icedtea
grep -n 'uniform border\|runner-up\|first image\|M1 covers\|not covered by M1' ui/README.md
```

Expected: no output — every M1-era claim is gone.

```bash
cargo test -p icedtea-ui 2>&1 | tail -20
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** None — this task changes no code. Its guard is the `grep`
above plus review: any number in the README must be pinned by a test in the
tree (113 rows, 95 longhands, 37 colours, 0/0).

**Step 4 — commit.**

```bash
cd /home/joseph/Projects/icedtea
git add ui/README.md
git commit -m "docs(ui): README for the M2 engine

The three M1-era claims are gone: the border is per-side, background is a layer
list, and an uninterpretable winner now yields the inherited or initial value
instead of the runner-up. New sections cover the registry spine, fontconfig
matching with the CSS<->fc scale conversions, the shaping caches, the two
warned-once shaper limits, and the coverage gate.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 12 — Spec status, and the whole-tree gate run

The last task: mark the spec implemented, and prove the M1 gate is untouched.

**Files**

- Modify:
  `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md` —
  the `**Status:**` line and a new implementation-status block.

**Interfaces** — none (documentation).

**Step 1 — prove the M1 gate is byte-identical.** Find the branch point and diff
every file the contract's §10.2 pins:

```bash
cd /home/joseph/Projects/icedtea
BASE=$(git merge-base HEAD develop)
git diff --stat "$BASE" -- ui/tests/themed_button_offscreen.rs \
                           ui/tests/layer_shell_screencopy.rs \
                           ui/src/shm.rs ui/src/wayland.rs ui/src/lib.rs
```

`ui/tests/themed_button_offscreen.rs` may differ only by P4's **mechanical**
rewrite (`paint_button` → `paint_node`, `CssNode` → `Node`, the reshaped
`Allocation`); `ui/src/lib.rs`, `ui/src/shm.rs`, `ui/src/wayland.rs` and
`ui/tests/layer_shell_screencopy.rs` must show **no diff at all**. Then prove no
*number* moved in the pixel gate:

```bash
git diff "$BASE" -- ui/tests/themed_button_offscreen.rs \
  | grep -E '^[+-]' | grep -oE '0x[0-9A-Fa-f]{8}|[0-9]+\.[0-9]+|\b[0-9]+\b' \
  | sort | uniq -c | awk '$1 % 2 == 1'
```

Expected: no output — every literal that leaves on a `-` line comes back on a
`+` line. Any odd count is a changed constant and fails review (contract
§10.3: "a diff that changes a *number* here fails review").

**Step 2 — run every gate.**

```bash
cd /home/joseph/Projects/icedtea
cargo test -p icedtea-ui 2>&1 | tail -30
cargo test --workspace 2>&1 | tail -30
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```

All four must be clean. The `--workspace` run is the contract's "+ workspace"
obligation: `icedtea-ui` is a workspace member and the harness-backed
compositor tests must still pass.

**Step 3 — edit the spec.** In
`docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`,
replace:

```markdown
**Status:** approved in brainstorming (2026-08-26); awaiting owner spec review
```

with:

```markdown
**Status:** implemented on `rebuild/pure-rust-gtk-m2` (parts 1–6 of
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`); awaiting owner review
before merge
```

and append, immediately before the `## Open items for M3` section:

```markdown
## Implementation status

Implemented across six part-plans against the frozen interface contract
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`:

| Part | Scope | Plan |
|---|---|---|
| P1 | registry, values, `@keyframes`/`@media` | `2026-08-26-m2-part1-registry-values-parse.md` |
| P2 | node tree, selectors | `2026-08-26-m2-part2-node-selectors.md` |
| P3 | cascade, computed style, M1 migration | `2026-08-26-m2-part3-cascade-computed.md` |
| P4 | layout, paint | `2026-08-26-m2-part4-layout-paint.md` |
| P5 | transitions, animations | `2026-08-26-m2-part5-animation.md` |
| P6 | fonts, coverage gate, docs | `2026-08-26-m2-part6-fonts-gate-docs.md` |

The gate (§7) is `ui/tests/adwaita_coverage.rs`: GTK 4.22 Adwaita light, dark
and high-contrast, every declaration through the registry, **0 unknown
properties / 0 unparseable declarations / 37 of 37 `@define-color`s resolved**,
plus `ui/tests/gtk4_property_reference.rs` pinning the registry's 113 rows
against the vendored GTK 4.22 property table. M1's pixel gate
`ui/tests/themed_button_offscreen.rs` keeps every one of its numbers.

Carried into later milestones as written here: `-gtk-icon-*` values parse,
compute and store but draw nothing (M4). Two limits of the shaper stack are
warned about once at runtime rather than silently ignored:
`font-feature-settings`/`font-variation-settings` cannot reach `rustybuzz`
through `skia-rs-text` 0.4.0, and a face index inside a font collection cannot
be selected.
```

If any part-plan filename above differs from what actually landed, correct the
row to the real filename rather than leaving a link that does not resolve:

```bash
ls docs/superpowers/plans/2026-08-26-m2-part*.md
```

**Step 4 — final verification.**

```bash
cd /home/joseph/Projects/icedtea
git status --porcelain          # expect only the spec edit staged/unstaged
cargo test -p icedtea-ui 2>&1 | grep -E '^test result'
```

**Mutation check.** None — documentation. Its guard is Step 1's diff check and
Step 2's four green gates, both of which must be pasted into the completion
report.

**Step 5 — commit, then stop.**

```bash
cd /home/joseph/Projects/icedtea
git add docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md
git commit -m "docs(spec): M2 CSS engine implemented, awaiting owner review

Records the six part-plans, the gate the milestone is measured by (0 unknown /
0 unparseable / 37 colours on three Adwaita sheets), and the two shaper limits
and the icon-drawing deferral M2 knowingly carries into M4.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

**Do not push, do not open a PR, do not merge.** Report the branch as ready —
tests green, gates clean — and hand the merge decision to the owner, who may
want to run `/simplify`, `/code-review` or `/optimize` first.

---

## Self-Review

### Spec coverage

Every spec obligation the contract assigns to P6 (§11: `text.rs`,
`tests/adwaita_coverage.rs`, `tests/gtk4_property_reference.rs`, the vendored
sheets and fixture, `ui/README.md`), mapped to the task that discharges it.

| Spec bullet (§6 Fonts & text, §7 Testing) | Task |
|---|---|
| §6 `FontDatabase` over the `fontconfig` 0.11 crate | 2, 3 |
| §6 `match(families, weight, style, stretch, size) -> FontFace { path, index, family }` | 2 (type + probe path), 3 (fontconfig path) |
| §6 via `FcPattern`/`FcFontMatch`, honouring user aliases/rules | 3 |
| §6 per-pattern typeface cache | 3 (`matches`, `match_cache_key`) |
| §6 per-`(path, index)` typeface cache | 2 (`typefaces`, keyed by `FontFace`) |
| §6 `probe_only()` — M1's file list — as the no-fontconfig fallback | 2 (`probe_face`, `FONT_CANDIDATES`), 3 (`new()` degrades to it) |
| §6 tests assert `sans-serif` resolves to *something*, not a fixed family | 3 (`a_generic_family_resolves_to_something`, plus the `fc-match` parity test) |
| §6 computed `font-family` list | 6 (`TextStyle::families`, `family_names`) |
| §6 `font-size` (px/pt/em/rem/%/keywords) | 6 (`TextStyle::size_px` via `ComputedStyle::font_size_px`; unit resolution is P3's) |
| §6 `font-weight` (100–900, bold, lighter/bolder relative to parent) | 6 reads the computed absolute; 1 converts it to `FC_WEIGHT` |
| §6 `font-style` | 1 (`css_style_to_fc_slant`), 6 |
| §6 `font-stretch`/`font-width` | 1 (`css_stretch_to_fc`), 6 (`font-width` first, `font-stretch` behind it) |
| §6 `font-variant*` | 6 (carried on `TextStyle` via the computed flags; drawing is P4's) |
| §6 `font-feature-settings`/`font-variation-settings` "to rustybuzz … else stored" | 5 (in the key, warned once at the shaper), 6 (read from the computed style) |
| §6 `line-height` | 6 (`TextStyle::line_height_px`) |
| §6 `letter-spacing` | 5 (applied per glyph and to the measured width), 6 (read) |
| §6 `text-transform` (incl. `full-width`) | 4 (`apply_text_transform`, incl. `full-size-kana`), 5 (applied before shaping) |
| §6 `-gtk-dpi` for pt→px | consumed, not owned: P3 resolves it into the computed `font-size` this part reads |
| §6 shaping cache keyed by `(text, FontFace, size, letter-spacing, features)` | 5 (`ShapeKey` + `shape_cache_key`, also keyed by `text-transform` and variations) |
| §6 cache invalidated by generation | 2 (`clear_caches`) — the caller drops the caches; the node generation lives in P2 |
| §6 `caret-color` stored for M3 | P1 registry row + P3 computed; nothing for P6 to do (no caret is drawn in M2) |
| §6 text paint: `text-shadow`, `text-decoration-*` | P4 (`paint/text.rs`) — out of P6's scope by the contract's §11 split |
| §7 coverage instrument = THE M2 GATE, `ui/tests/adwaita_coverage.rs` | 9, 10 |
| §7 compiles vendored `Default-light.css` + `Default-dark.css` + `Default-hc.css` | 7 (vendoring), 10 (all three walked) |
| §7 walks every rule × declaration through the registry | 9 (`Coverage::walk`, incl. `@keyframes` and `@media`) |
| §7 asserts 0 unparseable declarations | 9 (light), 10 (all three) |
| §7 asserts 0 unknown properties | 9 (light), 10 (all three) |
| §7 asserts 37/37 `@define-color`s resolved (relative colours included) | 10 |
| §7 reference conformance: `every_gtk4_property_is_registered` | 8 |
| §7 …with correct shorthand/longhand kind and inherited flag | 8 (all three columns, both directions, in registry order) |
| §7 never-panic fuzz per value parser | 1, 4, 5 (this part's total functions; the value parsers themselves are P1's) |
| §7 mutation discipline: every load-bearing test records a mutation check | every task (each ends with a **Mutation check** paragraph); 11 and 12 state why they have none |
| §7 gates: `cargo test -p icedtea-ui` (+ workspace), clippy `-D warnings`, `cargo fmt --all --check`, coverage at 0/0/37 | every task's run step; 12 runs all four plus `--workspace` |
| Risk "fontconfig in CI" mitigated by `probe_only()` + "resolves to something" | 2, 3 (the parity test skips loudly when `fc-match`/fontconfig is absent) |
| Docs: README and spec status reflect the shipped engine | 11, 12 |

### Placeholder scan

- No `TODO`, `FIXME`, `TBD`, `unimplemented!()`, `todo!()` or "…" stands in for
  code anywhere in this plan. Verify:
  ```bash
  grep -nE 'TODO|FIXME|TBD|unimplemented!|todo!|\.\.\.$' \
    docs/superpowers/plans/2026-08-26-m2-part6-fonts-gate-docs.md
  ```
  The only expected hits are inside prose (`section 7`-style references) and the
  Rust `..` struct-update / range syntax, never a stand-in for a body.
- No task says "similar to Task N": every code block is written out in full,
  including the repeated test helpers (`sans()`, `query()`, `key()`), which are
  defined once in Task 2/Task 5 and referenced by name thereafter because they
  live in the same `mod tests`.
- No undefined types. Every type named in an implementation block is either
  (a) defined in this plan, (b) copied verbatim from the contract with its
  owning part named, or (c) from a pinned external crate at the signature the
  research notes recorded.
- Every command is runnable as written and every expected failure message is a
  real compiler/test message shape (`error[E0432]: unresolved import`,
  `error[E0599]: no function or associated item named`, `error: couldn't read`).
- Two tasks (9, 10) deliberately have no implementation code of their own: the
  instrument *is* the deliverable, and their Step 3 is the named,
  bounded work of fixing whatever P1 gap the gate exposes, in P1's files, one
  commit per family. That is stated explicitly, not left implicit.

### Type consistency against the contract

| Contract §9 item | This plan | Status |
|---|---|---|
| `FontFace { path: PathBuf, index: i32, family: String }`, `Clone+Debug+PartialEq+Eq+Hash` | Task 2, identical | ✅ |
| `FontQuery<'a> { families: &'a [FontFamily], weight: f32, style: FontStyle, stretch: f32, size_px: f32 }`, `Copy+Clone+Debug+PartialEq` | Task 2, identical | ✅ |
| `FontDatabase::new() -> FontDatabase` | Task 3 | ✅ |
| `FontDatabase::probe_only() -> FontDatabase` | Task 2 | ✅ |
| `FontDatabase::has_fontconfig(&self) -> bool` | Task 2 | ✅ |
| `FontDatabase::match_face(&mut self, &FontQuery<'_>) -> Option<FontFace>` | Task 2 (probe branch), Task 3 (fontconfig branch); signature unchanged | ✅ |
| `FontDatabase::typeface(&mut self, &FontFace) -> Option<Arc<Typeface>>` | Task 2 | ✅ |
| `FontDatabase::font(&mut self, &FontFace, f32) -> Option<Font>` | Task 2 | ✅ |
| `FontDatabase::shape(&mut self, &ShapeKey<'_>) -> Rc<ShapedText>` | Task 5 | ✅ |
| `FontDatabase::clear_caches(&mut self)` | Task 2 | ✅ |
| "single-threaded, `!Send`" | Task 2 — the `Rc<ShapedText>` cache makes it `!Send`/`!Sync` by construction; documented in the module header | ✅ |
| `css_weight_to_fc(f32) -> i32`, `css_stretch_to_fc(f32) -> i32`, `css_style_to_fc_slant(FontStyle) -> i32`, "table lookups with piecewise-linear interpolation, not casts" | Task 1, identical, with the tables written out | ✅ |
| `ShapeKey<'a> { text, face, size_px, letter_spacing_px, features, variations, transform }`, `Clone+Debug+PartialEq` | Task 5, identical field names, types and order | ✅ |
| `TextMetrics { width, ascent, descent, line_height }` "unchanged from M1" | untouched; Task 5 keeps M1's ascent-sign and fallback rules | ✅ |
| `ShapedText { blob: Option<TextBlob>, metrics: TextMetrics, face: FontFace, size_px: f32 }` | Task 2 (fields), Task 5 (producer) | ✅ |
| "features/variations parse, compute, enter `ShapeKey`, dropped at the shaper with a one-time `tracing::warn!`" | Task 5 (`warned_unsupported_shaping`), Task 6 (they reach the key) | ✅ |
| "`text-transform` applied before shaping, inside `FontDatabase::shape`" | Task 4 + Task 5 | ✅ |
| "`letter-spacing` applied via `TextBlobBuilder::add_positioned_run`" | Task 5 | ✅ |
| §10.3 `src/text.rs` — 5 M1 tests rewritten, "same properties asserted; 'resolves to *something*' replaces any fixed family" | Task 5 keeps all five behaviours (size/length scaling, 1:1 Latin shaping with advancing positions, blob+metrics together, empty text, last-glyph-inside-width) and Task 2/3 supply the family rule | ✅ |
| §10.2 gate files untouched | no task edits any of them; Task 12 proves it with a diff | ✅ |
| §11 P6 "Owns: `text.rs`, `tests/adwaita_coverage.rs`, `tests/gtk4_property_reference.rs`, the vendored `Default-dark.css`/`Default-hc.css`/property-name fixture, `ui/README.md`" | exactly the File Structure table, plus the spec status line (documentation, no owner conflict) | ✅ |
| §11 P6 "Consumes §1, §2, §5, §8" | Tasks 6, 8, 9, 10 consume `registry`, `value`, `computed`, `parse`; nothing in P1–P5 is modified except the sanctioned registry/value fixes the gate forces (Tasks 9–10 Step 3), each its own commit | ✅ |

Deviations are the four listed at the top of this plan; nothing else in §9 or
§11 is changed, renamed or re-typed.
