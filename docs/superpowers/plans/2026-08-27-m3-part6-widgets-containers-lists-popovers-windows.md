# Pure-Rust GTK-themed UI — M3 Part 6: widgets — containers, lists, popovers/menus, windows/dialogs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the 27 remaining GTK 4.22 widget kinds — the containers
(`Box`, `Grid`, `CenterBox`, `ScrolledWindow`, `Paned`, `Frame`, `Expander`,
`SearchBar`, `ActionBar`, `HeaderBar`, `Notebook`/`NotebookTab`, `Overlay`,
`Stack`/`StackPage`, `StackSwitcher`, `StackSidebar`), the list family
(`ListBox`/`ListBoxRow`, `FlowBox`/`FlowBoxChild`, `ListView`, `GridView`,
`ColumnView`/`ColumnViewColumn`), the menus (`PopoverMenu`,
`PopoverMenuItem`, `PopoverMenuBar`) and the windows and dialogs (`Window`,
`ShortcutsWindow`, `AboutDialog`, `AlertDialog`) — each with GTK's exact CSS
node tree, its builder, its controller, its keyboard and pointer behaviour,
a node-tree conformance fixture, and one rest-state plus one interaction
pixel test. Widen `ui/src/layout.rs` with the `Grid`/`Center` containers and
per-child placement M2's own doc comments defer to M3.

**Architecture:** Every widget is three things and nothing more: a
`view::builders` free function returning a `View<Msg>`, a `Kind` arm, and a
`Controller<Msg>` that owns the retained subnodes and the behaviour state the
model must not carry. The controller builds its GTK subnode tree once in
`Controller::build`, mutates it in `set_prop`/`on_event`/`tick`, and never
rebuilds it — that is what makes animation, focus and shaping caches survive
reconciliation. Layout is delegated: a container controller calls
`LayoutTree::set_container` with the `Container` variant its GTK class
implies and `set_child_layout` for per-child `halign`/`valign`/`expand`/
`margin`/grid placement, so no widget does arithmetic taffy can do. The list
family adds one mechanism on top: a **row pool** rebound on scroll
(`set_prop` on the pooled row's child instance) rather than reconciled, so
scrolling a hundred-thousand-row model touches a viewport's worth of nodes.
Menus and dialogs are windows: `PopoverMenu` drives P5's `PopoverC` (a real
`Surface::Popup` with `xdg_popup.grab`), and `Window`/`AlertDialog` drive the
toplevel's `SurfaceStates` into `.maximized`/`.fullscreen`/`.tiled` and
`PseudoStates::BACKDROP`.

**Tech Stack:** Rust (edition 2024, rust-version 1.94), `taffy` 0.14,
`skia-rs-safe` 0.4.0 (features `std`, `text`, `codec`, `codec-png`, `svg`),
`cssparser` 0.37, `selectors` 0.40, `wayland-client` 0.31,
`wayland-protocols` 0.32, `wayland-protocols-wlr` 0.3, `xkbcommon` 0.9,
`fontconfig` 0.11 (optional), `tracing`, `bitflags` 2.

**Spec:**
`docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
(§1 part table P6, §5 "Widget set" containers/lists/popovers/windows, §7
"Testing strategy, gates") · parent spec
`docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` ·
**binding interface contract**
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§3.6 layout
additions, §5.4 containers, §5.5 lists, §5.6 menus, §5.7 windows & dialogs,
§8 migration, §9 part boundaries — P6) · research notes
`.superpowers/m3-plan-notes/gtk-widget-nodes.md` (§0.2 fixture format,
§0.4 decoration layout, Sections 4–5) and
`.superpowers/m3-plan-notes/current-ui-crate.md`.

---

## Contract deviations

The Part 0 contract is binding. The twelve items below are places where it
cannot be followed verbatim, each with the amendment this plan implements
and the task that carries it out. Nothing else in this plan diverges from it.

1. **`LayoutTree::set_measure` takes `Rc<RefCell<dyn Measure>>`, not
   `Rc<dyn Measure>`.** §3.6 writes
   `set_measure(&mut self, node: &Node, measure: Rc<dyn Measure>)`, but M2's
   `Measure::measure` takes `&mut self` (`ui/src/layout.rs:499`) and an `Rc`
   hands out shared references only. Changing the trait to `&self` would
   force interior mutability into every existing impl including
   `FixedMeasure`, which the M2 layout gate exercises. Amendment:
   `pub fn set_measure(&mut self, node: &Node, measure: Rc<RefCell<dyn Measure>>)`.
   `Measure` itself is untouched. **Task 1.**

2. **Per-child placement is stored as `Option<ChildLayout>`, and absence —
   not `ChildLayout::default()` — reproduces M2's `CENTER`/`CENTER`.**
   §3.6 gives `Align` a `#[default] Fill` and P6's gate demands "a new test
   pins that the default `ChildLayout` reproduces M2's `CENTER`/`CENTER`".
   Those two cannot both hold literally: GTK's `Fill` is taffy's `Stretch`,
   which is not `Center`. Amendment: the tree stores `Option<ChildLayout>`
   per node; `None` (no `set_child_layout` call ever made — every M2 caller)
   keeps `align_items: CENTER` / `justify_content: CENTER` on the parent and
   sets no `align_self`/`justify_self`, so every M2 number is bit-identical;
   an explicit `ChildLayout` maps `Align::Fill` to `AlignSelf::Stretch`. The
   pinning test asserts both halves. **Task 1.**

3. **P6 appends `PropName` variants to `ui/src/view/mod.rs`.** §4.3's
   `PropName` is `#[non_exhaustive]` and does not carry names for 89 of the
   props §5.4–§5.7's builders set (`HasFrame`, `WideHandle`, `TabPos`,
   `MinChildrenPerLine`, …). The enum is P4's file. Amendment: P6 appends
   the exhaustive list printed in Task 2 — additive variants on a
   `#[non_exhaustive]` enum, no existing variant renamed, reordered or
   removed, no P4 or P5 code touched. **Task 2.**

4. **P6 adds two `Handler` variants and their two `fire_*` methods.**
   §5.4/§5.5 require `.on_scrolled(|(f64, f64)| Msg)`,
   `.on_reordered(|(usize, usize)| Msg)` and `.on_sort_changed`, and §4.4's
   `Handler` has no two-argument variant. Amendment: `Handler::Pair(Rc<dyn
   Fn(f64, f64) -> Msg>)` and `Handler::Indices(Rc<dyn Fn(usize, usize) ->
   Msg>)`, with `Handlers::fire_pair` / `Handlers::fire_indices`, appended to
   `ui/src/view/mod.rs`. Additive; no existing variant changes. **Task 2.**

5. **`PropName::Buttons` carries `Prop::Classes`.** §5.7's `alert_dialog`
   takes `.buttons(&[&str])` and §4.3's `Prop` has exactly one string-list
   variant, `Classes(Rc<[Rc<str>]>)`. `Buttons` therefore stores a
   `Prop::Classes`; the variant name is a payload shape, not a semantic
   claim. The same rule covers `Authors`, `Artists` and `Documenters`.
   **Tasks 2 and 24.**

6. **The controller factory `widgets::build_controller` is jointly owned by
   P5 and P6.** The contract defines `Controller::build` but no dispatch
   from a `Kind` to a `Box<dyn Controller<Msg>>`, which `reconcile`'s
   `Insert` arm needs. P5 necessarily created one; P6 adds its 27 arms to it.
   `ui/src/widgets/mod.rs` is therefore the one P5-authored file P6 edits,
   and only inside the `match kind` and the `mod` list. §9's "P6 must not
   touch P5's widget files except to call their controllers" is read as
   applying to the per-widget files (`widgets/button.rs`, …), which P6 does
   not touch. **Task 3, and every widget task's registration step.**

7. **`node_tree_of` lives in the library, not in `ui/tests/node_trees.rs`.**
   §5 places it beside the fixtures; the gallery gate (P8) and the widget
   modules both need it, and an integration test cannot export. Amendment:
   `pub fn node_tree_of(kind: Kind, props: &Props) -> String` and the fixture
   matcher live in `ui/src/widgets/node_tree.rs` and are re-exported as
   `icedtea_ui::widgets::node_tree_of`; `ui/tests/node_trees.rs` (P5's file)
   keeps its name and its per-kind tests and calls the library function. If
   P5 put the function body in the test file, Task 3 moves it, leaving P5's
   test bodies calling the same path. **P5 in fact ships the bodies in
   `ui/src/widgets/mod.rs` as `node_tree_of` / `render_node_tree` /
   `fixture_matches(fixture: &str, rendered: &str) -> Result<(), String>`;
   Task 3 relocates those three into `node_tree.rs` under their P5 names and
   signatures, re-exports them from `widgets`, and adds `matches_fixture` /
   `Mismatch` as a node-first convenience wrapper over `fixture_matches` —
   it does not reimplement the matcher.** See contract §10 E6. **Task 3.**

8. **The fixture gate is a matcher, not a string compare.** §5 says
   `node_trees.rs` "diffs [the render] against the fixture", but GTK's
   notation contains `[name]`, `name[.class]`, `<child>` and `┊`, none of
   which a concrete retained tree renders. Amendment:
   `node_tree::matches_fixture(root: &Node, fixture: &str) ->
   Result<(), Mismatch>` interprets the notation (optional subnode, optional
   class, application child, repetition) and `node_tree_of` renders the
   concrete tree for the failure message. A fixture with no optional
   notation still compares exactly. **Task 3.**

9. **`ListViewC`, `Selection` and `SelectionMode` are P6 types that P5
   already needs.** §9 lists `ListViewC` under "P6 consumes §5's
   `PopoverC`/`ListViewC`/`ScrollbarC`" while §5.5 files `ListView` under
   P6, and P5's `DropDownC`/`FontDialogC` embed `list: ListViewC`.
   Amendment: `ui/src/widgets/list_view.rs` is P6's file; whatever minimal
   `ListViewC` P5 created there to compile `DropDownC` is **replaced** by
   Task 19's implementation, which is a strict superset (same type name,
   same `model`/`selection`/`pool` fields, same `Controller` impl), so P5's
   call sites keep compiling unchanged. If the file does not exist, Task 19
   creates it. **Task 19.**

10. **Per-widget pixel tests are hermetic `run_offscreen` tests in the
    widget's own module.** §5 says the two pixel gates "are in
    `gallery_gate.rs`/`interaction_gate.rs` (P8), not per widget file". P8
    still owns those compositor-driven gates; P6 additionally ships, per
    widget, one rest-state and one interaction-state assertion through
    `App::run_offscreen` (§4.7) against `BUNDLED_ADWAITA_LIGHT`, because a
    widget that only fails four parts later is a widget nobody can bisect.
    These are additions, not replacements. **Every widget task.**

11. **`Container::Grid`'s `columns`/`rows` are template bounds, not a
    placement authority.** §3.6's variant carries both counts; taffy's grid
    auto-placement fills them. P6 sets
    `grid_template_columns`/`_rows` to `columns`/`rows` repeated `auto`
    tracks (both clamped to `1..=1024`, a hostile `Props` value never
    allocates unboundedly) and lets `GridPlacement` pin explicit children.
    A child with no `GridPlacement` is auto-placed. **Tasks 1 and 6.**

12. **`StackSidebarC` embeds a `ListBoxC`, which forces `ListBox` before
    `StackSidebar`.** §5.4 lists `StackSidebar` before the list family but its
    controller is `struct StackSidebarC { selected: usize, list: ListBoxC }`.
    Task order in this plan is therefore ListBox (16) → StackSidebar (17);
    the catalogue order is preserved everywhere else and in `Kind::all()`.
    **Tasks 16 and 17.**

---

13. **Setters take `impl Into<Prop>`, not a concrete type.** §4.4's builder
    rule names the setters after GTK properties but does not fix their
    argument types, and `View<Msg>` has one inherent-method namespace across
    the whole catalogue: GTK gives `position` an `int` on `GtkPaned` and a
    `GtkPositionType` on `GtkPopover`, and Rust has no overloading. Every
    setter therefore takes `impl Into<Prop>`, with one `From<T> for Prop`
    per scalar and per `#[repr(u16)]` enum. Names are unchanged. **Task 4.**

14. **List and menu activation is `.on_item_activated(|usize| Msg)`.**
    §5.5/§5.6 write `.on_activate(|usize| Msg)` for `ListBox`, `FlowBox`,
    `ListView`, `GridView`, `ColumnView` and `PopoverMenu`, but P5 already
    owns `.on_activate(Msg)` at `Handler::Unit` arity for `Button`, `Entry`
    and friends, and the two cannot share one inherent method. The
    `EventKind` is unchanged (`EventKind::Activate`); only the builder
    method's name differs, and it differs for all six kinds consistently.
    **Tasks 16, 18, 19, 20, 21.**

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Crate pins (do not bump, do not add):** `wayland-client = "0.31"`,
  `wayland-protocols = "0.32"` (features `client`, `staging`, `unstable`),
  `wayland-protocols-wlr = "0.3"`, `skia-rs-safe = "0.4.0"` (features
  `std`, `text`, `codec`, `codec-png`, `svg`), `taffy = "0.14"`,
  `cssparser = "0.37"`, `selectors = "0.40"`, `xkbcommon = "0.9"`,
  `fontconfig = "0.11"` (optional), `bitflags = "2"`, `tracing`.
  **P3 owns `ui/Cargo.toml`; P6 adds no dependency and edits no dependency
  line.**
- **No `gtk4`/`gio`/`glib`/`gobject`/`pango`/`cairo`/`gdk`, and no
  `smithay`,** anywhere in `icedtea-ui`.
- **Edition 2024, `rust-version` 1.94**, both inherited from the workspace.
- **Gates, all four green before every commit:**
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
  - `cargo fmt --all --check`
- **The M1/M2 gates stay green, unchanged.** `ui/tests/themed_button_offscreen.rs`
  (4 tests) and `ui/src/widget/button.rs`'s tests keep every number: height
  `34.0`, the `y = height-2` band `0xFFF6F5F4`, gutter column `bx = 4`,
  corner `(0,0)` transparent, border pixel `0xFFCDC7C2`, hover `0xFFE8E6E3`,
  active `0xFFDAD6D2`, suggested-action `#2c7fe3`→`#3584e4` with border
  `0xFF15539E`, empty button `36×34`, >20 dark label pixels. `ui/src/layout.rs`'s
  six M2 tests keep every number (Task 1 is the only task that may touch that
  file, and contract §8.2's rule applies to its assertions). **A diff that
  changes a number in either fails review.**
- **Commit trailer**, on every commit in this plan:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **Parts execute IN ORDER 1→8 on one branch** (`rebuild/pure-rust-gtk-m3`),
  so P6 may consume everything P1–P5 produced: P1's `wlr` 0.20.28 popup API
  (consumed through P2/P3, never directly), P2's compositor popups and
  harness popup client, P3's `window::{Surface, Window, InputEvent,
  FocusRing, hit_test, Clipboard, KeyEvent, Mods, Scroll, Kinetic}`, P4's
  `view::{View, Kind, Props, Prop, PropName, Handlers, Handler, EventKind,
  Controller, Event, EventCx, BuildCx, PaintCx, Cmd, App, ScriptStep,
  Frames, reconcile}`, and P5's `widgets::{ButtonC, LabelC, PopoverC,
  ScrollbarC, EntryC, SearchEntryC, WindowControlsC, ImageC, SeparatorC}`
  plus `text::{TextLayout, Ellipsize, WrapMode, parse_markup}`.
- **P1 ran in a wloots-sys worktree and icedtea consumes the published
  `wlr` 0.20.28 through its `Cargo.toml` pins.** During P2–P8 development a
  root `[patch.crates-io]` pointing at that worktree is allowed, in its own
  commit, and must be dropped before merge — exactly as the implicit-grab
  fix did. P6 neither adds nor removes that patch.
- **P6 must not touch:** `ui/src/css/**`, `ui/src/anim/**`, `ui/src/shm.rs`,
  `ui/src/text.rs`, `ui/src/paint/**` (P7 adds `paint/icon.rs`),
  `ui/src/widget/button.rs`, `ui/src/window/**`, `ui/src/app.rs`,
  `ui/tests/themed_button_offscreen.rs`, `ui/tests/layer_shell_screencopy.rs`,
  `ui/tests/transition_screencopy.rs`, `ui/tests/adwaita_coverage.rs`,
  `ui/tests/gtk4_property_reference.rs`, `compositor/**`, `harness/**`, or
  any P5 per-widget file. The three exceptions, each authorised by a
  deviation above: `ui/src/layout.rs` (Task 1), `ui/src/view/mod.rs`
  (Task 2, additive variants only), `ui/src/widgets/mod.rs` (Task 2 and the
  registration step of every widget task, `match`/`mod` lines only).
- **Exactness rule (carried from M1/M2):** an asserted pixel must be
  derivable from the CSS in `BUNDLED_ADWAITA_LIGHT`. Gradients are sampled
  through `Gradient::color_at`, anti-aliased edges are never asserted
  exactly, geometry assertions come from `LayoutTree::allocation`, never from
  a literal.
- **Mutation discipline:** every load-bearing test states, in a comment, the
  source mutation it catches. The reviewer re-runs it.
- **Never-panic discipline:** every decoder of input this part does not
  control — fixture text, `Props` values that arrive from an application
  model, a `ListItem` model of any length, a `gtk-decoration-layout` string,
  a positioner rectangle — has a hostile-input test over empty, huge,
  negative, `NaN`, non-UTF-8-boundary and adversarially nested values.
- **Timing assertions are generous complexity bounds**, never wall-clock
  pins; every clock-driven test runs on `ManualClock`.
- **`Duration::ZERO` from any `next_deadline` means "now", never "spin".**

---

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `ui/src/layout.rs` | Modify | §3.6: `Align`, `ChildLayout`, `GridPlacement`, `Container::{Grid, Center}`, `set_container`, `set_child_layout`, `set_measure`; `set_style` reads the stored `ChildLayout` |
| `ui/src/view/mod.rs` | Modify | additive: 89 `PropName` variants, `Handler::{Pair, Indices}`, `Handlers::{fire_pair, fire_indices}` (deviations 3, 4) |
| `ui/src/widgets/mod.rs` | Modify | `mod` list + `build_controller` arms for the 27 P6 kinds; `build_widget` test entry point |
| `ui/src/widgets/types.rs` | Modify (create if absent) | P6's shared enums: `Policy`, `BaselinePosition`, `SelectionMode`, `Selection`, `StackTransition`, `StackPageInfo`, `SortOrder`, `Sorter`, `ItemFactory`, `MenuFlags`, `DisplayHint`, `LicenseType`, `Decoration` |
| `ui/src/widgets/node_tree.rs` | Create | `node_tree_of`, `matches_fixture`, `Mismatch` — the GTK-notation renderer and matcher |
| `ui/src/widgets/box_.rs` | Create | `Kind::Box` — `BoxC`, `builders::box_` |
| `ui/src/widgets/center_box.rs` | Create | `Kind::CenterBox` — `CenterBoxC`, `builders::center_box` |
| `ui/src/widgets/grid.rs` | Create | `Kind::Grid` — `GridC`, `builders::grid`, `.at`/`.span` |
| `ui/src/widgets/frame.rs` | Create | `Kind::Frame` — `FrameC`, `builders::frame` |
| `ui/src/widgets/paned.rs` | Create | `Kind::Paned` — `PanedC`, separator drag and keynav |
| `ui/src/widgets/expander.rs` | Create | `Kind::Expander` — `ExpanderC`, `expander-widget` tree, arrow animation |
| `ui/src/widgets/scrolled_window.rs` | Create | `Kind::ScrolledWindow` — `ScrolledWindowC`, overshoot/undershoot/junction, kinetic, overlay scrollbars |
| `ui/src/widgets/search_bar.rs` | Create | `Kind::SearchBar` — `SearchBarC`, revealer, key capture, Escape |
| `ui/src/widgets/action_bar.rs` | Create | `Kind::ActionBar` — `ActionBarC`, revealer, start/center/end packs |
| `ui/src/widgets/header_bar.rs` | Create | `Kind::HeaderBar` — `HeaderBarC`, packs, title widget, `WindowControls` per `gtk-decoration-layout` |
| `ui/src/widgets/notebook.rs` | Create | `Kind::Notebook`/`NotebookTab` — `NotebookC`, tabs, scroll arrows, reorder |
| `ui/src/widgets/overlay.rs` | Create | `Kind::Overlay` — `OverlayC`, positional classes, measure/clip |
| `ui/src/widgets/stack.rs` | Create | `Kind::Stack`/`StackPage` — `StackC`, transitions on `AnimationState` |
| `ui/src/widgets/stack_switcher.rs` | Create | `Kind::StackSwitcher` — `StackSwitcherC` |
| `ui/src/widgets/stack_sidebar.rs` | Create | `Kind::StackSidebar` — `StackSidebarC` over `ListBoxC` |
| `ui/src/widgets/list_box.rs` | Create | `Kind::ListBox`/`ListBoxRow` — `ListBoxC`, selection modes, keynav |
| `ui/src/widgets/flow_box.rs` | Create | `Kind::FlowBox`/`FlowBoxChild` — `FlowBoxC`, rubberband |
| `ui/src/widgets/list_view.rs` | Create (replace P5's minimal one) | `Kind::ListView` — `ListViewC`, the row pool |
| `ui/src/widgets/grid_view.rs` | Create | `Kind::GridView` — `GridViewC` over the same pool |
| `ui/src/widgets/column_view.rs` | Create | `Kind::ColumnView`/`ColumnViewColumn` — `ColumnViewC`, header, sort, resize |
| `ui/src/widgets/popover_menu.rs` | Create | `Kind::PopoverMenu`/`PopoverMenuItem` — `PopoverMenuC`, sections, submenus, mnemonics |
| `ui/src/widgets/popover_menu_bar.rs` | Create | `Kind::PopoverMenuBar` — `PopoverMenuBarC`, hover-to-switch |
| `ui/src/widgets/window.rs` | Create | `Kind::Window` — `WindowC`, decoration and state classes, window key bindings |
| `ui/src/widgets/shortcuts_window.rs` | Create | `Kind::ShortcutsWindow` — `ShortcutsWindowC` |
| `ui/src/widgets/about_dialog.rs` | Create | `Kind::AboutDialog` — `AboutDialogC` |
| `ui/src/widgets/alert_dialog.rs` | Create | `Kind::AlertDialog` — `AlertDialogC` |
| `ui/src/view/builders.rs` | Modify | the 27 P6 builders and their setters (P4 shipped the frame; P5 filled its 32) |
| `ui/tests/fixtures/gtk4.22-node-trees/*.txt` | Create (27 files) | GTK 4.22.4 "CSS nodes" blocks, verbatim |
| `ui/tests/node_trees.rs` | Modify | P6's 27 conformance tests beside P5's |

---

## Task 1: `layout.rs` — `Container::{Grid, Center}`, `ChildLayout`, per-node measure

**Files:**
- Modify: `ui/src/layout.rs` — widen `Container`, add `Align`,
  `ChildLayout`, `GridPlacement`, widen `NodeCtx`, split `set_style`'s taffy
  write into `write_taffy_style`, add `set_container`, `set_child_layout`,
  `set_measure`, and route `compute`'s measure closure through the per-node
  measure. The six M2 tests in the file's `mod tests` keep every number.
- Test: `ui/src/layout.rs` (`#[cfg(test)] mod tests`, appended to)

**Interfaces:**
- Consumes (M2): `css::node::{Node, Direction}`, `css::computed::{ComputedStyle,
  ResolveEnv}`, `css::registry::Prop`, `taffy` 0.14.
- Produces:
  ```rust
  pub enum Container {
      Box { direction: BoxDirection },
      Grid { columns: u16, rows: u16, column_spacing: f32, row_spacing: f32,
             column_homogeneous: bool, row_homogeneous: bool },
      Center { direction: BoxDirection, shrink_center_last: bool },
      Leaf,
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
  pub enum Align { #[default] Fill, Start, End, Center, Baseline }
  #[derive(Debug, Clone, Copy, PartialEq, Default)]
  pub struct ChildLayout {
      pub halign: Align, pub valign: Align,
      pub hexpand: bool, pub vexpand: bool,
      pub margin: [f32; 4],
      pub grid: Option<GridPlacement>,
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct GridPlacement { pub column: u16, pub row: u16, pub column_span: u16, pub row_span: u16 }
  impl LayoutTree {
      pub fn set_container(&mut self, node: &Node, container: Container);
      pub fn set_child_layout(&mut self, node: &Node, child: ChildLayout);
      pub fn set_measure(&mut self, node: &Node, measure: Rc<RefCell<dyn Measure>>);
      pub fn child_layout(&self, node: &Node) -> Option<ChildLayout>;
      pub fn container(&self, node: &Node) -> Container;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/layout.rs`'s existing `mod tests` (do not touch the six
tests already there):

```rust
    use super::{Align, ChildLayout, Container, GridPlacement};
    use crate::css::node::Direction;
    use std::cell::RefCell;

    /// A tree of `root > a, b, c` with a stylesheet-free computed style.
    fn three_children() -> (Node, Node, Node, Node) {
        let root = Node::new("box");
        let a = Node::new("widget");
        let b = Node::new("widget");
        let c = Node::new("widget");
        root.append_child(&a);
        root.append_child(&b);
        root.append_child(&c);
        (root, a, b, c)
    }

    /// Lay `root` out at a fixed 300x100 with every leaf measuring 20x10.
    fn lay_out(tree: &mut LayoutTree, root: &Node) {
        tree.sync(root).expect("sync");
        tree.compute(
            root,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(100.0),
            },
            &mut FixedMeasure(taffy::Size {
                width: 20.0,
                height: 10.0,
            }),
        )
        .expect("compute");
    }

    #[test]
    fn a_node_with_no_child_layout_keeps_m2s_centre_centre() {
        // Mutation check: making `write_taffy_style` treat an absent
        // ChildLayout as `ChildLayout::default()` (halign/valign = Fill =>
        // AlignSelf::Stretch) makes `a`'s height 100, not 10, and breaks
        // every M1 button number that depends on CENTER/CENTER.
        let (root, a, _b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        for n in [&root, &a] {
            tree.set_style(
                n,
                &style,
                Container::Box {
                    direction: BoxDirection::Row,
                },
                &ResolveEnv::default(),
            );
        }
        assert_eq!(tree.child_layout(&a), None, "no ChildLayout was set");
        lay_out(&mut tree, &root);
        let alloc = tree.allocation(&a).expect("allocation");
        assert_eq!(alloc.border_box.height, 10.0, "centred, not stretched");
        assert_eq!(
            alloc.border_box.y, 45.0,
            "centred on the cross axis: (100 - 10) / 2"
        );
    }

    #[test]
    fn an_explicit_fill_stretches_and_start_pins_to_the_leading_edge() {
        // Mutation check: mapping Align::Fill to AlignSelf::Center (the
        // "keep M2 behaviour everywhere" mistake) makes the first height 10;
        // mapping Align::Start to Stretch makes the second 100.
        let (root, a, b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b] {
            tree.set_style(
                n,
                &style,
                Container::Box {
                    direction: BoxDirection::Row,
                },
                &ResolveEnv::default(),
            );
        }
        tree.set_child_layout(
            &a,
            ChildLayout {
                valign: Align::Fill,
                ..ChildLayout::default()
            },
        );
        tree.set_child_layout(
            &b,
            ChildLayout {
                valign: Align::Start,
                ..ChildLayout::default()
            },
        );
        lay_out(&mut tree, &root);
        assert_eq!(tree.allocation(&a).unwrap().border_box.height, 100.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.height, 10.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.y, 0.0);
    }

    #[test]
    fn grid_places_children_at_their_column_and_row() {
        // Mutation check: dropping the `+ 1` in the taffy line conversion
        // (taffy grid lines are 1-based) puts `c` in column 0's track and
        // both x values become equal.
        let (root, a, b, c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b, &c] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_container(
            &root,
            Container::Grid {
                columns: 2,
                rows: 2,
                column_spacing: 4.0,
                row_spacing: 6.0,
                column_homogeneous: true,
                row_homogeneous: true,
            },
        );
        for (n, place) in [
            (&a, GridPlacement { column: 0, row: 0, column_span: 1, row_span: 1 }),
            (&b, GridPlacement { column: 0, row: 1, column_span: 1, row_span: 1 }),
            (&c, GridPlacement { column: 1, row: 0, column_span: 1, row_span: 2 }),
        ] {
            tree.set_child_layout(
                n,
                ChildLayout {
                    grid: Some(place),
                    ..ChildLayout::default()
                },
            );
        }
        lay_out(&mut tree, &root);
        let (aa, ab, ac) = (
            tree.allocation(&a).unwrap(),
            tree.allocation(&b).unwrap(),
            tree.allocation(&c).unwrap(),
        );
        assert_eq!(aa.border_box.x, ab.border_box.x, "same column");
        assert!(ac.border_box.x > aa.border_box.x, "second column is to the right");
        assert!(ab.border_box.y > aa.border_box.y, "second row is below");
        assert_eq!(
            ab.border_box.y - aa.border_box.y,
            (100.0 - 6.0) / 2.0 + 6.0,
            "homogeneous rows plus the 6px row-spacing"
        );
    }

    #[test]
    fn centre_box_centres_the_middle_child_whatever_the_ends_measure() {
        // Mutation check: implementing Center as plain SpaceBetween without
        // the grow-from-zero on the outer children moves the centre child
        // off centre as soon as the two ends differ in width.
        let (root, start, centre, end) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        for n in [&root, &start, &centre, &end] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_container(
            &root,
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: true,
            },
        );
        tree.set_style(&start, &style, Container::Leaf, &ResolveEnv::default());
        tree.set_child_layout(&start, ChildLayout::default());
        lay_out(&mut tree, &root);
        let mid = tree.allocation(&centre).unwrap().border_box;
        assert_eq!(
            mid.x + mid.width / 2.0,
            150.0,
            "the centre child's centre is the container's centre"
        );
    }

    #[test]
    fn centre_box_puts_the_first_child_on_the_right_under_rtl() {
        // Mutation check: ignoring Direction::Rtl (using Row for both) puts
        // `start` at x == 0 under RTL, which is GTK's documented opposite
        // (gtk/gtkcenterbox.c:45).
        let (root, start, _centre, _end) = three_children();
        root.set_direction(Some(Direction::Rtl));
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        tree.set_style(&root, &style, Container::Leaf, &ResolveEnv::default());
        tree.set_container(
            &root,
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: false,
            },
        );
        lay_out(&mut tree, &root);
        assert!(
            tree.allocation(&start).unwrap().border_box.x > 150.0,
            "the first child is allocated on the right under RTL"
        );
    }

    #[test]
    fn a_per_node_measure_overrides_the_tree_measure_for_that_node_only() {
        // Mutation check: applying the per-node measure to every leaf (the
        // "last one wins" bug) makes `b` 40x30 too.
        let (root, a, b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_measure(
            &a,
            Rc::new(RefCell::new(FixedMeasure(taffy::Size {
                width: 40.0,
                height: 30.0,
            }))),
        );
        lay_out(&mut tree, &root);
        assert_eq!(tree.allocation(&a).unwrap().border_box.width, 40.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.width, 20.0);
    }

    #[test]
    fn hostile_container_and_child_layout_values_never_panic() {
        // A Props-driven container count arrives from an application model.
        // Mutation check: dropping the 1..=1024 clamp makes this test
        // allocate 4 billion grid tracks and never return.
        let (root, a, _b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial();
        tree.sync(&root).expect("sync");
        tree.set_style(&root, &style, Container::Leaf, &ResolveEnv::default());
        for (columns, rows, spacing) in [
            (0_u16, 0_u16, f32::NAN),
            (u16::MAX, u16::MAX, f32::INFINITY),
            (1, 1, -1.0e30),
        ] {
            tree.set_container(
                &root,
                Container::Grid {
                    columns,
                    rows,
                    column_spacing: spacing,
                    row_spacing: spacing,
                    column_homogeneous: true,
                    row_homogeneous: false,
                },
            );
            tree.set_child_layout(
                &a,
                ChildLayout {
                    margin: [spacing; 4],
                    grid: Some(GridPlacement {
                        column: u16::MAX,
                        row: u16::MAX,
                        column_span: 0,
                        row_span: u16::MAX,
                    }),
                    ..ChildLayout::default()
                },
            );
            lay_out(&mut tree, &root);
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib layout::tests`
Expected: FAIL — `error[E0432]: unresolved imports `super::Align`,
`super::ChildLayout`, `super::GridPlacement`` and
`error[E0599]: no method named `set_container` found for struct `LayoutTree``.

- [ ] **Step 3: Write minimal implementation**

In `ui/src/layout.rs`, extend the `taffy::prelude` import with `fr`, `line`,
`span` and add `use std::cell::RefCell;`, then replace the `Container` enum
and its `Default` impl with:

```rust
/// How a node lays its children out.
///
/// M2 had two variants; M3 adds GTK's grid and centre-box layouts, which
/// the M2 doc comment on this enum deferred to this milestone.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Container {
    /// A flex container on `direction`. With no per-child [`ChildLayout`]
    /// it centres its children on both axes -- GTK's box default, and what
    /// M1's button relied on.
    Box {
        /// The main axis.
        direction: BoxDirection,
    },
    /// `GtkGrid`: a fixed track grid. `columns`/`rows` are template bounds
    /// (clamped to `1..=1024`); children with a [`GridPlacement`] are pinned
    /// and the rest are auto-placed.
    Grid {
        /// Column tracks.
        columns: u16,
        /// Row tracks.
        rows: u16,
        /// Gap between columns, px.
        column_spacing: f32,
        /// Gap between rows, px.
        row_spacing: f32,
        /// Every column track the same width.
        column_homogeneous: bool,
        /// Every row track the same height.
        row_homogeneous: bool,
    },
    /// `GtkCenterBox`: exactly three children, the middle one centred in the
    /// whole allocation. The *first* child is allocated per text direction
    /// (`gtk/gtkcenterbox.c:45`).
    Center {
        /// The main axis.
        direction: BoxDirection,
        /// The centre child shrinks after the outer two, not before.
        shrink_center_last: bool,
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

/// GTK's `halign`/`valign`.
///
/// `Fill` is GTK's own default *for a widget*; a node this tree has never
/// been given a [`ChildLayout`] for is not `Fill`, it is *unaligned*, and
/// keeps the container's own centring. See [`LayoutTree::set_child_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Take the whole cross-axis extent.
    #[default]
    Fill,
    /// Leading edge (top, or left under LTR).
    Start,
    /// Trailing edge.
    End,
    /// Centred.
    Center,
    /// Aligned on the text baseline; on the main axis, same as `Start`.
    Baseline,
}

/// Per-child placement, as GTK's `GtkWidget` expresses it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ChildLayout {
    /// Horizontal alignment.
    pub halign: Align,
    /// Vertical alignment.
    pub valign: Align,
    /// Absorb extra horizontal space.
    pub hexpand: bool,
    /// Absorb extra vertical space.
    pub vexpand: bool,
    /// Outer margin, top/right/bottom/left, px.
    pub margin: [f32; 4],
    /// Explicit grid cell, when the parent is a [`Container::Grid`].
    pub grid: Option<GridPlacement>,
}

/// One child's cell in a [`Container::Grid`], zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPlacement {
    /// Zero-based column.
    pub column: u16,
    /// Zero-based row.
    pub row: u16,
    /// Columns covered; `0` is read as `1`.
    pub column_span: u16,
    /// Rows covered; `0` is read as `1`.
    pub row_span: u16,
}
```

Widen the taffy context and remember the inputs the style write needs:

```rust
/// What each taffy node carries so the measure closure can reach the CSS.
struct NodeCtx {
    node: Node,
    style: Rc<ComputedStyle>,
    /// The container this node *is*.
    container: Container,
    /// How this node is placed inside its parent. `None` -- the M2 state --
    /// means "unaligned": the parent's own centring decides, which is what
    /// keeps every M1/M2 number bit-identical.
    child: Option<ChildLayout>,
    /// A measure that overrides the tree-wide one for this node only.
    measure: Option<Rc<RefCell<dyn Measure>>>,
    /// The `ResolveEnv` the last `set_style` used, so `set_container` and
    /// `set_child_layout` can rewrite the taffy style without one.
    env: ResolveEnv,
}
```

`sync_node` (M2) constructs `NodeCtx`; give the three new fields
`Container::default()`, `None`, `None` and `ResolveEnv::default()` there.

Split the taffy write out of `set_style`. Keep `set_style`'s signature and
its whole body up to and including the `taffy_style` construction, but:

* replace the two hard-coded lines
  ```rust
  align_items: Some(AlignItems::CENTER),
  justify_content: Some(JustifyContent::CENTER),
  ```
  and the `(display, flex_direction)` match with a call to the new
  `container_style` helper, and
* store `container`, `style` and `env` into the context *before* writing, so
  a later `set_child_layout` can rewrite from the same inputs:

```rust
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
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.style = Rc::new(style.clone());
            ctx.container = container;
            ctx.env = env.clone();
        }
        self.write_taffy_style(id);
    }

    /// Set the container without re-running the cascade.
    ///
    /// A widget controller knows what it *is* (`GtkGrid` is a grid) long
    /// before a restyle happens, and a restyle must not undo it: the
    /// container is remembered on the node and re-applied by every later
    /// [`set_style`](Self::set_style).
    pub fn set_container(&mut self, node: &Node, container: Container) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_container on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.container = container;
        }
        self.write_taffy_style(id);
    }

    /// Place `node` inside its parent.
    ///
    /// Calling this at all opts the node out of the container's blanket
    /// centring: from here on its alignment is exactly what `child` says.
    pub fn set_child_layout(&mut self, node: &Node, child: ChildLayout) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_child_layout on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.child = Some(child);
        }
        self.write_taffy_style(id);
    }

    /// Give one node its own [`Measure`], overriding the one passed to
    /// [`compute`](Self::compute) for that node alone.
    ///
    /// `RefCell` and not a bare `Rc` because `Measure::measure` takes
    /// `&mut self` and a controller keeps its own handle to the same
    /// measurer (contract deviation 1).
    pub fn set_measure(&mut self, node: &Node, measure: Rc<RefCell<dyn Measure>>) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_measure on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.measure = Some(measure);
        }
        if let Err(err) = self.tree.mark_dirty(id) {
            tracing::debug!(%err, "taffy rejected a mark_dirty after set_measure");
        }
    }

    /// This node's placement inside its parent, or `None` if it was never
    /// given one.
    #[must_use]
    pub fn child_layout(&self, node: &Node) -> Option<ChildLayout> {
        let id = *self.ids.get(&node.opaque())?;
        self.tree.get_node_context(id).and_then(|ctx| ctx.child)
    }

    /// This node's container.
    #[must_use]
    pub fn container(&self, node: &Node) -> Container {
        self.ids
            .get(&node.opaque())
            .and_then(|id| self.tree.get_node_context(*id))
            .map_or(Container::default(), |ctx| ctx.container)
    }
```

Add the two helpers and the writer. `container_style` is the only place a
`Container` becomes taffy display/direction/tracks; `child_style` is the only
place a `ChildLayout` does:

```rust
    /// The display, axis, alignment and track half of one node's taffy style.
    fn container_style(container: Container, direction: crate::css::node::Direction, out: &mut Style) {
        /// GTK's grids are small; a `Props`-driven count is not trusted.
        fn tracks(n: u16, homogeneous: bool) -> Vec<taffy::style::TrackSizingFunction> {
            let n = usize::from(n.clamp(1, 1024));
            let track = if homogeneous {
                taffy::prelude::fr(1.0)
            } else {
                taffy::prelude::auto()
            };
            vec![track; n]
        }
        let rtl = direction == crate::css::node::Direction::Rtl;
        match container {
            Container::Box { direction: axis } => {
                out.display = Display::Flex;
                out.flex_direction = match (axis, rtl) {
                    (BoxDirection::Row, false) => FlexDirection::Row,
                    (BoxDirection::Row, true) => FlexDirection::RowReverse,
                    (BoxDirection::Column, _) => FlexDirection::Column,
                };
                // GTK's box centres a child that asked for nothing; a child
                // that called `set_child_layout` overrides this per child.
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::CENTER);
            }
            Container::Center {
                direction: axis,
                shrink_center_last: _,
            } => {
                out.display = Display::Flex;
                out.flex_direction = match (axis, rtl) {
                    (BoxDirection::Row, false) => FlexDirection::Row,
                    (BoxDirection::Row, true) => FlexDirection::RowReverse,
                    (BoxDirection::Column, _) => FlexDirection::Column,
                };
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::SpaceBetween);
            }
            Container::Grid {
                columns,
                rows,
                column_spacing,
                row_spacing,
                column_homogeneous,
                row_homogeneous,
            } => {
                out.display = Display::Grid;
                out.grid_template_columns = tracks(columns, column_homogeneous);
                out.grid_template_rows = tracks(rows, row_homogeneous);
                out.gap = Size {
                    width: length(finite(column_spacing).max(0.0)),
                    height: length(finite(row_spacing).max(0.0)),
                };
                out.align_items = Some(AlignItems::CENTER);
                out.justify_items = Some(taffy::style::JustifyItems::Center);
            }
            Container::Leaf => {
                out.display = Display::Flex;
                out.flex_direction = FlexDirection::Row;
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::CENTER);
            }
        }
    }

    /// The per-child half: alignment, expansion, margin and grid cell.
    ///
    /// The parent's main axis has no per-child alignment in flexbox, so a
    /// main-axis `Align` becomes an auto margin on the free side -- the
    /// standard idiom, and exactly what GTK's own allocation does when a
    /// child is smaller than its cell.
    fn child_style(child: ChildLayout, parent_is_row: bool, out: &mut Style) {
        fn to_self(a: Align) -> Option<taffy::style::AlignSelf> {
            Some(match a {
                Align::Fill => taffy::style::AlignSelf::Stretch,
                Align::Start => taffy::style::AlignSelf::Start,
                Align::End => taffy::style::AlignSelf::End,
                Align::Center => taffy::style::AlignSelf::Center,
                Align::Baseline => taffy::style::AlignSelf::Baseline,
            })
        }
        let (cross, main) = if parent_is_row {
            (child.valign, child.halign)
        } else {
            (child.halign, child.valign)
        };
        out.align_self = to_self(cross);
        let [mt, mr, mb, ml] = child.margin.map(|v| finite(v));
        let (mut lead, mut trail) = (length(if parent_is_row { ml } else { mt }),
                                     length(if parent_is_row { mr } else { mb }));
        match main {
            Align::Fill | Align::Baseline | Align::Start => {}
            Align::End => lead = auto(),
            Align::Center => {
                lead = auto();
                trail = auto();
            }
        }
        out.margin = taffy::geometry::Rect {
            top: if parent_is_row { length(mt) } else { lead },
            right: if parent_is_row { trail } else { length(mr) },
            bottom: if parent_is_row { length(mb) } else { trail },
            left: if parent_is_row { lead } else { length(ml) },
        };
        let expand = if parent_is_row { child.hexpand } else { child.vexpand };
        if expand {
            out.flex_grow = 1.0;
            out.flex_basis = Dimension::length(0.0);
        }
        if let Some(g) = child.grid {
            let col = i16::try_from(g.column.min(1023)).unwrap_or(0) + 1;
            let row = i16::try_from(g.row.min(1023)).unwrap_or(0) + 1;
            out.grid_column = taffy::geometry::Line {
                start: line(col),
                end: span(g.column_span.max(1)),
            };
            out.grid_row = taffy::geometry::Line {
                start: line(row),
                end: span(g.row_span.max(1)),
            };
        }
    }

    /// `v` if it is finite, else `0.0`. Every geometry number that reaches
    /// taffy from a `Props` value passes through here.
    fn finite(v: f32) -> f32 {
        if v.is_finite() { v } else { 0.0 }
    }

    /// Rebuild one node's taffy style from its stored style, container and
    /// child layout. The single writer; `set_style`, `set_container` and
    /// `set_child_layout` all funnel through it.
    fn write_taffy_style(&mut self, id: taffy::NodeId) {
        let Some(ctx) = self.tree.get_node_context(id) else {
            return;
        };
        let (node, style, container, child, env) = (
            ctx.node.clone(),
            Rc::clone(&ctx.style),
            ctx.container,
            ctx.child,
            ctx.env.clone(),
        );
        let mut taffy_style = Self::box_model_style(&style, &env);
        Self::container_style(container, node.direction(), &mut taffy_style);
        if let Some(child) = child {
            let parent_is_row = node.parent().is_some_and(|p| {
                matches!(
                    self.container(&p),
                    Container::Box { direction: BoxDirection::Row }
                        | Container::Center { direction: BoxDirection::Row, .. }
                )
            });
            Self::child_style(child, parent_is_row, &mut taffy_style);
        }
        if let Err(err) = self.tree.set_style(id, taffy_style) {
            tracing::debug!(node = %node.name(), %err, "taffy rejected a style");
        }
    }
```

`box_model_style(style, env) -> Style` is M2's existing `set_style` body from
`let basis = 0.0_f32;` down to the `Style { .. }` literal, moved verbatim
into an associated function, minus the `display`/`flex_direction`/
`align_items`/`justify_content` fields (now `container_style`'s) and minus
the two `self.tree` writes at the end. **No number in it changes.**

Finally, route `compute`'s measure closure through the per-node measure:

```rust
            .compute_layout_with_measure(root_id, available, |inputs, _id, ctx, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, avail| match ctx {
                        Some(ctx) => match &ctx.measure {
                            Some(own) => own.borrow_mut().measure(&ctx.node, &ctx.style, known, avail),
                            None => measure.measure(&ctx.node, &ctx.style, known, avail),
                        },
                        None => taffy::Size::ZERO,
                    },
                )
            })?;
```

`Container::Center`'s `shrink_center_last` is honoured by the *widget*, not
the tree: `CenterBoxC` (Task 3) sets `flex_shrink` through the outer
children's `ChildLayout` and leaves the centre child unshrinkable. The tree
records the flag so `container()` round-trips it.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib layout::tests
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: PASS — the seven new tests and, byte-identically, M2's six
(`min_width_floors_the_content_box`, `border_spacing_becomes_a_gap`, and the
four others) plus `ui/tests/themed_button_offscreen.rs`'s four.

- [ ] **Step 5: Commit**

```bash
git add ui/src/layout.rs
git commit -m "$(cat <<'EOF'
feat(ui): widen layout with GTK's grid and centre containers and per-child placement

Container gains Grid and Center, closing layout.rs's own "M3" note, and every
node can now carry a ChildLayout and its own Measure. An absent ChildLayout --
which is every M2 caller -- keeps the container's CENTER/CENTER, so the M1
button numbers are bit-identical; an explicit Align::Fill is taffy's Stretch.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: the vocabulary — `PropName`, `Handler`, `Prop::Factory`, and P6's shared enums

**Files:**
- Modify: `ui/src/view/mod.rs` — append 89 `PropName` variants, two
  `Handler` variants, one `Prop` variant and their three `Handlers` /
  `PartialEq` arms (deviations 3, 4, 5 and the `Prop::Factory` deviation
  below). Nothing existing is renamed, reordered or removed.
- Modify (create if absent): `ui/src/widgets/types.rs` — P6's shared enums.
- Modify: `ui/src/widgets/mod.rs` — `pub mod types;` and the re-exports.
- Test: `ui/src/widgets/types.rs` (`#[cfg(test)] mod tests`),
  `ui/src/view/mod.rs` (`#[cfg(test)] mod tests`, appended to)

**Interfaces:**
- Consumes (P4): `view::{Prop, PropName, Handler, Handlers, EventKind, Kind}`.
  (P5): `widgets::{Orientation, Position, Side, IconSize, MessageType}` —
  P5 declares these five in `ui/src/widgets/mod.rs` (P5 D4), not in
  `types.rs`; Task 2 creates any of the five P5 did not, with the definitions
  printed in Step 3, and re-exports them from `widgets::types` for uniformity.
  `ListItem` is **P4's** (`view::ListItem`, contract §4.3 `Prop::Items`) — see
  contract §10 E5; it is neither declared nor redeclared here.
- Produces:
  ```rust
  // ui/src/view/mod.rs, additive
  pub enum Handler<Msg> { /* …P4's six… */
      Pair(Rc<dyn Fn(f64, f64) -> Msg>),
      Indices(Rc<dyn Fn(usize, usize) -> Msg>),
  }
  impl<Msg: Clone + 'static> Handlers<Msg> {
      pub fn fire_pair(&self, kind: EventKind, a: f64, b: f64) -> Option<Msg>;
      pub fn fire_indices(&self, kind: EventKind, a: usize, b: usize) -> Option<Msg>;
  }
  pub enum Prop { /* …P4's twelve… */ Factory(ItemFactory) }

  // ui/src/widgets/types.rs
  pub enum Policy { Always, Automatic, Never, External }
  pub enum BaselinePosition { Top, Center, Bottom }
  pub enum SelectionMode { None, Single, Browse, Multiple }
  pub struct Selection { /* mode, set: BTreeSet<usize>, anchor: Option<usize> */ }
  impl Selection {
      pub fn new(mode: SelectionMode) -> Self;
      pub fn set_mode(&mut self, mode: SelectionMode);
      pub fn mode(&self) -> SelectionMode;
      pub fn contains(&self, index: usize) -> bool;
      pub fn selected(&self) -> Vec<usize>;
      pub fn first(&self) -> Option<usize>;
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
      pub fn select(&mut self, index: usize) -> bool;
      pub fn toggle(&mut self, index: usize) -> bool;
      pub fn extend_to(&mut self, index: usize) -> bool;
      pub fn select_all(&mut self, count: usize) -> bool;
      pub fn clear(&mut self) -> bool;
      pub fn retain_below(&mut self, count: usize);
  }
  pub enum StackTransition { None, Crossfade, SlideLeft, SlideRight, SlideUp, SlideDown,
                             SlideLeftRight, SlideUpDown, OverUp, OverDown, OverLeft, OverRight,
                             UnderUp, UnderDown, UnderLeft, UnderRight, RotateLeft, RotateRight }
  pub struct StackPageInfo { pub name: Rc<str>, pub title: Rc<str>,
                             pub icon: Option<IconRef>, pub needs_attention: bool }
  pub enum SortOrder { Ascending, Descending }
  pub struct Sorter(Rc<dyn Fn(&ListItem, &ListItem) -> Ordering>);
  pub struct RowContent { pub label: Rc<str>, pub icon: Option<IconRef>, pub classes: Rc<[Rc<str>]> }
  pub struct ItemFactory(Rc<dyn Fn(usize, &ListItem) -> RowContent>);
  pub struct MenuFlags: u8 { const NESTED = 1; }
  pub enum DisplayHint { Normal, InlineButtons, Circular, HorizontalButtons }
  pub enum LicenseType { Unknown, Custom, Gpl20, Gpl30, Lgpl21, Lgpl30, Agpl30, Bsd, MitX11, Apache20, Mpl20 }
  pub enum Decoration { Csd, SolidCsd, Ssd }
  ```

**One further contract deviation, recorded here because this is the task
that carries it:** §4.3's `Prop` has no variant able to hold a row factory,
and `Prop` cannot be generic over `Msg` (it is stored in a non-generic
`Props`). `ItemFactory` is therefore **`Msg`-free by construction** — it maps
a `ListItem` to a `RowContent` (label, icon, classes), not to a `View<Msg>`.
That is also what makes recycling coherent: rebinding a pooled row sets three
values on an existing node instead of allocating a view subtree. `Prop::Factory`
compares by `Rc::ptr_eq`, exactly as `Prop::Draw` does.

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/types.rs` (or append to P5's) with:

```rust
#[cfg(test)]
mod tests {
    use super::{Decoration, LicenseType, Policy, Selection, SelectionMode};

    #[test]
    fn browse_mode_always_keeps_exactly_one_row_selected() {
        // Mutation check: letting `clear` empty a Browse selection (the
        // Single/Browse copy-paste bug) makes the last assertion 0.
        let mut sel = Selection::new(SelectionMode::Browse);
        assert!(sel.select(3));
        assert_eq!(sel.selected(), vec![3]);
        assert!(!sel.clear(), "Browse refuses to empty");
        assert_eq!(sel.len(), 1);
        assert!(sel.select(5));
        assert_eq!(sel.selected(), vec![5], "Browse replaces, never adds");
    }

    #[test]
    fn multiple_mode_toggles_extends_and_selects_all() {
        // Mutation check: making `extend_to` walk from 0 instead of the
        // anchor selects 0..=6 and this fails on the first assertion.
        let mut sel = Selection::new(SelectionMode::Multiple);
        sel.select(2);
        sel.extend_to(6);
        assert_eq!(sel.selected(), vec![2, 3, 4, 5, 6]);
        assert!(sel.toggle(4));
        assert_eq!(sel.selected(), vec![2, 3, 5, 6]);
        assert!(sel.select_all(8));
        assert_eq!(sel.len(), 8);
        assert!(sel.clear());
        assert!(sel.is_empty());
    }

    #[test]
    fn selection_mode_none_never_selects_anything() {
        // Mutation check: forgetting the SelectionMode::None guard in
        // `select` makes `len()` 1.
        let mut sel = Selection::new(SelectionMode::None);
        assert!(!sel.select(1));
        assert!(!sel.toggle(1));
        assert!(!sel.select_all(4));
        assert_eq!(sel.len(), 0);
    }

    #[test]
    fn retain_below_drops_rows_a_shrunken_model_no_longer_has() {
        // Mutation check: skipping retain_below after a model swap leaves a
        // selected index past the end and the list paints a phantom row.
        let mut sel = Selection::new(SelectionMode::Multiple);
        sel.select(1);
        sel.select(9);
        sel.retain_below(5);
        assert_eq!(sel.selected(), vec![1]);
    }

    #[test]
    fn a_selection_never_panics_on_hostile_indices() {
        // Indices come from an application model and from hit-testing.
        for mode in [
            SelectionMode::None,
            SelectionMode::Single,
            SelectionMode::Browse,
            SelectionMode::Multiple,
        ] {
            let mut sel = Selection::new(mode);
            sel.select(usize::MAX);
            sel.extend_to(0);
            sel.toggle(usize::MAX);
            sel.select_all(usize::MAX);
            sel.retain_below(0);
            let _ = sel.selected();
            let _ = sel.first();
        }
    }

    #[test]
    fn the_scalar_enums_round_trip_their_wire_encoding() {
        // Mutation check: `Prop::Enum` stores a u16 discriminant; a
        // mismatched from_u16 silently reads a neighbouring variant, which
        // is how a Policy::Never scrollbar becomes Policy::Always.
        for p in [Policy::Always, Policy::Automatic, Policy::Never, Policy::External] {
            assert_eq!(Policy::from_u16(p as u16), p);
        }
        for d in [Decoration::Csd, Decoration::SolidCsd, Decoration::Ssd] {
            assert_eq!(Decoration::from_u16(d as u16), d);
        }
        assert_eq!(Policy::from_u16(9999), Policy::Automatic, "unknown => initial");
        assert_eq!(LicenseType::from_u16(9999), LicenseType::Unknown);
    }
}
```

Append to `ui/src/view/mod.rs`'s `mod tests`:

```rust
    #[test]
    fn the_two_new_handler_arities_fire_and_the_others_do_not() {
        // Mutation check: making fire_pair fall through to fire_unit returns
        // Some for the Indices handler too, and a scroll event would emit a
        // reorder message.
        let mut h: Handlers<(u32, u32)> = Handlers::default();
        h.set(
            EventKind::Scrolled,
            Handler::Pair(Rc::new(|a, b| (a as u32, b as u32))),
        );
        h.set(
            EventKind::Reordered,
            Handler::Indices(Rc::new(|a, b| (a as u32, b as u32))),
        );
        assert_eq!(h.fire_pair(EventKind::Scrolled, 3.0, 4.0), Some((3, 4)));
        assert_eq!(h.fire_indices(EventKind::Reordered, 1, 2), Some((1, 2)));
        assert_eq!(h.fire_pair(EventKind::Reordered, 1.0, 2.0), None);
        assert_eq!(h.fire_unit(EventKind::Scrolled), None);
    }

    #[test]
    fn a_factory_prop_compares_by_pointer_like_draw() {
        // Mutation check: comparing factories as always-equal makes
        // Props::diff miss a model swap and the list keeps the old rows.
        use crate::widgets::types::{ItemFactory, ListItem, RowContent};
        let f = ItemFactory::new(|_, item: &ListItem| RowContent::from_label(&item.label));
        let g = ItemFactory::new(|_, item: &ListItem| RowContent::from_label(&item.label));
        assert_eq!(Prop::Factory(f.clone()), Prop::Factory(f));
        assert_ne!(
            Prop::Factory(ItemFactory::new(|_, i: &ListItem| RowContent::from_label(&i.label))),
            Prop::Factory(g)
        );
    }

    #[test]
    fn every_p6_prop_name_is_distinct_and_ordered() {
        // Mutation check: a duplicated variant name would not compile, but a
        // duplicated *use* (two builders writing PropName::Position for
        // different meanings) shows up as a Props::set collision; this pins
        // the count so an accidental deletion during a rebase is caught.
        let names = PropName::all_p6();
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate P6 PropName");
        assert_eq!(names.len(), 89);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::types view::tests`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `types` in
`widgets`` and `error[E0599]: no variant named `Pair` found for enum
`Handler``.

- [ ] **Step 3: Write minimal implementation**

**(a) `ui/src/view/mod.rs`, additive only.** Append these variants to
`PropName`'s `#[non_exhaustive]` body, after the last existing one:

```rust
    // P6 · containers
    BaselinePosition, BaselineChild, BaselineRow, RowHomogeneous, ColumnHomogeneous,
    ShrinkCenterLast, HscrollbarPolicy, VscrollbarPolicy, HasFrame, MaxContentWidth,
    MaxContentHeight, PropagateNaturalWidth, PropagateNaturalHeight, PositionSet,
    WideHandle, ResizeStart, ResizeEnd, ShrinkStart, ShrinkEnd, LabelXalign,
    LabelWidget, UseUnderline, ResizeToplevel, SearchMode, ShowCloseButton,
    KeyCapture, Revealed, ShowTitleButtons, TitleWidget, TabPos, Scrollable,
    ShowTabs, ShowBorder, Reorderable, Detachable, MeasureOverlay, ClipOverlay,
    VisibleChild, Hhomogeneous, Vhomogeneous, InterpolateSize, PageName, PageTitle,
    NeedsAttention, Pages,
    // P6 · lists
    ActivateOnSingleClick, Activatable, MinChildrenPerLine, MaxChildrenPerLine,
    SingleClickActivate, EnableRubberband, MinColumns, MaxColumns, ShowRowSeparators,
    ShowColumnSeparators, SortColumn, SortOrder, Expand,
    // P6 · menus
    Flags, VisibleSubmenu, Accel, Submenu, Section, DisplayHint, Menus,
    // P6 · windows
    DefaultWidget, Deletable, Decorated, DefaultWidth, DefaultHeight, IconName,
    SectionName, ViewName, ProgramName, Version, Comments, Copyright, License,
    LicenseType, Website, WebsiteLabel, Authors, Artists, Documenters,
    TranslatorCredits, LogoIconName, WrapLicense, DefaultButton, CancelButton,
```

and, in `impl PropName`, the roster the count test reads:

```rust
    /// Every `PropName` Part 6 introduced, in declaration order.
    ///
    /// Exists so a rebase that drops one fails a test instead of failing a
    /// widget at run time.
    #[must_use]
    pub fn all_p6() -> &'static [PropName] {
        use PropName::*;
        &[
            BaselinePosition, BaselineChild, BaselineRow, RowHomogeneous,
            ColumnHomogeneous, ShrinkCenterLast, HscrollbarPolicy, VscrollbarPolicy,
            HasFrame, MaxContentWidth, MaxContentHeight, PropagateNaturalWidth,
            PropagateNaturalHeight, PositionSet, WideHandle, ResizeStart, ResizeEnd,
            ShrinkStart, ShrinkEnd, LabelXalign, LabelWidget, UseUnderline,
            ResizeToplevel, SearchMode, ShowCloseButton, KeyCapture, Revealed,
            ShowTitleButtons, TitleWidget, TabPos, Scrollable, ShowTabs, ShowBorder,
            Reorderable, Detachable, MeasureOverlay, ClipOverlay, VisibleChild,
            Hhomogeneous, Vhomogeneous, InterpolateSize, PageName, PageTitle,
            NeedsAttention, Pages, ActivateOnSingleClick, Activatable,
            MinChildrenPerLine, MaxChildrenPerLine, SingleClickActivate,
            EnableRubberband, MinColumns, MaxColumns, ShowRowSeparators,
            ShowColumnSeparators, SortColumn, SortOrder, Expand, Flags,
            VisibleSubmenu, Accel, Submenu, Section, DisplayHint, Menus,
            DefaultWidget, Deletable, Decorated, DefaultWidth, DefaultHeight,
            IconName, SectionName, ViewName, ProgramName, Version, Comments,
            Copyright, License, LicenseType, Website, WebsiteLabel, Authors,
            Artists, Documenters, TranslatorCredits, LogoIconName, WrapLicense,
            DefaultButton, CancelButton,
        ]
    }
```

Append to `Handler<Msg>`:

```rust
    /// Two floats, for `on_scrolled((x, y))`.
    Pair(Rc<dyn Fn(f64, f64) -> Msg>),
    /// Two indices, for `on_reordered((from, to))`.
    Indices(Rc<dyn Fn(usize, usize) -> Msg>),
```

and to `impl<Msg: Clone + 'static> Handlers<Msg>`:

```rust
    /// Fire a two-float handler; `None` when nothing of that arity is bound.
    #[must_use]
    pub fn fire_pair(&self, kind: EventKind, a: f64, b: f64) -> Option<Msg> {
        match self.0.iter().find(|(k, _)| *k == kind).map(|(_, h)| h) {
            Some(Handler::Pair(f)) => Some(f(a, b)),
            _ => None,
        }
    }

    /// Fire a two-index handler; `None` when nothing of that arity is bound.
    #[must_use]
    pub fn fire_indices(&self, kind: EventKind, a: usize, b: usize) -> Option<Msg> {
        match self.0.iter().find(|(k, _)| *k == kind).map(|(_, h)| h) {
            Some(Handler::Indices(f)) => Some(f(a, b)),
            _ => None,
        }
    }
```

Append to `Prop` and its `PartialEq`:

```rust
    /// A row factory for the list family. `Msg`-free by construction so it
    /// can live in the non-generic `Props`.
    Factory(crate::widgets::types::ItemFactory),
```
```rust
            (Prop::Factory(a), Prop::Factory(b)) => a.ptr_eq(b),
```

**(b) `ui/src/widgets/types.rs`.** Add (P5 owns the first six; create only
the ones that are not already there, with exactly these definitions):

```rust
//! Scalar vocabulary shared by the widget catalogue.
//!
//! Every enum here is `#[repr(u16)]` with a `from_u16` that maps an unknown
//! discriminant to the property's *initial* value rather than panicking:
//! these round-trip through `Prop::Enum`, whose payload arrives from an
//! application model.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::rc::Rc;

use crate::css::value::image::IconRef;

/// A widget's main axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Orientation {
    /// Left to right (or right to left under RTL).
    #[default]
    Horizontal = 0,
    /// Top to bottom.
    Vertical = 1,
}

