# Pure-Rust GTK-themed UI — M3 Part 4: reactive framework: View, builders, keyed reconciler, controllers, App loop, Cmd — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## Contract deviations

The contract (`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`) is
binding. Every place this plan departs from it is listed here, with the
conflicting text and the ruling. Each becomes a `§10` amendment
(`### E<n>`) in the contract when P4 lands; nothing below is silent.

- **D1 — `App::run` takes a `Window`, not a `Surface`.** §4.7 writes
  `pub fn run(self, surface: Surface) -> Result<(), AppError>`. §3.1 gives
  `Window` — not `Surface` — the `pump`, `next_deadline`, `open_popup`,
  `close_popup`, `clipboard`, `is_closed` and root-`Node` accessors that
  §4.7's own loop description requires, and there is no public constructor
  from a `Surface` to a `Window`. P4 ships
  `pub fn run(self, window: Window) -> Result<(), AppError>`.
- **D2 — P4 adds `Window::paint_with` to `ui/src/window/mod.rs`.** Additive
  only (P3-owned file, no existing signature touched). §9 gives P4 "the
  recursive paint walker", so `Window::render` cannot be the paint path, and
  §3.1 exposes no way to reach the window's skia surface.
- **D3 — `App` owns the `LayoutTree`, the `StyleMap` and a **per-node**
  `AnimationState`.** §3.1's `Window::animations()` returns one window-wide
  `AnimationState`; M2's `AnimationState::restyle` diffs exactly one node's
  `ComputedStyle`, so one per window cannot animate two nodes. P4 keeps
  `HashMap<NodeAddr, AnimationState>` and never calls `Window::{render,
  layout, animations, mark_dirty}`.
- **D4 — `App` folds controller deadlines itself.** §3.1 says
  `Window::next_deadline` folds "every controller's `next_deadline`";
  `Window` has no controller registry and cannot. `App` computes
  `min(window.next_deadline(), every instance's next_deadline)` and passes it
  as the `pump` timeout.
- **D5 — `EventCx` gains `pub phase: Phase`.** §4.6 requires dispatch
  "capture → target → bubble" and says a controller that sets
  `cx.handled = true` "stops the phase it is in" — unimplementable when the
  controller cannot see which phase it is in. `pub enum Phase { Capture,
  Target, Bubble }` is added to `view/controller.rs`.
- **D6 — `NodeAddr` is `selectors::OpaqueElement`.** §3.4 says `StyleMap` is
  `HashMap<NodeAddr, Rc<ComputedStyle>>` "keyed by `Node::addr()`";
  `Node::addr` is `pub(crate)` (`css/node.rs:445`) and §9 forbids P4 from
  touching `css/**`. `Element::opaque` is the public equivalent and is what
  `LayoutTree` (`layout.rs:225`) already keys on.
  **Superseded in part by contract §11 E2**, which P3 discharged first: P3
  already ships that alias, `node_addr` and `StyleMap` in
  `ui/src/window/mod.rs`, and `hit_test`/`hit_chain` take P3's. So
  `view/render.rs` re-exports them —
  `pub use crate::window::{NodeAddr, StyleMap, node_addr};` — and declares
  only `Animations` and the walkers. One public spelling, not two.
- **D7 — P4 defines `ListItem`.** §4.3's `Prop::Items(Rc<[ListItem]>)`
  references a type the contract never defines.
- **D8 — P4 adds `layout::Align` to `ui/src/layout.rs`.** Additive only; §3.6
  gives P6 the `Container`/`ChildLayout`/`set_style` edit, but §4.1's
  `View::halign` and §4.3's `Prop::Align(Align)` need the enum and P6 runs
  after P4. `ChildLayout`, `GridPlacement`, the `Container` widening and
  `set_style`'s rework stay P6's.
- **D9 — P4 creates `ui/src/icons/{mod,theme}.rs` with `IconTheme`'s
  constructors only.** §0 gives `icons/**` to P7, but §4.5/§4.6 put
  `&mut IconTheme` in `BuildCx`/`EventCx` and P4 runs first. P4 ships
  `from_env`, `with_name_and_roots`, `name`, `chain`, `clear_caches` — all
  fully implemented, no stub bodies. P7 adds `lookup`, `render`, the caches
  and the rest of the module.
- **D10 — P4 adds `Clipboard::offscreen()` to `ui/src/window/selection.rs`.**
  Additive only. §4.7's `run_offscreen` has "no Wayland connection" while
  §4.6's `EventCx` requires `&mut Clipboard`.
- **D11 — `App::{with_sheet, with_fonts, with_icons}`.** §4.7's `App::new`
  takes only `model`/`update`/`view`, and `run_offscreen` takes no sheet;
  these three `self`-consuming setters supply them for the offscreen path.
  `run` reads the sheet and fonts from the `Window`.
- **D12 — `paint/mod.rs`'s `#[allow(unused_variables)]` stays; only its
  `reason` is rewritten.** §8.1 says the attribute "is removed" once
  "`view::app`'s recursive walker" reads `node`. The walker is the *caller*;
  the parameter is still unread *inside* `paint_node`/
  `paint_node_with_children`, so removing the allow would not compile under
  `-D warnings`. P4 rewrites the reason to name the now-existing caller.
- **D13 — `on_date_selected` is `Handler::Text` (`YYYY-MM-DD`);
  `on_reordered` is `Handler::Index` (destination index).** §4.4's six
  `Handler` variants cannot carry §5's `(i32, u32, u32)` (Calendar) or
  `(usize, usize)` (Notebook) payloads, and §4.4 is the normative one.
- **D14 — pinned counts: `Kind` 64, `PropName` 97, `EventKind` 18.** §8.4 R1
  estimates "~59 `Kind` variants"; §4.2's own enumeration contains 64. The
  enumeration wins and a test pins all three counts.
- **D15 — P4 fixes the CSS node names §5 leaves unnamed:** `NotebookTab` →
  `tab`, `StackPage` → `stackpage`, `ColumnViewColumn` → `button`,
  `PopoverMenuItem` → `button` + base class `model`. P6 may amend via §10 if
  its vendored fixtures disagree.
- **D16 — `ui/src/view/render.rs` is added beyond §0's module map.** The
  restyle/layout/paint walker is ~400 lines and does not belong in
  `view/app.rs` alongside the event loop.
- **D17 — `ScriptStep`, not `Script`.** §0's module map line says
  `App, AppError, Script, Frames`; §4.7 defines `ScriptStep<Msg>`. §4.7 wins.
- **D18 — `Op::SetHandlers` is emitted for kept instances only.** A freshly
  built instance's `Insert` already carries its handlers; emitting both would
  fail §4.5's "minimal" requirement as read by the op-minimality test.
- **D19 — `cx.handled` ends the whole dispatch, not just its phase.** §4.6
  (quoted by D5) says a controller that sets `cx.handled = true` "stops the
  phase it is in". Taken literally that is self-contradictory: capture, target
  and bubble run over one path, so "capture stops descending" and "the target
  still gets the event" cannot both hold — a capture that stopped only its own
  phase would hand the event straight to the node it just intercepted. P4
  ships `handled` as "the node that set it is the last one this event
  reaches": a handled capture suppresses target and bubble, a handled target
  suppresses bubble. `Phase` (D5) is unaffected. `view::app::deliver`'s
  rustdoc states this, not the contract's wording.
- **D20 — `CompiledRule`, `CompiledSheet` and `RuleBuckets` gain
  `#[derive(Clone)]`.** §9 tells P4 not to touch `css/**`; these three added
  derives are the one exception, needed because `App::run` must own a sheet
  (`window.sheet().clone()`) while `window` stays mutably borrowed for
  `pump`/`paint_with` across the same loop body. Purely additive: no field, no
  signature and no behaviour changes, and no `css` test moves. The alternatives
  both cost more — borrowing the sheet fails the borrow checker, and changing
  `Window::sheet`'s return type would edit a P3 signature, which D2 forbids.
- **D21 — P4 adds four more `Window` methods and one re-export.** D2 declared
  `Window::paint_with`; `App::run` also needs `Window::{size, set_title,
  minimize, toggle_maximized}`, and `view::controller` needs
  `pub use layer::BTN_LEFT;` at `window`'s root to name the left button
  without reaching into a submodule. Same rule as D2: additive only, no
  existing signature touched, `ui/src/window/mod.rs` otherwise unmodified.

---

**Goal:** Build `ui/src/view/` — the Elm-shaped reactive layer that turns a
pure `view(&Model) -> View<Msg>` description tree into M2's retained
`css::node::Node` tree through a keyed reconciler, routes P3's `InputEvent`s
to per-kind controllers that emit `Msg`s, folds them with `update`, and
drives restyle → relayout → repaint every frame, on a real Wayland `Window`
and on an offscreen scripted surface alike.

**Architecture:** A `View<Msg>` is an immutable description (`Kind` + `Key` +
typed `Props` + `Handlers` + children). `reconcile` diffs last frame's
`Vec<Instance<Msg>>` against this frame's `Vec<View<Msg>>` with a keyed LCS,
producing a minimal `Vec<Op>`; surviving instances keep their `Node`, their
controller and therefore their animation, focus and shaping state. Each
`Instance` owns a `Box<dyn Controller<Msg>>` holding the behaviour state the
model must not (press, hover, cursor, scroll offset). `App` is the loop:
`Window::pump` → hit-test → capture/target/bubble dispatch → `Vec<Msg>` →
`update` (queued, never nested) → `view` → `reconcile` → `render::restyle_tree`
→ `render::layout_tree` → `render::paint_tree` → `Window::paint_with`.

**Tech Stack:** Rust 2024 (rust-version 1.94), `skia-rs-safe` 0.4.0,
`taffy` 0.14, `cssparser` 0.37, `selectors` 0.40, `bitflags` 2,
`wayland-client` 0.31, `wayland-protocols` 0.32, `wayland-protocols-wlr` 0.3,
`xkbcommon` 0.9, `fontconfig` 0.11 (optional), `tracing`. No `gtk4`/`gio`/
`glib`/`pango`/`cairo`/`gdk`, no `smithay`.

**Spec:** `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
(§1 part table, §4 "Reactive framework", §7 gates) plus the binding contract
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§4 in full; §3 as
consumed API; §8 migration; §9 P4 boundary).

## Global Constraints

- Crate pins, unchanged: `wayland-client` 0.31, `wayland-protocols` 0.32
  (features `client,staging,unstable`), `wayland-protocols-wlr` 0.3,
  `skia-rs-safe` 0.4.0 (features `std,text,codec,codec-png,svg`), `taffy`
  0.14, `cssparser` 0.37, `selectors` 0.40, `fontconfig` 0.11 (optional,
  default-on), `xkbcommon` 0.9, `bitflags` 2. P4 adds **no** dependency.
- No `gtk4`, `gio`, `glib`, `pango`, `cairo`, `gdk`; no `smithay` (these are
  Wayland *clients*; wlroots is the server side).
- `edition = 2024`, `rust-version = 1.94`, both from `[workspace.package]`.
- Gates for every task in this part:
  `cargo test -p icedtea-ui`,
  `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
  `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`,
  `cargo fmt --all --check`.
  (P1's gates are the `wlr` crate tests + `cargo test -p wlr --test
  coverage_audit` + `cargo xtask coverage` + `cargo publish --dry-run`; P2's
  is `cargo test -p icedtea-compositor --test popups` run three times. Neither
  applies to P4, which touches neither repo.)
- Every commit message ends with the trailer
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- The M1/M2 gates stay green unless the contract's §8.3 migration table names
  the file: `ui/tests/themed_button_offscreen.rs` (4),
  `ui/tests/adwaita_coverage.rs` (9), `ui/tests/gtk4_property_reference.rs`
  (4), `ui/tests/transition_screencopy.rs` (1), every test under
  `ui/src/css/**`, `ui/src/anim/**`, `ui/src/shm.rs` (11), and
  `ui/src/widget/button.rs` (4) must not change. P4 edits none of them.
- Nothing outside `css::registry` names a CSS property string; `Node` and
  `PseudoStates` from `css::node` are the only node/state types; property
  counts are 114/95/19 (M2 E14); call `anim::keyframes::resolve_segment`, not
  `Keyframes::segment` (E9).
- Untrusted input never panics. In P4 that means: a `Prop` of the wrong
  variant for its `PropName`, a `Key` collision, a `View` tree deeper than
  the recursion budget, and a `Prop::Draw` closure that draws nothing are all
  dropped/clamped and logged once — never `unwrap`, never `panic!`.
- Parts execute **in order P1 → P8**. P4 may consume anything P1–P3 produced.
  P1 runs in a `wlroots-sys` worktree and icedtea consumes the **published**
  `wlr` 0.20.28 through its pins; during P2–P8 development a root
  `[patch.crates-io]` pointing at that worktree is allowed in its own commit
  and is dropped before merge, exactly as the implicit-grab fix did. P4 adds
  and removes no patch of its own.
- `Duration::ZERO` from any `next_deadline` means "now", never "spin".

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/src/view/mod.rs` | create | `View`, `Kind`, `Key`, `PropName`, `Prop`, `ListItem`, `Props`, `EventKind`, `Handler`, `Handlers`; re-exports the submodules |
| `ui/src/view/builders.rs` | create | `widget(kind)` free constructor, the eighteen `on_<event>` setters, and the normative builder-naming doc P5/P6 follow |
| `ui/src/view/render.rs` | create | `NodeAddr`, `StyleMap`, `restyle_tree`, `layout_tree`, `ViewMeasure`, `paint_tree` — the recursive walkers |
| `ui/src/view/controller.rs` | create | `Controller` trait, `Event`, `Phase`, `EventCx`, `GenericC`, `build_controller` |
| `ui/src/view/reconcile.rs` | create | `Instance`, `Op`, `BuildCx`, `reconcile` (keyed LCS with moves) |
| `ui/src/view/cmd.rs` | create | `Cmd` |
| `ui/src/view/app.rs` | create | `App`, `AppError`, `ScriptStep`, `Frames`, dispatch, `run`, `run_offscreen` |
| `ui/src/icons/mod.rs` | create (D9) | `pub mod theme; pub use theme::IconTheme;` |
| `ui/src/icons/theme.rs` | create (D9) | `IconTheme` constructors, name, inheritance chain, `clear_caches` |
| `ui/src/layout.rs` | modify (D8) | `+ pub enum Align` — additive, nothing else touched |
| `ui/src/paint/mod.rs` | modify (D12) | the two `#[allow(unused_variables, reason = …)]` reason strings |
| `ui/src/window/mod.rs` | modify (D2, D21) | `+ Window::{paint_with, size, set_title, minimize, toggle_maximized}`, `+ pub use layer::BTN_LEFT` |
| `ui/src/css/{cascade,select}.rs` | modify (D20) | `+ #[derive(Clone)]` on `CompiledRule`, `CompiledSheet`, `RuleBuckets` |
| `ui/src/window/selection.rs` | modify (D10) | `+ Clipboard::offscreen` |
| `ui/src/lib.rs` | modify | `pub mod icons; pub mod view;` |
| `ui/tests/reconcile_props.rs` | create | reconciler property tests (identity, minimality, drop-once) |
| `ui/tests/counter_app.rs` | create | the offscreen counter app, pixel-asserted |
| `ui/README.md` | modify | a "Reactive framework" section |

---

## Task 1: `PropName`, `Prop`, `ListItem`, `Props`, and `layout::Align`

**Files:**
- Create: `ui/src/view/mod.rs`
- Modify: `ui/src/layout.rs` (append `Align` after `BoxDirection`, around line 158)
- Modify: `ui/src/lib.rs` (add `pub mod view;`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/mod.rs`

**Interfaces:**
- Consumes: `crate::css::value::image::IconRef` (M2, `Clone + Debug + PartialEq`);
  `skia_rs_safe::canvas::Canvas`; `crate::layout::Rect`.
- Produces:
  ```rust
  pub enum PropName { /* 97 variants, §4.3 order */ }
  pub struct ListItem { pub id: u64, pub text: Rc<str>,
                        pub subtitle: Option<Rc<str>>, pub icon: Option<IconRef> }
  pub enum Prop { Str(Rc<str>), Bool(bool), Int(i64), Float(f64), Icon(IconRef),
                  Classes(Rc<[Rc<str>]>), Edges([i32; 4]), Align(crate::layout::Align),
                  Enum(u16), Items(Rc<[ListItem]>),
                  Draw(Rc<dyn Fn(&mut Canvas<'_>, crate::layout::Rect)>), None }
  pub struct Props(Vec<(PropName, Prop)>);
  impl Props {
      pub fn set(&mut self, name: PropName, value: Prop);
      pub fn get(&self, name: PropName) -> Option<&Prop>;
      pub fn str(&self, name: PropName) -> Option<&str>;
      pub fn bool(&self, name: PropName, default: bool) -> bool;
      pub fn int(&self, name: PropName, default: i64) -> i64;
      pub fn float(&self, name: PropName, default: f64) -> f64;
      pub fn diff(&self, prev: &Props) -> Vec<PropName>;
      pub fn iter(&self) -> impl Iterator<Item = (PropName, &Prop)>;
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
  }
  // ui/src/layout.rs
  pub enum Align { Fill, Start, End, Center, Baseline }   // Default = Fill
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/view/mod.rs` containing only the test module for now:

```rust
//! The reactive framework: `View` description trees, keyed reconciliation
//! into M2's retained [`Node`](crate::css::node::Node)s, per-kind
//! controllers, and the `App` loop that folds their messages.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_are_kept_sorted_and_set_overwrites_in_place() {
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));
        props.set(PropName::Classes, Prop::Classes(Rc::from([Rc::from("flat")])));
        props.set(PropName::Label, Prop::Str("Cancel".into()));

        let names: Vec<PropName> = props.iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec![PropName::Classes, PropName::Label]);
        assert_eq!(props.str(PropName::Label), Some("Cancel"));
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn typed_accessors_fall_back_when_the_variant_is_wrong() {
        let mut props = Props::default();
        props.set(PropName::Active, Prop::Str("yes".into()));
        props.set(PropName::Value, Prop::Int(7));

        // A `Str` under `Active` is a caller bug, not a panic.
        assert!(!props.bool(PropName::Active, false));
        // `Int` widens to `float`, because a builder may hand either.
        assert!((props.float(PropName::Value, 0.0) - 7.0).abs() < f64::EPSILON);
        assert_eq!(props.int(PropName::Value, 0), 7);
        assert_eq!(props.str(PropName::Value), None);
        assert_eq!(props.int(PropName::Digits, -1), -1);
    }

    #[test]
    fn diff_reports_changed_added_and_removed_names_sorted() {
        let mut prev = Props::default();
        prev.set(PropName::Label, Prop::Str("Ok".into()));
        prev.set(PropName::Sensitive, Prop::Bool(true));
        prev.set(PropName::Tooltip, Prop::Str("gone".into()));

        let mut next = Props::default();
        next.set(PropName::Label, Prop::Str("Ok".into()));       // unchanged
        next.set(PropName::Sensitive, Prop::Bool(false));        // changed
        next.set(PropName::Visible, Prop::Bool(true));           // added

        assert_eq!(
            next.diff(&prev),
            vec![PropName::Visible, PropName::Sensitive, PropName::Tooltip]
                .into_iter()
                .collect::<Vec<_>>()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
            "diff must report added, changed and removed names, sorted"
        );
    }

    #[test]
    fn draw_props_compare_by_pointer_not_by_call() {
        let f: Rc<dyn Fn(&mut skia_rs_safe::canvas::Canvas<'_>, crate::layout::Rect)> =
            Rc::new(|_, _| {});
        let g: Rc<dyn Fn(&mut skia_rs_safe::canvas::Canvas<'_>, crate::layout::Rect)> =
            Rc::new(|_, _| {});
        assert_eq!(Prop::Draw(Rc::clone(&f)), Prop::Draw(Rc::clone(&f)));
        assert_ne!(Prop::Draw(f), Prop::Draw(g));
    }

    #[test]
    fn the_prop_name_table_is_the_contract_s_ninety_seven() {
        assert_eq!(PropName::ALL.len(), 97);
        assert_eq!(PropName::ALL[0], PropName::Classes);
        assert_eq!(PropName::ALL[96], PropName::MessageType);
        let mut sorted = PropName::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 97, "PropName has a duplicate or an ordering gap");
    }

    #[test]
    fn align_defaults_to_fill() {
        assert_eq!(crate::layout::Align::default(), crate::layout::Align::Fill);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: FAIL — `error[E0433]: failed to resolve: use of undeclared crate or module 'view'`
(the module is not in `lib.rs` yet), and after adding it,
`cannot find type 'Props' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/lib.rs`, keeping the `pub mod` list alphabetical:

```rust
pub mod view;
```

Append to `ui/src/layout.rs`, directly after the `BoxDirection` enum:

```rust
/// GTK's per-child alignment (`halign`/`valign`).
///
/// P6 (contract §3.6) makes `LayoutTree::set_style` read this through a
/// `ChildLayout`; P4 needs only the enum, because `view::Prop::Align` and
/// `View::halign`/`View::valign` carry it (contract deviation D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Fill the whole allocation — GTK's default.
    #[default]
    Fill,
    /// Pack at the start edge (left under LTR).
    Start,
    /// Pack at the end edge.
    End,
    /// Centre in the allocation.
    Center,
    /// Align on the first baseline; falls back to `Start` where there is none.
    Baseline,
}
```

Write the body of `ui/src/view/mod.rs` above the test module:

```rust
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::value::image::IconRef;
use crate::layout::{Align, Rect};

/// A typed property name, never a string — contract §0's "nothing names a
/// property string" rule, extended from CSS properties to widget props.
///
/// The order is contract §4.3's: the fifteen `GtkWidget`-universal names
/// first, then the shared widget names. `Ord` follows declaration order, and
/// [`Props`] keeps its entries sorted by it, so a `Props` comparison is a
/// slice comparison and `diff` is a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PropName {
    // GtkWidget-universal (15).
    Classes, Id, Visible, Sensitive, Focusable, Tooltip, Halign, Valign,
    Hexpand, Vexpand, Margin, WidthRequest, HeightRequest, Cursor, Opacity,
    // Shared widget props (82).
    Label, Text, Placeholder, Icon, IconSize, Active, Checked, Indeterminate,
    Value, Lower, Upper, StepIncrement, PageIncrement, Digits, Wrap, Ellipsize,
    Xalign, Yalign, Markup, Selectable, Uri, Group, Orientation, Spacing,
    Homogeneous, RowSpacing, ColumnSpacing, Position, Ratio, Expanded,
    Title, Subtitle, Fraction, Pulse, Inverted, ShowText, MaxLength,
    Visibility, Editable, EnableUndo, SelectionMode, Model, Selected,
    EnableSearch, ShowArrow, Modal, Autohide, Transition, TransitionDuration,
    Reveal, Decoration, Side, Anchor, Gravity, Constraint, Offset, Reactive,
    Columns, Rows, Column, Row, ColumnSpan, RowSpan, Fit, Paintable,
    DrawFn, ItemFactory, Page, Sortable, Resizable, MinContentWidth,
    MinContentHeight, OverlayScrolling, Kinetic, ShowSeparators, MarksTop,
    MarksBottom, FillLevel, Message, Detail, Buttons, MessageType,
}

impl PropName {
    /// Every name, in declaration order. Pinned at 97 by this module's tests
    /// so the table and the contract cannot drift.
    pub const ALL: &'static [PropName] = &[
        PropName::Classes, PropName::Id, PropName::Visible, PropName::Sensitive,
        PropName::Focusable, PropName::Tooltip, PropName::Halign, PropName::Valign,
        PropName::Hexpand, PropName::Vexpand, PropName::Margin,
        PropName::WidthRequest, PropName::HeightRequest, PropName::Cursor,
        PropName::Opacity,
        PropName::Label, PropName::Text, PropName::Placeholder, PropName::Icon,
        PropName::IconSize, PropName::Active, PropName::Checked,
        PropName::Indeterminate, PropName::Value, PropName::Lower, PropName::Upper,
        PropName::StepIncrement, PropName::PageIncrement, PropName::Digits,
        PropName::Wrap, PropName::Ellipsize, PropName::Xalign, PropName::Yalign,
        PropName::Markup, PropName::Selectable, PropName::Uri, PropName::Group,
        PropName::Orientation, PropName::Spacing, PropName::Homogeneous,
        PropName::RowSpacing, PropName::ColumnSpacing, PropName::Position,
        PropName::Ratio, PropName::Expanded, PropName::Title, PropName::Subtitle,
        PropName::Fraction, PropName::Pulse, PropName::Inverted,
        PropName::ShowText, PropName::MaxLength, PropName::Visibility,
        PropName::Editable, PropName::EnableUndo, PropName::SelectionMode,
        PropName::Model, PropName::Selected, PropName::EnableSearch,
        PropName::ShowArrow, PropName::Modal, PropName::Autohide,
        PropName::Transition, PropName::TransitionDuration, PropName::Reveal,
        PropName::Decoration, PropName::Side, PropName::Anchor, PropName::Gravity,
        PropName::Constraint, PropName::Offset, PropName::Reactive,
        PropName::Columns, PropName::Rows, PropName::Column, PropName::Row,
        PropName::ColumnSpan, PropName::RowSpan, PropName::Fit,
        PropName::Paintable, PropName::DrawFn, PropName::ItemFactory,
        PropName::Page, PropName::Sortable, PropName::Resizable,
        PropName::MinContentWidth, PropName::MinContentHeight,
        PropName::OverlayScrolling, PropName::Kinetic, PropName::ShowSeparators,
        PropName::MarksTop, PropName::MarksBottom, PropName::FillLevel,
        PropName::Message, PropName::Detail, PropName::Buttons,
        PropName::MessageType,
    ];
}

/// One row of a list model.
///
/// Contract deviation D7: §4.3's `Prop::Items(Rc<[ListItem]>)` names this
/// type but never defines it. `id` is the stable identity a keyed
/// [`View`] uses, so a re-sorted model moves rows rather than rebuilding them.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Stable identity across model edits.
    pub id: u64,
    /// The primary text a default item factory renders.
    pub text: Rc<str>,
    /// Optional second line.
    pub subtitle: Option<Rc<str>>,
    /// Optional leading icon.
    pub icon: Option<IconRef>,
}

/// A property value.
///
/// `Draw` compares by [`Rc::ptr_eq`] (contract §4.3): two closures are the
/// same prop only when they are the same allocation, so a `view` that rebuilds
/// its draw closure every frame repaints every frame — which is what a caller
/// that captures the model wants.
#[derive(Clone)]
pub enum Prop {
    /// An interned string.
    Str(Rc<str>),
    /// A flag.
    Bool(bool),
    /// A whole number, including enum discriminants that need a range.
    Int(i64),
    /// A real number.
    Float(f64),
    /// A GTK image function (`-gtk-icontheme`, `-gtk-recolor`, `-gtk-scaled`).
    Icon(IconRef),
    /// CSS classes to add to the node, beyond `Kind::base_classes`.
    Classes(Rc<[Rc<str>]>),
    /// `top, right, bottom, left`, in px.
    Edges([i32; 4]),
    /// `halign`/`valign`.
    Align(Align),
    /// A widget-local `#[repr(u16)]` enum.
    Enum(u16),
    /// A list model.
    Items(Rc<[ListItem]>),
    /// A `DrawingArea` paint callback.
    Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect)>),
    /// The property is absent — what [`Props::diff`] reports for a removal
    /// and what [`crate::view::controller::Controller::set_prop`] receives
    /// when a prop disappears.
    None,
}

impl std::fmt::Debug for Prop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Prop::Str(s) => f.debug_tuple("Str").field(s).finish(),
            Prop::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Prop::Int(i) => f.debug_tuple("Int").field(i).finish(),
            Prop::Float(x) => f.debug_tuple("Float").field(x).finish(),
            Prop::Icon(icon) => f.debug_tuple("Icon").field(icon).finish(),
            Prop::Classes(c) => f.debug_tuple("Classes").field(c).finish(),
            Prop::Edges(e) => f.debug_tuple("Edges").field(e).finish(),
            Prop::Align(a) => f.debug_tuple("Align").field(a).finish(),
            Prop::Enum(v) => f.debug_tuple("Enum").field(v).finish(),
            Prop::Items(items) => f.debug_tuple("Items").field(items).finish(),
            // A closure has no useful representation; its identity is its
            // address, which is what `PartialEq` compares.
            Prop::Draw(rc) => write!(f, "Draw({:p})", Rc::as_ptr(rc)),
            Prop::None => f.write_str("None"),
        }
    }
}

impl PartialEq for Prop {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Prop::Str(a), Prop::Str(b)) => a == b,
            (Prop::Bool(a), Prop::Bool(b)) => a == b,
            (Prop::Int(a), Prop::Int(b)) => a == b,
            // Bit equality, so a value that did not change does not restyle:
            // `NaN != NaN` would make an unset `fill-level` dirty every frame.
            (Prop::Float(a), Prop::Float(b)) => a.to_bits() == b.to_bits(),
            (Prop::Icon(a), Prop::Icon(b)) => a == b,
            (Prop::Classes(a), Prop::Classes(b)) => a == b,
            (Prop::Edges(a), Prop::Edges(b)) => a == b,
            (Prop::Align(a), Prop::Align(b)) => a == b,
            (Prop::Enum(a), Prop::Enum(b)) => a == b,
            (Prop::Items(a), Prop::Items(b)) => a == b,
            (Prop::Draw(a), Prop::Draw(b)) => Rc::ptr_eq(a, b),
            (Prop::None, Prop::None) => true,
            _ => false,
        }
    }
}

impl From<&str> for Prop {
    fn from(value: &str) -> Self {
        Prop::Str(Rc::from(value))
    }
}
impl From<String> for Prop {
    fn from(value: String) -> Self {
        Prop::Str(Rc::from(value.as_str()))
    }
}
impl From<bool> for Prop {
    fn from(value: bool) -> Self {
        Prop::Bool(value)
    }
}
impl From<i64> for Prop {
    fn from(value: i64) -> Self {
        Prop::Int(value)
    }
}
impl From<i32> for Prop {
    fn from(value: i32) -> Self {
        Prop::Int(i64::from(value))
    }
}
impl From<usize> for Prop {
    fn from(value: usize) -> Self {
        Prop::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}
impl From<f64> for Prop {
    fn from(value: f64) -> Self {
        Prop::Float(value)
    }
}
impl From<Align> for Prop {
    fn from(value: Align) -> Self {
        Prop::Align(value)
    }
}
impl From<IconRef> for Prop {
    fn from(value: IconRef) -> Self {
        Prop::Icon(value)
    }
}

/// A small sorted map from [`PropName`] to [`Prop`].
///
/// Most widgets carry under a dozen props, so a sorted `Vec` beats a hash
/// map on every operation that matters here: construction, whole-value
/// comparison, and the merge [`Props::diff`] performs.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Props(Vec<(PropName, Prop)>);

impl Props {
    /// Set `name`, replacing any previous value and keeping the map sorted.
    ///
    /// [`Prop::None`] is stored like any other value; a *removal* is
    /// expressed by the name being absent from the next frame's `Props`,
    /// which is what [`Props::diff`] reports.
    pub fn set(&mut self, name: PropName, value: Prop) {
        match self.0.binary_search_by_key(&name, |(n, _)| *n) {
            Ok(index) => self.0[index].1 = value,
            Err(index) => self.0.insert(index, (name, value)),
        }
    }

    /// The value under `name`, if any.
    #[must_use]
    pub fn get(&self, name: PropName) -> Option<&Prop> {
        self.0
            .binary_search_by_key(&name, |(n, _)| *n)
            .ok()
            .map(|index| &self.0[index].1)
    }

