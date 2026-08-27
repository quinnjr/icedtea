# Pure-Rust GTK-themed UI — M2: GTK-CSS Engine Breadth — Design

**Date:** 2026-08-26
**Status:** implemented on `rebuild/pure-rust-gtk-m2` (parts 1–6 of
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`); awaiting owner review
before merge
**Branch:** `rebuild/pure-rust-gtk-m2` (off `develop` @ 216a8e8)
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` (§Decomposition, M2; M1 hand-off list)
**Crate:** `ui/` (`icedtea-ui`), continuing M1 (`develop` @ a4dbbe6 + fix waves)

## Goal

Grow M1's single-button CSS slice into the **complete GTK 4.22 CSS engine**:
every property in GTK's CSS reference is parsed, cascaded, inherited,
computed, **painted, and animated** on a widget-independent styled-node tree,
so that M3's widgets are behaviour layered on nodes the engine already
styles correctly — and so that any installed GTK4 theme file "just works".

## Decisions (settled in brainstorming, 2026-08-26)

1. **Coverage bar = the full GTK 4 CSS reference** (docs.gtk.org/gtk4/
   css-properties.html + css-overview.html, GTK 4.22 as installed), not
   merely "Adwaita-complete". Measured by the coverage instrument (§7).
2. **Compute + paint + animate.** Every non-icon property is rendered by the
   paint layer and every animatable one is interpolated by an animation
   engine with a deterministic test clock. Transitions/animations move here
   from M6. Icon *drawing* (`-gtk-icon-*`) stays M4: values parse and store.
3. **Test vehicle = a generic styled-node tree** (`css::node::Node`): the
   engine styles it, `taffy` lays it out, a generic painter renders it, and
   animations tick on it. M1's `Button` becomes a thin behaviour over a
   node; its gate test stays byte-identical.
4. **Fonts via fontconfig** (`fontconfig` 0.11 crate → system libfontconfig):
   exact `fc-match` parity for `font-family` lists, weights, styles,
   stretch, aliases. The parent spec's "pure Rust" means leaving the GNOME
   toolkit, not purging native libraries (wlroots is already C).
5. **Architecture = a declarative property registry** (Servo/GTK-style
   table driving cascade, shorthands, inheritance, computed values, and
   interpolation generically) — not per-property hand code, not Stylo.
6. **CSS-correct invalid-at-computed-value-time** replaces M1's
   "fall back to the runner-up" rule (M1 ruling closed): an uninterpretable
   winner yields the inherited value (inherited properties) or the initial
   value. Runner-ups stay in `CascadedValues` for diagnostics only.

## Section 1 — Architecture & modules

```
ui/src/css/
  registry.rs     static PROPERTIES: &[PropertyDef]; Prop enum (index = row)
  value/          typed values + cssparser token parsers, one file per family:
                  length.rs color.rs image.rs shadow.rs border.rs outline.rs
                  font.rs text.rs transform.rs filter.rs timing.rs keyframes.rs
                  calc.rs (expression tree), keyword.rs (inherit/initial/unset)
  parse.rs        + @keyframes, @media(prefers-color-scheme|prefers-contrast) via MediaEnv
  node.rs         Node tree (new; replaces CssNode)
  select.rs       complete Element impl over Node; rule buckets; bloom filter
  cascade.rs      registry-driven; shorthand expand via PropertyDef::expand
  computed.rs     ComputedStyle = registry-indexed typed table; inheritance; unit resolution
ui/src/anim/      clock.rs transition.rs keyframes.rs (AnimationState -> Overrides)
ui/src/layout.rs  node-tree layout (taffy block/flex, margins, border-spacing, content-box mins)
ui/src/paint/     mod.rs (paint_node order) background.rs border.rs shadow.rs outline.rs
                  text.rs effects.rs (opacity/transform/filter save-layer)
ui/src/text.rs    FontDatabase (fontconfig) + skia-rs-text shaping cache
ui/src/widget/    button.rs = Node behaviour (states -> node); kept for the M1 gate
```

`PropertyDef { name: &'static str, kind: Longhand { parse, initial, inherited, animatable: Option<Interpolate> } | Shorthand { expand } }`.
Nothing outside `registry.rs` names a property string; consumers use `Prop::*`.

## Section 2 — Node tree & selectors

**`Node`** = `Rc<NodeInner>`; `RefCell` fields: `name`, `id: Option<String>`,
`classes` (interned), `states: PseudoStates` (hover, active, focus,
focus-visible, focus-within (derived from descendants), checked,
indeterminate, disabled, backdrop, selected, link/visited (never match),
drop-active), `direction: Ltr|Rtl` (inherited from the parent unless set),
`parent: Weak`, `children: Vec<Node>` in order. Mutations
(`append/insert/remove_child`, `set_states`, `add/remove_class`,
`set_direction`, `set_id`) bump a per-tree `generation`.

**`Element` impl** is complete: `parent_element`, `prev/next_sibling_element`,
`first_element_child`, `is_empty` (no children), `is_root` (no parent),
`has_id`, `has_class`, the full GTK pseudo-class list (`:link :visited
:active :hover :focus :focus-within :focus-visible :disabled :checked
:indeterminate :backdrop :selected :not() :dir(ltr|rtl) :drop(active) :root
:nth-child(an+b) :nth-last-child(an+b) :first-child :last-child
:only-child`), combinators descendant/child `>`/adjacent `+`/general `~`,
universal `*`. `attr_matches`, `has_custom_state`, `imported_part`,
`is_html_slot_element` stay `false` (GTK has no attributes/parts;
documented on the impl).

**Matching cost:** rules bucketed by the rightmost compound's name/id/class;
one caller-owned `MatchingContext` with the `selectors` bloom filter over
ancestors; restyle re-matches only nodes whose generation changed, plus
descendants when an inherited value changed.

## Section 3 — Registry, values, cascade, inheritance

**Rows (GTK 4.22 reference):** colors (`color`, `opacity`) · fonts/text
(`font-family font-size font-style font-variant font-weight font-width
font-stretch font-kerning font-variant-ligatures font-variant-position
font-variant-caps font-variant-numeric font-variant-alternates
font-variant-east-asian font-feature-settings font-variation-settings
-gtk-dpi font letter-spacing text-transform line-height caret-color
-gtk-secondary-caret-color`) · text decoration (`text-decoration-line
-color -style text-shadow text-decoration`) · icons (`-gtk-icon-source
-gtk-icon-size -gtk-icon-style -gtk-icon-transform -gtk-icon-palette
-gtk-icon-shadow -gtk-icon-filter -gtk-icon-weight`; stored, drawn in M4) ·
`filter transform transform-origin` · box model (`min-width min-height
margin-* margin padding-* padding`) · borders (`border-*-width/-style/-color`
×4, `border-width/-style/-color`, `border-*-*-radius` ×4, `border-radius`,
`border-top/right/bottom/left`, `border`, `border-image-source/-repeat/
-slice/-width`, `border-image`) · outlines (`outline-style -width -color
-offset outline`) · backgrounds (`background-color -image -position -size
-repeat -origin -clip -blend-mode background`) + `box-shadow` · transitions
(`transition-property -duration -timing-function -delay transition`) ·
animations (`animation-name -duration -timing-function -iteration-count
-direction -play-state -delay -fill-mode animation`) · `border-spacing`.
Inherited flags follow CSS/GTK (text/font/colour/caret/`-gtk-dpi`/
`-gtk-icon-*` inherit; box/border/background/outline/effects do not).

**Value types:** `Length` (px pt em rem ex %, `calc()` expression tree,
percentages only where GTK allows), `Color` (hex, rgb/rgba/hsl/hwb comma
and space syntax, named, `currentColor`, `transparent`, `@name`,
`color-mix()`, relative `rgb(from …)`/`hsl(from …)`, legacy
`alpha()/shade()/mix()/lighter()/darker()`), `Image` (`none`, `url()` —
PNG, and SVG when `skia-rs-svg` decodes it, else recorded-unresolved —
`image(color)`, `linear-/radial-/conic-gradient` with any stops/angles and
`repeating-` forms, `cross-fade()`, `-gtk-recolor()/-gtk-scaled()/
-gtk-icontheme()` → `IconRef` for M4), `Shadow` lists (inset/outset,
offsets, blur, spread, colour), `BorderImage`, font enums,
`Transform` lists, `Filter` lists, `TimingFunction` (`linear`, `ease*`,
`steps()`, `cubic-bezier()`), `Time`, `Keyframes`, wide keywords
`inherit/initial/unset` on every property. Every parser is
cssparser-token based, ASCII-case-insensitive, whitespace-insensitive,
never panics (fuzz-ish tests over odd inputs).

**Cascade:** M1's mechanism (important → specificity per matched selector →
source order; shorthands expanded at cascade time with the shorthand's
key; `CascadedValues` keeps runner-ups) iterating the registry. Wide
keywords and invalid-at-computed-value-time resolve in `computed.rs`
(Decision 6). `@define-color` resolves lazily and cycle-guarded so
relative-colour definitions may reference names defined later.