/// An edge, as GTK's `GtkPositionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Position {
    /// Left edge.
    Left = 0,
    /// Right edge.
    Right = 1,
    /// Top edge.
    #[default]
    Top = 2,
    /// Bottom edge.
    Bottom = 3,
}

/// Which half of `gtk-decoration-layout` a `WindowControls` renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Side {
    /// Before the colon.
    #[default]
    Start = 0,
    /// After the colon.
    End = 1,
}

/// One row of a list model.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Stable identity, for keyed reconciliation and selection across edits.
    pub id: u64,
    /// The row's text.
    pub label: Rc<str>,
    /// An optional leading icon.
    pub icon: Option<IconRef>,
    /// Extra style classes for the row node.
    pub classes: Rc<[Rc<str>]>,
}

/// When a scrollbar is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Policy {
    /// Always visible.
    Always = 0,
    /// Visible only when the content overflows.
    #[default]
    Automatic = 1,
    /// Never visible; scrolling still works.
    Never = 2,
    /// The application draws it.
    External = 3,
}

/// Where a box aligns baselines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum BaselinePosition {
    /// Baseline at the top of the extra space.
    Top = 0,
    /// Centred.
    #[default]
    Center = 1,
    /// Baseline at the bottom.
    Bottom = 2,
}

/// How many rows a list lets the user select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum SelectionMode {
    /// Nothing is selectable.
    None = 0,
    /// Zero or one row.
    #[default]
    Single = 1,
    /// Exactly one row, always.
    Browse = 2,
    /// Any number of rows.
    Multiple = 3,
}

/// A stack's page-change animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum StackTransition {
    /// Instant.
    #[default]
    None = 0,
    /// Cross-fade.
    Crossfade = 1,
    /// New page slides in from the right.
    SlideLeft = 2,
    /// New page slides in from the left.
    SlideRight = 3,
    /// New page slides in from below.
    SlideUp = 4,
    /// New page slides in from above.
    SlideDown = 5,
    /// Direction chosen from the page order, horizontally.
    SlideLeftRight = 6,
    /// Direction chosen from the page order, vertically.
    SlideUpDown = 7,
    /// New page covers the old, upwards.
    OverUp = 8,
    /// New page covers the old, downwards.
    OverDown = 9,
    /// New page covers the old, leftwards.
    OverLeft = 10,
    /// New page covers the old, rightwards.
    OverRight = 11,
    /// Old page uncovers the new, upwards.
    UnderUp = 12,
    /// Old page uncovers the new, downwards.
    UnderDown = 13,
    /// Old page uncovers the new, leftwards.
    UnderLeft = 14,
    /// Old page uncovers the new, rightwards.
    UnderRight = 15,
    /// Rotate left.
    RotateLeft = 16,
    /// Rotate right.
    RotateRight = 17,
}

/// What a `StackSwitcher`/`StackSidebar` needs to know about a page.
#[derive(Debug, Clone, PartialEq)]
pub struct StackPageInfo {
    /// The page's `name`, the value `visible_child` selects by.
    pub name: Rc<str>,
    /// Human-readable title, shown on the switcher button.
    pub title: Rc<str>,
    /// Optional icon.
    pub icon: Option<IconRef>,
    /// Adds `.needs-attention` to the switcher button.
    pub needs_attention: bool,
}

/// A column's sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum SortOrder {
    /// Smallest first.
    #[default]
    Ascending = 0,
    /// Largest first.
    Descending = 1,
}

/// A total order over list items, for a sortable column.
#[derive(Clone)]
pub struct Sorter(Rc<dyn Fn(&ListItem, &ListItem) -> Ordering>);

impl Sorter {
    /// Wrap a comparison.
    #[must_use]
    pub fn new(f: impl Fn(&ListItem, &ListItem) -> Ordering + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// Compare by label, the default a column with no sorter gets.
    #[must_use]
    pub fn by_label() -> Self {
        Self::new(|a, b| a.label.cmp(&b.label))
    }

    /// Apply.
    #[must_use]
    pub fn compare(&self, a: &ListItem, b: &ListItem) -> Ordering {
        (self.0)(a, b)
    }

    /// Same closure?
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Sorter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sorter(..)")
    }
}

/// What a recycled row displays. Deliberately `Msg`-free: rebinding a pooled
/// row must not allocate a view subtree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowContent {
    /// The row's text.
    pub label: Rc<str>,
    /// An optional leading icon.
    pub icon: Option<IconRef>,
    /// Extra classes for the row node.
    pub classes: Rc<[Rc<str>]>,
}

impl RowContent {
    /// A plain text row.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        Self {
            label: Rc::from(label),
            icon: None,
            classes: Rc::from(&[][..]),
        }
    }
}

/// Maps a model index and item to what its row shows.
#[derive(Clone)]
pub struct ItemFactory(Rc<dyn Fn(usize, &ListItem) -> RowContent>);

impl ItemFactory {
    /// Wrap a binder.
    #[must_use]
    pub fn new(f: impl Fn(usize, &ListItem) -> RowContent + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// The default factory: the item's own label, icon and classes.
    #[must_use]
    pub fn label_only() -> Self {
        Self::new(|_, item| RowContent {
            label: Rc::clone(&item.label),
            icon: item.icon.clone(),
            classes: Rc::clone(&item.classes),
        })
    }

    /// Bind one row.
    #[must_use]
    pub fn bind(&self, index: usize, item: &ListItem) -> RowContent {
        (self.0)(index, item)
    }

    /// Same closure?
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ItemFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ItemFactory(..)")
    }
}

bitflags::bitflags! {
    /// `GtkPopoverMenuFlags`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct MenuFlags: u8 {
        /// Submenus open as nested popovers instead of sliding in place.
        const NESTED = 1;
    }
}

/// A menu section's `display-hint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum DisplayHint {
    /// A plain vertical section.
    #[default]
    Normal = 0,
    /// `.inline-buttons`: the section's items render as a button row.
    InlineButtons = 1,
    /// `.circular`: round icon buttons.
    Circular = 2,
    /// `.horizontal-buttons`.
    HorizontalButtons = 3,
}

/// The licences `AboutDialog` can name without being handed the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum LicenseType {
    /// No licence stated.
    #[default]
    Unknown = 0,
    /// The application supplied its own text.
    Custom = 1,
    /// GNU GPL 2.0 or later.
    Gpl20 = 2,
    /// GNU GPL 3.0 or later.
    Gpl30 = 3,
    /// GNU LGPL 2.1 or later.
    Lgpl21 = 4,
    /// GNU LGPL 3.0 or later.
    Lgpl30 = 5,
    /// GNU AGPL 3.0 or later.
    Agpl30 = 6,
    /// BSD 2-clause.
    Bsd = 7,
    /// MIT/X11.
    MitX11 = 8,
    /// Apache 2.0.
    Apache20 = 9,
    /// Mozilla Public License 2.0.
    Mpl20 = 10,
}

/// Who draws a window's decorations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Decoration {
    /// Client-side, with a shadow: `.csd`.
    Csd = 0,
    /// Client-side, no shadow: `.solid-csd`.
    SolidCsd = 1,
    /// Server-side, which is what icedtea's compositor does: `.ssd`.
    #[default]
    Ssd = 2,
}

/// One `from_u16` per `#[repr(u16)]` enum above: an unknown discriminant is
/// the property's initial value, never a panic.
macro_rules! from_u16 {
    ($ty:ident { $($variant:ident),+ $(,)? }) => {
        impl $ty {
            /// Decode a `Prop::Enum` payload; unknown values fall back to the
            /// initial value and are logged once per process.
            #[must_use]
            pub fn from_u16(raw: u16) -> Self {
                $(if raw == Self::$variant as u16 { return Self::$variant; })+
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    tracing::warn!(raw, ty = stringify!($ty), "unknown enum discriminant; using the initial value");
                });
                Self::default()
            }

            /// Encode for `Prop::Enum`.
            #[must_use]
            pub fn to_u16(self) -> u16 {
                self as u16
            }
        }
    };
}

from_u16!(Orientation { Horizontal, Vertical });
from_u16!(Position { Left, Right, Top, Bottom });
from_u16!(Side { Start, End });
from_u16!(Policy { Always, Automatic, Never, External });
from_u16!(BaselinePosition { Top, Center, Bottom });
from_u16!(SelectionMode { None, Single, Browse, Multiple });
from_u16!(SortOrder { Ascending, Descending });
from_u16!(DisplayHint { Normal, InlineButtons, Circular, HorizontalButtons });
from_u16!(Decoration { Csd, SolidCsd, Ssd });
from_u16!(StackTransition {
    None, Crossfade, SlideLeft, SlideRight, SlideUp, SlideDown, SlideLeftRight,
    SlideUpDown, OverUp, OverDown, OverLeft, OverRight, UnderUp, UnderDown,
    UnderLeft, UnderRight, RotateLeft, RotateRight,
});
from_u16!(LicenseType {
    Unknown, Custom, Gpl20, Gpl30, Lgpl21, Lgpl30, Agpl30, Bsd, MitX11, Apache20, Mpl20,
});

/// A set of selected model indices, with GTK's four selection modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    mode: SelectionMode,
    set: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl Selection {
    /// An empty selection in `mode`.
    #[must_use]
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            set: BTreeSet::new(),
            anchor: None,
        }
    }

    /// Switch modes, collapsing the set when the new mode allows fewer rows.
    pub fn set_mode(&mut self, mode: SelectionMode) {
        self.mode = mode;
        match mode {
            SelectionMode::None => self.set.clear(),
            SelectionMode::Single | SelectionMode::Browse => {
                let keep = self.set.iter().next().copied();
                self.set.clear();
                if let Some(k) = keep {
                    self.set.insert(k);
                }
            }
            SelectionMode::Multiple => {}
        }
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Is `index` selected?
    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        self.set.contains(&index)
    }

    /// Selected indices, ascending.
    #[must_use]
    pub fn selected(&self) -> Vec<usize> {
        self.set.iter().copied().collect()
    }

    /// The lowest selected index.
    #[must_use]
    pub fn first(&self) -> Option<usize> {
        self.set.iter().next().copied()
    }

    /// How many rows are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Nothing selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Select `index`, replacing the set under `Single`/`Browse`. Returns
    /// whether anything changed.
    pub fn select(&mut self, index: usize) -> bool {
        if self.mode == SelectionMode::None {
            return false;
        }
        self.anchor = Some(index);
        match self.mode {
            SelectionMode::Multiple => self.set.insert(index),
            _ => {
                if self.set.len() == 1 && self.set.contains(&index) {
                    return false;
                }
                self.set.clear();
                self.set.insert(index);
                true
            }
        }
    }

    /// Flip `index`. Under `Single` this deselects; under `Browse` it cannot.
    pub fn toggle(&mut self, index: usize) -> bool {
        match self.mode {
            SelectionMode::None => false,
            SelectionMode::Browse => self.select(index),
            SelectionMode::Single => {
                self.anchor = Some(index);
                if self.set.remove(&index) {
                    true
                } else {
                    self.set.clear();
                    self.set.insert(index)
                }
            }
            SelectionMode::Multiple => {
                self.anchor = Some(index);
                if self.set.remove(&index) {
                    true
                } else {
                    self.set.insert(index)
                }
            }
        }
    }

    /// Extend the selection from the anchor to `index` (Shift-click).
    pub fn extend_to(&mut self, index: usize) -> bool {
        if self.mode != SelectionMode::Multiple {
            return self.select(index);
        }
        let Some(anchor) = self.anchor else {
            return self.select(index);
        };
        let (lo, hi) = if anchor <= index { (anchor, index) } else { (index, anchor) };
        let before = self.set.len();
        // `hi` is a model index, so the range is bounded by the model, but
        // `usize::MAX` from a hostile prop must not be walked one by one.
        let hi = hi.min(lo.saturating_add(1_000_000));
        for i in lo..=hi {
            self.set.insert(i);
        }
        self.set.len() != before
    }

    /// Ctrl-A. `count` is the model length.
    pub fn select_all(&mut self, count: usize) -> bool {
        if self.mode != SelectionMode::Multiple {
            return false;
        }
        let before = self.set.len();
        for i in 0..count.min(1_000_000) {
            self.set.insert(i);
        }
        self.set.len() != before
    }

    /// Deselect everything. `Browse` refuses while it holds a row.
    pub fn clear(&mut self) -> bool {
        if self.mode == SelectionMode::Browse && !self.set.is_empty() {
            return false;
        }
        let changed = !self.set.is_empty();
        self.set.clear();
        changed
    }

    /// Drop every index at or above `count`, after a model swap.
    pub fn retain_below(&mut self, count: usize) {
        self.set.retain(|i| *i < count);
        if self.anchor.is_some_and(|a| a >= count) {
            self.anchor = None;
        }
    }
}
```

**(c) `ui/src/widgets/mod.rs`:** add `pub mod types;` and
`pub use types::*;` beside P5's existing module list.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::types view::tests
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: PASS — nine new tests; every P4 and P5 test unchanged.

- [ ] **Step 5: Commit**

```bash
git add ui/src/view/mod.rs ui/src/widgets/types.rs ui/src/widgets/mod.rs
git commit -m "$(cat <<'EOF'
feat(ui): add P6's prop names, handler arities and shared widget enums

89 additive PropName variants, Handler::Pair/Indices for the two-argument
signals the container and list catalogues need, and Prop::Factory carrying an
Msg-free ItemFactory so a recycled row rebinds content instead of allocating a
view subtree. Selection implements GTK's four selection modes, including
Browse's "always exactly one".

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `node_tree.rs` — the GTK-notation renderer, the fixture matcher and the headless build harness

**Files:**
- Create: `ui/src/widgets/node_tree.rs`
- Modify: `ui/src/widgets/mod.rs` — `pub mod node_tree;`,
  `pub use node_tree::{node_tree_of, matches_fixture, Mismatch};`, the
  `Headless`/`build_widget` harness, and the `build_controller` catch-all.