    /// `name` as a string; `None` when absent or of another variant.
    #[must_use]
    pub fn str(&self, name: PropName) -> Option<&str> {
        match self.get(name) {
            Some(Prop::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// `name` as a flag, falling back to `default`.
    #[must_use]
    pub fn bool(&self, name: PropName, default: bool) -> bool {
        match self.get(name) {
            Some(Prop::Bool(b)) => *b,
            _ => default,
        }
    }

    /// `name` as a whole number, falling back to `default`.
    #[must_use]
    pub fn int(&self, name: PropName, default: i64) -> i64 {
        match self.get(name) {
            Some(Prop::Int(i)) => *i,
            Some(Prop::Enum(v)) => i64::from(*v),
            _ => default,
        }
    }

    /// `name` as a real number, falling back to `default`. An `Int` widens,
    /// because a builder may hand either for a numeric property.
    #[must_use]
    pub fn float(&self, name: PropName, default: f64) -> f64 {
        match self.get(name) {
            Some(Prop::Float(x)) => *x,
            #[allow(
                clippy::cast_precision_loss,
                reason = "widget props never reach 2^53"
            )]
            Some(Prop::Int(i)) => *i as f64,
            _ => default,
        }
    }

    /// Names whose value differs from `prev`, plus names present in one side
    /// only. Sorted, deduplicated — the reconciler's `SetProp` op set.
    #[must_use]
    pub fn diff(&self, prev: &Props) -> Vec<PropName> {
        let mut out = Vec::new();
        let (mut a, mut b) = (0, 0);
        while a < self.0.len() || b < prev.0.len() {
            match (self.0.get(a), prev.0.get(b)) {
                (Some((na, va)), Some((nb, vb))) if na == nb => {
                    if va != vb {
                        out.push(*na);
                    }
                    a += 1;
                    b += 1;
                }
                (Some((na, _)), Some((nb, _))) if na < nb => {
                    out.push(*na);
                    a += 1;
                }
                (Some(_), Some((nb, _))) => {
                    out.push(*nb);
                    b += 1;
                }
                (Some((na, _)), None) => {
                    out.push(*na);
                    a += 1;
                }
                (None, Some((nb, _))) => {
                    out.push(*nb);
                    b += 1;
                }
                (None, None) => break,
            }
        }
        out
    }

    /// Every `(name, value)`, in `PropName` order.
    pub fn iter(&self) -> impl Iterator<Item = (PropName, &Prop)> {
        self.0.iter().map(|(name, value)| (*name, value))
    }

    /// How many properties are set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: PASS — 6 tests.

Then the whole-crate gates:
Run: `cargo test -p icedtea-ui`
Expected: PASS, and the M2 counts are untouched (4 + 9 + 4 + 1 e2e).
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
Expected: no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
Expected: no warnings.
Run: `cargo fmt --all --check`
Expected: no output.

**Mutation check (load-bearing: `diff_reports_changed_added_and_removed_names_sorted`).**
Change `Props::diff`'s `if va != vb` to `if false`, re-run
`cargo test -p icedtea-ui --lib view::tests::diff_reports` — it must FAIL
(the changed `Sensitive` disappears from the result). Restore. Record the
result in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/mod.rs ui/src/layout.rs ui/src/lib.rs
git commit -m "feat(ui): typed widget props for the reactive framework

PropName's 97 names (contract §4.3), Prop with pointer-equal Draw
closures, ListItem (deviation D7), and the sorted Props map whose diff
drives the reconciler's SetProp ops. layout::Align lands here too
(deviation D8) because Prop::Align needs it and P6 runs later.

Mutation check: neutering Props::diff's value comparison drops the
changed Sensitive prop and fails diff_reports_changed_added_and_removed.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2: `Kind`

**Files:**
- Modify: `ui/src/view/mod.rs` (append `Kind` after `Props`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/mod.rs`

**Interfaces:**
- Consumes: nothing from Task 1 beyond the module existing.
- Produces:
  ```rust
  pub enum Kind { /* 64 variants, §4.2 order */ }
  impl Kind {
      pub fn css_name(self) -> &'static str;
      pub fn base_classes(self) -> &'static [&'static str];
      pub fn is_focusable_by_default(self) -> bool;
      pub fn all() -> &'static [Kind];
  }
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/mod.rs`'s test module:

```rust
    #[test]
    fn every_kind_is_in_all_exactly_once() {
        // Contract deviation D14: §8.4 R1 estimates "~59"; §4.2's own
        // enumeration is 64, and the enumeration is the normative one.
        assert_eq!(Kind::all().len(), 64);
        let mut seen: Vec<Kind> = Kind::all().to_vec();
        seen.sort_unstable_by_key(|k| *k as usize);
        seen.dedup();
        assert_eq!(seen.len(), 64, "Kind::all has a duplicate");
        assert_eq!(Kind::all()[0], Kind::Label);
        assert_eq!(Kind::all()[63], Kind::AlertDialog);
    }

    #[test]
    fn kinds_that_share_a_css_name_are_told_apart_by_their_base_classes() {
        assert_eq!(Kind::Button.css_name(), "button");
        assert_eq!(Kind::ToggleButton.css_name(), "button");
        assert_eq!(Kind::LinkButton.css_name(), "button");
        assert_eq!(Kind::Button.base_classes(), &[] as &[&str]);
        assert_eq!(Kind::ToggleButton.base_classes(), &["toggle"]);
        assert_eq!(Kind::LinkButton.base_classes(), &["link"]);

        assert_eq!(Kind::Box.css_name(), "box");
        assert_eq!(Kind::CenterBox.css_name(), "box");

        assert_eq!(Kind::Entry.css_name(), "entry");
        assert_eq!(Kind::SearchEntry.base_classes(), &["search"]);
        assert_eq!(Kind::PasswordEntry.base_classes(), &["password"]);

        assert_eq!(Kind::Window.css_name(), "window");
        assert_eq!(Kind::Window.base_classes(), &["background"]);
        assert_eq!(Kind::AlertDialog.base_classes(), &["dialog", "message"]);
        assert_eq!(Kind::PopoverMenu.base_classes(), &["background", "menu"]);
    }

    #[test]
    fn no_css_name_is_empty_and_none_carries_a_dot_or_a_space() {
        for kind in Kind::all() {
            let name = kind.css_name();
            assert!(!name.is_empty(), "{kind:?} has no CSS node name");
            assert!(
                !name.contains('.') && !name.contains(' '),
                "{kind:?}'s node name {name:?} smuggles a class in"
            );
            for class in kind.base_classes() {
                assert!(
                    !class.is_empty() && !class.starts_with('.'),
                    "{kind:?} base class {class:?} must be bare"
                );
            }
        }
    }

    #[test]
    fn the_focusable_default_follows_gtk_not_the_node_name() {
        assert!(Kind::Button.is_focusable_by_default());
        assert!(Kind::Entry.is_focusable_by_default());
        assert!(Kind::ListBoxRow.is_focusable_by_default());
        // Contract §3.5: WindowControls' three buttons are never candidates.
        assert!(!Kind::WindowControls.is_focusable_by_default());
        assert!(!Kind::Label.is_focusable_by_default());
        assert!(!Kind::Box.is_focusable_by_default());
        assert!(!Kind::ListBox.is_focusable_by_default());
        assert_eq!(
            Kind::all()
                .iter()
                .filter(|k| k.is_focusable_by_default())
                .count(),
            27
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::tests::every_kind`
Expected: FAIL with `cannot find type 'Kind' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/mod.rs`:

```rust
/// One variant per in-scope widget, plus the sub-kinds GTK renders as their
/// own CSS node. Declaration order is contract §5's catalogue order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    // P5 · display
    Label, Spinner, Statusbar, LevelBar, ProgressBar, InfoBar, Scrollbar,
    Image, Picture, Separator, TextView, Scale, DrawingArea, WindowControls,
    Calendar, Popover,
    // P5 · buttons
    Button, ToggleButton, LinkButton, CheckButton, MenuButton, Switch,
    DropDown, ColorDialogButton, ColorDialog, FontDialogButton, FontDialog,
    // P5 · entries
    Entry, SearchEntry, PasswordEntry, SpinButton, EditableLabel,
    // P6 · containers
    Box, Grid, CenterBox, ScrolledWindow, Paned, Frame, Expander, SearchBar,
    ActionBar, HeaderBar, Notebook, NotebookTab, Overlay, Stack, StackPage,
    StackSwitcher, StackSidebar,
    // P6 · lists
    ListBox, ListBoxRow, FlowBox, FlowBoxChild, ListView, GridView,
    ColumnView, ColumnViewColumn,
    // P6 · menus
    PopoverMenu, PopoverMenuBar, PopoverMenuItem,
    // P6 · windows
    Window, ShortcutsWindow, AboutDialog, AlertDialog,
}

impl Kind {
    /// The CSS node name this kind's **root** node carries.
    ///
    /// Several kinds share one (`Button`/`ToggleButton`/`LinkButton` →
    /// `button`; `Box`/`CenterBox` → `box`; the four window kinds →
    /// `window`), so node-tree fixtures key on `Kind`, never on this string.
    ///
    /// Contract deviation D15: §5 names no node for `StackPage` and
    /// `ColumnViewColumn` (GTK renders neither as a node of its own), so P4
    /// pins `stackpage` and `button`; P6 may amend via §10.
    #[must_use]
    pub fn css_name(self) -> &'static str {
        match self {
            Kind::Label => "label",
            Kind::Spinner => "spinner",
            Kind::Statusbar => "statusbar",
            Kind::LevelBar => "levelbar",
            Kind::ProgressBar => "progressbar",
            Kind::InfoBar => "infobar",
            Kind::Scrollbar => "scrollbar",
            Kind::Image => "image",
            Kind::Picture => "picture",
            Kind::Separator => "separator",
            Kind::TextView => "textview",
            Kind::Scale => "scale",
            // GTK sets no CSS name on GtkDrawingArea; the node is
            // GtkWidget's default.
            Kind::DrawingArea => "widget",
            Kind::WindowControls => "windowcontrols",
            Kind::Calendar => "calendar",
            Kind::Popover | Kind::PopoverMenu => "popover",
            Kind::Button | Kind::ToggleButton | Kind::LinkButton => "button",
            Kind::CheckButton => "checkbutton",
            Kind::MenuButton => "menubutton",
            Kind::Switch => "switch",
            Kind::DropDown => "dropdown",
            Kind::ColorDialogButton => "colorbutton",
            Kind::FontDialogButton => "fontbutton",
            Kind::ColorDialog
            | Kind::FontDialog
            | Kind::Window
            | Kind::ShortcutsWindow
            | Kind::AboutDialog
            | Kind::AlertDialog => "window",
            Kind::Entry | Kind::SearchEntry | Kind::PasswordEntry => "entry",
            Kind::SpinButton => "spinbutton",
            Kind::EditableLabel => "editablelabel",
            Kind::Box | Kind::CenterBox => "box",
            Kind::Grid => "grid",
            Kind::ScrolledWindow => "scrolledwindow",
            Kind::Paned => "paned",
            Kind::Frame => "frame",
            Kind::Expander => "expander-widget",
            Kind::SearchBar => "searchbar",
            Kind::ActionBar => "actionbar",
            Kind::HeaderBar => "headerbar",
            Kind::Notebook => "notebook",
            Kind::NotebookTab => "tab",
            Kind::Overlay => "overlay",
            Kind::Stack => "stack",
            Kind::StackPage => "stackpage",
            Kind::StackSwitcher => "stackswitcher",
            Kind::StackSidebar => "stacksidebar",
            Kind::ListBox => "list",
            Kind::ListBoxRow => "row",
            Kind::FlowBox => "flowbox",
            Kind::FlowBoxChild => "flowboxchild",
            Kind::ListView => "listview",
            Kind::GridView => "gridview",
            Kind::ColumnView => "columnview",
            Kind::ColumnViewColumn | Kind::PopoverMenuItem => "button",
            Kind::PopoverMenuBar => "menubar",
        }
    }

    /// Style classes the kind always adds, on top of `css_name`.
    #[must_use]
    pub fn base_classes(self) -> &'static [&'static str] {
        match self {
            Kind::ToggleButton => &["toggle"],
            Kind::LinkButton => &["link"],
            Kind::SearchEntry => &["search"],
            Kind::PasswordEntry => &["password"],
            Kind::ColorDialog | Kind::FontDialog => &["dialog"],
            Kind::AlertDialog => &["dialog", "message"],
            Kind::Window => &["background"],
            Kind::ShortcutsWindow => &["shortcuts"],
            Kind::AboutDialog => &["aboutdialog"],
            Kind::Popover => &["background"],
            Kind::PopoverMenu => &["background", "menu"],
            Kind::PopoverMenuItem => &["model"],
            Kind::StackSwitcher => &["stack-switcher"],
            Kind::StackSidebar => &["sidebar"],
            Kind::Calendar => &["view"],
            _ => &[],
        }
    }

    /// GTK's `focusable` default for this widget class.
    ///
    /// A `View` may override it with [`View::focusable`]; this is only the
    /// starting value the controller writes when the node is built.
    #[must_use]
    pub fn is_focusable_by_default(self) -> bool {
        matches!(
            self,
            Kind::Button
                | Kind::ToggleButton
                | Kind::LinkButton
                | Kind::CheckButton
                | Kind::MenuButton
                | Kind::Switch
                | Kind::DropDown
                | Kind::ColorDialogButton
                | Kind::FontDialogButton
                | Kind::Entry
                | Kind::SearchEntry
                | Kind::PasswordEntry
                | Kind::SpinButton
                | Kind::EditableLabel
                | Kind::TextView
                | Kind::Scale
                | Kind::Calendar
                | Kind::Expander
                | Kind::Paned
                | Kind::Notebook
                | Kind::NotebookTab
                | Kind::ListBoxRow
                | Kind::FlowBoxChild
                | Kind::ListView
                | Kind::GridView
                | Kind::ColumnView
                | Kind::PopoverMenuItem
        )
    }

    /// Every kind, in declaration order.
    #[must_use]
    pub fn all() -> &'static [Kind] {
        &[
            Kind::Label, Kind::Spinner, Kind::Statusbar, Kind::LevelBar,
            Kind::ProgressBar, Kind::InfoBar, Kind::Scrollbar, Kind::Image,
            Kind::Picture, Kind::Separator, Kind::TextView, Kind::Scale,
            Kind::DrawingArea, Kind::WindowControls, Kind::Calendar, Kind::Popover,
            Kind::Button, Kind::ToggleButton, Kind::LinkButton, Kind::CheckButton,
            Kind::MenuButton, Kind::Switch, Kind::DropDown, Kind::ColorDialogButton,
            Kind::ColorDialog, Kind::FontDialogButton, Kind::FontDialog,
            Kind::Entry, Kind::SearchEntry, Kind::PasswordEntry, Kind::SpinButton,
            Kind::EditableLabel,
            Kind::Box, Kind::Grid, Kind::CenterBox, Kind::ScrolledWindow,
            Kind::Paned, Kind::Frame, Kind::Expander, Kind::SearchBar,
            Kind::ActionBar, Kind::HeaderBar, Kind::Notebook, Kind::NotebookTab,
            Kind::Overlay, Kind::Stack, Kind::StackPage, Kind::StackSwitcher,
            Kind::StackSidebar,
            Kind::ListBox, Kind::ListBoxRow, Kind::FlowBox, Kind::FlowBoxChild,
            Kind::ListView, Kind::GridView, Kind::ColumnView, Kind::ColumnViewColumn,
            Kind::PopoverMenu, Kind::PopoverMenuBar, Kind::PopoverMenuItem,
            Kind::Window, Kind::ShortcutsWindow, Kind::AboutDialog, Kind::AlertDialog,
        ]
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: PASS — 10 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `kinds_that_share_a_css_name_…`).** Change
`Kind::ToggleButton`'s `base_classes` arm to `&[]`; the test must FAIL.
Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/mod.rs
git commit -m "feat(ui): the 64 widget Kinds and their CSS node identity

Kind::{css_name, base_classes, is_focusable_by_default, all} per contract
§4.2/§5. Deviation D14 pins 64, not R1's '~59'; deviation D15 fixes the
node names §5 leaves unnamed for StackPage and ColumnViewColumn.

Mutation check: emptying ToggleButton's base_classes fails
kinds_that_share_a_css_name_are_told_apart_by_their_base_classes.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3: `Key`, `EventKind`, `Handler`, `Handlers`

**Files:**
- Modify: `ui/src/view/mod.rs` (append after `Kind`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/mod.rs`

**Interfaces:**
- Consumes: `crate::window::keyboard::KeyEvent` (P3, contract §3.3).
- Produces:
  ```rust
  pub enum Key { Index(usize), Id(u64), Name(Rc<str>) }
  impl From<usize> for Key; impl From<u64> for Key; impl From<&str> for Key;
  pub enum EventKind { /* 18 variants, §4.4 order */ }
  pub enum Handler<Msg> {
      Unit(Msg), Text(Rc<dyn Fn(&str) -> Msg>), Bool(Rc<dyn Fn(bool) -> Msg>),
      Index(Rc<dyn Fn(usize) -> Msg>), Float(Rc<dyn Fn(f64) -> Msg>),
      Key(Rc<dyn Fn(&KeyEvent) -> Option<Msg>>),
  }
  pub struct Handlers<Msg>(Vec<(EventKind, Handler<Msg>)>);
  impl<Msg: Clone + 'static> Handlers<Msg> {
      pub fn set(&mut self, kind: EventKind, handler: Handler<Msg>);
      pub fn fire_unit(&self, kind: EventKind) -> Option<Msg>;
      pub fn fire_text(&self, kind: EventKind, value: &str) -> Option<Msg>;
      pub fn fire_bool(&self, kind: EventKind, value: bool) -> Option<Msg>;
      pub fn fire_index(&self, kind: EventKind, value: usize) -> Option<Msg>;
      pub fn fire_float(&self, kind: EventKind, value: f64) -> Option<Msg>;
      pub fn fire_key(&self, kind: EventKind, ev: &KeyEvent) -> Option<Msg>;
      pub fn has(&self, kind: EventKind) -> bool;
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
  }
  ```

Note: `Handlers::fire_key` is additive (contract §4.4 lists five `fire_*`
methods but declares a `Handler::Key` variant with no way to fire it; adding
the sixth is a private-item addition permitted by the contract preamble).

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/mod.rs`'s test module:

```rust
    #[derive(Debug, Clone, PartialEq)]
    enum TestMsg {
        Ok,
        Named(String),
        Toggled(bool),
        Picked(usize),
        Moved(u64),
    }

    #[test]
    fn keys_convert_from_the_three_identity_shapes() {
        assert_eq!(Key::from(3usize), Key::Index(3));
        assert_eq!(Key::from(9_u64), Key::Id(9));
        assert_eq!(Key::from("row-a"), Key::Name(Rc::from("row-a")));
        // Distinct constructors never collide, even at the same number.
        assert_ne!(Key::from(3usize), Key::from(3_u64));
    }

    #[test]
    fn a_unit_handler_clones_its_message_on_every_fire() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(EventKind::Click, Handler::Unit(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Activate), None);
    }

    #[test]
    fn typed_fires_only_match_their_own_handler_variant() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(
            EventKind::Change,
            Handler::Text(Rc::new(|s: &str| TestMsg::Named(s.to_owned()))),
        );
        handlers.set(
            EventKind::Toggle,
            Handler::Bool(Rc::new(TestMsg::Toggled)),
        );
        handlers.set(
            EventKind::Selected,
            Handler::Index(Rc::new(TestMsg::Picked)),
        );
        handlers.set(
            EventKind::ValueChanged,
            Handler::Float(Rc::new(|v: f64| TestMsg::Moved(v as u64))),
        );

        assert_eq!(
            handlers.fire_text(EventKind::Change, "hi"),
            Some(TestMsg::Named("hi".into()))
        );
        assert_eq!(handlers.fire_bool(EventKind::Toggle, true), Some(TestMsg::Toggled(true)));
        assert_eq!(handlers.fire_index(EventKind::Selected, 2), Some(TestMsg::Picked(2)));
        assert_eq!(handlers.fire_float(EventKind::ValueChanged, 5.0), Some(TestMsg::Moved(5)));

        // A mismatched fire is a controller bug and yields nothing rather
        // than firing the wrong message.
        assert_eq!(handlers.fire_unit(EventKind::Change), None);
        assert_eq!(handlers.fire_bool(EventKind::Change, true), None);
    }

    #[test]
    fn setting_the_same_event_twice_replaces_rather_than_stacks() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(EventKind::Click, Handler::Unit(TestMsg::Ok));
        handlers.set(
            EventKind::Click,
            Handler::Unit(TestMsg::Named("second".into())),
        );
        assert_eq!(handlers.len(), 1);
        assert_eq!(
            handlers.fire_unit(EventKind::Click),
            Some(TestMsg::Named("second".into()))
        );
        assert!(handlers.has(EventKind::Click));
        assert!(!handlers.has(EventKind::Close));
    }

    #[test]
    fn the_event_kind_table_is_the_contract_s_eighteen() {
        assert_eq!(EventKind::ALL.len(), 18);
        assert_eq!(EventKind::ALL[0], EventKind::Click);
        assert_eq!(EventKind::ALL[17], EventKind::DateSelected);
        let mut sorted = EventKind::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 18);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::tests::keys_convert`
Expected: FAIL with `cannot find type 'Key' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/mod.rs`:

```rust
/// A child's identity across frames.
///
/// Identity is what keeps a row's animation, focus, shaping cache and
/// attached popup alive when the list around it is edited. The three
/// constructors are deliberately distinct variants: `Key::from(3usize)` and
/// `Key::from(3u64)` are different keys, so a positional index can never
/// alias a model id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A positional index the caller assigned explicitly.
    Index(usize),
    /// A model identity.
    Id(u64),
    /// A name.
    Name(Rc<str>),
}

impl From<usize> for Key {
    fn from(value: usize) -> Self {
        Key::Index(value)
    }
}
impl From<u64> for Key {
    fn from(value: u64) -> Self {
        Key::Id(value)
    }
}
impl From<&str> for Key {
    fn from(value: &str) -> Self {
        Key::Name(Rc::from(value))
    }
}
impl From<String> for Key {
    fn from(value: String) -> Self {
        Key::Name(Rc::from(value.as_str()))
    }
}

/// The events a widget can produce a message from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum EventKind {
    Click, Activate, Toggle, Change, Selected, ValueChanged, PageChanged,
    Expanded, Scrolled, Close, Response, FocusIn, FocusOut, KeyPressed,
    ActivateLink, Reordered, Search, DateSelected,
}

impl EventKind {
    /// Every event kind, in declaration order.
    pub const ALL: &'static [EventKind] = &[
        EventKind::Click, EventKind::Activate, EventKind::Toggle,
        EventKind::Change, EventKind::Selected, EventKind::ValueChanged,
        EventKind::PageChanged, EventKind::Expanded, EventKind::Scrolled,
        EventKind::Close, EventKind::Response, EventKind::FocusIn,
        EventKind::FocusOut, EventKind::KeyPressed, EventKind::ActivateLink,
        EventKind::Reordered, EventKind::Search, EventKind::DateSelected,
    ];
}

/// How one event turns into a message.
///
/// Contract deviation D13: the six variants are §4.4's, verbatim. §5's
/// Calendar (`(i32, u32, u32)`) and Notebook (`(usize, usize)`) handler
/// shapes have no variant to live in, so `on_date_selected` uses `Text` with
/// an ISO-8601 `YYYY-MM-DD` payload and `on_reordered` uses `Index` with the
/// destination index.
pub enum Handler<Msg> {
    /// A constant message, cloned on every fire.
    Unit(Msg),
    /// From the widget's text.
    Text(Rc<dyn Fn(&str) -> Msg>),
    /// From a boolean state.
    Bool(Rc<dyn Fn(bool) -> Msg>),
    /// From an index into a model or a page list.
    Index(Rc<dyn Fn(usize) -> Msg>),
    /// From a numeric value.
    Float(Rc<dyn Fn(f64) -> Msg>),
    /// From a key event; `None` means "not mine, keep bubbling".
    Key(Rc<dyn Fn(&crate::window::keyboard::KeyEvent) -> Option<Msg>>),
}

impl<Msg> Clone for Handler<Msg>
where
    Msg: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Handler::Unit(msg) => Handler::Unit(msg.clone()),
            Handler::Text(f) => Handler::Text(Rc::clone(f)),
            Handler::Bool(f) => Handler::Bool(Rc::clone(f)),
            Handler::Index(f) => Handler::Index(Rc::clone(f)),
            Handler::Float(f) => Handler::Float(Rc::clone(f)),
            Handler::Key(f) => Handler::Key(Rc::clone(f)),
        }
    }
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for Handler<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Handler::Unit(msg) => f.debug_tuple("Unit").field(msg).finish(),
            Handler::Text(_) => f.write_str("Text(..)"),
            Handler::Bool(_) => f.write_str("Bool(..)"),
            Handler::Index(_) => f.write_str("Index(..)"),
            Handler::Float(_) => f.write_str("Float(..)"),
            Handler::Key(_) => f.write_str("Key(..)"),
        }
    }
}

/// A widget's event-to-message bindings, sorted by [`EventKind`].
///
/// The reconciler replaces the whole set every frame (contract §4.7):
/// handlers close over the current model, so diffing them would be both
/// impossible and pointless.
pub struct Handlers<Msg>(Vec<(EventKind, Handler<Msg>)>);

impl<Msg> Default for Handlers<Msg> {
    // Not `#[derive]`: that would demand `Msg: Default`.
    fn default() -> Self {
        Handlers(Vec::new())
    }
}

impl<Msg: Clone> Clone for Handlers<Msg> {
    fn clone(&self) -> Self {
        Handlers(self.0.clone())
    }
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for Handlers<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl<Msg: Clone + 'static> Handlers<Msg> {
    /// Bind `kind`, replacing any previous binding.
    pub fn set(&mut self, kind: EventKind, handler: Handler<Msg>) {
        match self.0.binary_search_by_key(&kind, |(k, _)| *k) {
            Ok(index) => self.0[index].1 = handler,
            Err(index) => self.0.insert(index, (kind, handler)),
        }
    }

    /// Whether `kind` is bound at all.
    #[must_use]
    pub fn has(&self, kind: EventKind) -> bool {
        self.get(kind).is_some()
    }

    /// How many events are bound.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, kind: EventKind) -> Option<&Handler<Msg>> {
        self.0
            .binary_search_by_key(&kind, |(k, _)| *k)
            .ok()
            .map(|index| &self.0[index].1)
    }