**Computed style:** `ComputedStyle { values: Box<[Value; N]> }` indexed by
`Prop`; typed `get::<T>(Prop::X)`; inheritance copies the parent's
computed value; `em`/`rem`/`%`/`-gtk-dpi` resolved here; per-side borders
and per-corner radii are first-class.

## Section 4 — Layout & paint

**Layout:** one `taffy` node per `Node`; GTK box model — `margin` outside,
`border` + `padding` inside, content-box `min-width/min-height` (M1 rule),
`border-spacing` as gap on containers. M2 container behaviours: **box**
(row/column, direction set by the widget layer; default column) and
**leaf** (intrinsic size from text or none). Grid/center layouts are M3.
`transform`/`opacity`/`filter` never affect layout.

**Painter:** `paint_node(canvas, node, style, allocation, overrides)` in
order: `box-shadow` outset → background layers (per layer: clip box, origin
box, size, position, repeat; blend-mode across layers; image = colour /
gradient / cross-fade / url) → `border-image` if set, else per-side borders
with per-corner radii (each side a filled path between the outer and inner
rounded rects, so mixed widths/colours join like GTK) → `box-shadow` inset
→ `outline` (offset; solid/dashed/dotted/double; wavy drawn solid; own
radius) → text (colour, `text-shadow`, `text-decoration-*`,
`letter-spacing`, `text-transform`, `line-height`) → children. `opacity <
1`, `transform`, or `filter` paint through a `save_layer` with that
alpha/matrix/filter (skia-rs: blur, brightness, contrast, grayscale,
hue-rotate, invert, opacity, saturate, sepia, drop-shadow). `-gtk-icon-*`
draws nothing (M4).

