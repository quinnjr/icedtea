# icedtea Displays + settings-app review fixes — Design

**Date:** 2026-08-19
**Status:** proposed — awaiting user approval
**Branch:** `settings-app` (folded in per user decision; one combined re-review + merge)
**Predecessor spec:** `docs/superpowers/specs/2026-08-19-icedtea-settings-app-design.md` (which listed multi-monitor/per-output config as *out of scope* — this spec brings it in scope as a follow-on)

## Goal

Give icedtea real multi-monitor support: **advertise the standard
`zwlr_output_management_v1` protocol** so any output tool (the icedtea settings
app, `wlr-randr`, `kanshi`, `wdisplays`, `nwg-displays`) can set per-monitor
**placement, resolution, refresh, scale, and enabled/disabled** live; a
**visual drag-to-arrange canvas** in the settings app; and **persistence** so a
chosen layout survives reboot and hotplug. Fold in the fixes for the seven
findings the user's `/code-review` raised against the settings-app branch (one
— Shift-keysym normalization — is a genuine ship-blocker).

## Decisions

1. **Mechanism: `zwlr_output_management_v1` (standards protocol) + redb
   persistence.** *(User revised the earlier "config-driven private path"
   choice to bring the protocol into scope.)* The compositor advertises the
   protocol via wlroots' `wlr_output_manager_v1`; it is the live enumeration +
   test/apply path. On a **successful apply the compositor persists** the
   resulting per-output config to redb and re-applies it on startup/hotplug, so
   persistence is automatic and tool-agnostic. The settings app is a **protocol
   client** (not a redb editor for displays).
2. **Visual drag canvas: in scope.** The Displays page's primary UI is a
   scaled drag-to-arrange monitor canvas, with per-monitor numeric/dropdown
   controls beside it.
3. **Sequencing: fold into `settings-app`.** No separate merge; the combined
   whole-branch re-review at the end covers the settings app, the review
   fixes, and Displays together.

## Global constraints

- **`wlr` crate is pinned from crates.io** (`wlr = "0.20.21"`, `links = "wlroots"`
  forbids a path dep). New output API + protocol wrapper therefore requires a
  **`wlroots-sys` change → `wlr` 0.20.22 publish → compositor bump**.
  Additive-only within 0.20.x. The publish is a **hard stop for user consent**
  (a crates.io publish, and it touches the repo shared with the parallel M5
  session — coordinate via SendMessage before publishing).
- Keybindings store the compositor's form: `key: "KEY_<unshifted xkb keysym
  name>"` + `modifiers: Vec<String>` matched as an order-independent **set**.
- Commit trailer on every commit: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Config load falls back per-field on missing/corrupt data; **empty display
  config ⇒ today's behavior exactly** (preferred mode + auto-placement). No
  regression for single-monitor / unconfigured users.

---

## Part A — Displays feature

### A1. Output identity & matching

`wlr::OutputId` is only valid within one compositor session, so persistence
cannot key on it. Key on the **connector name** (`output.name()`, e.g. `DP-1`,
`HDMI-A-1`, `eDP-1`) — the same identifier `wlr-randr`/`kanshi` use. Persist the
output **description** (make/model/serial when the backend provides it) as a
secondary, informational field for friendly labels; matching is by `name`.

Documented limitation (not solved here): connector names can move across a
GPU/cable change. Serial/EDID matching is a future refinement; name matching is
the standard baseline and matches user expectation for a fixed desk.

### A2. Persistence config schema (`icedtea-config`)

Written **by the compositor** on a successful protocol apply; read by the
compositor on startup/hotplug. The settings app does **not** write it.

```rust
pub struct DisplayConfig {
    pub name: String,        // connector, the match key, e.g. "DP-1"
    pub enabled: bool,
    pub width: i32,          // mode width in px; 0×0 => "preferred"
    pub height: i32,
    pub refresh_mhz: i32,    // millihertz (wlroots' unit); 0 => highest for w×h
    pub x: i32,              // layout position (top-left), global coords
    pub y: i32,
    pub scale: f64,          // 1.0 default
    pub transform: i32,      // wl_output.transform enum value; 0 = normal
}
// on Config:
pub displays: Vec<DisplayConfig>,   // default: empty
```

- New redb table `DB_DISPLAYS`, `serde_json`-encoded, same per-section pattern
  as `DB_APPEARANCE` et al. (`schema.rs`, `Config::save`, `load_or_default`).