    /// Fire a parameterless event. `None` when nothing is bound, or when
    /// what is bound wants a value this call cannot supply.
    #[must_use]
    pub fn fire_unit(&self, kind: EventKind) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a text event.
    #[must_use]
    pub fn fire_text(&self, kind: EventKind, value: &str) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Text(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a boolean event.
    #[must_use]
    pub fn fire_bool(&self, kind: EventKind, value: bool) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Bool(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire an index event.
    #[must_use]
    pub fn fire_index(&self, kind: EventKind, value: usize) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Index(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a numeric event.
    #[must_use]
    pub fn fire_float(&self, kind: EventKind, value: f64) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Float(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a key event. `None` both when nothing is bound and when the
    /// bound closure declined the key, so the caller keeps bubbling.
    #[must_use]
    pub fn fire_key(
        &self,
        kind: EventKind,
        ev: &crate::window::keyboard::KeyEvent,
    ) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Key(f) => f(ev),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }
}
```

Note the deliberate asymmetry: every typed `fire_*` accepts a `Handler::Unit`
(a caller that bound `.on_change(Msg::Dirty)` wants the message regardless of
the text), but `fire_unit` refuses a typed handler, because there is no value
to hand it. The test pins both directions.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: PASS — 15 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.

**Mutation check (load-bearing: `typed_fires_only_match_their_own_handler_variant`).**
Change `fire_unit`'s `_ => None` arm to also accept `Handler::Text(f) =>
Some(f(""))`; the test's `assert_eq!(handlers.fire_unit(EventKind::Change),
None)` must FAIL. Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/mod.rs
git commit -m "feat(ui): keys, event kinds and typed message handlers

Key's three non-aliasing identity shapes, EventKind's 18 names, Handler's
six payload shapes and the sorted Handlers map with its fire_* entry
points. fire_key is the sixth fire (§4.4 declared Handler::Key with no
way to fire it); deviation D13 records how Calendar's and Notebook's
tuple payloads map onto Text and Index.

Mutation check: letting fire_unit accept a Text handler fails
typed_fires_only_match_their_own_handler_variant.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 4: `View` and its universal setters

**Files:**
- Modify: `ui/src/view/mod.rs` (append after `Handlers`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/mod.rs`

**Interfaces:**
- Consumes: `Kind`, `Key`, `Props`, `PropName`, `Prop`, `Handlers`,
  `Handler`, `EventKind` (Tasks 1–3); `crate::layout::Align` (Task 1).
- Produces:
  ```rust
  pub struct View<Msg> {
      pub kind: Kind, pub key: Option<Key>, pub props: Props,
      pub children: Vec<View<Msg>>, pub handlers: Handlers<Msg>,
  }
  impl<Msg: Clone + 'static> View<Msg> {
      pub fn new(kind: Kind) -> Self;
      pub fn key(self, key: impl Into<Key>) -> Self;
      pub fn child(self, child: View<Msg>) -> Self;
      pub fn children(self, children: impl IntoIterator<Item = View<Msg>>) -> Self;
      pub fn prop(self, name: PropName, value: impl Into<Prop>) -> Self;
      pub fn on(self, event: EventKind, handler: Handler<Msg>) -> Self;
      pub fn class(self, class: &str) -> Self;
      pub fn classes(self, classes: &[&str]) -> Self;
      pub fn id(self, id: &str) -> Self;
      pub fn visible(self, on: bool) -> Self;
      pub fn sensitive(self, on: bool) -> Self;
      pub fn focusable(self, on: bool) -> Self;
      pub fn tooltip(self, text: &str) -> Self;
      pub fn halign(self, a: Align) -> Self;
      pub fn valign(self, a: Align) -> Self;
      pub fn hexpand(self, on: bool) -> Self;
      pub fn vexpand(self, on: bool) -> Self;
      pub fn margin(self, top: i32, right: i32, bottom: i32, left: i32) -> Self;
      pub fn width_request(self, px: i32) -> Self;
      pub fn height_request(self, px: i32) -> Self;
      pub fn cursor(self, name: &str) -> Self;
      pub fn opacity(self, value: f64) -> Self;
  }
  ```

`View::opacity` is additive: §4.3 lists `PropName::Opacity` in the universal
set and §5 says every widget inherits `opacity`, but §4.1's setter list omits
it. Adding the setter is a private-item addition, not a signature change.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/mod.rs`'s test module:

```rust
    #[test]
    fn a_new_view_carries_only_its_kind() {
        let v: View<TestMsg> = View::new(Kind::Button);
        assert_eq!(v.kind, Kind::Button);
        assert_eq!(v.key, None);
        assert!(v.props.is_empty());
        assert!(v.children.is_empty());
        assert!(v.handlers.is_empty());
    }

    #[test]
    fn universal_setters_write_the_props_they_are_named_for() {
        let v: View<TestMsg> = View::new(Kind::Label)
            .id("title")
            .classes(&["heading", "dim-label"])
            .class("extra")
            .visible(false)
            .sensitive(false)
            .focusable(true)
            .tooltip("a tip")
            .halign(crate::layout::Align::Start)
            .valign(crate::layout::Align::Center)
            .hexpand(true)
            .vexpand(false)
            .margin(1, 2, 3, 4)
            .width_request(120)
            .height_request(24)
            .cursor("pointer")
            .opacity(0.5);

        assert_eq!(v.props.str(PropName::Id), Some("title"));
        let Some(Prop::Classes(classes)) = v.props.get(PropName::Classes) else {
            panic!("classes were not stored as Prop::Classes");
        };
        assert_eq!(
            classes.iter().map(|c| &**c).collect::<Vec<_>>(),
            vec!["heading", "dim-label", "extra"],
            "`class` appends to `classes`, in call order"
        );
        assert!(!v.props.bool(PropName::Visible, true));
        assert!(!v.props.bool(PropName::Sensitive, true));
        assert!(v.props.bool(PropName::Focusable, false));
        assert_eq!(v.props.str(PropName::Tooltip), Some("a tip"));
        assert_eq!(
            v.props.get(PropName::Halign),
            Some(&Prop::Align(crate::layout::Align::Start))
        );
        assert_eq!(
            v.props.get(PropName::Valign),
            Some(&Prop::Align(crate::layout::Align::Center))
        );
        assert!(v.props.bool(PropName::Hexpand, false));
        assert!(!v.props.bool(PropName::Vexpand, true));
        assert_eq!(v.props.get(PropName::Margin), Some(&Prop::Edges([1, 2, 3, 4])));
        assert_eq!(v.props.int(PropName::WidthRequest, 0), 120);
        assert_eq!(v.props.int(PropName::HeightRequest, 0), 24);
        assert_eq!(v.props.str(PropName::Cursor), Some("pointer"));
        assert!((v.props.float(PropName::Opacity, 1.0) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn children_and_keys_chain_in_order() {
        let v: View<TestMsg> = View::new(Kind::Box)
            .child(View::new(Kind::Label).key("a"))
            .children([
                View::new(Kind::Button).key("b"),
                View::new(Kind::Button).key(7_u64),
            ]);
        assert_eq!(v.children.len(), 3);
        assert_eq!(v.children[0].key, Some(Key::Name(Rc::from("a"))));
        assert_eq!(v.children[1].key, Some(Key::Name(Rc::from("b"))));
        assert_eq!(v.children[2].key, Some(Key::Id(7)));
        assert_eq!(v.children[2].kind, Kind::Button);
    }

    #[test]
    fn on_binds_a_handler_the_view_can_fire() {
        let v: View<TestMsg> =
            View::new(Kind::Button).on(EventKind::Click, Handler::Unit(TestMsg::Ok));
        assert_eq!(v.handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
    }

    #[test]
    fn setting_a_universal_prop_twice_keeps_the_last_value() {
        let v: View<TestMsg> = View::new(Kind::Label).tooltip("first").tooltip("second");
        assert_eq!(v.props.str(PropName::Tooltip), Some("second"));
        assert_eq!(v.props.len(), 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::tests::a_new_view`
Expected: FAIL with `cannot find type 'View' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/mod.rs`:

```rust
/// An immutable description of one widget and its subtree.
///
/// A `view(&Model) -> View<Msg>` is pure: it allocates a fresh tree every
/// frame and the reconciler (`view::reconcile`) diffs it into the retained
/// [`Node`](crate::css::node::Node)s, so identity — animation, focus,
/// shaping caches, attached popups — lives in the `Instance` tree, never here.
pub struct View<Msg> {
    /// Which widget.
    pub kind: Kind,
    /// The identity that survives an edit of the surrounding list.
    pub key: Option<Key>,
    /// Typed properties.
    pub props: Props,
    /// Child views, in order.
    pub children: Vec<View<Msg>>,
    /// Event-to-message bindings.
    pub handlers: Handlers<Msg>,
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for View<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("props", &self.props)
            .field("handlers", &self.handlers)
            .field("children", &self.children)
            .finish()
    }
}

impl<Msg: Clone + 'static> View<Msg> {
    /// An empty view of `kind`.
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        View {
            kind,
            key: None,
            props: Props::default(),
            children: Vec::new(),
            handlers: Handlers::default(),
        }
    }

    /// Give this view an identity for keyed reconciliation.
    #[must_use]
    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Append one child.
    #[must_use]
    pub fn child(mut self, child: View<Msg>) -> Self {
        self.children.push(child);
        self
    }

    /// Append many children, in iteration order.
    #[must_use]
    pub fn children(mut self, children: impl IntoIterator<Item = View<Msg>>) -> Self {
        self.children.extend(children);
        self
    }

    /// Set one property.
    #[must_use]
    pub fn prop(mut self, name: PropName, value: impl Into<Prop>) -> Self {
        self.props.set(name, value.into());
        self
    }

    /// Bind one event.
    #[must_use]
    pub fn on(mut self, event: EventKind, handler: Handler<Msg>) -> Self {
        self.handlers.set(event, handler);
        self
    }

    /// Append one CSS class, after anything `classes` already added.
    #[must_use]
    pub fn class(mut self, class: &str) -> Self {
        let mut list: Vec<Rc<str>> = match self.props.get(PropName::Classes) {
            Some(Prop::Classes(existing)) => existing.to_vec(),
            _ => Vec::new(),
        };
        list.push(Rc::from(class));
        self.props
            .set(PropName::Classes, Prop::Classes(Rc::from(list)));
        self
    }

    /// Append several CSS classes, in order.
    #[must_use]
    pub fn classes(mut self, classes: &[&str]) -> Self {
        let mut list: Vec<Rc<str>> = match self.props.get(PropName::Classes) {
            Some(Prop::Classes(existing)) => existing.to_vec(),
            _ => Vec::new(),
        };
        list.extend(classes.iter().map(|c| Rc::from(*c)));
        self.props
            .set(PropName::Classes, Prop::Classes(Rc::from(list)));
        self
    }

    /// Set the CSS id.
    #[must_use]
    pub fn id(self, id: &str) -> Self {
        self.prop(PropName::Id, id)
    }

    /// GTK's `visible`. An invisible widget keeps its `Node` and its
    /// controller but is skipped by layout, paint and hit-testing.
    #[must_use]
    pub fn visible(self, on: bool) -> Self {
        self.prop(PropName::Visible, on)
    }

    /// GTK's `sensitive`. Insensitive sets `PseudoStates::DISABLED` and
    /// takes the node out of the focus ring and the hit-test.
    #[must_use]
    pub fn sensitive(self, on: bool) -> Self {
        self.prop(PropName::Sensitive, on)
    }

    /// Override [`Kind::is_focusable_by_default`].
    #[must_use]
    pub fn focusable(self, on: bool) -> Self {
        self.prop(PropName::Focusable, on)
    }

    /// Tooltip text.
    #[must_use]
    pub fn tooltip(self, text: &str) -> Self {
        self.prop(PropName::Tooltip, text)
    }

    /// Horizontal alignment within the parent's allocation.
    #[must_use]
    pub fn halign(self, a: Align) -> Self {
        self.prop(PropName::Halign, a)
    }

    /// Vertical alignment within the parent's allocation.
    #[must_use]
    pub fn valign(self, a: Align) -> Self {
        self.prop(PropName::Valign, a)
    }

    /// Take any extra horizontal space the parent has.
    #[must_use]
    pub fn hexpand(self, on: bool) -> Self {
        self.prop(PropName::Hexpand, on)
    }

    /// Take any extra vertical space the parent has.
    #[must_use]
    pub fn vexpand(self, on: bool) -> Self {
        self.prop(PropName::Vexpand, on)
    }

    /// Margins, in px, in CSS order.
    #[must_use]
    pub fn margin(self, top: i32, right: i32, bottom: i32, left: i32) -> Self {
        self.prop(PropName::Margin, Prop::Edges([top, right, bottom, left]))
    }

    /// GTK's `width-request` — a minimum, not a fixed size.
    #[must_use]
    pub fn width_request(self, px: i32) -> Self {
        self.prop(PropName::WidthRequest, px)
    }

    /// GTK's `height-request`.
    #[must_use]
    pub fn height_request(self, px: i32) -> Self {
        self.prop(PropName::HeightRequest, px)
    }

    /// A CSS `cursor` keyword; mapped to a `wp_cursor_shape_v1` name by
    /// `window::pointer::cursor_shape_for`.
    #[must_use]
    pub fn cursor(self, name: &str) -> Self {
        self.prop(PropName::Cursor, name)
    }

    /// Widget opacity, `0.0..=1.0`, applied as an inline style override.
    #[must_use]
    pub fn opacity(self, value: f64) -> Self {
        self.prop(PropName::Opacity, value.clamp(0.0, 1.0))
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: PASS — 20 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`universal_setters_write_the_props_they_are_named_for`).** Make `View::class`
overwrite (`Prop::Classes(Rc::from(vec![Rc::from(class)]))`) instead of
appending; the `["heading", "dim-label", "extra"]` assertion must FAIL.
Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/mod.rs
git commit -m "feat(ui): View and the GtkWidget-universal setters

View<Msg> plus §4.1's chained setters, with class/classes appending in
call order and opacity added (§4.3 lists PropName::Opacity and §5 says
every widget inherits it, but §4.1's setter list omits it).

Mutation check: making class() overwrite rather than append fails
universal_setters_write_the_props_they_are_named_for.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 5: `view/builders.rs` — the builder frame

**Files:**
- Create: `ui/src/view/builders.rs`
- Modify: `ui/src/view/mod.rs` (add `pub mod builders;` and re-exports)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/builders.rs`

**Interfaces:**
- Consumes: `View`, `Kind`, `EventKind`, `Handler`, `Msg: Clone + 'static`
  (Tasks 2–4); `crate::window::keyboard::KeyEvent` (P3).
- Produces:
  ```rust
  pub fn widget<Msg: Clone + 'static>(kind: Kind) -> View<Msg>;
  impl<Msg: Clone + 'static> View<Msg> {
      pub fn on_click(self, msg: Msg) -> Self;
      pub fn on_activate(self, msg: Msg) -> Self;
      pub fn on_close(self, msg: Msg) -> Self;
      pub fn on_focus_in(self, msg: Msg) -> Self;
      pub fn on_focus_out(self, msg: Msg) -> Self;
      pub fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
      pub fn on_expanded(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
      pub fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      pub fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      pub fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      pub fn on_date_selected(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      pub fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      pub fn on_page_changed(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      pub fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      pub fn on_reordered(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      pub fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
      pub fn on_scrolled(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
      pub fn on_key(self, f: impl Fn(&KeyEvent) -> Option<Msg> + 'static) -> Self;
  }
  ```
  Eighteen setters, one per [`EventKind`].

- [ ] **Step 1: Write the failing test**

Create `ui/src/view/builders.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{EventKind, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Clicked,
        Text(String),
        Index(usize),
        Value(u64),
        Flag(bool),
    }

    #[test]
    fn widget_builds_a_bare_view_of_the_kind() {
        let v: View<Msg> = widget(Kind::Spinner);
        assert_eq!(v.kind, Kind::Spinner);
        assert!(v.props.is_empty());
    }

    #[test]
    fn every_event_kind_has_exactly_one_on_setter() {
        // The eighteen setters, each binding its own EventKind and nothing
        // else. A new EventKind without a setter fails this test.
        let bound: Vec<EventKind> = vec![
            widget::<Msg>(Kind::Button).on_click(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_activate(Msg::Clicked),
            widget::<Msg>(Kind::Popover).on_close(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_in(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_out(Msg::Clicked),
            widget::<Msg>(Kind::ToggleButton).on_toggle(Msg::Flag),
            widget::<Msg>(Kind::Expander).on_expanded(Msg::Flag),
            widget::<Msg>(Kind::Entry).on_change(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::SearchEntry).on_search(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::LinkButton).on_activate_link(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::Calendar).on_date_selected(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::DropDown).on_selected(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_page_changed(Msg::Index),
            widget::<Msg>(Kind::AlertDialog).on_response(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_reordered(Msg::Index),
            widget::<Msg>(Kind::Scale).on_value_changed(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::ScrolledWindow).on_scrolled(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::Entry).on_key(|_| Some(Msg::Clicked)),
        ]
        .into_iter()
        .map(|v| {
            assert_eq!(v.handlers.len(), 1, "a setter bound more than one event");
            *EventKind::ALL
                .iter()
                .find(|k| v.handlers.has(**k))
                .expect("the setter bound no event")
        })
        .collect();

        let mut sorted = bound.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            18,
            "two setters bound the same EventKind: {bound:?}"
        );
        assert_eq!(sorted, EventKind::ALL.to_vec());
    }

    #[test]
    fn the_typed_setters_carry_their_payload_through() {
        let v: View<Msg> = widget(Kind::Entry).on_change(|s| Msg::Text(s.to_owned()));
        assert_eq!(
            v.handlers.fire_text(EventKind::Change, "abc"),
            Some(Msg::Text("abc".into()))
        );

        let v: View<Msg> = widget(Kind::DropDown).on_selected(Msg::Index);
        assert_eq!(
            v.handlers.fire_index(EventKind::Selected, 4),
            Some(Msg::Index(4))
        );

        let v: View<Msg> = widget(Kind::Scale).on_value_changed(|x| Msg::Value(x as u64));
        assert_eq!(
            v.handlers.fire_float(EventKind::ValueChanged, 12.0),
            Some(Msg::Value(12))
        );
    }

    #[test]
    fn a_key_handler_that_declines_produces_nothing() {
        let v: View<Msg> = widget(Kind::Entry).on_key(|_| None);
        let ev = crate::window::keyboard::Keymap::from_string(
            include_str!("../../tests/fixtures/keymaps/us.xkb"),
        )
        .expect("the vendored us keymap compiles")
        .translate(38, true, 1, 0); // evdev 38 == `a`
        assert_eq!(v.handlers.fire_key(EventKind::KeyPressed, &ev), None);
    }

    #[test]
    fn builders_chain_with_the_universal_setters() {
        let v: View<Msg> = widget::<Msg>(Kind::Button)
            .class("suggested-action")
            .margin(0, 6, 0, 6)
            .on_click(Msg::Clicked)
            .key("ok");
        assert_eq!(v.kind, Kind::Button);
        assert_eq!(v.props.get(PropName::Margin), Some(&crate::view::Prop::Edges([0, 6, 0, 6])));
        assert_eq!(v.handlers.fire_unit(EventKind::Click), Some(Msg::Clicked));
        assert_eq!(v.key, Some(crate::view::Key::Name(std::rc::Rc::from("ok"))));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::builders`
Expected: FAIL — `file not found for module 'builders'` until `mod.rs`
declares it, then `cannot find function 'widget' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/view/mod.rs`, above the type definitions:

```rust
pub mod builders;

pub use builders::widget;
```

Write the body of `ui/src/view/builders.rs` above its test module:

```rust
//! Free-function widget builders and the event setters every one of them
//! chains.
//!
//! # The builder naming rule (normative — contract §4.4)
//!
//! P4 ships this frame; P5 and P6 fill it, and must follow the rule exactly.
//!
//! * A widget `Foo` gets **one** free constructor `fn foo(..) -> View<Msg>`
//!   in this module, named in `snake_case` from the GTK class name with the
//!   `Gtk` prefix dropped: `GtkCheckButton` → `check_button`,
//!   `GtkColorDialogButton` → `color_dialog_button`.
//! * Its required content is a **positional** argument: `button("Ok")`,
//!   `label("Hi")`, `scale(0.0, 100.0)`.
//! * Everything else is a chained `self`-consuming setter named after the
//!   **GTK property**, in `snake_case`, with no `set_` prefix:
//!   `.wrap(true)`, `.show_text(true)`, `.max_length(32)`.
//! * Handlers are `on_<eventkind snake_case>`, taking a `Msg` for
//!   [`Handler::Unit`] and a closure otherwise. All eighteen live in this
//!   file, on `View<Msg>` itself, so every builder inherits them.
//! * Every builder returns [`View<Msg>`], so §4.1's `.class()`, `.margin()`
//!   and `.key()` chain after it.

use crate::view::{EventKind, Handler, Kind, View};
use crate::window::keyboard::KeyEvent;
use std::rc::Rc;

/// A bare [`View`] of `kind`, with no props and no handlers.
///
/// The generic constructor every named builder in P5/P6 starts from, and the
/// only one P4 itself needs.
#[must_use]
pub fn widget<Msg: Clone + 'static>(kind: Kind) -> View<Msg> {
    View::new(kind)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// The widget was clicked.
    #[must_use]
    pub fn on_click(self, msg: Msg) -> Self {
        self.on(EventKind::Click, Handler::Unit(msg))
    }

    /// The widget was activated — Space/Enter, or a click that completed
    /// inside it.
    #[must_use]
    pub fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }

    /// A popover, dialog or window was closed.
    #[must_use]
    pub fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }

    /// The widget took the focus.
    #[must_use]
    pub fn on_focus_in(self, msg: Msg) -> Self {
        self.on(EventKind::FocusIn, Handler::Unit(msg))
    }

    /// The widget lost the focus.
    #[must_use]
    pub fn on_focus_out(self, msg: Msg) -> Self {
        self.on(EventKind::FocusOut, Handler::Unit(msg))
    }

    /// A toggle/check/switch changed state.
    #[must_use]
    pub fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }

    /// An expander opened or closed.
    #[must_use]
    pub fn on_expanded(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Expanded, Handler::Bool(Rc::new(f)))
    }

    /// Editable text changed.
    #[must_use]
    pub fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }

    /// A search entry's debounced text changed.
    #[must_use]
    pub fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Search, Handler::Text(Rc::new(f)))
    }

    /// A link button's URI was activated.
    #[must_use]
    pub fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }

    /// A calendar date was picked.
    ///
    /// Contract deviation D13: the payload is an ISO-8601 `YYYY-MM-DD`
    /// string, because §4.4's `Handler` has no `(i32, u32, u32)` variant.
    #[must_use]
    pub fn on_date_selected(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::DateSelected, Handler::Text(Rc::new(f)))
    }

    /// A row, item or dropdown entry was selected.
    #[must_use]
    pub fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }

    /// A notebook or stack switched page.
    #[must_use]
    pub fn on_page_changed(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::PageChanged, Handler::Index(Rc::new(f)))
    }

    /// A dialog button was chosen; the index is into the dialog's button list.
    #[must_use]
    pub fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }

    /// A reorderable child was dropped.
    ///
    /// Contract deviation D13: the payload is the **destination** index,
    /// because §4.4's `Handler` has no `(usize, usize)` variant.
    #[must_use]
    pub fn on_reordered(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Reordered, Handler::Index(Rc::new(f)))
    }

    /// A scale, scrollbar or spin button's value changed.
    #[must_use]
    pub fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }

    /// A scrolled window's offset changed; the payload is the vertical
    /// offset in px.
    #[must_use]
    pub fn on_scrolled(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::Scrolled, Handler::Float(Rc::new(f)))
    }

    /// A key reached this widget. Return `None` to let it keep bubbling to
    /// the window's focus navigation (contract §3.5).
    #[must_use]
    pub fn on_key(self, f: impl Fn(&KeyEvent) -> Option<Msg> + 'static) -> Self {
        self.on(EventKind::KeyPressed, Handler::Key(Rc::new(f)))
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::builders`
Expected: PASS — 5 tests.
Run: `cargo test -p icedtea-ui` — everything green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `every_event_kind_has_exactly_one_on_setter`).**
Change `on_search` to bind `EventKind::Change`; the test must FAIL with
"two setters bound the same EventKind". Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/builders.rs ui/src/view/mod.rs
git commit -m "feat(ui): the builder frame and eighteen event setters

view::builders carries the normative builder-naming rule P5/P6 follow,
the generic widget(kind) constructor, and one on_<event> setter per
EventKind on View<Msg> itself so every named builder inherits them.

Mutation check: pointing on_search at EventKind::Change fails
every_event_kind_has_exactly_one_on_setter.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 6: `IconTheme` — the constructors P4 needs (deviation D9)

**Files:**
- Create: `ui/src/icons/mod.rs`
- Create: `ui/src/icons/theme.rs`
- Modify: `ui/src/lib.rs` (add `pub mod icons;`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/icons/theme.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  ```rust
  pub struct IconTheme { /* private */ }
  impl IconTheme {
      pub fn from_env() -> Self;
      pub fn with_name_and_roots(name: &str, roots: Vec<PathBuf>) -> Self;
      pub fn name(&self) -> &str;
      pub fn chain(&self) -> &[Rc<str>];
      pub fn roots(&self) -> &[PathBuf];
      pub fn clear_caches(&mut self);
  }
  pub fn theme_name_from_settings_ini(text: &str) -> Option<String>;
  ```
  P7 adds `lookup`, `render`, `IconFile`, `IconFormat`, `DirKind`, `Palette`
  and the caches to this same module, and owns it from then on.

- [ ] **Step 1: Write the failing test**

Create `ui/src/icons/theme.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hermetic_theme_appends_hicolor_to_its_chain() {
        let theme = IconTheme::with_name_and_roots("Adwaita", vec![]);
        assert_eq!(theme.name(), "Adwaita");
        assert_eq!(
            theme.chain().iter().map(|s| &**s).collect::<Vec<_>>(),
            vec!["Adwaita", "hicolor"]
        );
        assert!(theme.roots().is_empty());
    }

    #[test]
    fn hicolor_is_never_appended_twice() {
        let theme = IconTheme::with_name_and_roots("hicolor", vec![]);
        assert_eq!(
            theme.chain().iter().map(|s| &**s).collect::<Vec<_>>(),
            vec!["hicolor"]
        );
    }

    #[test]
    fn a_blank_theme_name_falls_back_to_adwaita() {
        let theme = IconTheme::with_name_and_roots("   ", vec![]);
        assert_eq!(theme.name(), "Adwaita");
    }

    #[test]
    fn settings_ini_is_read_only_from_the_settings_section() {
        let ini = "[Other]\ngtk-icon-theme-name=Wrong\n\
                   [Settings]\ngtk-theme-name = Adwaita\n\
                   gtk-icon-theme-name = Papirus \n";
        assert_eq!(
            theme_name_from_settings_ini(ini).as_deref(),
            Some("Papirus")
        );
    }

    #[test]
    fn a_malformed_settings_ini_never_panics_and_yields_nothing() {
        // Untrusted input (contract "cross-cutting rules"): every one of
        // these is a real shape a hand-edited settings.ini can take.
        for text in [
            "",
            "\u{0}\u{1}\u{2}",
            "[Settings",
            "[Settings]\n=",
            "[Settings]\ngtk-icon-theme-name",
            "[Settings]\ngtk-icon-theme-name=",
            "[Settings]\ngtk-icon-theme-name=   ",
            "[Settings]\n=Papirus",
            "gtk-icon-theme-name=NoSection",
            "[Settings]\n\u{feff}gtk-icon-theme-name=Ok\u{0}",
            "[Settings]\ngtk-icon-theme-name=日本語のテーマ",
            &"[".repeat(10_000),
            &format!("[Settings]\ngtk-icon-theme-name={}", "x".repeat(100_000)),
        ] {
            let got = theme_name_from_settings_ini(text);
            match text {
                t if t.contains("=Ok") => assert_eq!(got.as_deref(), Some("Ok")),
                t if t.contains("日本語") => assert_eq!(got.as_deref(), Some("日本語のテーマ")),
                t if t.contains(&"x".repeat(64)) => assert_eq!(got.map(|s| s.len()), Some(100_000)),
                _ => assert_eq!(got, None, "unexpected name from {text:?}"),
            }
        }
    }

    #[test]
    fn clear_caches_keeps_the_identity_of_the_theme() {
        let mut theme = IconTheme::with_name_and_roots("Papirus", vec![
            std::path::PathBuf::from("/tmp/icons"),
        ]);
        theme.clear_caches();
        assert_eq!(theme.name(), "Papirus");
        assert_eq!(theme.roots().len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib icons::`
Expected: FAIL — `failed to resolve: use of undeclared crate or module 'icons'`,
then `cannot find type 'IconTheme' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/lib.rs`:

```rust
pub mod icons;
```

Create `ui/src/icons/mod.rs`:

```rust
//! Freedesktop icon themes: lookup, rendering, symbolic recolouring and
//! GTK's builtin shapes.
//!
//! P4 (contract deviation D9) ships only [`IconTheme`]'s construction and
//! inheritance chain, because `view::BuildCx`/`view::EventCx` carry an
//! `&mut IconTheme` and P4 executes before P7. P7 owns everything else in
//! this module: `lookup`, `render`, `IconFile`, `DirKind`, `Palette`, the
//! symbolic recolour path, the builtin shapes and the caches.

pub mod theme;

pub use theme::{IconTheme, theme_name_from_settings_ini};
```

Create the body of `ui/src/icons/theme.rs` above its test module:

```rust
//! The icon theme handle: which theme, where its roots are, and what it
//! inherits from.

use std::path::PathBuf;
use std::rc::Rc;

/// The theme GTK falls back to when `settings.ini` names none.
const DEFAULT_THEME: &str = "Adwaita";

/// The theme every chain ends at, per the Icon Theme Specification.
const FALLBACK_THEME: &str = "hicolor";

/// A resolved icon theme: its name, its search roots and its inheritance
/// chain.
///
/// P7 grows this with `lookup`, `render` and their caches; P4 needs only
/// enough for `BuildCx`/`EventCx` to carry one and for tests to build a
/// hermetic instance with [`IconTheme::with_name_and_roots`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconTheme {
    name: Rc<str>,
    roots: Vec<PathBuf>,
    chain: Vec<Rc<str>>,
}

impl IconTheme {
    /// Read the theme name from `$XDG_CONFIG_HOME/gtk-4.0/settings.ini`
    /// (falling back to `$HOME/.config/...`), defaulting to `Adwaita`, and
    /// build the standard root list: `$XDG_DATA_HOME/icons`, `$HOME/.icons`,
    /// each `$XDG_DATA_DIRS/icons`, then `/usr/share/pixmaps` last.
    ///
    /// A missing or unreadable `settings.ini` is not an error — it is the
    /// normal case on a machine with no GTK configuration.
    #[must_use]
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let config_home = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| PathBuf::from(h).join(".config")));

        let name = config_home
            .as_ref()
            .map(|dir| dir.join("gtk-4.0").join("settings.ini"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| theme_name_from_settings_ini(&text))
            .unwrap_or_else(|| DEFAULT_THEME.to_owned());

        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(data_home) = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| PathBuf::from(h).join(".local/share")))
        {
            roots.push(data_home.join("icons"));
        }
        if let Some(home) = home.as_ref() {
            roots.push(PathBuf::from(home).join(".icons"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        for dir in data_dirs.split(':').filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(dir).join("icons"));
        }
        // Flat, non-themed, last resort only.
        roots.push(PathBuf::from("/usr/share/pixmaps"));

        IconTheme::with_name_and_roots(&name, roots)
    }

    /// Hermetic construction: no environment is read at all.
    #[must_use]
    pub fn with_name_and_roots(name: &str, roots: Vec<PathBuf>) -> Self {
        let trimmed = name.trim();
        let name: Rc<str> = if trimmed.is_empty() {
            Rc::from(DEFAULT_THEME)
        } else {
            Rc::from(trimmed)
        };
        // P7 replaces this with the real `Inherits=` walk over each theme's
        // `index.theme`; the invariant it must preserve is the one pinned
        // here: the chain starts at `name` and ends at `hicolor`, once.
        let mut chain = vec![Rc::clone(&name)];
        if &*name != FALLBACK_THEME {
            chain.push(Rc::from(FALLBACK_THEME));
        }
        IconTheme { name, roots, chain }
    }

    /// The theme's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved inheritance chain: this theme first, `hicolor` last.
    #[must_use]
    pub fn chain(&self) -> &[Rc<str>] {
        &self.chain
    }

    /// The search roots, in priority order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Drop every memoized lookup and render.
    ///
    /// P4 holds no caches yet, so this is a no-op that P7 fills in; it exists
    /// now because `App` calls it when the theme changes.
    pub fn clear_caches(&mut self) {}
}

/// `gtk-icon-theme-name` from a GTK `settings.ini`, or `None`.
///
/// A hand-edited, truncated or non-UTF-8-intentioned file is untrusted
/// input: every malformed shape yields `None` and none of them panics.
/// Only the `[Settings]` section is consulted, keys are matched
/// ASCII-case-insensitively, and both key and value are trimmed.
#[must_use]
pub fn theme_name_from_settings_ini(text: &str) -> Option<String> {
    let mut in_settings = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            // An unterminated section header is malformed; treat it as
            // "not the Settings section" rather than guessing.
            in_settings = rest
                .strip_suffix(']')
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("Settings"));
            continue;
        }
        if !in_settings {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key
            .trim()
            .trim_start_matches('\u{feff}')
            .eq_ignore_ascii_case("gtk-icon-theme-name")
        {
            continue;
        }
        let value = value.trim().trim_matches('\u{0}').trim();
        if value.is_empty() {
            continue;
        }
        return Some(value.to_owned());
    }
    None
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib icons::`
Expected: PASS — 6 tests, including the never-panic sweep over thirteen
malformed `settings.ini` shapes.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`settings_ini_is_read_only_from_the_settings_section`).** Delete the
`if !in_settings { continue; }` guard; the test must FAIL, returning
`"Wrong"` from the `[Other]` section. Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/icons/mod.rs ui/src/icons/theme.rs ui/src/lib.rs
git commit -m "feat(ui): IconTheme construction and inheritance chain

Contract deviation D9: §0 gives icons/** to P7, but §4.5/§4.6 put
&mut IconTheme in BuildCx/EventCx and P4 runs first. P4 ships only
from_env/with_name_and_roots/name/chain/roots/clear_caches, all fully
implemented; P7 adds lookup, render, the caches and the rest.

settings.ini is untrusted input: thirteen malformed shapes are swept for
never-panic, including an unterminated section, a BOM'd key, a NUL-padded
value and a 100 kB name.

Mutation check: dropping the [Settings]-section guard makes the parser
answer from [Other] and fails settings_ini_is_read_only_from_the_settings_section.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 7: `NodeAddr`, `StyleMap` and `restyle_tree`

**Files:**
- Create: `ui/src/view/render.rs`
- Modify: `ui/src/view/mod.rs` (add `pub mod render;` and re-exports)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/render.rs`

**Interfaces:**
- Consumes: `crate::css::node::Node`; `crate::css::cascade::CompiledSheet`;
  `crate::css::computed::{ComputedStyle, ResolveEnv}`;
  `crate::css::select::MatchCx`; `crate::anim::{AnimationState, Overrides,
  Clock}`; `selectors::Element`.
- Produces (amended by contract §11 E2 — P3's three are canonical, P4
  re-exports rather than redeclares them):
  ```rust
  pub use crate::window::{NodeAddr, StyleMap, node_addr};
  pub struct Animations(HashMap<NodeAddr, AnimationState>);
  impl Animations {
      pub fn new() -> Self;
      pub fn sample(&mut self, node: &Node, now: Duration) -> Overrides;
      pub fn get(&self, node: &Node) -> Option<&AnimationState>;
      pub fn is_active(&self, now: Duration) -> bool;
      pub fn next_deadline(&self, now: Duration) -> Option<Duration>;
      pub fn retain_live(&mut self, styles: &StyleMap);
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
  }
  pub fn restyle_tree(root: &Node, sheet: &CompiledSheet, env: &ResolveEnv,
                      styles: &mut StyleMap, anims: &mut Animations,
                      now: Duration) -> usize;
  ```
  `restyle_tree` returns the number of nodes whose `ComputedStyle` changed.

- [ ] **Step 1: Write the failing test**

Create `ui/src/view/render.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::ManualClock;
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop as CssProp;

    const SHEET: &str = "
        window { background-color: #ffffff; color: #000000; }
        button { background-color: #cccccc; transition: background-color 200ms linear; }
        button:hover { background-color: #eeeeee; }
        label { color: #112233; }
    ";

    fn fixture() -> (CompiledSheet, ResolveEnv, Node, Node, Node) {
        let sheet = CompiledSheet::compile(SHEET);
        let env = ResolveEnv::default();
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        let label = Node::new("label");
        window.append_child(&button);
        button.append_child(&label);
        (sheet, env, window, button, label)
    }

    #[test]
    fn the_first_restyle_styles_every_node_and_starts_no_transition() {
        let (sheet, env, window, button, label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();

        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        assert_eq!(changed, 3, "window, button and label all got a first style");
        assert_eq!(styles.len(), 3);
        assert!(styles.contains_key(&node_addr(&button)));
        // CSS Transitions: there is no before-change style, so nothing runs.
        assert!(!anims.is_active(Duration::ZERO));
        assert_eq!(anims.next_deadline(Duration::ZERO), None);
    }

    #[test]
    fn a_restyle_with_no_change_reports_nothing_dirty() {
        let (sheet, env, window, _button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(1),
        );
        assert_eq!(changed, 0);
    }

    #[test]
    fn a_state_change_restyles_that_node_and_starts_its_transition() {
        let (sheet, env, window, button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);

        button.set_state(PseudoStates::HOVER, true);
        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(10),
        );
        assert_eq!(changed, 1, "only the button's own style changed");
        assert!(anims.is_active(Duration::from_millis(10)));

        // Mid-transition the sampled value is neither endpoint.
        let clock = ManualClock::new();
        clock.set_ms(110);
        let overrides = anims.sample(&button, Duration::from_millis(110));
        let mid = overrides
            .get(CssProp::BackgroundColor)
            .expect("background-color is transitioning");
        let end = styles[&node_addr(&button)].raw(CssProp::BackgroundColor);
        assert_ne!(mid, end, "the transition was skipped to its end value");
    }

    #[test]
    fn detached_nodes_lose_their_style_and_their_animation_state() {
        let (sheet, env, window, button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        button.set_state(PseudoStates::HOVER, true);
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::from_millis(1));
        assert_eq!(anims.len(), 1);

        button.detach();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::from_millis(2));
        assert_eq!(styles.len(), 1, "only the window is still styled");
        assert!(anims.is_empty(), "a detached node's animations are dropped");
    }

    #[test]
    fn a_deeply_nested_tree_never_blows_the_stack() {
        // Untrusted input: a `view` that builds a pathological chain must be
        // clamped, not crash the client.
        let sheet = CompiledSheet::compile(SHEET);
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut cursor = root.clone();
        for _ in 0..10_000 {
            let child = Node::new("box");
            cursor.append_child(&child);
            cursor = child;
        }
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        let changed = restyle_tree(&root, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        assert!(changed >= 1);
        assert!(
            changed <= MAX_TREE_DEPTH + 1,
            "the walk did not stop at the depth budget: {changed}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::render`
Expected: FAIL — `file not found for module 'render'`, then
`cannot find function 'restyle_tree' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/view/mod.rs`:

```rust
pub mod render;

pub use render::{Animations, NodeAddr, StyleMap, node_addr};
```

Write the body of `ui/src/view/render.rs` above its test module:

```rust
//! The three recursive walkers the reactive layer drives every frame:
//! restyle, layout and paint.
//!
//! M2 built each of these for exactly one node (`widget::button::Button`);
//! contract §9 makes them P4's, over an arbitrary retained tree.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use selectors::Element;

use crate::anim::{AnimationState, Overrides};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::select::MatchCx;

/// How deep a `View` tree may nest before the walkers stop descending.
///
/// A `view` function is application code and can recurse without bound; the
/// walkers are recursive, so an unbounded tree is a stack overflow, which is
/// an abort, not an error. GTK's own widget trees are shallow — this is two
/// orders of magnitude past anything real.
pub const MAX_TREE_DEPTH: usize = 512;

/// A stable per-node identity for caches.
///
/// Contract deviation D6: §3.4 says "keyed by `Node::addr()`", which is
/// `pub(crate)` (`css/node.rs:445`) and off-limits to P4 (§9 forbids
/// touching `css/**`). `Element::opaque` is the public equivalent and is
/// what `LayoutTree` already keys on (`layout.rs:225`).
pub type NodeAddr = selectors::OpaqueElement;

/// `node`'s cache key.
#[must_use]
pub fn node_addr(node: &Node) -> NodeAddr {
    Element::opaque(node)
}

/// Every live node's computed style. P3's `hit_test`/`hit_chain` take this
/// by reference and never build one (contract §3.4).
pub type StyleMap = HashMap<NodeAddr, Rc<ComputedStyle>>;

/// One [`AnimationState`] per node.
///
/// Contract deviation D3: §3.1's `Window::animations()` hands back a single
/// window-wide `AnimationState`, but M2's `AnimationState::restyle` diffs
/// exactly one node's `ComputedStyle` — one per window cannot animate two
/// nodes independently.
#[derive(Debug, Default)]
pub struct Animations(HashMap<NodeAddr, AnimationState>);

impl Animations {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Animations(HashMap::new())
    }

    /// This frame's animated overrides for `node`; empty when nothing runs.
    pub fn sample(&mut self, node: &Node, now: Duration) -> Overrides {
        self.0
            .get_mut(&node_addr(node))
            .map(|state| state.sample(now))
            .unwrap_or_default()
    }

    /// `node`'s animation state, if it has one.
    #[must_use]
    pub fn get(&self, node: &Node) -> Option<&AnimationState> {
        self.0.get(&node_addr(node))
    }

    /// Whether any node still needs a frame.
    #[must_use]
    pub fn is_active(&self, now: Duration) -> bool {
        self.0.values().any(|state| state.is_active(now))
    }

    /// The earliest deadline across every node; `None` when nothing runs.
    ///
    /// `Duration::ZERO` here means "now", never "spin" — M2's
    /// `AnimationState::next_deadline` documents the same.
    #[must_use]
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.0
            .values()
            .filter_map(|state| state.next_deadline(now))
            .min()
    }

    /// Drop the state of every node that is no longer styled — i.e. no
    /// longer in the tree.
    pub fn retain_live(&mut self, styles: &StyleMap) {
        self.0.retain(|addr, _| styles.contains_key(addr));
    }

    /// How many nodes are animating.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is animating.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Recascade the whole tree under `root`, feeding every change into the
/// per-node [`Animations`], and return how many nodes' styles changed.
///
/// The parent's `ComputedStyle` is threaded down, so this is M2's
/// `ComputedStyle::resolve` (one cascade per node) and not `resolve_chain`
/// (one cascade per ancestor per node) — the difference is O(n) versus
/// O(n·depth), which matters at gallery scale.
///
/// Styles of nodes that left the tree are dropped, and
/// [`Animations::retain_live`] follows, so a removed subtree cannot leak a
/// running transition.
pub fn restyle_tree(
    root: &Node,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    styles: &mut StyleMap,
    anims: &mut Animations,
    now: Duration,
) -> usize {
    let mut cx = MatchCx::new();
    let mut live: Vec<NodeAddr> = Vec::with_capacity(styles.len().max(16));
    let mut changed = 0usize;
    restyle_one(
        root, None, sheet, env, styles, anims, now, &mut cx, &mut live, &mut changed, 0,
    );
    let live: std::collections::HashSet<NodeAddr> = live.into_iter().collect();
    styles.retain(|addr, _| live.contains(addr));
    anims.retain_live(styles);
    changed
}

#[allow(
    clippy::too_many_arguments,
    reason = "one recursive walker threading the whole per-frame context; \
              bundling it into a struct would only rename the arguments"
)]
fn restyle_one(
    node: &Node,
    parent: Option<&ComputedStyle>,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    styles: &mut StyleMap,
    anims: &mut Animations,
    now: Duration,
    cx: &mut MatchCx,
    live: &mut Vec<NodeAddr>,
    changed: &mut usize,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        tracing::warn!(
            depth,
            limit = MAX_TREE_DEPTH,
            node = %node.name(),
            "view tree deeper than the walker's budget; the subtree is not styled"
        );
        return;
    }

    let addr = node_addr(node);
    live.push(addr);
    let computed = Rc::new(ComputedStyle::resolve(sheet, node, parent, env, cx));
    let previous = styles.insert(addr, Rc::clone(&computed));

    let differs = previous
        .as_ref()
        .is_none_or(|old| !styles_equal(old, &computed));
    if differs {
        *changed += 1;
        let state = anims.0.entry(addr).or_default();
        state.restyle(previous.as_deref(), &computed, now, sheet);
        // A node that started nothing carries no state worth keeping; the
        // entry would otherwise grow once per node and never shrink.
        if !state.is_active(now) && state.transition_count() == 0 && state.animation_count() == 0 {
            anims.0.remove(&addr);
        }
    }

    for child in node.children() {
        restyle_one(
            &child,
            Some(&computed),
            sheet,
            env,
            styles,
            anims,
            now,
            cx,
            live,
            changed,
            depth + 1,
        );
    }
}

/// Whether two computed styles are the same for every longhand.
///
/// `ComputedStyle` is not `PartialEq` in M2, and adding the derive would
/// touch `css/**`, which §9 forbids P4 from doing. Comparing the raw values
/// longhand by longhand is exactly what a derive would generate.
fn styles_equal(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    crate::css::registry::Prop::longhands().all(|prop| a.raw(prop) == b.raw(prop))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::render`
Expected: PASS — 5 tests.
Run: `cargo test -p icedtea-ui` — everything green, M2 gates untouched.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `detached_nodes_lose_their_style_and_their_animation_state`).**
Delete the `styles.retain(...)` and `anims.retain_live(styles)` lines at the
end of `restyle_tree`; the test must FAIL on `styles.len() == 1`. Restore.
Record in the commit body.

**Second mutation check (load-bearing: `a_restyle_with_no_change_reports_nothing_dirty`).**
Make `styles_equal` return `false` unconditionally; the test must FAIL with
`changed == 3`. Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/render.rs ui/src/view/mod.rs
git commit -m "feat(ui): the restyle walker, StyleMap and per-node animations

restyle_tree threads the parent ComputedStyle down (resolve, not
resolve_chain: O(n) rather than O(n·depth)), feeds every change into a
per-node AnimationState (deviation D3), drops the styles and animations
of nodes that left the tree, and stops at MAX_TREE_DEPTH so an unbounded
view() cannot abort the client. NodeAddr is selectors::OpaqueElement
(deviation D6, since Node::addr is pub(crate) and css/** is off-limits).

Mutation checks: dropping the retain() pair leaks a detached subtree's
style and fails detached_nodes_lose_their_style_and_their_animation_state;
making styles_equal always-false fails a_restyle_with_no_change_reports_nothing_dirty.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 8: `BuildCx`, `Op`, `Cmd`, the `Controller` trait, `Event`, `Phase`, `EventCx`

**Files:**
- Create: `ui/src/view/reconcile.rs` (`BuildCx` and `Op` only, for now)
- Create: `ui/src/view/cmd.rs`
- Create: `ui/src/view/controller.rs`
- Modify: `ui/src/window/selection.rs` (add `Clipboard::offscreen` — deviation D10)
- Modify: `ui/src/view/mod.rs` (declare both modules, re-export)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/controller.rs`

**Interfaces:**
- Consumes: `Kind`, `PropName`, `Prop`, `Props`, `Handlers`, `EventKind`
  (Tasks 1–4); `StyleMap` (Task 7); `IconTheme` (Task 6);
  `crate::css::cascade::CompiledSheet`; `crate::css::computed::{ComputedStyle,
  ResolveEnv}`; `crate::text::FontDatabase`; `crate::layout::{Allocation,
  LayoutTree}`; `crate::anim::Clock`; `crate::paint::PaintCx`;
  `crate::window::keyboard::KeyEvent`; `crate::window::pointer::Scroll`;
  `crate::window::focus::{FocusCause, FocusRing}`;
  `crate::window::selection::Clipboard`; `crate::window::SurfaceStates`;
  `skia_rs_safe::canvas::Canvas`.
- Produces:
  ```rust
  // view/reconcile.rs
  pub struct BuildCx<'a> {
      pub sheet: &'a CompiledSheet, pub fonts: &'a mut FontDatabase,
      pub icons: &'a mut IconTheme, pub clock: &'a Rc<dyn Clock>,
      pub env: &'a ResolveEnv,
  }
  pub enum Op {
      Insert { index: usize }, Remove { index: usize },
      Move { from: usize, to: usize },
      SetProp { index: usize, name: PropName },
      SetHandlers { index: usize }, Recurse { index: usize },
  }
  // view/controller.rs
  pub use crate::paint::PaintCx;   // re-export: `Controller::paint`'s `cx` is
                                   // M2's paint context, and every P5/P6
                                   // controller imports it as
                                   // `view::controller::PaintCx` (contract §10 E7)
  pub enum Phase { Capture, Target, Bubble }
  pub enum Event { PointerEnter{..}, PointerMotion{..}, PointerLeave,
                   PointerDown{..}, PointerUp{..}, Scroll(Scroll), Key(KeyEvent),
                   FocusIn{..}, FocusOut, Activate, PopupDone, Configure{..} }
  pub struct EventCx<'a, Msg> { /* eleven contract fields + `phase` (D5) */ }
  pub trait Controller<Msg>: 'static { /* kind, build, set_prop, on_event,
      tick, next_deadline, measure, paint */ }
  // view/cmd.rs
  pub enum Cmd<Msg> { None, Batch(Vec<Cmd<Msg>>), After(Duration, Rc<dyn Fn() -> Msg>),
      Copy(String), Paste(Rc<dyn Fn(Option<String>) -> Msg>), SetPrimary(String),
      Primary(Rc<dyn Fn(Option<String>) -> Msg>),
      OpenPopup { anchor: PopupAnchorPoint, positioner: Positioner,
                  view: Rc<dyn Fn() -> View<Msg>> },
      ClosePopup(PopupKey), Focus(Node), SetTitle(String), Minimize,
      ToggleMaximized, CloseWindow, Quit }
  impl<Msg> Cmd<Msg> { pub fn flatten(self) -> Vec<Cmd<Msg>>; pub fn is_none(&self) -> bool; }
  // ui/src/window/selection.rs
  impl Clipboard { pub fn offscreen() -> Clipboard; }
  ```
  `Cmd::flatten`/`Cmd::is_none` are additive helpers the loop needs; §4.7's
  variant list is verbatim.

- [ ] **Step 1: Write the failing test**

Create `ui/src/view/controller.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Kind, PropName, Prop, Props};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Pressed,
    }

    /// The smallest possible controller: it proves the trait is object
    /// safe, that every defaulted method has a working default, and that a
    /// `Box<dyn Controller<Msg>>` can be driven without knowing its type.
    #[derive(Debug, Default)]
    struct Counting {
        events: usize,
        label: Option<String>,
    }

    impl<M: Clone + 'static> Controller<M> for Counting {
        fn kind(&self) -> Kind {
            Kind::Button
        }

        fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
            Counting {
                events: 0,
                label: props.str(PropName::Label).map(str::to_owned),
            }
        }

        fn set_prop(
            &mut self,
            _node: &Node,
            name: PropName,
            value: &Prop,
            _cx: &mut BuildCx<'_>,
        ) {
            if name == PropName::Label {
                self.label = match value {
                    Prop::Str(s) => Some((**s).to_owned()),
                    _ => None,
                };
            }
        }

        fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, M>) -> Vec<M> {
            self.events += 1;
            Vec::new()
        }
    }

    #[test]
    fn the_trait_is_object_safe_and_its_defaults_are_inert() {
        let node = Node::new("button");
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));

        let sheet = CompiledSheet::compile("button { color: #000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };

        let mut boxed: Box<dyn Controller<Msg>> =
            Box::new(<Counting as Controller<Msg>>::build(&node, &props, &mut cx));
        assert_eq!(boxed.kind(), Kind::Button);
        assert_eq!(boxed.next_deadline(Duration::ZERO), None);
        assert_eq!(boxed.measure((None, None), &mut cx), None);
        boxed.set_prop(&node, PropName::Label, &Prop::Str("Cancel".into()), &mut cx);
    }

    #[test]
    fn phases_are_ordered_capture_then_target_then_bubble() {
        assert_eq!(
            Phase::ALL,
            [Phase::Capture, Phase::Target, Phase::Bubble]
        );
        assert!(Phase::Capture < Phase::Target);
        assert!(Phase::Target < Phase::Bubble);
    }

    #[test]
    fn the_offscreen_clipboard_round_trips_without_a_compositor() {
        // Deviation D10: `run_offscreen` has no Wayland connection, and
        // every `EventCx` needs a `&mut Clipboard`.
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        assert!(!clipboard.has_selection());
        assert_eq!(clipboard.paste(Duration::from_millis(1)), None);
        clipboard.copy("hello", 1);
        assert!(clipboard.has_selection());
        assert_eq!(
            clipboard.paste(Duration::from_millis(1)).as_deref(),
            Some("hello")
        );
        assert!(!clipboard.has_primary());
        clipboard.set_primary("mid", 2);
        assert!(clipboard.has_primary());
        assert_eq!(
            clipboard.primary(Duration::from_millis(1)).as_deref(),
            Some("mid")
        );
    }

    #[test]
    fn ops_compare_by_value_so_a_test_can_pin_a_whole_op_list() {
        use crate::view::reconcile::Op;
        assert_eq!(Op::Insert { index: 0 }, Op::Insert { index: 0 });
        assert_ne!(Op::Insert { index: 0 }, Op::Remove { index: 0 });
        assert_ne!(
            Op::SetProp { index: 1, name: PropName::Label },
            Op::SetProp { index: 1, name: PropName::Text }
        );
        assert_eq!(
            Op::Move { from: 2, to: 0 },
            Op::Move { from: 2, to: 0 }
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::controller`
Expected: FAIL — `file not found for module 'controller'`, then
`cannot find trait 'Controller' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/view/mod.rs`:

```rust
pub mod cmd;
pub mod controller;
pub mod reconcile;

pub use cmd::Cmd;
pub use controller::{Controller, Event, EventCx, Phase};
pub use reconcile::{BuildCx, Op};
```

Create `ui/src/view/reconcile.rs` with, for now, only these two items:

```rust
//! Keyed reconciliation of a `View` description tree into retained
//! [`Node`](crate::css::node::Node)s.

use std::rc::Rc;

use crate::anim::Clock;
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ResolveEnv;
use crate::icons::IconTheme;
use crate::text::FontDatabase;
use crate::view::PropName;

/// Everything building or updating a controller needs, threaded down the
/// whole reconcile.
pub struct BuildCx<'a> {
    /// The compiled theme, for `@keyframes` lookup and colour resolution.
    pub sheet: &'a CompiledSheet,
    /// Font matching, loading and shaping.
    pub fonts: &'a mut FontDatabase,
    /// Icon lookup and rendering.
    pub icons: &'a mut IconTheme,
    /// The animation clock; `ManualClock` under test.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
}

impl std::fmt::Debug for BuildCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildCx")
            .field("rules", &self.sheet.rules.len())
            .field("icon_theme", &self.icons.name())
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

/// One reconciliation step, at one level of the tree.
///
/// The list a `reconcile` call returns covers **that level only**: a
/// `Recurse` says the child at `index` was descended into, and the ops the
/// recursion produced are not spliced into the parent's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// A new child was built and inserted at `index`.
    Insert {
        /// Position in the new child list.
        index: usize,
    },
    /// The child that was at `index` was dropped.
    Remove {
        /// Position in the *previous* child list.
        index: usize,
    },
    /// A kept child moved.
    Move {
        /// Position in the previous child list.
        from: usize,
        /// Position in the new child list.
        to: usize,
    },
    /// One property of the child at `index` changed.
    SetProp {
        /// Position in the new child list.
        index: usize,
        /// Which property.
        name: PropName,
    },
    /// The child at `index` took this frame's handler set.
    SetHandlers {
        /// Position in the new child list.
        index: usize,
    },
    /// The child at `index` was reconciled recursively.
    Recurse {
        /// Position in the new child list.
        index: usize,
    },
}
```

In `ui/src/window/selection.rs`, add to `impl Clipboard` (deviation D10 —
purely additive; no existing signature changes):

```rust
    /// A clipboard with no Wayland connection: an in-process buffer that
    /// `copy`/`paste`/`set_primary`/`primary` round-trip through.
    ///
    /// `App::run_offscreen` (contract §4.7) runs the whole loop with no
    /// compositor, and every `EventCx` requires a `&mut Clipboard`. Each
    /// method keeps its signature; only the transport changes.
    #[must_use]
    pub fn offscreen() -> Clipboard {
        Clipboard::new_offscreen()
    }