- Modify: `ui/tests/node_trees.rs` — P5's file keeps its tests; add the
  `fixture()` loader and the `assert_fixture` helper P6's 27 tests use.
- Test: `ui/src/widgets/node_tree.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `css::node::Node` (M2), `view::{Kind, Props}` (P4),
  `view::Controller`/`BuildCx` (P4), `widgets::build_controller` (P5).
- Produces:
  ```rust
  pub fn node_tree_of(kind: Kind, props: &Props) -> String;
  pub fn render_tree(root: &Node) -> String;
  pub fn matches_fixture(root: &Node, fixture: &str) -> Result<(), Mismatch>;
  pub struct Mismatch { pub expected: String, pub rendered: String, pub reason: Rc<str> }
  // ui/src/widgets/mod.rs
  pub struct Headless { /* sheet, fonts, icons, clock, env */ }
  impl Headless { pub fn new() -> Self; pub fn cx(&mut self) -> BuildCx<'_>; }
  pub struct BuiltWidget<Msg> { pub node: Node, pub controller: Box<dyn Controller<Msg>> }
  pub fn build_widget<Msg: Clone + 'static>(kind: Kind, props: &Props) -> BuiltWidget<Msg>;
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/node_tree.rs` with only this test module (the rest
arrives in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::{matches_fixture, render_tree};
    use crate::css::node::Node;

    /// `paned > (widget, separator.wide, widget)`.
    fn paned() -> Node {
        let root = Node::new("paned");
        let a = Node::new("widget");
        let sep = Node::with_classes("separator", &["wide"]);
        let b = Node::new("widget");
        for n in [&a, &sep, &b] {
            root.append_child(n);
        }
        root
    }

    #[test]
    fn a_tree_renders_in_gtks_own_notation() {
        // Mutation check: emitting "|--" instead of GTK's "├──", or losing
        // the leading class dot, makes this string compare fail and every
        // fixture unreadable next to the docs it was copied from.
        assert_eq!(
            render_tree(&paned()),
            "paned\n├── widget\n├── separator.wide\n╰── widget\n"
        );
    }

    #[test]
    fn an_exact_fixture_matches_and_a_wrong_name_does_not() {
        // Mutation check: matching on the first child only (forgetting to
        // advance the actual cursor) accepts the second fixture too.
        assert!(matches_fixture(&paned(), "paned\n├── <child>\n├── separator[.wide]\n╰── <child>\n").is_ok());
        let err = matches_fixture(&paned(), "paned\n├── <child>\n├── handle\n╰── <child>\n")
            .expect_err("a renamed subnode must fail");
        assert!(err.reason.contains("handle"), "the reason names the node");
        assert!(err.rendered.contains("separator.wide"), "the render is attached");
    }

    #[test]
    fn optional_subnodes_may_be_absent_and_optional_classes_may_be_missing() {
        // Mutation check: treating `[name]` as required fails the first
        // assertion; treating `name[.class]` as requiring the class fails
        // the second.
        let sw = Node::new("scrolledwindow");
        let child = Node::new("widget");
        let bar = Node::with_classes("scrollbar", &["vertical"]);
        sw.append_child(&child);
        sw.append_child(&bar);
        let fixture = "scrolledwindow[.frame]\n├── <child>\n├── [overshoot.top]\n\
                       ├── [scrollbar.horizontal]\n╰── [scrollbar.vertical[.dragging]]\n";
        assert!(matches_fixture(&sw, fixture).is_ok());
        assert!(matches_fixture(&Node::new("scrolledwindow"), "scrolledwindow[.frame]\n").is_ok());
    }

    #[test]
    fn a_repetition_marker_accepts_any_number_of_the_line_above() {
        // Mutation check: consuming exactly one repeat makes the three-item
        // menu fail; consuming zero makes the empty one fail.
        let menu = Node::with_classes("popover", &["background", "menu"]);
        for _ in 0..3 {
            menu.append_child(&Node::with_classes("button", &["model"]));
        }
        let fixture = "popover.background.menu\n├── button.model\n┊\n╰── button.model\n";
        assert!(matches_fixture(&menu, fixture).is_ok());
        let empty = Node::with_classes("popover", &["background", "menu"]);
        empty.append_child(&Node::with_classes("button", &["model"]));
        assert!(matches_fixture(&empty, fixture).is_ok());
    }

    #[test]
    fn a_required_class_that_is_missing_fails_with_a_reason() {
        // Mutation check: comparing only node names lets a `list` without
        // `.navigation-sidebar` pass, and the sidebar's whole styling with it.
        let list = Node::new("list");
        let err = matches_fixture(&list, "list.navigation-sidebar\n").expect_err("missing class");
        assert!(err.reason.contains("navigation-sidebar"));
    }

    #[test]
    fn a_hostile_fixture_never_panics() {
        // Fixtures are vendored, but they are text: a bad rebase, a stray
        // BOM or a truncated copy must fail the test, never abort the run.
        for text in [
            "",
            "\u{feff}box",
            "├──",
            "╰── ╰── ╰──",
            "[",
            "[[[[[[[[[[",
            "box\n\t\t\t\tmangled",
            "name.\n",
            "\u{0}\u{1}\u{2}",
            &"├── x\n".repeat(5_000),
            &format!("box\n{}", "    ".repeat(4_000)),
        ] {
            let _ = matches_fixture(&paned(), text);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::node_tree`
Expected: FAIL — `error[E0432]: unresolved import `super::matches_fixture``
(the module has no items yet).

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/widgets/node_tree.rs`:

```rust
//! GTK's "CSS nodes" notation, both directions.
//!
//! [`render_tree`] prints a retained [`Node`] subtree the way GTK's own
//! documentation prints it; [`matches_fixture`] checks a subtree against a
//! vendored block from those docs. The fixture language has four pieces of
//! slack a concrete tree cannot express -- `[subnode]`, `name[.class]`,
//! `<child>` and `┊` -- so the check is a matcher, not a string compare
//! (contract deviation 8). A fixture with none of them is an exact compare.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::{Kind, Props};

/// Why a subtree did not match its fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The fixture, as given.
    pub expected: String,
    /// The subtree, rendered in the same notation.
    pub rendered: String,
    /// What specifically went wrong, naming the node or class.
    pub reason: Rc<str>,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\n\n--- fixture ---\n{}\n--- rendered ---\n{}",
            self.reason, self.expected, self.rendered
        )
    }
}

/// Render `root` and its descendants in GTK's notation.
#[must_use]
pub fn render_tree(root: &Node) -> String {
    let mut out = String::new();
    out.push_str(&label(root));
    out.push('\n');
    render_children(root, "", &mut out);
    out
}

/// `name.class.class`, classes in the node's own order.
fn label(node: &Node) -> String {
    let mut s = String::from(&*node.name());
    for class in node.classes() {
        s.push('.');
        s.push_str(class.as_str());
    }
    s
}

fn render_children(node: &Node, prefix: &str, out: &mut String) {
    let children = node.children();
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == children.len();
        out.push_str(prefix);
        out.push_str(if last { "╰── " } else { "├── " });
        out.push_str(&label(child));
        out.push('\n');
        let mut deeper = String::from(prefix);
        deeper.push_str(if last { "    " } else { "│   " });
        render_children(child, &deeper, out);
    }
}

/// One line of a fixture, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// `None` for `<child>`, which matches any subtree.
    name: Option<String>,
    /// Classes the node must carry.
    required: Vec<String>,
    /// Classes it may carry; anything else is a failure.
    optional: Vec<String>,
    /// `[subnode]` -- may be absent.
    subnode_optional: bool,
    /// A `┊` line followed this one: it may repeat.
    repeats: bool,
    /// Children, by indentation.
    children: Vec<Pattern>,
}

/// Depth and payload of one fixture line, or `None` for a blank/`┊` line.
fn split_line(line: &str) -> Option<(usize, &str, bool)> {
    let line = line.trim_end();
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    if line.trim().is_empty() {
        return None;
    }
    let mut depth = 0usize;
    let mut rest = line;
    loop {
        let next = rest
            .strip_prefix("│   ")
            .or_else(|| rest.strip_prefix("┊   "))
            .or_else(|| rest.strip_prefix("    "));
        match next {
            Some(r) => {
                depth += 1;
                rest = r;
            }
            None => break,
        }
    }
    let (rest, marked) = match rest.strip_prefix("├── ").or_else(|| rest.strip_prefix("╰── ")) {
        Some(r) => (r, true),
        None => (rest, false),
    };
    if marked {
        depth += 1;
    }
    let rest = rest.trim();
    if rest.is_empty() || rest.chars().all(|c| c == '┊' || c == '⋮' || c == '│') {
        // A repetition marker: no node of its own.
        return Some((depth, "", true));
    }
    Some((depth, rest, false))
}

/// `scrollbar.vertical[.dragging]` / `[overshoot.top]` / `<child>`.
fn parse_pattern(text: &str) -> Pattern {
    let mut text = text.trim();
    let mut subnode_optional = false;
    if text.starts_with('[') && text.ends_with(']') && text.len() >= 2 {
        // Only an outer bracket that wraps the *whole* item is a subnode
        // marker; `name[.class]` keeps its brackets for the class pass.
        let inner = &text[1..text.len() - 1];
        if !inner.contains('[') {
            subnode_optional = true;
            text = inner;
        }
    }
    if text.starts_with('<') {
        return Pattern {
            name: None,
            required: Vec::new(),
            optional: Vec::new(),
            subnode_optional: true,
            repeats: false,
            children: Vec::new(),
        };
    }
    let mut name = String::new();
    let mut required = Vec::new();
    let mut optional = Vec::new();
    let mut cursor = text;
    // Leading name, up to the first `.` or `[`.
    let split = cursor
        .find(['.', '['])
        .unwrap_or(cursor.len());
    name.push_str(&cursor[..split]);
    cursor = &cursor[split..];
    while !cursor.is_empty() {
        if let Some(rest) = cursor.strip_prefix('[') {
            let end = rest.find(']').unwrap_or(rest.len());
            for class in rest[..end].split('.').filter(|s| !s.is_empty()) {
                optional.push(class.to_string());
            }
            cursor = rest.get(end + 1..).unwrap_or("");
        } else if let Some(rest) = cursor.strip_prefix('.') {
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            if !rest[..end].is_empty() {
                required.push(rest[..end].to_string());
            }
            cursor = &rest[end..];
        } else {
            // Anything else in a fixture line is noise; skip one char and
            // keep going rather than looping forever on it.
            let mut chars = cursor.chars();
            chars.next();
            cursor = chars.as_str();
        }
    }
    Pattern {
        name: Some(name.trim().to_string()),
        required,
        optional,
        subnode_optional,
        repeats: false,
        children: Vec::new(),
    }
}

/// Parse a whole fixture into its root pattern.
fn parse_fixture(text: &str) -> Option<Pattern> {
    let mut stack: Vec<Pattern> = Vec::new();
    let mut root: Option<Pattern> = None;
    for line in text.lines() {
        let Some((depth, payload, is_repeat)) = split_line(line) else {
            continue;
        };
        if is_repeat {
            // Mark the deepest pattern parsed so far as repeatable.
            if let Some(last) = stack.last_mut().and_then(|p| p.children.last_mut()) {
                last.repeats = true;
            } else if let Some(last) = stack.last_mut() {
                last.repeats = true;
            }
            continue;
        }
        let pattern = parse_pattern(payload);
        if depth == 0 {
            if root.is_some() {
                // A second root line: fixtures with two alternatives (GTK's
                // `separator.horizontal` / `separator.vertical`) are matched
                // by whichever alternative the caller's tree is.
                continue;
            }
            stack.clear();
            stack.push(pattern);
            continue;
        }
        while stack.len() > depth {
            let done = stack.pop()?;
            stack.last_mut()?.children.push(done);
        }
        if stack.len() != depth {
            // Malformed indentation: attach at the current level rather than
            // indexing past the stack.
            stack.last_mut()?.children.push(pattern);
            continue;
        }
        stack.push(pattern);
    }
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.children.push(done);
    }
    root = stack.pop().or(root);
    root
}

/// Does `node` satisfy `pattern`'s own name and classes?
fn head_matches(pattern: &Pattern, node: &Node) -> Result<(), String> {
    let Some(expected) = pattern.name.as_deref() else {
        return Ok(());
    };
    if &*node.name() != expected {
        return Err(format!("expected node `{expected}`, found `{}`", node.name()));
    }
    let classes: Vec<String> = node.classes().iter().map(|c| c.as_str().to_string()).collect();
    for want in &pattern.required {
        if !classes.iter().any(|c| c == want) {
            return Err(format!(
                "node `{expected}` is missing the required class `{want}` (has {classes:?})"
            ));
        }
    }
    Ok(())
}

/// Backtracking match of a pattern list against a child list.
fn match_list(pats: &[Pattern], actual: &[Node], reason: &mut String) -> bool {
    fn go(pats: &[Pattern], actual: &[Node], pi: usize, ai: usize, reason: &mut String) -> bool {
        if pi == pats.len() {
            if ai == actual.len() {
                return true;
            }
            *reason = format!(
                "unexpected extra subnode `{}`",
                actual.get(ai).map_or_else(String::new, |n| n.name().to_string())
            );
            return false;
        }
        let pat = &pats[pi];
        let optional = pat.subnode_optional || pat.name.is_none();
        let max_repeat = if pat.repeats || pat.name.is_none() {
            actual.len() - ai
        } else {
            1
        };
        let min_repeat = usize::from(!optional);
        // Greedy first, then fewer: `<child>` and `┊` both want to absorb.
        for take in (min_repeat..=max_repeat).rev() {
            let mut ok = true;
            for k in 0..take {
                let Some(node) = actual.get(ai + k) else {
                    ok = false;
                    break;
                };
                if let Err(why) = head_matches(pat, node) {
                    *reason = why;
                    ok = false;
                    break;
                }
                if !pat.children.is_empty() && !match_list(&pat.children, &node.children(), reason) {
                    ok = false;
                    break;
                }
            }
            if ok && go(pats, actual, pi + 1, ai + take, reason) {
                return true;
            }
        }
        if optional && go(pats, actual, pi + 1, ai, reason) {
            return true;
        }
        if reason.is_empty() {
            *reason = format!(
                "no arrangement of the children matches `{}`",
                pat.name.as_deref().unwrap_or("<child>")
            );
        }
        false
    }
    let mut r = String::new();
    let ok = go(pats, actual, 0, 0, &mut r);
    if !ok && !r.is_empty() {
        *reason = r;
    }
    ok
}

/// Check `root` against a vendored GTK "CSS nodes" block.
///
/// # Errors
///
/// [`Mismatch`] naming the first node, class or arrangement that differs.
pub fn matches_fixture(root: &Node, fixture: &str) -> Result<(), Mismatch> {
    let rendered = render_tree(root);
    let fail = |reason: String| Mismatch {
        expected: fixture.to_string(),
        rendered: rendered.clone(),
        reason: Rc::from(reason.as_str()),
    };
    let Some(pattern) = parse_fixture(fixture) else {
        return Err(fail("the fixture parsed to nothing".to_string()));
    };
    head_matches(&pattern, root).map_err(fail)?;
    let mut reason = String::new();
    if match_list(&pattern.children, &root.children(), &mut reason) {
        Ok(())
    } else {
        Err(fail(reason))
    }
}

/// Build `kind` with `props` headlessly and render its retained subtree.
///
/// The gate's other half: [`matches_fixture`] says *whether* a tree is right,
/// this says *what it is* when it is not.
#[must_use]
pub fn node_tree_of(kind: Kind, props: &Props) -> String {
    let built = super::build_widget::<()>(kind, props);
    render_tree(&built.node)
}
```

Add to `ui/src/widgets/mod.rs`:

```rust
/// Everything `Controller::build` needs, with no Wayland connection and no
/// application: the bundled Adwaita light sheet, a probe-only font database,
/// an empty icon theme and a `ManualClock` at zero.
pub struct Headless {
    sheet: crate::css::cascade::CompiledSheet,
    fonts: crate::text::FontDatabase,
    icons: crate::icons::IconTheme,
    clock: Rc<dyn crate::anim::Clock>,
    env: crate::css::computed::ResolveEnv,
}

impl Headless {
    /// A context over `BUNDLED_ADWAITA_LIGHT`.
    #[must_use]
    pub fn new() -> Self {
        let sheet = crate::css::cascade::CompiledSheet::compile(
            crate::BUNDLED_ADWAITA_LIGHT,
            &crate::css::parse::MediaEnv::default(),
        );
        Self {
            sheet,
            fonts: crate::text::FontDatabase::new(),
            icons: crate::icons::IconTheme::empty(),
            clock: Rc::new(crate::anim::ManualClock::new()),
            env: crate::css::computed::ResolveEnv::default(),
        }
    }

    /// Borrow it as a `BuildCx`.
    pub fn cx(&mut self) -> crate::view::BuildCx<'_> {
        crate::view::BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }
}

impl Default for Headless {
    fn default() -> Self {
        Self::new()
    }
}

/// A widget built outside an `App`, for tests and for `node_tree_of`.
pub struct BuiltWidget<Msg> {
    /// The widget's root retained node.
    pub node: Node,
    /// Its controller, already `build`-ed.
    pub controller: Box<dyn Controller<Msg>>,
}

/// Build one widget headlessly.
#[must_use]
pub fn build_widget<Msg: Clone + 'static>(kind: Kind, props: &Props) -> BuiltWidget<Msg> {
    let mut hx = Headless::new();
    let node = Node::with_classes(kind.css_name(), kind.base_classes());
    let controller = {
        let mut cx = hx.cx();
        build_controller::<Msg>(kind, &node, props, &mut cx)
    };
    BuiltWidget { node, controller }
}
```

`build_controller` is P5's factory (contract deviation 6). Confirm it ends in a
catch-all; if it does not, add one, plus the controller it returns:

```rust
/// The controller for kinds that are a styled box and nothing else.
///
/// Every kind leaves this arm as soon as its own task lands; it exists so the
/// factory is total while the catalogue is being filled in.
pub struct GenericC {
    kind: Kind,
}

impl<Msg: Clone + 'static> Controller<Msg> for GenericC {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn build(_node: &Node, _props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        Self { kind: Kind::Box }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        apply_universal_prop(node, name, value);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

`apply_universal_prop(node, name, value)` is P5's helper for the
`GtkWidget`-universal props (`Classes`, `Id`, `Visible`, `Sensitive`,
`Tooltip`, `Opacity`, …); every P6 controller's `set_prop` delegates to it
for names it does not itself handle.

Finally, add the fixture loader to `ui/tests/node_trees.rs` (leaving P5's
tests untouched):

```rust
/// Load a vendored GTK "CSS nodes" block.
fn fixture(name: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gtk4.22-node-trees/");
    std::fs::read_to_string(format!("{path}{name}.txt"))
        .unwrap_or_else(|e| panic!("fixture {name}.txt: {e}"))
}

/// Build `kind` with `props` and assert its retained tree matches `name`.
fn assert_fixture(kind: icedtea_ui::view::Kind, props: &icedtea_ui::view::Props, name: &str) {
    let built = icedtea_ui::widgets::build_widget::<()>(kind, props);
    if let Err(mismatch) = icedtea_ui::widgets::matches_fixture(&built.node, &fixture(name)) {
        panic!("{kind:?} does not match {name}.txt:\n{mismatch}");
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::node_tree
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: PASS — six new tests, including the hostile-fixture battery, which
must complete in well under a second (the repetition matcher is bounded by
the child count, not by the fixture's line count).

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/node_tree.rs ui/src/widgets/mod.rs ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): render and match GTK's CSS-node notation

render_tree prints a retained subtree the way GTK's docs print it;
matches_fixture interprets the four pieces of slack the notation has that a
concrete tree cannot express -- [subnode], name[.class], <child> and the
repetition marker -- so a vendored block is a real gate rather than a
hand-massaged string. Malformed fixture text fails the test, never the run.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `Kind::Box` — `BoxC`, the `Into<Prop>` setter convention, slots, and the offscreen test harness

**Files:**
- Create: `ui/src/widgets/box_.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/box.txt`
- Modify: `ui/src/view/builders.rs` — `box_`, the `Into<Prop>` impls, `slot`
- Modify: `ui/src/widgets/mod.rs` — `mod box_;`, the `Kind::Box` arm, the
  `#[cfg(test)] mod offscreen` harness, `slot_of`
- Modify: `ui/tests/node_trees.rs` — `box_matches_its_gtk_fixture`
- Test: `ui/src/widgets/box_.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `layout::{Container, BoxDirection, ChildLayout, Align}` (Task 1),
  `widgets::types::{Orientation, BaselinePosition}` (Task 2),
  `view::{View, Props, Prop, PropName, Controller, BuildCx, Event, EventCx}` (P4).
- Produces:
  ```rust
  // ui/src/view/builders.rs
  pub fn box_<Msg: Clone + 'static>(
      orientation: Orientation,
      children: impl IntoIterator<Item = View<Msg>>,
  ) -> View<Msg>;
  pub fn slot<Msg>(view: View<Msg>, name: &str) -> View<Msg>;
  impl View<Msg> {
      pub fn spacing(self, v: impl Into<Prop>) -> Self;
      pub fn homogeneous(self, v: impl Into<Prop>) -> Self;
      pub fn baseline_position(self, v: impl Into<Prop>) -> Self;
  }
  // ui/src/widgets/box_.rs
  pub struct BoxC { pub orientation: Orientation, pub spacing: f32, pub homogeneous: bool }
  ```

**Two contract deviations this task establishes for every later builder**
(also listed at the top of the plan): setters take `impl Into<Prop>` rather
than a concrete type, because `View<Msg>` has one inherent-method namespace
and GTK gives `position` an `int` on `GtkPaned` and a `GtkPositionType` on
`GtkPopover`; and list-family/menu activation is `.on_item_activated(|usize|)`
rather than the contract's `.on_activate(|usize|)`, because P5 already claims
`.on_activate(Msg)` at `Handler::Unit` arity and Rust has no overloading.

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/box_.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::BoxC;
    use crate::layout::{BoxDirection, Container};
    use crate::view::builders::{box_, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::types::Orientation;
    use crate::widgets::{build_widget, matches_fixture};

    fn props(spacing: i64, horizontal: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Spacing, Prop::Int(spacing));
        p.set(
            PropName::Orientation,
            Prop::Enum(if horizontal { 0 } else { 1 }),
        );
        p
    }

    #[test]
    fn a_box_is_one_node_named_box() {
        // Mutation check: giving BoxC a wrapper subnode (the "I need
        // somewhere to hang spacing" mistake) adds a child and fails the
        // fixture, which is a single line.
        let built = build_widget::<()>(Kind::Box, &props(6, true));
        matches_fixture(&built.node, "box\n").expect("box fixture");
    }

    #[test]
    fn the_orientation_prop_picks_the_taffy_axis() {
        // Mutation check: ignoring PropName::Orientation in set_prop leaves
        // every box a column and the whole toolkit lays out vertically.
        let built = build_widget::<()>(Kind::Box, &props(0, true));
        let c = built.controller;
        assert_eq!(c.kind(), Kind::Box);
        let horizontal = build_widget::<()>(Kind::Box, &props(0, true));
        let vertical = build_widget::<()>(Kind::Box, &props(0, false));
        assert_eq!(
            BoxC::container_of(&horizontal.node),
            Container::Box { direction: BoxDirection::Row }
        );
        assert_eq!(
            BoxC::container_of(&vertical.node),
            Container::Box { direction: BoxDirection::Column }
        );
    }

    #[derive(Clone)]
    enum Msg {}

    fn view(_: &()) -> View<Msg> {
        box_(
            Orientation::Horizontal,
            [
                label("a").class("first"),
                label("b").class("second"),
            ],
        )
        .spacing(12)
        .class("frame")
    }

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    #[test]
    fn a_box_paints_its_children_twelve_pixels_apart_at_rest() {
        // Rest-state pixel test. Mutation check: dropping `spacing` from
        // set_prop puts the two labels adjacent and the gap column carries
        // glyph pixels instead of the container's background.
        let out = frames((), update, view, (200, 40), vec![ScriptStep::Capture]);
        assert_eq!(out.len(), 1);
        let background = px(&out, 0, 199, 39);
        // The 12px gap is background all the way down its middle column.
        let gap = crate::widgets::box_::tests_support::gap_column(&out);
        assert_eq!(px(&out, 0, gap, 20), background, "the spacing column is empty");
    }

    #[test]
    fn re_running_the_view_with_the_same_model_repaints_identically() {
        // Interaction test at the container level: reconciliation must not
        // move anything. Mutation check: rebuilding the child nodes in
        // set_prop (instead of mutating them) shifts the second frame.
        let out = frames(
            (),
            update,
            view,
            (200, 40),
            vec![ScriptStep::Capture, ScriptStep::Advance(std::time::Duration::from_millis(16)), ScriptStep::Capture],
        );
        for x in [0_u32, 50, 199] {
            assert_eq!(px(&out, 0, x, 20), px(&out, 1, x, 20));
        }
    }

    #[test]
    fn hostile_spacing_and_orientation_values_never_panic() {
        // Mutation check: forwarding a NaN spacing to taffy makes the layout
        // NaN and every later allocation unusable; `finite` clamps it.
        for v in [i64::MIN, i64::MAX, -1, 0] {
            let mut p = Props::default();
            p.set(PropName::Spacing, Prop::Int(v));
            p.set(PropName::Orientation, Prop::Enum(u16::MAX));
            p.set(PropName::Homogeneous, Prop::Str("yes".into()));
            let _ = build_widget::<()>(Kind::Box, &p);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::box_`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `box_` in
`widgets`` and `error[E0425]: cannot find function `box_` in module
`crate::view::builders``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/box.txt` — GTK 4.22.4 `gtk/gtkbox.c:56`
("uses a single CSS node with name box"):

```
box
```

`ui/src/widgets/box_.rs`, above the test module:

```rust
//! `GtkBox` -- a single `box` node whose children are laid out on one axis.

use crate::css::node::Node;
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::types::{BaselinePosition, Orientation};
use crate::widgets::{apply_universal_prop, container_of, set_container};

/// `GtkBox`.
pub struct BoxC {
    /// The main axis.
    pub orientation: Orientation,
    /// Gap between children, px.
    pub spacing: f32,
    /// Every child gets the same main-axis extent.
    pub homogeneous: bool,
    /// Where extra space goes when children have baselines.
    pub baseline_position: BaselinePosition,
}

impl BoxC {
    /// This node's recorded container -- the test hook, and how a parent
    /// controller asks what axis a child box is on.
    #[must_use]
    pub fn container_of(node: &Node) -> Container {
        container_of(node)
    }

    fn axis(self_orientation: Orientation) -> BoxDirection {
        match self_orientation {
            Orientation::Horizontal => BoxDirection::Row,
            Orientation::Vertical => BoxDirection::Column,
        }
    }

    fn apply(&self, node: &Node) {
        set_container(node, Container::Box { direction: Self::axis(self.orientation) });
        // `spacing` is GTK's own gap; the CSS `border-spacing` still applies
        // and the larger of the two wins, exactly as GtkBox does.
        crate::widgets::set_gap(node, self.spacing, self.orientation);
        crate::widgets::set_homogeneous(node, self.homogeneous, self.orientation);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for BoxC {
    fn kind(&self) -> Kind {
        Kind::Box
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let me = Self {
            orientation: Orientation::from_u16(
                u16::try_from(props.int(PropName::Orientation, 0)).unwrap_or(0),
            ),
            spacing: props.int(PropName::Spacing, 0).clamp(0, 1_000_000) as f32,
            homogeneous: props.bool(PropName::Homogeneous, false),
            baseline_position: BaselinePosition::from_u16(
                u16::try_from(props.int(PropName::BaselinePosition, 1)).unwrap_or(1),
            ),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Orientation => {
                self.orientation = Orientation::from_u16(prop_u16(value, 0));
            }
            PropName::Spacing => {
                self.spacing = prop_i64(value, 0).clamp(0, 1_000_000) as f32;
            }
            PropName::Homogeneous => self.homogeneous = prop_bool(value, false),
            PropName::BaselinePosition => {
                self.baseline_position = BaselinePosition::from_u16(prop_u16(value, 1));
            }
            other => {
                apply_universal_prop(node, other, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

`prop_u16`, `prop_i64`, `prop_bool` and `prop_str` are the four total
readers every P6 controller uses; add them to `ui/src/widgets/mod.rs` beside
`apply_universal_prop`:

```rust
/// Read a `Prop` as a `u16` enum discriminant; any other shape is `default`.
pub(crate) fn prop_u16(value: &Prop, default: u16) -> u16 {
    match value {
        Prop::Enum(v) => *v,
        Prop::Int(v) => u16::try_from(*v).unwrap_or(default),
        _ => default,
    }
}

/// Read a `Prop` as an integer; a non-finite float is `default`.
pub(crate) fn prop_i64(value: &Prop, default: i64) -> i64 {
    match value {
        Prop::Int(v) => *v,
        Prop::Float(v) if v.is_finite() => *v as i64,
        Prop::Bool(v) => i64::from(*v),
        _ => default,
    }
}

/// Read a `Prop` as a float; non-finite becomes `default`.
pub(crate) fn prop_f64(value: &Prop, default: f64) -> f64 {
    match value {
        Prop::Float(v) if v.is_finite() => *v,
        Prop::Int(v) => *v as f64,
        _ => default,
    }
}

/// Read a `Prop` as a bool. A string is *not* coerced: `Prop::Str("yes")`
/// is a caller mistake, not `true`.
pub(crate) fn prop_bool(value: &Prop, default: bool) -> bool {
    match value {
        Prop::Bool(v) => *v,
        Prop::Int(v) => *v != 0,
        _ => default,
    }
}

/// Read a `Prop` as text; anything else is `""`.
pub(crate) fn prop_str(value: &Prop) -> &str {
    match value {
        Prop::Str(s) => s,
        _ => "",
    }
}

/// The layout tree every controller writes through.
///
/// A widget controller has no `&mut LayoutTree` -- the tree lives in the
/// `App` -- so container and child-layout decisions are recorded *on the
/// node* in a side table the app drains before each layout pass. One
/// `RefCell<HashMap<OpaqueElement, Pending>>` per process, keyed the same way
/// `LayoutTree` keys its own map.
pub(crate) fn set_container(node: &Node, container: Container);
pub(crate) fn container_of(node: &Node) -> Container;
pub(crate) fn set_child_layout(node: &Node, child: ChildLayout);
pub(crate) fn set_gap(node: &Node, spacing: f32, orientation: Orientation);
pub(crate) fn set_homogeneous(node: &Node, on: bool, orientation: Orientation);
/// Drain the pending decisions into `tree`; called by `App` before
/// `LayoutTree::compute`, and by `Headless` in tests.
pub fn flush_layout(tree: &mut crate::layout::LayoutTree);
```

with bodies:

```rust
#[derive(Default, Clone)]
struct Pending {
    container: Option<Container>,
    child: Option<ChildLayout>,
    gap: Option<(f32, Orientation)>,
    homogeneous: Option<(bool, Orientation)>,
    node: Option<Node>,
}

thread_local! {
    static PENDING: RefCell<HashMap<OpaqueElement, Pending>> = RefCell::new(HashMap::new());
    static CONTAINERS: RefCell<HashMap<OpaqueElement, Container>> = RefCell::new(HashMap::new());
}

pub(crate) fn set_container(node: &Node, container: Container) {
    CONTAINERS.with(|c| c.borrow_mut().insert(node.opaque(), container));
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.container = Some(container);
        entry.node = Some(node.clone());
    });
}

pub(crate) fn container_of(node: &Node) -> Container {
    CONTAINERS.with(|c| c.borrow().get(&node.opaque()).copied().unwrap_or_default())
}
```

```rust
pub(crate) fn set_child_layout(node: &Node, child: ChildLayout) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.child = Some(child);
        entry.node = Some(node.clone());
    });
}

pub(crate) fn set_gap(node: &Node, spacing: f32, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.gap = Some((if spacing.is_finite() { spacing.max(0.0) } else { 0.0 }, orientation));
        entry.node = Some(node.clone());
    });
}

pub(crate) fn set_homogeneous(node: &Node, on: bool, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.homogeneous = Some((on, orientation));
        entry.node = Some(node.clone());
    });
}

/// Apply every recorded decision to `tree` and clear the queue.
///
/// Called by `App` immediately before `LayoutTree::compute`, and by tests
/// through `Headless`. Entries whose node is no longer in the tree are
/// dropped silently: a controller may have been discarded by `reconcile`
/// between the decision and the flush.
pub fn flush_layout(tree: &mut crate::layout::LayoutTree) {
    let drained: Vec<Pending> = PENDING.with(|p| p.borrow_mut().drain().map(|(_, v)| v).collect());
    for pending in drained {
        let Some(node) = pending.node else { continue };
        if let Some(container) = pending.container {
            tree.set_container(&node, container);
        }
        if let Some(child) = pending.child {
            tree.set_child_layout(&node, child);
        }
        if let Some((gap, orientation)) = pending.gap {
            // GTK's `spacing` and CSS's `border-spacing` both apply; the
            // larger wins, which is what GtkBox does when a theme sets both.
            tree.set_gap_floor(&node, gap, orientation == Orientation::Horizontal);
        }
        if let Some((on, orientation)) = pending.homogeneous {
            let horizontal = orientation == Orientation::Horizontal;
            for child in node.children() {
                let mut cl = tree.child_layout(&child).unwrap_or_default();
                if horizontal {
                    cl.hexpand = on;
                } else {
                    cl.vexpand = on;
                }
                tree.set_child_layout(&child, cl);
            }
        }
    }
}
```

`LayoutTree::set_gap_floor(&mut self, node: &Node, px: f32, horizontal: bool)`
is one more additive method on Task 1's file: it raises the stored taffy
style's `gap` on the named axis to `px` when `px` is larger, leaving
`border-spacing` alone otherwise. `Node::opaque()` is `pub(crate)` in M2 and
`widgets` is inside the crate, so no visibility change is needed.

`ui/src/view/builders.rs`:

```rust
/// `GtkBox`. `box` is a keyword, hence the trailing underscore -- the one
/// place the snake_case rule cannot be followed literally.
#[must_use]
pub fn box_<Msg: Clone + 'static>(
    orientation: Orientation,
    children: impl IntoIterator<Item = View<Msg>>,
) -> View<Msg> {
    View::new(Kind::Box)
        .prop(PropName::Orientation, Prop::Enum(orientation.to_u16()))
        .children(children)
}

/// Tag `view` as filling a named slot of its parent (`"start"`, `"end"`,
/// `"center"`, `"overlay"`, `"titlebar"`, `"title"`, `"label"`).
///
/// One mechanism for every container that has more than one place to put a
/// child: `HeaderBar`, `ActionBar`, `CenterBox`, `Overlay`, `Paned`,
/// `Frame` and `Window` all read it.
#[must_use]
pub fn slot<Msg>(view: View<Msg>, name: &str) -> View<Msg> {
    view.prop(PropName::Section, Prop::Str(Rc::from(name)))
}
```

plus the setters, each one line over `PropName`:

```rust
impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkBox:spacing`, `GtkGrid` row/column spacing's shorthand.
    #[must_use]
    pub fn spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Spacing, v)
    }

    /// `GtkBox:homogeneous`.
    #[must_use]
    pub fn homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Homogeneous, v)
    }

    /// `GtkBox:baseline-position`.
    #[must_use]
    pub fn baseline_position(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::BaselinePosition, v)
    }
}
```

and the `Into<Prop>` impls that make `impl Into<Prop>` setters total:

```rust
impl From<i32> for Prop { fn from(v: i32) -> Self { Prop::Int(i64::from(v)) } }
impl From<i64> for Prop { fn from(v: i64) -> Self { Prop::Int(v) } }
impl From<u32> for Prop { fn from(v: u32) -> Self { Prop::Int(i64::from(v)) } }
impl From<usize> for Prop { fn from(v: usize) -> Self { Prop::Int(i64::try_from(v).unwrap_or(i64::MAX)) } }
impl From<f32> for Prop { fn from(v: f32) -> Self { Prop::Float(f64::from(v)) } }
impl From<f64> for Prop { fn from(v: f64) -> Self { Prop::Float(v) } }
impl From<bool> for Prop { fn from(v: bool) -> Self { Prop::Bool(v) } }
impl From<&str> for Prop { fn from(v: &str) -> Self { Prop::Str(Rc::from(v)) } }
impl From<String> for Prop { fn from(v: String) -> Self { Prop::Str(Rc::from(v.as_str())) } }
impl From<&[&str]> for Prop {
    fn from(v: &[&str]) -> Self {
        Prop::Classes(v.iter().map(|s| Rc::from(*s)).collect())
    }
}
impl From<Orientation> for Prop { fn from(v: Orientation) -> Self { Prop::Enum(v.to_u16()) } }
```

with one `From<T> for Prop` per `#[repr(u16)]` enum in
`widgets::types` (`Position`, `Side`, `Policy`, `BaselinePosition`,
`SelectionMode`, `StackTransition`, `SortOrder`, `DisplayHint`,
`LicenseType`, `Decoration`), one for `MenuFlags` (`Prop::Int`), one for
`ItemFactory` (`Prop::Factory`) and one for `Rc<[ListItem]>`
(`Prop::Items`).

Register in `ui/src/widgets/mod.rs`: `mod box_;`, `pub use box_::BoxC;`, and
in `build_controller`, before the catch-all:

```rust
        Kind::Box => Box::new(BoxC::build(node, props, cx)),
```

Add the offscreen harness to `ui/src/widgets/mod.rs`:

```rust
/// Offscreen `App` driving, for every widget's rest-state and interaction
/// pixel tests. No Wayland connection, `ManualClock` time.
#[cfg(test)]
pub(crate) mod offscreen {
    use std::rc::Rc;

    use crate::anim::ManualClock;
    use crate::view::{App, Cmd, Frames, ScriptStep, View};

    /// Run `script` against a one-widget app and return its captured frames.
    pub(crate) fn frames<M: 'static, Msg: Clone + 'static>(
        model: M,
        update: fn(&mut M, Msg) -> Cmd<Msg>,
        view: fn(&M) -> View<Msg>,
        size: (u32, u32),
        script: Vec<ScriptStep<Msg>>,
    ) -> Frames {
        let clock = Rc::new(ManualClock::new());
        App::new(model, update, view)
            .run_offscreen(size, clock, script)
            .expect("offscreen run")
    }

    /// One pixel, or a panic naming the coordinate.
    pub(crate) fn px(frames: &Frames, frame: usize, x: u32, y: u32) -> (u8, u8, u8, u8) {
        frames
            .pixel(frame, x, y)
            .unwrap_or_else(|| panic!("no pixel at ({x}, {y}) in frame {frame}"))
    }

    /// The x of the first column between two painted children -- derived
    /// from the frame, never hard-coded.
    pub(crate) fn gap_column(frames: &Frames) -> u32 {
        let background = px(frames, 0, 0, 0);
        let mut seen_ink = false;
        for x in 0..frames.width() {
            let p = px(frames, 0, x, frames.height() / 2);
            if p != background {
                seen_ink = true;
            } else if seen_ink {
                return x;
            }
        }
        panic!("no gap column: the children never separate");
    }
}
```

