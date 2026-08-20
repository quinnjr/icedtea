# icedtea WM model extensions (A7) — Design

**Date:** 2026-08-20
**Status:** proposed — for refinement, then issue creation
**Roadmap item:** Track A / A7 ("WM model extensions") in
`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`
**Scope:** compositor-internal only — no new Wayland protocol, no `wlr`/
`wlroots-sys` change, no publish. Everything here lands in the `compositor`,
`config`, and `contract` crates (plus the shell/settings consumers of the
D-Bus contract).

## Goal

Two independent WM-model changes, both config-gated so today's behavior is
the unconfigured default:

1. **Per-output workspaces.** Replace the single global `active_workspace`
   (today: one flat `Vec<Workspace>` shared by every monitor) with an
   independent workspace set *per output*, so switching workspaces on one
   monitor does not blank the others — the way every other multi-monitor
   tiling/floating WM (i3, Sway, GNOME on X11 with "workspaces span
   displays" off, etc.) behaves.
2. **Focus-follows-mouse**, as a `behavior` config option alongside today's
   click-to-focus, so hovering a window raises/focuses it without a click.

## Current model (read before designing against it)

`compositor/src/window.rs` — `WindowManager`:

- `workspaces: Vec<Workspace>` — a flat list, `Workspace { id: u32, name:
  String, focused_window: Option<WindowId> }`. Indexed by `id as usize`
  everywhere (`workspace_mut`, `release_focus`, `set_workspace_names`'s
  migration loop) — **not** a `HashMap`, so today's code leans on "id == Vec
  index" as an invariant.
- `active_workspace: u32` — **one** integer for the whole compositor, not
  per-output. `Window.workspace: u32` names one of these ids; there is no
  output field on `Window` at all — an output is derived transiently, per
  call, from geometry (`State::output_for_window`) or the pointer
  (`State::output_for_pointer`), never stored.
- `is_visible(w)` = `w.workspace == self.active_workspace && !w.minimized &&
  w.mapped` (window.rs:374-376) — the single predicate every rendering,
  hit-testing (`window_at`), and alt-tab (`alt_tab_entries`) path calls
  through. This is the crux of the change: it has no output term at all
  today because there is only one active workspace to compare against.
- `set_active_workspace`/`switch_workspace` (state.rs:2931-2948) mutate the
  one global id and emit one `Event::WorkspaceSet { id, active: true }`.
- `set_workspace_names` (window.rs:539-587) is the **config-reload**
  reconciliation: renames workspaces in place by index, migrates any window
  on a truncated-away index to workspace 0, clamps `active_workspace` back
  into range. This is the shape any per-output redesign must keep matching
  in spirit (in-place reconciliation, never "drop every window and
  rebuild").
- `config::Config.workspace_names: Vec<String>` (config/src/lib.rs:46) is the
  on-disk schema — one flat list, persisted under `DB_WORKSPACES`/
  `KEY_WORKSPACES` (config/src/schema.rs) as a single `Vec<String>` JSON
  blob. `default_config()` (config/src/defaults.rs:53) seeds 4: `["1","2",
  "3","4"]`.
- Keybindings: `"workspace:<n>"` and `"move_to_workspace:<n>"` for n in
  1..=9, bound `SUPER+<n>` / `SUPER+CTRL+<n>` (defaults.rs:28-32),
  dispatched through `apply_action` (state.rs:3096-3099) as `self
  .switch_workspace(idx)`.
- D-Bus contract (`contract/src/{event.rs,types.rs}`, consumed by
  `shell/src/{taskbar.rs,wm_client.rs}` and the settings app's Workspaces
  page): `WorkspaceInfo { id: u32, name: String }`, `Event::WorkspaceSet {
  id: u32, active: bool }`, `Event::WorkspaceList(Vec<WorkspaceInfo>)`,
  `Snapshot { seq, windows, workspaces: Vec<WorkspaceInfo>, active_workspace:
  u32, ... }`. **None of these carry an output.** The taskbar model
  (`shell/src/taskbar.rs:16-55`) mirrors this shape directly: one
  `workspaces: Vec<WorkspaceInfo>` + one `active_workspace: u32`.
- Multi-monitor plumbing that already exists and per-output workspaces must
  reuse rather than duplicate: `State::outputs: HashMap<u32, OutputSurface>`
  (state.rs:506) is the existing per-output registry (index is a stable
  compositor-local `u32`, the same index space `OutputSurface.name` — the
  connector name — is matched against for the Displays feature); `new_output`
  / `destroyed` (state.rs:3711, 3897) are the hotplug add/remove handlers;
  `migrate_windows_from` (state.rs:1628) already re-homes windows by
  geometry when an output disappears — the per-output workspace design's
  hotplug story extends this, not replaces it.
- Click-to-focus today: `State::pointer_button` (state.rs:4645-4694) only
  acts `PointerEvent::Press`/`Release`; `State::pointer_motion`
  (state.rs:4634-4643) → `handle_pointer_motion` (state.rs:3462+) only drives
  drag/resize previews and SSD hover (`update_ssd_hover`) — **no focus
  mutation on motion at all today**. This is the exact seam
  focus-follows-mouse hooks into.

## Decisions

1. **Per-output workspaces are opt-in via `behavior.workspace_mode`
   (`"global"` default | `"per_output"`).** Not a hard cutover. Rationale:
   this is flagged in the roadmap itself as "the risk of the workspace model
   change touching many call sites" — every one of `is_visible`,
   `window_at`, `alt_tab_entries`, `switch_workspace`, `move_to_workspace`,
   `set_workspace_names`, the D-Bus contract, and the taskbar/settings
   consumers assumes one global active workspace today. A config-gated mode
   keeps the default (single-monitor and multi-monitor-content-mirrored
   users, the overwhelming common case) byte-for-byte on today's tested
   path, and confines the new complexity to users who opt in. It also lets
   the two behaviors share one D-Bus wire shape (decision 3) instead of
   forking the contract.
2. **Workspace *identity* stays global; what's *per-output* is which
   workspace is active on that output.** Concretely: `workspace_names`
   stays one flat `Vec<String>` (`Workspace.id` stays the stable 0..N key
   every `Window.workspace` already names) — a "workspace" is still one
   named container windows belong to, exactly as today. What per-output
   mode adds is `active_workspace_by_output: HashMap<u32, u32>` — an
   output's own pointer into that same shared id space — replacing the
   single `active_workspace: u32` scalar when the mode is on. This is
   deliberately **not** "output A has workspaces 1-4, output B has 5-8"
   (Awesome/some i3 configs' model): that would fork workspace identity
   itself, forcing new schema for which workspaces belong to which output
   (breaks on hotplug/reorder) and a second numbering scheme in every
   keybinding/D-Bus surface. Sway/GNOME's model — same named workspaces
   everywhere, but only one is "shown" per output, and a workspace instance
   is only ever shown on at most one output at a time — is simpler to graft
   onto the existing `Vec<Workspace>` and matches user mental model ("go to
   workspace 3" is unambiguous regardless of which monitor you're at).
3. **A workspace is visible on at most one output.** Switching workspace 2
   onto output B when it is currently shown on output A moves it — output A
   falls back to whatever it was last showing (or workspace 0 if that was
   also just taken). This mirrors Sway's `workspace <n> output <name>`
   semantics and is required for `is_visible`/`window_at`/alt-tab to stay
   well-defined: a window's visibility must depend only on `w.workspace`,
   never on "but which output am I asking from", which a workspace shown on
   two outputs simultaneously would force.
4. **`SetWorkspace`/`workspace:<n>` keybinding acts on "the current
   output"**, defined as `State::output_for_pointer()` — the same
   pointer-driven disambiguator `usable_geo_for_pointer`/snap/cascade
   placement already use for "which output is this user-initiated action
   about" (state.rs:1233-1243's own doc draws exactly this global-vs-
   pointer-driven distinction; a keybinding is a user-initiated action, not
   a client/D-Bus request, so it takes the pointer-driven reading). The
   D-Bus `SetWorkspace(id)` method (used by the settings app / any external
   tool) keeps its existing one-arg signature but is documented as "same
   pointer-output resolution as the keybinding" in per-output mode — adding
   an explicit output argument is deferred (see Out of scope) since no
   current caller needs to target a specific non-pointer output.
5. **Window→output is still never stored on `Window`.** Per-output mode does
   not add an `output: u32` field to `Window` — it derives visibility from
   `w.workspace` plus the output's `active_workspace_by_output` entry, the
   same "derive, don't duplicate" choice the existing code already makes for
   which output a window is *on* (`output_for_window`, geometry-derived).
   Storing output on `Window` would create a second source of truth that
   `set_workspace`/hotplug could desync from geometry.
6. **`workspace_mode` and `focus_follows_mouse` are independent flags** on
   `config::Behavior`, not coupled — either can be on/off in any combination
   (focus-follows-mouse works identically under both workspace modes: it
   only ever focuses among windows `is_visible` already returns, so it is
   naturally output-correct once `is_visible` is per-output-aware).
7. **Focus-follows-mouse is driven from `handle_pointer_motion`, gated by a
   still-air debounce, not a raw per-pixel focus call.** Every mouse-move
   event calls `window_at_point`/`focus`; without debouncing, a drag through
   several overlapping windows on the way to a target would thrash focus (and
   emit a `WindowUpdated` per window crossed) before the user's cursor comes
   to rest. Debounce = only act when the window under the pointer *changes*
   from the previous motion event AND the pointer is not currently pressed
   (mid-drag/resize motions must never steal focus mid-gesture — that would
   fight the drag machinery, which already owns focus via
   `begin_client_move`/`handle_pointer_press`). No timer/delay is introduced
   (Sway's `focus_follows_mouse always` vs plain `yes` distinction is not
   replicated) — "changed window under pointer, not currently dragging" is
   the entire predicate, matching the simplest common WM behavior and adding
   no new async/timer machinery to a synchronous event-handling codebase.
8. **Moving onto empty space (no window under pointer) does not clear
   focus.** Matches i3/Sway default and avoids fighting keyboard-only
   focus changes (alt-tab, D-Bus `Focus`) the instant the mouse so much as
   twitches over a gap.
9. **Raising on focus-follows-mouse reuses `Behavior.raise_on_focus`** — the
   config flag that already exists and already gates whether `focus()`-driven
   restacking happens (no new flag needed; this spec does not audit
   `raise_on_focus`'s current call sites beyond noting focus-follows-mouse's
   `focus()` call goes through the same path as any other `focus()` call, so
   it inherits that behavior for free).

## Architecture

### Per-output workspaces

**`config` crate (`config/src/lib.rs`):**

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub raise_on_focus: bool,
    pub hide_bar_on_fullscreen: bool,
    pub snap_enabled: bool,
    pub workspace_mode: WorkspaceMode,     // new
    pub focus_follows_mouse: bool,         // new (see below)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceMode {
    Global,
    PerOutput,
}
```

`default_config()` sets `workspace_mode: WorkspaceMode::Global,
focus_follows_mouse: false` — today's behavior, unconfigured. Persisted
inside the existing `DB_BEHAVIOR`/`KEY_BEHAVIOR` JSON blob (`Behavior` is
already one serde struct written/read as a unit in `Config::save`/
`read_config_from_db`) — **no new redb table**, so this is a pure additive
field change on the existing per-field-fallback path (`serde(default)` or an
explicit default via `#[serde(default)]` on the two new fields keeps an
old on-disk `Behavior` blob, which lacks them, loading cleanly rather than
falling back to the *whole* `Behavior` default — matching the "corrupt
appearance falls back per field" precedent in `config/src/lib.rs`'s own
tests, applied here at the field level within `Behavior` via serde rather
than the table-level fallback the outer loader does).

**`window.rs` (`WindowManager`):**

- `active_workspace: u32` becomes an enum-shaped field capturing both modes
  without the compositor's own per-call code branching on mode everywhere:

  ```rust
  enum ActiveWorkspace {
      Global(u32),
      PerOutput(BTreeMap<u32, u32>), // output index -> active workspace id
  }
  ```

  `BTreeMap` (not `HashMap`) so `workspace_info()`/snapshot iteration is
  deterministic across runs, matching the existing `BTreeMap<WindowId,
  Window>` `windows` field's own rationale.
- `WindowManager::new` gains a `mode: WorkspaceMode` parameter (threaded from
  `State::new`'s `config.behavior.workspace_mode`, mirroring how
  `workspace_names` is already threaded in). Starts `Global(0)` or
  `PerOutput(BTreeMap::new())` (empty — see "no outputs yet" below).
- **`is_visible` becomes output-aware**, its one new required input being
  "which output is `w` geometrically on" — resolved by the caller (`State`,
  which already has `output_for_window`), not looked up inside
  `WindowManager` (which has no output geometry at all — keeping that
  separation is why `output` becomes a parameter, not a new stored field
  per decision 5):

  ```rust
  // Global mode: unchanged, `for_output` ignored.
  // PerOutput mode: visible iff w.workspace == active_workspace_by_output[for_output].
  pub fn is_visible_on(&self, w: &Window, for_output: Option<u32>) -> bool
  ```

  `is_visible`/`is_visible_id` keep their existing signatures and become
  thin wrappers that call `is_visible_on` with the *output the window
  itself resolves to in Global mode's callers* — concretely, every existing
  call site in `Global` mode is unaffected byte-for-byte; every call site in
  `PerOutput` mode needs the resolved output threaded in from `State`. This
  is the "touches many call sites" risk called out below.
- `switch_workspace(&mut self, workspace: u32)` gains an output-aware
  sibling `switch_workspace_on(&mut self, workspace: u32, output: u32)` used
  when `mode == PerOutput`; `State::switch_workspace` picks the right one
  based on the manager's mode plus (per decision 4) `output_for_pointer()`.
  Implements decision 3 (steal-from-other-output): if `workspace` is
  currently the active one on some *other* output, that output's entry
  falls back to the workspace it last showed before this switch (tracked via
  a small `last_shown: HashMap<u32, u32>` cache alongside
  `active_workspace_by_output`, seeded to workspace 0) or to workspace 0 if
  that is unavailable too (mirrors `set_workspace_names`'s existing
  "migrate to workspace 0" fallback).
- `set_workspace_names` (the config-reload reconciler) keeps its
  index-based rename/truncate/migrate logic for the `Vec<Workspace>` list
  unchanged (workspace identity is global per decision 2), but its
  "clamp active workspace back into range" tail (window.rs:576-586) needs a
  `PerOutput` arm: clamp *every* output's entry in
  `active_workspace_by_output`, not one scalar, each clamp emitting its own
  `WorkspaceSet` (below).
- Hotplug (`State::new_output`/`destroyed`, state.rs:3711/3897): `PerOutput`
  mode's `active_workspace_by_output` gains an entry (defaulting to the
  first not-currently-shown-elsewhere workspace, else workspace 0) on
  `new_output`, and loses its entry on `destroyed` — windows that were only
  visible because they sat on that output's active workspace fall back to
  whatever `migrate_windows_from`'s existing geometry-based re-homing puts
  them on (state.rs:1628); once re-homed to a surviving output's geometry,
  their `w.workspace` is untouched (workspace identity doesn't change, only
  which output geometrically contains them), so they become visible again
  iff their workspace happens to be the one active on their new output —
  otherwise they are simply windows on a background workspace on that
  output, exactly like today's single-output "windows on inactive
  workspaces are hidden, not destroyed" behavior.