```

where `new_offscreen` is the private constructor that leaves every Wayland
object `None` and initialises the two in-process `Option<String>` slots
(`selection`, `primary`) that `copy`/`paste`/`set_primary`/`primary` already
consult before touching the protocol.

Create `ui/src/view/cmd.rs`:

```rust
//! Side effects an `update` (or a controller) may request.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::view::View;
use crate::window::popup::{PopupAnchorPoint, PopupKey, Positioner};

/// A side effect the [`App`](crate::view::app::App) performs on the model's
/// behalf.
///
/// Everything here is deliberately *data*: an `update` returns a `Cmd`, the
/// loop interprets it, and any message it produces is **queued**, never
/// folded re-entrantly (contract §4.7).
pub enum Cmd<Msg> {
    /// Do nothing.
    None,
    /// Do all of these, in order.
    Batch(Vec<Cmd<Msg>>),
    /// Fire once, after `Duration`, on the animation clock.
    After(Duration, Rc<dyn Fn() -> Msg>),
    /// Put text on the clipboard.
    Copy(String),
    /// Read the clipboard; the closure sees `None` on no offer or timeout.
    Paste(Rc<dyn Fn(Option<String>) -> Msg>),
    /// Put text on the primary selection.
    SetPrimary(String),
    /// Read the primary selection.
    Primary(Rc<dyn Fn(Option<String>) -> Msg>),
    /// Open a child popup surface rendering `view`.
    OpenPopup {
        /// Where it hangs off the parent window.
        anchor: PopupAnchorPoint,
        /// Size, anchor, gravity and constraint rules.
        positioner: Positioner,
        /// The popup's own view function.
        view: Rc<dyn Fn() -> View<Msg>>,
    },
    /// Dismiss a popup this app opened.
    ClosePopup(PopupKey),
    /// Move the focus.
    Focus(Node),
    /// Set the toplevel title.
    SetTitle(String),
    /// Minimise the toplevel.
    Minimize,
    /// Toggle the toplevel's maximised state.
    ToggleMaximized,
    /// Ask the compositor to close the window.
    CloseWindow,
    /// Leave the loop.
    Quit,
}

impl<Msg> Default for Cmd<Msg> {
    fn default() -> Self {
        Cmd::None
    }
}

impl<Msg> std::fmt::Debug for Cmd<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cmd::None => f.write_str("None"),
            Cmd::Batch(list) => f.debug_tuple("Batch").field(list).finish(),
            Cmd::After(d, _) => write!(f, "After({d:?}, ..)"),
            Cmd::Copy(text) => write!(f, "Copy({} bytes)", text.len()),
            Cmd::Paste(_) => f.write_str("Paste(..)"),
            Cmd::SetPrimary(text) => write!(f, "SetPrimary({} bytes)", text.len()),
            Cmd::Primary(_) => f.write_str("Primary(..)"),
            Cmd::OpenPopup { positioner, .. } => {
                f.debug_struct("OpenPopup").field("positioner", positioner).finish_non_exhaustive()
            }
            Cmd::ClosePopup(key) => f.debug_tuple("ClosePopup").field(key).finish(),
            Cmd::Focus(node) => write!(f, "Focus({})", node.name()),
            Cmd::SetTitle(title) => f.debug_tuple("SetTitle").field(title).finish(),
            Cmd::Minimize => f.write_str("Minimize"),
            Cmd::ToggleMaximized => f.write_str("ToggleMaximized"),
            Cmd::CloseWindow => f.write_str("CloseWindow"),
            Cmd::Quit => f.write_str("Quit"),
        }
    }
}

impl<Msg> Cmd<Msg> {
    /// Flatten nested [`Cmd::Batch`]es into one list, dropping every
    /// [`Cmd::None`]. The loop interprets the flat list, so a deeply nested
    /// batch from composed `update`s costs one pass, not a recursion.
    #[must_use]
    pub fn flatten(self) -> Vec<Cmd<Msg>> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(cmd) = stack.pop() {
            match cmd {
                Cmd::None => {}
                // Pushed in reverse so `pop` yields them in written order.
                Cmd::Batch(list) => stack.extend(list.into_iter().rev()),
                other => out.push(other),
            }
        }
        out
    }

    /// Whether this command asks for nothing at all.
    #[must_use]
    pub fn is_none(&self) -> bool {
        match self {
            Cmd::None => true,
            Cmd::Batch(list) => list.iter().all(Cmd::is_none),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        A,
        B,
        C,
    }

    #[test]
    fn flatten_preserves_written_order_and_drops_nones() {
        let cmd: Cmd<Msg> = Cmd::Batch(vec![
            Cmd::None,
            Cmd::Copy("a".into()),
            Cmd::Batch(vec![Cmd::SetTitle("b".into()), Cmd::None, Cmd::Quit]),
            Cmd::Minimize,
        ]);
        let flat = cmd.flatten();
        let names: Vec<String> = flat.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            names,
            vec![
                "Copy(1 bytes)".to_owned(),
                "SetTitle(\"b\")".to_owned(),
                "Quit".to_owned(),
                "Minimize".to_owned(),
            ]
        );
    }

    #[test]
    fn a_batch_of_nothing_is_nothing() {
        let cmd: Cmd<Msg> = Cmd::Batch(vec![Cmd::None, Cmd::Batch(vec![Cmd::None])]);
        assert!(cmd.is_none());
        assert!(cmd.flatten().is_empty());
    }

    #[test]
    fn an_after_command_keeps_its_message_thunk() {
        let cmd: Cmd<Msg> = Cmd::After(Duration::from_millis(5), Rc::new(|| Msg::A));
        let flat = cmd.flatten();
        assert_eq!(flat.len(), 1);
        let Cmd::After(d, f) = &flat[0] else {
            panic!("After was rewritten by flatten");
        };
        assert_eq!(*d, Duration::from_millis(5));
        assert_eq!(f(), Msg::A);
        // The other two variants exist and are distinct.
        assert_ne!(Msg::B, Msg::C);
    }
}
```

Write the body of `ui/src/view/controller.rs` above its test module:

```rust
//! Per-kind behaviour, and the state the model must not own.

use std::rc::Rc;
use std::time::Duration;

use skia_rs_safe::canvas::Canvas;

use crate::anim::Clock;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::icons::IconTheme;
use crate::layout::{Allocation, LayoutTree};
use crate::paint::PaintCx;
use crate::text::FontDatabase;
use crate::view::reconcile::BuildCx;
use crate::view::render::StyleMap;
use crate::view::{Handlers, Kind, Prop, PropName, Props};
use crate::window::SurfaceStates;
use crate::window::focus::{FocusCause, FocusRing};
use crate::window::keyboard::KeyEvent;
use crate::window::pointer::Scroll;
use crate::window::selection::Clipboard;
use crate::view::cmd::Cmd;

/// Which leg of GTK's three-phase dispatch is running.
///
/// Contract deviation D5: §4.6 requires "capture → target → bubble" and says
/// a controller that sets `cx.handled = true` "stops the phase it is in" —
/// which a controller cannot honour without seeing the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Root → target, before the target sees the event.
    Capture,
    /// The node the event landed on.
    Target,
    /// Target → root, after the target has had it.
    Bubble,
}

impl Phase {
    /// The three phases, in dispatch order.
    pub const ALL: [Phase; 3] = [Phase::Capture, Phase::Target, Phase::Bubble];
}

/// What a controller sees: P3's `InputEvent` narrowed to this node, plus the
/// lifecycle events the framework synthesises.
#[derive(Debug, Clone)]
pub enum Event {
    /// The pointer entered this node.
    PointerEnter {
        /// Point in the node's border-box space.
        local: (f32, f32),
    },
    /// The pointer moved within this node (or within its implicit grab).
    PointerMotion {
        /// Point in the node's border-box space.
        local: (f32, f32),
    },
    /// The pointer left this node.
    PointerLeave,
    /// A button went down on this node.
    PointerDown {
        /// evdev button code; `window::BTN_LEFT` is `0x110`.
        button: u32,
        /// Point in the node's border-box space.
        local: (f32, f32),
        /// The input serial, for `xdg_popup.grab` and clipboard requests.
        serial: u32,
    },
    /// A button was released.
    PointerUp {
        /// evdev button code.
        button: u32,
        /// Point in the node's border-box space; may be outside the node
        /// when an implicit grab is in force.
        local: (f32, f32),
        /// The input serial.
        serial: u32,
    },
    /// An axis event reached this node.
    Scroll(Scroll),
    /// A key reached this node because it holds the focus.
    Key(KeyEvent),
    /// This node took the focus.
    FocusIn {
        /// What moved the focus here.
        cause: FocusCause,
    },
    /// This node lost the focus.
    FocusOut,
    /// Space/Enter, or a click that completed inside — the "activate" GTK
    /// means.
    Activate,
    /// A popup this controller opened was dismissed by the compositor.
    PopupDone,
    /// The window's size or states changed.
    Configure {
        /// The new surface size, in px.
        size: (u32, u32),
        /// The new `xdg_toplevel` states.
        states: SurfaceStates,
    },
}

/// Everything a controller may reach while handling an event.
pub struct EventCx<'a, Msg> {
    /// The controller's own retained node.
    pub node: &'a Node,
    /// This frame's handler set — the controller's only way to emit.
    pub handlers: &'a Handlers<Msg>,
    /// Allocations, for hit-tests inside the widget.
    pub tree: &'a LayoutTree,
    /// Computed styles, keyed by [`crate::view::NodeAddr`].
    pub styles: &'a StyleMap,
    /// The window's focus owner.
    pub focus: &'a mut FocusRing,
    /// Copy/paste and the primary selection.
    pub clipboard: &'a mut Clipboard,
    /// Icon lookup.
    pub icons: &'a mut IconTheme,
    /// Font matching and shaping.
    pub fonts: &'a mut FontDatabase,
    /// The animation clock.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
    /// Side effects requested without going through the model.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
    /// Which dispatch leg is running (deviation D5).
    pub phase: Phase,
    /// Set to `true` to stop this phase.
    pub handled: bool,
}

impl<Msg> EventCx<'_, Msg> {
    /// A [`BuildCx`] over the same resources, for a controller that has to
    /// rebuild a subnode while handling an event.
    pub fn build_cx<'b>(&'b mut self, sheet: &'b crate::css::cascade::CompiledSheet)
        -> BuildCx<'b> {
        BuildCx {
            sheet,
            fonts: self.fonts,
            icons: self.icons,
            clock: self.clock,
            env: self.env,
        }
    }
}

/// Behaviour, and the state the model should not own: press state, entry
/// cursor / selection / undo stack, scroll offset, expander progress,
/// dropdown open state, spin repeat timer.
pub trait Controller<Msg>: 'static {
    /// Which widget this controller implements.
    fn kind(&self) -> Kind;

    /// Build the kind's subnodes under `node` and seed state from `props`.
    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self
    where
        Self: Sized;

    /// One prop changed. Must update `node` — classes, pseudo-states,
    /// subnode text. This is the only place a controller touches the
    /// retained tree outside [`Controller::on_event`] and
    /// [`Controller::tick`].
    ///
    /// `value` is [`Prop::None`] when the property was removed.
    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>);

    /// The whole behaviour. Returns the messages this event produced, in
    /// order.
    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg>;

    /// Clock-driven behaviour: spin repeat, kinetic scroll, expander
    /// animation, spinner rotation.
    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let _ = (now, cx);
        Vec::new()
    }

    /// Lower bound on the next [`Controller::tick`] that would do something.
    /// `Duration::ZERO` is a legitimate "now", never "spin".
    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let _ = now;
        None
    }

    /// Intrinsic size for a leaf the layout tree cannot measure itself
    /// (text, icon, drawing area). `None` lets taffy measure the box.
    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let _ = (available, cx);
        None
    }

    /// Extra painting inside the node's own effect layer, after M2's box
    /// painting and before children. `true` if anything was drawn.
    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let _ = (canvas, alloc, style, cx);
        false
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::`
Expected: PASS — 7 new tests in `view::controller` and `view::cmd`.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `phases_are_ordered_capture_then_target_then_bubble`).**
Swap `Capture` and `Bubble` in the `Phase` declaration; the test must FAIL on
`Phase::Capture < Phase::Target`. Restore. Record in the commit body.

**Second mutation check (load-bearing:
`flatten_preserves_written_order_and_drops_nones`).** Drop the `.rev()` from
`Cmd::flatten`'s `Batch` arm; the test must FAIL with the batch's commands in
reverse. Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/controller.rs ui/src/view/reconcile.rs ui/src/view/cmd.rs ui/src/view/mod.rs ui/src/window/selection.rs
git commit -m "feat(ui): the Controller contract, Event, Phase, Cmd and the two cxs

BuildCx and Op land in reconcile.rs, Cmd in cmd.rs and
Controller/Event/EventCx in controller.rs, per contract §0's module map.
Phase is deviation D5: §4.6 demands capture/target/bubble and 'stops the
phase it is in', which a controller cannot honour without seeing the
phase. Cmd::flatten/is_none are additive helpers the loop needs.
Deviation D10 adds Clipboard::offscreen (additive, P3-owned file) here
rather than with App, because every EventCx construction from this task
onward needs one.

Mutation checks: reordering the Phase variants fails
phases_are_ordered_capture_then_target_then_bubble; dropping flatten's
.rev() fails flatten_preserves_written_order_and_drops_nones.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 9: `GenericC` and `build_controller`

**Files:**
- Modify: `ui/src/view/controller.rs` (append after the trait)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/controller.rs`

**Interfaces:**
- Consumes: `Controller`, `Event`, `EventCx`, `BuildCx`, `Phase` (Task 8);
  `Kind`, `Props`, `Prop`, `PropName`, `EventKind` (Tasks 1–3);
  `crate::text::{TextStyle, ShapedText, TextMetrics}`;
  `crate::paint::text::paint_text`; `crate::css::node::PseudoStates`.
- Produces:
  ```rust
  pub struct GenericC { /* private */ }
  impl GenericC {
      pub fn text(&self) -> Option<&str>;
      pub fn is_pressed(&self) -> bool;
      pub fn is_hovered(&self) -> bool;
  }
  impl<Msg: Clone + 'static> Controller<Msg> for GenericC { .. }
  pub fn build_controller<Msg: Clone + 'static>(
      kind: Kind, node: &Node, props: &Props, cx: &mut BuildCx<'_>,
  ) -> Box<dyn Controller<Msg>>;
  ```

`GenericC` is the controller every `Kind` gets until P5/P6 write a specific
one; `build_controller` is the single `match` those parts extend, arm by arm.
It is a real implementation, not a stub: it applies the universal props,
tracks `:hover`/`:active`/`:focus`, fires `Click`/`Activate`/`FocusIn`/
`FocusOut`, and measures and paints `PropName::Label`/`PropName::Text` as a
single shaped line.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/controller.rs`'s test module:

```rust
    fn build_cx_fixture() -> (
        CompiledSheet,
        crate::text::FontDatabase,
        crate::icons::IconTheme,
        Rc<dyn Clock>,
        ResolveEnv,
    ) {
        (
            CompiledSheet::compile(
                "button { color: #000000; } button:hover { color: #ff0000; }",
            ),
            crate::text::FontDatabase::probe_only(),
            crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]),
            Rc::new(crate::anim::ManualClock::new()),
            ResolveEnv::default(),
        )
    }

    #[test]
    fn build_controller_applies_the_kind_s_identity_and_the_universal_props() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let node = Node::new(Kind::ToggleButton.css_name());
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));
        props.set(PropName::Sensitive, Prop::Bool(false));
        props.set(
            PropName::Classes,
            Prop::Classes(std::rc::Rc::from([std::rc::Rc::from("flat")])),
        );
        props.set(PropName::Id, Prop::Str("go".into()));

        let controller: Box<dyn Controller<Msg>> =
            build_controller(Kind::ToggleButton, &node, &props, &mut cx);
        assert_eq!(controller.kind(), Kind::ToggleButton);

        let classes: Vec<String> =
            node.classes().iter().map(|c| c.as_str().to_owned()).collect();
        assert_eq!(classes, vec!["toggle".to_owned(), "flat".to_owned()]);
        assert_eq!(node.id().map(|i| i.as_str().to_owned()), Some("go".to_owned()));
        assert!(node.states().contains(PseudoStates::DISABLED));
    }

    #[test]
    fn set_prop_moves_the_node_state_and_removing_a_prop_restores_the_default() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let node = Node::new("button");
        let mut controller: Box<dyn Controller<Msg>> =
            build_controller(Kind::Button, &node, &Props::default(), &mut cx);

        controller.set_prop(&node, PropName::Sensitive, &Prop::Bool(false), &mut cx);
        assert!(node.states().contains(PseudoStates::DISABLED));
        // A removed prop is `Prop::None` and must restore GTK's default.
        controller.set_prop(&node, PropName::Sensitive, &Prop::None, &mut cx);
        assert!(!node.states().contains(PseudoStates::DISABLED));

        controller.set_prop(&node, PropName::Checked, &Prop::Bool(true), &mut cx);
        assert!(node.states().contains(PseudoStates::CHECKED));
    }

    #[test]
    fn a_press_and_release_inside_fires_click_then_activate_once() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let node = Node::new("button");
        let mut handlers: Handlers<Msg> = Handlers::default();
        handlers.set(crate::view::EventKind::Click, crate::view::Handler::Unit(Msg::Pressed));

        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            build_controller::<Msg>(Kind::Button, &node, &Props::default(), &mut cx)
        };

        let tree = LayoutTree::new();
        let styles = StyleMap::new();
        let mut focus = FocusRing::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();

        let mut ecx = EventCx {
            node: &node,
            handlers: &handlers,
            tree: &tree,
            styles: &styles,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
            phase: Phase::Target,
            handled: false,
        };

        let down = controller.on_event(
            &Event::PointerDown { button: 0x110, local: (2.0, 2.0), serial: 1 },
            &mut ecx,
        );
        assert!(down.is_empty(), "a press alone is not a click");
        assert!(node.states().contains(PseudoStates::ACTIVE));

        let up = controller.on_event(
            &Event::PointerUp { button: 0x110, local: (2.0, 2.0), serial: 2 },
            &mut ecx,
        );
        assert_eq!(up, vec![Msg::Pressed]);
        assert!(!node.states().contains(PseudoStates::ACTIVE));

        // A second release with no press in between fires nothing.
        let again = controller.on_event(
            &Event::PointerUp { button: 0x110, local: (2.0, 2.0), serial: 3 },
            &mut ecx,
        );
        assert!(again.is_empty());
    }

    #[test]
    fn a_press_released_outside_the_node_does_not_click() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let node = Node::new("button");
        let mut handlers: Handlers<Msg> = Handlers::default();
        handlers.set(crate::view::EventKind::Click, crate::view::Handler::Unit(Msg::Pressed));
        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            build_controller::<Msg>(Kind::Button, &node, &Props::default(), &mut cx)
        };
        let tree = LayoutTree::new();
        let styles = StyleMap::new();
        let mut focus = FocusRing::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut ecx = EventCx {
            node: &node, handlers: &handlers, tree: &tree, styles: &styles,
            focus: &mut focus, clipboard: &mut clipboard, icons: &mut icons,
            fonts: &mut fonts, clock: &clock, env: &env, cmds: &mut cmds,
            phase: Phase::Target, handled: false,
        };

        controller.on_event(
            &Event::PointerDown { button: 0x110, local: (2.0, 2.0), serial: 1 },
            &mut ecx,
        );
        controller.on_event(&Event::PointerLeave, &mut ecx);
        // GTK keeps `:active` off while the pointer is away but the button
        // is still held; the release outside is not a click.
        let up = controller.on_event(
            &Event::PointerUp { button: 0x110, local: (-5.0, -5.0), serial: 2 },
            &mut ecx,
        );
        assert!(up.is_empty(), "releasing outside the node clicked anyway");
    }

    #[test]
    fn labelled_kinds_measure_their_text_and_unlabelled_ones_do_not() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet, fonts: &mut fonts, icons: &mut icons,
            clock: &clock, env: &env,
        };
        let node = Node::new("label");
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Hello".into()));
        let mut labelled = build_controller::<Msg>(Kind::Label, &node, &props, &mut cx);
        let measured = labelled.measure((None, None), &mut cx);
        if fonts.has_fontconfig() || crate::text::FontDatabase::probe_only().has_fontconfig() {
            // A machine with no usable face measures nothing; that is not a
            // failure, it is `FontDatabase::match_face` returning `None`.
        }
        if let Some((w, h)) = measured {
            assert!(w > 0.0 && h > 0.0, "a shaped label measured {w}x{h}");
        }

        let empty = Node::new("box");
        let mut plain = build_controller::<Msg>(Kind::Box, &empty, &Props::default(), &mut cx);
        assert_eq!(plain.measure((None, None), &mut cx), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::controller::tests::build_controller_applies`
Expected: FAIL with `cannot find function 'build_controller' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/controller.rs`, after the trait:

```rust
/// The controller every [`Kind`] gets until P5 or P6 writes a specific one.
///
/// It is not a placeholder: it implements the behaviour every GTK widget
/// shares — the universal props, `:hover`/`:active`/`:focus`/`:disabled`/
/// `:checked` bookkeeping, click and activate, and single-line text for the
/// kinds that carry a `Label`/`Text` prop. P5/P6 replace it kind by kind in
/// [`build_controller`]; anything they do not replace still renders and still
/// responds.
pub struct GenericC {
    kind: Kind,
    text: Option<String>,
    style: Option<crate::text::TextStyle>,
    shaped: Option<Rc<crate::text::ShapedText>>,
    pressed: bool,
    hovered: bool,
}

impl std::fmt::Debug for GenericC {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericC")
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("pressed", &self.pressed)
            .field("hovered", &self.hovered)
            .finish_non_exhaustive()
    }
}