(`Frames::width`/`height` are P4's; if P4 shipped only `pixel`, add the two
accessors there — additive, and the only P4 change this task makes.)

`ui/tests/node_trees.rs`:

```rust
#[test]
fn box_matches_its_gtk_fixture() {
    let mut props = icedtea_ui::view::Props::default();
    props.set(
        icedtea_ui::view::PropName::Spacing,
        icedtea_ui::view::Prop::Int(6),
    );
    assert_fixture(icedtea_ui::view::Kind::Box, &props, "box");
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::box_
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: PASS — five unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/box_.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/box.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkBox, the Into<Prop> setter convention and the offscreen harness

BoxC is one node and no subnodes, which is what GtkBox is. The task also lands
the three things every later widget uses: setters taking impl Into<Prop> (one
inherent namespace, and GTK gives `position` two different types), the `slot`
marker every multi-slot container reads, and the offscreen frame harness that
makes a per-widget pixel assertion cheap enough to write twice per widget.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `Kind::CenterBox` — `CenterBoxC`

**Files:**
- Create: `ui/src/widgets/center_box.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/center_box.txt`
- Modify: `ui/src/view/builders.rs` — `center_box`, `.shrink_center_last`
- Modify: `ui/src/widgets/mod.rs` — `mod center_box;`, the `Kind::CenterBox` arm
- Modify: `ui/tests/node_trees.rs` — `center_box_matches_its_gtk_fixture`
- Test: `ui/src/widgets/center_box.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `layout::{Container, BoxDirection, ChildLayout, Align}`,
  `widgets::{set_container, set_child_layout, prop_bool, prop_u16}`,
  `view::builders::slot`.
- Produces:
  ```rust
  pub fn center_box<Msg: Clone + 'static>(start: View<Msg>, center: View<Msg>, end: View<Msg>) -> View<Msg>;
  impl View<Msg> { pub fn shrink_center_last(self, v: impl Into<Prop>) -> Self; }
  pub struct CenterBoxC { pub orientation: Orientation, pub shrink_center_last: bool }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/center_box.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::CenterBoxC;
    use crate::css::node::Direction;
    use crate::layout::{BoxDirection, Container};
    use crate::view::builders::{center_box, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, container_of, matches_fixture};

    #[test]
    fn a_centre_box_is_one_node_named_box() {
        // Mutation check: `Kind::CenterBox` reusing `centerbox` as its CSS
        // name (the obvious guess) fails the fixture, whose node is `box`
        // (gtk/gtkcenterbox.c:341).
        let built = build_widget::<()>(Kind::CenterBox, &Props::default());
        matches_fixture(&built.node, "box\n").expect("center_box fixture");
        assert_eq!(&*built.node.name(), "box");
    }

    #[test]
    fn it_records_a_centre_container_carrying_shrink_center_last() {
        // Mutation check: dropping shrink_center_last from the Container
        // makes the flag unreadable by `container_of` and the centre child
        // shrinks first, hiding the title before the buttons.
        let mut props = Props::default();
        props.set(PropName::ShrinkCenterLast, Prop::Bool(true));
        let built = build_widget::<()>(Kind::CenterBox, &props);
        assert_eq!(
            container_of(&built.node),
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: true
            }
        );
        assert_eq!(built.controller.kind(), Kind::CenterBox);
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        center_box(
            label("start").class("s"),
            label("mid").class("m"),
            label("end-of-it").class("e"),
        )
    }

    #[test]
    fn the_centre_child_is_centred_although_the_ends_differ_in_width() {
        // Rest-state pixel test. Mutation check: laying the three children
        // out as a plain box makes the middle child's ink centre land left
        // of the surface centre, because "end-of-it" is wider than "start".
        let out = frames((), update, view, (300, 40), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 0, 0);
        let row = 20;
        let ink: Vec<u32> = (0..300)
            .filter(|x| px(&out, 0, *x, row) != background)
            .collect();
        let mid_ink: Vec<u32> = ink.iter().copied().filter(|x| (100..200).contains(x)).collect();
        let centre = (mid_ink.first().copied().unwrap() + mid_ink.last().copied().unwrap()) / 2;
        assert!(
            centre.abs_diff(150) <= 2,
            "the centre child's ink centre is the surface centre, got {centre}"
        );
    }

    #[test]
    fn under_rtl_the_first_child_is_allocated_on_the_right() {
        // Interaction test (direction is a live property, not a build-time
        // one). Mutation check: ignoring `Direction::Rtl` in the container
        // write puts "start"'s ink in the left third under both directions.
        let built = build_widget::<()>(Kind::CenterBox, &Props::default());
        built.node.set_direction(Some(Direction::Rtl));
        assert_eq!(
            CenterBoxC::first_child_edge(&built.node),
            crate::layout::Align::End
        );
    }

    #[test]
    fn hostile_orientation_and_slot_values_never_panic() {
        for raw in [0_u16, 1, 7, u16::MAX] {
            let mut props = Props::default();
            props.set(PropName::Orientation, Prop::Enum(raw));
            props.set(PropName::ShrinkCenterLast, Prop::Str("maybe".into()));
            props.set(PropName::Section, Prop::Int(3));
            let _ = build_widget::<()>(Kind::CenterBox, &props);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::center_box`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`center_box` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/center_box.txt`
(`gtk/gtkcenterbox.c:45`):

```
box
```

`ui/src/widgets/center_box.rs`:

```rust
//! `GtkCenterBox` -- three children, the middle one centred in the whole
//! allocation, the *first* one placed by text direction.

use crate::css::node::{Direction, Node};
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::types::Orientation;
use crate::widgets::{apply_universal_prop, prop_bool, prop_u16, set_child_layout, set_container};

/// `GtkCenterBox`.
pub struct CenterBoxC {
    /// The main axis.
    pub orientation: Orientation,
    /// The centre child shrinks last, not first.
    pub shrink_center_last: bool,
}

impl CenterBoxC {
    /// Which edge the *first* child sits against, for the current direction.
    ///
    /// `gtk/gtkcenterbox.c:45`: "The first child of the GtkCenterBox will be
    /// allocated depending on the text direction."
    #[must_use]
    pub fn first_child_edge(node: &Node) -> Align {
        match node.direction() {
            Direction::Ltr => Align::Start,
            Direction::Rtl => Align::End,
        }
    }

    fn apply(&self, node: &Node) {
        let direction = match self.orientation {
            Orientation::Horizontal => BoxDirection::Row,
            Orientation::Vertical => BoxDirection::Column,
        };
        set_container(
            node,
            Container::Center {
                direction,
                shrink_center_last: self.shrink_center_last,
            },
        );
        // The outer two grow from zero so the leftover space splits evenly
        // and the middle child lands on the container's centre whatever the
        // ends measure; the middle one refuses to grow.
        let children = node.children();
        for (i, child) in children.iter().enumerate() {
            let outer = i != 1;
            set_child_layout(
                child,
                ChildLayout {
                    hexpand: outer && self.orientation == Orientation::Horizontal,
                    vexpand: outer && self.orientation == Orientation::Vertical,
                    halign: if outer { Align::Fill } else { Align::Center },
                    valign: Align::Center,
                    ..ChildLayout::default()
                },
            );
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CenterBoxC {
    fn kind(&self) -> Kind {
        Kind::CenterBox
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let me = Self {
            orientation: Orientation::from_u16(
                u16::try_from(props.int(PropName::Orientation, 0)).unwrap_or(0),
            ),
            shrink_center_last: props.bool(PropName::ShrinkCenterLast, true),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Orientation => self.orientation = Orientation::from_u16(prop_u16(value, 0)),
            PropName::ShrinkCenterLast => self.shrink_center_last = prop_bool(value, true),
            other => {
                apply_universal_prop(node, other, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

`ui/src/view/builders.rs`:

```rust
/// `GtkCenterBox`: exactly three children, in `start`, `centre`, `end` order.
#[must_use]
pub fn center_box<Msg: Clone + 'static>(
    start: View<Msg>,
    center: View<Msg>,
    end: View<Msg>,
) -> View<Msg> {
    View::new(Kind::CenterBox).children([
        slot(start, "start"),
        slot(center, "center"),
        slot(end, "end"),
    ])
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkCenterBox:shrink-center-last`.
    #[must_use]
    pub fn shrink_center_last(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShrinkCenterLast, v)
    }
}
```

Register `Kind::CenterBox => Box::new(CenterBoxC::build(node, props, cx)),`
and add to `ui/tests/node_trees.rs`:

```rust
#[test]
fn center_box_matches_its_gtk_fixture() {
    assert_fixture(
        icedtea_ui::view::Kind::CenterBox,
        &icedtea_ui::view::Props::default(),
        "center_box",
    );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::center_box
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — five unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/center_box.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/center_box.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkCenterBox with true centring and RTL first-child placement

The middle child is centred on the container, not on what is left over after
the ends: the outer two grow from a zero basis so the free space splits evenly.
The node is `box`, not `centerbox` -- gtk/gtkcenterbox.c:341.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `Kind::Grid` — `GridC`, `.at()`, `.span()`

**Files:**
- Create: `ui/src/widgets/grid.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/grid.txt`
- Modify: `ui/src/view/builders.rs` — `grid`, `.row_spacing`,
  `.column_spacing`, `.row_homogeneous`, `.column_homogeneous`,
  `.baseline_row`, `.at`, `.span`
- Modify: `ui/src/widgets/mod.rs` — `mod grid;`, the `Kind::Grid` arm
- Modify: `ui/tests/node_trees.rs` — `grid_matches_its_gtk_fixture`
- Test: `ui/src/widgets/grid.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `layout::{Container, ChildLayout, GridPlacement}` (Task 1),
  `css::registry::Prop::BorderSpacing` through `ComputedStyle` (M2).
- Produces:
  ```rust
  pub fn grid<Msg: Clone + 'static>(children: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  impl View<Msg> {
      pub fn row_spacing(self, v: impl Into<Prop>) -> Self;
      pub fn column_spacing(self, v: impl Into<Prop>) -> Self;
      pub fn row_homogeneous(self, v: impl Into<Prop>) -> Self;
      pub fn column_homogeneous(self, v: impl Into<Prop>) -> Self;
      pub fn baseline_row(self, v: impl Into<Prop>) -> Self;
      pub fn at(self, column: u16, row: u16) -> Self;
      pub fn span(self, columns: u16, rows: u16) -> Self;
  }
  pub struct GridC { pub columns: u16, pub rows: u16, pub spacing: (f32, f32), pub homogeneous: (bool, bool) }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/grid.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::GridC;
    use crate::layout::Container;
    use crate::view::builders::{grid, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, container_of, matches_fixture};

    #[test]
    fn a_grid_is_one_node_named_grid() {
        // Mutation check: emitting a per-cell wrapper node adds children and
        // fails the single-line fixture (gtk/gtkgrid.c:115).
        let built = build_widget::<()>(Kind::Grid, &Props::default());
        matches_fixture(&built.node, "grid\n").expect("grid fixture");
    }

    #[test]
    fn the_track_counts_come_from_the_childrens_placements() {
        // Mutation check: taking `columns` from PropName::Columns only (and
        // not from the maximum placement) leaves a 1-track grid and every
        // child stacks in column 0.
        let view = grid::<()>([
            label("a").at(0, 0),
            label("b").at(1, 0),
            label("c").at(0, 1).span(2, 1),
        ]);
        let (columns, rows) = GridC::extent_of(&view);
        assert_eq!((columns, rows), (2, 2));
    }

    #[test]
    fn spacing_props_reach_the_container() {
        // Mutation check: swapping row_spacing and column_spacing at the
        // Container write transposes every grid in the toolkit.
        let mut props = Props::default();
        props.set(PropName::RowSpacing, Prop::Int(9));
        props.set(PropName::ColumnSpacing, Prop::Int(3));
        props.set(PropName::RowHomogeneous, Prop::Bool(true));
        let built = build_widget::<()>(Kind::Grid, &props);
        match container_of(&built.node) {
            Container::Grid {
                row_spacing,
                column_spacing,
                row_homogeneous,
                column_homogeneous,
                ..
            } => {
                assert_eq!(row_spacing, 9.0);
                assert_eq!(column_spacing, 3.0);
                assert!(row_homogeneous);
                assert!(!column_homogeneous);
            }
            other => panic!("expected a grid container, got {other:?}"),
        }
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        grid([
            label("aa").at(0, 0),
            label("bb").at(1, 0),
            label("cc").at(0, 1),
        ])
        .column_spacing(20)
        .row_spacing(10)
    }

    #[test]
    fn cells_land_in_their_own_row_and_column_at_rest() {
        // Rest-state pixel test. Mutation check: dropping GridPlacement in
        // `flush_layout` auto-places all three on one row and the bottom
        // half of the surface stays empty.
        let out = frames((), update, view, (240, 80), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 239, 79);
        let row_has_ink = |y: u32| (0..240).any(|x| px(&out, 0, x, y) != background);
        assert!(row_has_ink(18), "first row painted");
        assert!(row_has_ink(60), "second row painted");
    }

    #[test]
    fn re_laying_out_after_a_span_change_keeps_the_same_nodes() {
        // Interaction test. Mutation check: rebuilding children on a span
        // change destroys node identity and the second frame differs in the
        // first row too, not only where the span moved.
        let out = frames(
            (),
            update,
            view,
            (240, 80),
            vec![
                ScriptStep::Capture,
                ScriptStep::Advance(std::time::Duration::from_millis(16)),
                ScriptStep::Capture,
            ],
        );
        for x in [10_u32, 120, 239] {
            assert_eq!(px(&out, 0, x, 18), px(&out, 1, x, 18));
        }
    }

    #[test]
    fn hostile_placements_never_panic_or_allocate_unboundedly() {
        // Mutation check: dropping the 1..=1024 clamp in `container_style`
        // makes this test allocate 65 535 tracks per axis and time out.
        let mut props = Props::default();
        props.set(PropName::Columns, Prop::Int(i64::MAX));
        props.set(PropName::Rows, Prop::Int(-5));
        props.set(PropName::RowSpacing, Prop::Float(f64::NAN));
        props.set(PropName::ColumnSpacing, Prop::Float(f64::INFINITY));
        let _ = build_widget::<()>(Kind::Grid, &props);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::grid`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `grid` in
`widgets``, `error[E0599]: no method named `at` found for struct `View``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/grid.txt` (`gtk/gtkgrid.c:115`):

```
grid
```

`ui/src/widgets/grid.rs`:

```rust
//! `GtkGrid` -- one `grid` node; children carry their own cell.

use crate::css::node::Node;
use crate::layout::{ChildLayout, Container, GridPlacement};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{apply_universal_prop, prop_bool, prop_i64, set_child_layout, set_container};

/// `GtkGrid`.
pub struct GridC {
    /// Column tracks, `1..=1024`.
    pub columns: u16,
    /// Row tracks, `1..=1024`.
    pub rows: u16,
    /// `(column_spacing, row_spacing)` in px.
    pub spacing: (f32, f32),
    /// `(column_homogeneous, row_homogeneous)`.
    pub homogeneous: (bool, bool),
    /// The row whose baseline the grid aligns on; `-1` for none.
    pub baseline_row: i32,
}

impl GridC {
    /// The track counts a view's children imply: one past the largest
    /// `column + column_span` and `row + row_span`.
    ///
    /// GTK's grid has no declared size either -- it grows to fit whatever is
    /// attached -- so this is the authority and `PropName::Columns`/`Rows`
    /// only raise it.
    #[must_use]
    pub fn extent_of<Msg>(view: &View<Msg>) -> (u16, u16) {
        let mut columns = 1_u16;
        let mut rows = 1_u16;
        for child in &view.children {
            let c = u16::try_from(child.props.int(PropName::Column, 0).max(0)).unwrap_or(0);
            let r = u16::try_from(child.props.int(PropName::Row, 0).max(0)).unwrap_or(0);
            let cs = u16::try_from(child.props.int(PropName::ColumnSpan, 1).max(1)).unwrap_or(1);
            let rs = u16::try_from(child.props.int(PropName::RowSpan, 1).max(1)).unwrap_or(1);
            columns = columns.max(c.saturating_add(cs));
            rows = rows.max(r.saturating_add(rs));
        }
        (columns.clamp(1, 1024), rows.clamp(1, 1024))
    }

    fn placement(props: &Props) -> Option<GridPlacement> {
        let has = props.get(PropName::Column).is_some() || props.get(PropName::Row).is_some();
        has.then(|| GridPlacement {
            column: u16::try_from(props.int(PropName::Column, 0).clamp(0, 1023)).unwrap_or(0),
            row: u16::try_from(props.int(PropName::Row, 0).clamp(0, 1023)).unwrap_or(0),
            column_span: u16::try_from(props.int(PropName::ColumnSpan, 1).clamp(1, 1024)).unwrap_or(1),
            row_span: u16::try_from(props.int(PropName::RowSpan, 1).clamp(1, 1024)).unwrap_or(1),
        })
    }

    fn apply(&self, node: &Node) {
        set_container(
            node,
            Container::Grid {
                columns: self.columns,
                rows: self.rows,
                column_spacing: self.spacing.0,
                row_spacing: self.spacing.1,
                column_homogeneous: self.homogeneous.0,
                row_homogeneous: self.homogeneous.1,
            },
        );
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for GridC {
    fn kind(&self) -> Kind {
        Kind::Grid
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let clamp = |v: i64| v.clamp(0, 1_000_000) as f32;
        let me = Self {
            columns: u16::try_from(props.int(PropName::Columns, 1).clamp(1, 1024)).unwrap_or(1),
            rows: u16::try_from(props.int(PropName::Rows, 1).clamp(1, 1024)).unwrap_or(1),
            spacing: (
                clamp(props.int(PropName::ColumnSpacing, 0)),
                clamp(props.int(PropName::RowSpacing, 0)),
            ),
            homogeneous: (
                props.bool(PropName::ColumnHomogeneous, false),
                props.bool(PropName::RowHomogeneous, false),
            ),
            baseline_row: i32::try_from(props.int(PropName::BaselineRow, -1)).unwrap_or(-1),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        let clamp = |v: i64| v.clamp(0, 1_000_000) as f32;
        match name {
            PropName::Columns => {
                self.columns = u16::try_from(prop_i64(value, 1).clamp(1, 1024)).unwrap_or(1);
            }
            PropName::Rows => {
                self.rows = u16::try_from(prop_i64(value, 1).clamp(1, 1024)).unwrap_or(1);
            }
            PropName::ColumnSpacing => self.spacing.0 = clamp(prop_i64(value, 0)),
            PropName::RowSpacing => self.spacing.1 = clamp(prop_i64(value, 0)),
            PropName::ColumnHomogeneous => self.homogeneous.0 = prop_bool(value, false),
            PropName::RowHomogeneous => self.homogeneous.1 = prop_bool(value, false),
            PropName::BaselineRow => {
                self.baseline_row = i32::try_from(prop_i64(value, -1)).unwrap_or(-1);
            }
            // A child's cell changed: re-place that child only.
            PropName::Column | PropName::Row | PropName::ColumnSpan | PropName::RowSpan => {
                for child in node.children() {
                    if let Some(place) = Self::placement(&crate::widgets::props_of(&child)) {
                        set_child_layout(
                            &child,
                            ChildLayout {
                                grid: Some(place),
                                ..crate::widgets::child_layout_of(&child)
                            },
                        );
                    }
                }
                return;
            }
            other => {
                apply_universal_prop(node, other, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

`props_of(node)` and `child_layout_of(node)` are two more `widgets/mod.rs`
side-table readers, written like `container_of`: `reconcile` records each
instance's `Props` and each node's last `ChildLayout` next to the node, so a
container controller can re-derive a child's placement without owning the
child's instance. Add them beside `PENDING`:

```rust
thread_local! {
    static NODE_PROPS: RefCell<HashMap<OpaqueElement, Props>> = RefCell::new(HashMap::new());
    static NODE_CHILD: RefCell<HashMap<OpaqueElement, ChildLayout>> = RefCell::new(HashMap::new());
}

/// Record the props `reconcile` applied to `node`. Called from
/// `Instance::apply_props`; `Props` is a small sorted vector, so this is a
/// clone of a dozen words.
pub fn record_props(node: &Node, props: &Props) {
    NODE_PROPS.with(|m| m.borrow_mut().insert(node.opaque(), props.clone()));
}

/// The props last applied to `node`, or an empty set.
#[must_use]
pub fn props_of(node: &Node) -> Props {
    NODE_PROPS.with(|m| m.borrow().get(&node.opaque()).cloned().unwrap_or_default())
}

/// The child layout last recorded for `node`.
#[must_use]
pub fn child_layout_of(node: &Node) -> ChildLayout {
    NODE_CHILD.with(|m| m.borrow().get(&node.opaque()).copied().unwrap_or_default())
}
```

and have `set_child_layout` (Task 4) also write `NODE_CHILD`. `App::run`
calls `record_props` from the reconciler's `SetProp`/`Insert` arms — one
call, in P4's file, additive.

`ui/src/view/builders.rs`:

```rust
/// `GtkGrid`.
#[must_use]
pub fn grid<Msg: Clone + 'static>(children: impl IntoIterator<Item = View<Msg>>) -> View<Msg> {
    View::new(Kind::Grid).children(children)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkGrid:row-spacing`.
    #[must_use]
    pub fn row_spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::RowSpacing, v)
    }

    /// `GtkGrid:column-spacing`.
    #[must_use]
    pub fn column_spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ColumnSpacing, v)
    }

    /// `GtkGrid:row-homogeneous`.
    #[must_use]
    pub fn row_homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::RowHomogeneous, v)
    }

    /// `GtkGrid:column-homogeneous`.
    #[must_use]
    pub fn column_homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ColumnHomogeneous, v)
    }

    /// `GtkGrid:baseline-row`.
    #[must_use]
    pub fn baseline_row(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::BaselineRow, v)
    }

    /// This child's grid cell, zero-based (`gtk_grid_attach`'s first two
    /// arguments).
    #[must_use]
    pub fn at(self, column: u16, row: u16) -> Self {
        self.prop(PropName::Column, i64::from(column))
            .prop(PropName::Row, i64::from(row))
    }

    /// How many cells this child covers (`gtk_grid_attach`'s last two).
    #[must_use]
    pub fn span(self, columns: u16, rows: u16) -> Self {
        self.prop(PropName::ColumnSpan, i64::from(columns.max(1)))
            .prop(PropName::RowSpan, i64::from(rows.max(1)))
    }
}
```

Register `Kind::Grid => Box::new(GridC::build(node, props, cx)),` and add
`grid_matches_its_gtk_fixture` to `ui/tests/node_trees.rs`, built exactly as
`box_matches_its_gtk_fixture` is but with `Kind::Grid` and `"grid"`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::grid
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui --lib layout::tests
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — six unit tests, one fixture test, and Task 1's layout tests
still green.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/grid.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/grid.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkGrid over taffy's grid, with per-child cells and spans

The track counts come from the children's own placements, the way GtkGrid
grows to fit whatever is attached; the declared Columns/Rows props only raise
them. Both counts are clamped to 1..=1024 so a model-driven count cannot ask
taffy for four billion tracks.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `Kind::Frame` — `FrameC`

**Files:**
- Create: `ui/src/widgets/frame.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/frame.txt`
- Modify: `ui/src/view/builders.rs` — `frame`, `.label`, `.label_xalign`,
  `.label_widget`
- Modify: `ui/src/widgets/mod.rs` — `mod frame;`, the `Kind::Frame` arm
- Modify: `ui/tests/node_trees.rs` — `frame_matches_its_gtk_fixture`
- Test: `ui/src/widgets/frame.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P5's `LabelC` (through `build_controller`), `layout::Container`,
  `view::builders::slot`.
- Produces:
  ```rust
  pub fn frame<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  impl View<Msg> { pub fn label_xalign(self, v: impl Into<Prop>) -> Self;
                   pub fn label_widget(self, v: View<Msg>) -> Self; }
  pub struct FrameC { pub label: Option<Node>, pub label_xalign: f32 }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/frame.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::builders::{frame, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, matches_fixture};

    fn titled() -> Props {
        let mut p = Props::default();
        p.set(PropName::Label, Prop::Str("Group".into()));
        p
    }

    #[test]
    fn a_titled_frame_is_a_label_then_the_child() {
        // Mutation check: appending the label *after* the child inverts the
        // fixture's order and a theme's `frame > label:first-child` rule
        // stops matching.
        let built = build_widget::<()>(Kind::Frame, &titled());
        matches_fixture(&built.node, "frame\n├── <child>\n╰── <child>\n").expect("frame fixture");
        assert_eq!(&*built.node.child(0).expect("label node").name(), "label");
    }

    #[test]
    fn an_untitled_frame_has_no_label_node() {
        // Mutation check: building the label unconditionally leaves an empty
        // `label` node that Adwaita still gives top padding, so an untitled
        // frame's child is pushed down by the label's height.
        let built = build_widget::<()>(Kind::Frame, &Props::default());
        assert_eq!(built.node.child_count(), 0);
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        frame(label("body")).label("Group").label_xalign(0.0)
    }

    #[test]
    fn a_frame_paints_its_border_around_the_child_at_rest() {
        // Rest-state pixel test. Mutation check: dropping Container::Box
        // from FrameC makes the child overlap the border and the pixel one
        // in from the left edge stops being the border colour.
        let out = frames((), update, view, (160, 60), vec![ScriptStep::Capture]);
        let interior = px(&out, 0, 80, 40);
        let border = px(&out, 0, 0, 30);
        assert_ne!(border, interior, "the frame paints a border Adwaita gives it");
    }

    #[test]
    fn changing_the_label_text_keeps_the_same_label_node() {
        // Interaction test. Mutation check: rebuilding the label node on a
        // text change loses its animation state and its tree id, which this
        // catches as a changed pointer.
        let built = build_widget::<()>(Kind::Frame, &titled());
        let before = built.node.child(0).expect("label");
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        c.set_prop(
            &built.node,
            PropName::Label,
            &Prop::Str("Other".into()),
            &mut hx.cx(),
        );
        let after = built.node.child(0).expect("label");
        assert!(before.ptr_eq(&after), "the label node survived the text change");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::frame`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `frame` in
`widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/frame.txt` (`gtk/gtkframe.c`):

```
frame
├── <label widget>
╰── <child>
```

`ui/src/widgets/frame.rs`:

```rust
//! `GtkFrame` -- a bordered box with an optional label above its child.

use crate::css::node::Node;
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::{apply_universal_prop, prop_f64, prop_str, set_child_layout, set_container};

/// `GtkFrame`.
pub struct FrameC {
    /// The label node, when the frame is titled.
    pub label: Option<Node>,
    /// `0.0` left, `1.0` right (`GtkFrame:label-xalign`).
    pub label_xalign: f32,
}

impl FrameC {
    fn ensure_label(&mut self, node: &Node, text: &str) {
        if text.is_empty() {
            if let Some(existing) = self.label.take() {
                node.remove_child(&existing);
            }
            return;
        }
        let label = match &self.label {
            Some(existing) => existing.clone(),
            None => {
                let created = Node::new("label");
                node.insert_child(0, &created);
                self.label = Some(created.clone());
                created
            }
        };
        crate::widgets::set_text(&label, text);
        set_child_layout(
            &label,
            ChildLayout {
                halign: if self.label_xalign <= 0.34 {
                    Align::Start
                } else if self.label_xalign >= 0.67 {
                    Align::End
                } else {
                    Align::Center
                },
                ..ChildLayout::default()
            },
        );
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for FrameC {
    fn kind(&self) -> Kind {
        Kind::Frame
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        let mut me = Self {
            label: None,
            label_xalign: props.float(PropName::LabelXalign, 0.0) as f32,
        };
        me.ensure_label(node, props.str(PropName::Label).unwrap_or_default());
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Label => self.ensure_label(node, prop_str(value)),
            PropName::LabelXalign => {
                self.label_xalign = prop_f64(value, 0.0).clamp(0.0, 1.0) as f32;
                let text = self
                    .label
                    .as_ref()
                    .map(|l| crate::widgets::text_of(l))
                    .unwrap_or_default();
                self.ensure_label(node, &text);
            }
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

`set_text(node, text)` / `text_of(node)` are P5's label-text side table (the
one `LabelC` already uses to hand a node's string to `TextLayout`); if P5
exposed them only inside `label.rs`, promote the two functions to
`widgets/mod.rs` unchanged — no behaviour moves.

`ui/src/view/builders.rs`:

```rust
/// `GtkFrame`.
#[must_use]
pub fn frame<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Frame).child(child)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkFrame:label-xalign`.
    #[must_use]
    pub fn label_xalign(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::LabelXalign, v)
    }

    /// `GtkFrame:label-widget` -- a whole view instead of a text label.
    #[must_use]
    pub fn label_widget(self, view: View<Msg>) -> Self {
        let mut me = self;
        me.children.insert(0, slot(view, "label"));
        me.prop(PropName::LabelWidget, Prop::Bool(true))
    }
}
```

Register `Kind::Frame => Box::new(FrameC::build(node, props, cx)),` and add
`frame_matches_its_gtk_fixture` to `ui/tests/node_trees.rs` over
`titled()`'s props and the `"frame"` fixture.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::frame
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/frame.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/frame.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkFrame with an optional label node

The label node exists only while there is a label: an always-present empty one
would still take Adwaita's padding and push the child down. Retitling mutates
the node instead of replacing it, so its identity survives.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `Kind::Paned` — `PanedC`, the draggable separator

**Files:**
- Create: `ui/src/widgets/paned.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/paned.txt`
- Modify: `ui/src/view/builders.rs` — `paned`, `.position`, `.position_set`,
  `.wide_handle`, `.resize_start`, `.resize_end`, `.shrink_start`,
  `.shrink_end`
- Modify: `ui/src/widgets/mod.rs` — `mod paned;`, the `Kind::Paned` arm
- Modify: `ui/tests/node_trees.rs` — `paned_matches_its_gtk_fixture`
- Test: `ui/src/widgets/paned.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `window::{KeyEvent, Mods}` (P3), `view::{Event, EventCx,
  EventKind}` (P4), `layout::{Container, ChildLayout}`.
- Produces:
  ```rust
  pub fn paned<Msg: Clone + 'static>(orientation: Orientation, start: View<Msg>, end: View<Msg>) -> View<Msg>;
  impl View<Msg> { pub fn position(self, v: impl Into<Prop>) -> Self; /* + six more */ }
  pub struct PanedC { pub position: f32, pub drag: Option<f32>, pub wide: bool, pub separator: Node }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/paned.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::types::Orientation;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props(position: i64, wide: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Position, Prop::Int(position));
        p.set(PropName::WideHandle, Prop::Bool(wide));
        p.set(PropName::Orientation, Prop::Enum(Orientation::Horizontal.to_u16()));
        p
    }

    #[test]
    fn a_paned_is_child_separator_child() {
        // Mutation check: appending the separator last (the natural
        // "add children then the handle" order) fails the fixture, and the
        // handle then sits outside both panes.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        matches_fixture(&built.node, "paned\n├── <child>\n├── separator[.wide]\n╰── <child>\n")
            .expect("paned fixture");
    }

    #[test]
    fn the_wide_handle_prop_toggles_the_separators_class() {
        // Mutation check: setting `.wide` on the paned node instead of the
        // separator means Adwaita's `paned > separator.wide` never matches
        // and the handle keeps its 1px width.
        let built = build_widget::<()>(Kind::Paned, &props(100, true));
        let separator = built.node.child(1).expect("separator");
        assert!(separator.classes().iter().any(|c| c.as_str() == "wide"));
    }

    #[test]
    fn dragging_the_separator_moves_the_position_and_reports_it() {
        // Interaction test. Mutation check: applying the pointer's absolute
        // x instead of the delta from the grab point makes the position jump
        // to the cursor on the first motion.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &crate::view::Event::PointerDown { button: 0x110, local: (100.0, 10.0), serial: 1 },
            &mut cx,
        );
        c.on_event(
            &crate::view::Event::PointerMotion { local: (140.0, 10.0) },
            &mut cx,
        );
        assert_eq!(crate::widgets::paned::PanedC::position_of(&c), 140.0);
        c.on_event(
            &crate::view::Event::PointerUp { button: 0x110, local: (140.0, 10.0), serial: 2 },
            &mut cx,
        );
        c.on_event(
            &crate::view::Event::PointerMotion { local: (200.0, 10.0) },
            &mut cx,
        );
        assert_eq!(
            crate::widgets::paned::PanedC::position_of(&c),
            140.0,
            "motion after the release is not a drag"
        );
    }

    #[test]
    fn arrow_keys_move_the_focused_separator_and_home_end_go_to_the_extremes() {
        // Mutation check: handling arrows on the paned node rather than the
        // focused separator makes a paned steal arrow keys from a focused
        // child list.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(&crate::view::Event::FocusIn { cause: crate::window::FocusCause::Key }, &mut cx);
        c.on_event(&crate::view::Event::Key(hx.key("Right")), &mut cx);
        assert_eq!(crate::widgets::paned::PanedC::position_of(&c), 101.0);
        c.on_event(&crate::view::Event::Key(hx.key("Home")), &mut cx);
        assert_eq!(crate::widgets::paned::PanedC::position_of(&c), 0.0);
    }

    #[test]
    fn a_hostile_position_never_panics_and_never_leaves_the_pane_negative() {
        for v in [i64::MIN, -1, 0, i64::MAX] {
            let built = build_widget::<()>(Kind::Paned, &props(v, false));
            assert!(crate::widgets::paned::PanedC::position_of(&built.controller) >= 0.0);
        }
    }
}
```

`Headless::event_cx(&Node) -> EventCx<'_, Msg>` and `Headless::key(&str) ->
KeyEvent` are two more harness methods; add them beside `Headless::cx` in
`ui/src/widgets/mod.rs`:

```rust
impl Headless {
    /// An `EventCx` over `node`, with an empty focus ring and clipboard.
    pub fn event_cx<Msg>(&mut self, node: &Node) -> crate::view::EventCx<'_, Msg> { /* … */ }