**Exactness rule (from M1):** asserted pixels must be derivable from the
CSS — gradients keep the sampler model (now angle, multi-stop, radial,
conic), shadows are asserted at their flat cores, anti-aliased edges are
never asserted exactly.

## Section 5 — Transitions & animations

**Clock:** `anim::Clock` trait (`now() -> Duration`); production = monotonic
time driven by `wl_surface.frame` callbacks while anything animates (no
busy loop); tests = `ManualClock::advance(ms)`.

**Transitions:** on restyle, diff old vs new computed values for the
properties in `transition-property` (`all` = every animatable row); each
changed animatable property starts `Transition { from, to, duration, delay,
timing }` with reversal continuity (CSS Transitions §3.1); `transition-*`
values come from the new style; non-animatable changes switch at 50%.

**Animations:** `animation-name` → `@keyframes` from the compiled sheet;
per node an `Animation` with duration, delay, iteration-count (incl.
`infinite`), direction (normal/reverse/alternate/alternate-reverse),
fill-mode, play-state, per-span timing functions. Precedence: animation
overrides transition overrides base cascade.

**Interpolation** is the registry row's `animatable` fn: lengths, numbers,
colours (premultiplied sRGB, as GTK), shadow lists (component-wise, padded
with transparent shadows), gradients (stop-wise when structures match, else
discrete), transforms (component-wise for matching lists, else matrix
decomposition), opacity, per-side/per-corner values.

**Output:** `AnimationState::sample(now) -> Overrides` layered onto the
computed style before layout/paint; `is_active()` requests the next frame.
Timing functions: `linear`, `ease*`, `cubic-bezier()` (Newton–Raphson on
x), `steps(n, start|end|jump-*)`.

## Section 6 — Fonts & text

`text::FontDatabase` over the `fontconfig` 0.11 crate: `match(families,
weight, style, stretch, size) -> FontFace { path, index, family }` via
`FcPattern`/`FcFontMatch`, honouring user aliases/rules; per-pattern and
per-`(path,index)` typeface caches. `FontDatabase::probe_only()` (M1's file
list) is the fallback where fontconfig has no configuration (CI); the
crate's tests assert `sans-serif` resolves to *something*, not a fixed
family.