impl GenericC {
    /// The label or text this controller renders, if any.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Whether a button is currently held on this node.
    #[must_use]
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// Whether the pointer is over this node.
    #[must_use]
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Reshape after the text or the style changed.
    fn reshape(&mut self, node: &Node, cx: &mut BuildCx<'_>) {
        let Some(text) = self.text.as_deref().filter(|t| !t.is_empty()) else {
            self.shaped = None;
            return;
        };
        let mut match_cx = crate::css::select::MatchCx::new();
        let computed = ComputedStyle::resolve_chain(cx.sheet, node, cx.env, &mut match_cx);
        let style = crate::text::TextStyle::from_computed(&computed);
        let Some(face) = cx.fonts.match_face(&style.query()) else {
            // No usable typeface: a stripped container, not an error.
            self.shaped = None;
            self.style = Some(style);
            return;
        };
        self.shaped = Some(cx.fonts.shape(&style.shape_key(text, &face)));
        self.style = Some(style);
    }

    /// Write one universal prop onto the node.
    fn apply_universal(node: &Node, kind: Kind, name: PropName, value: &Prop) -> bool {
        match name {
            PropName::Classes => {
                let mut list: Vec<&str> = kind.base_classes().to_vec();
                if let Prop::Classes(extra) = value {
                    list.extend(extra.iter().map(|c| &**c));
                }
                node.set_classes(&list);
                true
            }
            PropName::Id => {
                node.set_id(match value {
                    Prop::Str(s) => Some(s),
                    _ => None,
                });
                true
            }
            PropName::Sensitive => {
                // Absent means GTK's default, which is sensitive.
                let sensitive = matches!(value, Prop::Bool(true) | Prop::None);
                node.set_state(crate::css::node::PseudoStates::DISABLED, !sensitive);
                true
            }
            PropName::Checked => {
                node.set_state(
                    crate::css::node::PseudoStates::CHECKED,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            PropName::Indeterminate => {
                node.set_state(
                    crate::css::node::PseudoStates::INDETERMINATE,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            PropName::Selected => {
                node.set_state(
                    crate::css::node::PseudoStates::SELECTED,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            _ => false,
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for GenericC {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        // The kind is not in `props`; `build_controller` sets it right after.
        // `Kind::Box` is the neutral placeholder for the two lines between.
        let mut me = GenericC {
            kind: Kind::Box,
            text: props
                .str(PropName::Label)
                .or_else(|| props.str(PropName::Text))
                .map(str::to_owned),
            style: None,
            shaped: None,
            pressed: false,
            hovered: false,
        };
        me.reshape(node, cx);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if GenericC::apply_universal(node, self.kind, name, value) {
            return;
        }
        if matches!(name, PropName::Label | PropName::Text) {
            self.text = match value {
                Prop::Str(s) => Some((**s).to_owned()),
                _ => None,
            };
            self.reshape(node, cx);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use crate::css::node::PseudoStates;
        use crate::view::EventKind;

        // Capture never acts here: a generic controller has no reason to
        // intercept an event on its way down (deviation D5).
        if cx.phase == Phase::Capture {
            return Vec::new();
        }

        match ev {
            Event::PointerEnter { .. } => {
                self.hovered = true;
                cx.node.set_state(PseudoStates::HOVER, true);
                Vec::new()
            }
            Event::PointerLeave => {
                self.hovered = false;
                cx.node.set_state(PseudoStates::HOVER, false);
                // GTK drops `:active` when the pointer leaves while held and
                // re-arms it on re-entry; `pressed` stays true so the
                // implicit grab can still complete the click if it comes back.
                cx.node.set_state(PseudoStates::ACTIVE, false);
                Vec::new()
            }
            Event::PointerMotion { .. } => {
                if self.pressed && self.hovered {
                    cx.node.set_state(PseudoStates::ACTIVE, true);
                }
                Vec::new()
            }
            Event::PointerDown { button, .. } if *button == crate::window::BTN_LEFT => {
                self.pressed = true;
                self.hovered = true;
                cx.node.set_state(PseudoStates::ACTIVE, true);
                cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
                cx.handled = true;
                Vec::new()
            }
            Event::PointerUp { button, .. } if *button == crate::window::BTN_LEFT => {
                let was_pressed = std::mem::replace(&mut self.pressed, false);
                cx.node.set_state(PseudoStates::ACTIVE, false);
                if !was_pressed || !self.hovered {
                    return Vec::new();
                }
                cx.handled = true;
                let mut out = Vec::new();
                out.extend(cx.handlers.fire_unit(EventKind::Click));
                out.extend(cx.handlers.fire_unit(EventKind::Activate));
                out
            }
            Event::Activate => {
                let mut out = Vec::new();
                out.extend(cx.handlers.fire_unit(EventKind::Activate));
                out.extend(cx.handlers.fire_unit(EventKind::Click));
                if !out.is_empty() {
                    cx.handled = true;
                }
                out
            }
            Event::FocusIn { .. } => cx
                .handlers
                .fire_unit(EventKind::FocusIn)
                .into_iter()
                .collect(),
            Event::FocusOut => cx
                .handlers
                .fire_unit(EventKind::FocusOut)
                .into_iter()
                .collect(),
            Event::Key(key) if key.pressed => cx
                .handlers
                .fire_key(EventKind::KeyPressed, key)
                .into_iter()
                .inspect(|_| cx.handled = true)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let shaped = self.shaped.as_ref()?;
        let style = self.style.as_ref()?;
        let width = shaped.metrics.width;
        let height = style.line_height_px(&shaped.metrics);
        Some((
            available.0.map_or(width, |cap| width.min(cap)),
            available.1.map_or(height, |cap| height.min(cap)),
        ))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(shaped) = self.shaped.as_ref() else {
            return false;
        };
        crate::paint::paint_text(
            canvas,
            shaped,
            alloc.content_box,
            style,
            &cx.base_length_ctx(),
        );
        true
    }
}

/// Build the controller for `kind`.
///
/// This single `match` is the seam P5 and P6 extend: each arm they add
/// replaces a [`GenericC`] with the kind's real controller, and nothing else
/// in the framework changes.
#[must_use]
pub fn build_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    // Identity first, so a `Classes` prop lands on top of the base classes.
    node.set_classes(kind.base_classes());
    let mut generic = <GenericC as Controller<Msg>>::build(node, props, cx);
    generic.kind = kind;

    let mut boxed: Box<dyn Controller<Msg>> = Box::new(generic);
    for (name, value) in props.iter() {
        boxed.set_prop(node, name, value, cx);
    }
    boxed
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::controller`
Expected: PASS — 8 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings` — no warnings
(this is the configuration where `FontDatabase::new` is `probe_only`, which is
why `labelled_kinds_measure_their_text_…` tolerates a `None` measurement).
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `a_press_released_outside_the_node_does_not_click`).**
Delete `|| !self.hovered` from `PointerUp`'s early return; the test must FAIL
with a `Msg::Pressed` from a release at `(-5, -5)`. Restore. Record in the
commit body.

**Second mutation check (load-bearing:
`set_prop_moves_the_node_state_and_removing_a_prop_restores_the_default`).**
Change `PropName::Sensitive`'s arm to `matches!(value, Prop::Bool(true))`
(dropping the `| Prop::None`); the removal half of the test must FAIL.
Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/controller.rs
git commit -m "feat(ui): GenericC and the build_controller seam

GenericC is the real, working controller every Kind gets until P5/P6
writes a specific one: universal props, hover/active/focus/disabled/
checked bookkeeping, click and activate, and single-line shaped text for
the kinds carrying Label or Text. build_controller is the one match P5
and P6 extend arm by arm.

Mutation checks: dropping the hovered guard from PointerUp makes a
release outside the node click and fails
a_press_released_outside_the_node_does_not_click; treating a removed
Sensitive prop as false fails set_prop_moves_the_node_state_....

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 10: `container_for` and the layout walker

**Files:**
- Modify: `ui/src/view/render.rs` (append after `restyle_tree`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/render.rs`

**Interfaces:**
- Consumes: `StyleMap`, `node_addr`, `MAX_TREE_DEPTH` (Task 7); `Kind`,
  `Props`, `PropName`, `Prop` (Tasks 1–2); `crate::layout::{BoxDirection,
  Container, LayoutError, LayoutTree, Measure}`.
- Produces:
  ```rust
  pub const ORIENTATION_HORIZONTAL: u16 = 0;
  pub const ORIENTATION_VERTICAL: u16 = 1;
  pub fn container_for(kind: Kind, props: &Props, has_children: bool) -> Container;
  pub fn layout_tree(
      root: &Node, styles: &StyleMap, containers: &HashMap<NodeAddr, Container>,
      tree: &mut LayoutTree, env: &ResolveEnv,
      available: (Option<f32>, Option<f32>), measure: &mut dyn Measure,
  ) -> Result<(), LayoutError>;
  ```
  `containers` is built by the reconciler (Task 12) and read here, so the
  layout walker never needs the `Instance` tree.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/render.rs`'s test module:

```rust
    #[test]
    fn a_childless_node_is_a_leaf_whatever_its_kind() {
        use crate::view::Kind;
        assert_eq!(
            container_for(Kind::Box, &crate::view::Props::default(), false),
            Container::Leaf
        );
        assert_eq!(
            container_for(Kind::Label, &crate::view::Props::default(), false),
            Container::Leaf
        );
    }

    #[test]
    fn gtk_s_default_orientations_are_reproduced_and_overridable() {
        use crate::view::{Kind, Prop, PropName, Props};
        let empty = Props::default();
        // GtkBox is horizontal by default.
        assert_eq!(
            container_for(Kind::Box, &empty, true),
            Container::Box { direction: BoxDirection::Row }
        );
        // A window stacks its children.
        assert_eq!(
            container_for(Kind::Window, &empty, true),
            Container::Box { direction: BoxDirection::Column }
        );
        assert_eq!(
            container_for(Kind::ListBox, &empty, true),
            Container::Box { direction: BoxDirection::Column }
        );

        let mut vertical = Props::default();
        vertical.set(PropName::Orientation, Prop::Enum(ORIENTATION_VERTICAL));
        assert_eq!(
            container_for(Kind::Box, &vertical, true),
            Container::Box { direction: BoxDirection::Column }
        );

        let mut horizontal = Props::default();
        horizontal.set(PropName::Orientation, Prop::Enum(ORIENTATION_HORIZONTAL));
        assert_eq!(
            container_for(Kind::Window, &horizontal, true),
            Container::Box { direction: BoxDirection::Row }
        );
    }

    #[test]
    fn the_layout_walker_allocates_every_styled_node() {
        let (sheet, env, window, button, label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);

        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(
            node_addr(&window),
            Container::Box { direction: BoxDirection::Column },
        );
        containers.insert(
            node_addr(&button),
            Container::Box { direction: BoxDirection::Row },
        );
        containers.insert(node_addr(&label), Container::Leaf);

        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size { width: 40.0, height: 16.0 });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(200.0), None),
            &mut measure,
        )
        .expect("the tree lays out");

        let window_alloc = tree.allocation(&window).expect("window allocation");
        let button_alloc = tree.allocation(&button).expect("button allocation");
        let label_alloc = tree.allocation(&label).expect("label allocation");
        assert!((window_alloc.border_box.width - 200.0).abs() < 0.5);
        assert!(label_alloc.border_box.width >= 40.0);
        assert!(
            button_alloc.border_box.height >= label_alloc.border_box.height,
            "the label overflowed its button"
        );
        // Allocations are absolute, so a nested node is offset by its parent.
        assert!(label_alloc.border_box.y >= button_alloc.border_box.y);
    }

    #[test]
    fn a_node_missing_from_the_container_map_is_laid_out_as_a_leaf() {
        let (sheet, env, window, _button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);

        // An empty map: nothing has been reconciled yet. This must not panic
        // and must still produce a laid-out tree.
        let containers: HashMap<NodeAddr, Container> = HashMap::new();
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size { width: 8.0, height: 8.0 });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(100.0), Some(50.0)),
            &mut measure,
        )
        .expect("an unmapped tree still lays out");
        assert!(tree.allocation(&window).is_some());
    }

    #[test]
    fn an_unstyled_root_is_an_error_not_a_panic() {
        let env = ResolveEnv::default();
        let orphan = Node::new("window");
        let styles = StyleMap::new();
        let containers: HashMap<NodeAddr, Container> = HashMap::new();
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size::ZERO);
        // `sync` inserts the node, so this succeeds; what must not happen is
        // an unwrap on a missing style.
        assert!(
            layout_tree(
                &orphan,
                &styles,
                &containers,
                &mut tree,
                &env,
                (None, None),
                &mut measure,
            )
            .is_ok()
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::render::tests::gtk_s_default_orientations`
Expected: FAIL with `cannot find function 'container_for' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/render.rs`:

```rust
use crate::layout::{BoxDirection, Container, LayoutError, LayoutTree, Measure};
use crate::view::{Kind, Prop, PropName, Props};

/// `PropName::Orientation`'s `Prop::Enum` discriminant for horizontal.
pub const ORIENTATION_HORIZONTAL: u16 = 0;
/// `PropName::Orientation`'s `Prop::Enum` discriminant for vertical.
pub const ORIENTATION_VERTICAL: u16 = 1;

/// The layout container a kind's node uses.
///
/// M2's `Container` has exactly `Box` and `Leaf`; P6 (contract §3.6) adds
/// `Grid` and `Center` and rewires `Grid`/`CenterBox` here. Until then every
/// container is a box, which is what GTK's own `GtkBoxLayout` gives most of
/// them anyway.
#[must_use]
pub fn container_for(kind: Kind, props: &Props, has_children: bool) -> Container {
    if !has_children {
        return Container::Leaf;
    }
    let default = match kind {
        // Everything that stacks its children top to bottom in GTK.
        Kind::Window
        | Kind::ShortcutsWindow
        | Kind::AboutDialog
        | Kind::AlertDialog
        | Kind::ColorDialog
        | Kind::FontDialog
        | Kind::Popover
        | Kind::PopoverMenu
        | Kind::PopoverMenuItem
        | Kind::ListBox
        | Kind::ListBoxRow
        | Kind::ListView
        | Kind::ColumnView
        | Kind::Notebook
        | Kind::NotebookTab
        | Kind::Stack
        | Kind::StackPage
        | Kind::StackSidebar
        | Kind::ScrolledWindow
        | Kind::Frame
        | Kind::Overlay
        | Kind::Expander
        | Kind::SearchBar
        | Kind::TextView
        | Kind::Calendar
        | Kind::InfoBar
        | Kind::DropDown
        | Kind::EditableLabel => BoxDirection::Column,
        // GtkBox, GtkHeaderBar, GtkActionBar, GtkCenterBox, the button
        // family and the entry family are all horizontal by default.
        _ => BoxDirection::Row,
    };
    let direction = match props.get(PropName::Orientation) {
        Some(Prop::Enum(ORIENTATION_HORIZONTAL)) => BoxDirection::Row,
        Some(Prop::Enum(ORIENTATION_VERTICAL)) => BoxDirection::Column,
        _ => default,
    };
    Container::Box { direction }
}

/// Sync the retained tree into taffy, write every node's box, and compute.
///
/// `available` is the surface's content box: `Some` fixes that axis,
/// `None` asks for the intrinsic size (`MaxContent`), which is how a popup
/// or a `width_request`-free toplevel gets measured.
///
/// A node with no entry in `styles` is skipped rather than unwrapped: the
/// restyle walker stops at [`MAX_TREE_DEPTH`], so a pathological tree has
/// styled and unstyled halves and layout must survive both.
///
/// # Errors
///
/// [`LayoutError::Unsynced`] if `root` never made it into the tree, or
/// [`LayoutError::Taffy`] if taffy fails.
pub fn layout_tree(
    root: &Node,
    styles: &StyleMap,
    containers: &HashMap<NodeAddr, Container>,
    tree: &mut LayoutTree,
    env: &ResolveEnv,
    available: (Option<f32>, Option<f32>),
    measure: &mut dyn Measure,
) -> Result<(), LayoutError> {
    tree.sync(root)?;
    write_styles(root, styles, containers, tree, env, 0);
    let space = taffy::Size {
        width: available
            .0
            .map_or(taffy::AvailableSpace::MaxContent, taffy::AvailableSpace::Definite),
        height: available
            .1
            .map_or(taffy::AvailableSpace::MaxContent, taffy::AvailableSpace::Definite),
    };
    tree.compute(root, space, measure)
}

fn write_styles(
    node: &Node,
    styles: &StyleMap,
    containers: &HashMap<NodeAddr, Container>,
    tree: &mut LayoutTree,
    env: &ResolveEnv,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let addr = node_addr(node);
    if let Some(style) = styles.get(&addr) {
        let children = node.children();
        let container = containers.get(&addr).copied().unwrap_or_else(|| {
            if children.is_empty() {
                Container::Leaf
            } else {
                Container::Box {
                    direction: BoxDirection::Row,
                }
            }
        });
        tree.set_style(node, style, container, env);
        for child in children {
            write_styles(&child, styles, containers, tree, env, depth + 1);
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::render`
Expected: PASS — 10 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`gtk_s_default_orientations_are_reproduced_and_overridable`).** Change the
`Some(Prop::Enum(ORIENTATION_VERTICAL))` arm to fall through to `_ => default`;
the `Kind::Box` + vertical assertion must FAIL. Restore. Record in the commit
body.

**Second mutation check (load-bearing:
`a_node_missing_from_the_container_map_is_laid_out_as_a_leaf`).** Replace
`containers.get(&addr).copied().unwrap_or_else(...)` with
`containers[&addr]`; the test must FAIL with a panic on the missing key
(a panic is a failure, and that is the point). Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/render.rs
git commit -m "feat(ui): the layout walker and GTK's default orientations

container_for reproduces each Kind's default GtkBoxLayout orientation and
honours an explicit Orientation prop; layout_tree syncs the retained tree
into taffy, writes every styled node's box and computes, skipping (never
unwrapping) nodes the depth-clamped restyle never reached.

Mutation checks: ignoring an explicit vertical Orientation fails
gtk_s_default_orientations_are_reproduced_and_overridable; indexing the
container map instead of defaulting panics and fails
a_node_missing_from_the_container_map_is_laid_out_as_a_leaf.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 11: `paint_tree` — the recursive painter

**Files:**
- Modify: `ui/src/view/render.rs` (append after `layout_tree`)
- Modify: `ui/src/paint/mod.rs` (the two `#[allow(...)] reason` strings only — deviation D12)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/render.rs`

**Interfaces:**
- Consumes: `StyleMap`, `Animations`, `node_addr`, `MAX_TREE_DEPTH` (Task 7);
  `crate::layout::LayoutTree`; `crate::paint::{PaintCx, ImageCache,
  paint_node_with_children}`; `skia_rs_safe::canvas::Canvas`.
- Produces:
  ```rust
  pub trait NodePainter {
      fn paint_content(&mut self, node: &Node, canvas: &mut Canvas<'_>,
                       alloc: &Allocation, style: &ComputedStyle,
                       cx: &mut PaintCx<'_>) -> bool;
  }
  pub struct NoContent;
  impl NodePainter for NoContent { .. }
  pub fn paint_tree(canvas: &mut Canvas<'_>, root: &Node, styles: &StyleMap,
                    tree: &LayoutTree, anims: &mut Animations, now: Duration,
                    origin: (f32, f32), cx: &mut PaintCx<'_>,
                    painter: &mut dyn NodePainter) -> usize;
  ```
  `NodePainter` is the seam `App` implements over the `Instance` tree to reach
  `Controller::paint`; `paint_tree` itself never sees an `Instance`, which is
  what lets this task land before the reconciler.
  `paint_tree` returns how many nodes it painted.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/render.rs`'s test module:

```rust
    /// Paint a solid rect wherever the node has a `content` marker class, so
    /// the test can prove `Controller::paint`'s seam runs *inside* the
    /// node's effect layer.
    struct MarkerPainter;

    impl NodePainter for MarkerPainter {
        fn paint_content(
            &mut self,
            node: &Node,
            canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
            alloc: &crate::layout::Allocation,
            _style: &ComputedStyle,
            _cx: &mut crate::paint::PaintCx<'_>,
        ) -> bool {
            if !node.classes().iter().any(|c| c.as_str() == "content") {
                return false;
            }
            let mut paint = skia_rs_safe::core::Paint::default();
            paint.set_color(skia_rs_safe::core::Color(0xFF00_FF00));
            canvas.draw_rect(alloc.content_box.to_skia(), &paint);
            true
        }
    }

    fn paint_fixture() -> (CompiledSheet, ResolveEnv, Node, Node) {
        let sheet = CompiledSheet::compile(
            "window { background-color: #ff0000; }
             box { background-color: #0000ff; opacity: 0.5; }",
        );
        let env = ResolveEnv::default();
        let window = Node::with_classes("window", &["background"]);
        let inner = Node::with_classes("box", &["content"]);
        window.append_child(&inner);
        (sheet, env, window, inner)
    }

    #[test]
    fn paint_tree_paints_every_node_once_root_first() {
        let (sheet, env, window, inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);

        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(
            node_addr(&window),
            Container::Box { direction: BoxDirection::Column },
        );
        containers.insert(node_addr(&inner), Container::Leaf);
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size { width: 30.0, height: 20.0 });
        layout_tree(
            &window, &styles, &containers, &mut tree, &env,
            (Some(60.0), Some(40.0)), &mut measure,
        )
        .expect("lays out");

        let mut surface = skia_rs_safe::canvas::Surface::new_raster_n32_premul(60, 40)
            .expect("raster surface");
        surface.canvas().clear(skia_rs_safe::core::Color::TRANSPARENT);
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let painted = {
            let mut canvas = surface.canvas();
            paint_tree(
                &mut canvas, &window, &styles, &tree, &mut anims,
                Duration::ZERO, (0.0, 0.0), &mut cx, &mut MarkerPainter,
            )
        };
        assert_eq!(painted, 2);

        let pixel = surface
            .pixel_buffer()
            .get_pixel(5, 5)
            .expect("pixel in range");
        // The inner box is 50% opaque green content over 50% opaque blue
        // over opaque red — the exact blend is M2's business; what this
        // pins is that all three layers ran, so no channel is saturated at
        // the pure red the window alone would leave.
        assert_ne!(pixel, skia_rs_safe::core::Color(0xFFFF_0000));

        let outside = surface
            .pixel_buffer()
            .get_pixel(59, 39)
            .expect("pixel in range");
        assert_eq!(
            outside,
            skia_rs_safe::core::Color(0xFFFF_0000),
            "the window background did not reach its own bottom-right corner"
        );
    }

    #[test]
    fn the_content_painter_runs_inside_the_node_s_own_effect_layer() {
        // `opacity: 0.5` on the box must dim the green content the painter
        // draws. If the content were painted after the layer closed, the
        // green would be fully opaque.
        let (sheet, env, window, inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(node_addr(&window), Container::Box { direction: BoxDirection::Column });
        containers.insert(node_addr(&inner), Container::Leaf);
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size { width: 60.0, height: 40.0 });
        layout_tree(&window, &styles, &containers, &mut tree, &env, (Some(60.0), Some(40.0)), &mut measure)
            .expect("lays out");

        let mut surface = skia_rs_safe::canvas::Surface::new_raster_n32_premul(60, 40)
            .expect("raster surface");
        surface.canvas().clear(skia_rs_safe::core::Color::TRANSPARENT);
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env, colors: &sheet.colors, fonts: &mut fonts,
            images: &mut images, text: None,
        };
        {
            let mut canvas = surface.canvas();
            paint_tree(&mut canvas, &window, &styles, &tree, &mut anims,
                       Duration::ZERO, (0.0, 0.0), &mut cx, &mut MarkerPainter);
        }
        let skia_rs_safe::core::Color(argb) =
            surface.pixel_buffer().get_pixel(30, 20).expect("pixel in range");
        let g = ((argb >> 8) & 0xFF) as u8;
        assert!(
            g > 0 && g < 0xFF,
            "the content green was not dimmed by the node's opacity: {g:#04x}"
        );
    }

    #[test]
    fn a_node_with_no_allocation_is_skipped_not_unwrapped() {
        let (sheet, env, window, _inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&window, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        // No `layout_tree` call at all: the LayoutTree is empty.
        let tree = LayoutTree::new();
        let mut surface = skia_rs_safe::canvas::Surface::new_raster_n32_premul(10, 10)
            .expect("raster surface");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env, colors: &sheet.colors, fonts: &mut fonts,
            images: &mut images, text: None,
        };
        let painted = {
            let mut canvas = surface.canvas();
            paint_tree(&mut canvas, &window, &styles, &tree, &mut anims,
                       Duration::ZERO, (0.0, 0.0), &mut cx, &mut NoContent)
        };
        assert_eq!(painted, 0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::render::tests::paint_tree_paints`
Expected: FAIL with `cannot find function 'paint_tree' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/render.rs`:

```rust
use crate::layout::Allocation;
use crate::paint::{PaintCx, paint_node_with_children};
use skia_rs_safe::canvas::Canvas;

/// The seam between the generic paint walk and per-widget content.
///
/// `App` implements this over the `Instance` tree so a node's own
/// [`Controller::paint`](crate::view::Controller::paint) runs inside that
/// node's effect layer. Keeping it a trait is what lets [`paint_tree`] know
/// nothing about `Instance`s.
pub trait NodePainter {
    /// Paint `node`'s own content, after M2's box painting and before its
    /// children. `true` if anything was drawn.
    fn paint_content(
        &mut self,
        node: &Node,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool;
}

/// A [`NodePainter`] that paints nothing — boxes, borders and backgrounds only.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoContent;

impl NodePainter for NoContent {
    fn paint_content(
        &mut self,
        _node: &Node,
        _canvas: &mut Canvas<'_>,
        _alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        false
    }
}

/// Paint the whole retained tree, root first, each node inside its own
/// effect layer.
///
/// This is contract §9's "recursive paint walker": M2 shipped
/// [`paint_node_with_children`] correct for one node and never called it over
/// a tree (`paint/mod.rs:245`'s own comment says so). `origin` translates the
/// tree into surface space, exactly as `Button::render` does.
///
/// Returns the number of nodes painted; a node with no allocation (never laid
/// out, or below [`MAX_TREE_DEPTH`]) is skipped along with its subtree.
#[allow(
    clippy::too_many_arguments,
    reason = "one recursive walker threading the whole per-frame paint context"
)]
pub fn paint_tree(
    canvas: &mut Canvas<'_>,
    root: &Node,
    styles: &StyleMap,
    tree: &LayoutTree,
    anims: &mut Animations,
    now: Duration,
    origin: (f32, f32),
    cx: &mut PaintCx<'_>,
    painter: &mut dyn NodePainter,
) -> usize {
    let mut painted = 0usize;
    paint_one(
        canvas, root, styles, tree, anims, now, origin, cx, painter, &mut painted, 0,
    );
    painted
}

#[allow(
    clippy::too_many_arguments,
    reason = "the recursive half of `paint_tree`"
)]
fn paint_one(
    canvas: &mut Canvas<'_>,
    node: &Node,
    styles: &StyleMap,
    tree: &LayoutTree,
    anims: &mut Animations,
    now: Duration,
    origin: (f32, f32),
    cx: &mut PaintCx<'_>,
    painter: &mut dyn NodePainter,
    painted: &mut usize,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let Some(style) = styles.get(&node_addr(node)) else {
        return;
    };
    let Some(alloc) = tree.allocation(node) else {
        return;
    };
    let alloc = Allocation {
        border_box: crate::layout::Rect::new(
            alloc.border_box.x + origin.0,
            alloc.border_box.y + origin.1,
            alloc.border_box.width,
            alloc.border_box.height,
        ),
        content_box: crate::layout::Rect::new(
            alloc.content_box.x + origin.0,
            alloc.content_box.y + origin.1,
            alloc.content_box.width,
            alloc.content_box.height,
        ),
        border: alloc.border,
        padding: alloc.padding,
    };

    let overrides = anims.sample(node, now);
    *painted += 1;
    let children = node.children();

    paint_node_with_children(
        canvas,
        node,
        style,
        &alloc,
        Some(&overrides),
        cx,
        |canvas, cx| {
            // The node's own content first, then its children — GTK's order,
            // and both inside the layer `paint_node_with_children` opened, so
            // `opacity`, `transform` and `filter` reach all of it.
            painter.paint_content(node, canvas, &alloc, style, cx);
            for child in children {
                paint_one(
                    canvas, &child, styles, tree, anims, now, origin, cx, painter, painted,
                    depth + 1,
                );
            }
        },
    );
}
```

Then, in `ui/src/paint/mod.rs`, rewrite **only** the `reason` strings of the
two `#[allow(unused_variables, ...)]` attributes (contract deviation D12 — the
attribute itself stays, because the walker is the *caller* and `node` is still
unread inside these two functions):

```rust
#[allow(
    unused_variables,
    reason = "`node` is contract §8's signature; the tree walk that passes it \
              is `view::render::paint_tree` (M3 P4)"
)]
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::render`
Expected: PASS — 13 tests.
Run: `cargo test -p icedtea-ui` — everything green; in particular
`ui/tests/themed_button_offscreen.rs`'s 4 and `ui/src/paint/mod.rs`'s own
tests are untouched (only a doc string changed).
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`the_content_painter_runs_inside_the_node_s_own_effect_layer`).** Move the
`painter.paint_content(...)` call out of the closure, to just after the
`paint_node_with_children` call; the test must FAIL with `g == 0xFF`
(undimmed green). Restore. Record in the commit body.

**Second mutation check (load-bearing:
`a_node_with_no_allocation_is_skipped_not_unwrapped`).** Change
`let Some(alloc) = tree.allocation(node) else { return; }` to
`tree.allocation(node).unwrap()`; the test must FAIL with a panic. Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/render.rs ui/src/paint/mod.rs
git commit -m "feat(ui): the recursive paint walker

paint_tree walks the retained tree root-first, sampling each node's own
AnimationState and painting its content and its children inside the one
effect layer paint_node_with_children opens — the caller M2 shaped that
function for and never wrote. NodePainter keeps Instances out of it.

Deviation D12: paint/mod.rs keeps its #[allow(unused_variables)] (the
walker is the caller; `node` is still unread inside those two functions,
so removing the allow would not build under -D warnings); only the reason
string is rewritten to name the caller that now exists.

Mutation checks: painting content after the layer closes leaves it
undimmed and fails the_content_painter_runs_inside_the_node_s_own_effect_layer;
unwrapping a missing allocation panics and fails
a_node_with_no_allocation_is_skipped_not_unwrapped.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 12: `Instance` and reconciliation without move minimality

**Files:**
- Modify: `ui/src/view/reconcile.rs` (append after `Op`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/reconcile.rs`

**Interfaces:**
- Consumes: `BuildCx`, `Op` (Task 8); `Controller`, `build_controller`
  (Tasks 8–9); `View`, `Kind`, `Key`, `Props`, `PropName`, `Prop`, `Handlers`
  (Tasks 1–4); `container_for`, `NodeAddr`, `node_addr` (Tasks 7, 10).
- Produces:
  ```rust
  pub struct Instance<Msg> {
      pub node: Node, pub kind: Kind, pub key: Option<Key>, pub props: Props,
      pub handlers: Handlers<Msg>, pub controller: Box<dyn Controller<Msg>>,
      pub children: Vec<Instance<Msg>>,
  }
  impl<Msg: Clone + 'static> Instance<Msg> {
      pub fn is_visible(&self) -> bool;
      pub fn find(&self, node: &Node) -> Option<&Instance<Msg>>;
      pub fn find_mut(&mut self, node: &Node) -> Option<&mut Instance<Msg>>;
  }
  pub fn reconcile<Msg: Clone + 'static>(
      parent: &Node, prev: &mut Vec<Instance<Msg>>, next: Vec<View<Msg>>,
      cx: &mut BuildCx<'_>,
  ) -> Vec<Op>;
  pub fn containers_of<Msg>(instances: &[Instance<Msg>],
                            out: &mut HashMap<NodeAddr, Container>);
  ```

Visibility carries no extra field: `Instance::is_visible` reads
`props.bool(PropName::Visible, true)`, and an invisible instance keeps its
`Node` and controller but is not attached to `parent`, which is exactly how
GTK's unmapped widgets behave — layout, paint and hit-testing never see it.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/reconcile.rs` a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::cascade::CompiledSheet;
    use crate::css::node::Node;
    use crate::view::builders::widget;
    use crate::view::{Kind, PropName, Prop};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Hit,
    }

    struct Fixture {
        sheet: CompiledSheet,
        fonts: crate::text::FontDatabase,
        icons: crate::icons::IconTheme,
        clock: std::rc::Rc<dyn crate::anim::Clock>,
        env: crate::css::computed::ResolveEnv,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                sheet: CompiledSheet::compile("window { color: #000; } button { color: #111; }"),
                fonts: crate::text::FontDatabase::probe_only(),
                icons: crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]),
                clock: std::rc::Rc::new(crate::anim::ManualClock::new()),
                env: crate::css::computed::ResolveEnv::default(),
            }
        }

        fn cx(&mut self) -> BuildCx<'_> {
            BuildCx {
                sheet: &self.sheet,
                fonts: &mut self.fonts,
                icons: &mut self.icons,
                clock: &self.clock,
                env: &self.env,
            }
        }
    }

    fn labelled(kind: Kind, key: &str, label: &str) -> View<Msg> {
        widget::<Msg>(kind).key(key).prop(PropName::Label, label)
    }

    fn names(parent: &Node) -> Vec<String> {
        parent.children().iter().map(|c| c.name().to_string()).collect()
    }

    #[test]
    fn a_first_reconcile_inserts_every_child_and_attaches_its_node() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A"), labelled(Kind::Label, "b", "B")],
            &mut f.cx(),
        );
        assert_eq!(ops, vec![Op::Insert { index: 0 }, Op::Insert { index: 1 }]);
        assert_eq!(prev.len(), 2);
        assert_eq!(names(&parent), vec!["button".to_owned(), "label".to_owned()]);
        assert_eq!(prev[0].kind, Kind::Button);
        assert_eq!(prev[0].key, Some(crate::view::Key::Name(std::rc::Rc::from("a"))));
    }

    #[test]
    fn an_unchanged_frame_emits_only_set_handlers() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "A")], &mut f.cx());
        let node_before = prev[0].node.clone();

        let ops = reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "A")], &mut f.cx());
        assert_eq!(ops, vec![Op::SetHandlers { index: 0 }]);
        assert!(
            prev[0].node.ptr_eq(&node_before),
            "identity was not preserved across an unchanged frame"
        );
    }

    #[test]
    fn a_changed_prop_emits_one_set_prop_and_reaches_the_controller() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "A")], &mut f.cx());
        let ops = reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "Z")], &mut f.cx());
        assert_eq!(
            ops,
            vec![
                Op::SetProp { index: 0, name: PropName::Label },
                Op::SetHandlers { index: 0 },
            ]
        );
        assert_eq!(prev[0].props.str(PropName::Label), Some("Z"));
    }

    #[test]
    fn a_removed_child_is_dropped_and_detached_and_its_controller_goes_with_it() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A"), labelled(Kind::Label, "b", "B")],
            &mut f.cx(),
        );
        let gone = prev[1].node.clone();
        let ops = reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "A")], &mut f.cx());
        assert!(ops.contains(&Op::Remove { index: 1 }));
        assert_eq!(prev.len(), 1);
        assert_eq!(names(&parent), vec!["button".to_owned()]);
        assert!(gone.parent().is_none(), "the removed node is still attached");
    }

    #[test]
    fn a_kind_change_under_the_same_key_rebuilds_rather_than_reusing() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(&parent, &mut prev, vec![labelled(Kind::Button, "a", "A")], &mut f.cx());
        let old = prev[0].node.clone();
        let ops = reconcile(&parent, &mut prev, vec![labelled(Kind::Label, "a", "A")], &mut f.cx());
        assert!(ops.contains(&Op::Remove { index: 0 }));
        assert!(ops.contains(&Op::Insert { index: 0 }));
        assert!(!prev[0].node.ptr_eq(&old));
        assert_eq!(names(&parent), vec!["label".to_owned()]);
    }

    #[test]
    fn unkeyed_children_match_positionally_within_their_kind() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Label).prop(PropName::Label, "one"),
                widget::<Msg>(Kind::Button).prop(PropName::Label, "two"),
                widget::<Msg>(Kind::Label).prop(PropName::Label, "three"),
            ],
            &mut f.cx(),
        );
        let first_label = prev[0].node.clone();
        let second_label = prev[2].node.clone();

        // The button disappears; the two labels must keep their identity in
        // their own positional order, not slide onto each other.
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                widget::<Msg>(Kind::Label).prop(PropName::Label, "one"),
                widget::<Msg>(Kind::Label).prop(PropName::Label, "three"),
            ],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Remove { index: 1 }));
        assert!(prev[0].node.ptr_eq(&first_label));
        assert!(prev[1].node.ptr_eq(&second_label));
    }

    #[test]
    fn an_invisible_child_keeps_its_instance_but_leaves_the_node_tree() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A"), labelled(Kind::Label, "b", "B")],
            &mut f.cx(),
        );
        let hidden = prev[1].node.clone();

        reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Button, "a", "A"),
                labelled(Kind::Label, "b", "B").visible(false),
            ],
            &mut f.cx(),
        );
        assert_eq!(prev.len(), 2, "the instance survived");
        assert!(!prev[1].is_visible());
        assert!(hidden.parent().is_none(), "an invisible child stayed attached");
        assert_eq!(names(&parent), vec!["button".to_owned()]);

        // And it comes back, in its own position.
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Button, "a", "A"), labelled(Kind::Label, "b", "B")],
            &mut f.cx(),
        );
        assert!(prev[1].node.ptr_eq(&hidden));
        assert_eq!(names(&parent), vec!["button".to_owned(), "label".to_owned()]);
    }

    #[test]
    fn children_are_reconciled_recursively_and_the_op_says_so() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![widget::<Msg>(Kind::Box).key("outer").child(labelled(Kind::Label, "in", "x"))],
            &mut f.cx(),
        );
        let inner = prev[0].children[0].node.clone();

        let ops = reconcile(
            &parent,
            &mut prev,
            vec![widget::<Msg>(Kind::Box).key("outer").child(labelled(Kind::Label, "in", "y"))],
            &mut f.cx(),
        );
        assert!(ops.contains(&Op::Recurse { index: 0 }));
        assert!(prev[0].children[0].node.ptr_eq(&inner));
        assert_eq!(prev[0].children[0].props.str(PropName::Label), Some("y"));
    }

    #[test]
    fn find_locates_an_instance_by_its_node_anywhere_in_the_subtree() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![widget::<Msg>(Kind::Box).key("outer").child(labelled(Kind::Label, "in", "x"))],
            &mut f.cx(),
        );
        let inner = prev[0].children[0].node.clone();
        let found = prev[0].find(&inner).expect("the nested instance is findable");
        assert_eq!(found.kind, Kind::Label);
        assert!(prev[0].find(&Node::new("stranger")).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::reconcile`
Expected: FAIL with `cannot find type 'Instance' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/reconcile.rs`:

```rust
use std::collections::HashMap;

use crate::css::node::Node;
use crate::layout::Container;
use crate::view::controller::{Controller, build_controller};
use crate::view::render::{NodeAddr, container_for, node_addr};
use crate::view::{Handlers, Key, Kind, Prop, Props, View};

/// One retained widget: the node it owns, the props it was last built from,
/// this frame's handlers, its controller, and its children.
pub struct Instance<Msg> {
    /// The retained M2 node.
    pub node: Node,
    /// Which widget.
    pub kind: Kind,
    /// The identity that matched it to this frame's view.
    pub key: Option<Key>,
    /// Last frame's props.
    pub props: Props,
    /// This frame's handlers — replaced wholesale, never diffed.
    pub handlers: Handlers<Msg>,
    /// The behaviour and the state the model must not own.
    pub controller: Box<dyn Controller<Msg>>,
    /// Child instances, in view order (including invisible ones).
    pub children: Vec<Instance<Msg>>,
}

impl<Msg> std::fmt::Debug for Instance<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("props", &self.props)
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

impl<Msg: Clone + 'static> Instance<Msg> {
    /// Whether this instance is attached to its parent node.
    ///
    /// GTK's `visible`: an invisible widget keeps its state but takes no
    /// space, receives no events and paints nothing. Detaching the node is
    /// how that falls out of M2's tree for free.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.props.bool(crate::view::PropName::Visible, true)
    }

    /// The instance owning `node`, searching this instance and its subtree.
    #[must_use]
    pub fn find(&self, node: &Node) -> Option<&Instance<Msg>> {
        if self.node.ptr_eq(node) {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(node))
    }

    /// The mutable form of [`Instance::find`].
    pub fn find_mut(&mut self, node: &Node) -> Option<&mut Instance<Msg>> {
        if self.node.ptr_eq(node) {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(node))
    }
}