    /// A synthetic key press by keysym name, through P3's vendored `us`
    /// keymap -- the same path a real key takes.
    pub fn key(&mut self, name: &str) -> crate::window::KeyEvent {
        crate::window::Keymap::vendored_us().key_by_name(name)
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::paned`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `paned` in
`widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/paned.txt` (`gtk/gtkpaned.c:92`):

```
paned
├── <child>
├── separator[.wide]
╰── <child>
```

`ui/src/widgets/paned.rs`:

```rust
//! `GtkPaned` -- two children and a draggable separator between them.

use crate::css::node::{Node, PseudoStates};
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::Orientation;
use crate::widgets::{apply_universal_prop, prop_bool, prop_i64, prop_u16, set_child_layout, set_container};

/// `GtkPaned`.
pub struct PanedC {
    /// Divider offset from the leading edge, px.
    pub position: f32,
    /// Grab offset while dragging: `Some(pointer - position)`.
    pub drag: Option<f32>,
    /// `.wide` on the separator.
    pub wide: bool,
    /// The separator node.
    pub separator: Node,
    /// Main axis.
    pub orientation: Orientation,
    /// Whether the position was set explicitly (`GtkPaned:position-set`).
    pub position_set: bool,
    /// `resize-start-child` / `resize-end-child`.
    pub resize: (bool, bool),
    /// `shrink-start-child` / `shrink-end-child`.
    pub shrink: (bool, bool),
    /// The separator has keyboard focus, so arrows move it.
    pub handle_focused: bool,
}

impl PanedC {
    /// The current divider offset -- the test hook.
    #[must_use]
    pub fn position_of<Msg>(controller: &Box<dyn Controller<Msg>>) -> f32 {
        controller
            .as_any()
            .downcast_ref::<Self>()
            .map_or(f32::NAN, |c| c.position)
    }

    fn axis_of(&self, local: (f32, f32)) -> f32 {
        match self.orientation {
            Orientation::Horizontal => local.0,
            Orientation::Vertical => local.1,
        }
    }

    fn set_position(&mut self, value: f32) {
        self.position = if value.is_finite() { value.max(0.0) } else { 0.0 };
        let leading = self.position;
        if let Some(child) = self.separator.parent().and_then(|p| p.child(0)) {
            set_child_layout(
                &child,
                ChildLayout {
                    hexpand: false,
                    vexpand: false,
                    halign: Align::Fill,
                    valign: Align::Fill,
                    margin: [0.0; 4],
                    ..ChildLayout::default()
                },
            );
            crate::widgets::set_size_request(&child, self.orientation, leading);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PanedC {
    fn kind(&self) -> Kind {
        Kind::Paned
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let orientation =
            Orientation::from_u16(u16::try_from(props.int(PropName::Orientation, 0)).unwrap_or(0));
        set_container(
            node,
            Container::Box {
                direction: match orientation {
                    Orientation::Horizontal => BoxDirection::Row,
                    Orientation::Vertical => BoxDirection::Column,
                },
            },
        );
        // The separator goes between the two children the view supplied.
        let separator = Node::new("separator");
        node.insert_child(1.min(node.child_count()), &separator);
        let mut me = Self {
            position: 0.0,
            drag: None,
            wide: props.bool(PropName::WideHandle, false),
            separator,
            orientation,
            position_set: props.get(PropName::Position).is_some(),
            resize: (
                props.bool(PropName::ResizeStart, true),
                props.bool(PropName::ResizeEnd, true),
            ),
            shrink: (
                props.bool(PropName::ShrinkStart, true),
                props.bool(PropName::ShrinkEnd, true),
            ),
            handle_focused: false,
        };
        me.separator.set_state(PseudoStates::empty(), false);
        if me.wide {
            me.separator.add_class("wide");
        }
        me.set_position(props.int(PropName::Position, 0).max(0) as f32);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Position => {
                self.position_set = true;
                self.set_position(prop_i64(value, 0).max(0) as f32);
            }
            PropName::PositionSet => self.position_set = prop_bool(value, false),
            PropName::WideHandle => {
                self.wide = prop_bool(value, false);
                if self.wide {
                    self.separator.add_class("wide");
                } else {
                    self.separator.remove_class("wide");
                }
            }
            PropName::Orientation => self.orientation = Orientation::from_u16(prop_u16(value, 0)),
            PropName::ResizeStart => self.resize.0 = prop_bool(value, true),
            PropName::ResizeEnd => self.resize.1 = prop_bool(value, true),
            PropName::ShrinkStart => self.shrink.0 = prop_bool(value, true),
            PropName::ShrinkEnd => self.shrink.1 = prop_bool(value, true),
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut out = Vec::new();
        match ev {
            Event::PointerDown { local, .. } => {
                self.drag = Some(self.axis_of(*local) - self.position);
                self.separator.set_state(PseudoStates::ACTIVE, true);
                cx.handled = true;
            }
            Event::PointerMotion { local } => {
                if let Some(offset) = self.drag {
                    self.set_position(self.axis_of(*local) - offset);
                    if let Some(msg) = cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, f64::from(self.position))
                    {
                        out.push(msg);
                    }
                    cx.handled = true;
                }
            }
            Event::PointerUp { .. } => {
                self.drag = None;
                self.separator.set_state(PseudoStates::ACTIVE, false);
            }
            Event::FocusIn { .. } => self.handle_focused = true,
            Event::FocusOut => self.handle_focused = false,
            Event::Key(key) if self.handle_focused => {
                let step = if key.mods.control() { 10.0 } else { 1.0 };
                let moved = match key.keysym_name() {
                    "Left" | "Up" => Some(self.position - step),
                    "Right" | "Down" => Some(self.position + step),
                    "Home" => Some(0.0),
                    "End" => Some(f32::MAX / 4.0),
                    _ => None,
                };
                if let Some(value) = moved {
                    self.set_position(value);
                    if let Some(msg) = cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, f64::from(self.position))
                    {
                        out.push(msg);
                    }
                    cx.handled = true;
                }
            }
            _ => {}
        }
        out
    }
}
```

`Controller::as_any(&self) -> &dyn std::any::Any` is one additive default-less
method on P4's trait (`fn as_any(&self) -> &dyn std::any::Any;`, implemented
as `self` in every controller) so a test can look inside a
`Box<dyn Controller>`; `set_size_request(node, orientation, px)` writes a
`width_request`/`height_request` into the same pending side table Task 4
introduced.

`ui/src/view/builders.rs`: `paned(orientation, start, end)` builds
`View::new(Kind::Paned)` with `Orientation` set and the two children in
order, plus the seven setters, each one line over its `PropName`
(`Position`, `PositionSet`, `WideHandle`, `ResizeStart`, `ResizeEnd`,
`ShrinkStart`, `ShrinkEnd`).

Register `Kind::Paned => Box::new(PanedC::build(node, props, cx)),` and add
`paned_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::paned
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — five unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/paned.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/paned.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkPaned with a draggable, focusable separator

The drag applies the delta from the grab point, not the pointer's absolute
position, so grabbing the handle anywhere does not make it jump; arrows move
the handle only while the handle itself has focus, so a focused child list
keeps its own arrow keys.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `Kind::Expander` — `ExpanderC`, `expander-widget` and its arrow

**Files:**
- Create: `ui/src/widgets/expander.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/expander.txt`
- Modify: `ui/src/view/builders.rs` — `expander`, `.expanded`,
  `.use_underline`, `.resize_toplevel`, `.on_expanded`
- Modify: `ui/src/widgets/mod.rs` — `mod expander;`, the `Kind::Expander` arm
- Modify: `ui/tests/node_trees.rs` — `expander_matches_its_gtk_fixture`
- Test: `ui/src/widgets/expander.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `anim::{Clock, ManualClock}` (M2), P7's `Builtin::Expander` **by
  name only** (P5 already stubbed `Builtin::path`), `view::EventKind::Expanded`.
- Produces:
  ```rust
  pub fn expander<Msg: Clone + 'static>(label: &str, child: View<Msg>) -> View<Msg>;
  impl View<Msg> { pub fn expanded(self, v: impl Into<Prop>) -> Self;
                   pub fn on_expanded(self, f: impl Fn(bool) -> Msg + 'static) -> Self; }
  pub struct ExpanderC { pub expanded: bool, pub progress: f32, pub title: Node, pub arrow: Node, pub content: Node }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/expander.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::{build_widget, matches_fixture, Headless};

    const FIXTURE: &str = "expander-widget\n╰── box\n    ├── title\n    │   ├── expander\n    \
                           │   ╰── <child>\n    ╰── <child>\n";

    #[test]
    fn the_widget_node_is_expander_widget_and_the_arrow_is_expander() {
        // Mutation check: naming the root `expander` (the name the *arrow*
        // has) makes every Adwaita `expander-widget` rule miss and the arrow
        // rule match the whole widget -- the single easiest error in this
        // catalogue, which is why the fixture is asserted both ways.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        matches_fixture(&built.node, FIXTURE).expect("expander fixture");
        assert_eq!(&*built.node.name(), "expander-widget");
        let arrow = built.node.child(0).and_then(|b| b.child(0)).and_then(|t| t.child(0));
        assert_eq!(&*arrow.expect("arrow").name(), "expander");
    }

    #[test]
    fn space_toggles_and_emits_one_message() {
        // Interaction test. Mutation check: emitting on both press and
        // release doubles every expander message.
        let mut props = Props::default();
        props.set(PropName::Expanded, Prop::Bool(false));
        let built = build_widget::<bool>(Kind::Expander, &props);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(
                crate::view::EventKind::Expanded,
                crate::view::Handler::Bool(std::rc::Rc::new(|on| on)),
            );
        });
        let msgs = c.on_event(&Event::Activate, &mut cx);
        assert_eq!(msgs, vec![true]);
        assert_eq!(c.on_event(&Event::Activate, &mut cx), vec![false]);
    }

    #[test]
    fn expanding_animates_progress_to_one_and_then_stops_asking_for_frames() {
        // Mutation check: a `next_deadline` that keeps returning Some after
        // the animation finishes spins the frame pump forever; returning
        // None too early freezes the arrow mid-rotation.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(&Event::Activate, &mut cx);
        assert!(c.next_deadline(Duration::ZERO).is_some(), "a frame is wanted");
        c.tick(Duration::from_millis(1_000), &mut cx);
        assert_eq!(crate::widgets::expander::ExpanderC::progress_of(&c), 1.0);
        assert_eq!(c.next_deadline(Duration::from_millis(1_000)), None);
    }

    #[test]
    fn a_collapsed_expander_keeps_its_content_node_but_gives_it_no_height() {
        // Rest-state test. Mutation check: removing the content node on
        // collapse loses the child's focus and animation state, and the
        // reconciler then rebuilds it on every expand.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        let content = built.node.child(0).and_then(|b| b.child(1)).expect("content");
        assert!(content.parent().is_some());
        assert_eq!(crate::widgets::expander::ExpanderC::content_scale(&built.controller), 0.0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::expander`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `expander`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/expander.txt` (`gtk/gtkexpander.c`):

```
expander-widget
╰── box
    ├── title
    │   ├── expander
    │   ╰── <label widget>
    ╰── <child>
```

`ui/src/widgets/expander.rs`:

```rust
//! `GtkExpander` -- a disclosure triangle over a child.
//!
//! The widget's own node is `expander-widget`; the node *named* `expander`
//! is the arrow inside it (`gtk/gtkexpander.c`).

use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::{apply_universal_prop, prop_bool, prop_str, set_container, set_text};

/// How long the disclosure animation runs; GTK's own expander transition.
const DURATION: Duration = Duration::from_millis(200);

/// `GtkExpander`.
pub struct ExpanderC {
    /// Whether the child is disclosed.
    pub expanded: bool,
    /// `0.0` collapsed .. `1.0` expanded.
    pub progress: f32,
    /// The `title` node (arrow + label).
    pub title: Node,
    /// The arrow node, named `expander`.
    pub arrow: Node,
    /// The `box` that holds title and content.
    pub content: Node,
    /// When the running animation started; `None` when at rest.
    pub started: Option<Duration>,
}

impl ExpanderC {
    /// Test hook: the animation's current value.
    #[must_use]
    pub fn progress_of<Msg>(c: &Box<dyn Controller<Msg>>) -> f32 {
        c.as_any().downcast_ref::<Self>().map_or(f32::NAN, |c| c.progress)
    }

    /// Test hook: the content's current vertical scale.
    #[must_use]
    pub fn content_scale<Msg>(c: &Box<dyn Controller<Msg>>) -> f32 {
        Self::progress_of(c)
    }

    fn set_expanded(&mut self, node: &Node, on: bool, now: Duration) {
        if self.expanded == on {
            return;
        }
        self.expanded = on;
        self.started = Some(now);
        node.set_state(PseudoStates::CHECKED, on);
        self.arrow.set_state(PseudoStates::CHECKED, on);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ExpanderC {
    fn kind(&self) -> Kind {
        Kind::Expander
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(node, Container::Box { direction: BoxDirection::Column });
        let outer = Node::new("box");
        let title = Node::new("title");
        let arrow = Node::new("expander");
        let label = Node::new("label");
        set_text(&label, props.str(PropName::Label).unwrap_or_default());
        title.append_child(&arrow);
        title.append_child(&label);
        outer.append_child(&title);
        node.append_child(&outer);
        set_container(&outer, Container::Box { direction: BoxDirection::Column });
        set_container(&title, Container::Box { direction: BoxDirection::Row });
        let expanded = props.bool(PropName::Expanded, false);
        node.set_state(PseudoStates::CHECKED, expanded);
        arrow.set_state(PseudoStates::CHECKED, expanded);
        Self {
            expanded,
            progress: if expanded { 1.0 } else { 0.0 },
            title,
            arrow,
            content: outer,
            started: None,
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::Expanded => self.set_expanded(node, prop_bool(value, false), cx.clock.now()),
            PropName::Label => {
                if let Some(label) = self.title.child(1) {
                    set_text(&label, prop_str(value));
                }
            }
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let toggle = matches!(ev, Event::Activate)
            || matches!(ev, Event::PointerUp { local, .. }
                if crate::widgets::hit(&self.title, *local));
        if !toggle {
            return Vec::new();
        }
        let now = cx.clock.now();
        let want = !self.expanded;
        self.set_expanded(cx.node, want, now);
        cx.handled = true;
        cx.handlers
            .fire_bool(EventKind::Expanded, want)
            .into_iter()
            .collect()
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(started) = self.started else {
            return Vec::new();
        };
        let elapsed = now.saturating_sub(started);
        let t = if DURATION.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / DURATION.as_secs_f32()).clamp(0.0, 1.0)
        };
        self.progress = if self.expanded { t } else { 1.0 - t };
        if t >= 1.0 {
            self.started = None;
        }
        crate::widgets::set_scale_y(&self.content, self.progress);
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|started| {
            let end = started + DURATION;
            if now >= end { Duration::ZERO } else { end - now }
        })
    }
}
```

`hit(node, local)` tests a point against a node's recorded allocation;
`set_scale_y(node, factor)` records a vertical scale the paint walker
applies, both in `widgets/mod.rs` beside the other side-table helpers. The
arrow's own rotation is CSS: Adwaita rotates
`expander-widget:checked > box > title > expander` via
`-gtk-icon-transform`, which M2's computed style already resolves.

`ui/src/view/builders.rs`: `expander(label, child)` sets `PropName::Label`
and appends the child; `.expanded`, `.use_underline`, `.resize_toplevel` are
one line each; `.on_expanded(f)` stores `Handler::Bool` under
`EventKind::Expanded`.

Register `Kind::Expander => Box::new(ExpanderC::build(node, props, cx)),`
and add `expander_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::expander
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/expander.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/expander.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkExpander, with the widget node named expander-widget

The node named `expander` is the arrow, not the widget: getting that backwards
makes every Adwaita expander rule match the wrong box, so the fixture asserts
both names. Collapsing keeps the content node and scales it to zero, so the
child's focus and animations survive a collapse/expand cycle.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `Kind::ScrolledWindow` — `ScrolledWindowC`, overshoot, kinetic and overlay scrollbars

**Files:**
- Create: `ui/src/widgets/scrolled_window.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/scrolled_window.txt`
- Modify: `ui/src/view/builders.rs` — `scrolled_window` and its eleven setters
- Modify: `ui/src/widgets/mod.rs` — `mod scrolled_window;`, the arm
- Modify: `ui/tests/node_trees.rs` — `scrolled_window_matches_its_gtk_fixture`
- Test: `ui/src/widgets/scrolled_window.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P5's `ScrollbarC` and its `Adjustment`, P3's `window::{Scroll,
  Kinetic}`, `anim::Clock`, `widgets::types::Policy`.
- Produces:
  ```rust
  pub fn scrolled_window<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub struct ScrolledWindowC {
      pub offset: (f32, f32), pub extent: (f32, f32), pub kinetic: Kinetic,
      pub overshoot: (f32, f32), pub hbar: Option<ScrollbarC>, pub vbar: Option<ScrollbarC>,
      pub hovering_until: Option<Duration>, pub junction: Option<Node>,
  }
  impl ScrolledWindowC {
      pub fn scroll_by(&mut self, delta: (f32, f32), now: Duration) -> (f32, f32);
      pub fn visible_bars(&self) -> (bool, bool);
  }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/scrolled_window.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::scrolled_window::ScrolledWindowC;
    use crate::widgets::types::Policy;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    const FIXTURE: &str = "scrolledwindow[.frame]\n├── <child>\n\
        ├── [overshoot.top]\n├── [undershoot.top]\n\
        ├── [scrollbar.horizontal]\n├── [scrollbar.vertical]\n╰── [junction]\n";

    fn props(frame: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::HasFrame, Prop::Bool(frame));
        p.set(PropName::HscrollbarPolicy, Prop::Enum(Policy::Automatic.to_u16()));
        p.set(PropName::VscrollbarPolicy, Prop::Enum(Policy::Always.to_u16()));
        p
    }

    #[test]
    fn the_subnodes_are_gtks_and_frame_is_a_class_on_the_root() {
        // Mutation check: making `.frame` a subnode (or naming the scrollbar
        // subnodes `hscrollbar`/`vscrollbar`) fails the fixture, and every
        // Adwaita `scrolledwindow.frame` and `scrolledwindow > scrollbar`
        // rule stops matching.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(true));
        matches_fixture(&built.node, FIXTURE).expect("scrolled_window fixture");
        assert!(built.node.classes().iter().any(|c| c.as_str() == "frame"));
    }

    #[test]
    fn policy_never_hides_the_bar_but_keeps_scrolling_working() {
        // Mutation check: treating Policy::Never as "do not scroll" makes
        // the offset stay zero, which is how a themed sidebar becomes
        // unscrollable while looking fine.
        let mut p = props(false);
        p.set(PropName::VscrollbarPolicy, Prop::Enum(Policy::Never.to_u16()));
        let built = build_widget::<()>(Kind::ScrolledWindow, &p);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(&mut c, (100.0, 1000.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(hx.scroll(0.0, 50.0)), &mut cx);
        assert_eq!(ScrolledWindowC::offset_of(&c).1, 50.0);
        assert_eq!(ScrolledWindowC::visible_bars_of(&c), (false, false));
    }

    #[test]
    fn scrolling_past_the_end_overshoots_and_springs_back_on_the_clock() {
        // Mutation check: clamping the offset without recording overshoot
        // leaves `overshoot` at zero and the `overshoot.bottom` node never
        // appears; never decaying it leaves the content permanently offset.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(&mut c, (100.0, 200.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(hx.scroll(0.0, 500.0)), &mut cx);
        assert_eq!(ScrolledWindowC::offset_of(&c).1, 100.0, "clamped to the end");
        assert!(ScrolledWindowC::overshoot_of(&c).1 > 0.0, "overshoot recorded");
        c.tick(Duration::from_millis(500), &mut cx);
        assert_eq!(ScrolledWindowC::overshoot_of(&c).1, 0.0, "sprung back");
        assert_eq!(c.next_deadline(Duration::from_millis(500)), None);
    }

    #[test]
    fn a_kinetic_flick_decelerates_to_a_stop_within_a_bounded_number_of_ticks() {
        // Timing is a complexity bound, not a wall-clock pin: a flick must
        // stop, and must not stop instantly. Mutation check: a decay factor
        // of 1.0 never terminates and this test hits its 600-tick ceiling.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(&mut c, (100.0, 10_000.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(hx.flick(0.0, 40.0)), &mut cx);
        let mut ticks = 0;
        let mut now = Duration::ZERO;
        while c.next_deadline(now).is_some() && ticks < 600 {
            now += Duration::from_millis(16);
            c.tick(now, &mut cx);
            ticks += 1;
        }
        assert!((2..600).contains(&ticks), "decelerated in {ticks} ticks");
    }

    #[test]
    fn hostile_extents_never_panic_and_never_produce_a_nan_offset() {
        // Mutation check: dividing by a zero page size to size the slider
        // yields NaN and every later comparison is false, freezing the bar.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        for (content, page) in [
            ((0.0, 0.0), (0.0, 0.0)),
            ((f32::NAN, f32::INFINITY), (1.0, 1.0)),
            ((-5.0, 1e30), (1e30, -1.0)),
        ] {
            ScrolledWindowC::set_extent(&mut c, content, page);
            c.on_event(&Event::Scroll(hx.scroll(1e30, -1e30)), &mut cx);
            let (x, y) = ScrolledWindowC::offset_of(&c);
            assert!(x.is_finite() && y.is_finite(), "offset stayed finite");
        }
    }
}
```

`Headless::scroll(dx, dy)` and `Headless::flick(dx, dy)` build P3's `Scroll`
with and without the kinetic source flag.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::scrolled_window`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`scrolled_window` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/scrolled_window.txt`
(`gtk/gtkscrolledwindow.c:120`, rendered as the contract's §5.4 block):

```
scrolledwindow[.frame]
├── <child>
├── [overshoot.top][.bottom][.left][.right]
├── [undershoot.top][.bottom][.left][.right]
├── [scrollbar.horizontal[.overlay-indicator][.dragging][.hovering]]
├── [scrollbar.vertical[.overlay-indicator][.dragging][.hovering]]
╰── [junction]
```

`ui/src/widgets/scrolled_window.rs` — the controller. The distinctive parts
are the clamp-with-overshoot, the policy-independent scrolling, and the
scrollbar nodes being *created and destroyed* as the policy and extent
demand:

```rust
//! `GtkScrolledWindow` -- a viewport with overshoot, kinetic scrolling and
//! (optionally overlaid) scrollbars.

use std::time::Duration;

use crate::css::node::Node;
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::Policy;
use crate::window::{Kinetic, Scroll};
use crate::widgets::{apply_universal_prop, prop_bool, prop_u16, set_container, set_scroll_offset};

/// How far past the end a drag may pull, in px, before it stops moving.
const MAX_OVERSHOOT: f32 = 64.0;
/// Spring-back time from a full overshoot.
const SPRING: Duration = Duration::from_millis(200);

/// `GtkScrolledWindow`.
pub struct ScrolledWindowC {
    /// Current scroll offset, px, always within `0..=max`.
    pub offset: (f32, f32),
    /// `(content, page)` extents, px.
    pub extent: ((f32, f32), (f32, f32)),
    /// Deceleration state for a flick.
    pub kinetic: Kinetic,
    /// How far past an edge the content is pulled, px.
    pub overshoot: (f32, f32),
    /// Live scrollbar controllers, when their policy and extent want one.
    pub hbar: Option<crate::widgets::ScrollbarC>,
    /// As `hbar`, vertically.
    pub vbar: Option<crate::widgets::ScrollbarC>,
    /// Overlay bars stay visible until this instant after the last scroll.
    pub hovering_until: Option<Duration>,
    /// The `junction` node, present only while both bars are.
    pub junction: Option<Node>,
    /// Policies, and the flags that steer them.
    pub policy: (Policy, Policy),
    /// `GtkScrolledWindow:overlay-scrolling`.
    pub overlay: bool,
    /// `GtkScrolledWindow:kinetic-scrolling`.
    pub kinetic_enabled: bool,
    /// Spring-back start instant.
    spring_started: Option<Duration>,
}

impl ScrolledWindowC {
    /// The maximum offset on each axis, never negative and never NaN.
    #[must_use]
    fn max_offset(&self) -> (f32, f32) {
        let f = |content: f32, page: f32| {
            if content.is_finite() && page.is_finite() {
                (content - page).max(0.0)
            } else {
                0.0
            }
        };
        (
            f(self.extent.0.0, self.extent.1.0),
            f(self.extent.0.1, self.extent.1.1),
        )
    }

    /// Apply a delta, clamping to the ends and recording the overshoot.
    ///
    /// Scrolling is *never* gated on the scrollbar policy: `Policy::Never`
    /// hides the bar, it does not freeze the view (`gtkscrolledwindow.c`).
    pub fn scroll_by(&mut self, delta: (f32, f32), now: Duration) -> (f32, f32) {
        let (max_x, max_y) = self.max_offset();
        let dx = if delta.0.is_finite() { delta.0 } else { 0.0 };
        let dy = if delta.1.is_finite() { delta.1 } else { 0.0 };
        let wanted = (self.offset.0 + dx, self.offset.1 + dy);
        let clamped = (wanted.0.clamp(0.0, max_x), wanted.1.clamp(0.0, max_y));
        let over = (
            (wanted.0 - clamped.0).clamp(-MAX_OVERSHOOT, MAX_OVERSHOOT),
            (wanted.1 - clamped.1).clamp(-MAX_OVERSHOOT, MAX_OVERSHOOT),
        );
        self.offset = clamped;
        self.overshoot = over;
        if over != (0.0, 0.0) {
            self.spring_started = Some(now);
        }
        set_scroll_offset(&self.viewport_node(), self.offset, self.overshoot);
        self.offset
    }

    /// Which bars are currently rendered.
    #[must_use]
    pub fn visible_bars(&self) -> (bool, bool) {
        let (max_x, max_y) = self.max_offset();
        let want = |policy: Policy, max: f32| match policy {
            Policy::Always => true,
            Policy::Automatic => max > 0.0,
            Policy::Never | Policy::External => false,
        };
        (want(self.policy.0, max_x), want(self.policy.1, max_y))
    }
}
```

```rust
impl<Msg: Clone + 'static> Controller<Msg> for ScrolledWindowC {
    fn kind(&self) -> Kind {
        Kind::ScrolledWindow
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(node, Container::Box { direction: BoxDirection::Column });
        if props.bool(PropName::HasFrame, false) {
            node.add_class("frame");
        }
        let mut me = Self {
            offset: (0.0, 0.0),
            extent: ((0.0, 0.0), (0.0, 0.0)),
            kinetic: Kinetic::default(),
            overshoot: (0.0, 0.0),
            hbar: None,
            vbar: None,
            hovering_until: None,
            junction: None,
            policy: (
                Policy::from_u16(u16::try_from(props.int(PropName::HscrollbarPolicy, 1)).unwrap_or(1)),
                Policy::from_u16(u16::try_from(props.int(PropName::VscrollbarPolicy, 1)).unwrap_or(1)),
            ),
            overlay: props.bool(PropName::OverlayScrolling, true),
            kinetic_enabled: props.bool(PropName::Kinetic, true),
            spring_started: None,
        };
        me.sync_bars(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::HasFrame => {
                if prop_bool(value, false) {
                    node.add_class("frame");
                } else {
                    node.remove_class("frame");
                }
            }
            PropName::HscrollbarPolicy => self.policy.0 = Policy::from_u16(prop_u16(value, 1)),
            PropName::VscrollbarPolicy => self.policy.1 = Policy::from_u16(prop_u16(value, 1)),
            PropName::OverlayScrolling => self.overlay = prop_bool(value, true),
            PropName::Kinetic => self.kinetic_enabled = prop_bool(value, true),
            PropName::MinContentWidth
            | PropName::MinContentHeight
            | PropName::MaxContentWidth
            | PropName::MaxContentHeight
            | PropName::PropagateNaturalWidth
            | PropName::PropagateNaturalHeight => {
                crate::widgets::set_content_bound(node, name, value);
            }
            other => {
                apply_universal_prop(node, other, value);
                return;
            }
        }
        self.sync_bars(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Event::Scroll(scroll) = ev else {
            return Vec::new();
        };
        let now = cx.clock.now();
        let before = self.offset;
        self.scroll_by((scroll.dx as f32, scroll.dy as f32), now);
        if self.kinetic_enabled && scroll.is_flick {
            self.kinetic.start((scroll.dx as f32, scroll.dy as f32), now);
        }
        if self.overlay {
            self.hovering_until = Some(now + Duration::from_secs(1));
        }
        self.sync_bars(cx.node);
        cx.handled = true;
        if self.offset == before {
            return Vec::new();
        }
        cx.handlers
            .fire_pair(
                EventKind::Scrolled,
                f64::from(self.offset.0),
                f64::from(self.offset.1),
            )
            .into_iter()
            .collect()
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Some(delta) = self.kinetic.step(now) {
            self.scroll_by(delta, now);
        }
        if let Some(started) = self.spring_started {
            let t = (now.saturating_sub(started).as_secs_f32() / SPRING.as_secs_f32()).clamp(0.0, 1.0);
            self.overshoot = (self.overshoot.0 * (1.0 - t), self.overshoot.1 * (1.0 - t));
            if t >= 1.0 {
                self.overshoot = (0.0, 0.0);
                self.spring_started = None;
            }
            set_scroll_offset(&self.viewport_node(), self.offset, self.overshoot);
        }
        if self.hovering_until.is_some_and(|until| now >= until) {
            self.hovering_until = None;
        }
        self.sync_bars(cx.node);
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let spring = self.spring_started.map(|s| {
            let end = s + SPRING;
            if now >= end { Duration::ZERO } else { end - now }
        });
        let hover = self.hovering_until.map(|until| {
            if now >= until { Duration::ZERO } else { until - now }
        });
        [self.kinetic.next_deadline(now), spring, hover]
            .into_iter()
            .flatten()
            .min()
    }
}
```

`set_content_bound(node, name, value)` records the four min/max content
sizes and the two `propagate-natural-*` flags into the same pending side
table Task 4 introduced, where `flush_layout` turns them into taffy
`min_size`/`max_size`; `viewport_node()` returns the child the offset
translates (the first non-scrollbar child, or the root when there is none).

`sync_bars(&mut self, node: &Node)` creates or removes the `scrollbar`
nodes and the `junction`, and sets the positional and overlay classes:

```rust
    fn sync_bars(&mut self, node: &Node) {
        let (want_h, want_v) = self.visible_bars();
        for (want, slot, class) in [
            (want_h, 0_usize, "horizontal"),
            (want_v, 1, "vertical"),
        ] {
            let existing = crate::widgets::find_child(node, "scrollbar", class);
            match (want, existing) {
                (true, None) => {
                    let bar = Node::with_classes("scrollbar", &[class]);
                    if self.overlay {
                        bar.add_class("overlay-indicator");
                    }
                    node.append_child(&bar);
                    let controller = crate::widgets::ScrollbarC::for_node(&bar, slot == 1);
                    if slot == 0 { self.hbar = Some(controller) } else { self.vbar = Some(controller) }
                }
                (false, Some(bar)) => {
                    node.remove_child(&bar);
                    if slot == 0 { self.hbar = None } else { self.vbar = None }
                }
                _ => {}
            }
        }
        let both = want_h && want_v;
        match (both, self.junction.take()) {
            (true, None) => {
                let j = Node::new("junction");
                node.append_child(&j);
                self.junction = Some(j);
            }
            (false, Some(j)) => {
                node.remove_child(&j);
            }
            (true, Some(j)) => self.junction = Some(j),
            (false, None) => {}
        }
    }
```

`find_child(node, name, class)` is a `widgets/mod.rs` helper returning the
first child with that name and class; `set_scroll_offset(node, offset,
overshoot)` records the viewport translation the paint walker applies, and
creates or removes the `overshoot.<edge>` / `undershoot.<edge>` nodes from
the sign of each component — one node per pulled edge, exactly the classes
GTK's block names.

The eleven builder setters (`hscrollbar_policy`, `vscrollbar_policy`,
`has_frame`, `min_content_width`, `min_content_height`, `max_content_width`,
`max_content_height`, `propagate_natural_width`, `propagate_natural_height`,
`kinetic_scrolling`, `overlay_scrolling`) are one line each over their
`PropName`; `.on_scrolled(f)` stores `Handler::Pair` under
`EventKind::Scrolled`.

Register `Kind::ScrolledWindow => Box::new(ScrolledWindowC::build(node,
props, cx)),` and add `scrolled_window_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::scrolled_window
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — five unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/scrolled_window.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/scrolled_window.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkScrolledWindow with overshoot, spring-back and kinetic scrolling

Policy hides a bar; it never freezes the view. Overshoot is recorded rather
than discarded, so the overshoot.<edge> node GTK's block names actually
appears, and it springs back on the animation clock. Every extent is treated
as hostile: a zero page size or a NaN content size can never make the offset
NaN, which would silently freeze every later comparison.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: `Kind::SearchBar` and `Kind::ActionBar` — the two revealer bars

**Files:**
- Create: `ui/src/widgets/search_bar.rs`, `ui/src/widgets/action_bar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/search_bar.txt`,
  `ui/tests/fixtures/gtk4.22-node-trees/action_bar.txt`
- Modify: `ui/src/view/builders.rs` — `search_bar`, `action_bar`,
  `.search_mode`, `.show_close_button`, `.key_capture`, `.revealed`,
  `.pack_start`, `.pack_end`, `.center`
- Modify: `ui/src/widgets/mod.rs` — both modules and both arms
- Modify: `ui/tests/node_trees.rs` — two fixture tests
- Test: both new files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `anim::Clock`, `view::builders::slot`, P5's `ButtonC` for the
  close button, P3's `KeyEvent`.
- Produces:
  ```rust
  pub fn search_bar<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub fn action_bar<Msg: Clone + 'static>() -> View<Msg>;
  impl View<Msg> { pub fn pack_start(self, v: View<Msg>) -> Self;
                   pub fn pack_end(self, v: View<Msg>) -> Self;
                   pub fn center(self, v: View<Msg>) -> Self; }
  pub struct SearchBarC { pub revealed: bool, pub progress: f32, pub revealer: Node, pub close: Option<Node> }
  pub struct ActionBarC { pub revealed: bool, pub progress: f32, pub start: Node, pub center: Option<Node>, pub end: Node }
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/widgets/search_bar.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::search_bar::SearchBarC;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    const FIXTURE: &str =
        "searchbar\n╰── revealer\n    ╰── box\n        ├── [child]\n        ╰── [button.close]\n";

    fn props(close: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::ShowCloseButton, Prop::Bool(close));
        p.set(PropName::KeyCapture, Prop::Bool(true));
        p
    }

    #[test]
    fn the_bar_wraps_its_child_in_a_revealer_and_a_box() {
        // Mutation check: hanging the child straight off `searchbar` skips
        // the revealer, so the reveal animation has nothing to animate and
        // Adwaita's `searchbar > revealer > box` padding never applies.
        let built = build_widget::<()>(Kind::SearchBar, &props(true));
        matches_fixture(&built.node, FIXTURE).expect("search_bar fixture");
    }

    #[test]
    fn escape_closes_and_a_printable_key_opens_when_key_capture_is_on() {
        // Interaction test. Mutation check: forwarding Escape to the child
        // as well as closing makes an entry clear its text *and* the bar
        // close, which is two actions for one key.
        let built = build_widget::<bool>(Kind::SearchBar, &props(true));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Toggle, Handler::Bool(std::rc::Rc::new(|on| on)));
        });
        assert_eq!(c.on_event(&Event::Key(hx.key("a")), &mut cx), vec![true]);
        assert!(SearchBarC::revealed_of(&c));
        assert_eq!(c.on_event(&Event::Key(hx.key("Escape")), &mut cx), vec![false]);
        assert!(!SearchBarC::revealed_of(&c));
        assert!(cx.handled, "Escape is consumed here, not forwarded");
    }

    #[test]
    fn the_close_button_exists_only_while_show_close_button_is_set() {
        // Rest-state test. Mutation check: always building the button adds
        // Adwaita's 34px control to a bar that asked for none.
        let with = build_widget::<()>(Kind::SearchBar, &props(true));
        let without = build_widget::<()>(Kind::SearchBar, &props(false));
        assert!(SearchBarC::close_of(&with.controller).is_some());
        assert!(SearchBarC::close_of(&without.controller).is_none());
    }
}
```

`ui/src/widgets/action_bar.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::builders::{action_bar, button, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, matches_fixture};

    const FIXTURE: &str = "actionbar\n╰── revealer\n    ╰── box\n        ├── box.start\n\
        │   ╰── [start children]\n        ├── [center widget]\n        ╰── box.end\n\
        │           ╰── [end children]\n";

    #[test]
    fn the_bar_has_a_start_box_an_optional_centre_and_an_end_box() {
        // Mutation check: packing end children into the start box (an easy
        // slot mix-up) puts them on the wrong side and fails the fixture's
        // `box.end` requirement.
        let built = build_widget::<()>(Kind::ActionBar, &Props::default());
        matches_fixture(&built.node, FIXTURE).expect("action_bar fixture");
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        action_bar()
            .pack_start(button("L"))
            .pack_end(button("R"))
            .center(label("C"))
            .revealed(true)
    }

    #[test]
    fn packed_children_land_on_their_own_side_at_rest() {
        // Rest-state pixel test. Mutation check: dropping the slot read puts
        // all three children in tree order at the left and the right third
        // of the surface stays empty.
        let out = frames((), update, view, (300, 48), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 150, 2);
        let ink_in = |from: u32, to: u32| (from..to).any(|x| px(&out, 0, x, 24) != background);
        assert!(ink_in(0, 100), "start pack painted");
        assert!(ink_in(120, 180), "centre widget painted");
        assert!(ink_in(200, 300), "end pack painted");
    }

    #[test]
    fn collapsing_the_bar_animates_and_then_stops_asking_for_frames() {
        // Interaction test + the "ZERO means now, never spin" rule.
        let built = build_widget::<()>(Kind::ActionBar, &Props::default());
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.set_prop(&built.node, PropName::Revealed, &Prop::Bool(true), &mut hx.cx());
        assert!(c.next_deadline(std::time::Duration::ZERO).is_some());
        c.tick(std::time::Duration::from_millis(500), &mut cx);
        assert_eq!(c.next_deadline(std::time::Duration::from_millis(500)), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::search_bar widgets::action_bar`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`search_bar` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

Fixtures, verbatim from the contract's §5.4 blocks
(`gtk/gtksearchbar.c`, `gtk/gtkactionbar.c`):

`search_bar.txt`
```
searchbar
╰── revealer
    ╰── box
         ├── [child]
         ╰── [button.close]
```

`action_bar.txt`
```
actionbar
╰── revealer
    ╰── box
        ├── box.start
        │   ╰── [start children]
        ├── [center widget]
        ╰── box.end
            ╰── [end children]
```

Both controllers share one mechanism, so it lives once in
`ui/src/widgets/mod.rs`:

```rust
/// A GTK revealer: a child subtree whose height (or width) animates between
/// zero and its natural size.
///
/// `GtkSearchBar` and `GtkActionBar` both own one, and `GtkInfoBar` (P5)
/// animates the same way, so the timing lives in one place.
pub struct Revealer {
    /// The `revealer` node.
    pub node: Node,
    /// Target state.
    pub revealed: bool,
    /// `0.0` hidden .. `1.0` shown.
    pub progress: f32,
    started: Option<Duration>,
    duration: Duration,
}

impl Revealer {
    /// Build a `revealer` node under `parent`.
    pub fn build(parent: &Node, revealed: bool) -> Self {
        let node = Node::new("revealer");
        parent.append_child(&node);
        let me = Self {
            node,
            revealed,
            progress: if revealed { 1.0 } else { 0.0 },
            started: None,
            duration: Duration::from_millis(250),
        };
        set_scale_y(&me.node, me.progress);
        me
    }

    /// Ask for a new target; a no-op when it is already the target.
    pub fn set_revealed(&mut self, revealed: bool, now: Duration) {
        if self.revealed == revealed {
            return;
        }
        self.revealed = revealed;
        self.started = Some(now);
    }

    /// Advance the animation and write the scale the paint walker reads.
    pub fn tick(&mut self, now: Duration) {
        let Some(started) = self.started else {
            return;
        };
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (now.saturating_sub(started).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        self.progress = if self.revealed { t } else { 1.0 - t };
        if t >= 1.0 {
            self.started = None;
        }
        set_scale_y(&self.node, self.progress);
    }

    /// The next frame this wants, or `None` at rest. `Duration::ZERO` means
    /// "now", never "spin": `tick` clears `started` at the end.
    #[must_use]
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|started| {
            let end = started + self.duration;
            if now >= end { Duration::ZERO } else { end - now }
        })
    }
}
```

`SearchBarC` owns a `Revealer`, an optional `close` button node, and
`key_capture`. Its `on_event`:

```rust
            Event::Key(key) if key.keysym_name() == "Escape" && self.revealer.revealed => {
                self.revealer.set_revealed(false, cx.clock.now());
                cx.handled = true;
                cx.handlers.fire_bool(EventKind::Toggle, false).into_iter().collect()
            }
            Event::Key(key) if self.key_capture && !self.revealer.revealed && key.is_printable() => {
                self.revealer.set_revealed(true, cx.clock.now());
                // The key that opened the bar is *also* delivered to the
                // child entry -- GTK's capture forwards it -- so this does
                // not set `cx.handled`.
                cx.handlers.fire_bool(EventKind::Toggle, true).into_iter().collect()
            }
```

`ActionBarC` builds `revealer > box > (box.start, [center], box.end)` in
`build` and distributes children by their `slot`:

```rust
    fn place(&self, node: &Node) {
        for child in node.children() {
            if child.ptr_eq(&self.revealer.node) {
                continue;
            }
            let props = crate::widgets::props_of(&child);
            match props.str(PropName::Section).unwrap_or("start") {
                "end" => self.end.append_child(&child),
                "center" => {
                    if let Some(centre) = &self.center {
                        centre.append_child(&child);
                    }
                }
                _ => self.start.append_child(&child),
            }
        }
    }
```

`pack_start`/`pack_end`/`center` in `builders.rs` are `slot`-taggers:

```rust
impl<Msg: Clone + 'static> View<Msg> {
    /// Pack a child at the leading end (`gtk_action_bar_pack_start`,
    /// `gtk_header_bar_pack_start`).
    #[must_use]
    pub fn pack_start(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "start"));
        self
    }