- `default_config()` → `displays: Vec::new()`.
- **No `SCHEMA_VERSION` bump / no migration** — `SCHEMA_VERSION` is write-only
  (never read by `load_or_default`), and every section already falls back
  per-field when its table is absent, so an older DB with no `DB_DISPLAYS`
  loads as an empty vec (= today's behavior) for free. Mirror the existing
  `if let Ok(table) = open_table(..) && let Some(v) = read_json(..)` chain.

### A3. `wlr` crate additions (→ `wlroots-sys`, publish `wlr` 0.20.22)

Additive only; wlroots does the client-facing protocol plumbing, the wrapper is
manager creation + typed event surface + config iteration + succeed/fail.

**Output state setters** on `Output<'h>` (also used directly by the compositor
when re-applying persisted config on hotplug):
- `modes(&self) -> Vec<Mode>` where `Mode { width, height, refresh_mhz,
  preferred }` — enumerate `wlr_output.modes`.
- `set_mode(&self, w, h, refresh_mhz)` — `wlr_output_state_set_mode` matching an
  existing mode, `set_custom_mode` fallback; mirrors `enable_with_preferred_mode`.
- `set_scale(&self, f64)`, `set_transform(i32)`, `disable(&self)`.
- `set_output_position(id, x, y)` **already exists** (currently dead) — reuse.

**Output-management protocol** wrapping `wlr_output_manager_v1`:
- `Runtime::create_output_manager()` — creates + advertises the global; the
  crate keeps the manager's advertised head state in sync with the current
  output layout (`wlr_output_manager_v1_set_configuration` on output/layout
  change).
- A new handler hook (on the existing handler surface) delivering **apply** and
  **test** requests as a typed `OutputConfiguration` — iterable
  `OutputConfigurationHead { output_id, enabled, mode/custom_mode, x, y, scale,
  transform }`.
- `OutputConfiguration::send_succeeded()` / `send_failed()` to answer the client
  after the compositor tests/commits.

Every new symbol gets a `wlr`-side test in the coverage suite (headless backend
advertises modes and can round-trip a configuration through apply/test).

### A4. Compositor (`compositor/src`)

- **Track name/description per output.** Add `name`/`description` to
  `OutputSurface` (or a parallel map), populated in `new_output` from
  `output.name()` / `output.description()`.
- **Advertise the manager.** Create `wlr_output_manager_v1` at startup; keep its
  head state synced as outputs come/go and modes change.
- **Handle apply/test.** For a `test` request: build the output states, run
  wlroots' test, `send_succeeded`/`send_failed` without committing. For `apply`:
  test, then commit `set_mode`/`set_scale`/`set_transform`/`set_output_position`/
  `disable` per head; on success `send_succeeded`, re-derive geometries
  (`create_output`/layout boxes), re-arrange layers/windows (windows on a
  now-disabled/shrunken output migrate via the existing
  `migrate_windows_from`/usable-area machinery), and **persist** the resulting
  config to redb `DB_DISPLAYS`. On failure `send_failed`, no state change.
- **Apply persisted config on connect.** In `new_output`, after `init_output`,
  look up persisted `config.displays` by `output.name()`: present+enabled →
  `set_mode`/`set_scale`/`set_transform`/`set_output_position` (fall back to
  `enable_with_preferred_mode()` + auto on any setter error so an output never
  stays dark); present+disabled → `disable()`; absent → today's path unchanged.
- **Hotplug** is just `new_output` firing later — same lookup, so a monitor
  re-plugged after a change comes up configured.

### A5. D-Bus / contract

- The **protocol replaces the previously-proposed `ListOutputs` D-Bus method** —
  enumeration and apply are the protocol's job now. No new D-Bus method.
- `ReloadConfig` / `ConfigReloaded(Appearance)` are unchanged and unrelated:
  they carry the *other* config sections (appearance/behavior/workspaces/
  keybindings). Display persistence is compositor-internal.
- `contract` gains the shared `Mode` / `OutputInfo` serde types only if a
  test/harness needs them across the crate boundary (the protocol carries the
  live data; contract types are for tests).

### A6. Settings app: Displays page (`settings/src/pages/displays.rs`)

The app becomes a `zwlr_output_management_v1` **client**:
- **Wayland access.** Open a dedicated second Wayland connection
  (`wayland-client`) for the output-management protocol, dispatched on a glib fd
  source so it integrates with GTK's main loop; GDK's own `wl_display` is left
  untouched. (New deps: `wayland-client`, `wayland-protocols-wlr`.) The client
  connection + protocol state machine live in a GTK-free module
  (`settings/src/outputs/` — enumerate heads/modes, build a configuration,
  test/apply, surface succeeded/failed) so it is unit-/integration-testable
  against the harness compositor without GTK.
- **Not running under this compositor?** If the protocol global is absent, the
  page shows "output management unavailable — this compositor does not advertise
  zwlr_output_management_v1" and disables its controls.
- **Visual drag canvas** (`GtkDrawingArea` + drag gesture): each enabled head is
  a rectangle scaled to a shared factor; drag to reposition with edge-snapping
  to neighbors and to the 0,0 origin; the canvas is the source of truth for X/Y.
  Selecting a monitor focuses the side controls.
- **Side controls** for the selected monitor: **Enabled** switch, **Resolution**
  dropdown (distinct w×h from the head's modes), **Refresh** dropdown (refresh
  values for the chosen w×h), **Scale** spin, **Transform** dropdown
  (normal/90/180/270), read-only **Position** reflecting the canvas.
- **Test** (optional button) previews via the protocol's test request; **Apply**
  sends the configuration; a failed apply shows the compositor's rejection and
  reverts the canvas. No redb writes from the app — the compositor persists on
  its side.
- Pure-core/GTK-view split: canvas geometry math (scale factor, hit-testing,
  snapping), mode-list derivation (dedup w×h, refresh grouping), and the
  configuration-builder are GTK-free functions, unit-tested; widgets are thin.

### A7. Testing (Displays)

- **Config**: round-trip `displays` through save/load; missing-table upgrade
  loads empty; corrupt table falls back to empty.
- **`wlr` crate**: `modes()`/`set_mode`/`set_scale`/`set_transform`/`disable`
  and the output-manager wrapper (create, apply/test event delivery, config-head
  iteration, succeed/fail) covered in the wlroots-sys suite against the headless
  backend.
- **Compositor (harness)**: bring up two headless outputs; drive an
  `OutputConfiguration` (position B right-of A at a chosen mode) through the
  protocol; assert layout boxes/usable areas match and that `DB_DISPLAYS` was
  persisted; disabled-output path asserts windows migrate off; a failed/invalid
  config asserts `send_failed` and no state change; **restart** with the
  persisted DB asserts the layout is re-applied on `new_output`.
- **Settings core (GTK-free)**: canvas snapping/hit-testing, mode-list
  derivation, and the configuration-builder are non-vacuously unit-tested; the
  `outputs/` client module round-trips an enumerate→apply against the harness
  compositor.

---

## Part B — `/code-review` fixes (folded in)

Fix all seven on this branch; each with a regression test where it's a logic bug.

1. **[ship-blocker] Shift-keysym normalization** —
   `settings/src/pages/keybindings.rs:157`. Capture must resolve the
   **unshifted, level-0** keysym, not GDK's shift-adjusted keyval. Use the
   hardware **keycode** (currently ignored `_keycode`) translated at group 0 /
   level 0 via the keymap (GTK4 `gdk::Display::map_keycode` / equivalent) to get
   the keysym the compositor's `match_action` (`input.rs:100-121`) actually
   compares against. `keyval_to_lower` is insufficient (fixes letters, not
   `Shift+1 → exclam` vs runtime `1`).
2. **Fix the vacuous test** — `settings/src/model.rs:220`. Drive
   `combo_from_keysym` from a **shifted** capture (e.g. `Q`/`exclam` with
   `shift:true`) and assert the serialized key resolves to the **unshifted**
   keysym the compositor uses. This test must *fail* before fix #1 and pass
   after (proves non-vacuity).
3. **`populating` guard write-back** — `settings/src/pages/appearance.rs:72`.
   The guard suppresses `mark_dirty()` but not the widget→model write, so a
   value the widget can't represent (bar_height clamp, unknown bar_position,
   invalid hex) silently mutates `working` on load → phantom "unsaved changes"
   and Apply overwriting the real value. During populate, suppress the
   **write-back** too (or set widgets without triggering the handler).