/// Build a fresh instance and its whole subtree.
fn build_instance<Msg: Clone + 'static>(
    view: View<Msg>,
    cx: &mut BuildCx<'_>,
) -> Instance<Msg> {
    let node = Node::new(view.kind.css_name());
    let controller = build_controller::<Msg>(view.kind, &node, &view.props, cx);
    let mut instance = Instance {
        node,
        kind: view.kind,
        key: view.key,
        props: view.props,
        handlers: view.handlers,
        controller,
        children: Vec::new(),
    };
    let node = instance.node.clone();
    reconcile(&node, &mut instance.children, view.children, cx);
    instance
}

/// Reconcile one level: diff `next` against `prev`, mutating `prev` in place
/// into this frame's instance list, and return the ops applied **at this
/// level** (a `Recurse` stands for the whole child call).
///
/// Matching, in order:
/// * a keyed view matches the previous instance with the same key **and**
///   the same kind — a kind change under one key is a rebuild, because the
///   controller and the node name both differ;
/// * an unkeyed view matches the next unconsumed previous instance of the
///   same kind, positionally;
/// * anything unmatched on the right is built, anything unmatched on the
///   left is dropped (its controller drops with it).
///
/// Identity survives every path but the rebuild: the `Node`, its animation
/// state, its focus, its shaping caches and any attached popup all belong to
/// the `Instance`, which is moved, never recreated.
pub fn reconcile<Msg: Clone + 'static>(
    parent: &Node,
    prev: &mut Vec<Instance<Msg>>,
    next: Vec<View<Msg>>,
    cx: &mut BuildCx<'_>,
) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let mut taken: Vec<Option<Instance<Msg>>> =
        std::mem::take(prev).into_iter().map(Some).collect();

    // 1. Decide, for every new child, which previous instance it reuses.
    let mut matches: Vec<Option<usize>> = Vec::with_capacity(next.len());
    let mut used = vec![false; taken.len()];
    for view in &next {
        let found = match &view.key {
            Some(key) => taken.iter().position(|slot| {
                slot.as_ref().is_some_and(|inst| {
                    inst.key.as_ref() == Some(key) && inst.kind == view.kind
                })
            }),
            None => taken.iter().enumerate().position(|(index, slot)| {
                !used[index]
                    && slot
                        .as_ref()
                        .is_some_and(|inst| inst.key.is_none() && inst.kind == view.kind)
            }),
        };
        if let Some(index) = found {
            used[index] = true;
        }
        matches.push(found);
    }

    // 2. Anything not reused is gone. Detach first so a rebuild under the
    //    same key cannot leave two nodes claiming one position.
    for (index, slot) in taken.iter_mut().enumerate() {
        if used[index] {
            continue;
        }
        if let Some(instance) = slot.take() {
            instance.node.detach();
            ops.push(Op::Remove { index });
            drop(instance);
        }
    }

    // 3. Build this frame's list.
    let mut built: Vec<Instance<Msg>> = Vec::with_capacity(next.len());
    for (index, view) in next.into_iter().enumerate() {
        match matches[index].and_then(|from| taken[from].take().map(|inst| (from, inst))) {
            Some((from, mut instance)) => {
                let changed = view.props.diff(&instance.props);
                for name in changed {
                    let value = view.props.get(name).cloned().unwrap_or(Prop::None);
                    instance
                        .controller
                        .set_prop(&instance.node, name, &value, cx);
                    ops.push(Op::SetProp { index, name });
                }
                instance.props = view.props;
                instance.handlers = view.handlers;
                ops.push(Op::SetHandlers { index });
                if from != index {
                    ops.push(Op::Move { from, to: index });
                }
                let node = instance.node.clone();
                if !reconcile(&node, &mut instance.children, view.children, cx).is_empty() {
                    ops.push(Op::Recurse { index });
                }
                built.push(instance);
            }
            None => {
                built.push(build_instance(view, cx));
                ops.push(Op::Insert { index });
            }
        }
    }

    // 4. Re-attach the visible nodes in order, touching only what moved.
    //    Every `insert_child` bumps the tree generation and dirties a
    //    restyle, so the comparison here is what keeps a steady frame from
    //    restyling the whole window.
    let mut position = 0usize;
    for instance in &built {
        if !instance.is_visible() {
            if instance.node.parent().is_some() {
                instance.node.detach();
            }
            continue;
        }
        let in_place = parent
            .child(position)
            .is_some_and(|current| current.ptr_eq(&instance.node));
        if !in_place {
            parent.insert_child(position, &instance.node);
        }
        position += 1;
    }
    while parent.child_count() > position {
        if let Some(extra) = parent.child(position) {
            extra.detach();
        } else {
            break;
        }
    }

    *prev = built;
    ops
}

/// Collect every instance's layout container, for
/// [`layout_tree`](crate::view::render::layout_tree).
pub fn containers_of<Msg: Clone + 'static>(
    instances: &[Instance<Msg>],
    out: &mut HashMap<NodeAddr, Container>,
) {
    for instance in instances {
        let visible_children = instance.children.iter().any(Instance::is_visible);
        out.insert(
            node_addr(&instance.node),
            container_for(instance.kind, &instance.props, visible_children),
        );
        containers_of(&instance.children, out);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::reconcile`
Expected: PASS — 9 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `an_unchanged_frame_emits_only_set_handlers`).**
Delete the `in_place` check and always call `parent.insert_child(position,
&instance.node)`; the test still passes, so add the stronger assertion it
needs: record `parent.generation()` before the second `reconcile` and assert
it is unchanged. With the check removed that assertion FAILs. Keep the
assertion; restore the check. Record in the commit body.

**Second mutation check (load-bearing:
`unkeyed_children_match_positionally_within_their_kind`).** Drop the
`inst.kind == view.kind` condition from the unkeyed arm; the test must FAIL
(the second label reuses the button's instance). Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/reconcile.rs
git commit -m "feat(ui): Instance and the reconciler's build/remove/prop paths

Keyed views match on key AND kind (a kind change is a rebuild); unkeyed
ones match positionally within their kind; removals detach the node and
drop the controller; props diff into Controller::set_prop; children
recurse. Node re-attachment touches only what actually moved, because
insert_child bumps the tree generation and would otherwise restyle the
whole window every steady frame. An invisible instance keeps its state
and leaves the node tree, which is what GTK's unmapped widgets do.

Mutation checks: always re-inserting every child bumps the generation on
a steady frame and fails an_unchanged_frame_emits_only_set_handlers;
dropping the kind check from unkeyed matching slides a label onto a
button's instance and fails unkeyed_children_match_positionally_within_their_kind.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 13: minimal `Move` ops — the keyed LCS

**Files:**
- Modify: `ui/src/view/reconcile.rs` (replace step 3's `if from != index` move rule)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/reconcile.rs`

**Interfaces:**
- Consumes: everything from Task 12.
- Produces:
  ```rust
  pub(crate) fn longest_increasing_subsequence(values: &[usize]) -> Vec<usize>;
  ```
  (indices *into* `values`, ascending — the kept children that need no `Move`).
  `reconcile`'s signature is unchanged; only its `Move` emission is.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/reconcile.rs`'s test module:

```rust
    #[test]
    fn lis_returns_the_longest_run_and_prefers_the_earlier_one_on_a_tie() {
        assert_eq!(longest_increasing_subsequence(&[]), Vec::<usize>::new());
        assert_eq!(longest_increasing_subsequence(&[5]), vec![0]);
        assert_eq!(longest_increasing_subsequence(&[0, 1, 2, 3]), vec![0, 1, 2, 3]);
        assert_eq!(longest_increasing_subsequence(&[3, 2, 1, 0]), vec![3]);
        // 2, 3, 5 (indices 1, 2, 4) is the longest run in 4 2 3 1 5.
        assert_eq!(longest_increasing_subsequence(&[4, 2, 3, 1, 5]), vec![1, 2, 4]);
    }

    #[test]
    fn moving_one_child_to_the_front_emits_exactly_one_move() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let rows = |order: [&str; 3]| {
            order
                .into_iter()
                .map(|k| labelled(Kind::Label, k, k))
                .collect::<Vec<_>>()
        };
        reconcile(&parent, &mut prev, rows(["a", "b", "c"]), &mut f.cx());
        let a = prev[0].node.clone();
        let b = prev[1].node.clone();
        let c = prev[2].node.clone();

        let ops = reconcile(&parent, &mut prev, rows(["c", "a", "b"]), &mut f.cx());
        let moves: Vec<&Op> = ops
            .iter()
            .filter(|op| matches!(op, Op::Move { .. }))
            .collect();
        assert_eq!(
            moves,
            vec![&Op::Move { from: 2, to: 0 }],
            "a rotation by one must cost one Move, not three: {ops:?}"
        );
        // Identity survived, and the node order followed.
        assert!(prev[0].node.ptr_eq(&c));
        assert!(prev[1].node.ptr_eq(&a));
        assert!(prev[2].node.ptr_eq(&b));
        assert_eq!(
            parent
                .children()
                .iter()
                .map(|n| n.ptr_eq(&c) as u8 * 3 + n.ptr_eq(&a) as u8 * 1 + n.ptr_eq(&b) as u8 * 2)
                .collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }

    #[test]
    fn a_reversal_moves_everything_but_one() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let rows = |order: [&str; 4]| {
            order
                .into_iter()
                .map(|k| labelled(Kind::Label, k, k))
                .collect::<Vec<_>>()
        };
        reconcile(&parent, &mut prev, rows(["a", "b", "c", "d"]), &mut f.cx());
        let ops = reconcile(&parent, &mut prev, rows(["d", "c", "b", "a"]), &mut f.cx());
        let moves = ops.iter().filter(|op| matches!(op, Op::Move { .. })).count();
        assert_eq!(moves, 3, "a full reversal keeps exactly one child in place");
    }

    #[test]
    fn a_pure_insertion_in_the_middle_moves_nothing() {
        let mut f = Fixture::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        reconcile(
            &parent,
            &mut prev,
            vec![labelled(Kind::Label, "a", "a"), labelled(Kind::Label, "c", "c")],
            &mut f.cx(),
        );
        let ops = reconcile(
            &parent,
            &mut prev,
            vec![
                labelled(Kind::Label, "a", "a"),
                labelled(Kind::Label, "b", "b"),
                labelled(Kind::Label, "c", "c"),
            ],
            &mut f.cx(),
        );
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Move { .. })),
            "an insertion emitted a Move: {ops:?}"
        );
        assert!(ops.contains(&Op::Insert { index: 1 }));
        assert_eq!(prev.len(), 3);
        assert_eq!(names(&parent).len(), 3);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::reconcile::tests::moving_one_child`
Expected: FAIL — first `cannot find function
'longest_increasing_subsequence'`, and once that exists,
`moving_one_child_to_the_front_emits_exactly_one_move` fails with three
`Move`s, because Task 12 emits one for every `from != index`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/view/reconcile.rs`:

```rust
/// Indices into `values` of a longest strictly-increasing subsequence.
///
/// The classic patience-sorting solution: `tails[len - 1]` holds the index of
/// the smallest possible tail of an increasing run of length `len`, and
/// `prev` threads each element back to its predecessor so the run can be
/// reconstructed. O(n log n), which matters for a `ListView` model with
/// thousands of rows.
pub(crate) fn longest_increasing_subsequence(values: &[usize]) -> Vec<usize> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut tails: Vec<usize> = Vec::with_capacity(values.len());
    let mut back: Vec<usize> = vec![usize::MAX; values.len()];
    for (index, &value) in values.iter().enumerate() {
        let position = tails.partition_point(|&tail| values[tail] < value);
        if position > 0 {
            back[index] = tails[position - 1];
        }
        if position == tails.len() {
            tails.push(index);
        } else {
            tails[position] = index;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cursor = *tails.last().expect("values is non-empty");
    loop {
        out.push(cursor);
        if back[cursor] == usize::MAX {
            break;
        }
        cursor = back[cursor];
    }
    out.reverse();
    out
}
```

Then replace step 3's move rule in `reconcile`. Before the `for (index, view)`
loop, compute which reused children may stay put:

```rust
    // Which reused children need no `Move`: the longest run whose previous
    // positions are already ascending. Everything outside it moves, and
    // that is the minimum — `Op::Move` is the expensive op, because
    // `Node::insert_child` reparents and dirties a restyle.
    let reused: Vec<usize> = matches.iter().flatten().copied().collect();
    let stable: std::collections::HashSet<usize> = longest_increasing_subsequence(&reused)
        .into_iter()
        .map(|position| reused[position])
        .collect();
```

and inside the loop replace

```rust
                if from != index {
                    ops.push(Op::Move { from, to: index });
                }
```

with

```rust
                if !stable.contains(&from) {
                    ops.push(Op::Move { from, to: index });
                }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::reconcile`
Expected: PASS — 13 tests.
Run: `cargo test -p icedtea-ui` — everything green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: the LCS —
`moving_one_child_to_the_front_emits_exactly_one_move`).** Replace
`longest_increasing_subsequence`'s body with
`(0..values.len()).collect()` (claim everything is stable); the reversal test
must FAIL with zero `Move`s while the node order is wrong. Then replace it
with `Vec::new()` (claim nothing is stable); the rotation test must FAIL with
three `Move`s. Restore both times. Record both results in the commit body —
this is the pair the contract's "mutation checks on the LCS" asks for.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/reconcile.rs
git commit -m "feat(ui): minimal Move ops via a patience-sorting LCS

The reused children whose previous positions already ascend stay put;
everything else moves. A rotation by one costs one Move rather than
three, and a middle insertion costs none — which matters because
Node::insert_child reparents and dirties a restyle. O(n log n) for the
thousand-row ListView models P6 will feed it.

Mutation checks (contract §4.8's 'mutation checks on the LCS'): an
all-stable LCS fails a_reversal_moves_everything_but_one with zero Moves
and a wrong node order; an empty LCS fails
moving_one_child_to_the_front_emits_exactly_one_move with three.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 14: reconciler property tests

**Files:**
- Create: `ui/tests/reconcile_props.rs`

**Interfaces:**
- Consumes: `reconcile`, `Instance`, `Op`, `BuildCx` (Tasks 8, 12, 13);
  `widget`, `Kind`, `PropName` (Tasks 2, 5).
- Produces: nothing consumed by later tasks — this is contract §4.8's
  "reconciler property tests" gate.

No `proptest` dependency is added (the Global Constraints forbid new deps): a
xorshift64\* PRNG seeded from a fixed list gives the same coverage,
deterministically, and a failing seed is printed so it can be replayed.

- [ ] **Step 1: Write the failing test**

Create `ui/tests/reconcile_props.rs`:

```rust
//! Contract §4.8's reconciler property tests: for random keyed edit
//! sequences, every surviving key keeps its `Node`; the op list carries no
//! redundant `Move` or `SetProp`; and a removed key's controller is dropped
//! exactly once.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use icedtea_ui::anim::{Clock, ManualClock};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::ResolveEnv;
use icedtea_ui::css::node::Node;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::widget;
use icedtea_ui::view::reconcile::{Instance, Op, reconcile};
use icedtea_ui::view::{BuildCx, Kind, PropName, View};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Unused,
}

/// xorshift64*: three lines, no dependency, and the same sequence on every
/// machine, so a failure is replayable from its seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

struct Harness {
    sheet: CompiledSheet,
    fonts: FontDatabase,
    icons: IconTheme,
    clock: Rc<dyn Clock>,
    env: ResolveEnv,
}

impl Harness {
    fn new() -> Self {
        Harness {
            sheet: CompiledSheet::compile("window { color: #000; } label { color: #111; }"),
            fonts: FontDatabase::probe_only(),
            icons: IconTheme::with_name_and_roots("hicolor", vec![]),
            clock: Rc::new(ManualClock::new()),
            env: ResolveEnv::default(),
        }
    }

    fn cx(&mut self) -> BuildCx<'_> {
        BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }
}

fn row(key: u64, label: &str) -> View<Msg> {
    widget::<Msg>(Kind::Label)
        .key(key)
        .prop(PropName::Label, label)
}

/// A random frame: a subset of `0..pool`, shuffled, each with a label that
/// is either its key or its key plus a salt (so props change sometimes).
fn frame(rng: &mut Rng, pool: u64, salt: u64) -> Vec<(u64, String)> {
    let mut keys: Vec<u64> = (0..pool).filter(|_| rng.next() % 3 != 0).collect();
    for i in (1..keys.len()).rev() {
        keys.swap(i, rng.below(i + 1));
    }
    keys.into_iter()
        .map(|k| {
            let label = if rng.next() % 4 == 0 {
                format!("{k}-{salt}")
            } else {
                k.to_string()
            };
            (k, label)
        })
        .collect()
}

#[test]
fn every_surviving_key_keeps_its_node_across_random_edits() {
    for seed in [1_u64, 7, 42, 1337, 90_210, 0xDEAD_BEEF] {
        let mut rng = Rng(seed);
        let mut h = Harness::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let mut nodes: HashMap<u64, Node> = HashMap::new();

        for step in 0..40_u64 {
            let spec = frame(&mut rng, 12, step);
            let views: Vec<View<Msg>> =
                spec.iter().map(|(k, label)| row(*k, label)).collect();
            reconcile(&parent, &mut prev, views, &mut h.cx());

            assert_eq!(prev.len(), spec.len(), "seed {seed} step {step}: arity");
            for (index, (key, label)) in spec.iter().enumerate() {
                let instance = &prev[index];
                assert_eq!(
                    instance.key,
                    Some(icedtea_ui::view::Key::Id(*key)),
                    "seed {seed} step {step}: key at {index}"
                );
                assert_eq!(
                    instance.props.str(PropName::Label),
                    Some(label.as_str()),
                    "seed {seed} step {step}: label at {index}"
                );
                match nodes.get(key) {
                    Some(known) => assert!(
                        instance.node.ptr_eq(known),
                        "seed {seed} step {step}: key {key} lost its node"
                    ),
                    None => {
                        nodes.insert(*key, instance.node.clone());
                    }
                }
            }
            // Keys absent this frame are gone for good; their next
            // appearance is a fresh node, so forget them.
            nodes.retain(|k, _| spec.iter().any(|(key, _)| key == k));

            // The attached node order equals the visible instance order.
            let attached: Vec<Node> = parent.children();
            assert_eq!(attached.len(), prev.len(), "seed {seed} step {step}: attached arity");
            for (a, b) in attached.iter().zip(prev.iter()) {
                assert!(a.ptr_eq(&b.node), "seed {seed} step {step}: node order");
            }
        }
    }
}

#[test]
fn the_op_list_carries_no_redundant_move_or_set_prop() {
    for seed in [3_u64, 11, 99, 65_537] {
        let mut rng = Rng(seed);
        let mut h = Harness::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let mut last: Vec<(u64, String)> = Vec::new();

        for step in 0..40_u64 {
            let spec = frame(&mut rng, 10, step);
            let views: Vec<View<Msg>> = spec.iter().map(|(k, l)| row(*k, l)).collect();
            let ops = reconcile(&parent, &mut prev, views, &mut h.cx());

            for op in &ops {
                match op {
                    Op::Move { from, to } => assert_ne!(
                        from, to,
                        "seed {seed} step {step}: a Move to its own position"
                    ),
                    Op::SetProp { index, name } => {
                        assert_eq!(*name, PropName::Label);
                        let (key, label) = &spec[*index];
                        let before = last.iter().find(|(k, _)| k == key).map(|(_, l)| l.as_str());
                        assert_ne!(
                            before,
                            Some(label.as_str()),
                            "seed {seed} step {step}: SetProp for an unchanged label"
                        );
                    }
                    _ => {}
                }
            }
            // A pure reshuffle of the same keys must never cost more Moves
            // than there are keys out of their longest ascending run.
            let moves = ops.iter().filter(|o| matches!(o, Op::Move { .. })).count();
            assert!(
                moves < spec.len().max(1),
                "seed {seed} step {step}: {moves} Moves for {} children",
                spec.len()
            );
            last = spec;
        }
    }
}

#[test]
fn a_removed_key_s_controller_is_dropped_exactly_once() {
    // A controller that counts its own drops through a shared cell. Building
    // it needs no framework support: `build_controller` is only consulted by
    // `reconcile`, so the count is taken from a `Kind::DrawingArea` view
    // whose `Prop::Draw` closure owns the guard — the closure is dropped
    // exactly when the instance's props are, i.e. with the instance.
    let drops = Rc::new(Cell::new(0usize));

    struct Guard(Rc<Cell<usize>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let mut h = Harness::new();
    let parent = Node::new("window");
    let mut prev: Vec<Instance<Msg>> = Vec::new();

    {
        let guard = Rc::new(Guard(Rc::clone(&drops)));
        let view: View<Msg> = widget::<Msg>(Kind::DrawingArea).key(1_u64).prop(
            PropName::DrawFn,
            icedtea_ui::view::Prop::Draw(Rc::new(move |_canvas, _rect| {
                // Capturing the guard is the whole point; the body never runs.
                let _ = &guard;
            })),
        );
        reconcile(&parent, &mut prev, vec![view], &mut h.cx());
    }
    assert_eq!(drops.get(), 0, "the instance is alive, so nothing dropped");

    reconcile(&parent, &mut prev, Vec::new(), &mut h.cx());
    assert_eq!(drops.get(), 1, "the removed instance dropped its state once");

    // A second empty frame must not drop it again.
    reconcile(&parent, &mut prev, Vec::new(), &mut h.cx());
    assert_eq!(drops.get(), 1, "the drop ran twice");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test reconcile_props`
Expected: FAIL — the crate does not re-export `view::reconcile::reconcile`
publicly yet if `pub use` is missing; add
`pub use reconcile::{Instance, containers_of, reconcile};` to
`ui/src/view/mod.rs` and re-run. With the exports in place the three tests
compile and pass.

- [ ] **Step 3: Write minimal implementation**

The only production change is the re-export. In `ui/src/view/mod.rs`:

```rust
pub use reconcile::{BuildCx, Instance, Op, containers_of, reconcile};
```

(replacing the earlier `pub use reconcile::{BuildCx, Op};`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --test reconcile_props`
Expected: PASS — 3 tests, six and four seeds respectively, 40 frames each.
Run: `cargo test -p icedtea-ui` — everything green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`every_surviving_key_keeps_its_node_across_random_edits`).** In `reconcile`'s
keyed match arm, drop the `inst.key.as_ref() == Some(key)` condition so any
same-kind instance matches; the test must FAIL on "key N lost its node".
Restore. Record in the commit body.

**Second mutation check (load-bearing:
`the_op_list_carries_no_redundant_move_or_set_prop`, the `Props::diff` half
the contract asks for).** Make `Props::diff` return
`self.iter().map(|(n, _)| n).collect()` (every name, changed or not); the test
must FAIL on "SetProp for an unchanged label". Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/tests/reconcile_props.rs ui/src/view/mod.rs
git commit -m "test(ui): reconciler property tests over random keyed edits

Contract §4.8's gate: 40-frame random edit sequences across six seeds
prove every surviving key keeps its Node and its attached position; four
more prove no Move lands on its own index, no SetProp fires for an
unchanged value, and a reshuffle never costs a Move per child; a
drop-counting closure proves a removed instance's state is released
exactly once. A xorshift64* PRNG keeps it dependency-free and replayable.

Mutation checks: matching keyed views on kind alone fails
every_surviving_key_keeps_its_node_across_random_edits; a Props::diff
that reports every name fails the_op_list_carries_no_redundant_move_or_set_prop.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 15: event dispatch — capture, target, bubble

**Files:**
- Create: `ui/src/view/app.rs` (dispatch only, for now)
- Modify: `ui/src/view/mod.rs` (`pub mod app;`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/app.rs`

**Interfaces:**
- Consumes: `Instance` (Task 12); `Event`, `EventCx`, `Phase` (Task 8);
  `StyleMap` (Task 7); `crate::window::pointer::{Hit, hit_chain, ImplicitGrab}`;
  `crate::window::focus::{FocusCause, FocusRing}`;
  `crate::window::selection::Clipboard`.
- Produces:
  ```rust
  pub struct Dispatch<'a, Msg> {
      pub styles: &'a StyleMap, pub tree: &'a crate::layout::LayoutTree,
      pub focus: &'a mut FocusRing, pub clipboard: &'a mut Clipboard,
      pub icons: &'a mut IconTheme, pub fonts: &'a mut FontDatabase,
      pub clock: &'a Rc<dyn Clock>, pub env: &'a ResolveEnv,
      pub cmds: &'a mut Vec<Cmd<Msg>>,
  }
  pub fn deliver<Msg: Clone + 'static>(
      roots: &mut [Instance<Msg>], path: &[Node], event: &Event,
      cx: &mut Dispatch<'_, Msg>,
  ) -> Vec<Msg>;
  pub fn path_to<Msg: Clone + 'static>(roots: &[Instance<Msg>], target: &Node) -> Vec<Node>;
  ```
  `path` is outermost-first and ends at the target, matching
  `window::pointer::hit_chain`'s order.

- [ ] **Step 1: Write the failing test**

Create `ui/src/view/app.rs` with a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::cascade::CompiledSheet;
    use crate::view::builders::widget;
    use crate::view::reconcile::reconcile;
    use crate::view::{BuildCx, Handler, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Outer,
        Inner,
    }

    fn tree() -> (
        CompiledSheet,
        crate::text::FontDatabase,
        crate::icons::IconTheme,
        Rc<dyn Clock>,
        ResolveEnv,
        Node,
        Vec<Instance<Msg>>,
    ) {
        let sheet = CompiledSheet::compile("box { color: #000; } button { color: #111; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<Msg>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<Msg>(Kind::Box)
                        .key("outer")
                        .on(crate::view::EventKind::Click, Handler::Unit(Msg::Outer))
                        .child(
                            widget::<Msg>(Kind::Button)
                                .key("inner")
                                .prop(PropName::Label, "Go")
                                .on(crate::view::EventKind::Click, Handler::Unit(Msg::Inner)),
                        ),
                ],
                &mut cx,
            );
        }
        (sheet, fonts, icons, clock, env, root, instances)
    }

    #[test]
    fn path_to_is_outermost_first_and_ends_at_the_target() {
        let (_s, _f, _i, _c, _e, _root, instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        assert_eq!(path.len(), 2);
        assert!(path[0].ptr_eq(&instances[0].node));
        assert!(path[1].ptr_eq(&inner));
        assert!(path_to(&instances, &Node::new("stranger")).is_empty());
    }

    #[test]
    fn a_click_on_the_inner_node_bubbles_to_the_outer_one() {
        let (sheet, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);

        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = crate::window::focus::FocusRing::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        let _ = &sheet;

        deliver(
            &mut instances,
            &path,
            &Event::PointerDown { button: crate::window::BTN_LEFT, local: (1.0, 1.0), serial: 1 },
            &mut cx,
        );
        let out = deliver(
            &mut instances,
            &path,
            &Event::PointerUp { button: crate::window::BTN_LEFT, local: (1.0, 1.0), serial: 2 },
            &mut cx,
        );
        // The inner button handled it and stopped the target phase; the
        // outer box never saw a press, so it produces nothing on bubble.
        assert_eq!(out, vec![Msg::Inner]);
    }

    #[test]
    fn an_empty_path_delivers_nothing_and_does_not_panic() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = crate::window::focus::FocusRing::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles, tree: &tree_layout, focus: &mut focus,
            clipboard: &mut clipboard, icons: &mut icons, fonts: &mut fonts,
            clock: &clock, env: &env, cmds: &mut cmds,
        };
        assert!(deliver(&mut instances, &[], &Event::PointerLeave, &mut cx).is_empty());
    }

    #[test]
    fn messages_come_back_in_dispatch_order_target_before_bubble() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = crate::window::focus::FocusRing::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles, tree: &tree_layout, focus: &mut focus,
            clipboard: &mut clipboard, icons: &mut icons, fonts: &mut fonts,
            clock: &clock, env: &env, cmds: &mut cmds,
        };
        // `Activate` is not handled-stopping unless a handler fires, and both
        // nodes bind Click, so the target answers first and the ancestor
        // second.
        let out = deliver(&mut instances, &path, &Event::Activate, &mut cx);
        assert_eq!(out.first(), Some(&Msg::Inner));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::app`
Expected: FAIL — `file not found for module 'app'`, then
`cannot find function 'deliver' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add `pub mod app;` to `ui/src/view/mod.rs`, then write the body of
`ui/src/view/app.rs` above its test module:

```rust
//! The loop: events in, messages folded, a new view reconciled, the dirty
//! parts restyled, relaid out and repainted.

use std::rc::Rc;
use std::time::Duration;

use crate::anim::Clock;
use crate::css::computed::ResolveEnv;
use crate::css::node::Node;
use crate::icons::IconTheme;
use crate::layout::LayoutTree;
use crate::text::FontDatabase;
use crate::view::cmd::Cmd;
use crate::view::controller::{Event, EventCx, Phase};
use crate::view::reconcile::Instance;
use crate::view::render::StyleMap;
use crate::window::focus::FocusRing;
use crate::window::selection::Clipboard;

/// The borrows one dispatch pass needs, minus the per-node ones.
pub struct Dispatch<'a, Msg> {
    /// Computed styles, for a controller that hit-tests inside itself.
    pub styles: &'a StyleMap,
    /// Allocations.
    pub tree: &'a LayoutTree,
    /// The window's focus owner.
    pub focus: &'a mut FocusRing,
    /// Copy/paste and the primary selection.
    pub clipboard: &'a mut Clipboard,
    /// Icon lookup.
    pub icons: &'a mut IconTheme,
    /// Font matching and shaping.
    pub fonts: &'a mut FontDatabase,
    /// The animation clock.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
    /// Side effects controllers requested.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
}