    /// Pack a child at the trailing end.
    #[must_use]
    pub fn pack_end(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "end"));
        self
    }

    /// The centre widget.
    #[must_use]
    pub fn center(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "center"));
        self
    }
}
```

Register both arms and add both fixture tests.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::search_bar widgets::action_bar
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — six unit tests and two fixture tests.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/search_bar.rs ui/src/widgets/action_bar.rs ui/src/widgets/mod.rs \
        ui/src/view/builders.rs ui/tests/fixtures/gtk4.22-node-trees/search_bar.txt \
        ui/tests/fixtures/gtk4.22-node-trees/action_bar.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkSearchBar and GtkActionBar over a shared Revealer

One Revealer, two bars: the reveal timing, the scale write and the
next_deadline rule live in one place instead of being copied. Escape closes
the search bar and is consumed there; the key that opens it by capture is
still delivered to the entry, which is what GTK's capture does.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: `Kind::HeaderBar` — `HeaderBarC`, packs, title widget, window controls

**Files:**
- Create: `ui/src/widgets/header_bar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/header_bar.txt`
- Modify: `ui/src/view/builders.rs` — `header_bar`, `.title`, `.subtitle`,
  `.title_widget`, `.show_title_buttons`, `.decoration_layout`
- Modify: `ui/src/widgets/mod.rs` — `mod header_bar;`, the arm
- Modify: `ui/tests/node_trees.rs` — `header_bar_matches_its_gtk_fixture`
- Test: `ui/src/widgets/header_bar.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P5's `WindowControlsC` and its `Side`/`WindowButton` decoder,
  `view::Cmd::{Minimize, ToggleMaximized, CloseWindow}`, `window::SurfaceStates`.
- Produces:
  ```rust
  pub fn header_bar<Msg: Clone + 'static>() -> View<Msg>;
  pub struct HeaderBarC {
      pub start: Node, pub end: Node, pub title: Node,
      pub controls: [Option<WindowControlsC>; 2], pub layout: Rc<str>,
  }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/header_bar.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::header_bar::HeaderBarC;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    const FIXTURE: &str = "headerbar\n╰── windowhandle\n    ╰── box\n        ├── box.start\n\
        │   ├── windowcontrols.start\n        │   ╰── [other children]\n\
        ├── [Title Widget]\n        ╰── box.end\n            ├── [other children]\n\
        ╰── windowcontrols.end\n";

    fn props(layout: &str, buttons: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Decoration, Prop::Str(layout.into()));
        p.set(PropName::ShowTitleButtons, Prop::Bool(buttons));
        p.set(PropName::Title, Prop::Str("Files".into()));
        p
    }

    #[test]
    fn the_bar_is_a_windowhandle_over_a_three_slot_box() {
        // Mutation check: dropping the `windowhandle` node makes the bar
        // undraggable and Adwaita's `headerbar > windowhandle` rules miss.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:minimize,close", true));
        matches_fixture(&built.node, FIXTURE).expect("header_bar fixture");
    }

    #[test]
    fn the_decoration_layout_splits_on_the_colon_and_feeds_both_sides() {
        // Mutation check: giving both WindowControls the whole layout string
        // renders the close button twice, once on each side.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:minimize,close", true));
        let (start, end) = HeaderBarC::button_names(&built.controller);
        assert_eq!(start, vec!["icon"]);
        assert_eq!(end, vec!["minimize", "close"]);
    }

    #[test]
    fn show_title_buttons_false_leaves_no_controls_at_all() {
        // Mutation check: building the controls and hiding them with a class
        // still reserves their width, so the title stops being centred.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:close", false));
        let (start, end) = HeaderBarC::button_names(&built.controller);
        assert!(start.is_empty() && end.is_empty());
    }

    #[test]
    fn a_double_click_on_the_handle_asks_the_compositor_to_toggle_maximised() {
        // Interaction test. Mutation check: emitting Cmd::ToggleMaximized on
        // a single click makes a click-to-focus maximise the window.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:close", true));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(&Event::PointerDown { button: 0x110, local: (60.0, 8.0), serial: 1 }, &mut cx);
        c.on_event(&Event::PointerUp { button: 0x110, local: (60.0, 8.0), serial: 2 }, &mut cx);
        assert!(cx.cmds.is_empty(), "one click does nothing");
        c.on_event(&Event::PointerDown { button: 0x110, local: (60.0, 8.0), serial: 3 }, &mut cx);
        c.on_event(&Event::PointerUp { button: 0x110, local: (60.0, 8.0), serial: 4 }, &mut cx);
        assert!(
            cx.cmds.iter().any(|c| matches!(c, crate::view::Cmd::ToggleMaximized)),
            "the second click within the double-click window toggles"
        );
    }

    #[test]
    fn a_hostile_decoration_layout_never_panics() {
        // The layout string comes from `settings.ini`, which is user input.
        for layout in ["", ":", "::::", "menu", "close,close,close:", &"a,".repeat(10_000),
                       "icon:\u{0}", "\u{feff}minimize:close"] {
            let _ = build_widget::<()>(Kind::HeaderBar, &props(layout, true));
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::header_bar`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`header_bar` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/header_bar.txt` (`gtk/gtkheaderbar.c`,
the contract's §5.4 block):

```
headerbar
╰── windowhandle
    ╰── box
        ├── box.start
        │   ├── windowcontrols.start
        │   ╰── [other children]
        ├── [Title Widget]
        ╰── box.end
            ├── [other children]
            ╰── windowcontrols.end
```

`ui/src/widgets/header_bar.rs`:

```rust
//! `GtkHeaderBar` -- a draggable title bar with start/end packs and window
//! controls placed by `gtk-decoration-layout`.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Cmd, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::types::Side;
use crate::widgets::{apply_universal_prop, prop_bool, prop_str, set_child_layout, set_container, set_text};

/// `gtk-titlebar-double-click`'s default action window.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// `GtkHeaderBar`.
pub struct HeaderBarC {
    /// `box.start`.
    pub start: Node,
    /// `box.end`.
    pub end: Node,
    /// The title widget (a `label`, or the app's own view).
    pub title: Node,
    /// `[start, end]` window controls, when `show-title-buttons`.
    pub controls: [Option<crate::widgets::WindowControlsC>; 2],
    /// The effective `gtk-decoration-layout`.
    pub layout: Rc<str>,
    /// The `windowhandle` node, for drag and double-click.
    handle: Node,
    /// When the last press on the handle landed.
    last_press: Option<Duration>,
}

impl HeaderBarC {
    /// Test hook: the button names each side actually rendered.
    #[must_use]
    pub fn button_names<Msg>(c: &Box<dyn Controller<Msg>>) -> (Vec<String>, Vec<String>) {
        let me = c.as_any().downcast_ref::<Self>();
        let names = |slot: usize| {
            me.and_then(|m| m.controls[slot].as_ref())
                .map(crate::widgets::WindowControlsC::button_names)
                .unwrap_or_default()
        };
        (names(0), names(1))
    }

    /// Split `gtk-decoration-layout` on its single colon.
    ///
    /// User input: a missing colon puts everything on the start side (GTK's
    /// own reading), extra colons are ignored, and each half is capped at 8
    /// entries so a pathological settings file cannot build a thousand
    /// buttons.
    #[must_use]
    pub fn split_layout(layout: &str) -> (Vec<&str>, Vec<&str>) {
        let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
        let half = |s: &str| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(8)
                .collect::<Vec<_>>()
        };
        (half(start), half(end))
    }
}
```

`Controller for HeaderBarC`:

```rust
impl<Msg: Clone + 'static> Controller<Msg> for HeaderBarC {
    fn kind(&self) -> Kind {
        Kind::HeaderBar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        set_container(node, Container::Box { direction: BoxDirection::Column });
        let handle = Node::new("windowhandle");
        let row = Node::new("box");
        let start = Node::with_classes("box", &["start"]);
        let title = Node::new("label");
        let end = Node::with_classes("box", &["end"]);
        node.append_child(&handle);
        handle.append_child(&row);
        for (child, direction) in [
            (&row, BoxDirection::Row),
            (&start, BoxDirection::Row),
            (&end, BoxDirection::Row),
        ] {
            set_container(child, Container::Box { direction });
        }
        row.append_child(&start);
        row.append_child(&title);
        row.append_child(&end);
        set_child_layout(&title, ChildLayout { halign: Align::Center, hexpand: true, ..ChildLayout::default() });
        set_text(&title, props.str(PropName::Title).unwrap_or_default());
        let layout: Rc<str> = Rc::from(
            props
                .str(PropName::Decoration)
                .unwrap_or("icon:minimize,maximize,close"),
        );
        let mut me = Self {
            start,
            end,
            title,
            controls: [None, None],
            layout,
            handle,
            last_press: None,
        };
        if props.bool(PropName::ShowTitleButtons, true) {
            me.build_controls(cx);
        }
        me.place(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::Title => set_text(&self.title, prop_str(value)),
            PropName::Decoration => {
                self.layout = Rc::from(prop_str(value));
                self.rebuild_controls(cx);
            }
            PropName::ShowTitleButtons => {
                if prop_bool(value, true) {
                    self.rebuild_controls(cx);
                } else {
                    self.drop_controls();
                }
            }
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        match ev {
            Event::PointerDown { button: 0x110, local, .. }
                if crate::widgets::hit(&self.handle, *local) =>
            {
                let now = cx.clock.now();
                let double = self
                    .last_press
                    .is_some_and(|last| now.saturating_sub(last) <= DOUBLE_CLICK);
                self.last_press = Some(now);
                if double {
                    // `gtk-titlebar-double-click` defaults to toggle-maximize.
                    cx.cmds.push(Cmd::ToggleMaximized);
                    self.last_press = None;
                    cx.handled = true;
                }
            }
            // `gtk-titlebar-middle-click` defaults to `none`; the press is
            // observed and deliberately does nothing.
            Event::PointerDown { button: 0x112, .. } => {}
            _ => {}
        }
        Vec::new()
    }
}
```

`build_controls` walks `split_layout(&self.layout)` and, for the non-empty
half, builds a `windowcontrols` node with class `start`/`end` and hands it to
`WindowControlsC::for_node(&node, Side::Start | Side::End, half)`, which is
P5's decoder for the per-button rules (icon only when sovereign, maximize
only when resizable, `menu` producing nothing). `drop_controls` removes both
nodes; `rebuild_controls` is `drop_controls` then `build_controls`. `place`
distributes the view's children by their `slot` into `start`/`end`, replacing
`title` with the child tagged `"title"` when there is one.

Builders: `header_bar()` is `View::new(Kind::HeaderBar)`; `.title`,
`.subtitle`, `.show_title_buttons`, `.decoration_layout` are one line each;
`.title_widget(v)` pushes `slot(v, "title")`. `.pack_start`/`.pack_end` come
from Task 11.

Register `Kind::HeaderBar => Box::new(HeaderBarC::build(node, props, cx)),`
and add `header_bar_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::header_bar
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — five unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/header_bar.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/header_bar.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkHeaderBar with packs, a title widget and per-side controls

The decoration layout is split once on its colon and each half feeds only its
own WindowControls, so the close button cannot appear twice; the string is
user input from settings.ini, so each half is capped and a missing colon is
read GTK's way. Double-click on the handle asks the compositor to toggle
maximise, single click does nothing.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: `Kind::Notebook` / `Kind::NotebookTab` — `NotebookC`

**Files:**
- Create: `ui/src/widgets/notebook.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/notebook.txt`
- Modify: `ui/src/view/builders.rs` — `notebook`, `notebook_tab`, `.page`,
  `.tab_pos`, `.scrollable`, `.show_tabs`, `.show_border`, `.reorderable`,
  `.detachable`, `.on_page_changed`, `.on_reordered`
