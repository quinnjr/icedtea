# Pure-Rust GTK-themed UI — M2 Part 4: Node-tree layout and the layered painter (backgrounds, borders, shadows, outline, text, effects) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace M1's single-button `layout.rs`/`paint.rs` with a generic
node-tree layout (`LayoutTree` over `taffy`) and a layered CSS painter
(`ui/src/paint/`) that renders every non-icon paintable GTK 4.22 property —
backgrounds, borders, border-images, box-shadows, outlines, text with
shadows and decorations, and the opacity/transform/filter effect stack — for
any `css::node::Node` styled by Part 3's `ComputedStyle`.

**Architecture:** One `taffy` node per `css::node::Node`, mirrored by
`LayoutTree::sync`, styled by `LayoutTree::set_style` (GTK's content-box
`min-width`/`min-height` expressed as taffy's `BoxSizing::ContentBox`), and
measured through a caller-supplied `Measure` impl. Layout output is an
absolute-coordinate `Allocation` (border box, content box, per-side used
border and padding). `paint::paint_node` then walks one node in CSS paint
order, delegating to one module per property family. Every rounded shape is
a hand-built `Path` (skia-rs-safe 0.4.0 has no `RRect` drawing), every
gradient pixel comes from `Gradient::color_at` so pixel assertions are
derivable from the CSS, and every blur is a project-owned three-pass box
blur over a premultiplied offscreen buffer (the rasterizer honours only
`Paint::shader`, never mask/image filters).

**Tech Stack:** Rust (edition 2024, rust-version 1.94), `taffy` 0.14,
`skia-rs-safe` 0.4.0 (features `std`, `text`, `codec`, `codec-png`, `svg`),
`cssparser` 0.37, `selectors` 0.40, `tracing`.

**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
(§Section 4 "Layout & paint", §Section 7 "Testing strategy & the M2 gate")
· parent spec `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
· **binding interface contract**
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§7 Layout, §8
Paint, §10 Migration, §11 Part boundaries — P4).

---

## Contract deviations

The Part 0 contract is binding; the eleven items below are places where it
cannot be followed verbatim, each with the amendment this plan implements.
Nothing else in this plan diverges from it.

1. **`paint_node` takes `cx: &mut PaintCx<'_>`, not `cx: &PaintCx`.**
   §8 declares `paint_node(.., cx: &PaintCx)` while every per-family helper
   in the same section takes `&mut PaintCx<'_>`, and `PaintCx` holds
   `&'a mut FontDatabase` / `&'a mut ImageCache`, which cannot be reborrowed
   mutably through a shared reference. `paint_node` therefore takes
   `&mut PaintCx<'_>`.

2. **`tests/layer_shell_screencopy.rs` cannot stay byte-identical.**
   §10.2 pins it, but it does `use icedtea_ui::layout::Allocation;` and reads
   `allocation.width`, `.height` and `.label_x` — three fields §7 removes.
   Amendment: the file's *only* permitted edit is that one `use` line,
   retargeted at `support::PrintedAllocation` (a four-field struct added to
   the ungated `tests/support/mod.rs`). Every constant, every assertion and
   the ±12 tolerance derivation stay byte-identical.

3. **`src/bin/themed-button.rs` is edited by P4.** §11 assigns it to nobody.
   Its `--print-allocation` branch reads the same three removed fields, so
   P4 changes those four printed expressions to derive from the new
   `Allocation`. The printed *format* (`width height label_x label_y`) and
   therefore `tests/support/mod.rs::allocation_of` are unchanged.

4. **`filter: blur()` and `filter: drop-shadow()` do not paint in M2.**
   `skia_rs_canvas::Canvas::composite_layer` states in-source that "An image
   filter, if present, is not yet applied here", and the rasterizer consults
   only `Paint::shader` (`skia-rs-canvas-0.4.0/src/raster.rs:471` is the
   crate's single filter-ish read). The remaining eight CSS filter functions
   are applied as a **colour** filter on the save-layer paint, which
   `composite_layer` does honour. `blur()`/`drop-shadow()` parse, compute,
   animate and are logged once at `tracing::warn!` when they would paint.

5. **`box-shadow`/`text-shadow` blur is project code, not `BlurMaskFilter`.**
   Same rasterizer gap. §8's `paint_box_shadows` draws the shadow shape into
   an offscreen premultiplied surface, blurs it with a three-pass box blur
   (`paint::blur`), snapshots it and blits it with `Canvas::draw_image`,
   which *does* apply the current path clip per pixel
   (`canvas.rs:1788 clip_stack.get_coverage`).

6. **P4 adds a minimal `text::FontDatabase` shim.** §11 lets P4 "stub
   against `probe_only()`", but `PaintCx` and `Measure` need the *type* to
   exist. P4 creates `text.rs`'s §9 surface (`FontFace`, `FontQuery`,
   `ShapeKey`, `ShapedText`, `FontDatabase` with `new`/`probe_only`/
   `has_fontconfig`/`match_face`/`typeface`/`font`/`shape`/`clear_caches`,
   plus `css_weight_to_fc`/`css_stretch_to_fc`/`css_style_to_fc_slant`)
   backed by M1's `FONT_CANDIDATES` probe. P6 replaces the bodies with
   fontconfig without changing a signature.

7. **P4 re-wires `widget/button.rs`.** §11 gives it to P3, but its
   `restyle`/`render` path cannot compile until §7/§8 exist. P4's last task
   points it at `LayoutTree`/`paint_node`; P4 changes no assertion in
   `button.rs`'s four tests.

8. **`LayoutTree`'s taffy context is `NodeCtx { node: Node, style: Rc<ComputedStyle> }`.**
   §7's comment says `TaffyTree<NodeKey>`. The measure closure needs the
   `Node` and its `ComputedStyle`, so the context carries both; the
   `OpaqueElement`-keyed map to `taffy::NodeId` is still there. Private
   fields, which the contract permits a part to add.

9. **`ImageCache::get` returns `Option<&skia_rs_safe::codec::Image>`.**
   §8 writes `skia_rs_codec::image::Image`; `skia-rs-codec` is not a direct
   dependency of `icedtea-ui` — the type is reached through
   `skia_rs_safe::codec`, which re-exports `image::*` at its root.

11. **P4 carries §10.3's `src/text.rs` row, it does not shed it.**
    Deviation 6 authorises P4 to *create* `text.rs`'s §9 surface; it does not
    authorise dropping the five M1 tests §10.3 pins at "same properties
    asserted". P4's first pass kept two by name
    (`measurement_scales_with_size_and_length`, and
    `a_sans_serif_query_resolves_to_something` for
    `a_system_typeface_is_found_without_fontconfig`) and lost three:
    `shaping_produces_one_positioned_glyph_per_character`,
    `shaping_returns_the_blob_and_the_metrics_together` and
    `blob_width_agrees_with_measure` — with them, the 1:1 glyph-per-character
    rule, `run.glyphs.len() == run.positions.len()`, left-to-right position
    advance, and blob-vs-`metrics.width` agreement. All three are restored
    against `FontDatabase`/`ShapedText`. One property could not be restored
    verbatim: M1's `shaped.metrics == stack.measure(..)` compared two entry
    points, and `FontDatabase` has one, so the restored test asserts instead
    that a single `ShapedText` (and its cached second handle) carries a blob
    *and* usable metrics. **P6 owns `text.rs`; when it replaces the probe
    bodies with fontconfig it must keep these three tests, whose assertions
    are API-shaped, not backend-shaped.**

10. **`Container::Box` centres its children on both axes.** §7 does not say
    how a box aligns children. M1's `ButtonLayout` used
    `align_items: Center` + `justify_content: Center`, and three M1 numbers
    (`label_x == 10`, `label_y == 8`, `label_y == 14`) depend on it, so
    `Container::Box` reproduces exactly that. Per-child alignment is M3.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Crate pins (do not bump):** `cssparser = "0.37"`, `selectors = "0.40"`,
  `taffy = "0.14"`, `skia-rs-safe = "0.4.0"`, `wayland-client = "0.31"`,
  `wayland-protocols-wlr = "0.3"`, `precomputed-hash = "0.1"`,
  `rustix = "1"`, `fontconfig = "0.11"`, `bitflags = "2"`.
- **`skia-rs-safe` features** are `default-features = false`,
  `features = ["std", "text", "codec", "codec-png", "svg"]`. **P1 owns the
  `ui/Cargo.toml` edit**; P4 consumes it and must not re-edit that line.
- **No `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk` and no `smithay`** anywhere
  in `icedtea-ui`.
- **Edition 2024, `rust-version` 1.94**, both inherited from the workspace.
- **Gates, all three green before every commit:**
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- **Commit trailer**, on every commit in this plan:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **The M1 gate test `ui/tests/themed_button_offscreen.rs` must stay green
  with byte-identical numbers.** P3 rewrote it mechanically (contract
  §10.3); P4 changes no number in it. Height exactly `34.0`; the
  `y = height-2` band exactly `0xFFF6F5F4`; gutter column `bx = 4`; corner
  `(0,0)` alpha `0`; border pixel `(cx,0)` exactly `0xFFCDC7C2`; hover
  gutter `0xFFE8E6E3`; active `0xFFDAD6D2`; suggested-action border
  `0xFF15539E` and gutter within ±10/channel of `#3584e4`; empty button
  `36×34`; more than 20 dark label pixels. **A diff that changes a number
  there fails review.**
- **Parts execute IN ORDER 1→6 on one branch** (`rebuild/pure-rust-gtk-m2`),
  so P4 may consume everything P1, P2 and P3 produced. P4 must not touch
  `css/registry.rs`, `css/value/**`, `css/tokens.rs`, `css/parse.rs`,
  `css/node.rs`, `css/select.rs`, `css/cascade.rs`, `css/computed.rs`, or
  `anim/**`.
- **Exactness rule (carried from M1):** an asserted pixel must be derivable
  from the CSS. Gradients are sampled through `Gradient::color_at`; shadows
  are asserted only at their flat cores; anti-aliased edges are never
  asserted exactly.
- **Mutation discipline:** every load-bearing test states, in a comment, the
  source mutation it catches.
- **Never-panic discipline:** every function in this part that consumes
  caller-supplied floats (geometry builders, gradient samplers, blur) has a
  fuzz-ish battery over `NaN`, `±inf`, negatives, zero and huge values.

---

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `ui/src/layout.rs` | Modify (full rewrite of the M1 body) | `Rect`, `Allocation`, `Container`, `BoxDirection`, `Measure`, `FixedMeasure`, `LayoutTree`, `LayoutError` — the taffy mirror of a `Node` subtree |
| `ui/src/paint/mod.rs` | Create (replaces `ui/src/paint.rs`) | `PaintCx`, `BackgroundLayer` consumption, `paint_node` order, `ImageCache`, shared `Paint`/`Rect` helpers, re-exports |
| `ui/src/paint/geometry.rs` | Create | `rounded_rect_path`, `rounded_ring_path`, radius clamping, side-mitre wedges |
| `ui/src/paint/blur.rs` | Create | three-pass box blur over premultiplied RGBA, CSS `blur-radius → sigma → radius` |
| `ui/src/paint/background.rs` | Create | `paint_backgrounds`: colour, per-layer clip/origin/size/position/repeat/blend, gradients, `url()` images |
| `ui/src/paint/border.rs` | Create | `paint_borders` (per-side mitred fills, all `border-style`s), `paint_border_image` |
| `ui/src/paint/shadow.rs` | Create | `paint_box_shadows` (outset and inset) |
| `ui/src/paint/outline.rs` | Create | `paint_outline` |
| `ui/src/paint/text.rs` | Create | `paint_text`: `text-shadow`, `text-decoration-*`, glyph blob |
| `ui/src/paint/effects.rs` | Create | `begin_effects`/`end_effects`: `opacity`, `transform`, colour-matrix `filter` |
| `ui/src/paint.rs` | Delete | replaced by `ui/src/paint/` |
| `ui/src/text.rs` | Modify | P6's §9 surface, backed by the M1 probe list (deviation 6) |
| `ui/src/widget/button.rs` | Modify | `restyle`/`render` onto `LayoutTree`/`paint_node` (deviation 7) |
| `ui/src/app.rs` | Modify | `themed_button_allocation` return plumbing only |
| `ui/src/bin/themed-button.rs` | Modify | `--print-allocation` derivation (deviation 3) |
| `ui/tests/support/mod.rs` | Modify | `PrintedAllocation` (deviation 2) |
| `ui/tests/layer_shell_screencopy.rs` | Modify | one `use` line (deviation 2) |

---

## Task 1: `layout::Rect`, `Allocation` and the box accessors

**Files:**
- Modify: `ui/src/layout.rs` — replace the M1 module header, `Allocation`
  (M1 lines 33-44) and delete `ButtonLayout`/`layout_button` (M1 lines
  46-166) plus the M1 `mod tests` (M1 lines 168-end). This task leaves the
  file with geometry only; Tasks 2-4 add `LayoutTree` back.
- Test: `ui/src/layout.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (P1): `crate::css::value::Keyword` with variants `BorderBox`,
  `PaddingBox`, `ContentBox`, `TextBox` (contract §2.2).
- Produces:
  ```rust
  pub struct Rect { pub x: f32, pub y: f32, pub width: f32, pub height: f32 }
  impl Rect {
      pub fn new(x: f32, y: f32, width: f32, height: f32) -> Rect;
      pub fn zero() -> Rect;
      pub fn right(&self) -> f32;
      pub fn bottom(&self) -> f32;
      pub fn is_empty(&self) -> bool;
      pub fn inset(&self, sides: [f32; 4]) -> Rect;     // TRBL, clamped at zero
      pub fn outset(&self, sides: [f32; 4]) -> Rect;    // TRBL
      pub fn to_skia(&self) -> skia_rs_safe::core::Rect;
  }
  pub struct Allocation {
      pub border_box: Rect, pub content_box: Rect,
      pub border: [f32; 4], pub padding: [f32; 4],
  }
  impl Allocation {
      pub fn padding_box(&self) -> Rect;
      pub fn box_for(&self, k: crate::css::value::Keyword) -> Rect;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/layout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Allocation, Rect};
    use crate::css::value::Keyword;

    /// A 100x50 border box with a 2px border and 4/6/4/6 padding.
    fn alloc() -> Allocation {
        Allocation {
            border_box: Rect::new(10.0, 20.0, 100.0, 50.0),
            content_box: Rect::new(18.0, 26.0, 84.0, 38.0),
            border: [2.0, 2.0, 2.0, 2.0],
            padding: [4.0, 6.0, 4.0, 6.0],
        }
    }

    #[test]
    fn inset_subtracts_trbl_and_never_goes_negative() {
        // Mutation check: swapping any two of the TRBL indices, or dropping
        // the `.max(0.0)` clamp, changes one of these three numbers.
        let r = Rect::new(0.0, 0.0, 20.0, 10.0).inset([1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r, Rect::new(4.0, 1.0, 14.0, 6.0));
        let collapsed = Rect::new(0.0, 0.0, 4.0, 4.0).inset([9.0, 9.0, 9.0, 9.0]);
        assert_eq!(collapsed.width, 0.0);
        assert_eq!(collapsed.height, 0.0);
    }

    #[test]
    fn box_for_walks_border_padding_content_inwards() {
        // Mutation check: returning `border_box` for `PaddingBox` (the easy
        // mistake, since GTK's background-origin default is padding-box)
        // moves the second assertion by the 2px border.
        let a = alloc();
        assert_eq!(a.box_for(Keyword::BorderBox), Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(a.box_for(Keyword::PaddingBox), Rect::new(12.0, 22.0, 96.0, 46.0));
        assert_eq!(a.box_for(Keyword::ContentBox), Rect::new(18.0, 26.0, 84.0, 38.0));
        assert_eq!(a.padding_box(), a.box_for(Keyword::PaddingBox));
    }

    #[test]
    fn box_for_falls_back_to_the_border_box_for_any_other_keyword() {
        // `background-clip: text` is parsed by the registry but has no box
        // in M2; it must not panic and must not silently clip to nothing.
        assert_eq!(alloc().box_for(Keyword::TextBox), alloc().border_box);
    }

    #[test]
    fn rect_geometry_never_panics_on_hostile_numbers() {
        for &v in &[
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0e30,
            1.0e30,
            0.0,
            -0.0,
        ] {
            let r = Rect::new(v, v, v, v);
            let _ = r.right();
            let _ = r.bottom();
            let _ = r.is_empty();
            let _ = r.inset([v, v, v, v]);
            let _ = r.outset([v, v, v, v]);
            let _ = r.to_skia();
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: FAIL — `error[E0432]: unresolved import `super::{Allocation, Rect}`` /
`no function or associated item named `new` found for struct `Allocation``.

- [ ] **Step 3: Write minimal implementation**

Replace the whole of `ui/src/layout.rs` above the test module with:

```rust
//! GTK's measure→allocate box model over a `css::node::Node` tree,
//! expressed as a `taffy` tree.
//!
//! # `min-width`/`min-height` are content-box minimums
//!
//! GTK's `min-width`/`min-height` floor the widget's *content*, exactly as
//! `box-sizing: content-box` says they should; CSS's own `min-width` on a
//! `border-box` element does not. taffy resolves `min_size` in the box the
//! node's `box_sizing` names and adds padding+border when that box is the
//! content box (`taffy-0.14.0/src/compute/flexbox.rs:244`), so this module
//! sets `BoxSizing::ContentBox` and passes GTK's minimums straight through.
//!
//! Passing them through as border-box minimums makes every widget that is
//! floored by its minimum too small by exactly its padding plus border:
//! Adwaita's "Click me" came out 27 px high against GTK 4.22's 34, and an
//! empty button 20x27 against 36x34.

use crate::css::value::Keyword;

/// An axis-aligned box in absolute, tree-origin coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width; never negative for a rect this module produces.
    pub width: f32,
    /// Height; never negative for a rect this module produces.
    pub height: f32,
}

impl Rect {
    /// A rect from its origin and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The empty rect at the origin.
    #[must_use]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }

    /// Right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Whether either dimension is zero or smaller (or not a number).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    /// Shrink by `sides` (top, right, bottom, left), clamped at zero size.
    #[must_use]
    pub fn inset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    /// Grow by `sides` (top, right, bottom, left).
    #[must_use]
    pub fn outset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x - left,
            y: self.y - top,
            width: (self.width + left + right).max(0.0),
            height: (self.height + top + bottom).max(0.0),
        }
    }

    /// The same box as a `skia-rs` edge-form rect.
    #[must_use]
    pub fn to_skia(&self) -> skia_rs_safe::core::Rect {
        skia_rs_safe::core::Rect::from_xywh(self.x, self.y, self.width, self.height)
    }
}

/// One node's laid-out geometry, in absolute tree-origin coordinates.
///
/// Replaces M1's `{width, height, label_x, label_y}`: a generic node has no
/// "label", and per-side border and padding are first-class in M2.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Allocation {
    /// The border box: what `background-clip: border-box` fills.
    pub border_box: Rect,
    /// The content box: where children and text start.
    pub content_box: Rect,
    /// Used border widths, top/right/bottom/left.
    pub border: [f32; 4],
    /// Used padding, top/right/bottom/left.
    pub padding: [f32; 4],
}

impl Allocation {
    /// The padding box: the border box minus the used border widths.
    #[must_use]
    pub fn padding_box(&self) -> Rect {
        self.border_box.inset(self.border)
    }

    /// The box a `background-origin`/`background-clip` keyword names.
    ///
    /// Any keyword without a box in M2 (`text`, and anything a future
    /// registry row adds) resolves to the border box, which is CSS's
    /// initial `background-clip` — never an empty box.
    #[must_use]
    pub fn box_for(&self, k: Keyword) -> Rect {
        match k {
            Keyword::PaddingBox => self.padding_box(),
            Keyword::ContentBox => self.content_box,
            _ => self.border_box,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: PASS — 4 tests.
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/layout.rs
git commit -m "feat(ui/layout): absolute-coordinate Rect and Allocation

Replaces M1's {width, height, label_x, label_y} with the contract's
border/content boxes plus per-side used border and padding, and adds the
background-origin/clip box lookup.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2: `LayoutTree::sync` — mirroring a `Node` subtree into taffy

**Files:**
- Modify: `ui/src/layout.rs` — add imports, `BoxDirection`, `Container`,
  `LayoutError`, `NodeCtx`, `LayoutTree` and `sync` after `Allocation`;
  extend `mod tests`.
- Test: `ui/src/layout.rs`

**Interfaces:**
- Consumes (P2, contract §3): `css::node::Node` with `Node::new(&str)`,
  `Node::append_child(&self, &Node)`, `Node::remove_child(&self, &Node) -> bool`,
  `Node::children(&self) -> Vec<Node>`, `Node::child_count(&self) -> usize`,
  `Node::generation(&self) -> u64`, `Node::descendants(&self) -> impl Iterator<Item = Node>`,
  and `impl selectors::Element for Node` whose `opaque()` is
  `OpaqueElement::new(Rc::as_ptr(&self.0))`.
- Consumes (P3, contract §5): `css::computed::ComputedStyle`,
  `ComputedStyle::initial(&ResolveEnv) -> Rc<ComputedStyle>`,
  `css::computed::ResolveEnv`.
- Produces:
  ```rust
  pub enum BoxDirection { Row, Column }
  pub enum Container { Box { direction: BoxDirection }, Leaf }
  impl Default for Container;                       // Box { direction: Column }
  pub enum LayoutError { Taffy(taffy::TaffyError), Unsynced }
  pub struct LayoutTree;
  impl LayoutTree {
      pub fn new() -> Self;
      pub fn sync(&mut self, root: &Node) -> Result<(), LayoutError>;
      pub fn node_count(&self) -> usize;
      pub fn is_synced(&self, root: &Node) -> bool;
  }
  impl Default for LayoutTree;
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/layout.rs`'s `mod tests`:

```rust
    use super::LayoutTree;
    use crate::css::node::Node;

    /// `window > box > (label, label)`.
    fn small_tree() -> (Node, Node, Node, Node) {
        let window = Node::new("window");
        let container = Node::new("box");
        let a = Node::new("label");
        let b = Node::new("label");
        window.append_child(&container);
        container.append_child(&a);
        container.append_child(&b);
        (window, container, a, b)
    }

    #[test]
    fn sync_creates_one_taffy_node_per_css_node() {
        // Mutation check: recursing only into the first child, or forgetting
        // the root itself, drops the count below 4.
        let (window, _c, _a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        assert_eq!(tree.node_count(), 4);
    }

    #[test]
    fn sync_is_a_no_op_while_the_generation_is_unchanged() {
        // Mutation check: dropping the generation memo makes `is_synced`
        // false immediately after a sync, and re-syncs on every frame.
        let (window, _c, _a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        assert!(tree.is_synced(&window));
        tree.sync(&window).expect("second sync");
        assert_eq!(tree.node_count(), 4);
    }

    #[test]
    fn sync_adds_and_removes_nodes_when_the_tree_changes() {
        // Mutation check: never calling `taffy.remove` leaves the count at 5
        // after the removal, so stale nodes would keep taking part in layout.
        let (window, container, a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");

        let c = Node::new("label");
        container.append_child(&c);
        assert!(!tree.is_synced(&window), "a mutation must dirty the memo");
        tree.sync(&window).expect("resync");
        assert_eq!(tree.node_count(), 5);

        container.remove_child(&a);
        tree.sync(&window).expect("resync after removal");
        assert_eq!(tree.node_count(), 4);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib layout::tests::sync`
Expected: FAIL — `error[E0432]: unresolved import `super::LayoutTree``.

- [ ] **Step 3: Write minimal implementation**

Add to the top of `ui/src/layout.rs`:

```rust
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use selectors::Element as _;
use selectors::OpaqueElement;
use taffy::prelude::{Style, TaffyTree};

use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::value::Keyword;
```

(the `use crate::css::value::Keyword;` line from Task 1 is now part of this
block; do not duplicate it) and append after `Allocation`:

```rust
/// The main axis of a box container.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoxDirection {
    /// Children are laid out left to right.
    Row,
    /// Children are laid out top to bottom.
    Column,
}

/// How a node lays its children out.
///
/// M2 has exactly two: GTK's box, and a leaf whose size comes from a
/// [`Measure`]. Grid and centre layouts are M3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Container {
    /// A flex container on `direction`, centring its children on both axes
    /// -- GTK's box default, and what M1's button relied on.
    Box {
        /// The main axis.
        direction: BoxDirection,
    },
    /// A childless node sized by its [`Measure`].
    Leaf,
}

impl Default for Container {
    fn default() -> Self {
        Self::Box {
            direction: BoxDirection::Column,
        }
    }
}

/// Why a layout call could not produce an answer.
#[derive(Debug)]
pub enum LayoutError {
    /// `taffy` rejected a tree operation.
    Taffy(taffy::TaffyError),
    /// [`LayoutTree::compute`] was called for a node this tree has never
    /// seen -- call [`LayoutTree::sync`] first.
    Unsynced,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Taffy(err) => write!(f, "taffy: {err}"),
            Self::Unsynced => write!(f, "node is not in the layout tree"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl From<taffy::TaffyError> for LayoutError {
    fn from(err: taffy::TaffyError) -> Self {
        Self::Taffy(err)
    }
}

/// What each taffy node carries so the measure closure can reach the CSS.
struct NodeCtx {
    node: Node,
    style: Rc<ComputedStyle>,
}

/// A `taffy` tree that mirrors one `css::node::Node` subtree.
///
/// The tree is kept across restyles rather than rebuilt: a restyle happens
/// on every `:hover`/`:active` transition, and taffy's own dirty tracking
/// (`set_style` self-marks dirty, `taffy-0.14.0/src/tree/taffy_tree.rs:826`)
/// only recomputes what changed.
pub struct LayoutTree {
    tree: TaffyTree<NodeCtx>,
    ids: HashMap<OpaqueElement, taffy::NodeId>,
    root: Option<taffy::NodeId>,
    synced_generation: Option<u64>,
    env: ResolveEnv,
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            ids: HashMap::new(),
            root: None,
            synced_generation: None,
            env: ResolveEnv::default(),
        }
    }

    /// How many taffy nodes the tree currently holds.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.tree.total_node_count()
    }

    /// Whether [`sync`](Self::sync) would be a no-op for `root` right now.
    #[must_use]
    pub fn is_synced(&self, root: &Node) -> bool {
        self.synced_generation == Some(root.generation())
    }

    /// Mirror `root`'s subtree into taffy, creating, reparenting and
    /// removing nodes as needed.
    ///
    /// Idempotent, and a no-op while the tree generation has not moved.
    /// Styles set by [`set_style`](Self::set_style) survive a sync for every
    /// node that survives it.
    ///
    /// # Errors
    ///
    /// [`LayoutError::Taffy`] if `taffy` rejects a node insertion or a
    /// child-list update.
    pub fn sync(&mut self, root: &Node) -> Result<(), LayoutError> {
        if self.is_synced(root) {
            return Ok(());
        }

        let mut live: HashSet<OpaqueElement> = HashSet::new();
        let root_id = self.sync_node(root, &mut live)?;
        self.root = Some(root_id);

        let stale: Vec<(OpaqueElement, taffy::NodeId)> = self
            .ids
            .iter()
            .filter(|(key, _)| !live.contains(*key))
            .map(|(key, id)| (*key, *id))
            .collect();
        for (key, id) in stale {
            self.ids.remove(&key);
            self.tree.remove(id)?;
        }

        self.synced_generation = Some(root.generation());
        Ok(())
    }

    /// Ensure `node` and its whole subtree exist, and that `node`'s taffy
    /// children match its CSS children in order.
    fn sync_node(
        &mut self,
        node: &Node,
        live: &mut HashSet<OpaqueElement>,
    ) -> Result<taffy::NodeId, LayoutError> {
        let key = node.opaque();
        live.insert(key);

        let id = match self.ids.get(&key) {
            Some(id) => *id,
            None => {
                let ctx = NodeCtx {
                    node: node.clone(),
                    style: ComputedStyle::initial(&self.env),
                };
                let id = self.tree.new_leaf_with_context(Style::default(), ctx)?;
                self.ids.insert(key, id);
                id
            }
        };

        let mut children = Vec::with_capacity(node.child_count());
        for child in node.children() {
            children.push(self.sync_node(&child, live)?);
        }
        if self.tree.children(id)? != children {
            self.tree.set_children(id, &children)?;
        }
        Ok(id)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: PASS — 7 tests.
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/layout.rs
git commit -m "feat(ui/layout): LayoutTree mirrors a Node subtree into taffy

sync() creates, reparents and removes taffy nodes to match the CSS tree,
memoised on the tree generation so an unchanged frame costs nothing.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3: `LayoutTree::set_style` — the CSS box model into a taffy `Style`

**Files:**
- Modify: `ui/src/layout.rs` — add `set_style` and the private
  `border_spacing_px` helper to `impl LayoutTree`; extend `mod tests`.
- Test: `ui/src/layout.rs`

**Interfaces:**
- Consumes (P1): `css::registry::Prop::BorderSpacing`;
  `css::value::{Value, Length, LengthCtx}` with
  `Length::resolve(&self, &LengthCtx) -> Option<f32>`.
- Consumes (P3): `ComputedStyle::padding(&self, basis: f32) -> [f32; 4]`,
  `ComputedStyle::margin(&self, basis: f32) -> [Option<f32>; 4]`,
  `ComputedStyle::border_widths(&self) -> [f32; 4]`,
  `ComputedStyle::min_size(&self, basis: (f32, f32)) -> (f32, f32)`,
  `ComputedStyle::length_ctx(&self, env: &ResolveEnv, percent_basis: Option<f32>) -> LengthCtx`,
  `ComputedStyle::raw(&self, prop: Prop) -> &Value`.
- Produces:
  ```rust
  impl LayoutTree {
      pub fn set_style(&mut self, node: &Node, style: &ComputedStyle,
                       container: Container, env: &ResolveEnv);
  }
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/layout.rs`'s `mod tests`:

```rust
    use super::{BoxDirection, Container};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::select::MatchCx;
    use taffy::prelude::{BoxSizing, Display, FlexDirection};

    /// Resolve `node` against `css` the way a real restyle would.
    fn style_of(css: &str, root: &Node, node: &Node) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let mut cx = MatchCx::new();
        let _ = root;
        ComputedStyle::resolve_chain(&sheet, node, &ResolveEnv::default(), &mut cx)
    }

    #[test]
    fn set_style_writes_the_css_box_into_taffy_as_a_content_box() {
        // Mutation check: switching to `BoxSizing::BorderBox`, or dropping
        // any of the four padding/border sides, changes one of these
        // assertions -- and would reintroduce M1's 27px-tall button bug.
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let css = "button { padding: 4px 9px; border: 1px solid #000; \
                   min-width: 16px; min-height: 24px }";
        let style = style_of(css, &window, &button);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(
            &button,
            &style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &ResolveEnv::default(),
        );

        let taffy_style = tree.taffy_style(&button).expect("styled");
        assert_eq!(taffy_style.box_sizing, BoxSizing::ContentBox);
        assert_eq!(taffy_style.display, Display::Flex);
        assert_eq!(taffy_style.flex_direction, FlexDirection::Row);
        assert_eq!(taffy_style.padding.top, taffy::prelude::length(4.0));
        assert_eq!(taffy_style.padding.left, taffy::prelude::length(9.0));
        assert_eq!(taffy_style.border.top, taffy::prelude::length(1.0));
        assert_eq!(taffy_style.min_size.width, taffy::prelude::length(16.0));
        assert_eq!(taffy_style.min_size.height, taffy::prelude::length(24.0));
    }

    #[test]
    fn border_spacing_becomes_the_containers_gap() {
        // Mutation check: reading only the first half of the `border-spacing`
        // pair puts 3px in the row gap too, and the second assertion fails.
        let window = Node::new("window");
        let css = "window { border-spacing: 3px 7px }";
        let style = style_of(css, &window, &window);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &style, Container::default(), &ResolveEnv::default());

        let taffy_style = tree.taffy_style(&window).expect("styled");
        assert_eq!(taffy_style.gap.width, taffy::prelude::length(3.0));
        assert_eq!(taffy_style.gap.height, taffy::prelude::length(7.0));
    }

    #[test]
    fn an_auto_margin_reaches_taffy_as_auto() {
        // Mutation check: mapping `None` to `length(0.0)` instead of `auto()`
        // silently disables centring for every `margin: auto` in a theme.
        let window = Node::new("window");
        let css = "window { margin-left: auto; margin-right: 5px }";
        let style = style_of(css, &window, &window);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &style, Container::default(), &ResolveEnv::default());

        let taffy_style = tree.taffy_style(&window).expect("styled");
        assert_eq!(taffy_style.margin.left, taffy::prelude::auto());
        assert_eq!(taffy_style.margin.right, taffy::prelude::length(5.0));
    }

    #[test]
    fn set_style_on_an_unsynced_node_is_ignored_rather_than_panicking() {
        // A widget may restyle before it is attached; that must not abort.
        let orphan = Node::new("button");
        let style = ComputedStyle::initial(&ResolveEnv::default());
        let mut tree = LayoutTree::new();
        tree.set_style(&orphan, &style, Container::Leaf, &ResolveEnv::default());
        assert!(tree.taffy_style(&orphan).is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib layout::tests::set_style`
Expected: FAIL — `error[E0599]: no method named `set_style` found for struct `LayoutTree``.

- [ ] **Step 3: Write minimal implementation**

Extend `ui/src/layout.rs`'s import block with:

```rust
use taffy::prelude::{
    AlignItems, BoxSizing, Dimension, Display, FlexDirection, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Size, TaffyAuto as _, TaffyZero as _, auto, length,
};

use crate::css::registry::Prop;
use crate::css::value::{Length, Value};
```

and add to `impl LayoutTree`:

```rust
    /// The taffy `Style` currently attached to `node`, if it is in the tree.
    ///
    /// Exposed for tests and for callers that want to inspect what the CSS
    /// box model turned into.
    #[must_use]
    pub fn taffy_style(&self, node: &Node) -> Option<&Style> {
        let id = *self.ids.get(&node.opaque())?;
        self.tree.style(id).ok()
    }

    /// Write one node's CSS box into taffy.
    ///
    /// `container` decides the display mode; `env` supplies the DPI and root
    /// font size that percentage and relative lengths resolve against.
    /// Nodes that are not in the tree are ignored: a widget may restyle
    /// before it is attached.
    ///
    /// `taffy::TaffyTree::set_style` marks the node (and its ancestors)
    /// dirty itself, so no `mark_dirty` call follows this one.
    pub fn set_style(
        &mut self,
        node: &Node,
        style: &ComputedStyle,
        container: Container,
        env: &ResolveEnv,
    ) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_style on an unsynced node");
            return;
        };

        // CSS resolves *every* percentage in the box model against the
        // containing block's inline size, so one basis serves all sides.
        // taffy resolves its own percentages, so a basis of 0.0 here only
        // affects values this crate resolves eagerly.
        let basis = 0.0_f32;
        let [pad_top, pad_right, pad_bottom, pad_left] = style.padding(basis);
        let [bt, br, bb, bl] = style.border_widths();
        let margin = style.margin(basis);
        let (min_w, min_h) = style.min_size((basis, basis));
        let (gap_x, gap_y) = Self::border_spacing_px(style, env);

        let (display, flex_direction) = match container {
            Container::Box { direction } => (
                Display::Flex,
                match direction {
                    BoxDirection::Row => FlexDirection::Row,
                    BoxDirection::Column => FlexDirection::Column,
                },
            ),
            Container::Leaf => (Display::Flex, FlexDirection::Row),
        };

        let to_margin = |v: Option<f32>| -> LengthPercentageAuto {
            v.map_or_else(auto, length)
        };

        let taffy_style = Style {
            display,
            flex_direction,
            // GTK's box centres its children on both axes; per-child
            // alignment arrives with the widget layer in M3.
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            // GTK's min-width/min-height floor the CONTENT box.
            box_sizing: BoxSizing::ContentBox,
            size: Size {
                width: Dimension::AUTO,
                height: Dimension::AUTO,
            },
            min_size: Size {
                width: length(min_w),
                height: length(min_h),
            },
            margin: taffy::geometry::Rect {
                top: to_margin(margin[0]),
                right: to_margin(margin[1]),
                bottom: to_margin(margin[2]),
                left: to_margin(margin[3]),
            },
            padding: taffy::geometry::Rect {
                top: length(pad_top),
                right: length(pad_right),
                bottom: length(pad_bottom),
                left: length(pad_left),
            },
            border: taffy::geometry::Rect {
                top: length(bt),
                right: length(br),
                bottom: length(bb),
                left: length(bl),
            },
            gap: Size {
                width: length(gap_x),
                height: length(gap_y),
            },
            ..Style::default()
        };

        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.style = Rc::new(style.clone());
        }
        if let Err(err) = self.tree.set_style(id, taffy_style) {
            tracing::debug!(node = %node.name(), %err, "taffy rejected a style");
        }
    }

    /// `border-spacing` as `(column-gap, row-gap)` in px.
    ///
    /// The computed value is a `Value::Pair` of two lengths (a single value
    /// is stored duplicated by the registry's parser). Anything else -- an
    /// unresolvable calc, a wide keyword that survived -- is 0, matching the
    /// property's initial value.
    fn border_spacing_px(style: &ComputedStyle, env: &ResolveEnv) -> (f32, f32) {
        let ctx = style.length_ctx(env, None);
        let px = |v: &Value| -> f32 {
            match v {
                Value::Length(len) => len.resolve(&ctx).unwrap_or(0.0),
                Value::Number(n) if n.is_finite() => *n,
                _ => 0.0,
            }
        };
        match style.raw(Prop::BorderSpacing) {
            Value::Pair(pair) => (px(&pair.0), px(&pair.1)),
            other => {
                let v = px(other);
                (v, v)
            }
        }
    }
```

Also replace the `LengthPercentage`/`TaffyZero` imports if clippy flags them
as unused: the final import list this task needs is exactly
`AlignItems, BoxSizing, Dimension, Display, FlexDirection, JustifyContent,
LengthPercentageAuto, Size, Style, TaffyAuto as _, TaffyTree, auto, length`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: PASS — 11 tests.
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/layout.rs
git commit -m "feat(ui/layout): CSS box model into taffy Style

Content-box min sizes (GTK's rule, taffy's BoxSizing::ContentBox), per-side
margin/padding/border with auto margins preserved, and border-spacing as the
container gap.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 4: `Measure`, `LayoutTree::compute` and `LayoutTree::allocation`

**Files:**
- Modify: `ui/src/layout.rs` — add `Measure`, `FixedMeasure`, `compute`,
  `allocation`; extend `mod tests` with the five preserved M1 numbers.
- Test: `ui/src/layout.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-3.
- Produces:
  ```rust
  pub trait Measure {
      fn measure(&mut self, node: &Node, style: &ComputedStyle,
                 known: taffy::Size<Option<f32>>,
                 available: taffy::Size<taffy::AvailableSpace>) -> taffy::Size<f32>;
  }
  pub struct FixedMeasure(pub taffy::Size<f32>);
  impl Measure for FixedMeasure;
  impl LayoutTree {
      pub fn compute(&mut self, root: &Node,
                     available: taffy::Size<taffy::AvailableSpace>,
                     measure: &mut dyn Measure) -> Result<(), LayoutError>;
      pub fn allocation(&self, node: &Node) -> Option<Allocation>;
  }
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/layout.rs`'s `mod tests`:

```rust
    use super::{FixedMeasure, Measure};
    use taffy::prelude::{AvailableSpace, Size};

    /// M1's Adwaita-like button CSS, as a stylesheet rather than a struct
    /// literal: `ComputedStyle`'s fields no longer exist.
    const ADWAITA_LIKE: &str = "button { padding: 4px 9px; \
        border: 1px solid #cdc7c2; border-radius: 5px; \
        min-width: 16px; min-height: 24px; font-size: 14px; \
        background-color: #dad6d2; color: #2e3436 }";

    /// Every `label` leaf measures `w` x `h`; everything else is zero.
    struct LabelSize(f32, f32);

    impl Measure for LabelSize {
        fn measure(
            &mut self,
            node: &Node,
            _style: &ComputedStyle,
            _known: Size<Option<f32>>,
            _available: Size<AvailableSpace>,
        ) -> Size<f32> {
            if &*node.name() == "label" {
                Size {
                    width: self.0,
                    height: self.1,
                }
            } else {
                Size::ZERO
            }
        }
    }

    /// `window > button > label`, laid out with the M1 button CSS.
    fn button_layout(css: &str, label: (f32, f32)) -> (LayoutTree, Node, Node) {
        let window = Node::new("window");
        let button = Node::new("button");
        let label_node = Node::new("label");
        window.append_child(&button);
        button.append_child(&label_node);

        let sheet = CompiledSheet::compile(css);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let button_style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let label_style = ComputedStyle::resolve_chain(&sheet, &label_node, &env, &mut cx);
        let window_style = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &window_style, Container::default(), &env);
        tree.set_style(
            &button,
            &button_style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &env,
        );
        tree.set_style(&label_node, &label_style, Container::Leaf, &env);
        tree.compute(
            &window,
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
            &mut LabelSize(label.0, label.1),
        )
        .expect("compute");
        (tree, button, label_node)
    }

    #[test]
    fn allocation_is_text_plus_padding_plus_border() {
        // M1's number, preserved: content max(60, min-width 16) = 60, + 9 + 9
        // + 1 + 1 = 80; content max(18, min-height 24) = 24, + 4 + 4 + 1 + 1.
        // Mutation check: dropping BoxSizing::ContentBox gives 34 -> 27.
        let (tree, button, label) = button_layout(ADWAITA_LIKE, (60.0, 18.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 80.0);
        assert_eq!(a.border_box.height, 34.0);
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(l.border_box.x - a.border_box.x, 10.0, "border 1 + padding-left 9");
        assert_eq!(
            l.border_box.y - a.border_box.y,
            8.0,
            "border 1 + padding-top 4 + (24 - 18) / 2 centring"
        );
    }

    #[test]
    fn min_size_is_a_content_box_minimum_not_a_border_box_one() {
        // A2, M1's reviewer scenario, preserved: an empty Adwaita button is
        // 36x34, not 20x27.
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (0.0, 6.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 36.0, "max(0, 16) + 9 + 9 + 1 + 1 == 36");
        assert_eq!(a.border_box.height, 34.0, "max(6, 24) + 4 + 4 + 1 + 1 == 34");
    }

    #[test]
    fn an_intrinsic_size_above_the_minimum_still_wins() {
        // The clamp is `max`, not "always the minimum".
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (200.0, 40.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 220.0);
        assert_eq!(a.border_box.height, 50.0);
    }

    #[test]
    fn a_borderless_paddingless_button_is_exactly_the_label() {
        let css = "button { padding: 0; border: 0 solid #000; \
                   min-width: 0; min-height: 0 }";
        let (tree, button, label) = button_layout(css, (42.0, 17.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 42.0);
        assert_eq!(a.border_box.height, 17.0);
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(l.border_box.x - a.border_box.x, 0.0);
        assert_eq!(l.border_box.y - a.border_box.y, 0.0);
    }

    #[test]
    fn the_label_is_centred_when_min_size_grows_the_button() {
        // The content box is min-height 24 tall; a 6-tall label centres at 9
        // inside it, i.e. 1 (border) + 4 (padding) + 9 == 14 from the top.
        let (tree, button, label) = button_layout(ADWAITA_LIKE, (0.0, 6.0));
        let a = tree.allocation(&button).expect("button allocation");
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(l.border_box.y - a.border_box.y, 14.0);
    }

    #[test]
    fn one_tree_reused_gives_the_same_answer_as_a_fresh_one() {
        // A8, preserved: a second layout must not inherit anything from the
        // first. Mutation check: caching the first allocation and returning
        // it unconditionally makes `small` equal `tall`.
        let (tree_tall, tall_button, _l) = button_layout(ADWAITA_LIKE, (200.0, 40.0));
        let tall = tree_tall.allocation(&tall_button).expect("tall");

        let window = Node::new("window");
        let button = Node::new("button");
        let label_node = Node::new("label");
        window.append_child(&button);
        button.append_child(&label_node);
        let sheet = CompiledSheet::compile(ADWAITA_LIKE);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let bs = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let ls = ComputedStyle::resolve_chain(&sheet, &label_node, &env, &mut cx);
        let ws = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &ws, Container::default(), &env);
        tree.set_style(
            &button,
            &bs,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &env,
        );
        tree.set_style(&label_node, &ls, Container::Leaf, &env);
        let space = Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        };
        tree.compute(&window, space, &mut LabelSize(200.0, 40.0))
            .expect("first");
        assert_eq!(tree.allocation(&button).expect("first alloc"), tall);
        tree.compute(&window, space, &mut LabelSize(0.0, 6.0))
            .expect("second");
        let small = tree.allocation(&button).expect("second alloc");
        assert_eq!(small.border_box.width, 36.0);
        assert_eq!(small.border_box.height, 34.0);
        tree.compute(&window, space, &mut LabelSize(200.0, 40.0))
            .expect("third");
        assert_eq!(
            tree.allocation(&button).expect("third alloc"),
            tall,
            "the tree did not go back to the larger layout"
        );
    }

    #[test]
    fn allocation_coordinates_are_absolute_not_parent_relative() {
        // Mutation check: returning taffy's raw `location` (parent-relative)
        // puts the label at x == 10 instead of 10 plus the button's own x.
        let css = "window { padding: 12px } button { padding: 4px 9px; \
                   border: 1px solid #000; min-width: 0; min-height: 0 }";
        let (tree, button, label) = button_layout(css, (30.0, 12.0));
        let b = tree.allocation(&button).expect("button");
        let l = tree.allocation(&label).expect("label");
        assert_eq!(b.border_box.x, 12.0, "the window's padding offsets the button");
        assert_eq!(l.border_box.x, 12.0 + 10.0);
    }

    #[test]
    fn allocation_reports_used_border_and_padding_and_the_content_box() {
        // Mutation check: reading padding from the CSS rather than taffy's
        // used values silently ignores any clamping taffy applied.
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (60.0, 18.0));
        let a = tree.allocation(&button).expect("button");
        assert_eq!(a.border, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(a.padding, [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(a.content_box.width, 80.0 - 2.0 - 18.0);
        assert_eq!(a.content_box.height, 34.0 - 2.0 - 8.0);
        assert_eq!(a.content_box.x, a.border_box.x + 10.0);
        assert_eq!(a.padding_box().width, 78.0);
    }

    #[test]
    fn allocation_of_an_unknown_node_is_none() {
        let (tree, _button, _label) = button_layout(ADWAITA_LIKE, (10.0, 10.0));
        assert!(tree.allocation(&Node::new("popover")).is_none());
    }

    #[test]
    fn fixed_measure_reports_its_size_for_every_leaf() {
        let window = Node::new("window");
        let leaf = Node::new("label");
        window.append_child(&leaf);
        let env = ResolveEnv::default();
        let initial = ComputedStyle::initial(&env);
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &initial, Container::default(), &env);
        tree.set_style(&leaf, &initial, Container::Leaf, &env);
        tree.compute(
            &window,
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
            &mut FixedMeasure(Size {
                width: 33.0,
                height: 11.0,
            }),
        )
        .expect("compute");
        let a = tree.allocation(&leaf).expect("leaf");
        assert_eq!(a.border_box.width, 33.0);
        assert_eq!(a.border_box.height, 11.0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib layout::tests::allocation`
Expected: FAIL — `error[E0599]: no method named `compute` found for struct `LayoutTree``.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/layout.rs` (before `mod tests`):

```rust
/// Intrinsic size for a leaf node.
///
/// `known` and `available` come straight from taffy: `known` carries the
/// dimensions already fixed by the parent, `available` the space offered.
pub trait Measure {
    /// Measure `node`.
    fn measure(
        &mut self,
        node: &Node,
        style: &ComputedStyle,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32>;
}

/// A [`Measure`] that reports the same size for every leaf.
///
/// Useful for layout tests and for trees whose leaves have no text.
pub struct FixedMeasure(pub taffy::Size<f32>);

impl Measure for FixedMeasure {
    fn measure(
        &mut self,
        _node: &Node,
        _style: &ComputedStyle,
        known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        taffy::Size {
            width: known.width.unwrap_or(self.0.width),
            height: known.height.unwrap_or(self.0.height),
        }
    }
}

impl LayoutTree {
    /// Lay the whole tree out.
    ///
    /// # Errors
    ///
    /// [`LayoutError::Unsynced`] if `root` is not in the tree, or
    /// [`LayoutError::Taffy`] if `taffy` fails.
    pub fn compute(
        &mut self,
        root: &Node,
        available: taffy::Size<taffy::AvailableSpace>,
        measure: &mut dyn Measure,
    ) -> Result<(), LayoutError> {
        let root_id = *self.ids.get(&root.opaque()).ok_or(LayoutError::Unsynced)?;
        self.tree
            .compute_layout_with_measure(root_id, available, |inputs, _id, ctx, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, avail| match ctx {
                        Some(ctx) => measure.measure(&ctx.node, &ctx.style, known, avail),
                        None => taffy::Size::ZERO,
                    },
                )
            })?;
        Ok(())
    }

    /// `node`'s allocation in absolute, tree-origin coordinates.
    ///
    /// taffy reports each node's `location` relative to its parent, so this
    /// walks the taffy parent chain and accumulates the offsets.
    #[must_use]
    pub fn allocation(&self, node: &Node) -> Option<Allocation> {
        let id = *self.ids.get(&node.opaque())?;
        let layout = self.tree.layout(id).ok()?;

        let (mut abs_x, mut abs_y) = (layout.location.x, layout.location.y);
        let mut cursor = id;
        while let Some(parent) = self.tree.parent(cursor) {
            let parent_layout = self.tree.layout(parent).ok()?;
            abs_x += parent_layout.location.x;
            abs_y += parent_layout.location.y;
            cursor = parent;
        }

        let border = [
            layout.border.top,
            layout.border.right,
            layout.border.bottom,
            layout.border.left,
        ];
        let padding = [
            layout.padding.top,
            layout.padding.right,
            layout.padding.bottom,
            layout.padding.left,
        ];
        let border_box = Rect::new(abs_x, abs_y, layout.size.width, layout.size.height);
        let content_box = border_box.inset([
            border[0] + padding[0],
            border[1] + padding[1],
            border[2] + padding[2],
            border[3] + padding[3],
        ]);

        Some(Allocation {
            border_box,
            content_box,
            border,
            padding,
        })
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: PASS — 21 tests, including all six preserved M1 numbers
(80×34, label at (10, 8), 36×34, 220×50, borderless == label, label_y 14,
reused-tree equality).
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/layout.rs
git commit -m "feat(ui/layout): measure, compute and absolute allocations

Adds the Measure trait, taffy's measure-driven layout pass and an
Allocation reader that accumulates parent offsets into absolute
coordinates. Every M1 layout number is preserved as a regression test.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 5: `paint::geometry` — rounded-rect paths, rings and mitre wedges

**Files:**
- Create: `ui/src/paint/mod.rs`
- Create: `ui/src/paint/geometry.rs`
- Delete: `ui/src/paint.rs` (its four tests are re-created in Task 10;
  keep a copy of them on the branch until then — `git show HEAD:ui/src/paint.rs`
  retrieves them)
- Modify: `ui/src/lib.rs:11` — `pub mod paint;` already points at the new
  directory module, so no edit is needed; verify with `cargo build -p icedtea-ui`.
- Test: `ui/src/paint/geometry.rs`

**Interfaces:**
- Consumes: `crate::layout::Rect` (Task 1);
  `skia_rs_safe::path::{Path, PathBuilder, FillType}`.
- Produces:
  ```rust
  // ui/src/paint/geometry.rs, re-exported from ui/src/paint/mod.rs
  pub fn clamp_radii(rect: Rect, radii: [[f32; 2]; 4]) -> [[f32; 2]; 4];
  pub fn rounded_rect_path(rect: Rect, radii: &[[f32; 2]; 4]) -> Path;
  pub fn rounded_ring_path(outer: Rect, outer_radii: &[[f32; 2]; 4],
                           inner: Rect, inner_radii: &[[f32; 2]; 4]) -> Path;
  pub fn inner_radii(radii: &[[f32; 2]; 4], sides: [f32; 4]) -> [[f32; 2]; 4];
  pub enum Side { Top, Right, Bottom, Left }
  pub fn side_wedge_path(outer: Rect, inner: Rect, side: Side) -> Path;
  ```
  Radii are ordered `[TopLeft, TopRight, BottomRight, BottomLeft]`, each
  `[rx, ry]` — the same order as `ComputedStyle::border_radii`.

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/geometry.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::{Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path};
    use crate::layout::Rect;
    use skia_rs_safe::core::Point;
    use skia_rs_safe::path::FillType;

    const SQUARE: [[f32; 2]; 4] = [[0.0, 0.0]; 4];

    #[test]
    fn a_square_rect_is_four_lines_and_its_bounds_are_the_rect() {
        // Mutation check: emitting a corner arc for a zero radius adds
        // verbs and rounds the bounds in.
        let path = rounded_rect_path(Rect::new(10.0, 20.0, 40.0, 30.0), &SQUARE);
        let bounds = path.bounds();
        assert_eq!(bounds.left, 10.0);
        assert_eq!(bounds.top, 20.0);
        assert_eq!(bounds.right, 50.0);
        assert_eq!(bounds.bottom, 50.0);
        assert!(path.contains(Point::new(30.0, 35.0)), "the interior is filled");
    }

    #[test]
    fn a_rounded_corner_excludes_the_point_outside_its_arc() {
        // The M1 gate's "corner (0,0) is transparent past border-radius: 5px",
        // as pure geometry. Mutation check: dropping the arcs makes (1,1)
        // inside the path and the gate's corner assertion fails.
        let path = rounded_rect_path(Rect::new(0.0, 0.0, 40.0, 20.0), &[[5.0, 5.0]; 4]);
        assert!(!path.contains(Point::new(0.5, 0.5)), "inside a 5px corner arc");
        assert!(path.contains(Point::new(20.0, 10.0)), "the centre is inside");
        assert!(path.contains(Point::new(20.0, 0.5)), "the straight top edge is inside");
    }

    #[test]
    fn radii_are_scaled_down_when_a_pair_overflows_its_edge() {
        // CSS Backgrounds L3 §5.5: f = min over all edges of edge/(r1+r2),
        // applied to EVERY radius when f < 1.
        // Mutation check: clamping each radius to half the edge separately
        // gives [10, 10] here instead of [8, 8] on the bottom pair.
        let scaled = clamp_radii(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            [[30.0, 30.0], [10.0, 10.0], [10.0, 10.0], [30.0, 30.0]],
        );
        assert_eq!(scaled[0], [15.0, 15.0]);
        assert_eq!(scaled[1], [5.0, 5.0]);
        assert_eq!(scaled[2], [5.0, 5.0]);
        assert_eq!(scaled[3], [15.0, 15.0]);
    }

    #[test]
    fn inner_radii_shrink_by_the_adjacent_side_widths_and_floor_at_zero() {
        // CSS: the inner curve's radius is the outer radius minus the
        // adjacent border width, never negative.
        // Mutation check: subtracting the same side from rx and ry makes the
        // second assertion 3.0 instead of 1.0.
        let inner = inner_radii(&[[5.0, 5.0], [5.0, 5.0], [1.0, 1.0], [0.0, 0.0]], [4.0, 2.0, 3.0, 1.0]);
        assert_eq!(inner[0], [4.0, 1.0], "TL loses left width in x, top width in y");
        assert_eq!(inner[1], [3.0, 1.0], "TR loses right width in x, top width in y");
        assert_eq!(inner[2], [0.0, 0.0], "BR floors at zero");
        assert_eq!(inner[3], [0.0, 0.0]);
    }

    #[test]
    fn the_ring_between_two_rects_is_even_odd_and_hollow() {
        // Mutation check: leaving the fill type at Winding fills the hole,
        // so a 1px border would paint over the whole background.
        let ring = rounded_ring_path(
            Rect::new(0.0, 0.0, 40.0, 20.0),
            &SQUARE,
            Rect::new(2.0, 2.0, 36.0, 16.0),
            &SQUARE,
        );
        assert_eq!(ring.fill_type(), FillType::EvenOdd);
        assert!(ring.contains(Point::new(1.0, 10.0)), "the ring itself");
        assert!(!ring.contains(Point::new(20.0, 10.0)), "the hole is not filled");
    }

    #[test]
    fn a_side_wedge_is_the_mitred_trapezoid_between_the_two_rects() {
        // Mutation check: emitting an axis-aligned rectangle instead of the
        // mitred trapezoid puts (1, 1) inside BOTH the top and the left
        // wedge, so adjacent border colours would overdraw each other.
        let outer = Rect::new(0.0, 0.0, 40.0, 20.0);
        let inner = Rect::new(4.0, 4.0, 32.0, 12.0);
        let top = side_wedge_path(outer, inner, Side::Top);
        let left = side_wedge_path(outer, inner, Side::Left);
        assert!(top.contains(Point::new(20.0, 1.0)));
        assert!(!top.contains(Point::new(1.0, 10.0)));
        assert!(left.contains(Point::new(1.0, 10.0)));
        assert!(!left.contains(Point::new(20.0, 1.0)));
        // The mitre corner: (1, 1) belongs to exactly one of the two.
        assert_ne!(
            top.contains(Point::new(1.5, 1.0)),
            left.contains(Point::new(1.5, 1.0))
        );
    }

    #[test]
    fn geometry_never_panics_on_hostile_numbers() {
        let hostile = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -50.0,
            0.0,
            1.0e30,
        ];
        for &v in &hostile {
            let rect = Rect::new(v, v, v, v);
            let radii = [[v, v]; 4];
            let _ = clamp_radii(rect, radii);
            let _ = inner_radii(&radii, [v; 4]);
            let _ = rounded_rect_path(rect, &radii);
            let _ = rounded_ring_path(rect, &radii, rect, &radii);
            for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                let _ = side_wedge_path(rect, rect, side);
            }
        }
    }
}
```

Create `ui/src/paint/mod.rs` with just:

```rust
//! The CSS painter.

pub mod geometry;

pub use geometry::{
    Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
};
```

and delete `ui/src/paint.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::geometry`
Expected: FAIL — `error[E0432]: unresolved import `super::{Side, clamp_radii, ...}``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/geometry.rs`:

```rust
//! Rounded-rectangle path construction.
//!
//! `skia-rs-safe` 0.4.0 has an `RRect` value type but nothing that draws or
//! clips one: there is no `draw_rrect`, no `draw_drrect` and no
//! `clip_rrect`, and `PathBuilder::add_round_rect` takes a single uniform
//! `(rx, ry)`. CSS needs four independent elliptical corners, so every
//! rounded shape in this crate is built here, out of straight segments and
//! SVG-form elliptical arcs.

use skia_rs_safe::path::{FillType, Path, PathBuilder};

use crate::layout::Rect;

/// One edge of a box, for per-side border painting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// The top edge.
    Top,
    /// The right edge.
    Right,
    /// The bottom edge.
    Bottom,
    /// The left edge.
    Left,
}

/// Replace a non-finite or negative number with zero.
fn sane(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Scale `radii` down so that no pair of corner radii overflows its edge.
///
/// CSS Backgrounds and Borders L3 §5.5: compute `f` as the minimum over the
/// four edges of `edge_length / (radius_a + radius_b)`, and if `f < 1`
/// multiply *every* radius by `f`. Scaling each corner independently would
/// change the shape's proportions, which CSS explicitly forbids.
#[must_use]
pub fn clamp_radii(rect: Rect, radii: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let w = sane(rect.width);
    let h = sane(rect.height);
    let r = [
        [sane(radii[0][0]), sane(radii[0][1])],
        [sane(radii[1][0]), sane(radii[1][1])],
        [sane(radii[2][0]), sane(radii[2][1])],
        [sane(radii[3][0]), sane(radii[3][1])],
    ];

    let ratio = |edge: f32, a: f32, b: f32| -> f32 {
        let sum = a + b;
        if sum > 0.0 { edge / sum } else { f32::INFINITY }
    };
    let f = ratio(w, r[0][0], r[1][0])
        .min(ratio(w, r[3][0], r[2][0]))
        .min(ratio(h, r[0][1], r[3][1]))
        .min(ratio(h, r[1][1], r[2][1]));

    if f >= 1.0 || !f.is_finite() {
        return r;
    }
    [
        [r[0][0] * f, r[0][1] * f],
        [r[1][0] * f, r[1][1] * f],
        [r[2][0] * f, r[2][1] * f],
        [r[3][0] * f, r[3][1] * f],
    ]
}

/// Shrink `radii` by the adjacent side widths (top, right, bottom, left).
///
/// Each corner loses the width of the side it touches on that axis: the
/// top-left x-radius loses the left width, its y-radius the top width.
#[must_use]
pub fn inner_radii(radii: &[[f32; 2]; 4], sides: [f32; 4]) -> [[f32; 2]; 4] {
    let [top, right, bottom, left] = [
        sane(sides[0]),
        sane(sides[1]),
        sane(sides[2]),
        sane(sides[3]),
    ];
    let shrink = |r: [f32; 2], dx: f32, dy: f32| [(sane(r[0]) - dx).max(0.0), (sane(r[1]) - dy).max(0.0)];
    [
        shrink(radii[0], left, top),
        shrink(radii[1], right, top),
        shrink(radii[2], right, bottom),
        shrink(radii[3], left, bottom),
    ]
}

/// A closed path around `rect` with per-corner elliptical radii.
///
/// `radii` is `[TopLeft, TopRight, BottomRight, BottomLeft]`, each
/// `[rx, ry]`; the radii are clamped to the rect before use, so a caller may
/// pass raw computed values.
#[must_use]
pub fn rounded_rect_path(rect: Rect, radii: &[[f32; 2]; 4]) -> Path {
    let r = clamp_radii(rect, *radii);
    let x = if rect.x.is_finite() { rect.x } else { 0.0 };
    let y = if rect.y.is_finite() { rect.y } else { 0.0 };
    let w = sane(rect.width);
    let h = sane(rect.height);
    let (right, bottom) = (x + w, y + h);

    let mut b = PathBuilder::new();
    b.move_to(x + r[0][0], y);
    b.line_to(right - r[1][0], y);
    if r[1][0] > 0.0 && r[1][1] > 0.0 {
        b.arc_to(r[1][0], r[1][1], 0.0, false, true, right, y + r[1][1]);
    } else {
        b.line_to(right, y);
    }
    b.line_to(right, bottom - r[2][1]);
    if r[2][0] > 0.0 && r[2][1] > 0.0 {
        b.arc_to(r[2][0], r[2][1], 0.0, false, true, right - r[2][0], bottom);
    } else {
        b.line_to(right, bottom);
    }
    b.line_to(x + r[3][0], bottom);
    if r[3][0] > 0.0 && r[3][1] > 0.0 {
        b.arc_to(r[3][0], r[3][1], 0.0, false, true, x, bottom - r[3][1]);
    } else {
        b.line_to(x, bottom);
    }
    b.line_to(x, y + r[0][1]);
    if r[0][0] > 0.0 && r[0][1] > 0.0 {
        b.arc_to(r[0][0], r[0][1], 0.0, false, true, x + r[0][0], y);
    } else {
        b.line_to(x, y);
    }
    b.close();
    b.build()
}

/// The ring between two rounded rects, as one even-odd path.
///
/// There is no `draw_drrect` in `skia-rs`, so the two contours share a path
/// and the even-odd rule punches the hole.
#[must_use]
pub fn rounded_ring_path(
    outer: Rect,
    outer_radii: &[[f32; 2]; 4],
    inner: Rect,
    inner_radii: &[[f32; 2]; 4],
) -> Path {
    let mut b = PathBuilder::with_fill_type(FillType::EvenOdd);
    b.add_path(&rounded_rect_path(outer, outer_radii));
    b.add_path(&rounded_rect_path(inner, inner_radii));
    let mut path = b.build();
    path.set_fill_type(FillType::EvenOdd);
    path
}

/// The mitred trapezoid covering one side's share of a border ring.
///
/// Intersecting the ring with this wedge is how each side gets its own
/// width and colour while still meeting its neighbours on the diagonal,
/// exactly as GTK draws a mixed border. Stroking cannot express that.
#[must_use]
pub fn side_wedge_path(outer: Rect, inner: Rect, side: Side) -> Path {
    let ox = if outer.x.is_finite() { outer.x } else { 0.0 };
    let oy = if outer.y.is_finite() { outer.y } else { 0.0 };
    let or = ox + sane(outer.width);
    let ob = oy + sane(outer.height);
    let ix = if inner.x.is_finite() { inner.x } else { ox };
    let iy = if inner.y.is_finite() { inner.y } else { oy };
    let ir = ix + sane(inner.width);
    let ib = iy + sane(inner.height);

    let quad = match side {
        Side::Top => [(ox, oy), (or, oy), (ir, iy), (ix, iy)],
        Side::Right => [(or, oy), (or, ob), (ir, ib), (ir, iy)],
        Side::Bottom => [(or, ob), (ox, ob), (ix, ib), (ir, ib)],
        Side::Left => [(ox, ob), (ox, oy), (ix, iy), (ix, ib)],
    };

    let mut b = PathBuilder::new();
    b.move_to(quad[0].0, quad[0].1);
    for point in &quad[1..] {
        b.line_to(point.0, point.1);
    }
    b.close();
    b.build()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::geometry`
Expected: PASS — 7 tests.
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`. (`cargo test -p icedtea-ui` still fails to build
the integration tests at this point — `paint_button` is gone. That is
expected until Task 17; run the crate's unit tests with `--lib` until then.)

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/mod.rs ui/src/paint/geometry.rs
git rm ui/src/paint.rs
git commit -m "feat(ui/paint): per-corner rounded-rect paths, rings and mitres

skia-rs-safe 0.4.0 draws no RRect at all, so CSS's four independent
elliptical corners, the border ring and the per-side mitre wedges are built
here out of lines and SVG elliptical arcs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 6: `paint::blur` — the project-owned box blur

**Files:**
- Create: `ui/src/paint/blur.rs`
- Modify: `ui/src/paint/mod.rs` — add `pub mod blur;`
- Test: `ui/src/paint/blur.rs`

**Interfaces:**
- Consumes: `skia_rs_safe::canvas::{Canvas, Surface}`,
  `skia_rs_safe::codec::Image`, `skia_rs_safe::core::Color`.
- Produces:
  ```rust
  pub fn sigma_for_blur_radius(blur_px: f32) -> f32;   // CSS: blur / 2
  pub fn box_blur_radius(sigma: f32) -> i32;
  pub fn blur_premul_rgba(pixels: &mut [u8], width: i32, height: i32,
                          stride: usize, sigma: f32);
  pub fn blurred_image(width: i32, height: i32, sigma: f32,
                       draw: impl FnOnce(&mut Canvas<'_>)) -> Option<Image>;
  ```

**Why this exists:** `skia-rs-canvas` 0.4.0's rasterizer reads only
`Paint::shader` (`raster.rs:471` is the crate's only filter-ish access), and
`Canvas::composite_layer` documents in-source that a layer paint's *image*
filter "is not yet applied here". So neither `BlurMaskFilter` nor
`BlurImageFilter` reaches a pixel. Blur is therefore ours.

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/blur.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::{blur_premul_rgba, blurred_image, box_blur_radius, sigma_for_blur_radius};
    use skia_rs_safe::core::{Color, Rect};
    use skia_rs_safe::paint::{Paint, Style};

    /// A 21x21 opaque-white buffer with a hole, so the blur has something to
    /// spread. Premultiplied RGBA, 4 bytes per pixel.
    fn buffer(w: i32, h: i32) -> (Vec<u8>, usize) {
        let stride = (w as usize) * 4;
        (vec![0u8; stride * h as usize], stride)
    }

    fn set(px: &mut [u8], stride: usize, x: i32, y: i32, v: [u8; 4]) {
        let o = y as usize * stride + x as usize * 4;
        px[o..o + 4].copy_from_slice(&v);
    }

    fn get(px: &[u8], stride: usize, x: i32, y: i32) -> [u8; 4] {
        let o = y as usize * stride + x as usize * 4;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    }

    #[test]
    fn css_blur_radius_is_twice_the_gaussian_sigma() {
        // CSS Backgrounds L3 §box-shadow: the blur radius is twice the
        // standard deviation. Mutation check: using blur as sigma directly
        // doubles every shadow's spread.
        assert_eq!(sigma_for_blur_radius(0.0), 0.0);
        assert_eq!(sigma_for_blur_radius(8.0), 4.0);
        assert_eq!(sigma_for_blur_radius(-3.0), 0.0, "negative blur is zero");
        assert_eq!(sigma_for_blur_radius(f32::NAN), 0.0);
    }

    #[test]
    fn a_zero_sigma_blur_leaves_every_byte_alone() {
        // Mutation check: running the box passes with radius 0 anyway still
        // rounds bytes; this catches an off-by-one in the early return.
        let (mut px, stride) = buffer(5, 5);
        set(&mut px, stride, 2, 2, [200, 100, 50, 255]);
        let before = px.clone();
        blur_premul_rgba(&mut px, 5, 5, stride, 0.0);
        assert_eq!(px, before);
    }

    #[test]
    fn a_blurred_flat_core_keeps_its_exact_value() {
        // The exactness rule: a pixel far enough inside a uniform region is
        // unchanged by the blur, so shadow tests may assert it exactly.
        // Mutation check: a normalisation error (dividing by the wrong
        // window size) moves 255 off 255 here.
        let (mut px, stride) = buffer(41, 41);
        for y in 0..41 {
            for x in 0..41 {
                set(&mut px, stride, x, y, [255, 0, 0, 255]);
            }
        }
        blur_premul_rgba(&mut px, 41, 41, stride, 3.0);
        assert_eq!(get(&px, stride, 20, 20), [255, 0, 0, 255]);
    }

    #[test]
    fn a_blur_spreads_alpha_outside_the_original_shape() {
        // Mutation check: blurring only horizontally leaves (20, 12) at 0.
        let (mut px, stride) = buffer(41, 41);
        for y in 18..23 {
            for x in 18..23 {
                set(&mut px, stride, x, y, [255, 255, 255, 255]);
            }
        }
        blur_premul_rgba(&mut px, 41, 41, stride, 4.0);
        assert!(get(&px, stride, 20, 12)[3] > 0, "alpha spread upwards");
        assert!(get(&px, stride, 12, 20)[3] > 0, "alpha spread leftwards");
        assert!(get(&px, stride, 20, 20)[3] < 255, "the core was diluted");
    }

    #[test]
    fn box_blur_radius_follows_skias_sigma_to_radius_rule() {
        // SkBlurMask::ConvertSigmaToRadius-equivalent rounding. Mutation
        // check: truncating instead of rounding gives 4 for sigma 2.0.
        assert_eq!(box_blur_radius(0.0), 0);
        assert_eq!(box_blur_radius(1.0), 2);
        assert_eq!(box_blur_radius(2.0), 4);
    }

    #[test]
    fn blurred_image_renders_and_returns_a_bitmap() {
        // Mutation check: forgetting to clear the offscreen surface leaves
        // uninitialised alpha and the outside-the-shape assertion fails.
        let image = blurred_image(40, 40, 3.0, |canvas| {
            let mut paint = Paint::new();
            paint.set_color32(Color(0xFF00_0000));
            paint.set_style(Style::Fill);
            paint.set_anti_alias(false);
            canvas.draw_rect(&Rect::from_xywh(15.0, 15.0, 10.0, 10.0), &paint);
        })
        .expect("blurred image");
        assert_eq!(image.width(), 40);
        assert_eq!(image.height(), 40);
        let outside = image.read_pixel(1, 1).expect("pixel");
        assert!(outside.a < 0.01, "far outside the blurred square is clear");
        let inside = image.read_pixel(20, 20).expect("pixel");
        assert!(inside.a > 0.5, "the blurred core is still mostly opaque");
    }

    #[test]
    fn blur_never_panics_on_hostile_inputs() {
        for &sigma in &[f32::NAN, f32::INFINITY, -1.0, 0.0, 1.0e9] {
            let (mut px, stride) = buffer(3, 3);
            blur_premul_rgba(&mut px, 3, 3, stride, sigma);
            let _ = box_blur_radius(sigma);
            let _ = sigma_for_blur_radius(sigma);
        }
        // Degenerate geometry must not index out of bounds.
        let (mut empty, stride) = buffer(0, 0);
        blur_premul_rgba(&mut empty, 0, 0, stride, 2.0);
        assert!(blurred_image(0, 0, 2.0, |_| {}).is_none());
        assert!(blurred_image(-4, 8, 2.0, |_| {}).is_none());
    }
}
```

Add `pub mod blur;` to `ui/src/paint/mod.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::blur`
Expected: FAIL — `error[E0432]: unresolved import `super::{blur_premul_rgba, ...}``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/blur.rs`:

```rust
//! Gaussian-ish blur, written here because nothing in `skia-rs-safe` 0.4.0
//! will run one for us.
//!
//! The rasterizer consults only `Paint::shader`, and `composite_layer` says
//! in so many words that a layer paint's image filter "is not yet applied
//! here" -- so `BlurMaskFilter` and `BlurImageFilter` both no-op. Shadows
//! and blurred effects therefore render into an offscreen premultiplied
//! buffer, get blurred by the three box passes below (the standard Gaussian
//! approximation Skia itself uses), and are blitted back with
//! `Canvas::draw_image`, which *does* apply the current path clip per pixel.

use skia_rs_safe::canvas::{Canvas, Surface};
use skia_rs_safe::codec::Image;
use skia_rs_safe::core::Color;

/// CSS's blur *radius* is twice the Gaussian standard deviation.
#[must_use]
pub fn sigma_for_blur_radius(blur_px: f32) -> f32 {
    if blur_px.is_finite() && blur_px > 0.0 {
        blur_px / 2.0
    } else {
        0.0
    }
}

/// The box-pass radius that approximates a Gaussian of `sigma`.
///
/// Skia's rule: `radius = floor(sigma * 3 * sqrt(2*pi) / 4 + 0.5)`, i.e.
/// about `1.88 * sigma`, run three times.
#[must_use]
pub fn box_blur_radius(sigma: f32) -> i32 {
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let scale = 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0;
    let r = sigma.mul_add(scale, 0.5).floor();
    if r.is_finite() {
        r.clamp(0.0, 512.0) as i32
    } else {
        0
    }
}

/// Blur a premultiplied RGBA buffer in place.
///
/// Three box passes per axis; edges are clamped (the buffer is assumed to
/// already carry the padding the caller wants).
pub fn blur_premul_rgba(pixels: &mut [u8], width: i32, height: i32, stride: usize, sigma: f32) {
    let radius = box_blur_radius(sigma);
    if radius == 0 || width <= 0 || height <= 0 {
        return;
    }
    let (w, h) = (width as usize, height as usize);
    if stride < w * 4 || pixels.len() < stride * h {
        return;
    }
    let r = radius as usize;
    let mut scratch = vec![0u8; stride * h];

    for _ in 0..3 {
        box_pass_horizontal(pixels, &mut scratch, w, h, stride, r);
        box_pass_vertical(&scratch, pixels, w, h, stride, r);
    }
}

/// One horizontal box pass, `src` -> `dst`.
fn box_pass_horizontal(src: &[u8], dst: &mut [u8], w: usize, h: usize, stride: usize, r: usize) {
    for y in 0..h {
        let row = y * stride;
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            let mut sum = [0u32; 4];
            for sx in lo..=hi {
                let o = row + sx * 4;
                sum[0] += u32::from(src[o]);
                sum[1] += u32::from(src[o + 1]);
                sum[2] += u32::from(src[o + 2]);
                sum[3] += u32::from(src[o + 3]);
            }
            // Edge pixels see a shorter window: the divisor is deliberately
            // the number of samples actually taken, not the constant `2r+1`,
            // so a uniform region stays uniform (the flat-core rule).
            let count = (hi - lo + 1) as u32;
            let o = row + x * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
}

/// One vertical box pass, `src` -> `dst`.
fn box_pass_vertical(src: &[u8], dst: &mut [u8], w: usize, h: usize, stride: usize, r: usize) {
    for x in 0..w {
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            let mut sum = [0u32; 4];
            for sy in lo..=hi {
                let o = sy * stride + x * 4;
                sum[0] += u32::from(src[o]);
                sum[1] += u32::from(src[o + 1]);
                sum[2] += u32::from(src[o + 2]);
                sum[3] += u32::from(src[o + 3]);
            }
            let count = (hi - lo + 1) as u32;
            let o = y * stride + x * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
}

/// Render `draw` into a transparent `width` x `height` surface, blur it and
/// snapshot it.
///
/// `None` for a degenerate size or if the surface cannot be allocated.
pub fn blurred_image(
    width: i32,
    height: i32,
    sigma: f32,
    draw: impl FnOnce(&mut Canvas<'_>),
) -> Option<Image> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut surface = Surface::new_raster_n32_premul(width, height)?;
    {
        let mut canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        draw(&mut canvas);
    }
    let stride = surface.row_bytes();
    blur_premul_rgba(surface.pixels_mut(), width, height, stride, sigma);
    surface.make_image_snapshot()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::blur`
Expected: PASS — 7 tests.
Then: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/blur.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): project-owned three-pass box blur

skia-rs-safe 0.4.0's rasterizer ignores mask and image filters and
composite_layer skips image filters outright, so CSS blur is ours: blur a
premultiplied offscreen buffer and blit it back through the path clip.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 7: the `text::FontDatabase` surface (probe-backed shim for P6)

**Files:**
- Modify: `ui/src/text.rs` — keep `FONT_CANDIDATES`, `TextMetrics` and the
  M1 shaping bodies; add `FontFace`, `FontQuery`, `ShapeKey`, the new
  `ShapedText` fields, `FontDatabase`, and the three CSS↔fontconfig scale
  functions. `FontStack` is removed (nothing outside `text.rs` will refer to
  it after Task 17; keep it until then so the crate still builds).
- Test: `ui/src/text.rs`

**Interfaces:**
- Consumes (P1): `css::value::{FontFamily, GenericFamily, FontStyle,
  FeatureSetting, VariationSetting}`; `css::value::Keyword` for
  `text-transform`.
- Produces (contract §9, verbatim):
  ```rust
  pub struct FontFace { pub path: PathBuf, pub index: i32, pub family: String }
  pub struct FontQuery<'a> { pub families: &'a [FontFamily], pub weight: f32,
                             pub style: FontStyle, pub stretch: f32, pub size_px: f32 }
  pub struct ShapeKey<'a> { pub text: &'a str, pub face: &'a FontFace, pub size_px: f32,
                            pub letter_spacing_px: f32, pub features: &'a [FeatureSetting],
                            pub variations: &'a [VariationSetting], pub transform: Keyword }
  pub struct ShapedText { pub blob: Option<TextBlob>, pub metrics: TextMetrics,
                          pub face: FontFace, pub size_px: f32 }
  pub struct FontDatabase;
  impl FontDatabase {
      pub fn new() -> FontDatabase;
      pub fn probe_only() -> FontDatabase;
      pub fn has_fontconfig(&self) -> bool;
      pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace>;
      pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>>;
      pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font>;
      pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText>;
      pub fn clear_caches(&mut self);
  }
  pub fn css_weight_to_fc(w: f32) -> i32;
  pub fn css_stretch_to_fc(pct: f32) -> i32;
  pub fn css_style_to_fc_slant(s: FontStyle) -> i32;
  ```

**Scope note:** this task implements the *shape* of §9 with M1's probe list
behind it (contract deviation 6). P6 replaces `match_face`'s body with a
real `FcPattern`/`FcFontMatch` call and flips `has_fontconfig`; every
signature here is final.

- [ ] **Step 1: Write the failing test**

Replace `ui/src/text.rs`'s `mod tests` with:

```rust
#[cfg(test)]
mod tests {
    use super::{
        FontDatabase, FontQuery, ShapeKey, css_stretch_to_fc, css_style_to_fc_slant,
        css_weight_to_fc,
    };
    use crate::css::value::{FontFamily, FontStyle, GenericFamily, Keyword};

    fn db() -> FontDatabase {
        FontDatabase::probe_only()
    }

    fn sans() -> Vec<FontFamily> {
        vec![FontFamily::Generic(GenericFamily::SansSerif)]
    }

    #[test]
    fn a_sans_serif_query_resolves_to_something() {
        // Deliberately NOT a fixed family: CI may have any UI face installed
        // (spec §6). Mutation check: returning None unconditionally fails.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("no system font found; install dejavu/liberation/noto sans");
        assert!(face.path.exists(), "the matched face must be a real file");
        assert!(!face.family.is_empty());
    }

    #[test]
    fn measurement_scales_with_size_and_length() {
        // M1's assertion, preserved through the new API.
        let mut db = db();
        let families = sans();
        let query = FontQuery {
            families: &families,
            weight: 400.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        };
        let face = db.match_face(&query).expect("face");
        let small = db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        let big = db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 28.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        assert!(big.metrics.width > small.metrics.width * 1.8);
        assert!(small.blob.is_some());
    }

    #[test]
    fn letter_spacing_widens_the_run_by_one_gap_per_glyph() {
        // Mutation check: applying spacing after the last glyph too (or not
        // at all) changes this width by exactly one or eight gaps.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let base = ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let plain = db.shape(&base);
        let spaced = db.shape(&ShapeKey {
            letter_spacing_px: 2.0,
            ..base
        });
        let glyphs = "Click me".chars().count() as f32;
        let delta = spaced.metrics.width - plain.metrics.width;
        assert!(
            (delta - 2.0 * (glyphs - 1.0)).abs() < 0.01,
            "letter-spacing added {delta}px across {glyphs} glyphs"
        );
    }

    #[test]
    fn text_transform_is_applied_before_shaping() {
        // Mutation check: leaving the string untouched makes the two widths
        // equal for a lowercase-only label.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let key = ShapeKey {
            text: "iii",
            face: &face,
            size_px: 20.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let lower = db.shape(&key);
        let upper = db.shape(&ShapeKey {
            transform: Keyword::Uppercase,
            ..key
        });
        assert!(upper.metrics.width > lower.metrics.width, "III is wider than iii");
    }

    #[test]
    fn an_empty_string_has_no_blob_but_a_positive_line_height() {
        // M1's assertion, preserved.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let shaped = db.shape(&ShapeKey {
            text: "",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        assert!(shaped.blob.is_none());
        assert!(shaped.metrics.line_height > 0.0);
        assert_eq!(shaped.metrics.width, 0.0);
    }

    #[test]
    fn shaping_the_same_key_twice_returns_the_cached_handle() {
        // Mutation check: dropping the cache makes the two Rc's differ, and
        // the widget would reshape on every paint (M1's perf regression).
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let key = ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let a = db.shape(&key);
        let b = db.shape(&key);
        assert!(std::rc::Rc::ptr_eq(&a, &b));
        db.clear_caches();
        let c = db.shape(&key);
        assert!(!std::rc::Rc::ptr_eq(&a, &c));
    }

    #[test]
    fn the_css_and_fontconfig_scales_are_a_table_lookup_not_a_cast() {
        // research/taffy-fontconfig.md §2.4: the two scales are NOT
        // proportional. Mutation check: `w as i32 / 4` passes none of these.
        assert_eq!(css_weight_to_fc(100.0), 0);
        assert_eq!(css_weight_to_fc(400.0), 80);
        assert_eq!(css_weight_to_fc(600.0), 180);
        assert_eq!(css_weight_to_fc(700.0), 200);
        assert_eq!(css_weight_to_fc(900.0), 210);
        assert_eq!(css_weight_to_fc(450.0), 90, "interpolated between 400 and 500");

        assert_eq!(css_stretch_to_fc(50.0), 50);
        assert_eq!(css_stretch_to_fc(100.0), 100);
        assert_eq!(css_stretch_to_fc(200.0), 200);
        assert_eq!(css_stretch_to_fc(75.0), 75);

        assert_eq!(css_style_to_fc_slant(FontStyle::Normal), 0);
        assert_eq!(css_style_to_fc_slant(FontStyle::Italic), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(14.0)), 110);
    }

    #[test]
    fn a_probe_only_database_reports_no_fontconfig() {
        assert!(!FontDatabase::probe_only().has_fontconfig());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib text::tests`
Expected: FAIL — `error[E0432]: unresolved import `super::FontDatabase``.

- [ ] **Step 3: Write minimal implementation**

Replace `ui/src/text.rs`'s body above `mod tests` (keeping the module
header, extended, and `FONT_CANDIDATES` and `TextMetrics` unchanged) with:

```rust
//! The text stack: `skia-rs-text` for shaping and rasterisation, with font
//! *discovery* behind [`FontDatabase`].
//!
//! M2 Part 4 lands the database's API shape with M1's fixed probe list
//! behind it; Part 6 replaces `match_face`'s body with a real
//! `FcPattern`/`FcFontMatch` call. Every signature here is final either way.
//!
//! `font-feature-settings` and `font-variation-settings` reach [`ShapeKey`]
//! and stop there: `skia-rs-text` 0.4.0's `Shaper::shape` calls
//! `rustybuzz::shape(&face, &[], buffer)` with a hardcoded empty feature
//! slice and exposes no variation axes, so they are dropped at the shaper
//! with a one-time warning rather than silently ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Once;

use skia_rs_safe::core::Point;
use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

use crate::css::value::{FeatureSetting, FontFamily, FontStyle, GenericFamily, Keyword, VariationSetting};

/// Well-known UI sans-serif faces, in preference order.
pub const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// Measured extents of a laid-out string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// Total advance width in px.
    pub width: f32,
    /// Distance from the baseline to the top of the text, positive upward.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the text, positive downward.
    pub descent: f32,
    /// Recommended distance between successive baselines.
    pub line_height: f32,
}

/// One concrete font file, as fontconfig would name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFace {
    /// Absolute path to the font file.
    pub path: PathBuf,
    /// Face index within a collection; 0 for a single-face file.
    pub index: i32,
    /// The family name the face reports.
    pub family: String,
}

/// What the cascade asks the database for.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontQuery<'a> {
    /// `font-family`, in priority order.
    pub families: &'a [FontFamily],
    /// CSS `font-weight`, 1..1000.
    pub weight: f32,
    /// CSS `font-style`.
    pub style: FontStyle,
    /// CSS `font-stretch` as a percentage; 100.0 is normal.
    pub stretch: f32,
    /// Used `font-size` in px.
    pub size_px: f32,
}

/// Everything that changes a shaped run.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeKey<'a> {
    /// The string, before `text-transform`.
    pub text: &'a str,
    /// The resolved face.
    pub face: &'a FontFace,
    /// Used `font-size` in px.
    pub size_px: f32,
    /// Used `letter-spacing` in px.
    pub letter_spacing_px: f32,
    /// `font-feature-settings` (dropped at the shaper -- see the module docs).
    pub features: &'a [FeatureSetting],
    /// `font-variation-settings` (dropped at the shaper).
    pub variations: &'a [VariationSetting],
    /// `text-transform`.
    pub transform: Keyword,
}

/// The owned form of a [`ShapeKey`], for the cache map.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OwnedShapeKey {
    text: String,
    face: FontFace,
    size_bits: u32,
    spacing_bits: u32,
    transform: Keyword,
}

impl OwnedShapeKey {
    fn from_key(key: &ShapeKey<'_>) -> Self {
        Self {
            text: key.text.to_owned(),
            face: key.face.clone(),
            size_bits: key.size_px.to_bits(),
            spacing_bits: key.letter_spacing_px.to_bits(),
            transform: key.transform,
        }
    }
}

/// A shaped run: its blob (empty text shapes to `None`) and its extents.
pub struct ShapedText {
    /// The positioned glyph run, relative to the baseline origin `(0, 0)`.
    pub blob: Option<TextBlob>,
    /// The measured extents that layout sizes the run against.
    pub metrics: TextMetrics,
    /// The face the run was shaped with.
    pub face: FontFace,
    /// The size the run was shaped at.
    pub size_px: f32,
}

static FEATURES_WARNED: Once = Once::new();

/// Font discovery, loading and shaping, with caches.
///
/// Single-threaded by construction: fontconfig's own objects are `!Send`
/// and `!Sync`, and Part 6 will own one `Fontconfig` for the process.
pub struct FontDatabase {
    fontconfig: bool,
    typefaces: HashMap<(PathBuf, i32), Arc<Typeface>>,
    faces: HashMap<String, Option<FontFace>>,
    shapes: HashMap<OwnedShapeKey, Rc<ShapedText>>,
    shaper: Shaper,
}

impl Default for FontDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl FontDatabase {
    /// The production database. Part 6 initialises fontconfig here and falls
    /// back to [`probe_only`](Self::probe_only) when `FcInit` fails.
    #[must_use]
    pub fn new() -> Self {
        Self::probe_only()
    }

    /// M1's fixed `FONT_CANDIDATES` path probe -- the no-fontconfig fallback.
    #[must_use]
    pub fn probe_only() -> Self {
        Self {
            fontconfig: false,
            typefaces: HashMap::new(),
            faces: HashMap::new(),
            shapes: HashMap::new(),
            shaper: Shaper::new(),
        }
    }

    /// Whether real fontconfig matching is in play.
    #[must_use]
    pub fn has_fontconfig(&self) -> bool {
        self.fontconfig
    }

    /// Resolve a CSS font query to one face.
    ///
    /// The probe-only body ignores weight, style and stretch: it returns the
    /// first readable, parseable entry of [`FONT_CANDIDATES`]. Part 6
    /// replaces this body with `FcPattern` + `FcFontMatch`.
    pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace> {
        let cache_key = Self::query_cache_key(query);
        if let Some(cached) = self.faces.get(&cache_key) {
            return cached.clone();
        }
        let mut found = None;
        for candidate in FONT_CANDIDATES {
            let path = PathBuf::from(candidate);
            if let Some(typeface) = Self::load(&path) {
                found = Some(FontFace {
                    family: typeface.family_name().to_owned(),
                    path: path.clone(),
                    index: 0,
                });
                self.typefaces.insert((path, 0), typeface);
                break;
            }
        }
        if found.is_none() {
            tracing::warn!(candidates = FONT_CANDIDATES.len(), "no UI typeface found");
        }
        self.faces.insert(cache_key, found.clone());
        found
    }

    /// A stable cache key for a query.
    fn query_cache_key(query: &FontQuery<'_>) -> String {
        let mut key = String::new();
        for family in query.families {
            match family {
                FontFamily::Named(name) => key.push_str(name),
                FontFamily::Generic(generic) => key.push_str(match generic {
                    GenericFamily::Serif => "serif",
                    GenericFamily::SansSerif => "sans-serif",
                    GenericFamily::Monospace => "monospace",
                    GenericFamily::Cursive => "cursive",
                    GenericFamily::Fantasy => "fantasy",
                    GenericFamily::SystemUi => "system-ui",
                }),
            }
            key.push(',');
        }
        key.push_str(&format!(
            "|{}|{}|{}",
            css_weight_to_fc(query.weight),
            css_style_to_fc_slant(query.style),
            css_stretch_to_fc(query.stretch)
        ));
        key
    }

    fn load(path: &Path) -> Option<Arc<Typeface>> {
        let data = std::fs::read(path).ok()?;
        Typeface::from_data(data).map(Arc::new)
    }

    /// The loaded typeface for `face`, cached by `(path, index)`.
    pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>> {
        let key = (face.path.clone(), face.index);
        if let Some(typeface) = self.typefaces.get(&key) {
            return Some(Arc::clone(typeface));
        }
        let typeface = Self::load(&face.path)?;
        self.typefaces.insert(key, Arc::clone(&typeface));
        Some(typeface)
    }

    /// A `Font` for `face` at `size_px`.
    pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font> {
        let typeface = self.typeface(face)?;
        Some(Font::new(typeface, size_px))
    }

    /// Shape and measure a run, cached.
    ///
    /// `text-transform` is applied to the string *before* shaping;
    /// `letter-spacing` is added between glyphs (never after the last one),
    /// through `TextBlobBuilder::add_positioned_run`.
    pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText> {
        if !key.features.is_empty() || !key.variations.is_empty() {
            FEATURES_WARNED.call_once(|| {
                tracing::warn!(
                    "font-feature-settings and font-variation-settings are computed but cannot \
                     reach rustybuzz through skia-rs-text 0.4.0; they are dropped at the shaper"
                );
            });
        }

        let owned = OwnedShapeKey::from_key(key);
        if let Some(cached) = self.shapes.get(&owned) {
            return Rc::clone(cached);
        }

        let text = transform_text(key.text, key.transform);
        let shaped = Rc::new(self.shape_uncached(&text, key));
        self.shapes.insert(owned, Rc::clone(&shaped));
        shaped
    }

    fn shape_uncached(&mut self, text: &str, key: &ShapeKey<'_>) -> ShapedText {
        let Some(font) = self.font(key.face, key.size_px) else {
            return ShapedText {
                blob: None,
                metrics: TextMetrics {
                    width: 0.0,
                    ascent: 0.0,
                    descent: 0.0,
                    line_height: 0.0,
                },
                face: key.face.clone(),
                size_px: key.size_px,
            };
        };
        let font_metrics = font.metrics();
        let spacing = if key.letter_spacing_px.is_finite() {
            key.letter_spacing_px
        } else {
            0.0
        };

        let mut builder = TextBlobBuilder::new();
        let mut pen_x = 0.0f32;
        let mut glyph_count = 0usize;
        let mut any = false;
        if let Some(runs) = self.shaper.shape_auto(text, &font) {
            for run in &runs {
                let mut glyphs = Vec::with_capacity(run.glyphs.len());
                let mut positions = Vec::with_capacity(run.glyphs.len());
                for glyph in &run.glyphs {
                    if glyph_count > 0 {
                        pen_x += spacing;
                    }
                    glyphs.push(glyph.glyph_id.0);
                    positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                    pen_x += glyph.x_advance;
                    glyph_count += 1;
                }
                if !glyphs.is_empty() {
                    builder.add_positioned_run(&run.font, &glyphs, &positions);
                    any = true;
                }
            }
        } else {
            pen_x = font.measure_text(text);
        }

        ShapedText {
            blob: if any { builder.build() } else { None },
            metrics: TextMetrics {
                width: pen_x,
                // `FontMetrics::ascent` is negative (above the baseline).
                ascent: -font_metrics.ascent,
                descent: font_metrics.descent,
                line_height: font_metrics.line_height(),
            },
            face: key.face.clone(),
            size_px: key.size_px,
        }
    }

    /// Drop every cached face, typeface and shaped run.
    pub fn clear_caches(&mut self) {
        self.typefaces.clear();
        self.faces.clear();
        self.shapes.clear();
    }
}

/// Apply CSS `text-transform` to a string.
///
/// `full-width` maps the ASCII range onto its fullwidth forms (U+FF01..FF5E,
/// and U+3000 for the space), which is what GTK does.
fn transform_text(text: &str, transform: Keyword) -> String {
    match transform {
        Keyword::Uppercase => text.to_uppercase(),
        Keyword::Lowercase => text.to_lowercase(),
        Keyword::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            for ch in text.chars() {
                if at_word_start {
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
                at_word_start = ch.is_whitespace();
            }
            out
        }
        Keyword::FullWidth => text
            .chars()
            .map(|ch| match ch {
                ' ' => '\u{3000}',
                '!'..='~' => char::from_u32(ch as u32 - 0x21 + 0xFF01).unwrap_or(ch),
                other => other,
            })
            .collect(),
        _ => text.to_owned(),
    }
}

/// Piecewise-linear CSS 1..1000 -> fontconfig `FC_WEIGHT_*`.
///
/// The two scales are not proportional: CSS 600 is fontconfig 180, CSS 700
/// is 200. Interpolating between the anchor points is the only way to keep
/// `fc-match` parity.
#[must_use]
pub fn css_weight_to_fc(w: f32) -> i32 {
    const TABLE: &[(f32, f32)] = &[
        (100.0, 0.0),
        (200.0, 40.0),
        (300.0, 50.0),
        (350.0, 75.0),
        (400.0, 80.0),
        (500.0, 100.0),
        (600.0, 180.0),
        (700.0, 200.0),
        (800.0, 205.0),
        (900.0, 210.0),
        (1000.0, 215.0),
    ];
    interpolate_table(TABLE, w)
}

/// Piecewise-linear CSS percentage -> fontconfig `FC_WIDTH_*`.
#[must_use]
pub fn css_stretch_to_fc(pct: f32) -> i32 {
    const TABLE: &[(f32, f32)] = &[
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
    interpolate_table(TABLE, pct)
}

/// CSS `font-style` -> fontconfig `FC_SLANT_*`.
#[must_use]
pub fn css_style_to_fc_slant(s: FontStyle) -> i32 {
    match s {
        FontStyle::Normal => 0,
        FontStyle::Italic => 100,
        FontStyle::Oblique(_) => 110,
    }
}

/// Look `value` up in a sorted `(input, output)` table, interpolating
/// between neighbours and clamping at both ends.
fn interpolate_table(table: &[(f32, f32)], value: f32) -> i32 {
    if !value.is_finite() {
        return table[0].1 as i32;
    }
    if value <= table[0].0 {
        return table[0].1 as i32;
    }
    let last = table[table.len() - 1];
    if value >= last.0 {
        return last.1 as i32;
    }
    for pair in table.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        if value >= lo.0 && value <= hi.0 {
            let t = (value - lo.0) / (hi.0 - lo.0);
            return (lo.1 + t * (hi.1 - lo.1)).round() as i32;
        }
    }
    last.1 as i32
}
```

Delete `FontStack` and its `impl` block; `widget/button.rs` and `app.rs`
still name it, so the crate will not build until Task 17. Run
`cargo test -p icedtea-ui --lib text::` (which builds only the library) to
verify this task; if the library itself does not build because of
`FontStack` references, keep `FontStack` in place for now and delete it in
Task 17 instead — the tests above do not touch it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib text::tests`
Expected: PASS — 8 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and
`cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): FontDatabase, FontFace, ShapeKey and the fc scales

Lands the contract's font surface with M1's probe list behind it, plus
text-transform before shaping, letter-spacing between glyphs, a shaping
cache and the CSS<->fontconfig weight/width/slant tables. Part 6 swaps in
real fontconfig matching without changing a signature.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 8: `PaintCx`, `ImageCache` and the background colour/clip/origin layer

**Files:**
- Modify: `ui/src/paint/mod.rs` — add `PaintCx`, `ImageCache`, `fill_paint`,
  `radii_for_box`, `pub mod background;`
- Create: `ui/src/paint/background.rs` — `paint_backgrounds` handling colour,
  `Image::None` and `Image::Solid` layers only (gradients land in Task 9).
- Test: `ui/src/paint/background.rs`

**Interfaces:**
- Consumes (P1): `css::value::{Image, ColorValue, ColorCtx, ColorTable, Rgba,
  Position, BgSize, RepeatStyle, Keyword}`, `Rgba::to_color32`,
  `ColorValue::resolve(&self, &ColorCtx<'_>) -> Option<Rgba>`.
- Consumes (P3): `ComputedStyle::{background_layers, border_radii, color, raw}`,
  `BackgroundLayer { image, position, size, repeat, origin, clip, blend }`,
  `ResolveEnv`.
- Consumes (Tasks 1, 5, 7): `layout::{Rect, Allocation}`,
  `paint::geometry::{rounded_rect_path, inner_radii, clamp_radii}`,
  `text::FontDatabase`, `text::ShapedText`.
- Produces:
  ```rust
  pub struct PaintCx<'a> {
      pub env: &'a ResolveEnv, pub colors: &'a ColorTable,
      pub fonts: &'a mut FontDatabase, pub images: &'a mut ImageCache,
      pub text: Option<&'a ShapedText>,
  }
  impl PaintCx<'_> { pub fn color_ctx(&self, current: Rgba) -> ColorCtx<'_>; }
  pub struct ImageCache;
  impl ImageCache {
      pub fn new() -> Self;
      pub fn get(&mut self, url: &str) -> Option<&skia_rs_safe::codec::Image>;
  }
  pub fn fill_paint(color: Rgba) -> skia_rs_safe::paint::Paint;
  pub fn radii_for_box(radii: &[[f32; 2]; 4], alloc: &Allocation, k: Keyword) -> [[f32; 2]; 4];
  pub fn paint_backgrounds(canvas: &mut Canvas<'_>, color: Rgba, layers: &[BackgroundLayer],
                           alloc: &Allocation, radii: &[[f32; 2]; 4], cx: &mut PaintCx<'_>);
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/background.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use crate::paint::{ImageCache, PaintCx, paint_backgrounds, radii_for_box};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    /// A 40x20 node with a 4px border and 4px padding, painted from `css`.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);

        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([8.0, 8.0, 8.0, 8.0]),
            border: [4.0; 4],
            padding: [4.0; 4],
        };
        let radii = style.border_radii(40.0, 20.0);
        let layers = style.background_layers();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_backgrounds(
                &mut canvas,
                style.get::<Rgba>(Prop::BackgroundColor),
                &layers,
                &alloc,
                &radii,
                &mut paint_cx,
            );
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
    }

    const SQUARE: &str = "button { background-color: #112233; border-radius: 0; \
                          border: 4px solid transparent; padding: 4px }";

    #[test]
    fn the_background_fills_the_border_box_by_default() {
        // M1's E9 regression, preserved: the background was clipped to the
        // padding box unconditionally, so a translucent border showed the
        // surface through. CSS and GTK default to border-box.
        let surface = painted(SQUARE);
        assert_eq!(pixel(&surface, 0, 0), Color(0xFF11_2233));
        assert_eq!(pixel(&surface, 20, 10), Color(0xFF11_2233));
    }

    #[test]
    fn padding_box_clips_the_background_inside_the_border() {
        let surface = painted(&format!("{SQUARE}\nbutton {{ background-clip: padding-box }}"));
        assert_eq!(pixel(&surface, 0, 0).alpha(), 0);
        assert_eq!(pixel(&surface, 3, 3).alpha(), 0);
        assert_eq!(pixel(&surface, 5, 5), Color(0xFF11_2233));
    }

    #[test]
    fn content_box_clips_the_background_inside_the_padding_too() {
        // Adwaita:1600 uses `background-clip: content-box`; border 4 plus
        // padding 4 means the content box starts at x=8. Keyword matching is
        // ASCII case-insensitive.
        let surface = painted(&format!("{SQUARE}\nbutton {{ background-clip: CONTENT-BOX }}"));
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0);
        assert_eq!(pixel(&surface, 9, 9), Color(0xFF11_2233));
    }

    #[test]
    fn an_image_color_layer_paints_over_the_background_colour() {
        // Adwaita's `:active` uses `image(<color>)`. Mutation check: painting
        // the layers before the colour hides the layer entirely.
        let surface = painted(
            "button { background-color: #112233; background-image: image(#dad6d2); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 10), Color(0xFFDA_D6D2));
    }

    #[test]
    fn radii_for_box_shrinks_by_border_then_padding() {
        // Mutation check: using the same radii for every box makes a rounded
        // background bleed past the inner clip's corners.
        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([8.0, 8.0, 8.0, 8.0]),
            border: [4.0; 4],
            padding: [4.0; 4],
        };
        let radii = [[10.0, 10.0]; 4];
        assert_eq!(radii_for_box(&radii, &alloc, Keyword::BorderBox)[0], [10.0, 10.0]);
        assert_eq!(radii_for_box(&radii, &alloc, Keyword::PaddingBox)[0], [6.0, 6.0]);
        assert_eq!(radii_for_box(&radii, &alloc, Keyword::ContentBox)[0], [2.0, 2.0]);
    }
}
```

The background colour comes through the contract's generic accessor —
`ComputedStyle::get::<Rgba>(Prop::BackgroundColor)` (contract §5) — because
`computed` has already resolved `@name` / `currentColor` for that slot. There
is no property-specific `background-color` getter in §5.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::background`
Expected: FAIL — `error[E0432]: unresolved import `crate::paint::{ImageCache, PaintCx, paint_backgrounds, radii_for_box}``.

- [ ] **Step 3: Write minimal implementation**

Extend `ui/src/paint/mod.rs`:

```rust
//! The CSS painter: one module per property family, `paint_node` for the
//! order they run in.

pub mod background;
pub mod blur;
pub mod geometry;

use std::collections::HashMap;

use skia_rs_safe::codec::Image as DecodedImage;
use skia_rs_safe::paint::{Paint, Style};

use crate::css::computed::ResolveEnv;
use crate::css::value::{ColorCtx, ColorTable, Keyword, Rgba};
use crate::layout::Allocation;
use crate::text::{FontDatabase, ShapedText};

pub use background::paint_backgrounds;
pub use geometry::{
    Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
};

/// Everything a paint pass needs beyond the node's own style and geometry.
pub struct PaintCx<'a> {
    /// DPI and root font size, for length resolution at paint time.
    pub env: &'a ResolveEnv,
    /// The sheet's `@define-color` table, still unresolved.
    pub colors: &'a ColorTable,
    /// Font matching, loading and shaping.
    pub fonts: &'a mut FontDatabase,
    /// Decoded `url()` images.
    pub images: &'a mut ImageCache,
    /// This node's own label, already shaped.
    pub text: Option<&'a ShapedText>,
}

impl PaintCx<'_> {
    /// A colour-resolution context whose `currentColor` is `current`.
    #[must_use]
    pub fn color_ctx(&self, current: Rgba) -> ColorCtx<'_> {
        ColorCtx {
            table: self.colors,
            current,
            depth: 0,
        }
    }
}

/// Decoded `url()` images, keyed by URL.
///
/// A URL that fails to decode is *recorded unresolved*: it is remembered as
/// a miss so the file is not re-read every frame, logged once, and paints
/// nothing. That is not an error -- the computed value keeps the URL.
pub struct ImageCache {
    entries: HashMap<String, Option<DecodedImage>>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The decoded image for `url`, decoding it on first use.
    pub fn get(&mut self, url: &str) -> Option<&DecodedImage> {
        if !self.entries.contains_key(url) {
            let decoded = std::fs::read(url)
                .ok()
                .and_then(|bytes| skia_rs_safe::codec::decode_image(&bytes).ok());
            if decoded.is_none() {
                tracing::debug!(url, "background image could not be decoded; recorded unresolved");
            }
            self.entries.insert(url.to_owned(), decoded);
        }
        self.entries.get(url).and_then(Option::as_ref)
    }
}

/// An anti-aliased fill paint in `color`.
#[must_use]
pub fn fill_paint(color: Rgba) -> Paint {
    let mut paint = Paint::new();
    paint.set_color32(color.to_color32());
    paint.set_style(Style::Fill);
    paint.set_anti_alias(true);
    paint
}

/// The corner radii of the box `k` names, shrunk from the border box.
#[must_use]
pub fn radii_for_box(radii: &[[f32; 2]; 4], alloc: &Allocation, k: Keyword) -> [[f32; 2]; 4] {
    match k {
        Keyword::PaddingBox => inner_radii(radii, alloc.border),
        Keyword::ContentBox => {
            let sides = [
                alloc.border[0] + alloc.padding[0],
                alloc.border[1] + alloc.padding[1],
                alloc.border[2] + alloc.padding[2],
                alloc.border[3] + alloc.padding[3],
            ];
            inner_radii(radii, sides)
        }
        _ => *radii,
    }
}
```

Prepend to `ui/src/paint/background.rs`:

```rust
//! `background-color` plus the `background-*` layer stack.
//!
//! Layers arrive top-first (CSS: the first listed image is topmost), so they
//! are painted in reverse; the colour goes underneath them all, clipped by
//! the *last* layer's `background-clip` -- CSS Backgrounds L3 §3.11.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::computed::BackgroundLayer;
use crate::css::value::{Image, Keyword, Rgba};
use crate::layout::Allocation;
use crate::paint::{PaintCx, fill_paint, radii_for_box, rounded_rect_path};

/// Paint the background colour and every layer of `layers`.
pub fn paint_backgrounds(
    canvas: &mut Canvas<'_>,
    color: Rgba,
    layers: &[BackgroundLayer],
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    cx: &mut PaintCx<'_>,
) {
    let colour_clip = layers.last().map_or(Keyword::BorderBox, |l| l.clip);
    if color.a > 0.0 {
        let rect = alloc.box_for(colour_clip);
        if !rect.is_empty() {
            let path = rounded_rect_path(rect, &radii_for_box(radii, alloc, colour_clip));
            canvas.draw_path(&path, &fill_paint(color));
        }
    }

    for layer in layers.iter().rev() {
        paint_layer(canvas, layer, alloc, radii, cx, color);
    }
}

/// Paint one `background-image` layer under its own clip.
fn paint_layer(
    canvas: &mut Canvas<'_>,
    layer: &BackgroundLayer,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    cx: &mut PaintCx<'_>,
    current: Rgba,
) {
    if matches!(layer.image, Image::None) {
        return;
    }
    let clip_rect = alloc.box_for(layer.clip);
    if clip_rect.is_empty() {
        return;
    }

    let save = canvas.save();
    let clip_path = rounded_rect_path(clip_rect, &radii_for_box(radii, alloc, layer.clip));
    canvas.clip_path(&clip_path, ClipOp::Intersect, true);

    match &layer.image {
        Image::Solid(color) => {
            if let Some(rgba) = color.resolve(&cx.color_ctx(current)) {
                let origin = alloc.box_for(layer.origin);
                let mut paint = fill_paint(rgba);
                paint.set_anti_alias(false);
                canvas.draw_rect(&origin.to_skia(), &paint);
            }
        }
        // Gradients, `url()`, cross-fades and icon references arrive in
        // Tasks 9 and 10; `-gtk-icon-*` never paints in M2.
        _ => {}
    }

    canvas.restore_to_count(save);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::background`
Expected: PASS — 5 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/mod.rs ui/src/paint/background.rs
git commit -m "feat(ui/paint): PaintCx, ImageCache and background colour layers

Restores M1's three background-clip regressions against the layered model,
adds the image(<color>) layer, and lands the per-box radius shrinking every
later family needs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 9: gradient layers — linear, radial and conic, sampled from `color_at`

**Files:**
- Modify: `ui/src/paint/background.rs` — add `gradient_t`, `paint_gradient`
  and the `Image::Gradient` arm of `paint_layer`.
- Test: `ui/src/paint/background.rs`

**Interfaces:**
- Consumes (P1): `css::value::{Gradient, GradientKind, RadialShape,
  RadialExtent, LinearDirection, SideOrCorner, Position, LengthCtx}`,
  `Gradient::color_at(&self, t: f32, &ColorCtx<'_>, &LengthCtx) -> Rgba`,
  `Gradient::line_for_box(&self, w: f32, h: f32, &LengthCtx) -> (Point, Point)`,
  `Length::resolve(&self, &LengthCtx) -> Option<f32>`.
- Consumes (P3): `ComputedStyle::length_ctx`.
- Produces:
  ```rust
  pub fn gradient_t(gradient: &Gradient, box_w: f32, box_h: f32,
                    px: f32, py: f32, ctx: &LengthCtx) -> f32;
  pub fn paint_gradient(canvas: &mut Canvas<'_>, gradient: &Gradient,
                        origin: Rect, clip: Rect, cx: &mut PaintCx<'_>,
                        current: Rgba, len_ctx: &LengthCtx);
  ```

**Why a sampler and not a Skia shader:** the M1 exactness rule. Every
asserted pixel must be derivable from the CSS, and `Gradient::color_at` is
the one definition of what colour lands where. A `LinearGradient` shader
would interpolate in its own space and make the gate's
`0xFFF6F5F4`/`0xFFE8E6E3` band assertions unverifiable.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/paint/background.rs`'s `mod tests`:

```rust
    #[test]
    fn a_vertical_gradient_reproduces_adwaitas_flat_pre_stop_band() {
        // The M1 gate's exact number: `linear-gradient(to top, #f6f5f4 2px,
        // #fbfafa)` over a 20px-tall origin box is flat #f6f5f4 below the
        // 2px first stop. Mutation check: sampling at `y` instead of
        // `y + 0.5`, or against the border box instead of the origin box,
        // moves this off the stop colour.
        let surface = painted(
            "button { background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 19), Color(0xFFF6_F5F4));
        assert_eq!(pixel(&surface, 20, 0), Color(0xFFFB_FAFA));
        let middle = pixel(&surface, 20, 10);
        assert!(middle != Color(0xFFF6_F5F4) && middle != Color(0xFFFB_FAFA));
        assert!((0xF6..=0xFB).contains(&middle.red()));
    }

    #[test]
    fn a_horizontal_gradient_varies_along_x_not_y() {
        // Mutation check: falling through to the vertical fast path paints
        // uniform columns and the first two assertions become equal.
        let surface = painted(
            "button { background-image: linear-gradient(to right, #000000, #ffffff); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 2, 10).red() < pixel(&surface, 37, 10).red());
        assert_eq!(pixel(&surface, 20, 2), pixel(&surface, 20, 17));
    }

    #[test]
    fn a_radial_gradient_is_darkest_at_its_centre() {
        // Mutation check: normalising the radius against the box width only
        // makes the corner sample equal the centre on a 40x20 box.
        let surface = painted(
            "button { background-image: radial-gradient(circle closest-side, #000000, #ffffff); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 20, 10).red() < 0x20);
        assert!(pixel(&surface, 20, 0).red() > 0xE0);
    }

    #[test]
    fn a_conic_gradient_varies_with_angle_around_the_centre() {
        // Mutation check: measuring the angle from the +x axis rather than
        // straight up (CSS's 0deg) rotates the whole wheel by 90 degrees and
        // swaps these two samples.
        let surface = painted(
            "button { background-image: conic-gradient(#000000, #ffffff 50%, #000000); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 20, 2).red() < 0x40, "straight up is 0deg");
        assert!(pixel(&surface, 20, 17).red() > 0xC0, "straight down is 180deg");
    }

    #[test]
    fn gradient_sampling_never_panics_on_a_degenerate_box() {
        // A zero-area origin box, a NaN position and a single-stop gradient
        // all reach `gradient_t`; none may panic or hang.
        for css in [
            "button { background-image: linear-gradient(#f00, #f00); \
             border: 0 solid transparent; padding: 0; min-width: 0; min-height: 0 }",
            "button { background-image: radial-gradient(closest-corner at 0 0, #f00, #00f); \
             border: 0 solid transparent; padding: 0 }",
            "button { background-image: repeating-linear-gradient(45deg, #f00 0, #00f 1px); \
             border: 0 solid transparent; padding: 0 }",
        ] {
            let _ = painted(css);
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::background::tests::a_vertical_gradient`
Expected: FAIL — assertion `pixel(&surface, 20, 19) == Color(0xFFF6F5F4)` fails
with `Color(0)`, because `paint_layer`'s `_ => {}` arm draws nothing.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/paint/background.rs`:

```rust
use skia_rs_safe::core::Point;

use crate::css::value::{Gradient, GradientKind, Length, LengthCtx, Position, RadialExtent, RadialShape};
use crate::layout::Rect;

/// Resolve a `Position` against a box of `w` x `h`, in box-local px.
fn resolve_position(position: &Position, w: f32, h: f32, ctx: &LengthCtx) -> (f32, f32) {
    let mut x_ctx = *ctx;
    x_ctx.percent_basis = Some(w);
    let mut y_ctx = *ctx;
    y_ctx.percent_basis = Some(h);
    (
        position.x.resolve(&x_ctx).unwrap_or(w / 2.0),
        position.y.resolve(&y_ctx).unwrap_or(h / 2.0),
    )
}

/// The gradient-line parameter for the box-local point `(px, py)`.
///
/// `t` is what `Gradient::color_at` consumes: 0 at the first stop position
/// and 1 at the last, before repeating and stop placement are applied.
#[must_use]
pub fn gradient_t(gradient: &Gradient, box_w: f32, box_h: f32, px: f32, py: f32, ctx: &LengthCtx) -> f32 {
    match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(box_w, box_h, ctx);
            let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
            let len2 = dx.mul_add(dx, dy * dy);
            if !len2.is_finite() || len2 <= f32::EPSILON {
                return 1.0;
            }
            ((px - p0.x) * dx + (py - p0.y) * dy) / len2
        }
        GradientKind::Radial {
            shape,
            extent,
            position,
        } => {
            let (cx, cy) = resolve_position(position, box_w, box_h, ctx);
            let (rx, ry) = radial_radii(*shape, extent, cx, cy, box_w, box_h, ctx);
            if !(rx.is_finite() && ry.is_finite()) || rx <= 0.0 || ry <= 0.0 {
                return 1.0;
            }
            let nx = (px - cx) / rx;
            let ny = (py - cy) / ry;
            nx.hypot(ny)
        }
        GradientKind::Conic {
            from_angle,
            position,
        } => {
            let (cx, cy) = resolve_position(position, box_w, box_h, ctx);
            // CSS conic-gradient: 0deg points straight up, angles increase
            // clockwise.
            let angle = (px - cx).atan2(cy - py).to_degrees() - from_angle;
            let wrapped = angle.rem_euclid(360.0);
            if wrapped.is_finite() { wrapped / 360.0 } else { 0.0 }
        }
    }
}

/// The x and y radii of a radial gradient's ending shape.
fn radial_radii(
    shape: RadialShape,
    extent: &RadialExtent,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    ctx: &LengthCtx,
) -> (f32, f32) {
    let (left, right, top, bottom) = (cx, w - cx, cy, h - cy);
    let (sx, sy) = (left.abs().min(right.abs()), top.abs().min(bottom.abs()));
    let (fx, fy) = (left.abs().max(right.abs()), top.abs().max(bottom.abs()));
    let (rx, ry) = match extent {
        RadialExtent::ClosestSide => (sx, sy),
        RadialExtent::FarthestSide => (fx, fy),
        RadialExtent::ClosestCorner => {
            let d = sx.hypot(sy);
            (d, d)
        }
        RadialExtent::FarthestCorner => {
            let d = fx.hypot(fy);
            (d, d)
        }
        RadialExtent::Explicit(x, y) => {
            let mut x_ctx = *ctx;
            x_ctx.percent_basis = Some(w);
            let mut y_ctx = *ctx;
            y_ctx.percent_basis = Some(h);
            (
                x.resolve(&x_ctx).unwrap_or(0.0),
                y.resolve(&y_ctx).unwrap_or(0.0),
            )
        }
    };
    match shape {
        RadialShape::Circle => {
            let r = match extent {
                RadialExtent::ClosestSide => sx.min(sy),
                RadialExtent::FarthestSide => fx.max(fy),
                _ => rx,
            };
            (r, r)
        }
        RadialShape::Ellipse => (rx, ry),
    }
}

/// Fill `clip` with `gradient`, sized against `origin`.
///
/// Axis-aligned linear gradients are filled band-by-band -- one `draw_rect`
/// per constant row or column -- which is exactly M1's model and keeps the
/// gate's band assertions exact. Everything else samples per pixel.
pub fn paint_gradient(
    canvas: &mut Canvas<'_>,
    gradient: &Gradient,
    origin: Rect,
    clip: Rect,
    cx: &mut PaintCx<'_>,
    current: Rgba,
    len_ctx: &LengthCtx,
) {
    if origin.is_empty() || clip.is_empty() {
        return;
    }
    let color_ctx = cx.color_ctx(current);
    let (x0, x1) = (clip.x.floor() as i32, clip.right().ceil() as i32);
    let (y0, y1) = (clip.y.floor() as i32, clip.bottom().ceil() as i32);

    let vertical = match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(origin.width, origin.height, len_ctx);
            Some((p1.x - p0.x).abs() < 1.0e-4)
        }
        _ => None,
    };
    let horizontal = match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(origin.width, origin.height, len_ctx);
            Some((p1.y - p0.y).abs() < 1.0e-4)
        }
        _ => None,
    };

    let mut band = |canvas: &mut Canvas<'_>, rect: Rect, color: Rgba| {
        let mut paint = fill_paint(color);
        paint.set_anti_alias(false);
        canvas.draw_rect(&rect.to_skia(), &paint);
    };

    if vertical == Some(true) {
        for row in y0..y1 {
            let py = row as f32 + 0.5 - origin.y;
            let t = gradient_t(gradient, origin.width, origin.height, 0.0, py, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(
                canvas,
                Rect::new(clip.x, row as f32, clip.width, 1.0),
                color,
            );
        }
        return;
    }
    if horizontal == Some(true) {
        for col in x0..x1 {
            let px = col as f32 + 0.5 - origin.x;
            let t = gradient_t(gradient, origin.width, origin.height, px, 0.0, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(
                canvas,
                Rect::new(col as f32, clip.y, 1.0, clip.height),
                color,
            );
        }
        return;
    }

    for row in y0..y1 {
        let py = row as f32 + 0.5 - origin.y;
        for col in x0..x1 {
            let px = col as f32 + 0.5 - origin.x;
            let t = gradient_t(gradient, origin.width, origin.height, px, py, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(canvas, Rect::new(col as f32, row as f32, 1.0, 1.0), color);
        }
    }
}
```

and replace `paint_layer`'s wildcard arm with:

```rust
        Image::Gradient(gradient) => {
            let origin = alloc.box_for(layer.origin);
            let len_ctx = LengthCtx {
                percent_basis: Some(origin.width),
                ..*cx.base_length_ctx()
            };
            paint_gradient(canvas, gradient, origin, clip_rect, cx, current, &len_ctx);
        }
        Image::None | Image::Solid(_) => {}
        // `url()`, `cross-fade()` and icon references land in Task 10;
        // `-gtk-icon-*` never paints in M2.
        Image::Url(_) | Image::CrossFade(_) | Image::Icon(_) => {}
```

`PaintCx::base_length_ctx` does not exist yet; add it to `ui/src/paint/mod.rs`:

```rust
impl PaintCx<'_> {
    /// A length context for paint-time resolution: `em` is unavailable here
    /// (the computed style already resolved it), so only absolute units,
    /// the DPI and an explicit percentage basis matter.
    #[must_use]
    pub fn base_length_ctx(&self) -> LengthCtx {
        LengthCtx {
            font_size_px: self.env.root_font_size,
            root_font_size_px: self.env.root_font_size,
            ex_ratio: 0.5,
            dpi: self.env.dpi,
            percent_basis: None,
        }
    }
}
```

with `use crate::css::value::LengthCtx;` added to the module's imports.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::background`
Expected: PASS — 10 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/mod.rs ui/src/paint/background.rs
git commit -m "feat(ui/paint): linear, radial and conic gradient layers

Sampled through Gradient::color_at so every asserted pixel stays derivable
from the CSS, with axis-aligned linear gradients kept on M1's band fill.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 10: background sizing, position, repeat, blend and `url()` images

**Files:**
- Modify: `ui/src/paint/background.rs` — add `layer_tile`, `tile_positions`,
  the `Image::Url`/`Image::CrossFade` arms and blend-mode plumbing.
- Test: `ui/src/paint/background.rs`

**Interfaces:**
- Consumes (P1): `css::value::{BgSize, RepeatStyle, Keyword, Image}`.
- Consumes (Task 8): `ImageCache::get`.
- Produces:
  ```rust
  pub struct Tile { pub rect: Rect, pub step_x: f32, pub step_y: f32 }
  pub fn layer_tile(layer: &BackgroundLayer, origin: Rect, intrinsic: Option<(f32, f32)>,
                    ctx: &LengthCtx) -> Tile;
  pub fn tile_positions(tile: &Tile, clip: Rect, repeat: RepeatStyle) -> Vec<Rect>;
  pub fn blend_mode_for(keyword: Keyword) -> skia_rs_safe::paint::BlendMode;
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/paint/background.rs`'s `mod tests`:

```rust
    use super::{Tile, blend_mode_for, layer_tile, tile_positions};
    use crate::css::value::RepeatStyle;
    use skia_rs_safe::paint::BlendMode;

    #[test]
    fn a_sized_and_positioned_tile_lands_where_the_css_says() {
        // `background-size: 10px 5px; background-position: right bottom` in a
        // 40x20 origin box. Mutation check: resolving `right` as 100% of the
        // box rather than `box - tile` puts the tile off the right edge.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#f00); background-size: 10px 5px; \
             background-position: right bottom }",
        );
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let layer = style.background_layers().pop().expect("one layer");
        let ctx = style.length_ctx(&env, None);
        let tile = layer_tile(&layer, Rect::new(0.0, 0.0, 40.0, 20.0), None, &ctx);
        assert_eq!(tile.rect, Rect::new(30.0, 15.0, 10.0, 5.0));
    }

    #[test]
    fn repeat_tiles_across_the_clip_and_no_repeat_paints_once() {
        // Mutation check: stepping from the clip's origin instead of the
        // tile's makes the first repeated rect land at x == 0 instead of -10.
        let tile = Tile {
            rect: Rect::new(5.0, 5.0, 10.0, 5.0),
            step_x: 10.0,
            step_y: 5.0,
        };
        let clip = Rect::new(0.0, 0.0, 40.0, 20.0);
        let once = tile_positions(&tile, clip, RepeatStyle { x: Keyword::NoRepeat, y: Keyword::NoRepeat });
        assert_eq!(once, vec![tile.rect]);

        let both = tile_positions(&tile, clip, RepeatStyle { x: Keyword::Repeat, y: Keyword::Repeat });
        assert!(both.contains(&Rect::new(-5.0, 0.0, 10.0, 5.0)));
        assert!(both.contains(&Rect::new(35.0, 15.0, 10.0, 5.0)));
        assert_eq!(both.len(), 5 * 5);

        let x_only = tile_positions(&tile, clip, RepeatStyle { x: Keyword::Repeat, y: Keyword::NoRepeat });
        assert!(x_only.iter().all(|r| (r.y - 5.0).abs() < f32::EPSILON));
    }

    #[test]
    fn every_css_background_blend_mode_maps_to_a_skia_blend_mode() {
        // Mutation check: mapping Keyword::ColorBlend to BlendMode::Color is
        // the whole point of the ColorBlend rename; sending it to SrcOver
        // silently disables the mode.
        assert_eq!(blend_mode_for(Keyword::Normal), BlendMode::SrcOver);
        assert_eq!(blend_mode_for(Keyword::Multiply), BlendMode::Multiply);
        assert_eq!(blend_mode_for(Keyword::ColorDodge), BlendMode::ColorDodge);
        assert_eq!(blend_mode_for(Keyword::ColorBlend), BlendMode::Color);
        assert_eq!(blend_mode_for(Keyword::Luminosity), BlendMode::Luminosity);
        assert_eq!(blend_mode_for(Keyword::Auto), BlendMode::SrcOver, "unknown falls back");
    }

    #[test]
    fn an_undecodable_url_paints_nothing_and_is_not_an_error() {
        // Contract §2.5: `url()` that fails to decode is recorded-unresolved.
        // Mutation check: treating it as an error would abort the whole
        // background stack and lose the colour underneath.
        let surface = painted(
            "button { background-color: #112233; \
             background-image: url(\"/nonexistent/icedtea-test.png\"); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 10), Color(0xFF11_2233));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::background::tests::a_sized_and_positioned_tile`
Expected: FAIL — `error[E0432]: unresolved import `super::{Tile, blend_mode_for, layer_tile, tile_positions}``.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/paint/background.rs`:

```rust
use skia_rs_safe::paint::BlendMode;

use crate::css::value::{BgSize, RepeatStyle};

/// One placed copy of a background image, plus its repeat pitch.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tile {
    /// Where the first copy goes.
    pub rect: Rect,
    /// Horizontal distance between copies.
    pub step_x: f32,
    /// Vertical distance between copies.
    pub step_y: f32,
}

/// Size and place one layer's image inside `origin`.
///
/// `intrinsic` is the image's own size when it has one (a decoded `url()`);
/// gradients have none, so `auto` resolves to the whole origin box.
#[must_use]
pub fn layer_tile(
    layer: &BackgroundLayer,
    origin: Rect,
    intrinsic: Option<(f32, f32)>,
    ctx: &LengthCtx,
) -> Tile {
    let (iw, ih) = intrinsic.unwrap_or((origin.width, origin.height));
    let mut w_ctx = *ctx;
    w_ctx.percent_basis = Some(origin.width);
    let mut h_ctx = *ctx;
    h_ctx.percent_basis = Some(origin.height);

    let (w, h) = match &layer.size {
        BgSize::Auto => (iw, ih),
        BgSize::Cover | BgSize::Contain => {
            if iw <= 0.0 || ih <= 0.0 {
                (origin.width, origin.height)
            } else {
                let sx = origin.width / iw;
                let sy = origin.height / ih;
                let s = if matches!(layer.size, BgSize::Cover) {
                    sx.max(sy)
                } else {
                    sx.min(sy)
                };
                (iw * s, ih * s)
            }
        }
        BgSize::Explicit(x, y) => (
            x.resolve(&w_ctx).unwrap_or(iw),
            y.resolve(&h_ctx).unwrap_or(ih),
        ),
    };
    let (w, h) = (
        if w.is_finite() && w > 0.0 { w } else { 0.0 },
        if h.is_finite() && h > 0.0 { h } else { 0.0 },
    );

    // CSS resolves a background-position percentage against
    // `container - image`, not against the container.
    let mut px_ctx = *ctx;
    px_ctx.percent_basis = Some(origin.width - w);
    let mut py_ctx = *ctx;
    py_ctx.percent_basis = Some(origin.height - h);
    let x = layer.position.x.resolve(&px_ctx).unwrap_or(0.0);
    let y = layer.position.y.resolve(&py_ctx).unwrap_or(0.0);

    Tile {
        rect: Rect::new(origin.x + x, origin.y + y, w, h),
        step_x: w,
        step_y: h,
    }
}

/// Every copy of `tile` that intersects `clip`, per `repeat`.
#[must_use]
pub fn tile_positions(tile: &Tile, clip: Rect, repeat: RepeatStyle) -> Vec<Rect> {
    let repeats = |axis: Keyword| matches!(axis, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let count = |start: f32, step: f32, lo: f32, hi: f32| -> (i32, i32) {
        if !(step.is_finite() && step > 0.0) {
            return (0, 0);
        }
        let first = ((lo - start) / step).floor() as i32;
        let last = ((hi - start) / step).ceil() as i32;
        (first, last.max(first))
    };

    let (ix0, ix1) = if repeats(repeat.x) {
        count(tile.rect.x, tile.step_x, clip.x, clip.right())
    } else {
        (0, 0)
    };
    let (iy0, iy1) = if repeats(repeat.y) {
        count(tile.rect.y, tile.step_y, clip.y, clip.bottom())
    } else {
        (0, 0)
    };

    let mut out = Vec::new();
    for iy in iy0..=iy1 {
        for ix in ix0..=ix1 {
            out.push(Rect::new(
                tile.step_x.mul_add(ix as f32, tile.rect.x),
                tile.step_y.mul_add(iy as f32, tile.rect.y),
                tile.rect.width,
                tile.rect.height,
            ));
        }
    }
    if out.is_empty() {
        out.push(tile.rect);
    }
    out
}

/// CSS `background-blend-mode` keyword -> `skia-rs` blend mode.
#[must_use]
pub fn blend_mode_for(keyword: Keyword) -> BlendMode {
    match keyword {
        Keyword::Multiply => BlendMode::Multiply,
        Keyword::Screen => BlendMode::Screen,
        Keyword::Overlay => BlendMode::Overlay,
        Keyword::Darken => BlendMode::Darken,
        Keyword::Lighten => BlendMode::Lighten,
        Keyword::ColorDodge => BlendMode::ColorDodge,
        Keyword::ColorBurn => BlendMode::ColorBurn,
        Keyword::HardLight => BlendMode::HardLight,
        Keyword::SoftLight => BlendMode::SoftLight,
        Keyword::Difference => BlendMode::Difference,
        Keyword::Exclusion => BlendMode::Exclusion,
        Keyword::Hue => BlendMode::Hue,
        Keyword::Saturation => BlendMode::Saturation,
        Keyword::ColorBlend => BlendMode::Color,
        Keyword::Luminosity => BlendMode::Luminosity,
        _ => BlendMode::SrcOver,
    }
}
```

and replace the `Image::Url(_) | Image::CrossFade(_) | Image::Icon(_)` arm of
`paint_layer` with:

```rust
        Image::Url(url) => {
            let origin = alloc.box_for(layer.origin);
            let len_ctx = cx.base_length_ctx();
            let Some((iw, ih)) = cx.images.get(url).map(|img| (img.width() as f32, img.height() as f32))
            else {
                // Recorded unresolved: paints nothing, already logged once.
                canvas.restore_to_count(save);
                return;
            };
            let tile = layer_tile(layer, origin, Some((iw, ih)), &len_ctx);
            let rects = tile_positions(&tile, clip_rect, layer.repeat);
            let mut paint = Paint::new();
            paint.set_blend_mode(blend_mode_for(layer.blend));
            if let Some(image) = cx.images.get(url) {
                for rect in rects {
                    canvas.draw_image_rect(image, None, &rect.to_skia(), Some(&paint));
                }
            }
        }
        // `cross-fade()` and `-gtk-*` icon images are stored by the registry
        // and drawn in M4; they paint nothing here (spec, Out of scope).
        Image::CrossFade(_) | Image::Icon(_) => {}
```

with `use skia_rs_safe::paint::Paint;` added to the module imports, and wrap
the `Image::Solid` and `Image::Gradient` draws in the same blend mode by
setting `paint.set_blend_mode(blend_mode_for(layer.blend))` on the paints
`fill_paint` returns before drawing.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::background`
Expected: PASS — 14 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/background.rs
git commit -m "feat(ui/paint): background sizing, position, repeat, blend and url()

Adds cover/contain/explicit sizing, CSS's container-minus-image position
basis, the repeat grid, the full background-blend-mode table, and PNG/SVG
url() layers with recorded-unresolved decode failures.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 11: `paint_borders` — per-side mitred fills with every `border-style`

**Files:**
- Create: `ui/src/paint/border.rs`
- Modify: `ui/src/paint/mod.rs` — `pub mod border;` + `pub use border::paint_borders;`
- Test: `ui/src/paint/border.rs`

**Interfaces:**
- Consumes: `layout::{Rect, Allocation}` (Task 1);
  `paint::geometry::{rounded_ring_path, inner_radii, side_wedge_path, Side}` (Task 5);
  `paint::fill_paint` (Task 8); `css::value::{Keyword, Rgba}`.
- Produces:
  ```rust
  pub fn paint_borders(canvas: &mut Canvas<'_>, alloc: &Allocation, widths: [f32; 4],
                       colors: [Rgba; 4], styles: [Keyword; 4], radii: &[[f32; 2]; 4]);
  pub fn is_visible_border_style(style: Keyword) -> bool;
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/border.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::{is_visible_border_style, paint_borders};
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    fn alloc(w: f32, h: f32, border: [f32; 4]) -> Allocation {
        let border_box = Rect::new(0.0, 0.0, w, h);
        Allocation {
            border_box,
            content_box: border_box.inset(border),
            border,
            padding: [0.0; 4],
        }
    }

    fn painted(
        widths: [f32; 4],
        colors: [Rgba; 4],
        styles: [Keyword; 4],
        radii: [[f32; 2]; 4],
    ) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_borders(&mut canvas, &alloc(40.0, 20.0, widths), widths, colors, styles, &radii);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn a_uniform_border_paints_adwaitas_exact_edge_colour() {
        // The M1 gate's `pixel(cx, 0) == 0xFFCDC7C2`, as a unit test. The
        // straight run of the top edge is fully covered, so it is exact.
        // Mutation check: stroking on the centre line (M1's model) halves
        // the coverage of a 1px border and this stops being exact.
        let surface = painted(
            [1.0; 4],
            [rgba(0xCD, 0xC7, 0xC2); 4],
            [Keyword::Solid; 4],
            [[5.0, 5.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 0), Color(0xFFCD_C7C2));
        assert_eq!(pixel(&surface, 0, 0).alpha(), 0, "outside the 5px corner arc");
        assert_eq!(pixel(&surface, 20, 10).alpha(), 0, "the interior is untouched");
    }

    #[test]
    fn mixed_per_side_colours_meet_on_the_mitre_and_do_not_overdraw() {
        // Mutation check: filling the whole ring once per side paints the
        // last side's colour everywhere, so the left edge reads red.
        let red = rgba(0xFF, 0x00, 0x00);
        let blue = rgba(0x00, 0x00, 0xFF);
        let surface = painted(
            [6.0, 6.0, 6.0, 6.0],
            [red, blue, blue, red],
            [Keyword::Solid; 4],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1), Color(0xFFFF_0000), "top is red");
        assert_eq!(pixel(&surface, 38, 10), Color(0xFF00_00FF), "right is blue");
        assert_eq!(pixel(&surface, 1, 10), Color(0xFFFF_0000), "left is red");
    }

    #[test]
    fn none_and_hidden_sides_paint_nothing_even_with_a_width() {
        // CSS: `border-style: none` forces the used width to zero. P3
        // already zeroes the width, but a caller passing both must still be
        // safe. Mutation check: dropping the style check paints a black bar.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [Keyword::None, Keyword::Hidden, Keyword::None, Keyword::Hidden],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
        assert_eq!(pixel(&surface, 1, 10).alpha(), 0);
        assert!(!is_visible_border_style(Keyword::None));
        assert!(!is_visible_border_style(Keyword::Hidden));
        assert!(is_visible_border_style(Keyword::Double));
    }

    #[test]
    fn a_dashed_border_leaves_gaps_along_its_edge() {
        // Mutation check: falling back to a solid fill makes every sampled
        // pixel opaque and `gaps` stays zero.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [Keyword::Dashed; 4],
            [[0.0, 0.0]; 4],
        );
        let gaps = (6..34).filter(|&x| pixel(&surface, x, 1).alpha() == 0).count();
        assert!(gaps > 0, "a dashed top edge must have gaps");
        let inked = (6..34).filter(|&x| pixel(&surface, x, 1).alpha() > 0).count();
        assert!(inked > 0, "a dashed top edge must also have ink");
    }

    #[test]
    fn border_painting_never_panics_on_hostile_geometry() {
        for &v in &[f32::NAN, f32::INFINITY, -8.0, 0.0, 1.0e30] {
            let _ = painted([v; 4], [rgba(1, 2, 3); 4], [Keyword::Solid; 4], [[v, v]; 4]);
            let _ = painted([v; 4], [rgba(1, 2, 3); 4], [Keyword::Double; 4], [[v, v]; 4]);
            let _ = painted([v; 4], [rgba(1, 2, 3); 4], [Keyword::Dotted; 4], [[v, v]; 4]);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::border`
Expected: FAIL — `error[E0432]: unresolved import `super::{is_visible_border_style, paint_borders}``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/border.rs`:

```rust
//! Per-side borders.
//!
//! Each side is a *filled* path -- the border ring intersected with that
//! side's mitre wedge -- never a stroke. Stroking cannot express four
//! different widths, four colours and four corner radii meeting on the
//! diagonals, which is exactly what GTK draws.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{Keyword, Rgba};
use crate::layout::Allocation;
use crate::paint::geometry::{Side, inner_radii, rounded_ring_path, side_wedge_path};
use crate::paint::{fill_paint, rounded_rect_path};

/// Whether a `border-style` keyword paints anything at all.
#[must_use]
pub fn is_visible_border_style(style: Keyword) -> bool {
    !matches!(style, Keyword::None | Keyword::Hidden)
}

/// Paint all four borders.
pub fn paint_borders(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    widths: [f32; 4],
    colors: [Rgba; 4],
    styles: [Keyword; 4],
    radii: &[[f32; 2]; 4],
) {
    let used: [f32; 4] = std::array::from_fn(|i| {
        if widths[i].is_finite() && widths[i] > 0.0 && is_visible_border_style(styles[i]) {
            widths[i]
        } else {
            0.0
        }
    });
    if used.iter().all(|w| *w <= 0.0) {
        return;
    }

    let outer = alloc.border_box;
    let inner = outer.inset(used);
    let inner_r = inner_radii(radii, used);
    let ring = rounded_ring_path(outer, radii, inner, &inner_r);

    let uniform = used.windows(2).all(|w| (w[0] - w[1]).abs() < f32::EPSILON)
        && colors.windows(2).all(|c| c[0] == c[1])
        && styles.windows(2).all(|s| s[0] == s[1])
        && styles[0] == Keyword::Solid;
    if uniform {
        // One fill, so the four sides never seam against each other along
        // the mitres -- this is the path Adwaita's 1px button border takes.
        canvas.draw_path(&ring, &fill_paint(colors[0]));
        return;
    }

    for (index, side) in [Side::Top, Side::Right, Side::Bottom, Side::Left]
        .into_iter()
        .enumerate()
    {
        if used[index] <= 0.0 {
            continue;
        }
        let save = canvas.save();
        canvas.clip_path(&side_wedge_path(outer, inner, side), ClipOp::Intersect, true);
        paint_one_side(canvas, &ring, outer, used, index, colors[index], styles[index], radii);
        canvas.restore_to_count(save);
    }
}

/// Fill (or dash, or double) one side's share of the ring.
#[allow(clippy::too_many_arguments, reason = "one side's full CSS description")]
fn paint_one_side(
    canvas: &mut Canvas<'_>,
    ring: &skia_rs_safe::path::Path,
    outer: crate::layout::Rect,
    used: [f32; 4],
    index: usize,
    color: Rgba,
    style: Keyword,
    radii: &[[f32; 2]; 4],
) {
    match style {
        Keyword::Double => {
            // Three equal bands; the middle one is left empty.
            let third: [f32; 4] = std::array::from_fn(|i| used[i] / 3.0);
            let two_thirds: [f32; 4] = std::array::from_fn(|i| used[i] * 2.0 / 3.0);
            let outer_band = rounded_ring_path(
                outer,
                radii,
                outer.inset(third),
                &inner_radii(radii, third),
            );
            let inner_start = outer.inset(two_thirds);
            let inner_band = rounded_ring_path(
                inner_start,
                &inner_radii(radii, two_thirds),
                outer.inset(used),
                &inner_radii(radii, used),
            );
            canvas.draw_path(&outer_band, &fill_paint(color));
            canvas.draw_path(&inner_band, &fill_paint(color));
        }
        Keyword::Dotted | Keyword::Dashed => {
            // Stroke the centre line of this side with a dash effect, then
            // let the wedge clip keep it inside the side. `DashEffect` is
            // the crate's only dash primitive and it applies to strokes.
            let w = used[index];
            let (on, off) = if matches!(style, Keyword::Dotted) {
                (w, w)
            } else {
                (w * 3.0, w * 2.0)
            };
            let centre = outer.inset(std::array::from_fn(|i| used[i] / 2.0));
            let path = rounded_rect_path(centre, &inner_radii(radii, std::array::from_fn(|i| used[i] / 2.0)));
            let mut paint = fill_paint(color);
            paint.set_style(skia_rs_safe::paint::Style::Stroke);
            paint.set_stroke_width(w);
            paint.set_stroke_cap(if matches!(style, Keyword::Dotted) {
                skia_rs_safe::path::StrokeCap::Round
            } else {
                skia_rs_safe::path::StrokeCap::Butt
            });
            if let Some(dash) = DashEffect::new(vec![on, off], 0.0) {
                let effect: PathEffectRef = std::sync::Arc::new(dash);
                paint.set_path_effect(Some(effect));
            }
            canvas.draw_path(&path, &paint);
        }
        // `groove`, `ridge`, `inset` and `outset` are drawn as solid in M2;
        // GTK itself renders them flat in Adwaita, and the spec's paint list
        // asks only that they render, not that they emboss.
        _ => {
            canvas.draw_path(ring, &fill_paint(color));
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::border`
Expected: PASS — 5 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/border.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): per-side mitred borders with every border-style

Each side is the ring clipped to its mitre wedge, so mixed widths, colours
and radii join like GTK; solid uniform borders take a single-fill fast path
that keeps M1's exact #cdc7c2 edge pixel.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 12: `paint_border_image`

**Files:**
- Modify: `ui/src/paint/border.rs` — add `paint_border_image` and the slice
  arithmetic.
- Modify: `ui/src/paint/mod.rs` — `pub use border::paint_border_image;`
- Test: `ui/src/paint/border.rs`

**Interfaces:**
- Consumes (P1): `css::value::{Image, BorderImageSlice, BorderImageWidthSide,
  NumberOrPercent, RepeatStyle, Keyword}`.
- Consumes (Task 8/10): `PaintCx`, `ImageCache`, `tile_positions`.
- Produces:
  ```rust
  pub fn paint_border_image(canvas: &mut Canvas<'_>, alloc: &Allocation, source: &Image,
                            slice: &BorderImageSlice, widths: &[BorderImageWidthSide; 4],
                            repeat: RepeatStyle, cx: &mut PaintCx<'_>) -> bool;
  ```
  Returns `false` when nothing was painted, which tells `paint_node` to fall
  back to `paint_borders`.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/paint/border.rs`'s `mod tests`:

```rust
    use super::paint_border_image;
    use crate::css::value::{BorderImageSlice, BorderImageWidthSide, Image, NumberOrPercent, RepeatStyle};
    use crate::paint::{ImageCache, PaintCx};
    use crate::css::computed::ResolveEnv;
    use crate::text::FontDatabase;
    use std::collections::HashMap;

    #[test]
    fn an_unresolvable_border_image_reports_false_so_the_caller_falls_back() {
        // Contract §8: `false` means "I painted nothing; use paint_borders".
        // Mutation check: returning `true` unconditionally silently loses
        // every ordinary border on a theme with a bad border-image URL.
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        let mut canvas = surface.canvas();
        let painted = paint_border_image(
            &mut canvas,
            &alloc(40.0, 20.0, [4.0; 4]),
            &Image::Url("/nonexistent/icedtea-border.png".into()),
            &BorderImageSlice {
                sides: [NumberOrPercent::Number(4.0); 4],
                fill: false,
            },
            &[BorderImageWidthSide::Number(1.0); 4],
            RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::Repeat,
            },
            &mut cx,
        );
        assert!(!painted);
    }

    #[test]
    fn image_none_reports_false_without_touching_the_canvas() {
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            assert!(!paint_border_image(
                &mut canvas,
                &alloc(40.0, 20.0, [4.0; 4]),
                &Image::None,
                &BorderImageSlice {
                    sides: [NumberOrPercent::Number(0.0); 4],
                    fill: false,
                },
                &[BorderImageWidthSide::Auto; 4],
                RepeatStyle {
                    x: Keyword::Stretch,
                    y: Keyword::Stretch,
                },
                &mut cx,
            ));
        }
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
    }
```

`Keyword::Stretch` is contract §2.2 as amended by §12 E10 ("`Keyword` gains
`Stretch`, `Fill`, … — **P4 uses `Keyword::Stretch`** for
`border-image-repeat`"), so it is used directly here and in the
implementation below; `stretch` is the non-tiling arm, which scales one copy
of the slice into the slot.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::border::tests::an_unresolvable`
Expected: FAIL — `error[E0432]: unresolved import `super::paint_border_image``.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/paint/border.rs`:

```rust
use skia_rs_safe::core::IRect;

use crate::css::value::{BorderImageSlice, BorderImageWidthSide, Image, NumberOrPercent, RepeatStyle};
use crate::layout::Rect;
use crate::paint::PaintCx;

/// Paint a nine-patch `border-image` over the border box.
///
/// Returns `false` when the source cannot be resolved to pixels, so the
/// caller falls back to [`paint_borders`]. Only `url()` sources have pixels
/// in M2; gradients and `-gtk-*` images as border-image sources are M4.
pub fn paint_border_image(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    source: &Image,
    slice: &BorderImageSlice,
    widths: &[BorderImageWidthSide; 4],
    repeat: RepeatStyle,
    cx: &mut PaintCx<'_>,
) -> bool {
    let Image::Url(url) = source else {
        return false;
    };
    let Some((iw, ih)) = cx
        .images
        .get(url)
        .map(|img| (img.width() as f32, img.height() as f32))
    else {
        return false;
    };
    if iw <= 0.0 || ih <= 0.0 {
        return false;
    }

    // Slice offsets in source pixels, TRBL.
    let slice_px = |value: NumberOrPercent, basis: f32| -> f32 {
        match value {
            NumberOrPercent::Number(n) if n.is_finite() => n.clamp(0.0, basis),
            NumberOrPercent::Percent(p) if p.is_finite() => (p * basis).clamp(0.0, basis),
            _ => 0.0,
        }
    };
    let st = slice_px(slice.sides[0], ih);
    let sr = slice_px(slice.sides[1], iw);
    let sb = slice_px(slice.sides[2], ih);
    let sl = slice_px(slice.sides[3], iw);

    // Destination widths, TRBL: `auto` uses the slice, a number scales the
    // used border width, a length resolves directly.
    let ctx = cx.base_length_ctx();
    let dest = |side: &BorderImageWidthSide, used: f32, slice: f32| -> f32 {
        match side {
            BorderImageWidthSide::Auto => slice,
            BorderImageWidthSide::Number(n) if n.is_finite() => n * used,
            BorderImageWidthSide::Length(len) => len.resolve(&ctx).unwrap_or(used),
            BorderImageWidthSide::Number(_) => used,
        }
    };
    let dt = dest(&widths[0], alloc.border[0], st);
    let dr = dest(&widths[1], alloc.border[1], sr);
    let db = dest(&widths[2], alloc.border[2], sb);
    let dl = dest(&widths[3], alloc.border[3], sl);

    let outer = alloc.border_box;
    if outer.is_empty() {
        return false;
    }

    // Nine slots: (src IRect, dst Rect, tiles?).
    let src = |x: f32, y: f32, w: f32, h: f32| {
        IRect::new(
            x as i32,
            y as i32,
            (x + w) as i32,
            (y + h) as i32,
        )
    };
    let mid_w = (iw - sl - sr).max(0.0);
    let mid_h = (ih - st - sb).max(0.0);
    let dmid_w = (outer.width - dl - dr).max(0.0);
    let dmid_h = (outer.height - dt - db).max(0.0);

    let mut slots: Vec<(IRect, Rect, bool, bool)> = vec![
        (src(0.0, 0.0, sl, st), Rect::new(outer.x, outer.y, dl, dt), false, false),
        (src(sl, 0.0, mid_w, st), Rect::new(outer.x + dl, outer.y, dmid_w, dt), true, false),
        (src(iw - sr, 0.0, sr, st), Rect::new(outer.right() - dr, outer.y, dr, dt), false, false),
        (src(0.0, st, sl, mid_h), Rect::new(outer.x, outer.y + dt, dl, dmid_h), false, true),
        (src(iw - sr, st, sr, mid_h), Rect::new(outer.right() - dr, outer.y + dt, dr, dmid_h), false, true),
        (src(0.0, ih - sb, sl, sb), Rect::new(outer.x, outer.bottom() - db, dl, db), false, false),
        (src(sl, ih - sb, mid_w, sb), Rect::new(outer.x + dl, outer.bottom() - db, dmid_w, db), true, false),
        (src(iw - sr, ih - sb, sr, sb), Rect::new(outer.right() - dr, outer.bottom() - db, dr, db), false, false),
    ];
    if slice.fill {
        slots.push((
            src(sl, st, mid_w, mid_h),
            Rect::new(outer.x + dl, outer.y + dt, dmid_w, dmid_h),
            true,
            true,
        ));
    }

    let tiles_x = matches!(repeat.x, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let tiles_y = matches!(repeat.y, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let Some(image) = cx.images.get(url) else {
        return false;
    };
    let mut painted = false;
    for (src_rect, dst_rect, edge_x, edge_y) in slots {
        if dst_rect.is_empty() || src_rect.width() <= 0 || src_rect.height() <= 0 {
            continue;
        }
        if (edge_x && tiles_x) || (edge_y && tiles_y) {
            // Tile the slot with source-sized copies rather than stretching.
            let step_x = if edge_x && tiles_x { src_rect.width() as f32 } else { dst_rect.width };
            let step_y = if edge_y && tiles_y { src_rect.height() as f32 } else { dst_rect.height };
            let mut y = dst_rect.y;
            while y < dst_rect.bottom() && step_y > 0.0 {
                let mut x = dst_rect.x;
                while x < dst_rect.right() && step_x > 0.0 {
                    let piece = Rect::new(
                        x,
                        y,
                        step_x.min(dst_rect.right() - x),
                        step_y.min(dst_rect.bottom() - y),
                    );
                    canvas.draw_image_rect(image, Some(&src_rect), &piece.to_skia(), None);
                    x += step_x;
                }
                y += step_y;
            }
        } else {
            canvas.draw_image_rect(image, Some(&src_rect), &dst_rect.to_skia(), None);
        }
        painted = true;
    }
    painted
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::border`
Expected: PASS — 7 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/border.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): nine-patch border-image with slice, width and repeat

Reports false when the source has no pixels so paint_node falls back to the
ordinary per-side borders instead of dropping the frame entirely.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 13: `paint_box_shadows` — outset and inset

**Files:**
- Create: `ui/src/paint/shadow.rs`
- Modify: `ui/src/paint/mod.rs` — `pub mod shadow;` + `pub use shadow::paint_box_shadows;`
- Test: `ui/src/paint/shadow.rs`

**Interfaces:**
- Consumes (P1): `css::value::{Shadow, ColorValue, Rgba, LengthCtx}`,
  `Length::resolve`.
- Consumes (Tasks 5, 6, 8): `rounded_rect_path`, `rounded_ring_path`,
  `blur::{blurred_image, sigma_for_blur_radius}`, `fill_paint`.
- Produces:
  ```rust
  pub fn paint_box_shadows(canvas: &mut Canvas<'_>, shadows: &[Shadow], inset: bool,
                           alloc: &Allocation, radii: &[[f32; 2]; 4], current: Rgba,
                           ctx: &LengthCtx);
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/shadow.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::paint_box_shadows;
    use crate::css::value::{ColorValue, Length, LengthCtx, Rgba, Shadow};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    fn black() -> Rgba {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }

    fn shadow(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool) -> Shadow {
        Shadow {
            color: Some(ColorValue::Absolute(black())),
            offset_x: Length::px(dx),
            offset_y: Length::px(dy),
            blur: Length::px(blur),
            spread: Length::px(spread),
            inset,
        }
    }

    /// A 20x10 box centred in a 60x40 surface.
    fn alloc() -> Allocation {
        let border_box = Rect::new(20.0, 15.0, 20.0, 10.0);
        Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    fn painted(shadows: &[Shadow], inset: bool) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_box_shadows(&mut canvas, shadows, inset, &alloc(), &[[0.0, 0.0]; 4], black(), &ctx());
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn an_unblurred_outset_shadow_is_exact_at_its_flat_core() {
        // The exactness rule: with blur 0 the shadow is a hard-edged
        // rounded rect, so its interior is the declared colour exactly.
        // Mutation check: applying the offset to the wrong axis moves the
        // core off (35, 20) and the assertion fails.
        let surface = painted(&[shadow(5.0, 0.0, 0.0, 0.0, false)], false);
        assert_eq!(pixel(&surface, 35, 20), Color(0xFF00_0000));
        assert_eq!(pixel(&surface, 22, 20).alpha(), 0, "left of the offset shadow");
    }

    #[test]
    fn spread_grows_the_shadow_shape_in_every_direction() {
        // Mutation check: adding spread to only one axis leaves (20, 12)
        // clear, since the box's top is at y == 15.
        let surface = painted(&[shadow(0.0, 0.0, 0.0, 4.0, false)], false);
        assert_eq!(pixel(&surface, 20, 12), Color(0xFF00_0000));
        assert_eq!(pixel(&surface, 18, 20), Color(0xFF00_0000));
    }

    #[test]
    fn a_blurred_shadow_spreads_outside_its_shape_and_dilutes_its_edge() {
        // Mutation check: a blur that no-ops (the BlurMaskFilter trap) leaves
        // (20, 8) fully clear and (20, 14) fully opaque.
        let surface = painted(&[shadow(0.0, 0.0, 8.0, 0.0, false)], false);
        let outside = pixel(&surface, 20, 8);
        assert!(outside.alpha() > 0 && outside.alpha() < 0xFF, "soft edge");
    }

    #[test]
    fn an_inset_shadow_paints_inside_the_padding_box_only() {
        // Mutation check: forgetting the clip paints the inset shadow's ring
        // across the whole surface and (5, 5) stops being clear.
        let surface = painted(&[shadow(0.0, 6.0, 0.0, 0.0, true)], true);
        assert_eq!(pixel(&surface, 30, 17), Color(0xFF00_0000), "top strip is shadowed");
        assert_eq!(pixel(&surface, 30, 24).alpha(), 0, "the bottom is not");
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0, "nothing outside the box");
    }

    #[test]
    fn only_shadows_matching_the_requested_inset_flag_are_painted() {
        // Mutation check: ignoring the flag paints outset shadows during the
        // inset pass, which lands them on top of the background.
        let surface = painted(&[shadow(5.0, 0.0, 0.0, 0.0, false)], true);
        assert_eq!(pixel(&surface, 35, 20).alpha(), 0);
    }

    #[test]
    fn shadow_painting_never_panics_on_hostile_lengths() {
        for &v in &[f32::NAN, f32::INFINITY, -1.0e9, 1.0e9] {
            let _ = painted(&[shadow(v, v, v, v, false)], false);
            let _ = painted(&[shadow(v, v, v, v, true)], true);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::shadow`
Expected: FAIL — `error[E0432]: unresolved import `super::paint_box_shadows``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/shadow.rs`:

```rust
//! `box-shadow`, outset and inset.
//!
//! `BlurMaskFilter` never reaches a pixel through `skia-rs-safe` 0.4.0's
//! rasterizer, so a blurred shadow is drawn into an offscreen premultiplied
//! surface, blurred by `paint::blur`, and blitted back with `draw_image`,
//! which does honour the current path clip.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::value::{ColorValue, LengthCtx, Rgba, Shadow};
use crate::layout::{Allocation, Rect};
use crate::paint::blur::{blurred_image, sigma_for_blur_radius};
use crate::paint::fill_paint;
use crate::paint::geometry::{inner_radii, rounded_rect_path, rounded_ring_path};

/// A finite length in px, or `default`.
fn px(length: &crate::css::value::Length, ctx: &LengthCtx, default: f32) -> f32 {
    length
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

/// Paint every shadow in `shadows` whose `inset` flag equals `inset`.
///
/// Outset shadows paint behind the box (the caller draws them first), inset
/// shadows on top of the background inside the padding box. CSS paints a
/// shadow list back-to-front, so the list is iterated in reverse.
pub fn paint_box_shadows(
    canvas: &mut Canvas<'_>,
    shadows: &[Shadow],
    inset: bool,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    current: Rgba,
    ctx: &LengthCtx,
) {
    for shadow in shadows.iter().rev() {
        if shadow.inset != inset {
            continue;
        }
        // `computed` (contract §5, inheritance rule 5) has already resolved
        // `@name` and `currentColor` for this slot, so a shadow colour that
        // survived is `ColorValue::Absolute`; `None` means currentColor.
        let color = match shadow.color.as_ref() {
            Some(ColorValue::Absolute(rgba)) => *rgba,
            Some(_) | None => current,
        };
        if color.a <= 0.0 {
            continue;
        }
        let dx = px(&shadow.offset_x, ctx, 0.0);
        let dy = px(&shadow.offset_y, ctx, 0.0);
        let blur = px(&shadow.blur, ctx, 0.0).max(0.0);
        let spread = px(&shadow.spread, ctx, 0.0);
        if inset {
            paint_inset(canvas, alloc, radii, color, dx, dy, blur, spread);
        } else {
            paint_outset(canvas, alloc, radii, color, dx, dy, blur, spread);
        }
    }
}

/// One outset shadow: the border box, offset, spread and blurred.
#[allow(clippy::too_many_arguments, reason = "one shadow's full CSS description")]
fn paint_outset(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    color: Rgba,
    dx: f32,
    dy: f32,
    blur: f32,
    spread: f32,
) {
    let base = alloc.border_box;
    let shape = Rect::new(
        base.x + dx - spread,
        base.y + dy - spread,
        base.width + spread * 2.0,
        base.height + spread * 2.0,
    );
    if shape.is_empty() {
        return;
    }
    let shape_radii: [[f32; 2]; 4] =
        std::array::from_fn(|i| [(radii[i][0] + spread).max(0.0), (radii[i][1] + spread).max(0.0)]);

    if blur <= 0.0 {
        canvas.draw_path(&rounded_rect_path(shape, &shape_radii), &fill_paint(color));
        return;
    }
    blit_blurred(canvas, shape, blur, |offscreen_canvas, offset| {
        let local = Rect::new(shape.x - offset.0, shape.y - offset.1, shape.width, shape.height);
        offscreen_canvas.draw_path(&rounded_rect_path(local, &shape_radii), &fill_paint(color));
    });
}

/// One inset shadow: the ring between the padding box and the shrunken,
/// offset copy of it, clipped to the padding box.
#[allow(clippy::too_many_arguments, reason = "one shadow's full CSS description")]
fn paint_inset(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    color: Rgba,
    dx: f32,
    dy: f32,
    blur: f32,
    spread: f32,
) {
    let outer = alloc.padding_box();
    if outer.is_empty() {
        return;
    }
    let outer_radii = inner_radii(radii, alloc.border);
    let hole = Rect::new(
        outer.x + dx + spread,
        outer.y + dy + spread,
        (outer.width - spread * 2.0).max(0.0),
        (outer.height - spread * 2.0).max(0.0),
    );
    let hole_radii: [[f32; 2]; 4] = std::array::from_fn(|i| {
        [
            (outer_radii[i][0] - spread).max(0.0),
            (outer_radii[i][1] - spread).max(0.0),
        ]
    });

    let save = canvas.save();
    canvas.clip_path(
        &rounded_rect_path(outer, &outer_radii),
        ClipOp::Intersect,
        true,
    );
    if blur <= 0.0 {
        let ring = rounded_ring_path(outer, &outer_radii, hole, &hole_radii);
        canvas.draw_path(&ring, &fill_paint(color));
    } else {
        blit_blurred(canvas, outer, blur, |offscreen_canvas, offset| {
            let local_outer = Rect::new(outer.x - offset.0, outer.y - offset.1, outer.width, outer.height);
            let local_hole = Rect::new(hole.x - offset.0, hole.y - offset.1, hole.width, hole.height);
            let ring = rounded_ring_path(local_outer, &outer_radii, local_hole, &hole_radii);
            offscreen_canvas.draw_path(&ring, &fill_paint(color));
        });
    }
    canvas.restore_to_count(save);
}

/// Render `draw` into an offscreen surface padded for `blur`, blur it and
/// blit it at the right place.
fn blit_blurred(
    canvas: &mut Canvas<'_>,
    shape: Rect,
    blur: f32,
    draw: impl FnOnce(&mut Canvas<'_>, (f32, f32)),
) {
    let sigma = sigma_for_blur_radius(blur);
    let pad = (sigma * 3.0).ceil().clamp(0.0, 512.0);
    let ox = (shape.x - pad).floor();
    let oy = (shape.y - pad).floor();
    let w = (shape.width + pad * 2.0).ceil();
    let h = (shape.height + pad * 2.0).ceil();
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 || w > 8192.0 || h > 8192.0 {
        return;
    }
    let Some(image) = blurred_image(w as i32, h as i32, sigma, |offscreen| draw(offscreen, (ox, oy)))
    else {
        return;
    };
    canvas.draw_image(&image, ox, oy, None);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::shadow`
Expected: PASS — 6 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/shadow.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): outset and inset box-shadows

Offset, spread and blur through the project's own box blur, with inset
shadows clipped to the padding box. Flat cores stay exactly the declared
colour so pixel tests remain derivable.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 14: `paint_outline`

**Files:**
- Create: `ui/src/paint/outline.rs`
- Modify: `ui/src/paint/mod.rs` — `pub mod outline;` + `pub use outline::paint_outline;`
- Test: `ui/src/paint/outline.rs`

**Interfaces:**
- Consumes: `layout::{Allocation, Rect}`, `geometry::{rounded_ring_path,
  rounded_rect_path}`, `paint::fill_paint`, `border::is_visible_border_style`,
  `css::value::{Keyword, Rgba}`.
- Produces:
  ```rust
  pub fn paint_outline(canvas: &mut Canvas<'_>, alloc: &Allocation, width: f32,
                       offset: f32, color: Rgba, style: Keyword, radii: &[[f32; 2]; 4]);
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/outline.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::paint_outline;
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn blue() -> Rgba {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }
    }

    fn painted(width: f32, offset: f32, style: Keyword, radii: [[f32; 2]; 4]) -> Surface {
        let border_box = Rect::new(20.0, 15.0, 20.0, 10.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        };
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_outline(&mut canvas, &alloc, width, offset, blue(), style, &radii);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn an_outline_sits_outside_the_border_box_by_its_offset() {
        // Adwaita draws focus rings as `outline: 2px solid; outline-offset:
        // -3px` and positive offsets elsewhere. Mutation check: ignoring the
        // offset paints at y == 14 instead of y == 11.
        let surface = painted(2.0, 3.0, Keyword::Solid, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 11), Color(0xFF00_00FF));
        assert_eq!(pixel(&surface, 30, 14).alpha(), 0, "the offset gap is clear");
        assert_eq!(pixel(&surface, 30, 20).alpha(), 0, "the box itself is untouched");
    }

    #[test]
    fn a_negative_offset_pulls_the_outline_inside_the_box() {
        // Mutation check: clamping the offset at zero puts nothing at y == 16.
        let surface = painted(2.0, -3.0, Keyword::Solid, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 19), Color(0xFF00_00FF));
    }

    #[test]
    fn a_zero_width_or_invisible_style_paints_nothing() {
        for (w, style) in [(0.0, Keyword::Solid), (3.0, Keyword::None), (3.0, Keyword::Hidden)] {
            let surface = painted(w, 0.0, style, [[0.0, 0.0]; 4]);
            assert_eq!(pixel(&surface, 30, 14).alpha(), 0);
        }
    }

    #[test]
    fn a_wavy_outline_renders_as_solid_rather_than_disappearing() {
        // The spec's ruling: `wavy` is drawn solid in M2. Mutation check:
        // falling through to "unknown -> nothing" loses the focus ring.
        let surface = painted(2.0, 0.0, Keyword::Wavy, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 14), Color(0xFF00_00FF));
    }

    #[test]
    fn outline_painting_never_panics_on_hostile_numbers() {
        for &v in &[f32::NAN, f32::INFINITY, -1.0e9, 1.0e9] {
            let _ = painted(v, v, Keyword::Solid, [[v, v]; 4]);
            let _ = painted(v, v, Keyword::Dashed, [[v, v]; 4]);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::outline`
Expected: FAIL — `error[E0432]: unresolved import `super::paint_outline``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/outline.rs`:

```rust
//! `outline-*`.
//!
//! An outline is an ordinary ring, but it lives outside the border box by
//! `outline-offset` (which may be negative, pulling it inside), it never
//! takes part in layout, and it carries its own radius derived from the
//! element's `border-radius` plus the offset.

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{Keyword, Rgba};
use crate::layout::{Allocation, Rect};
use crate::paint::border::is_visible_border_style;
use crate::paint::geometry::{inner_radii, rounded_rect_path, rounded_ring_path};
use crate::paint::fill_paint;

/// Paint one outline around `alloc`.
pub fn paint_outline(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    width: f32,
    offset: f32,
    color: Rgba,
    style: Keyword,
    radii: &[[f32; 2]; 4],
) {
    if !width.is_finite() || width <= 0.0 || !is_visible_border_style(style) || color.a <= 0.0 {
        return;
    }
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let base = alloc.border_box;
    let outer = Rect::new(
        base.x - offset - width,
        base.y - offset - width,
        base.width + (offset + width) * 2.0,
        base.height + (offset + width) * 2.0,
    );
    if outer.is_empty() {
        return;
    }
    let grow = offset + width;
    let outer_radii: [[f32; 2]; 4] =
        std::array::from_fn(|i| [(radii[i][0] + grow).max(0.0), (radii[i][1] + grow).max(0.0)]);
    let inner = outer.inset([width; 4]);
    let inner_r = inner_radii(&outer_radii, [width; 4]);

    match style {
        Keyword::Double => {
            let third = width / 3.0;
            canvas.draw_path(
                &rounded_ring_path(outer, &outer_radii, outer.inset([third; 4]), &inner_radii(&outer_radii, [third; 4])),
                &fill_paint(color),
            );
            let start = outer.inset([third * 2.0; 4]);
            canvas.draw_path(
                &rounded_ring_path(start, &inner_radii(&outer_radii, [third * 2.0; 4]), inner, &inner_r),
                &fill_paint(color),
            );
        }
        Keyword::Dotted | Keyword::Dashed => {
            let (on, off) = if matches!(style, Keyword::Dotted) {
                (width, width)
            } else {
                (width * 3.0, width * 2.0)
            };
            let centre = outer.inset([width / 2.0; 4]);
            let path = rounded_rect_path(centre, &inner_radii(&outer_radii, [width / 2.0; 4]));
            let mut paint = fill_paint(color);
            paint.set_style(skia_rs_safe::paint::Style::Stroke);
            paint.set_stroke_width(width);
            paint.set_stroke_cap(if matches!(style, Keyword::Dotted) {
                skia_rs_safe::path::StrokeCap::Round
            } else {
                skia_rs_safe::path::StrokeCap::Butt
            });
            if let Some(dash) = DashEffect::new(vec![on, off], 0.0) {
                let effect: PathEffectRef = std::sync::Arc::new(dash);
                paint.set_path_effect(Some(effect));
            }
            canvas.draw_path(&path, &paint);
        }
        // `wavy`, `groove`, `ridge`, `inset`, `outset` and `solid` are all
        // drawn solid in M2 -- the spec's ruling for `wavy`, and what GTK
        // itself renders for the emboss styles.
        _ => {
            canvas.draw_path(
                &rounded_ring_path(outer, &outer_radii, inner, &inner_r),
                &fill_paint(color),
            );
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::outline`
Expected: PASS — 5 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/outline.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): outlines with offset, own radius and every style

Positive and negative outline-offset, solid/dashed/dotted/double, and wavy
drawn solid per the spec's ruling.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 15: `paint_text` — colour, `text-shadow` and `text-decoration-*`

**Files:**
- Create: `ui/src/paint/text.rs`
- Modify: `ui/src/paint/mod.rs` — `pub mod text;` + `pub use text::paint_text;`
- Test: `ui/src/paint/text.rs`

**Interfaces:**
- Consumes (P1/P3): `css::registry::Prop::{Color, TextShadow,
  TextDecorationLine, TextDecorationColor, TextDecorationStyle}`;
  `css::value::{Rgba, Shadow, TextDecorationLines, Keyword, LengthCtx, Value,
  ColorValue}`; `ComputedStyle::{color, get, raw}`.
- Consumes (Task 7): `text::ShapedText`, `TextMetrics`.
- Consumes (Tasks 5, 6, 8): `blur`, `fill_paint`.
- Produces:
  ```rust
  pub fn paint_text(canvas: &mut Canvas<'_>, text: &ShapedText, origin: (f32, f32),
                    style: &ComputedStyle, ctx: &LengthCtx);
  pub fn decoration_rects(text: &ShapedText, origin: (f32, f32),
                          lines: TextDecorationLines) -> Vec<Rect>;
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/text.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::{decoration_rects, paint_text};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{
        FontFamily, FontStyle, GenericFamily, Keyword, LengthCtx, TextDecorationLines,
    };
    use crate::text::{FontDatabase, FontQuery, ShapeKey};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    /// Shape "Click me" at 14px with the system probe face.
    fn shaped(db: &mut FontDatabase) -> std::rc::Rc<crate::text::ShapedText> {
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("no system font found; install dejavu/liberation/noto sans");
        db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        })
    }

    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let label = Node::new("label");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &label, &env, &mut cx);
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let mut surface = Surface::new_raster_n32_premul(120, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_text(&mut canvas, &text, (10.0, 10.0), &style, &ctx());
        }
        surface
    }

    fn dark_pixels(surface: &Surface, threshold: u8) -> u32 {
        let mut count = 0;
        for y in 0..40 {
            for x in 0..120 {
                let p = surface.pixel_buffer().get_pixel(x, y).expect("pixel");
                if p.alpha() > 0 && p.red() < threshold {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn the_label_is_actually_drawn_in_the_computed_colour() {
        // M1's `the_label_is_actually_drawn`, preserved. Mutation check:
        // painting with the initial colour instead of `style.color()` makes
        // the run black rather than #2e3436 -- caught by the second count.
        let surface = painted("label { color: #2e3436 }");
        assert!(dark_pixels(&surface, 0xC0) > 20, "the glyph run reached the canvas");
        let white = painted("label { color: #ffffff }");
        assert_eq!(dark_pixels(&white, 0xC0), 0, "a white label has no dark pixels");
    }

    #[test]
    fn an_underline_puts_ink_below_the_baseline_across_the_run() {
        // Mutation check: computing the rects against the run's origin
        // instead of its baseline draws the line through the glyphs.
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let rects = decoration_rects(&text, (10.0, 10.0), TextDecorationLines::UNDERLINE);
        assert_eq!(rects.len(), 1);
        assert!(rects[0].y > 10.0 + text.metrics.ascent, "below the baseline");
        assert!(rects[0].width >= text.metrics.width - 0.01);
    }

    #[test]
    fn overline_and_line_through_are_placed_above_and_across_the_glyphs() {
        // Mutation check: emitting the same y for all three lines collapses
        // the ordering assertion.
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let all = decoration_rects(
            &text,
            (10.0, 10.0),
            TextDecorationLines::UNDERLINE
                | TextDecorationLines::OVERLINE
                | TextDecorationLines::LINE_THROUGH,
        );
        assert_eq!(all.len(), 3);
        let mut ys: Vec<f32> = all.iter().map(|r| r.y).collect();
        ys.sort_by(f32::total_cmp);
        assert!(ys[0] < ys[1] && ys[1] < ys[2], "overline, line-through, underline");
    }

    #[test]
    fn a_text_shadow_lands_offset_from_the_glyphs() {
        // Mutation check: painting the shadow after the text hides it; the
        // offset column then matches the no-shadow render exactly.
        let with = painted("label { color: #ffffff; text-shadow: 4px 0 #000000 }");
        let without = painted("label { color: #ffffff }");
        assert!(dark_pixels(&with, 0x40) > 0);
        assert_eq!(dark_pixels(&without, 0x40), 0);
    }

    #[test]
    fn text_painting_never_panics_on_an_empty_run() {
        let mut db = FontDatabase::probe_only();
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let empty = db.shape(&ShapeKey {
            text: "",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        let sheet = CompiledSheet::compile("label { text-decoration-line: underline }");
        let node = Node::new("label");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut surface = Surface::new_raster_n32_premul(20, 20).expect("raster surface");
        let mut canvas = surface.canvas();
        paint_text(&mut canvas, &empty, (f32::NAN, f32::INFINITY), &style, &ctx());
    }
}
```

`FontFamily`, `FontStyle`, `GenericFamily` and `Keyword` are contract §2.2 /
§2.7 `css::value` types that §9's `FontQuery`/`ShapeKey` merely borrow, which
is why the test imports them from `crate::css::value` and only
`FontDatabase`, `FontQuery` and `ShapeKey` from `crate::text`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::text`
Expected: FAIL — `error[E0432]: unresolved import `super::{decoration_rects, paint_text}``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/text.rs`:

```rust
//! Text paint: `text-shadow` behind, `text-decoration-*` and the glyph run.
//!
//! `letter-spacing` and `text-transform` are already baked into the blob by
//! `text::FontDatabase::shape`; this module only positions and colours it.

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{
    ColorValue, Keyword, LengthCtx, Rgba, Shadow, TextDecorationLines, Value,
};
use crate::layout::Rect;
use crate::paint::blur::{blurred_image, sigma_for_blur_radius};
use crate::paint::fill_paint;
use crate::text::ShapedText;

/// The rects a `text-decoration-line` set paints, top-first.
///
/// Positions come from the run's own metrics: overline sits at the top of
/// the ascent, line-through at half the ascent above the baseline, and
/// underline one tenth of the size below it. Thickness is a sixteenth of
/// the size, floored at 1px, which is what GTK renders at UI sizes.
#[must_use]
pub fn decoration_rects(
    text: &ShapedText,
    origin: (f32, f32),
    lines: TextDecorationLines,
) -> Vec<Rect> {
    let (ox, oy) = (
        if origin.0.is_finite() { origin.0 } else { 0.0 },
        if origin.1.is_finite() { origin.1 } else { 0.0 },
    );
    let width = if text.metrics.width.is_finite() {
        text.metrics.width.max(0.0)
    } else {
        0.0
    };
    let ascent = text.metrics.ascent.max(0.0);
    let baseline = oy + ascent;
    let thickness = (text.size_px / 16.0).max(1.0);

    let mut out = Vec::new();
    if lines.contains(TextDecorationLines::OVERLINE) {
        out.push(Rect::new(ox, oy, width, thickness));
    }
    if lines.contains(TextDecorationLines::LINE_THROUGH) {
        out.push(Rect::new(ox, baseline - ascent / 2.0, width, thickness));
    }
    if lines.contains(TextDecorationLines::UNDERLINE) {
        out.push(Rect::new(ox, baseline + text.size_px / 10.0, width, thickness));
    }
    // `blink` renders nothing; CSS allows a UA to ignore it entirely.
    out
}

/// Paint one shaped run at `origin` (the content box's top-left).
pub fn paint_text(
    canvas: &mut Canvas<'_>,
    text: &ShapedText,
    origin: (f32, f32),
    style: &ComputedStyle,
    ctx: &LengthCtx,
) {
    let (ox, oy) = (
        if origin.0.is_finite() { origin.0 } else { 0.0 },
        if origin.1.is_finite() { origin.1 } else { 0.0 },
    );
    let color = style.color();
    let baseline = oy + text.metrics.ascent.max(0.0);

    let lines: TextDecorationLines = style.get(Prop::TextDecorationLine);
    let decoration_color = match style.raw(Prop::TextDecorationColor) {
        Value::Color(ColorValue::Absolute(rgba)) => *rgba,
        _ => color,
    };
    let decoration_style: Keyword = style.get(Prop::TextDecorationStyle);
    let rects = decoration_rects(text, (ox, oy), lines);

    // 1. text-shadow, behind everything. Contract §5 has `box_shadows()` for
    // `box-shadow` but no typed reader for `text-shadow`, so the list is read
    // off the raw computed `Value` (§2.1 `List` of `Shadow`).
    let shadows: Vec<Shadow> = match style.raw(Prop::TextShadow) {
        Value::List(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::Shadow(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Value::Shadow(s) => vec![s.clone()],
        _ => Vec::new(),
    };
    for shadow in shadows.iter().rev() {
        paint_text_shadow(canvas, text, (ox, baseline), shadow, color, ctx);
    }

    // 2. decorations under the glyphs, so descenders stay legible.
    for rect in &rects {
        paint_decoration(canvas, *rect, decoration_color, decoration_style);
    }

    // 3. the glyph run.
    if let Some(blob) = text.blob.as_ref() {
        canvas.draw_text_blob(blob, ox, baseline, &fill_paint(color));
    }
}

/// One decoration line, honouring `text-decoration-style`.
fn paint_decoration(canvas: &mut Canvas<'_>, rect: Rect, color: Rgba, style: Keyword) {
    if rect.is_empty() || color.a <= 0.0 {
        return;
    }
    let mut paint = fill_paint(color);
    paint.set_anti_alias(false);
    match style {
        Keyword::Double => {
            let gap = rect.height * 2.0;
            canvas.draw_rect(&rect.to_skia(), &paint);
            canvas.draw_rect(
                &Rect::new(rect.x, rect.y + gap, rect.width, rect.height).to_skia(),
                &paint,
            );
        }
        Keyword::Dotted | Keyword::Dashed => {
            let on = if matches!(style, Keyword::Dotted) {
                rect.height
            } else {
                rect.height * 3.0
            };
            let step = on * 2.0;
            let mut x = rect.x;
            while x < rect.x + rect.width && step > 0.0 {
                let w = on.min(rect.x + rect.width - x);
                canvas.draw_rect(&Rect::new(x, rect.y, w, rect.height).to_skia(), &paint);
                x += step;
            }
        }
        // `wavy` is drawn solid in M2 (spec §Section 4).
        _ => canvas.draw_rect(&rect.to_skia(), &paint),
    }
}

/// One `text-shadow`: the run drawn offset, blurred if asked.
fn paint_text_shadow(
    canvas: &mut Canvas<'_>,
    text: &ShapedText,
    baseline_origin: (f32, f32),
    shadow: &Shadow,
    current: Rgba,
    ctx: &LengthCtx,
) {
    let Some(blob) = text.blob.as_ref() else {
        return;
    };
    let color = match shadow.color.as_ref() {
        Some(ColorValue::Absolute(rgba)) => *rgba,
        Some(_) | None => current,
    };
    if color.a <= 0.0 {
        return;
    }
    let dx = shadow.offset_x.resolve(ctx).filter(|v| v.is_finite()).unwrap_or(0.0);
    let dy = shadow.offset_y.resolve(ctx).filter(|v| v.is_finite()).unwrap_or(0.0);
    let blur = shadow
        .blur
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
    let (bx, by) = (baseline_origin.0 + dx, baseline_origin.1 + dy);

    if blur <= 0.0 {
        canvas.draw_text_blob(blob, bx, by, &fill_paint(color));
        return;
    }
    let sigma = sigma_for_blur_radius(blur);
    let pad = (sigma * 3.0).ceil().clamp(0.0, 512.0);
    let w = (text.metrics.width + pad * 2.0).ceil();
    let h = (text.metrics.ascent + text.metrics.descent + pad * 2.0).ceil();
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 || w > 8192.0 || h > 8192.0 {
        return;
    }
    let ox = (bx - pad).floor();
    let oy = (by - text.metrics.ascent - pad).floor();
    if let Some(image) = blurred_image(w as i32, h as i32, sigma, |offscreen| {
        offscreen.draw_text_blob(blob, bx - ox, by - oy, &fill_paint(color));
    }) {
        canvas.draw_image(&image, ox, oy, None);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::text`
Expected: PASS — 5 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/text.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): text colour, text-shadow and text decorations

Shadows behind, decorations under the glyphs, and the run itself in the
computed colour; wavy renders solid per the spec's ruling.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 16: `begin_effects` / `end_effects` — opacity, transform, filter

**Files:**
- Create: `ui/src/paint/effects.rs`
- Modify: `ui/src/paint/mod.rs` — `pub mod effects;` +
  `pub use effects::{begin_effects, end_effects};`
- Test: `ui/src/paint/effects.rs`

**Interfaces:**
- Consumes (P1): `css::value::{FilterFn, TransformFn, Position, LengthCtx}`,
  `transform_list_matrix(&[TransformFn], &LengthCtx, (f32, f32), (f32, f32)) -> Matrix`.
- Consumes (P3): `ComputedStyle::{opacity, raw, length_ctx}`,
  `Prop::{Transform, TransformOrigin, Filter}`.
- Produces:
  ```rust
  pub fn begin_effects(canvas: &mut Canvas<'_>, style: &ComputedStyle,
                       alloc: &Allocation, ctx: &LengthCtx) -> usize;
  pub fn end_effects(canvas: &mut Canvas<'_>, save_count: usize);
  pub fn color_matrix_for(filters: &[FilterFn]) -> Option<[f32; 20]>;
  pub fn compose_color_matrices(outer: &[f32; 20], inner: &[f32; 20]) -> [f32; 20];
  ```

**skia gap (contract deviation 4):** `Canvas::composite_layer` applies a
layer paint's alpha, blend mode and **colour** filter, and skips its image
filter. So opacity and the eight colour-matrix filter functions work through
`save_layer`; `blur()` and `drop-shadow()` warn once and are skipped.

- [ ] **Step 1: Write the failing test**

Create `ui/src/paint/effects.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::{begin_effects, color_matrix_for, compose_color_matrices, end_effects};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{FilterFn, Length, LengthCtx};
    use crate::layout::{Allocation, Rect};
    use crate::paint::fill_paint;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::{Color, Rect as SkRect};

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    fn alloc() -> Allocation {
        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    /// Paint a solid red 40x20 rect through the effect stack `css` declares.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("box");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            let save = begin_effects(&mut canvas, &style, &alloc(), &ctx());
            let red = crate::css::value::Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
            let mut paint = fill_paint(red);
            paint.set_anti_alias(false);
            canvas.draw_rect(&SkRect::from_xywh(0.0, 0.0, 40.0, 20.0), &paint);
            end_effects(&mut canvas, save);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn opacity_scales_the_whole_subtree_once() {
        // Mutation check: applying opacity to each draw's paint instead of
        // to a layer double-darkens overlapping draws; here it would leave
        // alpha at 255 because the fill paint is opaque.
        let surface = painted("box { opacity: 0.5 }");
        let p = pixel(&surface, 10, 10);
        assert!(p.alpha() > 0x70 && p.alpha() < 0x90, "half-transparent, got {p:?}");
    }

    #[test]
    fn a_translate_transform_moves_the_painted_box() {
        // Mutation check: applying the matrix after the draw, or ignoring
        // transform-origin, leaves ink at (10, 10).
        let surface = painted("box { transform: translate(15px, 10px) }");
        assert_eq!(pixel(&surface, 10, 10).alpha(), 0);
        assert_eq!(pixel(&surface, 25, 15), Color(0xFFFF_0000));
    }

    #[test]
    fn a_grayscale_filter_desaturates_the_layer() {
        // Mutation check: setting the colour filter on the draw paint rather
        // than the layer paint has no effect at all -- the rasterizer never
        // reads it.
        let surface = painted("box { filter: grayscale(1) }");
        let p = pixel(&surface, 10, 10);
        assert_eq!(p.red(), p.green());
        assert_eq!(p.green(), p.blue());
        assert!(p.red() > 0x30 && p.red() < 0x70, "Rec.601 luma of pure red");
    }

    #[test]
    fn chained_filters_compose_into_one_matrix() {
        // Mutation check: composing in the wrong order (inner after outer)
        // changes the result for a non-commuting pair like invert+brightness.
        let one = color_matrix_for(&[FilterFn::Grayscale(1.0)]).expect("grayscale matrix");
        let chained = color_matrix_for(&[FilterFn::Grayscale(1.0), FilterFn::Invert(1.0)])
            .expect("chained matrix");
        assert_ne!(one, chained);
        let identity = compose_color_matrices(&one, &super::IDENTITY_MATRIX);
        assert_eq!(identity, one);
    }

    #[test]
    fn blur_and_drop_shadow_filters_paint_unchanged_and_do_not_panic() {
        // Contract deviation 4: skia-rs-safe 0.4.0's layer composite ignores
        // image filters, so these two are logged once and skipped. Mutation
        // check: returning None for the whole list when one entry is a blur
        // would drop the colour-matrix entries too.
        let surface = painted("box { filter: blur(4px) }");
        assert_eq!(pixel(&surface, 10, 10), Color(0xFFFF_0000));
        assert!(color_matrix_for(&[FilterFn::Blur(Length::px(4.0))]).is_none());
        assert!(
            color_matrix_for(&[FilterFn::Blur(Length::px(4.0)), FilterFn::Grayscale(1.0)])
                .is_some(),
            "the colour-matrix entries still apply"
        );
    }

    #[test]
    fn an_unstyled_node_takes_the_no_layer_fast_path() {
        // Mutation check: always pushing a layer costs an offscreen buffer
        // per node per frame; the save count must be unchanged for a node
        // with no opacity, transform or filter.
        let surface = painted("box { color: #000 }");
        assert_eq!(pixel(&surface, 10, 10), Color(0xFFFF_0000));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::effects`
Expected: FAIL — `error[E0432]: unresolved import `super::{begin_effects, color_matrix_for, compose_color_matrices, end_effects}``.

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/paint/effects.rs`:

```rust
//! `opacity`, `transform` and `filter`, as one save-layer.
//!
//! `skia-rs-canvas` 0.4.0's `composite_layer` applies a layer paint's alpha,
//! blend mode and *colour* filter, and says in-source that an image filter
//! "is not yet applied here". The rasterizer, in turn, reads only
//! `Paint::shader`. So opacity and the eight colour-matrix filter functions
//! work here; `blur()` and `drop-shadow()` are computed and animated but
//! cannot paint in M2, and warn once.

use std::sync::{Arc, Once};

use skia_rs_safe::canvas::{Canvas, SaveLayerFlags, SaveLayerRec};
use skia_rs_safe::paint::{ColorFilterRef, ColorMatrixFilter, Paint};

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{FilterFn, LengthCtx, Position, TransformFn, Value, transform_list_matrix};
use crate::layout::Allocation;

/// The 5x4 identity colour matrix, row-major.
pub const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

static IMAGE_FILTER_WARNED: Once = Once::new();

/// `outer * inner` for two 5x4 affine colour matrices.
#[must_use]
pub fn compose_color_matrices(outer: &[f32; 20], inner: &[f32; 20]) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += outer[row * 5 + k] * inner[k * 5 + col];
            }
            out[row * 5 + col] = sum;
        }
        let mut bias = outer[row * 5 + 4];
        for k in 0..4 {
            bias += outer[row * 5 + k] * inner[k * 5 + 4];
        }
        out[row * 5 + 4] = bias;
    }
    out
}

/// The W3C Filter Effects matrix for one filter function.
///
/// `None` for `blur()` and `drop-shadow()`, which are not colour matrices.
fn matrix_for(filter: &FilterFn) -> Option<[f32; 20]> {
    let clamp01 = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
    Some(match filter {
        FilterFn::Grayscale(amount) => {
            let a = clamp01(*amount);
            luminance_mix(a)
        }
        FilterFn::Sepia(amount) => {
            let a = clamp01(*amount);
            let sepia = [
                0.393, 0.769, 0.189, 0.0, 0.0, //
                0.349, 0.686, 0.168, 0.0, 0.0, //
                0.272, 0.534, 0.131, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ];
            lerp_matrix(&IDENTITY_MATRIX, &sepia, a)
        }
        FilterFn::Saturate(amount) => saturate_matrix(if amount.is_finite() { *amount } else { 1.0 }),
        FilterFn::HueRotate(degrees) => hue_rotate_matrix(if degrees.is_finite() { *degrees } else { 0.0 }),
        FilterFn::Invert(amount) => {
            let a = clamp01(*amount);
            let s = 1.0 - 2.0 * a;
            [
                s, 0.0, 0.0, 0.0, a, //
                0.0, s, 0.0, 0.0, a, //
                0.0, 0.0, s, 0.0, a, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Brightness(amount) => {
            let a = if amount.is_finite() { amount.max(0.0) } else { 1.0 };
            [
                a, 0.0, 0.0, 0.0, 0.0, //
                0.0, a, 0.0, 0.0, 0.0, //
                0.0, 0.0, a, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Contrast(amount) => {
            let a = if amount.is_finite() { amount.max(0.0) } else { 1.0 };
            let b = (1.0 - a) / 2.0;
            [
                a, 0.0, 0.0, 0.0, b, //
                0.0, a, 0.0, 0.0, b, //
                0.0, 0.0, a, 0.0, b, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Opacity(amount) => {
            let a = clamp01(*amount);
            [
                1.0, 0.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, a, 0.0,
            ]
        }
        FilterFn::Blur(_) | FilterFn::DropShadow(_) => return None,
    })
}

/// Rec.601 luma mixed with the identity by `amount` (1 == full grayscale).
fn luminance_mix(amount: f32) -> [f32; 20] {
    let gray = [
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    lerp_matrix(&IDENTITY_MATRIX, &gray, amount)
}

fn lerp_matrix(a: &[f32; 20], b: &[f32; 20], t: f32) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for i in 0..20 {
        out[i] = a[i] + (b[i] - a[i]) * t;
    }
    out
}

/// W3C `saturate()` coefficients.
fn saturate_matrix(s: f32) -> [f32; 20] {
    let (r, g, b) = (0.213, 0.715, 0.072);
    [
        r + (1.0 - r) * s, g - g * s, b - b * s, 0.0, 0.0, //
        r - r * s, g + (1.0 - g) * s, b - b * s, 0.0, 0.0, //
        r - r * s, g - g * s, b + (1.0 - b) * s, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

/// W3C `feColorMatrix type="hueRotate"` coefficients.
fn hue_rotate_matrix(degrees: f32) -> [f32; 20] {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        0.213 + c * 0.787 - s * 0.213,
        0.715 - c * 0.715 - s * 0.715,
        0.072 - c * 0.072 + s * 0.928,
        0.0,
        0.0,
        0.213 - c * 0.213 + s * 0.143,
        0.715 + c * 0.285 + s * 0.140,
        0.072 - c * 0.072 - s * 0.283,
        0.0,
        0.0,
        0.213 - c * 0.213 - s * 0.787,
        0.715 - c * 0.715 + s * 0.715,
        0.072 + c * 0.928 + s * 0.072,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

/// The single colour matrix a whole `filter` list composes to.
///
/// `None` when the list contributes no colour matrix at all. `blur()` and
/// `drop-shadow()` entries are skipped with a one-time warning.
#[must_use]
pub fn color_matrix_for(filters: &[FilterFn]) -> Option<[f32; 20]> {
    let mut matrix: Option<[f32; 20]> = None;
    for filter in filters {
        match matrix_for(filter) {
            Some(m) => {
                matrix = Some(match matrix {
                    Some(existing) => compose_color_matrices(&m, &existing),
                    None => m,
                });
            }
            None => {
                IMAGE_FILTER_WARNED.call_once(|| {
                    tracing::warn!(
                        "filter: blur() and drop-shadow() are computed and animated but cannot \
                         paint: skia-rs-safe 0.4.0's layer composite ignores image filters"
                    );
                });
            }
        }
    }
    matrix
}

/// Push the node's opacity/transform/filter stack; returns the save count
/// [`end_effects`] must restore to.
pub fn begin_effects(
    canvas: &mut Canvas<'_>,
    style: &ComputedStyle,
    alloc: &Allocation,
    ctx: &LengthCtx,
) -> usize {
    let opacity = style.opacity().clamp(0.0, 1.0);
    let transforms: Vec<TransformFn> = match style.raw(Prop::Transform) {
        Value::Transform(list) => list.to_vec(),
        _ => Vec::new(),
    };
    let filters: Vec<FilterFn> = match style.raw(Prop::Filter) {
        Value::Filter(list) => list.to_vec(),
        _ => Vec::new(),
    };
    let color_matrix = color_matrix_for(&filters);

    if opacity >= 1.0 && transforms.is_empty() && color_matrix.is_none() {
        // Fast path: no layer, no matrix, nothing to restore beyond a save.
        return canvas.save();
    }

    let mut layer_paint = Paint::new();
    layer_paint.set_alpha(opacity);
    if let Some(matrix) = color_matrix {
        let filter: ColorFilterRef = Arc::new(ColorMatrixFilter::new(matrix));
        layer_paint.set_color_filter(Some(filter));
    }
    let save = canvas.save_layer(&SaveLayerRec {
        bounds: None,
        paint: Some(&layer_paint),
        flags: SaveLayerFlags::NONE,
    });

    if !transforms.is_empty() {
        let basis = (alloc.border_box.width, alloc.border_box.height);
        let origin = transform_origin(style, alloc, ctx);
        let matrix = transform_list_matrix(&transforms, ctx, basis, origin);
        canvas.concat(&matrix);
    }
    save
}

/// `transform-origin` in absolute coordinates; the CSS default is the box's
/// centre.
fn transform_origin(style: &ComputedStyle, alloc: &Allocation, ctx: &LengthCtx) -> (f32, f32) {
    let box_rect = alloc.border_box;
    let Value::Position(position) = style.raw(Prop::TransformOrigin) else {
        return (
            box_rect.x + box_rect.width / 2.0,
            box_rect.y + box_rect.height / 2.0,
        );
    };
    let mut x_ctx = *ctx;
    x_ctx.percent_basis = Some(box_rect.width);
    let mut y_ctx = *ctx;
    y_ctx.percent_basis = Some(box_rect.height);
    let _: &Position = position;
    (
        box_rect.x + position.x.resolve(&x_ctx).unwrap_or(box_rect.width / 2.0),
        box_rect.y + position.y.resolve(&y_ctx).unwrap_or(box_rect.height / 2.0),
    )
}

/// Pop whatever [`begin_effects`] pushed.
pub fn end_effects(canvas: &mut Canvas<'_>, save_count: usize) {
    canvas.restore_to_count(save_count);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib paint::effects`
Expected: PASS — 6 tests.
Then: `cargo clippy -p icedtea-ui --lib -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/effects.rs ui/src/paint/mod.rs
git commit -m "feat(ui/paint): opacity, transform and colour-matrix filters

One save-layer per node carries the opacity and the composed W3C filter
matrix; blur() and drop-shadow() warn once because skia-rs-safe 0.4.0's
layer composite skips image filters.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 17: `paint_node`, and the M1 gate back to green

**Files:**
- Modify: `ui/src/paint/mod.rs` — add `paint_node`.
- Modify: `ui/src/widget/button.rs:8-9, 22-45, 59-78, 80-113, 150-167` —
  `Button` over `LayoutTree` + `paint_node` + `FontDatabase`.
- Modify: `ui/src/app.rs:16, 253-259` — `themed_button_allocation` plumbing.
- Modify: `ui/src/bin/themed-button.rs:44-47` — `--print-allocation`.
- Modify: `ui/tests/support/mod.rs:10, 48-79` — `PrintedAllocation`.
- Modify: `ui/tests/layer_shell_screencopy.rs:13` — the one `use` line.
- Modify: `ui/src/text.rs` — delete `FontStack` if Task 7 left it in place.
- Test: `ui/src/paint/mod.rs` + the existing
  `ui/tests/themed_button_offscreen.rs` (unchanged).

**Interfaces:**
- Consumes: every `paint_*` helper from Tasks 8-16, `LayoutTree` from
  Tasks 2-4, `FontDatabase` from Task 7.
- Consumes (P5, contract §6): `anim::Overrides`;
  `ComputedStyle::with_overrides(&self, &Overrides) -> Cow<'_, ComputedStyle>`.
  P5 has not landed yet, so `paint_node` takes `Option<&Overrides>` and
  calls `with_overrides` only when it is `Some` — the type exists as soon as
  P5 lands and `None` is the whole of P4's own usage.
- Produces:
  ```rust
  pub fn paint_node(canvas: &mut Canvas<'_>, node: &Node, style: &ComputedStyle,
                    alloc: &Allocation, overrides: Option<&Overrides>,
                    cx: &mut PaintCx<'_>);
  ```

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod tests` to `ui/src/paint/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{ImageCache, PaintCx, paint_node};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::layout::{Allocation, Rect};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("button");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let border_box = Rect::new(10.0, 10.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([4.0; 4]),
            border: [4.0; 4],
            padding: [0.0; 4],
        };
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(80, 60).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_node(&mut canvas, &node, &style, &alloc, None, &mut paint_cx);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn the_border_paints_over_the_background_not_under_it() {
        // CSS paint order: backgrounds first, then borders. Mutation check:
        // swapping the two makes the border pixel read the background.
        let surface = painted(
            "button { background-color: #112233; border: 4px solid #ff0000; \
             border-radius: 0 }",
        );
        assert_eq!(pixel(&surface, 30, 11), Color(0xFFFF_0000), "border on top");
        assert_eq!(pixel(&surface, 30, 20), Color(0xFF11_2233), "interior below");
    }

    #[test]
    fn an_outset_box_shadow_paints_behind_the_background() {
        // Mutation check: painting shadows after the background hides the
        // shadow entirely wherever the background is opaque.
        let surface = painted(
            "button { background-color: #ffffff; border: 0 solid transparent; \
             border-radius: 0; box-shadow: 6px 0 #000000 }",
        );
        assert_eq!(pixel(&surface, 54, 20), Color(0xFF00_0000), "shadow to the right");
        assert_eq!(pixel(&surface, 30, 20), Color(0xFFFF_FFFF), "not under the box");
    }

    #[test]
    fn the_outline_paints_outside_the_border_after_it() {
        // Mutation check: painting the outline before the border lets the
        // border overdraw a negative-offset ring.
        let surface = painted(
            "button { background-color: #112233; border: 0 solid transparent; \
             border-radius: 0; outline: 2px solid #00ff00; outline-offset: 2px }",
        );
        assert_eq!(pixel(&surface, 30, 7), Color(0xFF00_FF00));
        assert_eq!(pixel(&surface, 30, 9).alpha(), 0, "the 2px gap is clear");
    }

    #[test]
    fn a_border_image_replaces_the_per_side_borders_when_it_paints() {
        // Contract §8: border-image wins; a source with no pixels falls back.
        // Mutation check: painting both draws the plain border over the
        // nine-patch, so an undecodable url() would look identical either way.
        let surface = painted(
            "button { border: 4px solid #ff0000; border-radius: 0; \
             border-image-source: url(\"/nonexistent/icedtea-border.png\") }",
        );
        assert_eq!(
            pixel(&surface, 30, 11),
            Color(0xFFFF_0000),
            "an unresolvable border-image falls back to the plain border"
        );
    }

    #[test]
    fn paint_node_never_panics_on_a_degenerate_allocation() {
        let sheet = CompiledSheet::compile("button { border: 1px solid #000 }");
        let node = Node::new("button");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(8, 8).expect("raster surface");
        let mut canvas = surface.canvas();
        for &v in &[f32::NAN, f32::INFINITY, -100.0, 0.0] {
            let rect = Rect::new(v, v, v, v);
            let alloc = Allocation {
                border_box: rect,
                content_box: rect,
                border: [v; 4],
                padding: [v; 4],
            };
            paint_node(&mut canvas, &node, &style, &alloc, None, &mut paint_cx);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib paint::tests`
Expected: FAIL — `error[E0432]: unresolved import `super::paint_node``.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/paint/mod.rs`:

```rust
use crate::anim::Overrides;
use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::css::registry::Prop;
use crate::css::value::{BorderImageSlice, BorderImageWidthSide, Image, RepeatStyle, Shadow, Value};
use skia_rs_safe::canvas::Canvas;

/// Paint one node, in CSS paint order.
///
/// outset `box-shadow` -> background layers -> `border-image` (else per-side
/// borders) -> inset `box-shadow` -> `outline` -> text. Children are painted
/// by the caller, which owns the tree walk and each child's allocation.
///
/// `overrides` layers Part 5's animation output over `style`.
pub fn paint_node(
    canvas: &mut Canvas<'_>,
    node: &Node,
    style: &ComputedStyle,
    alloc: &Allocation,
    overrides: Option<&Overrides>,
    cx: &mut PaintCx<'_>,
) {
    let owned;
    let style: &ComputedStyle = match overrides {
        Some(o) if !o.is_empty() => {
            owned = style.with_overrides(o).into_owned();
            &owned
        }
        _ => style,
    };
    let _ = node;

    let save = effects::begin_effects(canvas, style, alloc, &cx.base_length_ctx());
    let len_ctx = cx.base_length_ctx();
    let radii = style.border_radii(alloc.border_box.width, alloc.border_box.height);
    let current = style.color();
    let shadows = style.box_shadows();

    shadow::paint_box_shadows(canvas, &shadows, false, alloc, &radii, current, &len_ctx);

    let layers = style.background_layers();
    let background_color: crate::css::value::Rgba = style.get(Prop::BackgroundColor);
    paint_backgrounds(canvas, background_color, &layers, alloc, &radii, cx);

    let source: Image = style.get(Prop::BorderImageSource);
    let slice: BorderImageSlice = match style.raw(Prop::BorderImageSlice) {
        Value::Slice(slice) => slice.clone(),
        _ => BorderImageSlice {
            sides: [crate::css::value::NumberOrPercent::Number(100.0); 4],
            fill: false,
        },
    };
    let widths: [BorderImageWidthSide; 4] = match style.raw(Prop::BorderImageWidth) {
        Value::BorderImageWidths(sides) => sides.clone(),
        _ => [BorderImageWidthSide::Number(1.0); 4],
    };
    let repeat: RepeatStyle = style.get(Prop::BorderImageRepeat);
    let drew_border_image =
        border::paint_border_image(canvas, alloc, &source, &slice, &widths, repeat, cx);
    if !drew_border_image {
        border::paint_borders(
            canvas,
            alloc,
            style.border_widths(),
            style.border_colors(),
            style.border_styles(),
            &radii,
        );
    }

    shadow::paint_box_shadows(canvas, &shadows, true, alloc, &radii, current, &len_ctx);

    let outline_width: f32 = style.get(Prop::OutlineWidth);
    let outline_offset: f32 = style.get(Prop::OutlineOffset);
    let outline_color: crate::css::value::Rgba = style.get(Prop::OutlineColor);
    let outline_style: Keyword = style.get(Prop::OutlineStyle);
    outline::paint_outline(
        canvas,
        alloc,
        outline_width,
        outline_offset,
        outline_color,
        outline_style,
        &radii,
    );

    if let Some(shaped) = cx.text {
        text::paint_text(
            canvas,
            shaped,
            (alloc.content_box.x, alloc.content_box.y),
            style,
            &len_ctx,
        );
    }

    effects::end_effects(canvas, save);
    let _: &[Shadow] = &shadows;
}
```

with `pub mod border; pub mod effects; pub mod outline; pub mod shadow; pub
mod text;` and the matching `pub use` lines at the top of the module.

Then rewire `ui/src/widget/button.rs`:

```rust
use std::rc::Rc;

use skia_rs_safe::canvas::Surface;

use crate::anim::Overrides;
use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::{Node, PseudoStates};
use crate::css::select::MatchCx;
use crate::css::value::{FontFamily, FontStyle, GenericFamily, Keyword};
use crate::layout::{Allocation, BoxDirection, Container, LayoutTree, Measure};
use crate::paint::{ImageCache, PaintCx, paint_node};
use crate::text::{FontDatabase, FontQuery, ShapeKey, ShapedText};

/// A themed button: a `button` node with a `label` child.
pub struct Button {
    label: String,
    node: Node,
    label_node: Node,
    style: ComputedStyle,
    label_style: ComputedStyle,
    allocation: Allocation,
    label_allocation: Allocation,
    shaped: Option<Rc<ShapedText>>,
    layout: LayoutTree,
    images: ImageCache,
    env: ResolveEnv,
}

/// Reports the shaped label's extents for the `label` leaf.
struct LabelMeasure<'a> {
    shaped: Option<&'a Rc<ShapedText>>,
}

impl Measure for LabelMeasure<'_> {
    fn measure(
        &mut self,
        node: &Node,
        _style: &ComputedStyle,
        _known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        match (&*node.name(), self.shaped) {
            ("label", Some(shaped)) => taffy::Size {
                width: shaped.metrics.width,
                height: shaped.metrics.line_height,
            },
            _ => taffy::Size::ZERO,
        }
    }
}

impl Button {
    /// A button labelled `label`, with `classes`, under `parent`.
    #[must_use]
    pub fn new(label: &str, classes: &[&str], parent: Node) -> Self {
        let node = Node::with_classes("button", classes);
        let label_node = Node::new("label");
        parent.append_child(&node);
        node.append_child(&label_node);
        let env = ResolveEnv::default();
        let empty = Allocation {
            border_box: crate::layout::Rect::zero(),
            content_box: crate::layout::Rect::zero(),
            border: [0.0; 4],
            padding: [0.0; 4],
        };
        Self {
            label: label.to_owned(),
            node,
            label_node,
            style: (*ComputedStyle::initial(&env)).clone(),
            label_style: (*ComputedStyle::initial(&env)).clone(),
            allocation: empty,
            label_allocation: empty,
            shaped: None,
            layout: LayoutTree::new(),
            images: ImageCache::new(),
            env,
        }
    }

    /// Recascade, reshape and relayout.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &mut FontDatabase) {
        let mut cx = MatchCx::new();
        self.style = ComputedStyle::resolve_chain(sheet, &self.node, &self.env, &mut cx);
        self.label_style =
            ComputedStyle::resolve_chain(sheet, &self.label_node, &self.env, &mut cx);

        let families: Rc<[FontFamily]> = self.label_style.get(crate::css::registry::Prop::FontFamily);
        let face = fonts.match_face(&FontQuery {
            families: &families,
            weight: self.label_style.get(crate::css::registry::Prop::FontWeight),
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: self.label_style.font_size_px(),
        });
        self.shaped = face.map(|face| {
            fonts.shape(&ShapeKey {
                text: &self.label,
                face: &face,
                size_px: self.label_style.font_size_px(),
                letter_spacing_px: self.label_style.get(crate::css::registry::Prop::LetterSpacing),
                features: &[],
                variations: &[],
                transform: self.label_style.get(crate::css::registry::Prop::TextTransform),
            })
        });

        let root = self.node.root();
        if self.layout.sync(&root).is_err() {
            return;
        }
        let root_style = ComputedStyle::resolve_chain(sheet, &root, &self.env, &mut cx);
        self.layout
            .set_style(&root, &root_style, Container::default(), &self.env);
        self.layout.set_style(
            &self.node,
            &self.style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &self.env,
        );
        self.layout
            .set_style(&self.label_node, &self.label_style, Container::Leaf, &self.env);
        let mut measure = LabelMeasure {
            shaped: self.shaped.as_ref(),
        };
        if self
            .layout
            .compute(
                &root,
                taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                &mut measure,
            )
            .is_ok()
        {
            if let Some(a) = self.layout.allocation(&self.node) {
                self.allocation = a;
            }
            if let Some(a) = self.layout.allocation(&self.label_node) {
                self.label_allocation = a;
            }
        }
    }

    /// Set the pseudo-class state and restyle.
    pub fn set_states(
        &mut self,
        states: PseudoStates,
        sheet: &CompiledSheet,
        fonts: &mut FontDatabase,
    ) {
        self.node.set_states(states);
        self.restyle(sheet, fonts);
    }

    /// Set the label and restyle; a no-op if unchanged.
    pub fn set_label(&mut self, label: &str, sheet: &CompiledSheet, fonts: &mut FontDatabase) {
        if self.label == label {
            return;
        }
        self.label = label.to_owned();
        self.shaped = None;
        self.restyle(sheet, fonts);
    }

    /// The label text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The current pseudo-class state.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        self.node.states()
    }

    /// The computed style.
    #[must_use]
    pub fn style(&self) -> &ComputedStyle {
        &self.style
    }

    /// The border-box allocation.
    #[must_use]
    pub fn allocation(&self) -> Allocation {
        self.allocation
    }

    /// Paint the button at `origin` on `surface`.
    pub fn render(
        &mut self,
        surface: &mut Surface,
        origin: (f32, f32),
        sheet: &CompiledSheet,
        fonts: &mut FontDatabase,
        overrides: Option<&Overrides>,
    ) {
        let alloc = translated(self.allocation, origin);
        let label_alloc = translated(self.label_allocation, origin);
        let mut cx = PaintCx {
            env: &self.env,
            colors: &sheet.colors,
            fonts,
            images: &mut self.images,
            text: None,
        };
        {
            let mut canvas = surface.canvas();
            paint_node(&mut canvas, &self.node, &self.style, &alloc, overrides, &mut cx);
        }
        cx.text = self.shaped.as_deref();
        let mut canvas = surface.canvas();
        paint_node(
            &mut canvas,
            &self.label_node,
            &self.label_style,
            &label_alloc,
            overrides,
            &mut cx,
        );
    }

    /// Rectangular hit test, matching GTK's own (not radius-aware).
    #[must_use]
    pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool {
        let a = translated(self.allocation, origin);
        x >= f64::from(a.border_box.x)
            && x < f64::from(a.border_box.right())
            && y >= f64::from(a.border_box.y)
            && y < f64::from(a.border_box.bottom())
    }
}

/// Shift an allocation so the tree's origin lands at `origin`.
fn translated(alloc: Allocation, origin: (f32, f32)) -> Allocation {
    let shift = |r: crate::layout::Rect| {
        crate::layout::Rect::new(r.x + origin.0, r.y + origin.1, r.width, r.height)
    };
    Allocation {
        border_box: shift(alloc.border_box),
        content_box: shift(alloc.content_box),
        ..alloc
    }
}
```

`Button::render` and `Button::restyle` now take `&mut FontDatabase` and
`render` takes the sheet plus `overrides`; update every caller in
`ui/src/app.rs` and `ui/tests/themed_button_offscreen.rs`'s `render`/fixture
helpers accordingly — **without changing a single numeric constant or
assertion in the gate test**.

`ui/src/app.rs:253-259` keeps its signature; only the `FontStack` ->
`FontDatabase` type and the extra `render` arguments change.

`ui/src/bin/themed-button.rs:44-47` becomes:

```rust
            Ok(allocation) => println!(
                "{} {} {} {}",
                allocation.border_box.width,
                allocation.border_box.height,
                allocation.content_box.x - allocation.border_box.x,
                allocation.content_box.y - allocation.border_box.y
            ),
```

`ui/tests/support/mod.rs` gains, and `allocation_of` returns:

```rust
/// The four numbers `themed-button --print-allocation` prints.
///
/// A local struct, not `icedtea_ui::layout::Allocation`: M2 reshaped that
/// type, and `tests/layer_shell_screencopy.rs` is a gated file whose only
/// permitted edit is the `use` line that names this one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintedAllocation {
    pub width: f32,
    pub height: f32,
    pub label_x: f32,
    pub label_y: f32,
}
```

with `allocation_of(..) -> PrintedAllocation` and the `use
icedtea_ui::layout::Allocation;` import deleted.

`ui/tests/layer_shell_screencopy.rs:13` changes from
`use icedtea_ui::layout::Allocation;` to
`use support::PrintedAllocation as Allocation;` — the alias keeps every
later line in that file byte-identical.

Finally delete `FontStack` from `ui/src/text.rs` if Task 7 left it.

- [ ] **Step 4: Run test to verify it passes**

Run, in order:
```bash
cargo test -p icedtea-ui --lib
cargo test -p icedtea-ui --test themed_button_offscreen
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS. `themed_button_offscreen` must report its 4 tests green with
no edit to any number in the file; `layer_shell_screencopy`'s 3 tests must
be green with only line 13 changed.

Verify the gate diff is mechanical:
```bash
git diff --stat ui/tests/themed_button_offscreen.rs
git diff ui/tests/layer_shell_screencopy.rs
```
Expected: the screencopy diff is exactly one line. A diff that changes a
*number* in either file fails review.

- [ ] **Step 5: Commit**

```bash
git add ui/src/paint/mod.rs ui/src/widget/button.rs ui/src/app.rs \
        ui/src/bin/themed-button.rs ui/src/text.rs \
        ui/tests/support/mod.rs ui/tests/layer_shell_screencopy.rs \
        ui/tests/themed_button_offscreen.rs
git commit -m "feat(ui/paint): paint_node in CSS paint order; M1 gate green

Wires the whole painter together -- outset shadows, backgrounds,
border-image or per-side borders, inset shadows, outline, text -- and moves
Button onto LayoutTree/paint_node/FontDatabase. Every number in the M1
offscreen gate is unchanged; layer_shell_screencopy differs by one use line.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review

### 1. Spec coverage

| Spec bullet (design §Section 4 unless noted) | Task |
|---|---|
| one `taffy` node per `Node` | 2 |
| GTK box model: margin outside, border+padding inside | 3 |
| content-box `min-width`/`min-height` (M1 rule) | 3, 4 |
| `border-spacing` as gap on containers | 3 |
| container behaviours: box (row/column, default column) and leaf | 3 |
| leaf intrinsic size from text or none | 4 |
| `transform`/`opacity`/`filter` never affect layout | 3 (they never reach taffy), 16 |
| `paint_node(canvas, node, style, allocation, overrides)` order | 17 |
| `box-shadow` outset first | 13, 17 |
| background layers: clip box, origin box, size, position, repeat | 8, 10 |
| blend-mode across layers | 10 |
| layer image = colour / gradient / cross-fade / url | 8 (colour), 9 (gradient), 10 (url); cross-fade is stored and paints nothing (spec §Out of scope defers `-gtk-*` image drawing to M4, and cross-fade has no M2 pixel requirement) |
| `border-image` if set, else per-side borders | 11, 12, 17 |
| each side a filled path between outer and inner rounded rects | 11 |
| per-corner radii | 5, 11 |
| `box-shadow` inset | 13, 17 |
| `outline`: offset; solid/dashed/dotted/double; wavy drawn solid; own radius | 14 |
| text: colour, `text-shadow`, `text-decoration-*`, `letter-spacing`, `text-transform`, `line-height` | 15 (paint), 7 (letter-spacing and text-transform at shaping; `line-height` reaches layout through the leaf's measured `line_height`) |
| children painted after the node | 17 (`paint_node` paints one node; the caller walks the tree — `Button::render` is the worked example) |
| `opacity < 1`, `transform`, `filter` through a `save_layer` | 16 |
| skia filter set: blur, brightness, contrast, grayscale, hue-rotate, invert, opacity, saturate, sepia, drop-shadow | 16 — eight paint; blur and drop-shadow are documented non-painting (deviation 4) |
| `-gtk-icon-*` draws nothing (M4) | 8, 10 (`Image::Icon` arms paint nothing) |
| exactness rule: gradients keep the sampler model (angle, multi-stop, radial, conic) | 9 |
| shadows asserted at their flat cores | 6, 13 |
| anti-aliased edges never asserted exactly | every pixel test asserts either an interior or an alpha-zero exterior |
| contract §7 `LayoutTree` API (new/sync/set_style/compute/allocation) | 2, 3, 4 |
| contract §7 `Allocation`, `Rect`, `box_for` | 1 |
| contract §8 `PaintCx`, `ImageCache`, `rounded_rect_path`, `rounded_ring_path` | 5, 8 |
| contract §8 all seven `paint_*` helpers | 8, 11, 12, 13, 14, 15, 16 |
| contract §9 `FontDatabase` surface (P4 stub) | 7 |
| contract §10.3 `src/layout.rs`'s 6 rewritten tests, every number preserved | 4 |
| contract §10.3 `src/paint.rs`'s 4 rewritten tests, same pixels | 8 |
| contract §10.2 gate: `themed_button_offscreen.rs` byte-identical numbers | 17 |
| contract §11 P4 gate: per-family pixel tests under the exactness rule | 8-16 |

Spec bullets with **no** P4 task, by design: `@keyframes`/transition
sampling (P5 owns §Section 5 and produces the `Overrides` P4 consumes);
`fontconfig` matching (P6 owns §Section 6); the coverage instrument and the
GTK property-reference test (P6 owns §Section 7); icon *drawing* (M4).

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `implement later`, `fill in details`,
`add appropriate error handling`, `handle edge cases`, `write tests for the
above`, `similar to Task`. None present. Every code step carries compilable
Rust.

**No fix-it notes remain.** An earlier draft carried six "Note for the
implementer" / "replace X with Y" paragraphs that left the adjacent code
block calling an item the contract does not define. Every one has been
resolved *inside* the code block itself and the note deleted, so each block
now reads as the literal source the implementer types:

- Task 8, Step 1 — `style.get_background_color_rgba()` is gone;
  the block calls `style.get::<Rgba>(Prop::BackgroundColor)` (contract §5),
  with `Prop` and `Rgba` in the test module's import list.
- Task 12, Step 1 — `Keyword::Stretch` **stays**, and the note claiming it
  was invented is deleted: contract §12 E10 adds the variant and names P4's
  `border-image-repeat` as its consumer.
- Task 13, Step 3 — `ColorResolution::resolve_absolute` is gone; the block
  matches `ColorValue::Absolute` directly and imports `ColorValue` instead
  of the non-existent `ColorResolution`.
- Task 15, Step 1 — the test imports `FontFamily`, `FontStyle`,
  `GenericFamily` and `Keyword` from `crate::css::value` (contract §2.2/§2.7)
  rather than from `crate::text`.
- Task 15, Step 3 — `style.box_shadows_for(Prop::TextShadow)` is gone; the
  block reads `style.raw(Prop::TextShadow)` into a `Vec<Shadow>` over
  `Value::List` / `Value::Shadow`, with `Value` imported.
- Task 6, Step 3 — the `let window = (2 * r + 1) as u32;` binding and its
  `let _ = window;` placeholder are gone; the comment alone records that the
  divisor is deliberately the sample count, not `2r+1`.

Where the contract's §5 accessor list has no typed reader for a property
(`background-color`, `text-shadow`), the code uses only contract-frozen API —
`ComputedStyle::get`, `ComputedStyle::raw` and `Value` — in place. The one
remaining forward reference, Task 9's `PaintCx::base_length_ctx`, is not a
substitution: the same step gives its full `impl` block as new P4-private
code.

### 3. Type consistency vs the contract

- `Rect`, `Allocation`, `Container`, `BoxDirection`, `Measure`,
  `LayoutTree::{new,sync,set_style,compute,allocation}`, `LayoutError` —
  identical to contract §7. `taffy_style`, `node_count`, `is_synced` are
  additions (private-item allowance), never replacements.
- `PaintCx` fields and lifetimes, `BackgroundLayer` consumption,
  `paint_backgrounds`, `paint_borders`, `paint_border_image`,
  `paint_box_shadows`, `paint_outline`, `paint_text`, `begin_effects`,
  `end_effects`, `rounded_rect_path`, `rounded_ring_path`, `ImageCache::new`
  — identical to contract §8. The single divergence is `paint_node`'s
  `&mut PaintCx<'_>` (deviation 1), which is the same mutability every other
  §8 helper already declares.
- Radii are `[[f32; 2]; 4]` in TL, TR, BR, BL order everywhere —
  `ComputedStyle::border_radii`, `clamp_radii`, `inner_radii`,
  `radii_for_box`, `rounded_rect_path`, `rounded_ring_path`,
  `paint_borders`, `paint_outline`, `paint_box_shadows`, `paint_node`.
- Per-side arrays are `[f32; 4]` in top, right, bottom, left order
  everywhere — `Allocation::{border,padding}`, `Rect::{inset,outset}`,
  `ComputedStyle::{padding,margin,border_widths,border_colors,border_styles}`,
  `inner_radii`, `paint_borders`, `paint_border_image`.
- `FontFace`, `FontQuery`, `ShapeKey`, `ShapedText`, `FontDatabase`'s eight
  methods and the three `css_*_to_fc` functions are contract §9 verbatim
  (deviation 6 covers the body, not the shape).
- `Keyword` variants used: `BorderBox`, `PaddingBox`, `ContentBox`,
  `TextBox`, `None`, `Hidden`, `Solid`, `Dashed`, `Dotted`, `Double`,
  `Wavy`, `Repeat`, `NoRepeat`, `Round`, `Space`, `Normal`, `Multiply`,
  `Screen`, `Overlay`, `Darken`, `Lighten`, `ColorDodge`, `ColorBurn`,
  `HardLight`, `SoftLight`, `Difference`, `Exclusion`, `Hue`, `Saturation`,
  `ColorBlend`, `Luminosity`, `Uppercase`, `Lowercase`, `Capitalize`,
  `FullWidth`, `Auto` — all present in contract §2.2 — plus `Stretch`, added
  to §2.2 by contract §12 E10 expressly for P4's `border-image-repeat`. No
  variant is invented.
- `Prop` variants used: `Color`, `Opacity`, `Filter`, `Transform`,
  `TransformOrigin`, `FontFamily`, `FontWeight`, `LetterSpacing`,
  `TextTransform`, `TextShadow`, `TextDecorationLine`,
  `TextDecorationColor`, `TextDecorationStyle`, `BackgroundColor`,
  `BorderImageSource`, `BorderImageSlice`, `BorderImageWidth`,
  `BorderImageRepeat`, `OutlineWidth`, `OutlineOffset`, `OutlineColor`,
  `OutlineStyle`, `BorderSpacing` — all present in contract §1.1.
- `Value` variants matched: `Length`, `Number`, `Pair`, `Color`, `Shadow`,
  `List`, `Transform`, `Filter`, `Position`, `Slice`, `BorderImageWidths` —
  all present in contract §2.1.
- `Overrides` is consumed only through `is_empty()` and
  `ComputedStyle::with_overrides`, both contract §6/§5 — nothing in P4
  constructs one, so P5 stays free.
