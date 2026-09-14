# M6 accesskit (accessibility) integration — Design

**Date:** 2026-09-12
**Status:** spec (workstream `feature/m6-accesskit`, base `develop@0be7ec1`)
**Scope:** `icedtea-ui` only. Additive; no rendering/input behavior change.

## Goal

Expose the retained `View`/`Instance` widget tree to assistive
technologies through `accesskit`, starting from zero (verified: no
`accesskit` hits repo-wide). First slice covers button, label, entry,
list/drop-down, switch/toggle, and panel/taskbar items — role + name +
state through an in-process adapter — with a test that drives
focus/disabled/checked transitions and asserts the a11y tree updates.

Program context: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
(M6 item 6; layer table names `accesskit` as the reuse choice, replacing
ATK/AT-SPI). The toolkit renders via `skia-rs`, lays out with `taffy`,
themes with GTK-CSS over a CSS node tree (`window`, `button`, `entry`,
classes, pseudo-classes).

## Constraints

- Sibling agents work in parallel on Entry internals (text-input client)
  and drag-and-drop (pointer/grab). Stay inside the workstream files:
  `ui/src/a11y.rs` (new), `ui/src/view/` (additive metadata hooks only),
  `ui/Cargo.toml`, `ui/tests/a11y.rs`, this spec. No widget behavior
  restructuring.
- `wlr` is registry-pinned at 0.20.35 (confirmed in `Cargo.lock`).
- `accesskit` choice: **0.25.0** (latest stable on crates.io at spec
  time; `rust-version 1.85`, builds on this toolchain's 1.97.1).
  `cargo search`/`cargo info` verified 2026-09-12.

## Approaches considered

1. **Full AT-SPI bus adapter now** (`accesskit_unix` + platform event
   loop + Wayland focus plumbing). Rejected for this slice: the bus
   crate (`accesskit_unix 0.23.0`) trails `accesskit 0.25.0` and pairs
   against an older schema; the async bus wiring plus focus/activation
   plumbing is a second project, and landing it half-done risks the
   sibling pointer/grab work.
2. **In-process tree + `TreeUpdate` behind a feature flag (chosen).**
   Build an `accesskit::TreeUpdate` from the retained `Instance` tree on
   demand, with stable `NodeId`s, focus tracking, and action hooks, but
   no bus connection yet. Screen-reader delivery becomes a small
   follow-up (swap the sink for `accesskit_unix`/`accesskit_winit` once
   versions align) without re-designing the mapping.
3. **String-name dump (no accesskit types).** Rejected: it proves
   nothing about role/state correctness and throws away the reuse the
   program spec mandates.

## The cut (explicit)

- **IN:** `ui/src/a11y.rs` adapter: `A11yTree` holding the last
  `TreeUpdate`, `NodeId` assignment stable across reconciles (keyed by
  `Instance` identity: `Key` when present, else node pointer address),
  `build_full(&Instance)` + `update(&Instance)` producing
  `accesskit::TreeUpdate { nodes, tree, focus }`; widget→node mapping
  (§Mapping) with role/name/state; focus/disabled/checked transitions
  reflected; `Action::Click`/`Focus` advertised on actionable nodes;
  `ui/tests/a11y.rs` acceptance tests; `accesskit = "0.25"` dependency
  (default-on `a11y` feature flag gating the module so `--no-default-features`
  keeps the old tree).
- **Shipped in the follow-up (bus) slice:** the AT-SPI bus connection
  (`ui/src/a11y_bus.rs`, feature `a11y-bus`, off by default) wraps
  `accesskit_unix`'s adapter with the version-aligned `accesskit 0.25` /
  `accesskit_unix 0.23` pairing and publishes `A11yTree`'s output on the
  session D-Bus; layout bounds from `LayoutTree` (`build_full_with_layout` /
  `update_with_layout`); `DropDown` option children (a synthetic
  `ListBox`/`ListBoxOption` subtree keyed by `ListItem::id`) and `Expanded`;
  and live regions (`Statusbar`/`InfoBar`/`AlertDialog`). The bus half still
  has **no runtime caller**: nothing in `App`/`Window` constructs an
  `A11yTree` or drives an `A11yBus` — it is an unattached seam exercised only
  by the tests.
- **OUT (the genuine remainder):** action requests are recorded
  (`A11yBus::take_actions`) but not dispatched back into `App`; text
  caret/selection ranges (needs a `Role::TextRun` subtree accesskit's
  `TextSelection` indexes into, which the widget model does not publish);
  layout **transforms** (only border-box bounds are published); bounds for
  the synthesized `DropDown` listbox/options (no `LayoutTree` allocation);
  and full-catalogue role coverage beyond §Mapping.

## Mapping (widget → accesskit node)

Names come from existing props (`Label`, `Text`, `Title`, `Tooltip`
fallback); states from CSS pseudo-states the controllers already
maintain (`DISABLED`, `CHECKED`, `SELECTED`) plus the window focus ring.

| `Kind` | `accesskit::Role` | Name | States / props |
|---|---|---|---|
| `Button`, `ToggleButton` (momentary), `LinkButton`, `MenuButton` | `Button` (`Link` for `LinkButton` when a `Uri` prop is set) | `Label` prop, else child label text | `Disabled` ← `:disabled`; `Click` + `Focus` actions; `Tooltip` → `tooltip` |
| `Label` | `Label` | text via `value` (per accesskit: `Label` text goes in `value`, not `label`) | never focusable; hidden when `!Visible` |
| `Entry`, `SearchEntry` | `TextInput` (`SearchInput` for `SearchEntry`) | `Text` → `value`, `Placeholder` → `placeholder` | `Disabled`; `ReadOnly` ← `!Editable`; focusable |
| `PasswordEntry` | `PasswordInput` | masked value **not** exposed (empty `value`) | `Disabled`, `ReadOnly` as above |
| `CheckButton` | `CheckBox` | `Label` | `Toggled::True/False` ← `:checked`; `Mixed` ← `:indeterminate`; `Disabled` |
| `Switch` | `Switch` | `Label` or tooltip | `Toggled` ← `:checked`; `Disabled` |
| `DropDown` | `ComboBox` (value = selected item text from `Model`+`Selected` props) | `Selected` index → parent `value` | `Disabled`; option children exposed as a synthetic `ListBox` of `ListBoxOption`s (the rows are controller-owned CSS subnodes, not `Instance`s), keyed by `ListItem::id`; `Expanded` from the prop; `HasPopup::Listbox` |
| `ListBox` + `ListBoxRow`, `ListView` rows | `ListBox` + `ListBoxOption` | row text | `Selected` ← `:selected`; `Multiselectable` ← `SelectionMode::Multiple` |
| `SpinButton`, `Scale`, `ProgressBar`, `LevelBar` | `SpinButton`, `Slider`, `ProgressIndicator`, `Meter` | `Label`/tooltip | numeric value/min/max |
| Panel/taskbar items (shell `Button`/`MenuButton` instances) | same as their `Kind` | same | same; window root is `Role::Window` with `Label` = app id |
| Containers (`Box`, `Grid`, `HeaderBar`, …) | `GenericContainer` (filtered from the platform tree) | none | `Hidden` when `!Visible`; `Disabled` is set only on controls, never on containers |

Focus: the adapter reads the window focus ring (`window/focus.rs`;
`FOCUSABLE_CLASS` membership) and sets `TreeUpdate::focus` to the
focused node's id. `Sensitive=false` sets `DISABLED` on the node (via
the existing `Universal::apply` path — the adapter only *reads* it) and
drops `Click` affordance; focus moves off a node that becomes disabled.