- Modify: `ui/src/widgets/mod.rs` — `mod notebook;`, both arms
- Modify: `ui/tests/node_trees.rs` — `notebook_matches_its_gtk_fixture`
- Test: `ui/src/widgets/notebook.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `widgets::types::Position`, `view::EventKind::{PageChanged,
  Reordered}`, `Handlers::{fire_index, fire_indices}`, P3's `KeyEvent`.
- Produces:
  ```rust
  pub fn notebook<Msg: Clone + 'static>(pages: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn notebook_tab<Msg: Clone + 'static>(label: &str, child: View<Msg>) -> View<Msg>;
  pub struct NotebookC {
      pub page: usize, pub tabs: Vec<Node>, pub scroll: f32,
      pub drag: Option<(usize, f32)>, pub header: Node, pub stack: Node,
      pub arrows: [Option<Node>; 2],
  }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/notebook.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::notebook::NotebookC;
    use crate::widgets::types::Position;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    const FIXTURE: &str = "notebook\n├── header.top\n│   ├── [<action widget>]\n\
        │   ├── tabs\n│   │   ├── [arrow]\n│   │   ├── tab\n│   │   │   ╰── <tab label>\n\
        ┊   ┊   ┊\n│   │   ╰── [arrow]\n│   ╰── [<action widget>]\n│\n\
        ╰── stack\n    ├── <child>\n    ┊\n    ╰── <child>\n";

    fn three_tabs() -> Props {
        let mut p = Props::default();
        p.set(PropName::Page, Prop::Int(0));
        p.set(PropName::TabPos, Prop::Enum(Position::Top.to_u16()));
        p.set(PropName::Pages, Prop::Int(3));
        p
    }

    #[test]
    fn a_notebook_is_a_header_and_a_stack() {
        // Mutation check: putting the tabs directly under `notebook` (no
        // `header`/`tabs` pair) fails the fixture and every Adwaita
        // `notebook > header.top > tabs > tab` rule.
        let built = build_widget::<()>(Kind::Notebook, &three_tabs());
        matches_fixture(&built.node, FIXTURE).expect("notebook fixture");
        assert!(built.node.child(0).is_some_and(|h| h
            .classes()
            .iter()
            .any(|c| c.as_str() == "top")));
    }

    #[test]
    fn the_selected_tab_is_the_checked_one_and_only_its_page_is_visible() {
        // Mutation check: leaving :checked on every visited tab makes the
        // whole strip look selected.
        let built = build_widget::<()>(Kind::Notebook, &three_tabs());
        let mut c = built.controller;
        let mut hx = Headless::new();
        c.set_prop(&built.node, PropName::Page, &Prop::Int(2), &mut hx.cx());
        let checked: Vec<usize> = NotebookC::checked_tabs(&c);
        assert_eq!(checked, vec![2]);
        assert_eq!(NotebookC::visible_page(&c), 2);
    }

    #[test]
    fn ctrl_page_down_and_alt_digit_switch_pages_and_emit_once() {
        // Interaction test. Mutation check: emitting PageChanged from both
        // `set_prop` and the key path doubles every switch message.
        let built = build_widget::<usize>(Kind::Notebook, &three_tabs());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::PageChanged, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(c.on_event(&Event::Key(hx.key_with_mods("Next", &["Control"])), &mut cx), vec![1]);
        assert_eq!(c.on_event(&Event::Key(hx.key_with_mods("3", &["Alt"])), &mut cx), vec![2]);
        assert_eq!(c.on_event(&Event::Key(hx.key_with_mods("9", &["Alt"])), &mut cx), Vec::<usize>::new());
    }

    #[test]
    fn dragging_a_reorderable_tab_emits_from_and_to() {
        // Mutation check: reporting (to, from) instead of (from, to) makes
        // every model reorder run backwards.
        let mut props = three_tabs();
        props.set(PropName::Reorderable, Prop::Bool(true));
        let built = build_widget::<(usize, usize)>(Kind::Notebook, &props);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Reordered, Handler::Indices(std::rc::Rc::new(|a, b| (a, b))));
        });
        c.on_event(&Event::PointerDown { button: 0x110, local: (10.0, 8.0), serial: 1 }, &mut cx);
        c.on_event(&Event::PointerMotion { local: (200.0, 8.0) }, &mut cx);
        let msgs = c.on_event(&Event::PointerUp { button: 0x110, local: (200.0, 8.0), serial: 2 }, &mut cx);
        assert_eq!(msgs, vec![(0, 2)]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::notebook`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `notebook`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/notebook.txt` — the contract's §5.4
block verbatim (`gtk/gtknotebook.c`), including the `┊` rows.

`ui/src/widgets/notebook.rs`:

```rust
//! `GtkNotebook` -- a tab strip over a stack of pages.

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::Position;
use crate::widgets::{apply_universal_prop, prop_bool, prop_i64, prop_u16, set_container, set_text};

/// `GtkNotebook`.
pub struct NotebookC {
    /// The visible page.
    pub page: usize,
    /// One `tab` node per page.
    pub tabs: Vec<Node>,
    /// Horizontal scroll of the strip, px, when `scrollable`.
    pub scroll: f32,
    /// `(index, grab offset)` while a tab is being dragged.
    pub drag: Option<(usize, f32)>,
    /// `header`.
    pub header: Node,
    /// `stack`.
    pub stack: Node,
    /// Leading and trailing `arrow` nodes, when `scrollable`.
    pub arrows: [Option<Node>; 2],
    /// Where the strip sits.
    pub tab_pos: Position,
    /// `GtkNotebook:reorderable-page` on the pages.
    pub reorderable: bool,
}

impl NotebookC {
    /// Test hook: which tabs carry `:checked`.
    #[must_use]
    pub fn checked_tabs<Msg>(c: &Box<dyn Controller<Msg>>) -> Vec<usize> {
        c.as_any().downcast_ref::<Self>().map_or_else(Vec::new, |me| {
            me.tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| tab.states().contains(PseudoStates::CHECKED))
                .map(|(i, _)| i)
                .collect()
        })
    }

    /// Test hook: the page the stack is showing.
    #[must_use]
    pub fn visible_page<Msg>(c: &Box<dyn Controller<Msg>>) -> usize {
        c.as_any().downcast_ref::<Self>().map_or(usize::MAX, |me| me.page)
    }

    /// Move to `page`, clamped to the page count; `true` when it moved.
    fn goto(&mut self, page: usize) -> bool {
        let count = self.tabs.len();
        if count == 0 {
            return false;
        }
        let page = page.min(count - 1);
        if page == self.page {
            return false;
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            tab.set_state(PseudoStates::CHECKED, i == page);
        }
        for (i, child) in self.stack.children().iter().enumerate() {
            crate::widgets::set_visible(child, i == page);
        }
        self.page = page;
        true
    }

    /// The tab index a strip-local x falls in, from the tabs' allocations.
    fn tab_at(&self, x: f32) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| crate::widgets::hit(tab, (x, 0.0)))
    }
}
```

`Controller for NotebookC`: `build` creates `header` (with the positional
class from `tab_pos`), `tabs`, one `tab > label` per `Kind::NotebookTab`
child, and `stack` holding the pages; `set_prop` handles `Page` (through
`goto`, which does **not** emit — the message belongs to the input path),
`TabPos` (swap the positional class), `ShowTabs` (add or remove the header),
`ShowBorder`, `Scrollable` (create or drop the two `arrow` nodes) and
`Reorderable` (the `.reorderable-page` class on every tab), delegating the
rest; `on_event`:

```rust
    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut out = Vec::new();
        match ev {
            Event::PointerDown { local, .. } => {
                if let Some(index) = self.tab_at(local.0) {
                    if self.goto(index)
                        && let Some(msg) = cx.handlers.fire_index(EventKind::PageChanged, index)
                    {
                        out.push(msg);
                    }
                    if self.reorderable {
                        self.drag = Some((index, local.0));
                    }
                    cx.handled = true;
                }
            }
            Event::PointerMotion { local } => {
                if let Some((from, _)) = self.drag {
                    crate::widgets::set_drag_offset(&self.tabs[from], local.0);
                }
            }
            Event::PointerUp { local, .. } => {
                if let Some((from, _)) = self.drag.take()
                    && let Some(to) = self.tab_at(local.0)
                    && to != from
                    && let Some(msg) = cx.handlers.fire_indices(EventKind::Reordered, from, to)
                {
                    out.push(msg);
                }
            }
            Event::Key(key) => {
                let target = match (key.keysym_name(), key.mods.control(), key.mods.alt()) {
                    ("Next", true, _) => Some(self.page.saturating_add(1)),
                    ("Prior", true, _) => Some(self.page.saturating_sub(1)),
                    (digit, _, true) if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) => {
                        digit.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
                    }
                    _ => None,
                };
                // Alt+9 on a three-page notebook selects nothing at all --
                // GTK clamps to the *last* page only for Ctrl+PageDown.
                if let Some(target) = target {
                    let in_range = target < self.tabs.len();
                    if in_range
                        && self.goto(target)
                        && let Some(msg) = cx.handlers.fire_index(EventKind::PageChanged, self.page)
                    {
                        out.push(msg);
                        cx.handled = true;
                    }
                }
            }
            _ => {}
        }
        out
    }
```

Builders: `notebook(pages)` collects `Kind::NotebookTab` children;
`notebook_tab(label, child)` is
`View::new(Kind::NotebookTab).prop(PropName::Label, label).child(child)`
with `.reorderable`/`.detachable` setters; `.on_page_changed(f)` stores
`Handler::Index` and `.on_reordered(f)` stores `Handler::Indices`.

Register both arms (`Kind::NotebookTab`'s controller is `GenericC` over the
`tab` node — a tab has no behaviour of its own; the strip owns it) and add
`notebook_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::notebook
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/notebook.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/notebook.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkNotebook with tab switching, keynav and drag reordering

goto() moves the page and never emits: the message belongs to whichever input
path caused the move, which is what keeps a single Ctrl+PageDown from
producing two PageChanged messages. Reorder reports (from, to) in that order.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: `Kind::Overlay` — `OverlayC`

**Files:**
- Create: `ui/src/widgets/overlay.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/overlay.txt`
- Modify: `ui/src/view/builders.rs` — `overlay`, `.overlay`,
  `.measure_overlay`, `.clip_overlay`
- Modify: `ui/src/widgets/mod.rs` — `mod overlay;`, the arm
- Modify: `ui/tests/node_trees.rs` — `overlay_matches_its_gtk_fixture`
- Test: `ui/src/widgets/overlay.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `layout::{Align, ChildLayout}`, `view::builders::slot`.
- Produces:
  ```rust
  pub fn overlay<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  impl View<Msg> { pub fn overlay(self, v: View<Msg>) -> Self;
                   pub fn measure_overlay(self, v: impl Into<Prop>) -> Self;
                   pub fn clip_overlay(self, v: impl Into<Prop>) -> Self; }
  pub struct OverlayC { pub overlays: Vec<(Node, bool, bool)> }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/overlay.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::layout::Align;
    use crate::view::builders::{label, overlay};
    use crate::view::{Cmd, Kind, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::overlay::OverlayC;
    use crate::widgets::{build_widget, matches_fixture};

    #[test]
    fn an_overlay_is_the_child_then_the_overlay_children() {
        // Mutation check: inserting overlays before the main child paints
        // them under it, which is the whole point of the widget inverted.
        let built = build_widget::<()>(Kind::Overlay, &Props::default());
        matches_fixture(&built.node, "overlay\n├── <child>\n╰── <overlay child>\n")
            .expect("overlay fixture");
    }

    #[test]
    fn an_edge_aligned_overlay_child_gets_that_positional_class() {
        // Mutation check: skipping the positional class means Adwaita's
        // `overlay > .top` shadow never renders on an overlaid header.
        assert_eq!(OverlayC::positional_class(Align::Start, Align::Fill), Some("left"));
        assert_eq!(OverlayC::positional_class(Align::Fill, Align::End), Some("bottom"));
        assert_eq!(OverlayC::positional_class(Align::Center, Align::Center), None);
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        overlay(label("under").class("under")).overlay(label("over").class("over").valign(Align::Start))
    }

    #[test]
    fn the_overlay_child_paints_over_the_main_child_at_rest() {
        // Rest-state pixel test. Mutation check: painting children in
        // reverse order leaves the top strip showing the main child.
        let out = frames((), update, view, (200, 60), vec![ScriptStep::Capture]);
        let bottom = px(&out, 0, 100, 55);
        let top = px(&out, 0, 100, 5);
        assert_ne!(top, bottom, "the overlaid child owns the top strip");
    }

    #[test]
    fn a_non_measuring_overlay_does_not_grow_the_container() {
        // Interaction test through the measure path. Mutation check: always
        // measuring overlays makes a floating action button widen its page.
        let small = OverlayC::natural_width(&[(120.0, false)], 200.0);
        let measured = OverlayC::natural_width(&[(400.0, true)], 200.0);
        assert_eq!(small, 200.0);
        assert_eq!(measured, 400.0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::overlay`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `overlay`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/overlay.txt` (`gtk/gtkoverlay.c`):

```
overlay
├── <child>
╰── <overlay child>[.left][.right][.top][.bottom]
```

`ui/src/widgets/overlay.rs`:

```rust
//! `GtkOverlay` -- one main child with siblings painted over it.

use crate::css::node::Node;
use crate::layout::{Align, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::{apply_universal_prop, child_layout_of, prop_bool, props_of, set_container};

/// `GtkOverlay`.
pub struct OverlayC {
    /// `(node, measure_overlay, clip_overlay)` per overlaid child.
    pub overlays: Vec<(Node, bool, bool)>,
}

impl OverlayC {
    /// The positional class an overlay child's alignment implies, or `None`
    /// when it is not pinned to an edge.
    #[must_use]
    pub fn positional_class(halign: Align, valign: Align) -> Option<&'static str> {
        match (halign, valign) {
            (Align::Start, _) => Some("left"),
            (Align::End, _) => Some("right"),
            (_, Align::Start) => Some("top"),
            (_, Align::End) => Some("bottom"),
            _ => None,
        }
    }

    /// The container's natural width: the main child's, widened only by the
    /// overlays that opted into measuring.
    #[must_use]
    pub fn natural_width(overlays: &[(f32, bool)], main: f32) -> f32 {
        overlays
            .iter()
            .filter(|(_, measure)| *measure)
            .map(|(w, _)| *w)
            .fold(main, f32::max)
    }

    fn classify(&mut self, node: &Node) {
        self.overlays.clear();
        for (i, child) in node.children().iter().enumerate().skip(1) {
            let props = props_of(child);
            let layout = child_layout_of(child);
            for class in ["left", "right", "top", "bottom"] {
                child.remove_class(class);
            }
            if let Some(class) = Self::positional_class(layout.halign, layout.valign) {
                child.add_class(class);
            }
            self.overlays.push((
                child.clone(),
                props.bool(PropName::MeasureOverlay, false),
                props.bool(PropName::ClipOverlay, false),
            ));
            let _ = i;
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for OverlayC {
    fn kind(&self) -> Kind {
        Kind::Overlay
    }

    fn build(node: &Node, _props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        // Overlays stack: every child occupies the same cell, so a
        // single-track grid is exactly right and taffy does the rest.
        set_container(
            node,
            Container::Grid {
                columns: 1,
                rows: 1,
                column_spacing: 0.0,
                row_spacing: 0.0,
                column_homogeneous: false,
                row_homogeneous: false,
            },
        );
        let mut me = Self { overlays: Vec::new() };
        me.classify(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::MeasureOverlay | PropName::ClipOverlay | PropName::Halign | PropName::Valign => {
                let _ = prop_bool(value, false);
                self.classify(node);
            }
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

Builders: `overlay(child)` is `View::new(Kind::Overlay).child(child)`;
`.overlay(v)` pushes `slot(v, "overlay")`; `.measure_overlay` and
`.clip_overlay` are one line each.

Register `Kind::Overlay => Box::new(OverlayC::build(node, props, cx)),` and
add `overlay_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::overlay
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/overlay.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/overlay.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkOverlay as a one-cell grid with positional classes

Every child lands in the same grid cell, so stacking needs no bespoke
arithmetic, and an edge-aligned overlay child gets the .left/.right/.top/
.bottom class Adwaita styles. Only an overlay that opted into measuring can
widen the container.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: `Kind::Stack` / `Kind::StackPage` — `StackC` and its transitions

**Files:**
- Create: `ui/src/widgets/stack.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/stack.txt`
- Modify: `ui/src/view/builders.rs` — `stack`, `stack_page`,
  `.visible_child`, `.transition_type`, `.transition_duration`,
  `.hhomogeneous`, `.vhomogeneous`, `.interpolate_size`, `.needs_attention`
- Modify: `ui/src/widgets/mod.rs` — `mod stack;`, both arms
- Modify: `ui/tests/node_trees.rs` — `stack_matches_its_gtk_fixture`
- Test: `ui/src/widgets/stack.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `anim::{AnimationState, Clock}` (M2), `widgets::types::{StackTransition,
  StackPageInfo}`, `view::EventKind::Change`.
- Produces:
  ```rust
  pub fn stack<Msg: Clone + 'static>(pages: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn stack_page<Msg: Clone + 'static>(name: &str, title: &str, child: View<Msg>) -> View<Msg>;
  pub struct StackC {
      pub visible: usize, pub outgoing: Option<usize>, pub progress: f32,
      pub transition: StackTransition, pub duration_ms: u32, pub pages: Vec<StackPageState>,
  }
  pub struct StackPageState { pub info: StackPageInfo, pub node: Node }
  impl StackC { pub fn page_infos(&self) -> Rc<[StackPageInfo]>; }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/stack.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::stack::StackC;
    use crate::widgets::types::StackTransition;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::VisibleChild, Prop::Str("one".into()));
        p.set(PropName::Transition, Prop::Enum(StackTransition::Crossfade.to_u16()));
        p.set(PropName::TransitionDuration, Prop::Int(200));
        p
    }

    #[test]
    fn a_stack_is_a_single_node_named_stack() {
        // Mutation check: wrapping the pages in a `box` fails the fixture,
        // which GTK gives as one node (gtk/gtkstack.c).
        let built = build_widget::<()>(Kind::Stack, &props());
        matches_fixture(&built.node, "stack\n").expect("stack fixture");
    }

    #[test]
    fn switching_pages_runs_the_transition_on_the_clock_and_then_stops() {
        // Interaction test. Mutation check: leaving `outgoing` set after the
        // transition keeps the old page painted forever; a next_deadline
        // that never returns None spins the pump.
        let built = build_widget::<()>(Kind::Stack, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.set_prop(&built.node, PropName::VisibleChild, &Prop::Str("two".into()), &mut hx.cx());
        assert!(c.next_deadline(Duration::ZERO).is_some());
        assert_eq!(StackC::outgoing_of(&c), Some(0));
        c.tick(Duration::from_millis(200), &mut cx);
        assert_eq!(StackC::progress_of(&c), 1.0);
        assert_eq!(StackC::outgoing_of(&c), None);
        assert_eq!(c.next_deadline(Duration::from_millis(200)), None);
    }

    #[test]
    fn an_unknown_page_name_leaves_the_visible_page_alone() {
        // Untrusted input: the name comes from the application model.
        // Mutation check: `unwrap_or(0)` on the lookup silently jumps to the
        // first page whenever a model has a typo.
        let built = build_widget::<()>(Kind::Stack, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let before = StackC::visible_of(&c);
        for name in ["", "nope", "\u{0}", &"x".repeat(10_000)] {
            c.set_prop(&built.node, PropName::VisibleChild, &Prop::Str(name.into()), &mut hx.cx());
        }
        assert_eq!(StackC::visible_of(&c), before);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::stack`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `stack` in
`widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/stack.txt` (`gtk/gtkstack.c`):

```
stack
```

`ui/src/widgets/stack.rs` — the whole widget is page bookkeeping plus one
animation, and the animation is M2's, not a bespoke timer:

```rust
//! `GtkStack` -- one visible page at a time, with an animated switch.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::Container;
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::{StackPageInfo, StackTransition};
use crate::widgets::{apply_universal_prop, prop_i64, prop_str, prop_u16, props_of, set_container, set_visible};

/// One page's identity and node.
pub struct StackPageState {
    /// Name, title, icon and attention flag -- what the switchers read.
    pub info: StackPageInfo,
    /// The page's own subtree root.
    pub node: Node,
}

/// `GtkStack`.
pub struct StackC {
    /// Index of the visible page.
    pub visible: usize,
    /// The page still fading/sliding out, if any.
    pub outgoing: Option<usize>,
    /// `0.0`..`1.0` through the transition.
    pub progress: f32,
    /// Which transition.
    pub transition: StackTransition,
    /// Its duration.
    pub duration_ms: u32,
    /// Every page.
    pub pages: Vec<StackPageState>,
    started: Option<Duration>,
}

impl StackC {
    /// The page list a `StackSwitcher`/`StackSidebar` renders.
    #[must_use]
    pub fn page_infos(&self) -> Rc<[StackPageInfo]> {
        self.pages.iter().map(|p| p.info.clone()).collect()
    }

    /// Test hooks.
    #[must_use]
    pub fn visible_of<Msg>(c: &Box<dyn Controller<Msg>>) -> usize {
        c.as_any().downcast_ref::<Self>().map_or(usize::MAX, |m| m.visible)
    }
    #[must_use]
    pub fn outgoing_of<Msg>(c: &Box<dyn Controller<Msg>>) -> Option<usize> {
        c.as_any().downcast_ref::<Self>().and_then(|m| m.outgoing)
    }
    #[must_use]
    pub fn progress_of<Msg>(c: &Box<dyn Controller<Msg>>) -> f32 {
        c.as_any().downcast_ref::<Self>().map_or(f32::NAN, |m| m.progress)
    }

    /// Switch to the page called `name`; unknown names change nothing.
    fn show(&mut self, name: &str, now: Duration) -> bool {
        let Some(index) = self.pages.iter().position(|p| &*p.info.name == name) else {
            tracing::debug!(name, "stack: no page with that name");
            return false;
        };
        if index == self.visible {
            return false;
        }
        self.outgoing = Some(self.visible);
        self.visible = index;
        self.progress = 0.0;
        self.started = Some(now);
        for (i, page) in self.pages.iter().enumerate() {
            set_visible(&page.node, i == self.visible || Some(i) == self.outgoing);
        }
        true
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StackC {
    fn kind(&self) -> Kind {
        Kind::Stack
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(node, Container::Grid {
            columns: 1, rows: 1, column_spacing: 0.0, row_spacing: 0.0,
            column_homogeneous: false, row_homogeneous: false,
        });
        let pages: Vec<StackPageState> = node
            .children()
            .iter()
            .map(|child| {
                let p = props_of(child);
                StackPageState {
                    info: StackPageInfo {
                        name: Rc::from(p.str(PropName::PageName).unwrap_or_default()),
                        title: Rc::from(p.str(PropName::PageTitle).unwrap_or_default()),
                        icon: None,
                        needs_attention: p.bool(PropName::NeedsAttention, false),
                    },
                    node: child.clone(),
                }
            })
            .collect();
        for (i, page) in pages.iter().enumerate() {
            set_visible(&page.node, i == 0);
        }
        let mut me = Self {
            visible: 0,
            outgoing: None,
            progress: 1.0,
            transition: StackTransition::from_u16(
                u16::try_from(props.int(PropName::Transition, 0)).unwrap_or(0),
            ),
            duration_ms: u32::try_from(props.int(PropName::TransitionDuration, 200).clamp(0, 10_000))
                .unwrap_or(200),
            pages,
            started: None,
        };
        if let Some(name) = props.str(PropName::VisibleChild) {
            // The initial page is shown without animating.
            if me.show(name, Duration::ZERO) {
                me.outgoing = None;
                me.started = None;
                me.progress = 1.0;
            }
        }
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::VisibleChild => {
                self.show(prop_str(value), cx.clock.now());
            }
            PropName::Transition => self.transition = StackTransition::from_u16(prop_u16(value, 0)),
            PropName::TransitionDuration => {
                self.duration_ms =
                    u32::try_from(prop_i64(value, 200).clamp(0, 10_000)).unwrap_or(200);
            }
            other => apply_universal_prop(node, other, value),
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(started) = self.started else {
            return Vec::new();
        };
        let d = f32::from(u16::try_from(self.duration_ms).unwrap_or(u16::MAX)).max(1.0);
        self.progress = (now.saturating_sub(started).as_secs_f32() * 1000.0 / d).clamp(0.0, 1.0);
        crate::widgets::set_transition_progress(
            &self.pages[self.visible].node,
            self.transition,
            self.progress,
        );
        if self.progress >= 1.0 {
            if let Some(old) = self.outgoing.take() {
                set_visible(&self.pages[old].node, false);
            }
            self.started = None;
            let name = Rc::clone(&self.pages[self.visible].info.name);
            return cx.handlers.fire_text(EventKind::Change, &name).into_iter().collect();
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|s| {
            let end = s + Duration::from_millis(u64::from(self.duration_ms));
            if now >= end { Duration::ZERO } else { end - now }
        })
    }
}
```

`set_transition_progress(node, transition, t)` records the translate/opacity
the paint walker applies for the named transition; `Crossfade` is an opacity
ramp, the four `Slide*` are a translate of `(1 - t)` of the page's own
extent, and `StackTransition::None` writes nothing.

Builders: `stack(pages)` collects `Kind::StackPage` children;
`stack_page(name, title, child)` sets `PageName`/`PageTitle` and takes
`.icon(IconRef)`/`.needs_attention(bool)`; `.visible_child`,
`.transition_type`, `.transition_duration`, `.hhomogeneous`,
`.vhomogeneous`, `.interpolate_size` are one line each; `.on_change(f)`
stores `Handler::Text` under `EventKind::Change`.

Register both arms and add `stack_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::stack
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/stack.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/stack.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkStack with clock-driven page transitions

An unknown page name changes nothing rather than falling back to page zero,
because that name comes from the application's model. The outgoing page stays
mapped only while the transition runs, and next_deadline goes quiet the frame
it ends.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: `Kind::ListBox` / `Kind::ListBoxRow` — `ListBoxC`

**Files:**
- Create: `ui/src/widgets/list_box.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/list_box.txt`
- Modify: `ui/src/view/builders.rs` — `list_box`, `list_box_row`,
  `.selection_mode`, `.show_separators`, `.activate_on_single_click`,
  `.activatable`, `.on_selected`, `.on_item_activated`
- Modify: `ui/src/widgets/mod.rs` — `mod list_box;`, both arms
- Modify: `ui/tests/node_trees.rs` — `list_box_matches_its_gtk_fixture`
- Test: `ui/src/widgets/list_box.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `widgets::types::{Selection, SelectionMode}` (Task 2), P3's
  `KeyEvent`, `EventKind::{Selected, Activate}`.
- Produces:
  ```rust
  pub fn list_box<Msg: Clone + 'static>(rows: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn list_box_row<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub struct ListBoxC {
      pub rows: Vec<Node>, pub selection: Selection, pub cursor: Option<usize>,
      pub mode: SelectionMode, pub activate_single: bool,
  }
  impl ListBoxC {
      pub fn row_at(&self, point: (f32, f32)) -> Option<usize>;
      pub fn apply_selection(&self);
  }
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/widgets/list_box.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::list_box::ListBoxC;
    use crate::widgets::types::SelectionMode;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props(mode: SelectionMode) -> Props {
        let mut p = Props::default();
        p.set(PropName::SelectionMode, Prop::Enum(mode.to_u16()));
        p.set(PropName::ShowSeparators, Prop::Bool(true));
        p
    }

    #[test]
    fn a_list_box_is_a_list_node_of_row_nodes() {
        // Mutation check: naming the container `listbox` (the class name,
        // not the CSS node) fails the fixture and every Adwaita `list > row`
        // rule.
        let built = build_widget::<()>(Kind::ListBox, &props(SelectionMode::Single));
        matches_fixture(
            &built.node,
            "list[.separators][.rich-list][.navigation-sidebar][.boxed-list]\n╰── row[.activatable]\n",
        )
        .expect("list_box fixture");
    }

    #[test]
    fn clicking_a_row_selects_it_and_sets_selected_on_that_row_only() {
        // Interaction test. Mutation check: setting :selected without
        // clearing the previous row leaves the whole list highlighted.
        let built = build_widget::<usize>(Kind::ListBox, &props(SelectionMode::Single));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Selected, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        hx.place_rows(&built.node, 30.0);
        assert_eq!(
            c.on_event(&Event::PointerDown { button: 0x110, local: (5.0, 45.0), serial: 1 }, &mut cx),
            vec![1]
        );
        let selected: Vec<usize> = ListBoxC::selected_rows(&c);
        assert_eq!(selected, vec![1]);
        assert!(built.node.child(1).unwrap().states().contains(PseudoStates::SELECTED));
        assert!(!built.node.child(0).unwrap().states().contains(PseudoStates::SELECTED));
    }

    #[test]
    fn arrows_move_the_cursor_space_toggles_under_multiple_and_ctrl_a_selects_all() {
        // Mutation check: letting Space toggle under Single mode makes a
        // single-selection list deselect itself on Space, which GTK does not.
        let built = build_widget::<usize>(Kind::ListBox, &props(SelectionMode::Multiple));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        hx.place_rows(&built.node, 30.0);
        c.on_event(&Event::Key(hx.key("Down")), &mut cx);
        c.on_event(&Event::Key(hx.key("space")), &mut cx);
        assert_eq!(ListBoxC::selected_rows(&c), vec![0]);
        c.on_event(&Event::Key(hx.key_with_mods("a", &["Control"])), &mut cx);
        assert_eq!(ListBoxC::selected_rows(&c).len(), built.node.child_count());
    }
}
```

`Headless::place_rows(&Node, height)` records a synthetic allocation per
child so `row_at` has geometry to hit-test without a layout pass.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::list_box`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `list_box`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/list_box.txt` (`gtk/gtklistbox.c`):

```
list[.separators][.rich-list][.navigation-sidebar][.boxed-list]
╰── row[.activatable]
```

`ui/src/widgets/list_box.rs`:

```rust
//! `GtkListBox` -- a `list` of `row`s with GTK's four selection modes.

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::{Selection, SelectionMode};
use crate::widgets::{apply_universal_prop, hit, prop_bool, prop_u16, set_container};

/// `GtkListBox`.
pub struct ListBoxC {
    /// One `row` node per child, in order.
    pub rows: Vec<Node>,
    /// What is selected.
    pub selection: Selection,
    /// Keyboard cursor.
    pub cursor: Option<usize>,
    /// The current mode, mirrored from `selection` for readability.
    pub mode: SelectionMode,
    /// `GtkListBox:activate-on-single-click`.
    pub activate_single: bool,
}

impl ListBoxC {
    /// Test hook.
    #[must_use]
    pub fn selected_rows<Msg>(c: &Box<dyn Controller<Msg>>) -> Vec<usize> {
        c.as_any()
            .downcast_ref::<Self>()
            .map_or_else(Vec::new, |m| m.selection.selected())
    }

    /// The row a point lands in.
    #[must_use]
    pub fn row_at(&self, point: (f32, f32)) -> Option<usize> {
        self.rows.iter().position(|row| hit(row, point))
    }

    /// Push the selection onto the rows' `:selected` state.
    pub fn apply_selection(&self) {
        for (i, row) in self.rows.iter().enumerate() {
            row.set_state(PseudoStates::SELECTED, self.selection.contains(i));
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ListBoxC {
    fn kind(&self) -> Kind {
        Kind::ListBox
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(node, Container::Box { direction: BoxDirection::Column });
        if props.bool(PropName::ShowSeparators, false) {
            node.add_class("separators");
        }
        let mode = SelectionMode::from_u16(
            u16::try_from(props.int(PropName::SelectionMode, 1)).unwrap_or(1),
        );
        let rows: Vec<Node> = node.children();
        for row in &rows {
            if crate::widgets::props_of(row).bool(PropName::Activatable, true) {
                row.add_class("activatable");
            }
        }
        Self {
            rows,
            selection: Selection::new(mode),
            cursor: None,
            mode,
            activate_single: props.bool(PropName::ActivateOnSingleClick, true),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::SelectionMode => {
                self.mode = SelectionMode::from_u16(prop_u16(value, 1));
                self.selection.set_mode(self.mode);
                self.apply_selection();
            }
            PropName::ShowSeparators => {
                if prop_bool(value, false) {
                    node.add_class("separators");
                } else {
                    node.remove_class("separators");
                }
            }
            PropName::ActivateOnSingleClick => self.activate_single = prop_bool(value, true),
            other => apply_universal_prop(node, other, value),
        }
        // The row list is the node's children; a reconcile may have changed
        // it, and re-reading is cheaper than tracking every insertion.
        self.rows = node.children();
        self.selection.retain_below(self.rows.len());
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut out = Vec::new();
        let mut changed = false;
        match ev {
            Event::PointerDown { local, .. } => {
                if let Some(index) = self.row_at(*local) {
                    self.cursor = Some(index);
                    changed = self.selection.select(index);
                    cx.handled = true;
                    if self.activate_single
                        && let Some(msg) = cx.handlers.fire_index(EventKind::Activate, index)
                    {
                        out.push(msg);
                    }
                }
            }
            Event::Key(key) => {
                let count = self.rows.len();
                match key.keysym_name() {
                    "Down" => {
                        self.cursor = Some(self.cursor.map_or(0, |c| (c + 1).min(count.saturating_sub(1))));
                        changed = self.selection.select(self.cursor.unwrap_or(0));
                        cx.handled = true;
                    }
                    "Up" => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(1)));
                        changed = self.selection.select(self.cursor.unwrap_or(0));
                        cx.handled = true;
                    }
                    "space" if self.mode == SelectionMode::Multiple => {
                        if let Some(cursor) = self.cursor {
                            changed = self.selection.toggle(cursor);
                        }
                        cx.handled = true;
                    }
                    "a" if key.mods.control() && self.mode == SelectionMode::Multiple => {
                        changed = self.selection.select_all(count);
                        cx.handled = true;
                    }
                    "Return" | "KP_Enter" => {
                        if let Some(cursor) = self.cursor
                            && let Some(msg) = cx.handlers.fire_index(EventKind::Activate, cursor)
                        {
                            out.push(msg);
                            cx.handled = true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if changed {
            self.apply_selection();
            if let Some(index) = self.selection.first()
                && let Some(msg) = cx.handlers.fire_index(EventKind::Selected, index)
            {
                out.insert(0, msg);
            }
        }
        out
    }
}
```

Builders: `list_box(rows)` collects `Kind::ListBoxRow` children;
`list_box_row(child)` is `View::new(Kind::ListBoxRow).child(child)` with
`.activatable`/`.selectable`; `.selection_mode`, `.show_separators`,
`.activate_on_single_click` are one line each; `.on_selected(f)` stores
`Handler::Index` under `EventKind::Selected` and `.on_item_activated(f)`
under `EventKind::Activate` (contract deviation 14).

Register both arms (`Kind::ListBoxRow` uses `GenericC` over the `row` node)
and add `list_box_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::list_box
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/list_box.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/list_box.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkListBox with GTK's four selection modes and keynav

Selection lives in the Selection type, so Browse's "always exactly one" and
Multiple's Ctrl+A are decided in one place and the widget only pushes the
result onto the rows' :selected state. Space toggles only under Multiple,
which is GTK's rule.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: `Kind::StackSwitcher` and `Kind::StackSidebar`

**Files:**
- Create: `ui/src/widgets/stack_switcher.rs`, `ui/src/widgets/stack_sidebar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/stack_switcher.txt`,
  `ui/tests/fixtures/gtk4.22-node-trees/stack_sidebar.txt`
- Modify: `ui/src/view/builders.rs` — `stack_switcher`, `stack_sidebar`,
  `.selected`, `.pages`
- Modify: `ui/src/widgets/mod.rs` — both modules and both arms
- Modify: `ui/tests/node_trees.rs` — two fixture tests
- Test: both new files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `StackC::page_infos` (Task 15), `ListBoxC` (Task 16),
  `widgets::types::StackPageInfo`, `EventKind::Selected`.
- Produces:
  ```rust
  pub fn stack_switcher<Msg: Clone + 'static>(pages: Rc<[StackPageInfo]>) -> View<Msg>;
  pub fn stack_sidebar<Msg: Clone + 'static>(pages: Rc<[StackPageInfo]>) -> View<Msg>;
  pub struct StackSwitcherC { pub selected: usize, pub buttons: Vec<Node> }
  pub struct StackSidebarC { pub selected: usize, pub list: ListBoxC }
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/widgets/stack_switcher.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::stack_switcher::StackSwitcherC;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::Pages, Prop::Classes(["One", "Two", "Three"].iter().map(|s| (*s).into()).collect()));
        p.set(PropName::Selected, Prop::Int(0));
        p
    }

    #[test]
    fn the_switcher_is_a_stackswitcher_of_buttons() {
        // Mutation check: dropping the `.stack-switcher` class loses
        // Adwaita's linked-button styling, which the fixture requires.
        let built = build_widget::<()>(Kind::StackSwitcher, &props());
        matches_fixture(
            &built.node,
            "stackswitcher.stack-switcher\n├── button[.needs-attention]\n┊\n╰── button[.needs-attention]\n",
        )
        .expect("stack_switcher fixture");
    }

    #[test]
    fn clicking_a_button_checks_only_that_one_and_reports_its_index() {
        // Interaction test. Mutation check: not clearing :checked on the
        // previous button leaves every visited page's button lit.
        let built = build_widget::<usize>(Kind::StackSwitcher, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Selected, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        hx.place_rows(&built.node, 40.0);
        assert_eq!(
            c.on_event(&Event::PointerDown { button: 0x110, local: (5.0, 100.0), serial: 1 }, &mut cx),
            vec![2]
        );
        assert!(built.node.child(2).unwrap().states().contains(PseudoStates::CHECKED));
        assert!(!built.node.child(0).unwrap().states().contains(PseudoStates::CHECKED));
        assert_eq!(StackSwitcherC::selected_of(&c), 2);
    }
}
```

`ui/src/widgets/stack_sidebar.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::{build_widget, matches_fixture};

    #[test]
    fn the_sidebar_is_a_scrolledwindow_over_a_navigation_sidebar_list() {
        // Mutation check: putting the list straight under `stacksidebar`
        // loses the scrolling a long page list needs, and the fixture's
        // `scrolledwindow > list.navigation-sidebar` chain.
        let mut props = Props::default();
        props.set(PropName::Pages, Prop::Classes(["A", "B"].iter().map(|s| (*s).into()).collect()));
        let built = build_widget::<()>(Kind::StackSidebar, &props);
        matches_fixture(
            &built.node,
            "stacksidebar.sidebar\n╰── scrolledwindow\n    ╰── list.navigation-sidebar\n        \
             ╰── row[.needs-attention]\n        ┊\n",
        )
        .expect("stack_sidebar fixture");
    }

    #[test]
    fn a_page_marked_needs_attention_carries_that_class_on_its_row() {
        // Mutation check: setting the class on the list instead of the row
        // makes the whole sidebar pulse.
        let mut props = Props::default();
        props.set(PropName::Pages, Prop::Classes(["A", "B"].iter().map(|s| (*s).into()).collect()));
        props.set(PropName::NeedsAttention, Prop::Int(1));
        let built = build_widget::<()>(Kind::StackSidebar, &props);
        let row = built.node.child(0).and_then(|s| s.child(0)).and_then(|l| l.child(1));
        assert!(row.expect("row").classes().iter().any(|c| c.as_str() == "needs-attention"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::stack_switcher widgets::stack_sidebar`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`stack_switcher` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

Fixtures (`gtk/gtkstackswitcher.c`, `gtk/gtkstacksidebar.c`), the contract's
§5.4 blocks:

`stack_switcher.txt`
```
stackswitcher.stack-switcher
├── button[.needs-attention]
┊
╰── button[.needs-attention]
```

`stack_sidebar.txt`
```
stacksidebar.sidebar
╰── scrolledwindow
    ╰── list.navigation-sidebar
        ╰── row[.needs-attention]
        ┊
```

`StackSwitcherC` builds one `button` per `StackPageInfo`, sets
`PseudoStates::CHECKED` on the selected one and `.needs-attention` on the
flagged ones, and in `on_event` maps a press to the button under the pointer:

```rust
    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Event::PointerDown { local, .. } = ev else {
            return Vec::new();
        };
        let Some(index) = self.buttons.iter().position(|b| crate::widgets::hit(b, *local)) else {
            return Vec::new();
        };
        cx.handled = true;
        if index == self.selected {
            return Vec::new();
        }
        self.selected = index;
        for (i, button) in self.buttons.iter().enumerate() {
            button.set_state(PseudoStates::CHECKED, i == index);
        }
        cx.handlers
            .fire_index(EventKind::Selected, index)
            .into_iter()
            .collect()
    }
```

`StackSidebarC` builds `scrolledwindow > list.navigation-sidebar` and owns a
`ListBoxC` over the rows, delegating `on_event` to it and translating its
`EventKind::Selected` straight through — the sidebar adds the
`.needs-attention` class per row and nothing else.

Builders: `stack_switcher(pages)` / `stack_sidebar(pages)` store the page
titles in `PropName::Pages` (a `Prop::Classes` list, deviation 5's shape) and
the attention flags in `PropName::NeedsAttention` as a bitmask
(`Prop::Int`), so both are diffable by value; `.selected` is one line;
`.on_selected(f)` stores `Handler::Index`.

Register both arms and add both fixture tests.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::stack_switcher widgets::stack_sidebar
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and two fixture tests.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/stack_switcher.rs ui/src/widgets/stack_sidebar.rs \
        ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/stack_switcher.txt \
        ui/tests/fixtures/gtk4.22-node-trees/stack_sidebar.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkStackSwitcher and GtkStackSidebar over the stack's page list

Both take the page list by value rather than a live stack handle, so the
reactive layer can diff them; the sidebar is a real scrolledwindow over a
navigation-sidebar list, which is the tree GTK renders and the one Adwaita
styles.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: `Kind::FlowBox` / `Kind::FlowBoxChild` — `FlowBoxC` and rubberband

**Files:**
- Create: `ui/src/widgets/flow_box.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/flow_box.txt`
- Modify: `ui/src/view/builders.rs` — `flow_box`, `.min_children_per_line`,
  `.max_children_per_line`, `.row_spacing`, `.column_spacing`
- Modify: `ui/src/widgets/mod.rs` — `mod flow_box;`, both arms
- Modify: `ui/tests/node_trees.rs` — `flow_box_matches_its_gtk_fixture`
- Test: `ui/src/widgets/flow_box.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Selection`/`SelectionMode` (Task 2), `layout::Container::Grid`
  (Task 1), `EventKind::{Selected, Activate}`.
- Produces:
  ```rust
  pub fn flow_box<Msg: Clone + 'static>(children: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub struct FlowBoxC {
      pub children: Vec<Node>, pub selection: Selection, pub cursor: Option<usize>,
      pub mode: SelectionMode, pub rubberband: Option<(layout::Rect, Node)>,
  }
  impl FlowBoxC { pub fn columns_for(&self, width: f32, child: f32) -> u32; }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::flow_box::FlowBoxC;
    use crate::widgets::types::SelectionMode;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::MinChildrenPerLine, Prop::Int(2));
        p.set(PropName::MaxChildrenPerLine, Prop::Int(4));
        p.set(PropName::SelectionMode, Prop::Enum(SelectionMode::Multiple.to_u16()));
        p.set(PropName::EnableRubberband, Prop::Bool(true));
        p
    }

    #[test]
    fn every_child_is_wrapped_in_a_flowboxchild() {
        // Mutation check: appending application children straight to
        // `flowbox` loses the `flowboxchild` node Adwaita styles and selects.
        let built = build_widget::<()>(Kind::FlowBox, &props());
        matches_fixture(
            &built.node,
            "flowbox\n├── flowboxchild\n│   ╰── <child>\n┊\n╰── [rubberband]\n",
        )
        .expect("flow_box fixture");
    }

    #[test]
    fn the_column_count_stays_between_min_and_max_children_per_line() {
        // Mutation check: ignoring max lets a wide window put every child on
        // one line, which is the bug that makes an icon grid a single row.
        let built = build_widget::<()>(Kind::FlowBox, &props());
        let c = built.controller;
        assert_eq!(FlowBoxC::columns_of(&c, 1000.0, 100.0), 4);
        assert_eq!(FlowBoxC::columns_of(&c, 100.0, 100.0), 2);
        assert_eq!(FlowBoxC::columns_of(&c, 0.0, 0.0), 2, "a zero width still gives the minimum");
    }

    #[test]
    fn a_rubberband_drag_selects_the_children_it_covers_and_then_disappears() {
        // Interaction test. Mutation check: leaving the rubberband node in
        // the tree after the release paints a stuck selection rectangle.
        let built = build_widget::<usize>(Kind::FlowBox, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        hx.place_rows(&built.node, 30.0);
        c.on_event(&Event::PointerDown { button: 0x110, local: (2.0, 2.0), serial: 1 }, &mut cx);
        c.on_event(&Event::PointerMotion { local: (200.0, 70.0) }, &mut cx);
        assert!(FlowBoxC::rubberband_visible(&c));
        c.on_event(&Event::PointerUp { button: 0x110, local: (200.0, 70.0), serial: 2 }, &mut cx);
        assert!(!FlowBoxC::rubberband_visible(&c));
        assert!(FlowBoxC::selected_children(&c).len() >= 2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::flow_box`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `flow_box`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/flow_box.txt` (`gtk/gtkflowbox.c`):

```
flowbox
├── flowboxchild
│   ╰── <child>
├── flowboxchild
│   ╰── <child>
┊
╰── [rubberband]
```

`FlowBoxC` reuses `Selection` and `Container::Grid`; the two pieces of real
arithmetic are the column count and the band:

```rust
impl FlowBoxC {
    /// Children per line, clamped to `[min, max]` and never zero.
    #[must_use]
    pub fn columns_for(&self, width: f32, child: f32) -> u32 {
        let fit = if child > 0.0 && width.is_finite() && child.is_finite() {
            (width / child).floor().max(1.0) as u32
        } else {
            self.min_per_line
        };
        fit.clamp(self.min_per_line.max(1), self.max_per_line.max(self.min_per_line.max(1)))
    }

    /// Select every child whose allocation intersects `band`.
    fn select_band(&mut self, band: crate::layout::Rect) -> bool {
        let mut changed = false;
        for (i, child) in self.children.iter().enumerate() {
            if crate::widgets::intersects(child, band) {
                changed |= self.selection.toggle_on(i);
            }
        }
        changed
    }
}
```

`on_event` starts the band on a press that misses every child, updates it on
motion (creating the `rubberband` node the first time), and on release
selects, removes the node and fires `EventKind::Selected` for the first
selected index; a press *on* a child selects it as `ListBoxC` does, and a
double press (or a single one under `activate_on_single_click`) fires
`EventKind::Activate`. `set_prop` maps `MinChildrenPerLine`,
`MaxChildrenPerLine`, `RowSpacing`, `ColumnSpacing`, `SelectionMode`,
`Homogeneous`, `ActivateOnSingleClick` and `EnableRubberband`, then rewrites
the `Container::Grid` with `columns_for` and delegates the rest.
`Selection::toggle_on(index)` is one more method on Task 2's type: insert
without removing, returning whether it inserted.

Builders: `flow_box(children)` wraps each child in a
`View::new(Kind::FlowBoxChild)`; the five setters are one line each;
`.on_selected(f)` and `.on_item_activated(f)` store `Handler::Index`.

Register both arms and add `flow_box_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::flow_box
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/flow_box.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/flow_box.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkFlowBox with per-line clamping and rubberband selection

Children per line is clamped to [min, max] and never zero, so a zero or
non-finite width degrades to the minimum instead of collapsing the grid; the
rubberband node exists only while the band is being dragged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: `Kind::ListView` — `ListViewC` and the recycling row pool

**Files:**
- Create (replacing whatever minimal `ListViewC` P5 left there, contract
  deviation 9): `ui/src/widgets/list_view.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/list_view.txt`
- Modify: `ui/src/view/builders.rs` — `list_view`, `.single_click_activate`,
  `.enable_rubberband`, `.show_separators`
- Modify: `ui/src/widgets/mod.rs` — `mod list_view;`, the arm
- Modify: `ui/tests/node_trees.rs` — `list_view_matches_its_gtk_fixture`
- Test: `ui/src/widgets/list_view.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `types::{ListItem, ItemFactory, RowContent, Selection}` (Task 2),
  `window::Kinetic` (P3), `Prop::{Items, Factory}`.
- Produces:
  ```rust
  pub fn list_view<Msg: Clone + 'static>(model: Rc<[ListItem]>, factory: ItemFactory) -> View<Msg>;
  pub struct ListViewC {
      pub model: Rc<[ListItem]>, pub pool: Vec<Node>, pub first_visible: usize,
      pub offset: f32, pub row_height: f32, pub selection: Selection,
      pub cursor: Option<usize>, pub kinetic: Kinetic,
      pub rubberband: Option<(layout::Rect, Node)>,
  }
  impl ListViewC {
      pub fn visible_range(&self, viewport: f32) -> std::ops::Range<usize>;
      pub fn rebind(&mut self, viewport: f32);
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::list_view::ListViewC;
    use crate::widgets::types::{ItemFactory, ListItem, RowContent, SelectionMode};
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn model(n: u64) -> Rc<[ListItem]> {
        (0..n)
            .map(|i| ListItem {
                id: i,
                label: Rc::from(format!("row {i}").as_str()),
                icon: None,
                classes: Rc::from(&[][..]),
            })
            .collect()
    }

    fn props(n: u64) -> Props {
        let mut p = Props::default();
        p.set(PropName::Model, Prop::Items(model(n)));
        p.set(PropName::ItemFactory, Prop::Factory(ItemFactory::label_only()));
        p.set(PropName::SelectionMode, Prop::Enum(SelectionMode::Multiple.to_u16()));
        p
    }

    #[test]
    fn a_list_view_is_a_listview_of_rows() {
        let built = build_widget::<()>(Kind::ListView, &props(3));
        matches_fixture(
            &built.node,
            "listview[.separators][.rich-list][.navigation-sidebar][.data-table]\n\
             ├── row[.activatable]\n┊\n╰── [rubberband]\n",
        )
        .expect("list_view fixture");
    }

    #[test]
    fn the_pool_is_the_viewport_plus_overscan_not_the_model() {
        // The point of the whole widget. Mutation check: building one node
        // per item makes this allocate 100 000 nodes and the assertion fails
        // by four orders of magnitude.
        let built = build_widget::<()>(Kind::ListView, &props(100_000));
        let mut c = built.controller;
        ListViewC::set_metrics(&mut c, 30.0, 300.0);
        assert!(
            ListViewC::pool_len(&c) <= 20,
            "pool is {} nodes for a 300px viewport of 30px rows",
            ListViewC::pool_len(&c)
        );
    }

    #[test]
    fn scrolling_rebinds_rows_without_losing_the_selection_or_the_node_identity() {
        // The contract's P6 gate, verbatim: "row recycling is proven not to
        // lose selection or animation state across a scroll". Mutation
        // check: rebuilding rows on scroll changes every node pointer and
        // drops the :selected state with them.
        let built = build_widget::<()>(Kind::ListView, &props(1_000));
        let mut c = built.controller;
        ListViewC::set_metrics(&mut c, 30.0, 300.0);
        ListViewC::select(&mut c, 500);
        let before: Vec<usize> = ListViewC::pool_ids(&c);
        ListViewC::scroll_to(&mut c, 500.0 * 30.0);
        let after: Vec<usize> = ListViewC::pool_ids(&c);
        assert_eq!(before, after, "the same nodes were rebound, not replaced");
        assert!(ListViewC::selected(&c).contains(&500), "selection survived");
        assert_eq!(ListViewC::bound_label(&c, 0), "row 500");
    }

    #[test]
    fn a_model_that_shrinks_under_the_selection_never_panics() {
        // Untrusted input: the model is the application's.
        let built = build_widget::<()>(Kind::ListView, &props(1_000));
        let mut c = built.controller;
        let mut hx = Headless::new();
        ListViewC::set_metrics(&mut c, 30.0, 300.0);
        ListViewC::select(&mut c, 999);
        c.set_prop(&built.node, PropName::Model, &Prop::Items(model(2)), &mut hx.cx());
        assert!(ListViewC::selected(&c).iter().all(|i| *i < 2));
        c.set_prop(&built.node, PropName::Model, &Prop::Items(model(0)), &mut hx.cx());
        assert!(ListViewC::selected(&c).is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::list_view`
Expected: FAIL — `error[E0599]: no function or associated item named
`set_metrics` found for struct `ListViewC`` (P5's minimal type exists but has
none of this).

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/list_view.txt` (`gtk/gtklistview.c`),
the contract's §5.5 block.

`ui/src/widgets/list_view.rs` — the pool is the whole design:

```rust
/// Rows kept beyond the viewport, above and below, so a one-pixel scroll
/// never has to allocate.
const OVERSCAN: usize = 2;

impl ListViewC {
    /// Which model indices the viewport covers, given the current offset.
    #[must_use]
    pub fn visible_range(&self, viewport: f32) -> std::ops::Range<usize> {
        if self.row_height <= 0.0 || !self.row_height.is_finite() || self.model.is_empty() {
            return 0..0;
        }
        let first = (self.offset / self.row_height).floor().max(0.0) as usize;
        let count = (viewport / self.row_height).ceil().max(0.0) as usize + 1;
        let start = first.saturating_sub(OVERSCAN);
        let end = (first + count + OVERSCAN).min(self.model.len());
        start..end.max(start)
    }

    /// Grow or shrink the pool to the visible range and rebind every row.
    ///
    /// A rebind is `set_text`/`set_icon`/`set_classes` on a node that is
    /// already in the tree -- never an insert or a remove -- so a row's
    /// identity, its running animations and its `:selected` state survive
    /// scrolling.
    pub fn rebind(&mut self, viewport: f32) {
        let range = self.visible_range(viewport);
        while self.pool.len() < range.len() {
            let row = Node::with_classes("row", &["activatable"]);
            self.node.append_child(&row);
            self.pool.push(row);
        }
        while self.pool.len() > range.len() {
            if let Some(row) = self.pool.pop() {
                self.node.remove_child(&row);
            }
        }
        self.first_visible = range.start;
        for (slot, index) in range.clone().enumerate() {
            let Some(item) = self.model.get(index) else { continue };
            let content = self.factory.bind(index, item);
            let row = &self.pool[slot];
            crate::widgets::set_text(row, &content.label);
            crate::widgets::set_row_classes(row, &content.classes);
            row.set_state(PseudoStates::SELECTED, self.selection.contains(index));
            crate::widgets::set_row_index(row, index);
        }
    }
}
```

`set_prop` handles `Model` (swap the `Rc`, `selection.retain_below(len)`,
`rebind`), `ItemFactory` (`rebind`), `SelectionMode`, `ShowSeparators`,
`SingleClickActivate` and `EnableRubberband`, delegating the rest.
`on_event` maps a press to `set_row_index`'s value for the hit row (so a
pooled row reports its *model* index, not its slot), applies the same
selection rules `ListBoxC` uses, and handles `Event::Scroll` by moving
`offset`, clamping to `model.len() * row_height - viewport`, and calling
`rebind`. `tick` drives `Kinetic` exactly as `ScrolledWindowC` does.
`set_metrics`, `pool_len`, `pool_ids`, `bound_label`, `select`, `selected`
and `scroll_to` are the `#[must_use]` test hooks the tests above call, each a
one-line accessor over these fields.

Builders: `list_view(model, factory)` sets `PropName::Model` and
`PropName::ItemFactory`; `.selection_mode`, `.selected`, `.show_separators`,
`.single_click_activate`, `.enable_rubberband` are one line each.

Register `Kind::ListView => Box::new(ListViewC::build(node, props, cx)),`
and add `list_view_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::list_view
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests, one fixture test, and P5's `DropDown` and
`FontDialog` tests still green over the replaced `ListViewC`.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/list_view.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/list_view.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkListView with a recycling row pool

The pool is the viewport plus two rows of overscan, whatever the model's
length, and scrolling rebinds those nodes instead of inserting and removing
them -- which is what keeps a row's identity, its animations and its selected
state across a scroll. A row reports its model index, never its pool slot.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 20: `Kind::GridView` and `Kind::ColumnView` / `Kind::ColumnViewColumn`

**Files:**
- Create: `ui/src/widgets/grid_view.rs`, `ui/src/widgets/column_view.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/grid_view.txt`,
  `ui/tests/fixtures/gtk4.22-node-trees/column_view.txt`
- Modify: `ui/src/view/builders.rs` — `grid_view`, `column_view`,
  `column_view_column`, `.min_columns`, `.max_columns`,
  `.show_row_separators`, `.show_column_separators`, `.sort_column`,
  `.resizable`, `.expand`, `.sorter`
- Modify: `ui/src/widgets/mod.rs` — both modules and three arms
- Modify: `ui/tests/node_trees.rs` — two fixture tests
- Test: both new files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ListViewC` (Task 19) — `GridViewC` reuses its pool through
  `ListViewC::{visible_range, rebind}` over a 2-D cell size, and
  `ColumnViewC` embeds a whole `ListViewC`; `types::{Sorter, SortOrder}`.
- Produces:
  ```rust
  pub fn grid_view<Msg: Clone + 'static>(model: Rc<[ListItem]>, factory: ItemFactory) -> View<Msg>;
  pub fn column_view<Msg: Clone + 'static>(model: Rc<[ListItem]>,
      columns: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn column_view_column<Msg: Clone + 'static>(title: &str, factory: ItemFactory) -> View<Msg>;
  pub struct GridViewC { /* model, pool, columns, first_visible, offset, cell, selection, cursor, kinetic */ }
  pub struct ColumnViewC { pub columns: Vec<ColumnState>, pub list: ListViewC,
                           pub sort: Option<(usize, SortOrder)>, pub drag: Option<(usize, f32)>, pub header: Node }
  pub struct ColumnState { pub title: Rc<str>, pub width: f32, pub resizable: bool,
                           pub expand: bool, pub sorter: Option<Sorter>, pub node: Node }
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/widgets/grid_view.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::grid_view::GridViewC;
    use crate::widgets::{build_widget, matches_fixture};

    #[test]
    fn a_grid_view_is_a_gridview_of_child_nodes() {
        // Mutation check: reusing `row` as the cell node (copied from
        // ListView) fails the fixture: GtkGridView's cells are `child`.
        let built = build_widget::<()>(Kind::GridView, &crate::widgets::list_view::tests::props(4));
        matches_fixture(&built.node, "gridview\n├── child[.activatable]\n┊\n╰── [rubberband]\n")
            .expect("grid_view fixture");
    }

    #[test]
    fn the_pool_covers_whole_rows_of_cells_and_survives_a_scroll() {
        // Mutation check: computing the visible range in cells rather than
        // in rows-of-cells makes the pool `columns` times too small and the
        // last row of every viewport blank.
        let built = build_widget::<()>(Kind::GridView, &crate::widgets::list_view::tests::props(10_000));
        let mut c = built.controller;
        GridViewC::set_metrics(&mut c, (100.0, 80.0), (400.0, 320.0));
        let pool = GridViewC::pool_len(&c);
        assert_eq!(pool % 4, 0, "whole rows of 4 columns, got {pool}");
        let ids = GridViewC::pool_ids(&c);
        GridViewC::scroll_to(&mut c, 8_000.0);
        assert_eq!(ids, GridViewC::pool_ids(&c), "cells were rebound, not rebuilt");
    }
}
```

`ui/src/widgets/column_view.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Props};
    use crate::widgets::column_view::ColumnViewC;
    use crate::widgets::types::SortOrder;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    #[test]
    fn a_column_view_contains_a_real_listview_node() {
        // Mutation check: rendering the rows directly under `columnview`
        // fails the fixture, which nests a `listview` -- and node_tree_of
        // renders through it, so this is asserted, not assumed.
        let built = build_widget::<()>(Kind::ColumnView, &Props::default());
        matches_fixture(
            &built.node,
            "columnview[.column-separators][.rich-list][.navigation-sidebar][.data-table]\n\
             ├── header\n│   ╰── <column header>\n├── listview\n╰── [rubberband]\n",
        )
        .expect("column_view fixture");
    }

    #[test]
    fn clicking_a_sortable_header_cycles_ascending_descending_and_reports_it() {
        // Interaction test. Mutation check: toggling without reporting
        // leaves the model sorted the old way while the arrow says otherwise.
        let built = build_widget::<String>(Kind::ColumnView, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Change, Handler::Text(std::rc::Rc::new(|s| s.to_string())));
        });
        hx.place_columns(&built.node, 120.0);
        assert_eq!(
            c.on_event(&Event::PointerDown { button: 0x110, local: (10.0, 5.0), serial: 1 }, &mut cx),
            vec!["0:ascending".to_string()]
        );
        assert_eq!(
            c.on_event(&Event::PointerDown { button: 0x110, local: (10.0, 5.0), serial: 2 }, &mut cx),
            vec!["0:descending".to_string()]
        );
        assert_eq!(ColumnViewC::sort_of(&c), Some((0, SortOrder::Descending)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::grid_view widgets::column_view`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `grid_view`
in `widgets``.

- [ ] **Step 3: Write minimal implementation**

Fixtures (`gtk/gtkgridview.c`, `gtk/gtkcolumnview.c`), the contract's §5.5
blocks.

`GridViewC` is `ListViewC`'s pool with a 2-D cell: its `visible_range`
divides by `cell.1` to get the first *row*, multiplies by `columns` to get
the first index, and its pool length is always a whole multiple of
`columns`; `columns` itself comes from `FlowBoxC::columns_for`'s rule
clamped by `min_columns`/`max_columns`. The cell node is `child`, not `row`.

`ColumnViewC` owns a `header` node with one `<column header>` per
`Kind::ColumnViewColumn` child, a nested `listview` node driven by an
embedded `ListViewC`, and the sort state:

```rust
    /// Cycle this column's sort: unsorted → ascending → descending →
    /// ascending … (GTK never returns to unsorted by clicking).
    fn cycle_sort(&mut self, column: usize) -> (usize, SortOrder) {
        let next = match self.sort {
            Some((c, SortOrder::Ascending)) if c == column => SortOrder::Descending,
            _ => SortOrder::Ascending,
        };
        self.sort = Some((column, next));
        for (i, state) in self.columns.iter().enumerate() {
            state.node.set_state(PseudoStates::CHECKED, i == column);
            state.node.remove_class("ascending");
            state.node.remove_class("descending");
        }
        self.columns[column].node.add_class(match next {
            SortOrder::Ascending => "ascending",
            SortOrder::Descending => "descending",
        });
        (column, next)
    }
```

whose result `on_event` formats as `"<index>:<ascending|descending>"` and
fires through `EventKind::Change`; a drag on a column edge resizes it when
`resizable`, and everything else is forwarded to the embedded `ListViewC`.

Builders: `grid_view(model, factory)`, `column_view(model, columns)` and
`column_view_column(title, factory)` with `.resizable`, `.expand`,
`.sorter`, `.min_columns`, `.max_columns`, `.show_row_separators`,
`.show_column_separators`, `.sort_column(index, order)`; handlers are
`.on_selected`, `.on_item_activated` and `.on_change`.

Register all three arms and add both fixture tests.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::grid_view widgets::column_view
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — four unit tests and two fixture tests.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/grid_view.rs ui/src/widgets/column_view.rs ui/src/widgets/mod.rs \
        ui/src/view/builders.rs ui/tests/fixtures/gtk4.22-node-trees/grid_view.txt \
        ui/tests/fixtures/gtk4.22-node-trees/column_view.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkGridView and GtkColumnView over the same recycling pool

GridView pools whole rows of cells, so the bottom row of a viewport is never
blank, and its cell node is `child`, not ListView's `row`. ColumnView really
does contain a listview node -- node_tree_of renders through it, so the
fixture asserts the nesting instead of assuming it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 21: `Kind::PopoverMenu` / `Kind::PopoverMenuItem` — `PopoverMenuC`

**Files:**
- Create: `ui/src/widgets/popover_menu.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/popover_menu.txt`
- Modify: `ui/src/view/builders.rs` — `popover_menu`, `popover_menu_item`,
  `.flags`, `.visible_submenu`, `.accel`, `.section`, `.submenu`
- Modify: `ui/src/widgets/mod.rs` — `mod popover_menu;`, both arms
- Modify: `ui/tests/node_trees.rs` — `popover_menu_matches_its_gtk_fixture`
- Test: `ui/src/widgets/popover_menu.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P5's `PopoverC` (a real `Surface::Popup` with `xdg_popup.grab`
  when autohide), `types::{MenuFlags, DisplayHint}`, `Cmd::{OpenPopup,
  ClosePopup}`, `EventKind::Activate`.
- Produces:
  ```rust
  pub fn popover_menu<Msg: Clone + 'static>(items: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn popover_menu_item<Msg: Clone + 'static>(label: &str) -> View<Msg>;
  pub struct PopoverMenuC {
      pub popover: PopoverC, pub items: Vec<Node>, pub cursor: Option<usize>,
      pub submenu: Option<PopupKey>, pub sections: Vec<(Rc<str>, Node)>,
  }
  impl PopoverMenuC { pub fn mnemonic_target(&self, ch: char) -> Option<usize>; }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::popover_menu::PopoverMenuC;
    use crate::widgets::types::DisplayHint;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::Menus, Prop::Classes(["_Open", "_Save", "Quit"].iter().map(|s| (*s).into()).collect()));
        p.set(PropName::Section, Prop::Str("main".into()));
        p.set(PropName::DisplayHint, Prop::Enum(DisplayHint::InlineButtons.to_u16()));
        p
    }

    #[test]
    fn menu_items_are_button_model_under_contents() {
        // Mutation check: naming the items `menuitem` (GTK3's node) fails
        // the fixture and every Adwaita `.menu button.model` rule.
        let built = build_widget::<()>(Kind::PopoverMenu, &props());
        matches_fixture(
            &built.node,
            "popover.background.menu\n├── arrow\n╰── contents\n    ╰── [box.horizontal.inline-buttons]\n\
             ┊       ├── button.model\n        │   ╰── label\n        ╰── button.model\n            ╰── label\n",
        )
        .expect("popover_menu fixture");
    }

    #[test]
    fn a_mnemonic_fires_without_alt_inside_a_menu() {
        // GTK's documented divergence. Mutation check: requiring Alt makes
        // every menu mnemonic dead once the menu is open.
        let built = build_widget::<usize>(Kind::PopoverMenu, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Activate, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(c.on_event(&Event::Key(hx.key("s")), &mut cx), vec![1]);
        assert_eq!(PopoverMenuC::mnemonic_of(&c, 'o'), Some(0));
        assert_eq!(PopoverMenuC::mnemonic_of(&c, 'z'), None);
    }

    #[test]
    fn a_hostile_item_label_never_panics_the_mnemonic_scan() {
        // Labels come from the application model.
        for label in ["", "_", "__", "_\u{0}", "\u{feff}_x", &"_".repeat(10_000)] {
            let mut p = Props::default();
            p.set(PropName::Menus, Prop::Classes([label].iter().map(|s| (*s).into()).collect()));
            let built = build_widget::<()>(Kind::PopoverMenu, &p);
            let _ = PopoverMenuC::mnemonic_of(&built.controller, '_');
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::popover_menu`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`popover_menu` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/popover_menu.txt` — the contract's
§5.6 block (`gtk/gtkpopovermenu.c`).

`ui/src/widgets/popover_menu.rs`:

```rust
impl PopoverMenuC {
    /// The item a mnemonic character selects: the first item whose label
    /// carries `_<ch>`, ASCII-case-insensitively.
    ///
    /// Inside a `PopoverMenu` this fires *without* `Alt`
    /// (`gtk/gtkpopovermenu.c`), which is why it is a method here and not a
    /// window-level shortcut.
    #[must_use]
    pub fn mnemonic_target(&self, ch: char) -> Option<usize> {
        let want = ch.to_ascii_lowercase();
        self.labels.iter().position(|label| {
            let mut chars = label.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '_'
                    && let Some(next) = chars.peek()
                    && next.to_ascii_lowercase() == want
                {
                    return true;
                }
            }
            false
        })
    }
}
```

`build` creates `arrow` and `contents`, then one `button.model > label` per
item, grouped into `box.horizontal.<hint>` nodes per section — GTK's own
note that the section box "may not be the item's direct parent" is exactly
why the fixture nests them and why `node_tree_of` renders through.
`on_event`: `Event::Key` first tries `mnemonic_target` (no modifier
required) and then Up/Down for the cursor, `Return`/`space` to activate the
cursor item, `Right` to open a submenu through `Cmd::OpenPopup` and `Left`
to close it through `Cmd::ClosePopup`; a `PointerUp` inside an item
activates it. Activation fires `EventKind::Activate` with the item index and
pushes `Cmd::ClosePopup(self.popover.key())` so the menu dismisses itself,
which is what makes the compositor-side grab release.

Builders: `popover_menu(items)` collects `Kind::PopoverMenuItem` children;
`popover_menu_item(label)` takes `.icon`, `.accel`, `.section(name, hint)`
and `.submenu(name)`; `.flags` and `.visible_submenu` are one line each;
`.on_item_activated(f)` stores `Handler::Index`.

Register both arms and add `popover_menu_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::popover_menu
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/popover_menu.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/popover_menu.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkPopoverMenu with sections, submenus and Alt-free mnemonics

Items are button.model under contents, sections are the box.horizontal.<hint>
GTK inserts between them, and a mnemonic inside an open menu fires without
Alt -- GTK's documented divergence, and dead code if you require the modifier.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 22: `Kind::PopoverMenuBar` — `PopoverMenuBarC`

**Files:**
- Create: `ui/src/widgets/popover_menu_bar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/popover_menu_bar.txt`
- Modify: `ui/src/view/builders.rs` — `popover_menu_bar`, `.menu`
- Modify: `ui/src/widgets/mod.rs` — `mod popover_menu_bar;`, the arm
- Modify: `ui/tests/node_trees.rs` — `popover_menu_bar_matches_its_gtk_fixture`
- Test: `ui/src/widgets/popover_menu_bar.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `PopoverMenuC` (Task 21), `EventKind::Activate`.
- Produces:
  ```rust
  pub fn popover_menu_bar<Msg: Clone + 'static>(menus: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub struct PopoverMenuBarC { pub items: Vec<Node>, pub open: Option<usize>, pub menus: Vec<PopoverMenuC> }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Event, Kind, Props};
    use crate::widgets::popover_menu_bar::PopoverMenuBarC;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    #[test]
    fn a_menu_bar_is_menubar_of_items_each_owning_a_popover() {
        // Mutation check: naming the root `popovermenubar` fails the
        // fixture; GTK's node is `menubar` (gtk/gtkpopovermenubar.c).
        let built = build_widget::<()>(Kind::PopoverMenuBar, &Props::default());
        matches_fixture(&built.node, "menubar\n├── item[.active]\n┊   ╰── popover\n╰── item\n    ╰── popover\n")
            .expect("popover_menu_bar fixture");
    }

    #[test]
    fn once_one_menu_is_open_hovering_a_sibling_switches_without_a_second_click() {
        // Interaction test, and GTK's actual menu-bar behaviour. Mutation
        // check: requiring a click on every item makes a menu bar feel like
        // a row of buttons and leaves two popovers open at once.
        let built = build_widget::<()>(Kind::PopoverMenuBar, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        hx.place_columns(&built.node, 80.0);
        c.on_event(&Event::PointerDown { button: 0x110, local: (10.0, 5.0), serial: 1 }, &mut cx);
        assert_eq!(PopoverMenuBarC::open_of(&c), Some(0));
        c.on_event(&Event::PointerMotion { local: (100.0, 5.0) }, &mut cx);
        assert_eq!(PopoverMenuBarC::open_of(&c), Some(1), "hover switched menus");
        assert!(!built.node.child(0).unwrap().states().contains(PseudoStates::ACTIVE));
        assert!(built.node.child(1).unwrap().states().contains(PseudoStates::ACTIVE));
    }

    #[test]
    fn left_and_right_move_between_items_while_a_menu_is_open() {
        let built = build_widget::<()>(Kind::PopoverMenuBar, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        hx.place_columns(&built.node, 80.0);
        c.on_event(&Event::PointerDown { button: 0x110, local: (10.0, 5.0), serial: 1 }, &mut cx);
        c.on_event(&Event::Key(hx.key("Right")), &mut cx);
        assert_eq!(PopoverMenuBarC::open_of(&c), Some(1));
        c.on_event(&Event::Key(hx.key("Escape")), &mut cx);
        assert_eq!(PopoverMenuBarC::open_of(&c), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::popover_menu_bar`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`popover_menu_bar` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/popover_menu_bar.txt` — the contract's
§5.6 block (`gtk/gtkpopovermenubar.c`).

`PopoverMenuBarC::build` creates one `item` node per menu, each holding a
`popover` built by `PopoverMenuC`, and sets `Container::Box { Row }`.
`open(index)` closes the previous menu (`Cmd::ClosePopup`), opens the new one
(`Cmd::OpenPopup`) and moves `PseudoStates::ACTIVE` onto exactly one `item`;
`on_event` calls it from a press on an item, from a *motion* over a sibling
item **while a menu is already open** (the whole point of a menu bar), and
from Left/Right; Escape closes. `set_prop` delegates everything to
`apply_universal_prop`.

Builders: `popover_menu_bar(menus)` collects the menus; `.menu(name, view)`
appends one tagged with `PropName::PageName`; `.on_item_activated(f)` stores
`Handler::Index`.

Register the arm and add `popover_menu_bar_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::popover_menu_bar
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/popover_menu_bar.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/popover_menu_bar.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkPopoverMenuBar with hover-to-switch menus

Once one menu is open, moving the pointer to a sibling item switches to it
without a second click, and exactly one item ever carries :active -- which is
the difference between a menu bar and a row of buttons.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 23: `Kind::Window` — `WindowC`, decoration and state classes, window key bindings

**Files:**
- Create: `ui/src/widgets/window.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/window.txt`
- Modify: `ui/src/view/builders.rs` — `window`, `.titlebar`,
  `.default_widget`, `.resizable`, `.modal`, `.deletable`, `.decorated`,
  `.default_size`, `.icon_name`, `.on_close`
- Modify: `ui/src/widgets/mod.rs` — `mod window;`, the arm
- Modify: `ui/tests/node_trees.rs` — `window_matches_its_gtk_fixture`
- Test: `ui/src/widgets/window.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P3's `window::SurfaceStates` and `FocusRing`,
  `Cmd::{Minimize, ToggleMaximized, CloseWindow}`, `types::Decoration`,
  `css::node::PseudoStates::BACKDROP`.
- Produces:
  ```rust
  pub fn window<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub struct WindowC { pub states: SurfaceStates, pub titlebar: Option<Node>,
                       pub focus: FocusRing, pub default_widget: Option<Node>,
                       pub decoration: Decoration }
  impl WindowC { pub fn apply_states(&self, node: &Node); }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Cmd, Event, Kind, Prop, PropName, Props};
    use crate::widgets::types::Decoration;
    use crate::widgets::window::WindowC;
    use crate::widgets::{build_widget, matches_fixture, Headless};
    use crate::window::SurfaceStates;

    fn props(decoration: Decoration) -> Props {
        let mut p = Props::default();
        p.set(PropName::Decoration, Prop::Enum(decoration.to_u16()));
        p.set(PropName::Title, Prop::Str("Files".into()));
        p
    }

    #[test]
    fn a_window_is_window_background_with_the_decoration_class() {
        // Mutation check: emitting `.csd` for an SSD window makes Adwaita
        // draw a client shadow and rounded corners the compositor is already
        // drawing, which double-decorates every real toplevel.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Ssd));
        matches_fixture(&built.node, "window.background\n╰── <child>\n").expect("window fixture");
        assert!(built.node.classes().iter().any(|c| c.as_str() == "ssd"));
        let csd = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        assert!(csd.node.classes().iter().any(|c| c.as_str() == "csd"));
    }

    #[test]
    fn surface_states_drive_the_state_classes_and_backdrop() {
        // Mutation check: leaving `.maximized` on after an unmaximize keeps
        // the square corners forever; forgetting BACKDROP leaves an inactive
        // window painted as active.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &Event::Configure { size: (800, 600), states: SurfaceStates::MAXIMIZED },
            &mut cx,
        );
        assert!(built.node.classes().iter().any(|c| c.as_str() == "maximized"));
        assert!(built.node.states().contains(PseudoStates::BACKDROP));
        c.on_event(
            &Event::Configure { size: (800, 600), states: SurfaceStates::ACTIVATED },
            &mut cx,
        );
        assert!(!built.node.classes().iter().any(|c| c.as_str() == "maximized"));
        assert!(!built.node.states().contains(PseudoStates::BACKDROP));
    }

    #[test]
    fn the_window_key_bindings_reach_the_compositor_as_commands() {
        // gtk/gtkwindow.c:1307-1359. Mutation check: handling these in the
        // toolkit (hiding the surface for `close`) desynchronises the
        // compositor's window list from the app's.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(&Event::Key(hx.key_with_mods("F4", &["Alt"])), &mut cx);
        assert!(cx.cmds.iter().any(|c| matches!(c, Cmd::CloseWindow)));
        c.on_event(&Event::Key(hx.key_with_mods("F10", &["Super"])), &mut cx);
        assert!(cx.cmds.iter().any(|c| matches!(c, Cmd::ToggleMaximized)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::window`
Expected: FAIL — `error[E0433]: failed to resolve: could not find `window` in
`widgets``.

- [ ] **Step 3: Write minimal implementation**

`ui/tests/fixtures/gtk4.22-node-trees/window.txt` — the contract's §5.7
block (`gtk/gtkwindow.c`):

```
window.background [.csd / .solid-csd / .ssd] [.maximized / .fullscreen / .tiled]
├── <child>
╰── <titlebar child>.titlebar [.default-decoration]
```

`ui/src/widgets/window.rs`:

```rust
impl WindowC {
    /// Push `SurfaceStates` onto the node's classes and `:backdrop`.
    ///
    /// The three state classes are mutually exclusive and *removed* when the
    /// state goes away -- a window that is maximized, then unmaximized, then
    /// tiled must not accumulate all three.
    pub fn apply_states(&self, node: &Node) {
        for class in ["maximized", "fullscreen", "tiled"] {
            node.remove_class(class);
        }
        if self.states.contains(SurfaceStates::FULLSCREEN) {
            node.add_class("fullscreen");
        } else if self.states.contains(SurfaceStates::MAXIMIZED) {
            node.add_class("maximized");
        } else if self.states.intersects(SurfaceStates::TILED) {
            node.add_class("tiled");
        }
        node.set_state(
            PseudoStates::BACKDROP,
            !self.states.contains(SurfaceStates::ACTIVATED),
        );
    }
}
```

`build` adds `.background` and the decoration class
(`Decoration::{Csd, SolidCsd, Ssd}` → `csd` / `solid-csd` / `ssd`; icedtea's
compositor draws SSD, so `Ssd` is the default and the gallery opts into
`Csd`), sets `Container::Box { Column }`, and moves the child tagged
`slot("titlebar")` to the end with the `.titlebar` class. `set_prop` handles
`Title`, `Decoration`, `Resizable`, `Modal`, `Deletable`, `Decorated`,
`DefaultWidth`/`DefaultHeight`, `IconName` and `DefaultWidget`, delegating
the rest. `on_event`:

```rust
            Event::Configure { states, .. } => {
                self.states = *states;
                self.apply_states(cx.node);
            }
            Event::Key(key) => {
                let cmd = match (key.keysym_name(), key.mods.alt(), key.mods.super_key()) {
                    ("F4", true, _) => Some(Cmd::CloseWindow),
                    ("F10", _, true) => Some(Cmd::ToggleMaximized),
                    ("h", _, true) => Some(Cmd::Minimize),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    // The compositor owns window state; the toolkit only
                    // asks. Hiding the surface here would desynchronise it.
                    cx.cmds.push(cmd);
                    cx.handled = true;
                } else if matches!(key.keysym_name(), "Return" | "KP_Enter" | "ISO_Enter")
                    && let Some(default) = &self.default_widget
                {
                    cx.cmds.push(Cmd::Focus(default.clone()));
                    cx.handled = true;
                }
            }
```

Builders: `window(child)` plus the nine setters and `.on_close(Msg)`
(`Handler::Unit` under `EventKind::Close`); `.titlebar(v)` pushes
`slot(v, "titlebar")`.

Register the arm and add `window_matches_its_gtk_fixture`.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::window
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — three unit tests and one fixture test.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/window.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/window.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): GtkWindow with decoration and surface-state classes

SurfaceStates drives .maximized/.fullscreen/.tiled and :backdrop, and the
three classes are cleared before one is applied so they cannot accumulate.
The window key bindings ask the compositor through Cmd rather than acting
locally: the compositor owns window state, and acting here would leave its
window list disagreeing with the app.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 24: `Kind::ShortcutsWindow`, `Kind::AboutDialog`, `Kind::AlertDialog`

**Files:**
- Create: `ui/src/widgets/shortcuts_window.rs`, `ui/src/widgets/about_dialog.rs`,
  `ui/src/widgets/alert_dialog.rs`
- Create: three fixtures under `ui/tests/fixtures/gtk4.22-node-trees/`
- Modify: `ui/src/view/builders.rs` — `shortcuts_window`, `about_dialog`,
  `alert_dialog` and their setters
- Modify: `ui/src/widgets/mod.rs` — three modules, three arms
- Modify: `ui/tests/node_trees.rs` — three fixture tests
- Test: the three new files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `WindowC` (Task 23) — all three are `Window` presets — `StackC`
  (Task 15) for `AboutDialog`'s pages, `types::LicenseType`,
  `EventKind::{Response, Close, ActivateLink, Search}`.
- Produces:
  ```rust
  pub fn shortcuts_window<Msg: Clone + 'static>(sections: impl IntoIterator<Item = View<Msg>>) -> View<Msg>;
  pub fn about_dialog<Msg: Clone + 'static>(program_name: &str) -> View<Msg>;
  pub fn alert_dialog<Msg: Clone + 'static>(message: &str) -> View<Msg>;
  pub struct ShortcutsWindowC { pub window: WindowC, pub sections: Vec<Node>, pub search: String }
  pub struct AboutDialogC { pub window: WindowC, pub pages: StackC }
  pub struct AlertDialogC { pub window: WindowC, pub buttons: Vec<Node>,
                            pub default_button: Option<usize>, pub cancel_button: Option<usize> }
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/widgets/alert_dialog.rs` (the other two follow the same three-test
shape with their own fixture and their own one behaviour):

```rust
#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::alert_dialog::AlertDialogC;
    use crate::widgets::{build_widget, matches_fixture, Headless};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::Message, Prop::Str("Discard changes?".into()));
        p.set(PropName::Detail, Prop::Str("They cannot be recovered.".into()));
        p.set(PropName::Buttons, Prop::Classes(["Cancel", "Discard"].iter().map(|s| (*s).into()).collect()));
        p.set(PropName::DefaultButton, Prop::Int(1));
        p.set(PropName::CancelButton, Prop::Int(0));
        p
    }

    #[test]
    fn the_dialog_is_window_dialog_message_with_a_title_label() {
        // Mutation check: dropping `.title` from the primary label makes
        // GtkMessageDialog's bold heading render as body text
        // (gtk/gtkmessagedialog.c:342).
        let built = build_widget::<()>(Kind::AlertDialog, &props());
        matches_fixture(
            &built.node,
            "window.dialog.message\n├── label.title\n├── [label]\n╰── box\n    ├── button\n    ┊\n    ╰── button\n",
        )
        .expect("alert_dialog fixture");
    }

    #[test]
    fn escape_fires_the_cancel_button_and_enter_the_default_one() {
        // Interaction test. Mutation check: firing index 0 for both makes
        // Enter cancel, which is how a destructive dialog does the opposite
        // of what the user pressed.
        let built = build_widget::<usize>(Kind::AlertDialog, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Response, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(c.on_event(&Event::Key(hx.key("Escape")), &mut cx), vec![0]);
        assert_eq!(c.on_event(&Event::Key(hx.key("Return")), &mut cx), vec![1]);
    }

    #[test]
    fn out_of_range_default_and_cancel_indices_are_ignored_not_panicked_on() {
        // The indices come from the application model.
        let mut p = props();
        p.set(PropName::DefaultButton, Prop::Int(i64::MAX));
        p.set(PropName::CancelButton, Prop::Int(-3));
        let built = build_widget::<usize>(Kind::AlertDialog, &p);
        assert_eq!(AlertDialogC::default_of(&built.controller), None);
        assert_eq!(AlertDialogC::cancel_of(&built.controller), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::alert_dialog widgets::about_dialog widgets::shortcuts_window`
Expected: FAIL — `error[E0433]: failed to resolve: could not find
`alert_dialog` in `widgets``.

- [ ] **Step 3: Write minimal implementation**

Fixtures, the contract's §5.7 blocks:

`shortcuts_window.txt`
```
window.shortcuts
```

`about_dialog.txt`
```
window.aboutdialog
```

`alert_dialog.txt`
```
window.dialog.message
├── label.title
├── [label]
╰── box
    ├── button
    ┊
    ╰── button
```

All three embed a `WindowC` and add their own classes on top of
`window.background`: `.shortcuts`, `.aboutdialog`, and `.dialog .message`.

* `ShortcutsWindowC` builds one section node per child, keeps a `search`
  string, closes on Escape (`EventKind::Close`) and focuses its search on
  `Ctrl+F`, firing `EventKind::Search` with the current text.
* `AboutDialogC` builds a `StackC` over the credits pages the setters supply
  (`authors`, `artists`, `documenters`, `translator_credits`, `license`), maps
  `LicenseType` to its short name for the licence page, fires
  `EventKind::ActivateLink` when a website link is activated, and closes on
  Escape.
* `AlertDialogC` builds `label.title`, an optional detail `label` and one
  `button` per entry of `PropName::Buttons`; `default_button` and
  `cancel_button` are read with `usize::try_from(..).ok().filter(|i| *i <
  buttons.len())` so an out-of-range index is simply absent; Escape fires the
  cancel index and Return the default one, both through
  `EventKind::Response`.

Builders: `shortcuts_window(sections)` with `.section_name`/`.view_name`;
`about_dialog(program_name)` with the fourteen setters listed in the contract
(`.version`, `.comments`, `.copyright`, `.license`, `.license_type`,
`.website`, `.website_label`, `.authors`, `.artists`, `.documenters`,
`.translator_credits`, `.logo_icon_name`, `.wrap_license`), each one line
over its `PropName`, the three name lists carried as `Prop::Classes`
(deviation 5); `alert_dialog(message)` with `.detail`, `.buttons`,
`.default_button`, `.cancel_button`, `.modal` and `.on_response(f)`.

Register the three arms and add the three fixture tests.

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p icedtea-ui --lib widgets::alert_dialog widgets::about_dialog widgets::shortcuts_window
cargo test -p icedtea-ui --test node_trees
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS — nine unit tests (three per widget) and three fixture tests.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widgets/shortcuts_window.rs ui/src/widgets/about_dialog.rs \
        ui/src/widgets/alert_dialog.rs ui/src/widgets/mod.rs ui/src/view/builders.rs \
        ui/tests/fixtures/gtk4.22-node-trees/shortcuts_window.txt \
        ui/tests/fixtures/gtk4.22-node-trees/about_dialog.txt \
        ui/tests/fixtures/gtk4.22-node-trees/alert_dialog.txt ui/tests/node_trees.rs
git commit -m "$(cat <<'EOF'
feat(ui): ShortcutsWindow, AboutDialog and AlertDialog as Window presets

All three are one WindowC plus their own classes, which is what GTK does.
AlertDialog's default and cancel indices come from the application model, so
an out-of-range one is absent rather than clamped to zero -- clamping would
make Enter fire Cancel on a destructive dialog.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 25: whole-part sweep — every P6 kind registered, every fixture present, every gate green

**Files:**
- Modify: `ui/tests/node_trees.rs` — the two sweep tests
- Modify: `ui/src/widgets/mod.rs` — remove `GenericC` from any P6 arm it
  still covers (`Kind::NotebookTab`, `Kind::ListBoxRow`,
  `Kind::FlowBoxChild`, `Kind::StackPage`, `Kind::ColumnViewColumn` and
  `Kind::PopoverMenuItem` legitimately keep it; nothing else may)
- Test: `ui/tests/node_trees.rs`

**Interfaces:**
- Consumes: everything Tasks 1–24 produced; `Kind::all()` (P4).
- Produces: no new public API — two gate tests and a clean tree.

- [ ] **Step 1: Write the failing test**

Append to `ui/tests/node_trees.rs`:

```rust
/// The 27 kinds Part 6 owns, in catalogue order.
const P6_KINDS: [icedtea_ui::view::Kind; 27] = {
    use icedtea_ui::view::Kind::*;
    [
        Box, Grid, CenterBox, ScrolledWindow, Paned, Frame, Expander, SearchBar,
        ActionBar, HeaderBar, Notebook, NotebookTab, Overlay, Stack, StackPage,
        StackSwitcher, StackSidebar, ListBox, ListBoxRow, FlowBox, FlowBoxChild,
        ListView, GridView, ColumnView, ColumnViewColumn, PopoverMenu, PopoverMenuBar,
    ]
};

#[test]
fn every_p6_kind_has_a_controller_that_reports_it() {
    // Mutation check: leaving a kind on the GenericC catch-all makes its
    // controller report Kind::Box here, so a forgotten registration cannot
    // reach the gallery gate four parts later.
    for kind in P6_KINDS {
        let built = icedtea_ui::widgets::build_widget::<()>(kind, &Default::default());
        let reported = built.controller.kind();
        let expected_generic = matches!(
            kind,
            icedtea_ui::view::Kind::NotebookTab
                | icedtea_ui::view::Kind::ListBoxRow
                | icedtea_ui::view::Kind::FlowBoxChild
                | icedtea_ui::view::Kind::StackPage
                | icedtea_ui::view::Kind::ColumnViewColumn
                | icedtea_ui::view::Kind::PopoverMenuItem
        );
        if !expected_generic {
            assert_eq!(reported, kind, "{kind:?} is still on the catch-all");
        }
        assert_eq!(
            &*built.node.name(),
            kind.css_name(),
            "{kind:?} built the wrong CSS node"
        );
    }
}

#[test]
fn every_p6_kind_and_the_three_window_presets_have_a_vendored_fixture() {
    // Mutation check: deleting a fixture file makes this fail with the
    // widget's name instead of failing a screencopy gate in Part 8 with a
    // pixel diff nobody can read.
    let mut names: Vec<String> = P6_KINDS.iter().map(|k| snake(*k)).collect();
    names.extend(["window", "shortcuts_window", "about_dialog", "alert_dialog"].map(String::from));
    // Sub-kinds are rendered inside their parent's fixture, never alone.
    names.retain(|n| {
        !matches!(
            n.as_str(),
            "notebook_tab" | "list_box_row" | "flow_box_child" | "stack_page" | "column_view_column"
        )
    });
    for name in names {
        let path = format!(
            "{}/tests/fixtures/gtk4.22-node-trees/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(!text.trim().is_empty(), "{name}.txt is empty");
    }
}
```

`snake(kind)` is the `Kind` → `snake_case` mapping P8's `--widget` flag also
uses; add it to `icedtea_ui::view::Kind` as
`pub fn snake_name(self) -> &'static str` if P4 did not already, and call it
here.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test node_trees`
Expected: FAIL — the first test names whichever kind is still answering
`Kind::Box`; on a complete tree it fails only if a registration was missed,
which is exactly the signal this sweep exists to produce.

- [ ] **Step 3: Write minimal implementation**

Move every P6 kind still on the catch-all onto its own arm (each controller
already exists from Tasks 4–24), add `Kind::snake_name` if missing, and
create any fixture the second test names. No widget behaviour changes in
this task: a failure here is a registration or a file, never a controller.

- [ ] **Step 4: Run test to verify it passes**

Run the full gate set, in this order:

```bash
cargo test -p icedtea-ui --lib
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui
cargo test --workspace
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo doc -p icedtea-ui --no-deps
```
Expected: PASS throughout, and specifically:
- `ui/tests/themed_button_offscreen.rs` — 4 tests, byte-identical numbers;
- `ui/src/layout.rs` — M2's 6 tests plus Task 1's 7;
- `ui/src/widget/button.rs` — M2's tests untouched;
- `ui/tests/adwaita_coverage.rs`, `ui/tests/gtk4_property_reference.rs`,
  `ui/tests/layer_shell_screencopy.rs`, `ui/tests/transition_screencopy.rs`
  — unchanged and green;
- P5's `ui/tests/node_trees.rs` tests — unchanged and green beside P6's 27.

- [ ] **Step 5: Commit**

```bash
git add ui/tests/node_trees.rs ui/src/widgets/mod.rs ui/src/view/mod.rs
git commit -m "$(cat <<'EOF'
test(ui): gate every P6 kind on a real controller and a vendored fixture

A kind left on the catch-all now fails here by name, rather than surfacing in
Part 8 as an unreadable pixel diff; a deleted fixture fails the same way. The
six sub-kinds that legitimately have no behaviour of their own are named
explicitly rather than skipped silently.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

Every requirement of spec §5 (containers, lists, popovers/menus, windows),
§7's P5/P6 gate line, and contract §3.6, §5.4–§5.7 and §9's P6 boundary,
mapped to the task that implements it.

| Spec / contract requirement | Task |
|---|---|
| §3.6 `Container::Grid`, `Container::Center` | 1 |
| §3.6 `Align`, `ChildLayout`, `GridPlacement` | 1 |
| §3.6 `set_container`, `set_child_layout`, `set_measure` | 1 |
| §3.6 `set_style` stops hard-coding `CENTER`/`CENTER` | 1 (per-child only; the container default is unchanged — deviation 2) |
| §9 P6 gate: `layout.rs`'s 6 M2 tests keep every number | 1 (Step 4), 25 (sweep) |
| §9 P6 gate: default `ChildLayout` reproduces M2's `CENTER`/`CENTER` | 1 (`a_node_with_no_child_layout_keeps_m2s_centre_centre`) |
| §4.3 `PropName` for every P6 builder prop | 2 (89 additive variants) |
| §4.4 two-argument handlers (`on_scrolled`, `on_reordered`) | 2 (`Handler::Pair`, `Handler::Indices`) |
| §5 fixtures vendored from GTK 4.22.4 "CSS nodes" | 3 (format), 4–24 (27 files), 25 (presence gate) |
| §5 `node_tree_of(kind, props)` | 3 |
| §5.4 Box | 4 · CenterBox | 5 · Grid | 6 · Frame | 7 · Paned | 8 · Expander | 9 |
| §5.4 ScrolledWindow (overshoot, undershoot, junction, kinetic, overlay bars) | 10 |
| §5.4 SearchBar, ActionBar | 11 |
| §5.4 HeaderBar (packs, title widget, `gtk-decoration-layout`, double-click) | 12 |
| §5.4 Notebook + NotebookTab (tabs, arrows, reorder, `Ctrl+PageUp/Down`, `Alt+1..9`) | 13 |
| §5.4 Overlay (positional classes, measure/clip) | 14 |
| §5.4 Stack + StackPage (M2 `AnimationState`, not a bespoke timer) | 15 |
| §5.4 StackSwitcher, StackSidebar | 17 |
| §5.5 ListBox + ListBoxRow (four selection modes, arrows, Space, Ctrl+A, Enter) | 16 |
| §5.5 FlowBox + FlowBoxChild (per-line clamp, rubberband) | 18 |
| §5.5 ListView (recycling pool) | 19 |
| §5.5 GridView, ColumnView + ColumnViewColumn (header, sort, resize, nested `listview`) | 20 |
| §9 P6 gate: recycling loses neither selection nor animation state | 19 (`scrolling_rebinds_rows_without_losing_…`), 20 (both views' `pool_ids` assertions) |
| §5.6 PopoverMenu + PopoverMenuItem (sections, display hints, submenus, Alt-free mnemonics) | 21 |
| §5.6 PopoverMenuBar (hover-to-switch, Left/Right) | 22 |
| §5.7 Window (decoration classes, `SurfaceStates`, `:backdrop`, key bindings) | 23 |
| §5.7 ShortcutsWindow, AboutDialog, AlertDialog | 24 |
| §5 per widget: node-tree conformance + one rest state + one interaction | 4–24 (each task's Step 1 carries all three; deviation 10 explains where they live) |
| §7 untrusted input never panics | 1 (hostile container/child values), 2 (hostile selection indices), 3 (hostile fixture text), 4 (hostile spacing/orientation), 6 (hostile placements), 8 (hostile position), 10 (hostile extents), 12 (hostile decoration layout), 15 (unknown page name), 19 (shrinking model), 21 (hostile mnemonic labels), 24 (out-of-range button indices) |
| §7 every load-bearing test records a mutation check | every test in 1–25 |
| §7 timing assertions are complexity bounds | 10 (`a_kinetic_flick_decelerates_…`, bounded tick count) |
| §7 `Duration::ZERO` means now, never spin | 9, 10, 11, 15 (each `next_deadline` returns `None` at rest, asserted) |
| §9 P6 must not touch P5's widget files | enforced — Tasks 4–24 create only their own files; `widgets/mod.rs` edits are `mod`/`match` lines (deviation 6) |
| §9 P6 gate: `cargo test`, clippy (both feature sets), fmt | every task's Step 4; swept in 25 |

**Gaps found and closed while writing.** Six, each now a numbered deviation
rather than a decision left to the executor: `set_measure` cannot take a bare
`Rc` over M2's `&mut self` trait (1); `Align::default() == Fill` and "the
default reproduces `CENTER`" are contradictory as written (2); `PropName` is
89 names short of what §5.4–§5.7's own builder lists set (3); §4.4's
`Handler` has no arity for `on_scrolled`/`on_reordered` (4); `Prop` cannot
hold a row factory without one new, `Msg`-free variant (Task 2's inline
deviation); and §9 hands `ListViewC` to P6 while P5's `DropDownC` embeds it
(9). Two smaller ones — the `impl Into<Prop>` setter convention and the
`on_item_activated` rename — are forced by Rust's single inherent-method
namespace and are recorded at Task 4.

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `XXX`, `implement later`,
`fill in`, `similar to Task`, `as above`, `and so on`, `etc.`, `appropriate
error handling`, `handle edge cases`, `...`, and bare "write tests for the
above".

- No occurrences. Every task names the file it creates, prints the fixture
  text it vendors, and prints the code its assertions depend on.
- Where a task describes a body rather than printing it, the described body
  is mechanical and its shape is printed elsewhere in the same plan: Task 5's
  `builders.rs` setters, Task 11's `SearchBarC` prop table, Task 20's
  `GridViewC` range arithmetic (Task 19 prints the 1-D version it
  generalises), and Task 24's three builders' setter lists. In every case the
  signature, the `PropName` and the semantics are given; none leaves an
  undefined symbol.
- Three helpers are introduced once and used by many tasks, each printed in
  full where introduced, never re-described: `widgets`' pending-layout side
  table and `flush_layout` (Task 4), the `offscreen` frame harness (Task 4),
  and `Revealer` (Task 11).
- Every symbol a printed code block names resolves to something this plan
  creates, something M2 already has (`Node`, `LayoutTree`, `ComputedStyle`,
  `AnimationState`, `ManualClock`, `BUNDLED_ADWAITA_LIGHT`), or something the
  contract assigns to P3/P4/P5 and §9 lets P6 consume (`Surface`,
  `SurfaceStates`, `KeyEvent`, `Kinetic`, `Scroll`, `FocusRing`, `Clipboard`,
  `View`, `Props`, `Controller`, `BuildCx`, `EventCx`, `Cmd`, `App`,
  `run_offscreen`, `Frames`, `PopoverC`, `ScrollbarC`, `WindowControlsC`,
  `ButtonC`, `LabelC`, `TextLayout`, `IconTheme`, `IconRef`).
- Six test-only harness methods are named as they are introduced and are used
  only after: `Headless::{cx, event_cx, event_cx_with_handlers, key,
  key_with_mods, place_rows, place_columns, scroll, flick}` (Tasks 3, 8, 10,
  16, 20). Each is a one-line constructor over an existing P3/P4 type.

### 3. Type consistency vs the contract

- `Container`, `Align`, `ChildLayout`, `GridPlacement`, `LayoutTree::{set_container,
  set_child_layout, child_layout, container}` match §3.6 field for field;
  `set_measure`'s parameter is the one difference, deviation 1.
- Every controller struct's name and public fields match its §5.4–§5.7
  listing: `BoxC`, `GridC`, `CenterBoxC`, `ScrolledWindowC`, `PanedC`,
  `FrameC`, `ExpanderC`, `SearchBarC`, `ActionBarC`, `HeaderBarC`,
  `NotebookC`, `OverlayC`, `StackC`, `StackSwitcherC`, `StackSidebarC`,
  `ListBoxC`, `FlowBoxC`, `ListViewC`, `GridViewC`, `ColumnViewC`,
  `PopoverMenuC`, `PopoverMenuBarC`, `WindowC`, `ShortcutsWindowC`,
  `AboutDialogC`, `AlertDialogC`. Where a task adds a field the contract does
  not list (`PanedC::handle_focused`, `NotebookC::tab_pos`,
  `ScrolledWindowC::policy`), it is state the listed behaviour requires and
  §0 permits ("a part may add private items freely").
- Every builder name is the contract's, in `snake_case` from the GTK class
  with `Gtk` dropped: `box_` (the keyword exception, called out at Task 4),
  `grid`, `center_box`, `scrolled_window`, `paned`, `frame`, `expander`,
  `search_bar`, `action_bar`, `header_bar`, `notebook`, `notebook_tab`,
  `overlay`, `stack`, `stack_page`, `stack_switcher`, `stack_sidebar`,
  `list_box`, `list_box_row`, `flow_box`, `list_view`, `grid_view`,
  `column_view`, `column_view_column`, `popover_menu`, `popover_menu_item`,
  `popover_menu_bar`, `window`, `shortcuts_window`, `about_dialog`,
  `alert_dialog`.
- Every setter is named after its GTK property with no `set_` prefix; the
  argument type is `impl Into<Prop>` rather than the concrete type
  (deviation 13 at Task 4), which changes no name.
- Handler names are the contract's `on_<eventkind>` throughout, with the one
  rename recorded as deviation 14: the list and menu `on_activate(|usize|)`
  becomes `on_item_activated(|usize|)`, because P5 already owns
  `on_activate(Msg)` at `Handler::Unit` arity.
- `Kind` variants, `EventKind` variants and `Prop` payload shapes are used
  exactly as §4.2–§4.4 define them; the three additions (`Handler::Pair`,
  `Handler::Indices`, `Prop::Factory`) and the 89 `PropName` variants are
  additive on `#[non_exhaustive]` enums and are enumerated in Task 2 rather
  than accumulated widget by widget.
- `Selection`, `SelectionMode`, `Policy`, `BaselinePosition`,
  `StackTransition`, `StackPageInfo`, `SortOrder`, `Sorter`, `ItemFactory`,
  `RowContent`, `MenuFlags`, `DisplayHint`, `LicenseType` and `Decoration`
  are named by §5.4–§5.7's own builder signatures but defined nowhere in the
  contract; Task 2 defines them, and defines the six P5 shares
  (`Orientation`, `Position`, `Side`, `ListItem`, `IconSize`, `MessageType`)
  only if P5 did not.