/// The chain of nodes from the outermost instance down to `target`,
/// outermost first — the same order `window::pointer::hit_chain` returns.
/// Empty when `target` is not in the tree.
#[must_use]
pub fn path_to<Msg: Clone + 'static>(roots: &[Instance<Msg>], target: &Node) -> Vec<Node> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        target: &Node,
        out: &mut Vec<Node>,
    ) -> bool {
        out.push(instance.node.clone());
        if instance.node.ptr_eq(target) {
            return true;
        }
        for child in &instance.children {
            if walk(child, target, out) {
                return true;
            }
        }
        out.pop();
        false
    }

    let mut out = Vec::new();
    for root in roots {
        if walk(root, target, &mut out) {
            return out;
        }
        out.clear();
    }
    Vec::new()
}

/// Deliver `event` along `path` in GTK's three phases: capture from the
/// outermost node inward, then the target, then bubble back out.
///
/// A controller that sets `cx.handled` stops **the phase it is in**
/// (deviation D5): capture stops descending, bubble stops climbing, and a
/// handled target simply ends the target phase. Messages come back in the
/// order they were produced.
pub fn deliver<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    path: &[Node],
    event: &Event,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let last = path.len() - 1;

    for phase in Phase::ALL {
        let order: Vec<usize> = match phase {
            Phase::Capture => (0..last).collect(),
            Phase::Target => vec![last],
            Phase::Bubble => (0..last).rev().collect(),
        };
        for index in order {
            let node = path[index].clone();
            let Some(instance) = roots.iter_mut().find_map(|root| root.find_mut(&node)) else {
                continue;
            };
            // Split the borrow: the controller is `&mut`, everything else
            // the `EventCx` needs is behind a different field or a clone.
            let Instance {
                controller,
                handlers,
                node: own,
                ..
            } = instance;
            let mut ecx = EventCx {
                node: own,
                handlers,
                tree: cx.tree,
                styles: cx.styles,
                focus: cx.focus,
                clipboard: cx.clipboard,
                icons: cx.icons,
                fonts: cx.fonts,
                clock: cx.clock,
                env: cx.env,
                cmds: cx.cmds,
                phase,
                handled: false,
            };
            out.extend(controller.on_event(event, &mut ecx));
            if ecx.handled {
                break;
            }
        }
    }
    out
}

/// Run every controller's `tick`, deepest last, and collect what they say.
pub fn tick_all<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    let mut out = Vec::new();
    for root in roots.iter_mut() {
        tick_one(root, now, cx, &mut out);
    }
    out
}

fn tick_one<Msg: Clone + 'static>(
    instance: &mut Instance<Msg>,
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
    out: &mut Vec<Msg>,
) {
    let Instance {
        controller,
        handlers,
        node,
        children,
        ..
    } = instance;
    let mut ecx = EventCx {
        node,
        handlers,
        tree: cx.tree,
        styles: cx.styles,
        focus: cx.focus,
        clipboard: cx.clipboard,
        icons: cx.icons,
        fonts: cx.fonts,
        clock: cx.clock,
        env: cx.env,
        cmds: cx.cmds,
        phase: Phase::Target,
        handled: false,
    };
    out.extend(controller.tick(now, &mut ecx));
    for child in children {
        tick_one(child, now, cx, out);
    }
}

/// The earliest deadline any controller in the tree reports.
#[must_use]
pub fn next_controller_deadline<Msg: Clone + 'static>(
    roots: &[Instance<Msg>],
    now: Duration,
) -> Option<Duration> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        now: Duration,
        best: &mut Option<Duration>,
    ) {
        if let Some(deadline) = instance.controller.next_deadline(now) {
            *best = Some(best.map_or(deadline, |b: Duration| b.min(deadline)));
        }
        for child in &instance.children {
            walk(child, now, best);
        }
    }
    let mut best = None;
    for root in roots {
        walk(root, now, &mut best);
    }
    best
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::app`
Expected: PASS — 4 tests.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`a_click_on_the_inner_node_bubbles_to_the_outer_one`).** Delete the
`if ecx.handled { break; }`; the test must FAIL with
`[Msg::Inner, Msg::Outer]`. Restore. Record in the commit body.

**Second mutation check (load-bearing: `path_to_is_outermost_first_…`).**
Remove `out.pop()` from `walk`'s miss path; the test must FAIL with a path
longer than two, polluted by the abandoned branch. Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/app.rs ui/src/view/mod.rs
git commit -m "feat(ui): capture/target/bubble dispatch over the instance tree

deliver runs GTK's three phases along a hit path, with cx.handled ending
the phase it is set in (deviation D5); path_to builds that path and
backtracks cleanly out of dead branches; tick_all and
next_controller_deadline give the loop its clock-driven half.

Mutation checks: ignoring cx.handled lets a handled click bubble and
fails a_click_on_the_inner_node_bubbles_to_the_outer_one; dropping
path_to's backtracking pop fails path_to_is_outermost_first_and_ends_at_the_target.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 16: `App`, `ScriptStep`, `Frames` and `run_offscreen`

**Files:**
- Modify: `ui/src/view/app.rs` (append after `next_controller_deadline`)
- Modify: `ui/src/view/mod.rs` (re-export `App`, `AppError`, `Frames`, `ScriptStep`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/app.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–15.
- Produces:
  ```rust
  pub struct App<M, Msg> { /* private */ }
  impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
      pub fn new(model: M, update: fn(&mut M, Msg) -> Cmd<Msg>,
                 view: fn(&M) -> View<Msg>) -> Self;
      pub fn with_sheet(self, sheet: CompiledSheet) -> Self;          // D11
      pub fn with_fonts(self, fonts: FontDatabase) -> Self;           // D11
      pub fn with_icons(self, icons: IconTheme) -> Self;              // D11
      pub fn model(&self) -> &M;
      pub fn run_offscreen(self, size: (u32, u32), clock: Rc<ManualClock>,
                           script: Vec<ScriptStep<Msg>>) -> Result<Frames, AppError>;
  }
  pub enum ScriptStep<Msg> { Event(InputEvent), Advance(Duration), Message(Msg), Capture }
  pub struct Frames { /* private */ }
  impl Frames {
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
      pub fn pixel(&self, frame: usize, x: u32, y: u32) -> Option<(u8, u8, u8, u8)>;
      pub fn size(&self) -> (u32, u32);
  }
  pub enum AppError { Surface(SurfaceError), Layout(LayoutError) }
  ```

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/app.rs`'s test module:

```rust
    #[derive(Debug, Clone, PartialEq)]
    enum Count {
        Inc,
        Dec,
        Set(i32),
    }

    #[derive(Debug)]
    struct Model {
        n: i32,
    }

    fn update(model: &mut Model, msg: Count) -> Cmd<Count> {
        match msg {
            Count::Inc => model.n += 1,
            Count::Dec => model.n -= 1,
            Count::Set(v) => model.n = v,
        }
        Cmd::None
    }

    fn counter_view(model: &Model) -> View<Count> {
        widget::<Count>(Kind::Box)
            .key("root")
            .child(
                widget::<Count>(Kind::Button)
                    .key("plus")
                    .prop(PropName::Label, "+")
                    .on_click(Count::Inc),
            )
            .child(
                widget::<Count>(Kind::Label)
                    .key("value")
                    .prop(PropName::Label, model.n.to_string()),
            )
    }

    fn counter_app() -> App<Model, Count> {
        App::new(Model { n: 0 }, update, counter_view)
            .with_sheet(CompiledSheet::compile(
                "window { background-color: #ffffff; }
                 box { background-color: #ffffff; }
                 button { background-color: #3584e4; min-width: 20px; min-height: 20px; }
                 label { color: #000000; }",
            ))
            .with_fonts(crate::text::FontDatabase::probe_only())
            .with_icons(crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]))
    }

    #[test]
    fn a_scripted_message_folds_through_update_and_reaches_the_view() {
        let clock = Rc::new(crate::anim::ManualClock::new());
        let frames = counter_app()
            .run_offscreen(
                (80, 40),
                Rc::clone(&clock),
                vec![
                    ScriptStep::Capture,
                    ScriptStep::Message(Count::Set(7)),
                    ScriptStep::Capture,
                ],
            )
            .expect("the offscreen loop runs");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames.size(), (80, 40));
        // Two different models must not paint identical frames.
        let a: Vec<Option<(u8, u8, u8, u8)>> =
            (0..80).map(|x| frames.pixel(0, x, 20)).collect();
        let b: Vec<Option<(u8, u8, u8, u8)>> =
            (0..80).map(|x| frames.pixel(1, x, 20)).collect();
        assert_ne!(a, b, "the label did not repaint after the model changed");
    }

    #[test]
    fn identical_models_paint_identical_pixels() {
        let run = || {
            counter_app()
                .run_offscreen(
                    (60, 30),
                    Rc::new(crate::anim::ManualClock::new()),
                    vec![
                        ScriptStep::Message(Count::Set(3)),
                        ScriptStep::Capture,
                    ],
                )
                .expect("runs")
        };
        let first = run();
        let second = run();
        for y in 0..30 {
            for x in 0..60 {
                assert_eq!(
                    first.pixel(0, x, y),
                    second.pixel(0, x, y),
                    "pixel ({x}, {y}) differed between two runs of one model"
                );
            }
        }
    }

    #[test]
    fn a_message_produced_by_update_is_queued_never_folded_re_entrantly() {
        // `Cmd::After(ZERO)` fires on the next clock advance, not inside
        // `update` — the contract's "queued, never nested".
        fn update_after(model: &mut Model, msg: Count) -> Cmd<Count> {
            match msg {
                Count::Inc => {
                    model.n += 1;
                    if model.n < 3 {
                        return Cmd::After(Duration::ZERO, Rc::new(|| Count::Inc));
                    }
                    Cmd::None
                }
                Count::Dec => {
                    model.n -= 1;
                    Cmd::None
                }
                Count::Set(v) => {
                    model.n = v;
                    Cmd::None
                }
            }
        }

        let app = App::new(Model { n: 0 }, update_after, counter_view)
            .with_sheet(CompiledSheet::compile("box { color: #000; }"))
            .with_fonts(crate::text::FontDatabase::probe_only())
            .with_icons(crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]));
        let frames = app
            .run_offscreen(
                (40, 20),
                Rc::new(crate::anim::ManualClock::new()),
                vec![
                    ScriptStep::Message(Count::Inc),
                    ScriptStep::Advance(Duration::from_millis(1)),
                    ScriptStep::Advance(Duration::from_millis(1)),
                    ScriptStep::Capture,
                ],
            )
            .expect("runs");
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn a_synthetic_click_reaches_the_button_s_controller() {
        let frames = counter_app()
            .run_offscreen(
                (80, 40),
                Rc::new(crate::anim::ManualClock::new()),
                vec![
                    ScriptStep::Capture,
                    ScriptStep::Event(crate::window::InputEvent::PointerEnter {
                        x: 6.0,
                        y: 10.0,
                        serial: 1,
                    }),
                    ScriptStep::Event(crate::window::InputEvent::PointerButton {
                        button: crate::window::BTN_LEFT,
                        pressed: true,
                        serial: 2,
                        time_ms: 0,
                    }),
                    ScriptStep::Event(crate::window::InputEvent::PointerButton {
                        button: crate::window::BTN_LEFT,
                        pressed: false,
                        serial: 3,
                        time_ms: 10,
                    }),
                    ScriptStep::Capture,
                ],
            )
            .expect("runs");
        assert_eq!(frames.len(), 2);
        let before: Vec<Option<(u8, u8, u8, u8)>> =
            (0..80).map(|x| frames.pixel(0, x, 30)).collect();
        let after: Vec<Option<(u8, u8, u8, u8)>> =
            (0..80).map(|x| frames.pixel(1, x, 30)).collect();
        assert_ne!(before, after, "the click never incremented the counter");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::app::tests::a_scripted_message`
Expected: FAIL with `cannot find type 'App' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Append to `ui/src/view/app.rs`:

```rust
use std::collections::VecDeque;

use crate::anim::ManualClock;
use crate::css::cascade::CompiledSheet;
use crate::layout::{Container, LayoutError, Measure};
use crate::paint::{ImageCache, PaintCx};
use crate::view::reconcile::{containers_of, reconcile};
use crate::view::render::{
    Animations, NodeAddr, NodePainter, StyleMap, layout_tree, paint_tree, restyle_tree,
};
use crate::view::{Kind, PropName, View};
use crate::window::pointer::{ImplicitGrab, hit_chain};
use crate::window::{InputEvent, SurfaceError};

/// One step of an offscreen script.
pub enum ScriptStep<Msg> {
    /// Feed an input event, exactly as a `Window::pump` would have.
    Event(InputEvent),
    /// Move the `ManualClock` forward and run one frame.
    Advance(Duration),
    /// Inject a message as if a controller had produced it.
    Message(Msg),
    /// Render and keep the resulting RGBA frame.
    Capture,
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for ScriptStep<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptStep::Event(ev) => f.debug_tuple("Event").field(ev).finish(),
            ScriptStep::Advance(d) => f.debug_tuple("Advance").field(d).finish(),
            ScriptStep::Message(m) => f.debug_tuple("Message").field(m).finish(),
            ScriptStep::Capture => f.write_str("Capture"),
        }
    }
}

/// The frames a script captured.
#[derive(Debug, Default)]
pub struct Frames {
    size: (u32, u32),
    frames: Vec<Vec<u8>>,
}

impl Frames {
    /// How many frames were captured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether nothing was captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The surface size every frame shares.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// One pixel, as non-premultiplied-looking `(r, g, b, a)` straight out
    /// of skia's N32 buffer; `None` when the frame or the point is out of
    /// range.
    #[must_use]
    pub fn pixel(&self, frame: usize, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        let (w, h) = self.size;
        if x >= w || y >= h {
            return None;
        }
        let buffer = self.frames.get(frame)?;
        let offset = ((y as usize) * (w as usize) + (x as usize)) * 4;
        let bytes = buffer.get(offset..offset + 4)?;
        Some((bytes[0], bytes[1], bytes[2], bytes[3]))
    }
}

/// What can go wrong running an app.
#[derive(Debug)]
pub enum AppError {
    /// The surface or its connection failed.
    Surface(SurfaceError),
    /// Layout failed.
    Layout(LayoutError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Surface(e) => write!(f, "surface: {e}"),
            AppError::Layout(e) => write!(f, "layout: {e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<SurfaceError> for AppError {
    fn from(value: SurfaceError) -> Self {
        AppError::Surface(value)
    }
}
impl From<LayoutError> for AppError {
    fn from(value: LayoutError) -> Self {
        AppError::Layout(value)
    }
}

/// The Elm loop over a retained tree.
pub struct App<M, Msg> {
    model: M,
    update: fn(&mut M, Msg) -> Cmd<Msg>,
    view: fn(&M) -> View<Msg>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
}

/// The mutable half of a running app, shared by `run` and `run_offscreen`.
struct Runtime<Msg> {
    root: Node,
    instances: Vec<Instance<Msg>>,
    styles: StyleMap,
    anims: Animations,
    containers: std::collections::HashMap<NodeAddr, Container>,
    layout: LayoutTree,
    focus: FocusRing,
    grab: ImplicitGrab,
    hovered: Option<Node>,
    /// The last pointer position in window-frame space, so a button event
    /// (which carries none) can be aimed and localised.
    last_pointer: (f32, f32),
    queue: VecDeque<Msg>,
    cmds: Vec<Cmd<Msg>>,
    timers: Vec<(Duration, Rc<dyn Fn() -> Msg>)>,
    images: ImageCache,
    env: ResolveEnv,
    quit: bool,
}

/// Bridges `Controller::measure` into taffy.
struct ControllerMeasure<'a, Msg> {
    instances: &'a mut Vec<Instance<Msg>>,
    sheet: &'a CompiledSheet,
    fonts: &'a mut FontDatabase,
    icons: &'a mut IconTheme,
    clock: &'a Rc<dyn Clock>,
    env: &'a ResolveEnv,
}

impl<Msg: Clone + 'static> Measure for ControllerMeasure<'_, Msg> {
    fn measure(
        &mut self,
        node: &Node,
        _style: &crate::css::computed::ComputedStyle,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        let cap = |known: Option<f32>, space: taffy::AvailableSpace| match (known, space) {
            (Some(v), _) => Some(v),
            (None, taffy::AvailableSpace::Definite(v)) => Some(v),
            _ => None,
        };
        let mut cx = BuildCx {
            sheet: self.sheet,
            fonts: self.fonts,
            icons: self.icons,
            clock: self.clock,
            env: self.env,
        };
        let measured = self
            .instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
            .and_then(|instance| {
                instance.controller.measure(
                    (cap(known.width, available.width), cap(known.height, available.height)),
                    &mut cx,
                )
            });
        match measured {
            Some((w, h)) => taffy::Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(h),
            },
            None => taffy::Size {
                width: known.width.unwrap_or(0.0),
                height: known.height.unwrap_or(0.0),
            },
        }
    }
}

/// Bridges `Controller::paint` into `paint_tree`.
struct ControllerPainter<'a, Msg> {
    instances: &'a mut Vec<Instance<Msg>>,
}

impl<Msg: Clone + 'static> NodePainter for ControllerPainter<'_, Msg> {
    fn paint_content(
        &mut self,
        node: &Node,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        self.instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
            .is_some_and(|instance| instance.controller.paint(canvas, alloc, style, cx))
    }
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// A new app over `model`, folded by `update`, described by `view`.
    #[must_use]
    pub fn new(
        model: M,
        update: fn(&mut M, Msg) -> Cmd<Msg>,
        view: fn(&M) -> View<Msg>,
    ) -> Self {
        App {
            model,
            update,
            view,
            sheet: None,
            fonts: None,
            icons: None,
        }
    }

    /// The compiled theme `run_offscreen` styles with (deviation D11).
    /// `run` takes the window's instead.
    #[must_use]
    pub fn with_sheet(mut self, sheet: CompiledSheet) -> Self {
        self.sheet = Some(sheet);
        self
    }

    /// The font database `run_offscreen` shapes with (deviation D11).
    #[must_use]
    pub fn with_fonts(mut self, fonts: FontDatabase) -> Self {
        self.fonts = Some(fonts);
        self
    }

    /// The icon theme both loops resolve icons through (deviation D11).
    #[must_use]
    pub fn with_icons(mut self, icons: IconTheme) -> Self {
        self.icons = Some(icons);
        self
    }

    /// The current model — what an offscreen test asserts on besides pixels.
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Run the whole loop against an offscreen raster surface with no
    /// Wayland connection, driven by `script` on a [`ManualClock`].
    ///
    /// This is how the counter-app test and every controller unit test run.
    ///
    /// # Errors
    ///
    /// [`AppError::Surface`] if the raster surface cannot be created;
    /// [`AppError::Layout`] if taffy fails.
    pub fn run_offscreen(
        mut self,
        size: (u32, u32),
        clock: Rc<ManualClock>,
        script: Vec<ScriptStep<Msg>>,
    ) -> Result<Frames, AppError> {
        let sheet = self
            .sheet
            .take()
            .unwrap_or_else(|| CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT));
        let mut fonts = self.fonts.take().unwrap_or_else(FontDatabase::new);
        let mut icons = self.icons.take().unwrap_or_else(IconTheme::from_env);
        let mut clipboard = Clipboard::offscreen();
        let dyn_clock: Rc<dyn Clock> = clock.clone();

        let mut rt = Runtime {
            root: Node::with_classes(Kind::Window.css_name(), Kind::Window.base_classes()),
            instances: Vec::new(),
            styles: StyleMap::new(),
            anims: Animations::new(),
            containers: std::collections::HashMap::new(),
            layout: LayoutTree::new(),
            focus: FocusRing::default(),
            grab: ImplicitGrab::default(),
            hovered: None,
            last_pointer: (0.0, 0.0),
            queue: VecDeque::new(),
            cmds: Vec::new(),
            timers: Vec::new(),
            images: ImageCache::new(),
            env: ResolveEnv::default(),
            quit: false,
        };

        let mut surface = skia_rs_safe::canvas::Surface::new_raster_n32_premul(
            i32::try_from(size.0).unwrap_or(i32::MAX),
            i32::try_from(size.1).unwrap_or(i32::MAX),
        )
        .ok_or(SurfaceError::Render("offscreen raster surface"))?;
        let mut frames = Frames {
            size,
            frames: Vec::new(),
        };

        rebuild(&mut self, &mut rt, &sheet, &mut fonts, &mut icons, &dyn_clock);
        render_once(
            &mut rt, &sheet, &mut fonts, &mut icons, &dyn_clock, size, &mut surface,
        )?;

        for step in script {
            let mut capture = false;
            match step {
                ScriptStep::Capture => capture = true,
                ScriptStep::Message(msg) => rt.queue.push_back(msg),
                ScriptStep::Event(ev) => {
                    let produced = route(
                        &mut rt, &ev, &sheet, &mut fonts, &mut icons, &mut clipboard, &dyn_clock,
                    );
                    rt.queue.extend(produced);
                }
                ScriptStep::Advance(delta) => {
                    let before = clock.now();
                    clock.set_ms(
                        u64::try_from((before + delta).as_millis()).unwrap_or(u64::MAX),
                    );
                    let now = clock.now();
                    let due: Vec<Rc<dyn Fn() -> Msg>> = {
                        let (fired, pending): (Vec<_>, Vec<_>) =
                            std::mem::take(&mut rt.timers)
                                .into_iter()
                                .partition(|(at, _)| *at <= now);
                        rt.timers = pending;
                        fired.into_iter().map(|(_, f)| f).collect()
                    };
                    rt.queue.extend(due.iter().map(|f| f()));
                    let ticked = {
                        let mut cx = Dispatch {
                            styles: &rt.styles,
                            tree: &rt.layout,
                            focus: &mut rt.focus,
                            clipboard: &mut clipboard,
                            icons: &mut icons,
                            fonts: &mut fonts,
                            clock: &dyn_clock,
                            env: &rt.env,
                            cmds: &mut rt.cmds,
                        };
                        tick_all(&mut rt.instances, now, &mut cx)
                    };
                    rt.queue.extend(ticked);
                }
            }

            drain(
                &mut self, &mut rt, &sheet, &mut fonts, &mut icons, &mut clipboard, &dyn_clock,
                clock.now(),
            );
            render_once(
                &mut rt, &sheet, &mut fonts, &mut icons, &dyn_clock, size, &mut surface,
            )?;

            if capture {
                frames.frames.push(read_rgba(&surface, size));
            }
            if rt.quit {
                break;
            }
        }
        Ok(frames)
    }
}
```

Add the five helpers the loop calls, still in `ui/src/view/app.rs`:

```rust
/// Run `view`, reconcile it into the retained tree, and refresh the
/// container map the layout walker reads.
fn rebuild<M: 'static, Msg: Clone + 'static>(
    app: &mut App<M, Msg>,
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
) {
    let described = (app.view)(&app.model);
    let mut cx = BuildCx {
        sheet,
        fonts,
        icons,
        clock,
        env: &rt.env,
    };
    reconcile(&rt.root, &mut rt.instances, vec![described], &mut cx);
    rt.containers.clear();
    rt.containers.insert(
        crate::view::render::node_addr(&rt.root),
        Container::Box {
            direction: crate::layout::BoxDirection::Column,
        },
    );
    containers_of(&rt.instances, &mut rt.containers);
}

/// Restyle, relayout and repaint into `surface`.
fn render_once<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
    size: (u32, u32),
    surface: &mut skia_rs_safe::canvas::Surface,
) -> Result<(), AppError> {
    let now = clock.now();
    restyle_tree(&rt.root, sheet, &rt.env, &mut rt.styles, &mut rt.anims, now);
    {
        let mut measure = ControllerMeasure {
            instances: &mut rt.instances,
            sheet,
            fonts,
            icons,
            clock,
            env: &rt.env,
        };
        layout_tree(
            &rt.root,
            &rt.styles,
            &rt.containers,
            &mut rt.layout,
            &rt.env,
            (
                Some(size.0 as f32),
                Some(size.1 as f32),
            ),
            &mut measure,
        )?;
    }
    surface.canvas().clear(skia_rs_safe::core::Color::TRANSPARENT);
    let mut cx = PaintCx {
        env: &rt.env,
        colors: &sheet.colors,
        fonts,
        images: &mut rt.images,
        text: None,
    };
    let mut painter = ControllerPainter {
        instances: &mut rt.instances,
    };
    let mut canvas = surface.canvas();
    paint_tree(
        &mut canvas,
        &rt.root,
        &rt.styles,
        &rt.layout,
        &mut rt.anims,
        now,
        (0.0, 0.0),
        &mut cx,
        &mut painter,
    );
    Ok(())
}

/// Turn one `InputEvent` into controller events and return the messages.
#[allow(
    clippy::too_many_arguments,
    reason = "one routing pass threading the whole per-frame context"
)]
fn route<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    event: &InputEvent,
    _sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
    // The node an event is aimed at, and the point in its own space.
    let aim = |rt: &Runtime<Msg>, x: f32, y: f32| -> Option<(Node, (f32, f32))> {
        if let Some(target) = rt.grab.target() {
            let local = rt
                .layout
                .allocation(&target)
                .map_or((x, y), |a| (x - a.border_box.x, y - a.border_box.y));
            return Some((target, local));
        }
        let chain = hit_chain(&rt.root, &rt.layout, &rt.styles, (x, y));
        chain.last().map(|hit| (hit.node.clone(), hit.local))
    };

    let mut pending: Vec<(Node, Event)> = Vec::new();
    match event {
        InputEvent::PointerEnter { x, y, .. } | InputEvent::PointerMotion { x, y, .. } => {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "surface coordinates are within f32 exactly"
            )]
            let point = (*x as f32, *y as f32);
            rt.last_pointer = point;
            let target = aim(rt, point.0, point.1);
            let same = match (rt.hovered.as_ref(), target.as_ref()) {
                (Some(old), Some((new, _))) => old.ptr_eq(new),
                (None, None) => true,
                _ => false,
            };
            if !same {
                if let Some(old) = rt.hovered.take() {
                    pending.push((old, Event::PointerLeave));
                }
                if let Some((node, local)) = target.clone() {
                    pending.push((node.clone(), Event::PointerEnter { local }));
                    rt.hovered = Some(node);
                }
            }
            if let Some((node, local)) = target {
                pending.push((node, Event::PointerMotion { local }));
            }
        }
        InputEvent::PointerLeave => {
            if let Some(old) = rt.hovered.take() {
                pending.push((old, Event::PointerLeave));
            }
        }
        InputEvent::PointerButton { button, pressed, serial, .. } => {
            // `wl_pointer.button` carries no coordinates: the position is
            // the last motion's, and the target is the grab's while one is
            // held, exactly as the compositor's implicit grab decides it.
            let (x, y) = rt.last_pointer;
            if let Some((node, local)) = aim(rt, x, y) {
                if *pressed {
                    rt.grab.press(*button, &node);
                    pending.push((
                        node,
                        Event::PointerDown { button: *button, local, serial: *serial },
                    ));
                } else {
                    rt.grab.release(*button);
                    pending.push((
                        node,
                        Event::PointerUp { button: *button, local, serial: *serial },
                    ));
                }
            }
        }
        InputEvent::Scroll(scroll) => {
            if let Some(node) = rt.hovered.clone() {
                pending.push((node, Event::Scroll(*scroll)));
            }
        }
        InputEvent::Key(key) => {
            if let Some(node) = rt.focus.focus() {
                pending.push((node, Event::Key(key.clone())));
            }
        }
        InputEvent::Configure { size, states } => {
            for instance in &rt.instances {
                pending.push((
                    instance.node.clone(),
                    Event::Configure { size: *size, states: *states },
                ));
            }
        }
        InputEvent::PopupDone(_) => {
            if let Some(node) = rt.focus.focus() {
                pending.push((node, Event::PopupDone));
            }
        }
        _ => {}
    }

    let mut out = Vec::new();
    for (node, ev) in pending {
        let path = path_to(&rt.instances, &node);
        let mut cx = Dispatch {
            styles: &rt.styles,
            tree: &rt.layout,
            focus: &mut rt.focus,
            clipboard,
            icons,
            fonts,
            clock,
            env: &rt.env,
            cmds: &mut rt.cmds,
        };
        out.extend(deliver(&mut rt.instances, &path, &ev, &mut cx));
    }
    out
}

/// Fold every queued message, run the commands they produced, and rebuild
/// the view **once** per drained batch (contract §4.7).
#[allow(
    clippy::too_many_arguments,
    reason = "one fold pass threading the whole per-frame context"
)]
fn drain<M: 'static, Msg: Clone + 'static>(
    app: &mut App<M, Msg>,
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
    now: Duration,
) {
    if rt.queue.is_empty() && rt.cmds.is_empty() {
        return;
    }
    let mut folded = false;
    // A bound, so an `update` that enqueues on every message cannot wedge
    // the loop: the rest is carried to the next frame.
    for _ in 0..1024 {
        let Some(msg) = rt.queue.pop_front() else {
            break;
        };
        let cmd = (app.update)(&mut app.model, msg);
        rt.cmds.push(cmd);
        folded = true;
    }
    for cmd in std::mem::take(&mut rt.cmds).into_iter().flat_map(Cmd::flatten) {
        match cmd {
            Cmd::After(delay, f) => rt.timers.push((now + delay, f)),
            Cmd::Copy(text) => clipboard.copy(&text, 0),
            Cmd::Paste(f) => {
                let value = clipboard.paste(Duration::from_millis(50));
                rt.queue.push_back(f(value));
            }
            Cmd::SetPrimary(text) => clipboard.set_primary(&text, 0),
            Cmd::Primary(f) => {
                let value = clipboard.primary(Duration::from_millis(50));
                rt.queue.push_back(f(value));
            }
            Cmd::Focus(node) => rt
                .focus
                .set_focus(Some(&node), crate::window::focus::FocusCause::Programmatic),
            Cmd::Quit | Cmd::CloseWindow => rt.quit = true,
            // The window-bound commands are `run`'s (Task 17); offscreen
            // there is no surface to title, minimise or open a popup on, so
            // they are recorded and ignored.
            other => tracing::debug!(?other, "command has no effect offscreen"),
        }
    }
    if folded {
        rebuild(app, rt, sheet, fonts, icons, clock);
    }
}

/// Copy the surface out as tightly packed RGBA.
fn read_rgba(surface: &skia_rs_safe::canvas::Surface, size: (u32, u32)) -> Vec<u8> {
    let buffer = surface.pixel_buffer();
    let mut out = Vec::with_capacity((size.0 as usize) * (size.1 as usize) * 4);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let skia_rs_safe::core::Color(argb) = buffer
                .get_pixel(
                    i32::try_from(x).unwrap_or(0),
                    i32::try_from(y).unwrap_or(0),
                )
                .unwrap_or(skia_rs_safe::core::Color(0));
            out.push(((argb >> 16) & 0xFF) as u8);
            out.push(((argb >> 8) & 0xFF) as u8);
            out.push((argb & 0xFF) as u8);
            out.push(((argb >> 24) & 0xFF) as u8);
        }
    }
    out
}
```

Finally, add to `ui/src/view/mod.rs`:

```rust
pub use app::{App, AppError, Frames, ScriptStep};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::app`
Expected: PASS — 8 tests.
Run: `cargo test -p icedtea-ui` — everything green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing: `identical_models_paint_identical_pixels`).**
Seed `Runtime.env` with `ResolveEnv { dpi: 96.0 + (frames.len() as f32),
..Default::default() }` (make the render depend on run state); the test must
FAIL on a differing pixel. Restore. Record in the commit body.

**Second mutation check (load-bearing:
`a_message_produced_by_update_is_queued_never_folded_re_entrantly`).** In
`drain`, replace the `Cmd::After` arm with an immediate
`rt.queue.push_front(f())` and re-enter the fold in the same pass; the test
must FAIL (the counter reaches 3 in one step and the frame count changes).
Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/app.rs ui/src/view/mod.rs
git commit -m "feat(ui): App, the offscreen loop, ScriptStep and Frames

run_offscreen drives the whole loop against a raster surface with no
Wayland connection, on a ManualClock: events route through the implicit
grab and the hit chain, messages fold one at a time and are always
queued (never nested), commands are interpreted after the fold, and the
view is rebuilt once per drained batch. ControllerMeasure and
ControllerPainter are the two adapters that let taffy and paint_tree
reach a controller without knowing what an Instance is.

Deviation D11 adds with_sheet/with_fonts/with_icons, since §4.7's
App::new carries no theme and run_offscreen takes none.

Mutation checks: making the render depend on run state fails
identical_models_paint_identical_pixels; folding a Cmd::After message
re-entrantly fails a_message_produced_by_update_is_queued_never_folded_re_entrantly.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 17: `App::run` — the live Wayland loop

**Files:**
- Modify: `ui/src/window/mod.rs` (add `Window::paint_with` — deviation D2)
- Modify: `ui/src/view/app.rs` (append `App::run`)
- Test: inline `#[cfg(test)] mod tests` in `ui/src/view/app.rs`, plus the
  end-to-end assertion in Task 18