## Additive `view/` hooks (only)

- A stable per-`Instance` a11y id slot (e.g. `a11y_id: Cell<Option<NodeId>>`
  or a side table keyed by node pointer) assigned on first build, kept
  across reconciles by `Key`/identity. No prop, render, or hit-test change.
- Optional `accessible_label` override prop? **No** — reuse `Tooltip` as
  the fallback name; a new prop name would widen the contract table this
  slice cannot amend. Revisit in the bus slice if AT testers need it.
- `a11y.rs` reads `Instance { kind, props, node, children }` +
  `node.states()` + focus ring only. Controllers are untouched.

## API sketch

```rust
// ui/src/a11y.rs
pub struct A11yTree { /* last TreeUpdate, id counter */ }
impl A11yTree {
    pub fn new() -> Self;
    /// Full rebuild from the retained root. Stable ids across calls.
    pub fn build_full<Msg>(&mut self, root: &Instance<Msg>, focus: Option<&Node>) -> TreeUpdate;
    /// Incremental: same as full for this slice (diffing is a follow-up);
    /// asserts id stability, which is what the transition test checks.
    pub fn update<Msg>(&mut self, root: &Instance<Msg>, focus: Option<&Node>) -> TreeUpdate;
    pub fn node_for(&self, id: NodeId) -> Option<&accesskit::Node>;
}
```

`NodeId` allocation: root is always `NodeId(0)`; children allocate
monotonically and are memoised by instance identity so a
focus/disabled/checked transition emits the **same id** with changed
properties — the acceptance test asserts exactly this.

## Testing

`ui/tests/a11y.rs` (new, follows `ui/tests/` patterns — offscreen `App`,
`ManualClock`, hermetic sheet):

1. `button_label_entry_list_expose_role_name_state` — build each widget,
   run the adapter, assert role + name + initial state.
2. `focus_disabled_checked_transitions_update_the_tree` — drive focus
   change, `sensitive(false)`, and switch/check toggle through the
   reconciler; assert same `NodeId`, updated `focus`/`Disabled`/`Toggled`.
3. `password_value_is_never_exposed` — negative test.

Gates: `cargo test -p icedtea-ui`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --all --check`. Known pre-existing failures (3 `icedtea-ui`
gallery_gate render tests) are out of scope; any other failure is reported.

## Follow-ups (not this slice)

- **Shipped since:** the AT-SPI bus adapter (`ui/src/a11y_bus.rs`,
  `accesskit_unix` version-aligned), layout bounds from `LayoutTree`,
  `DropDown` option children + `Expanded`, and live regions (see "The cut").
- **Still open:** action dispatch (Click/Focus/SetValue) back into `App`;
  text caret/selection; layout transforms; bounds for the synthetic option
  nodes; a catalogue-wide role audit beyond §Mapping. No runtime caller
  drives the bus yet — `App`/`Window` wiring is the missing piece.