4. **Workspace Add misnumbering** — `settings/src/pages/workspaces.rs:140`.
   `(len+1)` duplicates/misnumbers after a Remove or rename. Name from
   `max existing numeric + 1` (or next unused), not length.
5. **Orphaned workspace bindings** — `settings/src/pages/workspaces.rs:114`.
   Removing a workspace must also drop its generated `workspace:N` /
   `move_to_workspace:N` bindings so stale bindings don't persist to disk.
6. **Dead `bridge` module + `async-channel` dep** —
   `settings/src/lib.rs:7`, `settings/Cargo.toml`. The sync `ReloadClient`
   made the copied-from-shell bridge and its `async-channel` dependency
   unused. Remove both.
7. **Second-row capture leaves stale label** —
   `settings/src/pages/keybindings.rs:214`. Starting capture on row B while row
   A is pending leaves A stuck on "Press a key…". Reset any previously-pending
   row when a new capture starts. (Cosmetic.)

---

## Out of scope (this milestone)

- Serial/EDID-based output matching (name-based baseline this milestone).
- Per-output wallpaper (tracked separately in the WM design's future work).
- Adaptive-sync / VRR toggle and mirroring (transform rotation *is* in scope;
  mirrored-output modelling is not).
- A `.desktop` install target/Makefile — the `.desktop` file itself is already
  added (`settings/data/org.icedtea.Settings.desktop`); packaging is a follow-up.

## Risks

- **`wlr` protocol wrapper is the biggest lift.** Wrapping
  `wlr_output_manager_v1` (apply/test events, config-head iteration, succeed/
  fail, keeping advertised head state in sync) is the milestone's hardest,
  highest-uncertainty work → a **dedicated early plan task, gated by its own
  wlroots-sys tests**, before anything downstream depends on it.
- **GTK app as a Wayland protocol client.** Running a `wayland-client`
  connection alongside GDK inside one process (second connection + glib fd
  dispatch) is the second uncertainty → an **early task with a smoke test**
  (bind the global, enumerate heads against the harness) before the canvas UI.
- **Publish + parallel session**: `wlr` 0.20.22 touches the shared wlroots-sys
  repo. Coordinate via SendMessage before the publish window; `[patch]` resolves
  to the checked-out branch for local testing pre-publish.
- **Combined re-review size**: folding in makes the branch large. The final
  whole-branch review is dispatched on the most capable model accordingly.