Computed font: `font-family` list, `font-size` (px/pt/em/rem/%/keywords),
`font-weight` (100–900, bold, lighter/bolder relative to parent),
`font-style`, `font-stretch`/`font-width`, `font-variant*`,
`font-feature-settings`/`font-variation-settings` (to rustybuzz features /
variation axes where skia-rs-text exposes them, else stored),
`line-height`, `letter-spacing`, `text-transform` (incl. `full-width`),
`-gtk-dpi` for pt→px. Shaping cache keyed by `(text, FontFace, size,
letter-spacing, features)`, invalidated by generation. Text paint handles
`text-shadow` (blur mask), `text-decoration-line/-color/-style`
(underline/overline/line-through; solid/double/dotted/dashed/wavy as
paths), `caret-color` stored for M3.

## Section 7 — Testing strategy & the M2 gate

- **Coverage instrument (THE M2 GATE):** `ui/tests/adwaita_coverage.rs`
  compiles the vendored Adwaita 4.22 `Default-light.css`, plus
  `Default-dark.css` and `Default-hc.css` (vendored from the same
  gresource), walks every rule × declaration through the registry and
  asserts **0 unparseable declarations, 0 unknown properties, 37/37
  `@define-color`s resolved** (relative colours included).
- **Reference conformance:** `every_gtk4_property_is_registered` — a
  vendored text fixture of the GTK 4.22 property names (with the doc URL)
  must be fully present in the registry with correct shorthand/longhand
  kind and inherited flag.
- **Engine unit tests:** per value type (parse incl. case/whitespace/calc/
  wide keywords; never-panic fuzz), cascade, inheritance,
  invalid-at-computed-value-time, selectors on trees, interpolation per
  animatable type, timing functions at known points.
- **Pixel tests:** per paint family on abstract trees, exact-equality at
  derivable points; the M1 button gate unchanged.
- **Animation tests:** manual-clock exactness (0/100/200 ms linear midpoint,
  `ease` at t=0.5, reversal continuity, `alternate`, `fill-mode: forwards`,
  Adwaita's button `transition` sampled at 100 ms).
- **Wayland proof:** existing layer-shell screencopy tests + one new
  `transition: background-color 200ms` start/end capture (intermediate
  frames not asserted on screen).
- **Mutation discipline:** every load-bearing test records a mutation check.
- **Gates:** `cargo test -p icedtea-ui` (+ workspace), clippy `-D
  warnings`, `cargo fmt --all --check`, coverage instrument at 0/0/37.

## Out of scope (explicit)

Icon rendering / `-gtk-icon-*` drawing (M4); widgets, focus/event model,
grid/center layouts, entries/text editing (M3); `accesskit`, input
methods, DnD (M6); `url()` images beyond PNG/SVG-if-available; `@media`
queries beyond `prefers-color-scheme`/`prefers-contrast`.

## Risks

- **Registry refactor of M1's computed style** — mitigated by keeping every
  M1 test as the regression bar during the swap.
- **fontconfig in CI** — mitigated by `probe_only()` fallback and
  "resolves to something" assertions.
- **Paint fidelity for mixed per-side borders / shadows** — mitigated by
  the exactness rule: assert only derivable pixels; compare visually
  against GTK for the rest (manual).
- **Animation timing on screen** — asserted only at start/end on screen;
  exact values via the manual clock.
- **Size** — this is the largest milestone; the plan decomposes by family
  (registry+values → node/selectors → cascade/computed → layout/paint per
  family → animation → fonts → coverage gate).

## Implementation status

Implemented across six part-plans against the frozen interface contract
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md`:

| Part | Scope | Plan |
|---|---|---|
| P1 | registry, values, `@keyframes`/`@media` | `2026-08-26-m2-part1-registry-values-parse.md` |
| P2 | node tree, selectors | `2026-08-26-m2-part2-node-selectors.md` |
| P3 | cascade, computed style, M1 migration | `2026-08-26-m2-part3-cascade-computed-migration.md` |
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

## Open items for M3

Widget behaviour/event model over `Node`; grid/center layouts; the entry
widget's caret/selection using `caret-color`; xdg-popup support in the
`wlr` crate is a blocker for menus/dropdowns (found 2026-08-26).