**`contract` crate — wire shape (decision: additive, not a breaking
rename):**

```rust
// contract/src/types.rs
pub struct WorkspaceInfo {
    pub id: u32,
    pub name: String,
    pub output: Option<u32>, // None in Global mode; Some(idx) in PerOutput mode
}
```

`Option<u32>` rather than always-present `u32` because in `Global` mode a
workspace has no single owning output (it can be inactive everywhere, or —
today — is inherently "the" active one with no output concept at all); a
non-optional field would force every `Global`-mode consumer (today's
taskbar, today's settings Workspaces page) to invent a meaningless value.
This is a **wire-compatible field addition** to a `zvariant::Type` struct —
`contract/src/types.rs`'s own `wire_signatures_are_locked` test documents
exactly this class of change is deliberately guarded (it will need its
expected-signature string updated from `(us)` to `(us(bu))` or equivalent
for `Option<u32>`'s `au`-style encoding, per the existing
`window_update_option_round_trip` test's documented encoding — this is the
one intentional edit to that locked test, not a silent drift).

`Event::WorkspaceSet` gains the acting output for the same reason:

```rust
pub enum Event {
    ...
    WorkspaceSet { id: u32, active: bool, output: Option<u32> }, // output: None in Global mode
    ...
}
```

`Snapshot.active_workspace: u32` is **kept as-is** for `Global`-mode
backward compatibility, and a new field added rather than repurposed:

```rust
pub struct Snapshot {
    pub seq: u64,
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: u32,                          // Global mode's reading; in PerOutput mode, output_for_pointer()'s output's active workspace (best-effort single-value compatibility for a client that has not been updated)
    pub active_workspace_by_output: Vec<(u32, u32)>,     // new; empty in Global mode
}
```

`Vec<(u32, u32)>` rather than a `HashMap` for the zvariant signature (`a(uu)`
is directly representable; `zvariant`/D-Bus has no native map-with-u32-keys
convenience here matching the crate's existing `Vec`-of-struct pattern for
`workspaces`/`windows` themselves).

**D-Bus surface (`compositor/src/dbus.rs`):** `SetWorkspace(u32)` keeps its
signature (decision 4 — pointer-output resolution, no new method needed for
the common case); a **new** optional method
`SetWorkspaceOnOutput(workspace: u32, output: u32)` is added for callers
(future per-output-aware shell/settings UI) that need to target a specific
output without relying on pointer position — additive, not a breaking
change to the existing method.

**Consumers (this spec's boundary — the actual UI update is Track B/settings
territory, out of scope for the compositor-only work here, but the contract
change's blast radius is noted):** `shell/src/taskbar.rs`'s `TaskbarModel`
and `shell/src/wm_client.rs`'s signal parsing, and the settings app's
Workspaces page, all currently assume one global `active_workspace: u32` and
would need a per-output-aware read of the new fields to render correctly
under `PerOutput` mode; under `Global` mode (the default) they are
unaffected since `output` is always `None`/absent and `active_workspace_by_
output` is always empty. This spec's compositor-crate change is what makes
that future work possible; it does not itself modify `shell`/`settings`
beyond what's needed to keep them compiling against the widened contract
(their existing `match`es on `Event::WorkspaceSet{id,active}` and
`WorkspaceInfo{id,name}` need the new fields added to their destructuring —
a mechanical, non-behavioral compile fix, not a UI change).

### Focus-follows-mouse

**`config` crate:** `Behavior.focus_follows_mouse: bool` (added above, next
to `workspace_mode`), default `false`.

**`compositor/src/state.rs`:** `handle_pointer_motion` (state.rs:3462+)
gains one new branch, ordered **after** the existing resize/drag-preview
early-returns (decision 7 — must not fire mid-gesture) and **after**
`update_ssd_hover` (hover feedback is unconditional today and stays so):

```rust
fn handle_pointer_motion(&mut self, pointer: (i32, i32)) -> Option<()> {
    self.update_ssd_hover(pointer);
    if let Some(id) = self.resize.window_id() { /* existing resize-preview path, unchanged, returns */ }
    if let Some(id) = self.drag.window_id() { /* existing drag path, unchanged, returns */ }

    // New: focus-follows-mouse, only once no gesture owns the pointer.
    if self.config.behavior.focus_follows_mouse && !self.pointer_pressed {
        if let Some(id) = self.window_at_point(pointer) {
            if Some(id) != self.focused_id() {
                let previous = self.focused_id();
                if self.window_manager.focus(id).is_some() {
                    self.sync_focus_change(previous);
                    self.emit_pending();
                }
            }
        }
    }
    ...
}
```

This reuses `window_at_point` (the same MRU/is-visible-aware topmost-hit
helper `pointer_button`'s press path already calls, review finding I1's
fix), `WindowManager::focus` (the same mutator click-to-focus calls — no
new focus primitive), and `sync_focus_change` (the existing seat-activation
sync every other focus change already goes through, so a focus-follows-
mouse focus change reaches the client's activation state exactly like a
click does). No SSD/decoration-action path is invoked (unlike
`handle_pointer_press`) — moving the mouse over a window's title bar must
not simulate a button click, only a focus change, so this new branch calls
`window_manager.focus` directly rather than routing through
`handle_pointer_press`.

`self.pointer_pressed` is the existing flag (`pointer_button`,
state.rs:4664-4666) — reused as-is, no new state.

## Decomposition into milestones

**A7.1 — `focus_follows_mouse` config + compositor wiring.** Smaller,
self-contained, no contract/wire changes at all (purely internal behavior),
so it lands first and independently:
- `config::Behavior.focus_follows_mouse: bool` (default `false`) +
  `default_config()`/round-trip test update.
- `handle_pointer_motion` branch above.
- Settings app: an "Behavior" page toggle (mechanical addition next to
  `raise_on_focus`/`hide_bar_on_fullscreen`, which the Behavior page already
  exposes — out of scope to detail further here, noted for completeness).

**A7.2 — Per-output workspace model, `WindowManager` internals.**
`config::WorkspaceMode` enum + `Behavior.workspace_mode`; `WindowManager`'s
`ActiveWorkspace` enum, `is_visible_on`, `switch_workspace_on`, hotplug
entry/exit handling, `set_workspace_names`'s `PerOutput` clamp arm. Land and
fully test **in `Global` mode only being exercised by existing tests** (every
existing `window.rs` test keeps passing unchanged — `WindowManager::new`'s
extra `mode` parameter defaults existing test helpers to `Global`), plus new
`PerOutput`-mode unit tests for: switching a workspace already shown on
another output steals it (decision 3); hotplug add seeds a sane default;
hotplug remove doesn't panic and re-homed windows land on the correct
output's active workspace or become correctly invisible.

**A7.3 — Contract + D-Bus wire changes.** `WorkspaceInfo.output`,
`Event::WorkspaceSet.output`, `Snapshot.active_workspace_by_output`,
`dbus.rs`'s `SetWorkspaceOnOutput` method, `wire_signatures_are_locked`'s
updated expected signature. Separated from A7.2 because it is the one piece
with a locked-signature test and downstream (shell/settings) compile impact
— isolating it lets that blast radius be reviewed on its own diff.

**A7.4 — `State` integration: output-aware `is_visible`/`window_at`/alt-tab,
`switch_workspace`/`move_to_workspace` output-aware dispatch, pointer-output
resolution for the `workspace:<n>` keybinding.** This is the milestone that
actually changes runtime *behavior* under `PerOutput` mode (A7.2-A7.3 are
additive and inert until this lands) and is where the "touches many call
sites" risk concentrates: every caller of `window_manager.is_visible*`/
`window_at`/`alt_tab_entries` in `state.rs` needs to pass or resolve an
output. Grep-verified call sites to update (from this investigation):
`window_at` (click-to-focus, `pointer_button`), `visible_windows`
(rendering/`sync_scene`), `alt_tab_entries` (alt-tab), `switch_workspace`/
`move_to_workspace` themselves, and `focus_follows_mouse`'s new
`window_at_point` call from A7.1 (which must also become output-aware once
A7.4 lands — A7.1 ships first against `Global`-only semantics, where
`window_at_point` is already correct as-is, and needs no change of its own
in A7.4 since `window_at_point` itself is what gets fixed centrally).

**A7.5 — shell/settings compile-through + minimal per-output-aware
rendering.** Mechanical fixes so `shell`/`settings` compile against A7.3's
widened contract (destructure the new `Option`/`Vec` fields), plus (only if
desired in this pass) the taskbar showing per-output workspace state when
running under a layer-shell surface that already knows its own output.
Full per-output taskbar/settings UX redesign is Track B territory and this
milestone intentionally stays minimal — compile-correctness plus "doesn't
regress Global mode," not a UX pass.

Suggested order: **A7.1 → A7.2 → A7.3 → A7.4 → A7.5**, matching dependency
order (A7.1 is fully independent and could ship in parallel with any other
A-track item since it touches no shared enum/contract).

## Testing

- **A7.1:** unit test in `state.rs`'s existing test module mirroring
  `apply_action_switches_workspaces_and_snaps`'s style: two windows with
  overlapping/adjacent geometry, `focus_follows_mouse = true`,
  `handle_pointer_motion` to a point over the second window without a
  press, assert focus moved and exactly one `WindowUpdated{focused}` pair
  emitted (mirrors `focus_minimized_already_focused_emits_event`'s "assert
  emission count" pattern in `window.rs`). A second test: pointer motion
  during an active drag (`self.drag.begin` first) must **not** refocus —
  regression guard for decision 7's gating.
- **A7.2:** table-driven `window.rs` tests analogous to
  `visible_windows_filters_by_workspace_and_minimized` and
  `focus_mru_in_workspace_picks_head_and_skips_minimized`, run under both
  `Global` (existing, unmodified) and a new `PerOutput` fixture. Specifically:
  workspace-steal-from-other-output (decision 3), hotplug seed/removal not
  panicking (mirrors the existing "empty workspace list falls back" panic-
  safety test's spirit), `set_workspace_names` reload clamping every
  output's entry.
- **A7.3:** extend `wire_signatures_are_locked` with the new expected
  signature strings (this is the test that *catches* an accidental
  encoding drift, per its own doc comment's stated purpose — updating it is
  the deliverable, not a workaround); a round-trip test for the new
  `Option<u32>` `WorkspaceInfo.output` field mirroring
  `window_update_option_round_trip`'s existing `Some`/`None` pair pattern.
- **A7.4:** integration-style tests in `state.rs` under `PerOutput` mode:
  two synthetic outputs (state.rs already builds multi-output test fixtures
  for the existing multi-monitor tests — reuse that harness), windows on
  different workspaces active on different outputs, assert `window_at`
  only ever hits the calling output's active workspace's windows (this is
  the per-output analogue of `window_at_only_hits_visible_windows`, which
  already exists for the single-output/global case and is the direct
  template); assert switching workspace 2 onto output B while it's active
  on output A moves it and A falls back correctly.
- **Full-branch regression:** run the entire existing `compositor`/`config`/
  `contract`/`shell` test suites with `workspace_mode` left at its default
  (`Global`) — the explicit acceptance bar for "no regression for the
  unconfigured path," matching the equivalent bar the Displays spec set for
  "empty display config ⇒ today's behavior exactly."

## Risks

- **Call-site fan-out (the roadmap's own called-out risk).** `is_visible`/
  `window_at`/`alt_tab_entries`/`switch_workspace`/`move_to_workspace` are
  read from render sync, click-to-focus, alt-tab, D-Bus handlers, and now
  (A7.1) focus-follows-mouse — a genuinely wide fan-out inside `state.rs`
  (8,372 lines, dense with review-history doc comments recording exactly
  this kind of "which output" ambiguity having bitten this codebase before,
  e.g. `output_for_window`'s J1 finding). Mitigated by: (a) the config gate
  (decision 1) meaning `Global` mode's code paths are provably untouched —
  every existing test is the regression guard; (b) doing the risky
  `State`-level fan-out as its own milestone (A7.4) after the inert
  model/contract groundwork (A7.2/A7.3) is fully tested in isolation, so a
  review can localize any behavioral bug to one milestone's diff.
- **A workspace "moving" between outputs (decision 3) is a UX judgment
  call, not a protocol/library constraint** — Sway users expect it; i3
  users (who don't have true per-output independent workspace *sets*, only
  "workspace N is assigned to output X" statically) might expect something
  closer to a hard per-output-workspace-range model instead (rejected in
  decision 2, but noted as the nearest alternative a future request might
  ask to revisit).
- **`Snapshot.active_workspace`'s dual meaning in `PerOutput` mode**
  (best-effort pointer-output single value, decision's own inline note) is
  a deliberate compatibility shim, not a clean semantics — a shell/settings
  client that has not been updated for A7.5 will show a plausible-looking
  but not fully correct single active workspace under `PerOutput` mode
  (correct for whichever output the pointer happens to be over at snapshot
  time, stale the instant the pointer moves to a different output without a
  new `GetState()` call). Acceptable because `PerOutput` mode is opt-in and
  its full UX lands with updated consumers regardless (A7.5 / Track B); not
  acceptable as a permanent end state, so any consumer meaningfully adopting
  `PerOutput` mode should read `active_workspace_by_output`, not
  `active_workspace`.
- **Focus-follows-mouse interacting with alt-tab and D-Bus `Focus`.** Once
  focus-follows-mouse is on, any programmatic focus change (`Focus(id)` over
  D-Bus, alt-tab landing on a window under a *different* screen location
  than the pointer) can be immediately "fought" by the very next pointer
  motion event re-focusing whatever the mouse happens to be sitting over.
  This is standard, expected focus-follows-mouse behavior in every WM that
  has it (the user is expected to also move the mouse), not a bug this spec
  needs to design around, but it is worth flagging as a real interaction
  users opting into this flag will notice — no mitigation beyond the
  existing "changed window only" debounce (decision 7) is proposed, since
  anything stronger (e.g. suppressing focus-follows-mouse for N ms after a
  programmatic focus change) is exactly the kind of timer/async machinery
  decision 7 explicitly declined to add.

## Out of scope

- Any actual UI for choosing/displaying `workspace_mode` or
  `focus_follows_mouse` beyond a mechanical settings-page toggle addition —
  a full Workspaces-page redesign to show per-output state is Track
  B/toolkit territory (the roadmap's own Track A/B split).
- A hard per-output-workspace-*range* model (i3's static assignment style) —
  noted as the rejected alternative in decision 2 and the risks section.
- `SetWorkspaceOnOutput`'s consumer — the method is added to the D-Bus
  surface in A7.3 for future callers; no caller is written in this spec's
  scope.
- A tiling layout mode — mentioned as a separate, optional, taste-dependent
  roadmap item (A7's own roadmap bullet parenthetical), not part of this
  design.
- Any new Wayland protocol (`ext-workspace-v1`, e.g.) — this is a
  compositor-internal model change, not a new client-facing global; if a
  future request wants third-party pagers/docks to see icedtea's
  workspaces, that is a new protocol-batch item (closer in shape to A2),
  not this spec.
- Focus-follows-mouse's interaction with layer-shell surfaces (panels,
  the taskbar) receiving keyboard focus on hover — today's
  `release_layer_focus`/layer-focus-assertion machinery
  (`handle_pointer_press`'s own doc references it) is a toplevel-focus
  concept; extending focus-follows-mouse to layer surfaces is not
  requested and not designed here.