**Interfaces:**
- Consumes: `Window`, `InputEvent`, `SurfaceError`, `PopupAnchorPoint`,
  `Positioner`, `PopupKey` (P3, contract §3.1–§3.2); everything from Task 16.
- Produces:
  ```rust
  // ui/src/window/mod.rs
  impl Window {
      pub fn paint_with(
          &mut self,
          f: impl FnOnce(&mut skia_rs_safe::canvas::Surface),
      ) -> Result<bool, SurfaceError>;
  }
  // ui/src/view/app.rs
  impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
      pub fn run(self, window: Window) -> Result<(), AppError>;   // D1
  }
  ```
  `paint_with` acquires a free buffer, hands the window's own skia surface to
  `f`, uploads, attaches and commits; `Ok(false)` means no buffer was free
  and the caller should retry next frame.

- [ ] **Step 1: Write the failing test**

Add to `ui/src/view/app.rs`'s test module:

```rust
    #[test]
    fn the_frame_deadline_folds_animations_timers_and_controllers() {
        // `App::run`'s wait is `min(window deadline, animation deadline,
        // timer deadline, controller deadline)`. The pure computation is
        // exercised here; the socket half is Task 18's e2e.
        let now = Duration::from_millis(100);
        assert_eq!(
            frame_deadline(
                Some(Duration::from_millis(16)),
                Some(Duration::from_millis(8)),
                &[(Duration::from_millis(150), ())],
                Some(Duration::from_millis(30)),
                now,
            ),
            Some(Duration::from_millis(8))
        );
        // A timer already due is "now", not "spin": ZERO is a legitimate
        // answer and the loop must not treat it as an error.
        assert_eq!(
            frame_deadline(None, None, &[(Duration::from_millis(50), ())], None, now),
            Some(Duration::ZERO)
        );
        // Nothing pending: block until an event arrives.
        assert_eq!(frame_deadline(None, None, &[], None, now), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib view::app::tests::the_frame_deadline`
Expected: FAIL with `cannot find function 'frame_deadline' in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `ui/src/window/mod.rs`, in `impl Window` (deviation D2 — additive):

```rust
    /// Paint one frame with `f` and commit it.
    ///
    /// Acquires a free buffer from the pool, clears the window's own skia
    /// surface, runs `f` against it, uploads the result and commits.
    /// `Ok(false)` means every buffer was still held by the compositor and
    /// nothing was painted — the caller retries on the next frame callback.
    ///
    /// Contract deviation D2: `Window::render` paints the window's own tree,
    /// but §9 gives the recursive paint walker to P4, so the reactive layer
    /// needs a way to paint *its* tree through the same buffer machinery.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] if the buffer pool cannot grow,
    /// [`SurfaceError::Closed`] if the surface is gone.
    pub fn paint_with(
        &mut self,
        f: impl FnOnce(&mut skia_rs_safe::canvas::Surface),
    ) -> Result<bool, SurfaceError> {
        if self.is_closed() {
            return Err(SurfaceError::Closed);
        }
        let Some(slot) = self.acquire_slot()? else {
            return Ok(false);
        };
        self.skia.canvas().clear(skia_rs_safe::core::Color::TRANSPARENT);
        f(&mut self.skia);
        self.upload_and_commit(slot)?;
        Ok(true)
    }
```

`acquire_slot` and `upload_and_commit` are P3's existing private helpers —
the same pair `Window::render` already uses; `paint_with` only replaces the
middle of that sandwich.

Append to `ui/src/view/app.rs`:

```rust
/// The shortest of the four things that could want the next frame.
///
/// `Duration::ZERO` is a legitimate "now" — an interpolating transition or
/// an already-due timer — and must never be read as "spin".
fn frame_deadline<T>(
    window: Option<Duration>,
    animation: Option<Duration>,
    timers: &[(Duration, T)],
    controllers: Option<Duration>,
    now: Duration,
) -> Option<Duration> {
    let soonest_timer = timers
        .iter()
        .map(|(at, _)| at.saturating_sub(now))
        .min();
    [window, animation, soonest_timer, controllers]
        .into_iter()
        .flatten()
        .min()
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Run the loop against a live `Window`.
    ///
    /// Contract deviation D1: §4.7 writes `run(self, surface: Surface)`, but
    /// `Window` — not `Surface` — owns `pump`, `next_deadline`, `open_popup`,
    /// `clipboard` and the root node the loop needs, and there is no public
    /// way to get one from the other.
    ///
    /// Each iteration: wait up to [`frame_deadline`] for events, route them
    /// to controllers, fold the messages one at a time, run their commands,
    /// rebuild the view once, then restyle, relayout and repaint the dirty
    /// tree into the window's next buffer.
    ///
    /// # Errors
    ///
    /// [`AppError::Surface`] on a protocol or buffer failure,
    /// [`AppError::Layout`] if taffy fails.
    pub fn run(mut self, mut window: Window) -> Result<(), AppError> {
        let sheet = self.sheet.take().unwrap_or_else(|| window.sheet().clone());
        let mut fonts = self
            .fonts
            .take()
            .unwrap_or_else(|| std::mem::take(window.fonts()));
        let mut icons = self.icons.take().unwrap_or_else(IconTheme::from_env);
        let clock: Rc<dyn Clock> = Rc::clone(window.clock());

        let mut rt = Runtime {
            root: window.root().clone(),
            instances: Vec::new(),
            styles: StyleMap::new(),
            anims: Animations::new(),
            containers: std::collections::HashMap::new(),
            layout: LayoutTree::new(),
            focus: FocusRing::default(),
            grab: ImplicitGrab::default(),
            hovered: None,
            last_pointer: (0.0, 0.0),
            queue: VecDeque::new(),
            cmds: Vec::new(),
            timers: Vec::new(),
            images: ImageCache::new(),
            env: ResolveEnv::default(),
            quit: false,
        };

        rebuild(&mut self, &mut rt, &sheet, &mut fonts, &mut icons, &clock);

        while !rt.quit && !window.is_closed() {
            let now = clock.now();
            let wait = frame_deadline(
                window.next_deadline(),
                rt.anims.next_deadline(now),
                &rt.timers,
                next_controller_deadline(&rt.instances, now),
                now,
            );
            let events = window.pump(wait)?;

            for event in &events {
                if matches!(event, InputEvent::Close) {
                    rt.quit = true;
                }
                let produced = {
                    let clipboard = window.clipboard();
                    route(
                        &mut rt, event, &sheet, &mut fonts, &mut icons, clipboard, &clock,
                    )
                };
                rt.queue.extend(produced);
            }

            let now = clock.now();
            let due: Vec<Msg> = {
                let (fired, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut rt.timers)
                    .into_iter()
                    .partition(|(at, _)| *at <= now);
                rt.timers = pending;
                fired.into_iter().map(|(_, f)| f()).collect()
            };
            rt.queue.extend(due);

            let ticked = {
                let clipboard = window.clipboard();
                let mut cx = Dispatch {
                    styles: &rt.styles,
                    tree: &rt.layout,
                    focus: &mut rt.focus,
                    clipboard,
                    icons: &mut icons,
                    fonts: &mut fonts,
                    clock: &clock,
                    env: &rt.env,
                    cmds: &mut rt.cmds,
                };
                tick_all(&mut rt.instances, now, &mut cx)
            };
            rt.queue.extend(ticked);

            // Window-bound commands the offscreen loop ignores.
            let window_cmds: Vec<Cmd<Msg>> = std::mem::take(&mut rt.cmds)
                .into_iter()
                .flat_map(Cmd::flatten)
                .filter_map(|cmd| match cmd {
                    Cmd::SetTitle(title) => {
                        window.set_title(&title);
                        None
                    }
                    Cmd::Minimize => {
                        window.minimize();
                        None
                    }
                    Cmd::ToggleMaximized => {
                        window.toggle_maximized();
                        None
                    }
                    Cmd::CloseWindow => {
                        rt.quit = true;
                        None
                    }
                    Cmd::OpenPopup { anchor, positioner, .. } => {
                        if let Err(error) = window.open_popup(anchor, positioner) {
                            tracing::warn!(?error, "opening a popup failed");
                        }
                        None
                    }
                    Cmd::ClosePopup(key) => {
                        window.close_popup(key);
                        None
                    }
                    other => Some(other),
                })
                .collect();
            rt.cmds = window_cmds;

            {
                let clipboard = window.clipboard();
                drain(
                    &mut self, &mut rt, &sheet, &mut fonts, &mut icons, clipboard, &clock, now,
                );
            }

            let (w, h) = window.size();
            restyle_tree(&rt.root, &sheet, &rt.env, &mut rt.styles, &mut rt.anims, now);
            {
                let mut measure = ControllerMeasure {
                    instances: &mut rt.instances,
                    sheet: &sheet,
                    fonts: &mut fonts,
                    icons: &mut icons,
                    clock: &clock,
                    env: &rt.env,
                };
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a surface is never 2^24 px on a side"
                )]
                layout_tree(
                    &rt.root,
                    &rt.styles,
                    &rt.containers,
                    &mut rt.layout,
                    &rt.env,
                    (Some(w as f32), Some(h as f32)),
                    &mut measure,
                )?;
            }

            let styles = &rt.styles;
            let layout = &rt.layout;
            let anims = &mut rt.anims;
            let images = &mut rt.images;
            let instances = &mut rt.instances;
            let root = &rt.root;
            let env = &rt.env;
            let fonts_ref = &mut fonts;
            window.paint_with(|surface| {
                let mut cx = PaintCx {
                    env,
                    colors: &sheet.colors,
                    fonts: fonts_ref,
                    images,
                    text: None,
                };
                let mut painter = ControllerPainter { instances };
                let mut canvas = surface.canvas();
                paint_tree(
                    &mut canvas, root, styles, layout, anims, now, (0.0, 0.0), &mut cx,
                    &mut painter,
                );
            })?;
        }
        Ok(())
    }
}
```

`Window::{set_title, minimize, toggle_maximized, clock, sheet, fonts, size,
root, clipboard, open_popup, close_popup, pump, next_deadline, is_closed}` are
all P3's, from contract §3.1–§3.2; `set_title`, `minimize` and
`toggle_maximized` are the `xdg_toplevel` requests §3.2's table already lists,
exposed on `Window` by P3.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib view::app`
Expected: PASS — 9 tests.
Run: `cargo test -p icedtea-ui` — everything green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.

**Mutation check (load-bearing:
`the_frame_deadline_folds_animations_timers_and_controllers`).** Drop the
`animation` entry from `frame_deadline`'s array; the test must FAIL with
`Some(16ms)` instead of `Some(8ms)`. Restore. Record in the commit body.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/app.rs ui/src/window/mod.rs
git commit -m "feat(ui): App::run, the live Wayland loop

run takes a Window (deviation D1: Window, not Surface, owns pump,
next_deadline, open_popup, clipboard and the root node §4.7's loop needs,
and there is no public Surface->Window path), waits the shortest of the
window, animation, timer and controller deadlines, routes events, folds
messages, runs the window-bound commands the offscreen loop cannot, and
repaints through Window::paint_with — deviation D2's additive accessor,
which reuses P3's own buffer-acquire/upload/commit sandwich.

Mutation check: dropping the animation deadline from the fold fails
the_frame_deadline_folds_animations_timers_and_controllers.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 18: the counter-app gate, the crate docs and the README

**Files:**
- Create: `ui/tests/counter_app.rs`
- Modify: `ui/README.md`
- Modify: `ui/src/view/mod.rs` (module-level docs)

**Interfaces:**
- Consumes: `App`, `ScriptStep`, `Frames` (Task 16); `widget`, `Kind`,
  `PropName` (Tasks 2, 5); `InputEvent`, `BTN_LEFT` (P3).
- Produces: contract §4.8's "offscreen counter app driven by `run_offscreen`
  with pixel assertions on the label". Nothing later in M3 depends on it.

- [ ] **Step 1: Write the failing test**

Create `ui/tests/counter_app.rs`:

```rust
//! Contract §4.8's counter app: the whole reactive stack — builders,
//! reconciler, controllers, dispatch, restyle, layout, paint — driven
//! offscreen by a script, asserted on pixels.

use std::rc::Rc;

use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::widget;
use icedtea_ui::view::{App, Cmd, Frames, Kind, PropName, ScriptStep, View};
use icedtea_ui::window::{BTN_LEFT, InputEvent};

const W: u32 = 160;
const H: u32 = 60;

/// A sheet with no gradients and no rounding, so a pixel assertion reads
/// exactly one declared colour.
const SHEET: &str = "
    window { background-color: #ffffff; }
    box { background-color: #ffffff; padding: 0; }
    button { background-color: #3584e4; min-width: 40px; min-height: 40px;
             border: 0; padding: 0; color: #ffffff; }
    label { color: #000000; min-width: 40px; min-height: 40px; }
";

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Inc,
    Dec,
}

#[derive(Debug)]
struct Model {
    n: i32,
}

fn update(model: &mut Model, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::Inc => model.n += 1,
        Msg::Dec => model.n -= 1,
    }
    Cmd::None
}

fn view(model: &Model) -> View<Msg> {
    widget::<Msg>(Kind::Box)
        .key("root")
        .child(
            widget::<Msg>(Kind::Button)
                .key("dec")
                .prop(PropName::Label, "-")
                .on_click(Msg::Dec),
        )
        .child(
            widget::<Msg>(Kind::Label)
                .key("value")
                .prop(PropName::Label, model.n.to_string()),
        )
        .child(
            widget::<Msg>(Kind::Button)
                .key("inc")
                .prop(PropName::Label, "+")
                .on_click(Msg::Inc),
        )
}

fn app() -> App<Model, Msg> {
    App::new(Model { n: 0 }, update, view)
        .with_sheet(CompiledSheet::compile(SHEET))
        .with_fonts(FontDatabase::new())
        .with_icons(IconTheme::with_name_and_roots("hicolor", vec![]))
}

fn run(script: Vec<ScriptStep<Msg>>) -> Frames {
    app()
        .run_offscreen(( W, H ), Rc::new(ManualClock::new()), script)
        .expect("the counter app runs offscreen")
}

/// Click at `(x, y)`: enter, press, release.
fn click(x: f64, y: f64) -> Vec<ScriptStep<Msg>> {
    vec![
        ScriptStep::Event(InputEvent::PointerEnter { x, y, serial: 1 }),
        ScriptStep::Event(InputEvent::PointerMotion { x, y, time_ms: 0 }),
        ScriptStep::Event(InputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: true,
            serial: 2,
            time_ms: 1,
        }),
        ScriptStep::Event(InputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: false,
            serial: 3,
            time_ms: 2,
        }),
    ]
}

/// How many pixels in the label column are dark — the glyph coverage the
/// contract's "pixel assertions on the label" means.
fn dark_pixels(frames: &Frames, frame: usize) -> usize {
    let mut count = 0;
    for y in 0..H {
        for x in 40..80 {
            if let Some((r, g, b, a)) = frames.pixel(frame, x, y) {
                if a > 0x80 && r < 0x60 && g < 0x60 && b < 0x60 {
                    count += 1;
                }
            }
        }
    }
    count
}

#[test]
fn the_counter_renders_its_three_children_in_order() {
    let frames = run(vec![ScriptStep::Capture]);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames.size(), (W, H));

    // The two buttons are Adwaita accent blue; the label between them is not.
    let (r, g, b, a) = frames.pixel(0, 20, 20).expect("inside the '-' button");
    assert_eq!((r, g, b, a), (0x35, 0x84, 0xE4, 0xFF));
    let (r, g, b, a) = frames.pixel(0, 100, 20).expect("inside the '+' button");
    assert_eq!((r, g, b, a), (0x35, 0x84, 0xE4, 0xFF));
    let (r, g, b, _) = frames.pixel(0, 60, 2).expect("above the label glyph");
    assert_eq!((r, g, b), (0xFF, 0xFF, 0xFF), "the label sits on the box background");
}

#[test]
fn clicking_plus_repaints_the_label_with_the_new_model() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(100.0, 20.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    assert_eq!(frames.len(), 2);

    let before = dark_pixels(&frames, 0);
    let after = dark_pixels(&frames, 1);
    assert!(before > 0, "the '0' label never rendered any glyph pixels");
    assert!(after > 0, "the '1' label never rendered any glyph pixels");
    assert_ne!(
        before, after,
        "'0' and '1' produced identical glyph coverage, so the label did not repaint"
    );
}

#[test]
fn clicking_minus_and_plus_returns_to_the_starting_pixels() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(20.0, 20.0));
    script.extend(click(100.0, 20.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    assert_eq!(frames.len(), 2);
    for y in 0..H {
        for x in 0..W {
            assert_eq!(
                frames.pixel(0, x, y),
                frames.pixel(1, x, y),
                "pixel ({x}, {y}) differs after -1 then +1 returned the model to 0"
            );
        }
    }
}

#[test]
fn a_click_outside_every_child_changes_nothing() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(159.0, 59.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    for y in 0..H {
        for x in 0..W {
            assert_eq!(frames.pixel(0, x, y), frames.pixel(1, x, y));
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test counter_app`
Expected: FAIL — `unresolved import icedtea_ui::view::Cmd` if the re-export is
missing (add `pub use cmd::Cmd;` — Task 8 already did), and then
`the_counter_renders_its_three_children_in_order` fails on the accent-blue
assertion until the whole stack from Tasks 1–17 is in place. With Tasks 1–17
merged it passes on the first run; if it does not, the failure names the
seam — a wrong pixel is layout or paint, a missing glyph is the measure path,
identical frames are the reconciler or `update`.

- [ ] **Step 3: Write minimal implementation**

No production code changes. Documentation only.

Replace the placeholder doc comment at the top of `ui/src/view/mod.rs` with
the module overview:

```rust
//! The reactive framework: `View` description trees, keyed reconciliation
//! into M2's retained [`Node`](crate::css::node::Node)s, per-kind
//! controllers, and the `App` loop that folds their messages.
//!
//! ```text
//! view(&Model) -> View<Msg>          pure, rebuilt every frame
//!        │
//!        ▼  reconcile (keyed LCS)
//! Vec<Instance<Msg>>                 retained: Node + Controller + children
//!        │
//!        ├─ render::restyle_tree     cascade → ComputedStyle + AnimationState
//!        ├─ render::layout_tree      taffy, through Controller::measure
//!        └─ render::paint_tree       skia,  through Controller::paint
//!        │
//!        ▼  InputEvent → hit chain → capture/target/bubble
//! Vec<Msg> → update(&mut Model, Msg) -> Cmd<Msg> → queued, never nested
//! ```
//!
//! Identity is the point of the reconciler: a child matched by key and kind
//! keeps its `Node`, and with it its running transitions, its focus, its
//! shaping caches and any popup attached to it.
//!
//! Two loops share every stage: [`App::run`](app::App::run) over a live
//! [`Window`](crate::window::Window), and
//! [`App::run_offscreen`](app::App::run_offscreen) over a raster surface on a
//! [`ManualClock`](crate::anim::ManualClock), which is how the tests drive it.
```

Add a section to `ui/README.md`, after the CSS-engine section:

```markdown
## Reactive framework (`view/`)

`view(&Model) -> View<Msg>` describes the UI; the reconciler diffs it into
the retained node tree from `css::node`, keeping identity — animations,
focus, shaping caches — for every child whose key and kind survive.
Behaviour lives in a `Controller` per widget kind, not in the model:

```rust
fn view(model: &Model) -> View<Msg> {
    widget::<Msg>(Kind::Box)
        .child(widget(Kind::Button).prop(PropName::Label, "+").on_click(Msg::Inc))
        .child(widget(Kind::Label).prop(PropName::Label, model.n.to_string()))
}

fn update(model: &mut Model, msg: Msg) -> Cmd<Msg> {
    match msg { Msg::Inc => model.n += 1 }
    Cmd::None
}

App::new(Model { n: 0 }, update, view).run(window)?;
```

Messages are queued and folded one at a time — an `update` that produces a
`Cmd` producing a `Msg` enqueues it; the fold never re-enters.
`App::run_offscreen` runs the identical loop against a raster surface with no
compositor, driven by a `Vec<ScriptStep<Msg>>` on a `ManualClock`; that is
what `ui/tests/counter_app.rs` and every controller unit test use.

Widget builders (`button("Ok")`, `label("Hi")`, …) arrive with P5 and P6;
`view::builders`' module docs carry the naming rule they follow.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --test counter_app`
Expected: PASS — 4 tests.
Run: `cargo test -p icedtea-ui`
Expected: PASS, including the M1/M2 gates untouched:
`themed_button_offscreen` 4, `adwaita_coverage` 9,
`gtk4_property_reference` 4, `transition_screencopy` 1,
`layer_shell_screencopy` 3.
Run: `cargo test --workspace` — green.
Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` — no warnings.
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings` — no warnings.
Run: `cargo fmt --all --check` — no output.
Run: `cargo doc -p icedtea-ui --no-deps` with `RUSTDOCFLAGS="-D warnings"` — no warnings.

**Mutation check (load-bearing:
`clicking_minus_and_plus_returns_to_the_starting_pixels`).** Delete the
`instance.props = view.props;` line in `reconcile`'s reuse arm; the test must
FAIL, because every later frame then diffs against the instance's first-frame
`Props` and the label stops tracking the model. Restore. Record in the commit
body.

**Second mutation check (load-bearing:
`clicking_plus_repaints_the_label_with_the_new_model`).** In `drain`, skip the
`rebuild` call when `folded` is true; the test must FAIL with identical glyph
coverage in both frames. Restore.

- [ ] **Step 5: Commit**

```bash
git add ui/tests/counter_app.rs ui/README.md ui/src/view/mod.rs
git commit -m "test(ui): the offscreen counter app, plus view/ docs

Contract §4.8's counter app drives the whole stack — builders,
reconciler, controllers, dispatch, restyle, layout, paint — through
run_offscreen and asserts on pixels: the two accent-blue buttons and the
white label ground, a glyph-coverage change when the model changes, and
byte-identical frames for a model that returns to where it started.

Mutation checks: dropping reconcile's props write-back leaves the diff
comparing against a stale frame and fails
clicking_minus_and_plus_returns_to_the_starting_pixels; skipping drain's
rebuild fails clicking_plus_repaints_the_label_with_the_new_model.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review

### 1. Spec coverage

Contract §4 and spec §4, requirement by requirement, against the tasks above.

| Requirement (source) | Task |
|---|---|
| §4.1 `View<Msg>` struct and its five public fields | 4 |
| §4.1 `new`/`key`/`child`/`children`/`prop`/`on` | 4 |
| §4.1 the fifteen `GtkWidget`-universal setters (+ `opacity`) | 4 |
| §4.1 `Key` and its three `From` impls | 3 |
| §4.2 `Kind`, 64 variants in catalogue order | 2 |
| §4.2 `css_name` / `base_classes` / `is_focusable_by_default` / `all` | 2 |
| §4.3 `PropName`'s 97 typed names | 1 |
| §4.3 `Prop`, with `Draw` compared by `Rc::ptr_eq` | 1 |
| §4.3 `Props::{set,get,str,bool,int,float,diff,iter}` | 1 |
| §4.3 `ListItem` (referenced, never defined — D7) | 1 |
| §4.4 `EventKind`'s 18 names | 3 |
| §4.4 `Handler`'s six payload shapes | 3 |
| §4.4 `Handlers::{set, fire_unit, fire_text, fire_bool, fire_index, fire_float}` | 3 |
| §4.4 the normative builder-naming rule | 5 (module docs) |
| §4.4 `on_<eventkind>` setters, one per `EventKind` | 5 |
| §4.5 `Instance<Msg>` and its seven fields | 12 |
| §4.5 `Op`'s six variants | 8 |
| §4.5 `BuildCx`'s five fields | 8 |
| §4.5 keyed match on key+kind; rebuild on kind change | 12 |
| §4.5 unkeyed positional match within kind | 12 |
| §4.5 `SetProp` → `Controller::set_prop` → node mutation | 12 |
| §4.5 removal drops the controller; animations finish per `fill-mode` | 12 (drop), 7 (`Animations::retain_live`) |
| §4.5 `Move` via `Node::insert_child`, reparenting in place | 12 (attachment), 13 (minimality) |
| §4.5 identity survives: animation, focus, caches, tree ids, popups | 12, 14 |
| §4.5 op minimality (no redundant `SetProp`/`Move`) | 13, 14 |
| §4.6 `Controller` trait: 8 methods, 4 defaulted | 8 |
| §4.6 `Event`'s twelve variants | 8 |
| §4.6 `EventCx`'s eleven fields (+ `phase`, D5) | 8 |
| §4.6 dispatch capture → target → bubble; `handled` stops the phase | 15 |
| §4.6 unhandled keys fall through to focus navigation | 15 (`Handler::Key` returning `None` bubbles), 9 |
| §4.7 `Cmd`'s sixteen variants | 8 |
| §4.7 `App::new(model, update, view)` | 16 |
| §4.7 `App::run` (D1: takes a `Window`) | 17 |
| §4.7 `App::run_offscreen(size, clock, script)` | 16 |
| §4.7 `ScriptStep`'s four variants (D17) | 16 |
| §4.7 `Frames::{len, is_empty, pixel}` (+ `size`) | 16 |
| §4.7 `AppError::{Surface, Layout}` | 16 |
| §4.7 messages queued, never nested; one `view` per drained batch | 16 (`drain`) |
| §4.7 handlers replaced wholesale, never diffed | 12 (`SetHandlers`) |
| §4.8 reconciler property tests | 14 |
| §4.8 controller units on a `ManualClock` | 9, 16 |
| §4.8 offscreen counter app with pixel assertions | 18 |
| §4.8 mutation checks on the LCS and on `Props::diff` | 13, 14 |
| §9 P4 owns "the recursive paint walker that reads `paint_node`'s `node`" | 11 (+ D12) |
| §9 P4 owns `StyleMap` | 7 |
| §9 gate: "the counter app renders identical pixels for identical models" | 16, 18 |
| Spec §4 "controllers own press state, cursor, scroll offset, …" | 8 (trait), 9 (`GenericC`) |
| Spec §4 "`AnimationState` ticks feed frames" | 7 (`Animations`), 17 (`frame_deadline`) |
| Cross-cutting: untrusted input never panics | 6 (`settings.ini` sweep), 7 (depth clamp), 10/11 (missing map/allocation) |
| Cross-cutting: `Duration::ZERO` means now, never spin | 17 (`frame_deadline` test) |

Gaps found and closed while writing this table:

* `IconTheme` is in `BuildCx`/`EventCx` (§4.5/§4.6) but owned by P7, which
  runs later — closed by Task 6 under deviation D9.
* `Clipboard` is in `EventCx` but `run_offscreen` has no compositor — closed
  by Task 16 under deviation D10.
* `layout::Align` is in `View::halign` and `Prop::Align` but owned by P6 —
  closed by Task 1 under deviation D8.
* `Handler::Key` had no `fire_*` — closed by Task 3's `fire_key`.
* §4.7 gives `App` no theme or fonts — closed by Task 16 under D11.
* §5's `(i32,u32,u32)` and `(usize,usize)` handler payloads have no `Handler`
  variant — closed by D13's string/index encodings.

Nothing in §4 is left without a task.

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `implement later`,
`fill in details`, `add appropriate error handling`, `handle edge cases`,
`write tests for the above`, `similar to Task`, `and so on`, `etc.` inside
code blocks, and for any code step consisting of prose alone.

* Two self-correcting notes written during drafting were removed: Task 8's
  "do it the other way" paragraph about `Cmd`'s ordering (fixed by moving the
  full `Cmd` into Task 8), and Task 16's `step_was_capture` stub line (fixed
  by the `capture` flag inline in the loop). Task 18's first mutation check
  was rewritten from a false start into the single `instance.props` deletion.
* Every code step carries compilable Rust, not a description of it. The only
  prose-only step in the plan is Task 18's Step 3, which is documentation
  text — and the documentation itself is given verbatim.
* `Controller::tick`/`next_deadline`/`measure`/`paint` have empty default
  bodies; those are the contract's own defaults (§4.6), not placeholders.
* `IconTheme::clear_caches` is an empty body: P4 holds no caches yet, and
  the doc comment says so and names P7 as the filler. It is called by `App`
  today, so it is live surface, not a stub.
* No type, function or method is referenced without being defined in this
  plan or in a named, existing file: every P3 item used (`Window`, `Surface`,
  `InputEvent`, `SurfaceError`, `SurfaceStates`, `KeyEvent`, `Scroll`,
  `ImplicitGrab`, `hit_chain`, `FocusRing`, `FocusCause`, `Clipboard`,
  `PopupAnchorPoint`, `Positioner`, `PopupKey`, `BTN_LEFT`) is cited to
  contract §3 with its exact signature; every M2 item (`Node`,
  `PseudoStates`, `CompiledSheet`, `ComputedStyle`, `ResolveEnv`, `MatchCx`,
  `AnimationState`, `Overrides`, `Clock`, `ManualClock`, `LayoutTree`,
  `Container`, `BoxDirection`, `Allocation`, `Rect`, `Measure`,
  `FixedMeasure`, `LayoutError`, `FontDatabase`, `TextStyle`, `ShapedText`,
  `PaintCx`, `ImageCache`, `paint_node_with_children`, `paint_text`,
  `IconRef`, `BUNDLED_ADWAITA_LIGHT`) is cited to its current file and line
  range from `.superpowers/m3-plan-notes/current-ui-crate.md`.

### 3. Type consistency

Cross-checked every name used in a later task against its definition.

* `Props::diff(&self, prev: &Props) -> Vec<PropName>` — defined Task 1, used
  Task 12 (`view.props.diff(&instance.props)`, i.e. new-against-old, matching
  the definition's argument order) and mutated in Task 14. Consistent.
* `Handlers::fire_*` — defined Task 3, called in Task 9 (`fire_unit`,
  `fire_key`) with the same `EventKind` arguments the builders of Task 5 bind.
  Consistent.
* `Kind::css_name`/`base_classes` — defined Task 2, used in Task 9
  (`build_controller`'s `set_classes`), Task 12 (`Node::new(view.kind.css_name())`)
  and Task 16 (`Runtime.root`). Consistent.
* `node_addr` / `NodeAddr` / `StyleMap` / `Animations` — defined Task 7, used
  in Tasks 8 (`EventCx.styles`), 10 (`containers` key), 11, 12
  (`containers_of`), 15, 16, 17. One spelling throughout.
* `BuildCx` field names (`sheet`, `fonts`, `icons`, `clock`, `env`) — defined
  Task 8, constructed identically in Tasks 9, 12, 14, 15, 16. Consistent.
* `EventCx` field names — defined Task 8, constructed in Tasks 9, 15
  (`deliver`, `tick_all`). The `phase` and `handled` fields appear in every
  construction. Consistent.
* `Controller::set_prop(&mut self, node, name, value: &Prop, cx)` — defined
  Task 8, implemented Task 9, called Task 12 with
  `(&instance.node, name, &value, cx)`. Consistent.
* `Instance` field names — defined Task 12, destructured in Task 15
  (`controller`, `handlers`, `node`, `children`) and Task 16
  (`ControllerMeasure`/`ControllerPainter` via `find_mut`). Consistent.
* `reconcile(parent, prev, next, cx) -> Vec<Op>` — defined Task 12, unchanged
  by Task 13, called in Tasks 14, 15 (test), 16 (`rebuild`). Consistent.
* `layout_tree(root, styles, containers, tree, env, available, measure)` —
  defined Task 10, called in Task 16 (`render_once`) and Task 17 with the
  same seven arguments in the same order. Consistent.
* `paint_tree(canvas, root, styles, tree, anims, now, origin, cx, painter)` —
  defined Task 11, called in Tasks 16 and 17 with the same nine. Consistent.
* `App::{new, with_sheet, with_fonts, with_icons, model, run, run_offscreen}`
  — defined Tasks 16–17, used in Task 18. Consistent.
* `Frames::{len, is_empty, pixel, size}` — defined Task 16, used Task 18.
  `pixel` returns `Option<(u8, u8, u8, u8)>` in both.
* `Cmd::{flatten, is_none}` — defined Task 8, used Task 16 (`drain`) and
  Task 17. Consistent.
* `Clipboard::offscreen` — first drafted in Task 16, but Tasks 9 and 15 both
  construct one in their tests, and both run earlier. **Fixed inline:** the
  `ui/src/window/selection.rs` edit and its round-trip test were moved into
  **Task 8**, whose `EventCx` is the first thing that needs a `Clipboard` at
  all; Task 8's `git add` and commit message carry D10, and Task 16 no
  longer touches `selection.rs`. Test counts were adjusted (Task 8: 6 → 7,
  Task 16: 9 → 8, Task 17: 10 → 9).
* `ORIENTATION_HORIZONTAL`/`ORIENTATION_VERTICAL` — defined Task 10, used in
  its own tests only; P5/P6 consume them for `Prop::Enum` orientation values.
  Documented in `container_for`'s doc comment.
* `MAX_TREE_DEPTH` — defined Task 7, honoured in Tasks 7, 10 and 11.

One inconsistency found (the `Clipboard::offscreen` ordering) and fixed
inline, above.
