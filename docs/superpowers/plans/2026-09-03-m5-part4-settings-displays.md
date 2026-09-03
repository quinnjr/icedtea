# Pure-Rust GTK-themed UI — M5 Part 4: settings Displays page: outputs fd ingress, DrawingArea canvas, head panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild `settings`' Displays page on `icedtea-ui` — a `DrawingArea` monitor canvas driven by view-layer pointer events, a per-head control panel, its own Test/Revert/Apply footer, and the `zwlr_output_management_v1` message handling — with `displays_canvas.rs` and `displays/state.rs` called verbatim and the page's behaviour proven by unit tests plus harness-driven rest-state and interaction gates.

**Architecture:** The page is three pure `view` functions (`displays::view` composing `displays::canvas::view` and `displays::controls::view`) plus a set of `&mut DisplaysState` mutators that `settings/src/app.rs`'s `update` match delegates to. The canvas paints through a widened `Prop::Draw` callback that carries `&mut PaintCx` (M5-D6) and receives `PointerDown`/`PointerMotion`/`PointerUp` (M5-D5); the drag maths is `displays_canvas::{compute_view, hit_test, snap}` called, never reimplemented. Outbound `zwlr_output_management_v1` submissions go straight through P1's `OutputsPump` on the loop thread; every reply arrives as `Msg::Outputs(Arc<OutputsMsg>)` from `App::on_fd`.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `icedtea-ui` (M3 + M5 P0), `skia-rs-safe` 0.4.0, `taffy` 0.14, `wayland-client` 0.31, `wayland-protocols-wlr` 0.3, `rustix` 1, `zbus` 5, `crossbeam-channel` 0.5, `async-channel` 2, `xkbcommon` 0.9, `icedtea-harness` (dev), `tempfile` 3 (dev). No `gtk4`/`gtk4-layer-shell`/`glib`/`gio`/`gdk`/`pango`/`cairo` anywhere.

**Spec:** `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md` (§5.4 "P4 Displays", §7 gates, D4/D8) and the frozen contract `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§2.8 is this part's normative text; §2.1–§2.5 are what P1 produced; §5's "P4 — Displays" is the ownership boundary).

---

## Contract deviations

Every item here is appended to the contract's §6 by Task 13. Nothing in this list is silent.

- **P4-D1 — `settings/tests/support/displays.rs`.** The contract's §0 module map names no shared settings test-support module, but §2.6/§2.7/§2.8 all require harness-driven gates that need one. P4 adds its own, self-contained, at `settings/tests/support/displays.rs`, pulled in by `#[path]` from `settings/tests/displays.rs` and `settings/tests/interaction.rs`. It shares no items with any support module P2 or P3 may have created and P4 edits none.
- **P4-D2 — a fresh `Prop::Draw` closure every frame is intended.** `Props::diff` compares `Prop::Draw` by `Rc` pointer, so the canvas's draw prop diffs as changed on every `view`. Contract §2.8 requires the closure to be rebuilt per `view` ("must not capture the model"), so this is the intended cost: one prop write and one repaint per frame in which the page is visible, no relayout.
- **P4-D3 — the canvas is fixed-size, and `CANVAS_W`/`CANVAS_H` are the single source of truth for `compute_view`.** The GTK page stashed the last-drawn `View` into `DisplaysState.view` from inside the draw callback; contract §2.8 forbids that ("a paint callback may not mutate the model") and tells `update` to "recompute … from the current allocation", which `update` cannot read. P4 pins the drawing area to `CANVAS_W = 360.0` × `CANVAS_H = 240.0` (the GTK `set_content_width(360)`/`set_content_height(240)` values) with `hexpand(false)`/`vexpand(false)`, and both `canvas::draw` and the drag mutators call `compute_view(&rects, CANVAS_W, CANVAS_H, CANVAS_MARGIN)`, so the painted view and the hit-tested view cannot drift. `DisplaysState.view` is still written — by `update`, on `HeadDragBegan` and on every `HeadsChanged` — so `state.rs` is called, not edited.
- **P4-D4 — P4 appends one rule to `settings/style.css` (P3's file).** `#displays_canvas { padding: 0; border: 0 none; margin: 0; }`. `Event::PointerDown`'s `local` is relative to the **border box** while `Prop::Draw`'s `Rect` is the **content box**; with no padding or border the two coincide, which is what makes `hit_test` against painted rects correct. Contract §2.9 already anticipates "any page-local spacing a part needs".
- **P4-D5 — `ui/src/widgets/drop_down.rs`'s list height.** Pre-authorised by contract §2.8 and §7's first discharged forward reference; landed by Task 9 and recorded here as required.
- **P4-D6 — P4 edits the Displays arm group of `settings/src/app.rs`'s `update`.** Contract §0's module map assigns `app.rs` to `[P1..P4]`; §5's "P4 — Owns" list omits it. §0 governs: P4 replaces the stub bodies P1 left for the fourteen Displays `Msg` variants and touches no other arm.
- **P4-D7 — `SettingsModel.outputs` is P1's field, consumed unchanged.** §2.5's `main.rs` snippet passes `pump.clone()` into `SettingsModel::new`, but §2.2's field list omits the field. P1 already lands it under its own deviation P1-D2 as `pub outputs: Option<crate::outputs::pump::OutputsPump>` (no `Rc`: `OutputsPump` is itself `Clone`), set through `#[must_use] pub fn with_outputs(self, pump: Option<OutputsPump>) -> Self` rather than a third constructor argument. P4 **consumes that shape verbatim** and adds nothing; this entry stays only to name the field P4's `submit`/`revert` read. (Consistency-check ruling E2.)
- **P4-D8 — the Displays submit runs inside `update`, and the pump's submit methods return `Result`.** §2.5 types `OutputsPump::{test_configuration, build_and_send_configuration}` as fire-and-forget `-> ()` "from `Cmd::Task`". `Cmd::Task` has no return channel, so a submit that fails before it reaches the wire (`OutputsError::{ManagerUnavailable, UnknownHead, Protocol}`) could neither set the GTK page's `"Test failed: {err}"` status nor clear `displays_in_flight` — the buttons would be dead for the rest of the session. P4 changes both to `-> Result<(), crate::outputs::OutputsError>` and calls them from `update`. Spec D8's "never inside `update`" is about the **blocking D-Bus** calls; this is a non-blocking wayland request build plus `flush` on the loop thread, exactly what the GTK click handler did.
- **P4-D9 — `ICEDTEA_SETTINGS_PAGE` is P2's knob, consumed here.** `main.rs` honours this environment variable as the initial `PageId` so a gate can open the Displays page without synthesising switcher clicks. Names not in `PageId::ALL` are ignored (untrusted input never panics). **P2 lands it first** (its own deviation P2-D8, Task 9); P4 verifies the knob is present and behaves as described and adds it only if it is not. Recorded here because P4's gates depend on it, not because P4 owns it. (Consistency-check ruling E3.)
- **P4-D10 — a third probe-report line kind, `state <key> <value…>`.** Contract §2.8 requires the drag interaction gate to assert "the model's rect through the probe report", and M5-D9's `probe`/`alloc` lines carry only geometry. P4 adds `state` lines to settings' report writer for `displays.position`, `displays.dirty`, `displays.selected` and `displays.in_flight`.
- **P4-D11 — cairo baseline → `TextLayout` origin.** `cr.move_to(x, y); cr.show_text(..)` positions a **baseline**; `TextLayout::draw(canvas, origin, colour)` takes the layout box's **top-left**. Contract §2.8 says the label offsets are "unchanged"; P4 keeps the two literals (`+16.0`, `+30.0`) as baselines and converts with `origin.y = baseline - CANVAS_FONT_PX`.
- **P4-D12 — `apply_reaches_reload_config_on_the_mock` lands in `settings/tests/interaction.rs`, owned by P4.** §2.8 lists it under P4's gates although it exercises the footer P1 owns. P4 adds it as a test only; it edits no page source outside `pages/displays/`. **`a_colour_pick_changes_the_swatch` is P2's, not P4's** — P2 claims it under P2-D4 and lands it in `settings/tests/appearance.rs`, because §5 forbids P4 from touching another page and the swatch is an Appearance-page widget P2 re-shaped (P2-D11). P4 neither writes nor owns it. (Consistency-check ruling E4.)
- **P4-D13 — the "recording `ReloadClient` seam" is a test-owned D-Bus service.** §2.8 asks for "a recording `ReloadClient` seam"; `compositor_reload.rs` is a file no part may touch. P4 instead stands up a `zbus` service claiming `org.icedtea.Compositor` on the session bus inside the test and records the `ReloadConfig` calls it receives — the same posture, and the same visible skip when the bus is absent or the name already owned, that `settings/tests/live_apply.rs` uses.

---

## Global Constraints

- **Pinned crates, exact:** `wayland-client` 0.31, `wayland-protocols-wlr` 0.3 (`features = ["client"]`), `zbus` 5 (`zbus.workspace = true`), `rustix` 1, `skia-rs-safe` 0.4.0, `taffy` 0.14, `xkbcommon` 0.9, `crossbeam-channel` 0.5 (`crossbeam-channel.workspace = true`), `async-channel` 2, `redb`/`tracing`/`tracing-subscriber` from the workspace, `wlr` 0.20.29.
- **No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gobject`, `gdk`, `gdk4`, `pango` or `cairo`** in a migrated app — source or `Cargo.toml`.
- **Edition 2024, `rust-version = "1.94"`.** No new workspace members, no new third-party dependency.
- **Gates, per crate touched:**
  - `cargo test -p icedtea-settings`
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-settings --all-targets -- -D warnings`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
  - `cargo fmt --all --check`
- **All M1–M3 gates stay green, unchanged:** `themed_button_offscreen`, `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`, `window_events`, `counter_app`, `node_trees`, `widget_pixels`, `reconcile_props`, `gallery_gate` (light/dark/hc), `interaction_gate` (16/16).
- **Parts execute IN ORDER 0 → 6.** P4's base is P3's head, so P0's toolkit additions (M5-D1…D11) and P1's extraction, `SettingsModel`/`Msg`, reload worker and `OutputsPump` are all present and may be consumed.
- **`settings` swapped `gtk4` out IN PLACE at P1 (spec D3).** There is no GTK Displays page left to compare against at runtime; `settings/src/pages/displays.rs` is already gone, split into `pages/displays/{mod,state}.rs` by P1.
- **`settings/tests/live_apply.rs` keeps spawning the REAL `icedtea-compositor` binary** (inherited decision 11). P4 does not touch it, and nothing P4 adds moves it onto `icedtea_harness::Compositor`.
- **Never edit `settings/src/pages/displays_canvas.rs` or `settings/src/pages/displays/state.rs`.** Call them.
- **Untrusted input never panics** — output-management heads, malformed modes, an out-of-range dropdown index, an environment variable. Drop, log once, fall back.
- **Every load-bearing test records a mutation check** in a comment: break the code it covers, confirm it fails, restore.
- **Commit trailer, every commit:**
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  ```

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `settings/src/pages/displays/canvas.rs` | Create | `DisplaysSnapshot`, `HeadTile`, `CanvasScene`, `scene`, `draw`, `view`, and the three drag mutators (`drag_began`/`dragged`/`drag_ended`). The only file that knows canvas geometry. |
| `settings/src/pages/displays/controls.rs` | Create | The per-head panel: `repopulate`, the four index helpers, `position_text`, `view`, and the six control mutators. |
| `settings/src/pages/displays/mod.rs` | Modify | Page assembly (`view`, `footer`), the outputs-message fold (`on_outputs`), the Test/Revert/Apply actions, and `pub mod {canvas, controls, state}`. |
| `settings/src/app.rs` | Modify | The fourteen Displays arms of `update`, `SettingsModel.outputs`, and the `state` probe-report lines. |
| `settings/src/main.rs` | Modify | `ICEDTEA_SETTINGS_PAGE`. |
| `settings/src/outputs/pump.rs` | Modify | `test_configuration`/`build_and_send_configuration` return `Result` (P4-D8). |
| `settings/style.css` | Modify | One `#displays_canvas` reset rule (P4-D4). |
| `settings/tests/support/displays.rs` | Create | `SettingsDriver`: harness compositor + settings process + screencopy + injectors + probe-report parsing. |
| `settings/tests/displays.rs` | Create | The Displays rest-state gate (light/dark/hc), the drag interaction gate, and the drop-down fit gate. |
| `settings/tests/interaction.rs` | Create | `apply_reaches_reload_config_on_the_mock` (the colour-pick gate is P2's — P4-D12/E4). |
| `ui/src/widgets/drop_down.rs` | Modify | Content-sized, capped, scrollable embedded list (P4-D5). |
| `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` | Modify | §6 amendments P4-D1…P4-D13. |

---

## Task 1: The canvas snapshot and scene (pure geometry)

**Files:**
- Create: `settings/src/pages/displays/canvas.rs`
- Modify: `settings/src/pages/displays/mod.rs` (add `pub mod canvas;`)
- Test: `settings/src/pages/displays/canvas.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from P1, called verbatim, never edited):
  - `crate::pages::displays::state::DisplaysState { heads: Vec<Head>, edits: Vec<HeadEdit>, selected: Option<usize>, dirty: bool, view: displays_canvas::View, drag: Option<Drag>, res_options: Vec<(i32, i32)>, refresh_options: Vec<i32> }`
  - `crate::pages::displays::state::all_rects(st: &DisplaysState) -> (Vec<usize>, Vec<displays_canvas::Rect>, Vec<bool>)`
  - `crate::pages::displays::state::CANVAS_MARGIN: f64` (`16.0`)
  - `crate::pages::displays_canvas::{Rect, View, compute_view}` where `Rect { x: f64, y: f64, w: f64, h: f64 }` and `View { scale: f64, off_x: f64, off_y: f64 }`, `View::to_canvas(&self, r: &Rect) -> Rect`
  - `crate::outputs::{Head, HeadEdit, ModeRequest}`
- Produces:
  ```rust
  pub const CANVAS_W: f64 = 360.0;
  pub const CANVAS_H: f64 = 240.0;
  pub const CANVAS_FONT_PX: f32 = 11.0;

  #[derive(Clone, Debug, PartialEq)]
  pub struct DisplaysSnapshot {
      pub rects: Vec<crate::pages::displays_canvas::Rect>,
      pub enabled: Vec<bool>,
      pub selected: Option<usize>,
      pub names: Vec<String>,
      pub details: Vec<String>,
  }
  impl DisplaysSnapshot {
      pub fn of(st: &crate::pages::displays::state::DisplaysState) -> DisplaysSnapshot;
  }

  #[derive(Clone, Debug, PartialEq)]
  pub struct HeadTile {
      pub rect: crate::pages::displays_canvas::Rect,
      pub enabled: bool,
      pub selected: bool,
      pub name: String,
      pub detail: String,
  }

  #[derive(Clone, Debug, PartialEq)]
  pub struct CanvasScene {
      pub view: crate::pages::displays_canvas::View,
      pub tiles: Vec<HeadTile>,
  }

  pub fn scene(snap: &DisplaysSnapshot, width: f64, height: f64) -> CanvasScene;
  ```

- [ ] **Step 1: Write the failing tests**

Create `settings/src/pages/displays/canvas.rs` with only the test module and the `use` line it needs:

```rust
//! The Displays drag canvas: snapshot, scene, paint callback, pointer drags.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, HeadEdit, Mode, ModeRequest};
    use crate::pages::displays::state::{DisplaysState, baseline_edit};

    /// A head with one mode, at `(x, y)`.
    fn head(name: &str, w: i32, h: i32, x: i32, y: i32, enabled: bool) -> Head {
        let mode = Mode {
            width: w,
            height: h,
            refresh_mhz: 60_000,
            preferred: true,
        };
        Head {
            name: name.to_string(),
            description: format!("{name} display"),
            enabled,
            modes: vec![mode],
            current_mode: Some(mode),
            x,
            y,
            scale: 1.0,
            transform: 0,
        }
    }

    /// A state with `heads`, baseline edits, and `selected` selected.
    fn state_of(heads: Vec<Head>, selected: Option<usize>) -> DisplaysState {
        let mut st = DisplaysState::new();
        st.edits = heads.iter().map(baseline_edit).collect();
        st.heads = heads;
        st.selected = selected;
        st
    }

    /// Mutation check: make `DisplaysSnapshot::of` read `enabled_rects`
    /// instead of `all_rects`; the disabled head disappears and this fails.
    /// Restore.
    #[test]
    fn a_snapshot_carries_every_head_including_the_disabled_one() {
        let st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1280, 720, 1920, 0, false),
            ],
            Some(1),
        );
        let snap = DisplaysSnapshot::of(&st);
        assert_eq!(snap.names, vec!["DP-1".to_string(), "HDMI-A-1".to_string()]);
        assert_eq!(snap.enabled, vec![true, false]);
        assert_eq!(snap.selected, Some(1));
        assert_eq!(
            snap.details,
            vec!["1920\u{d7}1080".to_string(), "off".to_string()],
            "a disabled head reads `off`, an enabled one its mode"
        );
        assert_eq!(snap.rects.len(), 2);
    }

    /// Mutation check: drop the `mode`-first branch of the detail text so it
    /// always reads the rect's rounded size; the 2x-scaled head then reports
    /// 960x540 and this fails. Restore.
    #[test]
    fn a_scaled_heads_detail_is_its_mode_not_its_logical_size() {
        let mut st = state_of(vec![head("DP-1", 3840, 2160, 0, 0, true)], Some(0));
        st.edits[0].scale = Some(2.0);
        st.edits[0].mode = Some(ModeRequest {
            width: 3840,
            height: 2160,
            refresh_mhz: 60_000,
        });
        let snap = DisplaysSnapshot::of(&st);
        assert_eq!(snap.details, vec!["3840\u{d7}2160".to_string()]);
        // The rect is the *logical* size: 3840/2 x 2160/2.
        assert_eq!(snap.rects[0].w, 1920.0);
        assert_eq!(snap.rects[0].h, 1080.0);
    }

    /// Mutation check: have `scene` ignore its `width`/`height` and pass
    /// `CANVAS_W`/`CANVAS_H` unconditionally; the half-size viewport then
    /// yields the same scale and this fails. Restore.
    #[test]
    fn a_scene_maps_every_tile_through_one_shared_view() {
        let st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1920, 1080, 1920, 0, true),
            ],
            Some(0),
        );
        let snap = DisplaysSnapshot::of(&st);
        let full = scene(&snap, CANVAS_W, CANVAS_H);
        let half = scene(&snap, CANVAS_W / 2.0, CANVAS_H / 2.0);
        assert!(
            half.view.scale < full.view.scale,
            "a smaller viewport must scale down further: {} vs {}",
            half.view.scale,
            full.view.scale
        );
        assert_eq!(full.tiles.len(), 2);
        // Every tile is its layout rect mapped through the scene's own view.
        for (tile, rect) in full.tiles.iter().zip(snap.rects.iter()) {
            assert_eq!(tile.rect, full.view.to_canvas(rect));
        }
        assert!(full.tiles[0].selected);
        assert!(!full.tiles[1].selected);
    }

    /// Mutation check: make `scene` mark every tile selected; this fails on
    /// the `None` case. Restore.
    #[test]
    fn a_scene_with_no_selection_marks_no_tile() {
        let st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], None);
        let sc = scene(&DisplaysSnapshot::of(&st), CANVAS_W, CANVAS_H);
        assert!(!sc.tiles[0].selected);
    }

    /// Mutation check: drop the empty-input guard from `scene`; `compute_view`
    /// still returns its identity view, so assert the tile list instead — make
    /// `scene` push a placeholder tile and this fails. Restore.
    #[test]
    fn an_empty_snapshot_scenes_to_no_tiles_without_panicking() {
        let sc = scene(&DisplaysSnapshot::of(&DisplaysState::new()), CANVAS_W, CANVAS_H);
        assert!(sc.tiles.is_empty());
        assert_eq!(sc.view.scale, 1.0);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: FAIL — `error[E0433]: failed to resolve: use of undeclared crate or module` / `cannot find function 'scene' in this scope`, and `error[E0583]: file not found for module 'canvas'` until Step 3's `pub mod canvas;` lands.

- [ ] **Step 3: Declare the module and write the implementation**

In `settings/src/pages/displays/mod.rs`, add to the existing module list (keep the `state` declaration P1 wrote):

```rust
pub mod canvas;
```

Then prepend the implementation to `settings/src/pages/displays/canvas.rs`, above the test module:

```rust
//! The Displays drag canvas: snapshot, scene, paint callback, pointer drags.
//!
//! The geometry is `displays_canvas`'s, called verbatim: `all_rects` gives the
//! layout-space rectangles, `compute_view` fits them into the drawing area, and
//! `View::to_canvas` maps each one. Nothing here re-derives a scale or a hit
//! test.
//!
//! The drawing area is a **fixed** `CANVAS_W` x `CANVAS_H` box (plan P4-D3).
//! The GTK page stashed the last-drawn `View` into the page state from inside
//! the draw callback; an Elm paint callback may not touch the model, so the
//! two constants are the shared source of truth instead and `update` recomputes
//! the identical `View` for the drag maths.

use crate::pages::displays::state::{self, DisplaysState};
use crate::pages::displays_canvas::{self, Rect, View};

/// The drawing area's content size, in canvas pixels — the GTK page's
/// `set_content_width(360)` / `set_content_height(240)`.
pub const CANVAS_W: f64 = 360.0;
/// See [`CANVAS_W`].
pub const CANVAS_H: f64 = 240.0;
/// The canvas label size — the GTK page's `cr.set_font_size(11.0)`.
pub const CANVAS_FONT_PX: f32 = 11.0;

/// Exactly what the paint callback reads, cheaply cloned out of the model.
///
/// The draw closure is rebuilt on every `view` and must not capture the model
/// (contract §2.8), so it captures one of these instead. Every vector is
/// parallel and indexed by *slot*, not by head index: `all_rects` already
/// collapses the head list into drawable slots.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaysSnapshot {
    /// Layout-space rectangles, one per slot.
    pub rects: Vec<Rect>,
    /// Whether each slot's head is enabled.
    pub enabled: Vec<bool>,
    /// The selected slot, if the selected head is drawable.
    pub selected: Option<usize>,
    /// Connector names, one per slot.
    pub names: Vec<String>,
    /// The second label line: `"{w}x{h}"` when enabled, `"off"` when not.
    pub details: Vec<String>,
}

impl DisplaysSnapshot {
    /// Read the paintable state out of `st`.
    #[must_use]
    pub fn of(st: &DisplaysState) -> DisplaysSnapshot {
        let (idxs, rects, enabled) = state::all_rects(st);
        let mut names = Vec::with_capacity(idxs.len());
        let mut details = Vec::with_capacity(idxs.len());
        let mut selected = None;
        for (slot, &head_idx) in idxs.iter().enumerate() {
            if st.selected == Some(head_idx) {
                selected = Some(slot);
            }
            names.push(
                st.heads
                    .get(head_idx)
                    .map_or_else(String::new, |h| h.name.clone()),
            );
            // The GTK label: the edit's own mode when it names one, else the
            // rectangle's rounded logical size. A disabled head reads `off`.
            details.push(if enabled[slot] {
                let (w, h) = st
                    .edits
                    .get(head_idx)
                    .and_then(|e| e.mode)
                    .map_or_else(
                        || (rects[slot].w.round() as i32, rects[slot].h.round() as i32),
                        |m| (m.width, m.height),
                    );
                format!("{w}\u{d7}{h}")
            } else {
                "off".to_string()
            });
        }
        DisplaysSnapshot {
            rects,
            enabled,
            selected,
            names,
            details,
        }
    }
}

/// One monitor as the painter wants it: a canvas-space rectangle plus the four
/// flags and strings that pick its colours and labels.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadTile {
    /// Canvas-space rectangle.
    pub rect: Rect,
    /// Whether the head is enabled (drives body, border and text colour).
    pub enabled: bool,
    /// Whether the head is selected (accent border).
    pub selected: bool,
    /// Connector name, the first label line.
    pub name: String,
    /// Resolution or `"off"`, the second label line.
    pub detail: String,
}

/// A whole canvas: the layout→canvas mapping the tiles were built with, kept so
/// `update` can reuse the identical `View` for the drag maths.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasScene {
    /// The mapping every tile went through.
    pub view: View,
    /// One tile per drawable head, in draw order.
    pub tiles: Vec<HeadTile>,
}

/// Fit `snap` into a `width` x `height` viewport and map every rectangle.
#[must_use]
pub fn scene(snap: &DisplaysSnapshot, width: f64, height: f64) -> CanvasScene {
    let view = displays_canvas::compute_view(&snap.rects, width, height, state::CANVAS_MARGIN);
    let tiles = snap
        .rects
        .iter()
        .enumerate()
        .map(|(slot, rect)| HeadTile {
            rect: view.to_canvas(rect),
            enabled: snap.enabled.get(slot).copied().unwrap_or(false),
            selected: snap.selected == Some(slot),
            name: snap.names.get(slot).cloned().unwrap_or_default(),
            detail: snap.details.get(slot).cloned().unwrap_or_default(),
        })
        .collect();
    CanvasScene { view, tiles }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: PASS — 5 tests.

- [ ] **Step 5: Run the crate gates**

Run:
```bash
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: both clean.

- [ ] **Step 6: Commit**

```bash
git add settings/src/pages/displays/canvas.rs settings/src/pages/displays/mod.rs
git commit -m "$(cat <<'MSG'
feat(settings): the Displays canvas snapshot and scene

`DisplaysSnapshot` is what the paint callback reads and the only thing it
captures; `scene` fits it through `displays_canvas::compute_view` and maps
every rectangle with the same `View` the drag maths will recompute.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 2: The paint callback and the canvas view

**Files:**
- Modify: `settings/src/pages/displays/canvas.rs`
- Modify: `settings/style.css`
- Test: `settings/src/pages/displays/canvas.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 1's `DisplaysSnapshot`, `CanvasScene`, `scene`, `CANVAS_W`, `CANVAS_H`, `CANVAS_FONT_PX`; from `icedtea-ui` — `icedtea_ui::paint::{PaintCx, fill_paint}`, `icedtea_ui::layout::Rect` (f32; `to_skia()`), `icedtea_ui::css::value::Rgba`, `icedtea_ui::text::{Ellipsize, TextLayout, TextStyle, WrapMode}`, `icedtea_ui::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, LineHeight}`, `skia_rs_safe::canvas::Canvas`, `skia_rs_safe::paint::Style`; the builders `icedtea_ui::view::builders::{DrawingAreaExt, drawing_area, frame}`; `crate::app::{Msg, SettingsModel}`.
- Produces:
  ```rust
  pub const CANVAS_BG: icedtea_ui::css::value::Rgba;
  pub const TILE_ON: icedtea_ui::css::value::Rgba;
  pub const TILE_OFF: icedtea_ui::css::value::Rgba;
  pub const BORDER_SELECTED: icedtea_ui::css::value::Rgba;
  pub const BORDER_ON: icedtea_ui::css::value::Rgba;
  pub const BORDER_OFF: icedtea_ui::css::value::Rgba;
  pub const TEXT_ON: icedtea_ui::css::value::Rgba;
  pub const TEXT_OFF: icedtea_ui::css::value::Rgba;

  pub fn canvas_text_style() -> icedtea_ui::text::TextStyle;
  pub fn draw(snap: DisplaysSnapshot)
      -> impl Fn(&mut skia_rs_safe::canvas::Canvas<'_>, icedtea_ui::layout::Rect,
                 &mut icedtea_ui::paint::PaintCx<'_>) + 'static;
  pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/displays/canvas.rs`'s `mod tests`:

```rust
    /// Mutation check: give `canvas_text_style` a `size_px` of `0.0`; the
    /// assertion on the font size fails. Restore.
    #[test]
    fn the_canvas_text_style_is_an_11px_sans_face() {
        let style = canvas_text_style();
        assert_eq!(style.size_px, CANVAS_FONT_PX);
        assert_eq!(style.weight, 400.0);
        assert_eq!(style.letter_spacing_px, 0.0);
        assert!(!style.families.is_empty(), "a face must be named");
    }

    /// The eight literal colours of the GTK draw func, unchanged.
    ///
    /// Mutation check: change any one channel; this fails. Restore.
    #[test]
    fn the_canvas_palette_is_the_gtk_palette() {
        assert_eq!((CANVAS_BG.r, CANVAS_BG.g, CANVAS_BG.b), (0.12, 0.12, 0.16));
        assert_eq!((TILE_ON.r, TILE_ON.g, TILE_ON.b), (0.26, 0.28, 0.36));
        assert_eq!((TILE_OFF.r, TILE_OFF.g, TILE_OFF.b), (0.17, 0.18, 0.22));
        assert_eq!(
            (BORDER_SELECTED.r, BORDER_SELECTED.g, BORDER_SELECTED.b),
            (0.54, 0.71, 0.98)
        );
        assert_eq!((BORDER_ON.r, BORDER_ON.g, BORDER_ON.b), (0.45, 0.47, 0.55));
        assert_eq!((BORDER_OFF.r, BORDER_OFF.g, BORDER_OFF.b), (0.34, 0.35, 0.42));
        assert_eq!((TEXT_ON.r, TEXT_ON.g, TEXT_ON.b), (0.90, 0.91, 0.95));
        assert_eq!((TEXT_OFF.r, TEXT_OFF.g, TEXT_OFF.b), (0.60, 0.61, 0.66));
        for c in [
            CANVAS_BG,
            TILE_ON,
            TILE_OFF,
            BORDER_SELECTED,
            BORDER_ON,
            BORDER_OFF,
            TEXT_ON,
            TEXT_OFF,
        ] {
            assert_eq!(c.a, 1.0, "every canvas colour is opaque");
        }
    }

    /// Mutation check: drop the `Msg::HeadDragged` registration from
    /// `view`; the kind count falls to two and this fails. Restore.
    #[test]
    fn the_canvas_view_registers_all_three_pointer_kinds() {
        use icedtea_ui::view::EventKind;

        let m = crate::app::SettingsModel::for_test();
        let v = view(&m);
        // `frame(drawing_area(..))` — the handlers sit on the child.
        let area = v.children().first().expect("the frame wraps the canvas");
        let kinds: Vec<EventKind> = area.handler_kinds();
        assert!(kinds.contains(&EventKind::PointerDown));
        assert!(kinds.contains(&EventKind::PointerMotion));
        assert!(kinds.contains(&EventKind::PointerUp));
        assert_eq!(area.id_of(), Some("displays_canvas"));
    }
```

`View::children`, `View::handler_kinds` and `View::id_of` are `icedtea-ui`
introspection helpers M3 already ships for exactly this
(`ui/src/view/mod.rs`); `SettingsModel::for_test()` is P1's test constructor.
If any of the four is absent under the name used here, replace that line with
the equivalent the crate does expose and note it in the commit body — do not
add a new public API to `icedtea-ui` for it (P4 owns exactly one `ui/` file,
`drop_down.rs`).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: FAIL — `cannot find function 'canvas_text_style'`, `cannot find value 'CANVAS_BG'`, `cannot find function 'view'`.

- [ ] **Step 3: Write the implementation**

Add to the top of `settings/src/pages/displays/canvas.rs`'s `use` block:

```rust
use std::rc::Rc;

use icedtea_ui::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, LineHeight, Rgba};
use icedtea_ui::layout::Rect as UiRect;
use icedtea_ui::paint::{PaintCx, fill_paint};
use icedtea_ui::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{DrawingAreaExt, drawing_area, frame};
use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::Style;

use crate::app::{Msg, SettingsModel};
```

Then, after `scene`:

```rust
/// One opaque colour from three channels in 0..=1.
const fn rgb(r: f32, g: f32, b: f32) -> Rgba {
    Rgba { r, g, b, a: 1.0 }
}

/// The canvas backdrop.
pub const CANVAS_BG: Rgba = rgb(0.12, 0.12, 0.16);
/// An enabled monitor's body.
pub const TILE_ON: Rgba = rgb(0.26, 0.28, 0.36);
/// A disabled monitor's body, dimmed.
pub const TILE_OFF: Rgba = rgb(0.17, 0.18, 0.22);
/// The selected monitor's accent border.
pub const BORDER_SELECTED: Rgba = rgb(0.54, 0.71, 0.98);
/// An enabled monitor's border.
pub const BORDER_ON: Rgba = rgb(0.45, 0.47, 0.55);
/// A disabled monitor's border.
pub const BORDER_OFF: Rgba = rgb(0.34, 0.35, 0.42);
/// An enabled monitor's label.
pub const TEXT_ON: Rgba = rgb(0.90, 0.91, 0.95);
/// A disabled monitor's label.
pub const TEXT_OFF: Rgba = rgb(0.60, 0.61, 0.66);

/// The selected monitor's border width, in canvas pixels.
const BORDER_SELECTED_PX: f32 = 2.5;
/// Every other monitor's border width.
const BORDER_PLAIN_PX: f32 = 1.0;
/// Label inset from the tile's left edge.
const LABEL_X: f64 = 6.0;
/// The name's *baseline* offset from the tile's top edge (cairo semantics).
const NAME_BASELINE: f64 = 16.0;
/// The detail line's baseline offset.
const DETAIL_BASELINE: f64 = 30.0;

/// The face the canvas labels are shaped with.
///
/// The GTK page used cairo's toy text API with `set_font_size(11.0)` and no
/// family at all, so there is no computed style to read here; this is the
/// registry's initial font at the same size.
#[must_use]
pub fn canvas_text_style() -> TextStyle {
    TextStyle {
        families: Rc::from(vec![FontFamily::Generic(GenericFamily::SansSerif)]),
        weight: 400.0,
        style: FontStyle::Normal,
        stretch: 100.0,
        size_px: CANVAS_FONT_PX,
        letter_spacing_px: 0.0,
        features: Rc::from(Vec::new()),
        variations: Rc::from(Vec::new()),
        transform: Keyword::None,
        line_height: LineHeight::Normal,
    }
}

/// Draw one string with its cairo *baseline* at `baseline_y` (plan P4-D11:
/// `TextLayout::draw`'s origin is the layout box's top-left, cairo's
/// `move_to` is a baseline, and the conversion is one font size).
fn draw_label(
    canvas: &mut Canvas<'_>,
    cx: &mut PaintCx<'_>,
    text: &str,
    x: f64,
    baseline_y: f64,
    colour: Rgba,
) {
    if text.is_empty() {
        return;
    }
    let style = canvas_text_style();
    let layout = TextLayout::build(text, &style, cx.fonts, None, WrapMode::None, Ellipsize::End);
    layout.draw(
        canvas,
        (x as f32, baseline_y as f32 - CANVAS_FONT_PX),
        colour,
    );
}

/// The `Prop::Draw` callback for `snap`.
///
/// The cairo calls of the GTK page map one for one: `set_source_rgb` +
/// `rectangle` + `fill` becomes `draw_rect` with [`fill_paint`];
/// `set_line_width` + `stroke` becomes a `Style::Stroke` paint; `set_font_size`
/// + `move_to` + `show_text` becomes a [`TextLayout`] through `cx.fonts`, which
/// is why M5-D6 widened the callback to carry `&mut PaintCx`.
///
/// The closure captures `snap` by value and never the model (contract §2.8).
#[must_use]
pub fn draw(
    snap: DisplaysSnapshot,
) -> impl Fn(&mut Canvas<'_>, UiRect, &mut PaintCx<'_>) + 'static {
    move |canvas, rect, cx| {
        // Neutral background across the whole content box.
        canvas.draw_rect(&rect.to_skia().clone(), &fill_paint(CANVAS_BG));

        let sc = scene(&snap, f64::from(rect.width), f64::from(rect.height));
        for tile in &sc.tiles {
            let r = UiRect::new(
                rect.x + tile.rect.x as f32,
                rect.y + tile.rect.y as f32,
                tile.rect.w as f32,
                tile.rect.h as f32,
            );
            // Body.
            let body = if tile.enabled { TILE_ON } else { TILE_OFF };
            canvas.draw_rect(&r.to_skia(), &fill_paint(body));
            // Border.
            let (edge, width) = if tile.selected {
                (BORDER_SELECTED, BORDER_SELECTED_PX)
            } else if tile.enabled {
                (BORDER_ON, BORDER_PLAIN_PX)
            } else {
                (BORDER_OFF, BORDER_PLAIN_PX)
            };
            let mut stroke = fill_paint(edge);
            stroke.set_style(Style::Stroke);
            stroke.set_stroke_width(width);
            canvas.draw_rect(&r.to_skia(), &stroke);
            // Labels.
            let ink = if tile.enabled { TEXT_ON } else { TEXT_OFF };
            let x = f64::from(rect.x) + tile.rect.x + LABEL_X;
            draw_label(
                canvas,
                cx,
                &tile.name,
                x,
                f64::from(rect.y) + tile.rect.y + NAME_BASELINE,
                ink,
            );
            draw_label(
                canvas,
                cx,
                &tile.detail,
                x,
                f64::from(rect.y) + tile.rect.y + DETAIL_BASELINE,
                ink,
            );
        }
    }
}

/// The canvas, framed, with its three pointer handlers.
///
/// Fixed size (plan P4-D3): `hexpand`/`vexpand` are off and the content size is
/// `CANVAS_W` x `CANVAS_H`, so `Prop::Draw`'s rectangle and the coordinates the
/// pointer handlers receive are the same box, and `update` can recompute the
/// identical `View` from the two constants.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    frame(
        drawing_area(draw(DisplaysSnapshot::of(&m.displays)))
            .content_width(CANVAS_W as i32)
            .content_height(CANVAS_H as i32)
            .id("displays_canvas")
            .hexpand(false)
            .vexpand(false)
            .on_pointer_down(Msg::HeadDragBegan)
            .on_pointer_motion(Msg::HeadDragged)
            .on_pointer_up(Msg::HeadDragEnded),
    )
}
```

- [ ] **Step 4: Add the stylesheet rule**

Append to `settings/style.css`:

```css
/* The Displays canvas: `Event::PointerDown`'s `local` is border-box relative
   while `Prop::Draw`'s rect is the content box. With no padding, border or
   margin the two coincide, which is what makes the canvas hit test correct
   (plan P4-D4). */
#displays_canvas {
    padding: 0;
    border: 0 none;
    margin: 0;
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: PASS — 8 tests.

- [ ] **Step 6: Run the crate gates**

Run:
```bash
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: both clean.

- [ ] **Step 7: Commit**

```bash
git add settings/src/pages/displays/canvas.rs settings/style.css
git commit -m "$(cat <<'MSG'
feat(settings): paint the Displays canvas through PaintCx

The eight GTK colours and both label offsets are unchanged; the cairo
baseline becomes a TextLayout origin one font size higher (P4-D11). The
drawing area is pinned to 360x240 with a zero-padding rule so the paint
rect and the pointer-local coordinates are one box (P4-D3, P4-D4).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 3: Pointer drags — hit test, snap, release

**Files:**
- Modify: `settings/src/pages/displays/canvas.rs`
- Modify: `settings/src/app.rs` (the `HeadDragBegan` / `HeadDragged` / `HeadDragEnded` arms)
- Test: `settings/src/pages/displays/canvas.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 1's `CANVAS_W`/`CANVAS_H`; from P1 — `state::{CANVAS_MARGIN, SNAP_THRESHOLD, Drag, all_rects, enabled_rects}` where `Drag { head: usize, origin: (f64, f64), start: (i32, i32) }`; `displays_canvas::{Rect, compute_view, hit_test, snap}` where `snap(dragged: Rect, others: &[Rect], threshold: f64) -> (i32, i32)` and `View::canvas_delta_to_layout(&self, dx: f64, dy: f64) -> (f64, f64)`; `crate::app::{Msg, SettingsModel}`; `icedtea_ui::view::Cmd`.
- Produces:
  ```rust
  pub fn drag_began(st: &mut DisplaysState, x: f64, y: f64);
  pub fn dragged(st: &mut DisplaysState, x: f64, y: f64);
  pub fn drag_ended(st: &mut DisplaysState, x: f64, y: f64);
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/displays/canvas.rs`'s `mod tests`:

```rust
    /// The canvas-space centre of `slot`'s tile at the fixed canvas size.
    fn tile_centre(st: &DisplaysState, slot: usize) -> (f64, f64) {
        let sc = scene(&DisplaysSnapshot::of(st), CANVAS_W, CANVAS_H);
        let r = sc.tiles[slot].rect;
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    /// Contract §2.8: "on a miss clear `drag` (selection is left alone)".
    ///
    /// Mutation check: make the miss branch `st.selected = None`; this fails.
    /// Restore.
    #[test]
    fn a_drag_that_hits_nothing_leaves_the_selection_alone() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        // The canvas margin corner is guaranteed to be outside every tile.
        drag_began(&mut st, 0.0, 0.0);
        assert_eq!(st.selected, Some(0), "a miss must not clear the selection");
        assert!(st.drag.is_none(), "a miss must not start a drag");
        assert!(!st.dirty, "a miss must not dirty the page");
    }

    /// Mutation check: pass an empty `others` slice to `snap`; the head lands
    /// at its raw dragged position and this fails. Restore.
    #[test]
    fn a_drag_snaps_the_dragged_head_against_its_neighbour() {
        let mut st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1920, 1080, 4000, 0, true),
            ],
            Some(1),
        );
        let (cx, cy) = tile_centre(&st, 1);
        drag_began(&mut st, cx, cy);
        assert_eq!(st.drag.map(|d| d.head), Some(1));
        assert_eq!(st.drag.map(|d| d.start), Some((4000, 0)));

        // Drag left by exactly the canvas distance that is 2080 layout px,
        // putting the head's left edge at 1920 — the neighbour's right edge.
        let view = st.view;
        let dx_canvas = -2080.0 * view.scale;
        dragged(&mut st, cx + dx_canvas, cy);
        assert_eq!(
            st.edits[1].position,
            Some((1920, 0)),
            "the head must snap flush against DP-1's right edge"
        );
        assert!(st.dirty, "a move dirties the page");
    }

    /// Contract §2.8 / M5-D5 §3: a `PointerUp` outside the node still arrives
    /// through the implicit grab, so a drag can never wedge.
    ///
    /// Mutation check: make `drag_ended` return early without clearing
    /// `st.drag`; this fails. Restore.
    #[test]
    fn a_release_outside_the_canvas_ends_the_drag() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        assert!(st.drag.is_some());
        // Far outside the 360x240 canvas, in both axes.
        drag_ended(&mut st, -400.0, 900.0);
        assert!(st.drag.is_none(), "the release must end the drag");
    }

    /// A drag whose head vanished mid-gesture (a hotplug clears `heads` and
    /// `edits`) must bail, not index out of bounds.
    ///
    /// Mutation check: drop the `drag.head >= st.edits.len()` guard in
    /// `dragged`; this panics instead of failing. Restore.
    #[test]
    fn a_drag_whose_head_disappeared_is_dropped_not_indexed() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        st.heads.clear();
        st.edits.clear();
        dragged(&mut st, cx + 40.0, cy);
        assert!(st.drag.is_none(), "a stale drag must be dropped");
    }

    /// `drag_began` must publish the view the paint used, so the delta maths
    /// and the hit test agree.
    ///
    /// Mutation check: have `drag_began` compute the view with a margin of
    /// `0.0`; the scales differ and this fails. Restore.
    #[test]
    fn a_drag_publishes_the_same_view_the_paint_computed() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let painted = scene(&DisplaysSnapshot::of(&st), CANVAS_W, CANVAS_H).view;
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        assert_eq!(st.view, painted);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: FAIL — `cannot find function 'drag_began' in this scope` (and the same for `dragged`, `drag_ended`).

- [ ] **Step 3: Write the implementation**

Append to `settings/src/pages/displays/canvas.rs`, after `view`:

```rust
/// Begin a drag at canvas point `(x, y)`.
///
/// Recomputes the view the paint used (plan P4-D3) and publishes it into
/// `st.view` so [`dragged`]'s delta maths reads the same mapping. Hit-tests
/// against **every** head, disabled ones included, so a disabled monitor can be
/// re-selected and switched back on (state.rs finding #10). A miss clears the
/// drag and leaves the selection alone (contract §2.8).
pub fn drag_began(st: &mut DisplaysState, x: f64, y: f64) {
    let (idxs, rects, _enabled) = state::all_rects(st);
    let view = displays_canvas::compute_view(&rects, CANVAS_W, CANVAS_H, state::CANVAS_MARGIN);
    st.view = view;
    match displays_canvas::hit_test(&rects, &view, x, y) {
        Some(slot) => {
            let Some(&head) = idxs.get(slot) else {
                st.drag = None;
                return;
            };
            let start = st
                .edits
                .get(head)
                .and_then(|e| e.position)
                .unwrap_or((0, 0));
            st.selected = Some(head);
            st.drag = Some(state::Drag {
                head,
                origin: (x, y),
                start,
            });
        }
        None => st.drag = None,
    }
}

/// Continue a drag at canvas point `(x, y)`.
///
/// The canvas delta from the press point becomes a layout delta through the
/// published view; the dragged head's own rectangle (which may be a disabled
/// head's placeholder) gives the width and height `snap` needs; the snap targets
/// are the *other* enabled heads plus the origin.
pub fn dragged(st: &mut DisplaysState, x: f64, y: f64) {
    let Some(drag) = st.drag else {
        return;
    };
    // A `HeadsChanged` mid-drag replaces `heads`/`edits` and clears `drag`, but
    // guard the cached index anyway: a stale drag bails rather than indexing
    // out of bounds (state.rs finding #2).
    if drag.head >= st.edits.len() {
        st.drag = None;
        return;
    }
    let (dx, dy) = st
        .view
        .canvas_delta_to_layout(x - drag.origin.0, y - drag.origin.1);
    let (aidxs, arects, _enabled) = state::all_rects(st);
    let size = aidxs
        .iter()
        .position(|&i| i == drag.head)
        .map_or((0.0, 0.0), |slot| (arects[slot].w, arects[slot].h));
    let candidate = Rect::new(
        f64::from(drag.start.0) + dx,
        f64::from(drag.start.1) + dy,
        size.0,
        size.1,
    );
    let (eidxs, erects) = state::enabled_rects(st);
    let others: Vec<Rect> = eidxs
        .iter()
        .zip(erects.iter())
        .filter(|&(&i, _)| i != drag.head)
        .map(|(_, r)| *r)
        .collect();
    let (nx, ny) = displays_canvas::snap(candidate, &others, state::SNAP_THRESHOLD);
    st.edits[drag.head].position = Some((nx, ny));
    st.dirty = true;
}

/// End a drag at canvas point `(x, y)`: one last fold, then the drag is gone.
///
/// The release is delivered even when it lands outside the canvas (M5-D5 §3's
/// grab semantics), which is what stops a drag wedging.
pub fn drag_ended(st: &mut DisplaysState, x: f64, y: f64) {
    dragged(st, x, y);
    st.drag = None;
}
```

- [ ] **Step 4: Wire the three arms in `settings/src/app.rs`**

Replace the three stub arms P1 left in `update`'s `match`:

```rust
        Msg::HeadDragBegan(x, y) => {
            crate::pages::displays::canvas::drag_began(&mut m.displays, x, y);
            crate::pages::displays::controls::repopulate(&mut m.displays);
            Cmd::None
        }
        Msg::HeadDragged(x, y) => {
            crate::pages::displays::canvas::dragged(&mut m.displays, x, y);
            Cmd::None
        }
        Msg::HeadDragEnded(x, y) => {
            crate::pages::displays::canvas::drag_ended(&mut m.displays, x, y);
            Cmd::None
        }
```

`controls::repopulate` lands in Task 4; until then, comment those two calls out
with a `// Task 4` marker and uncomment them there. (This is the one forward
reference in the part, and it is closed inside the next task.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays::canvas`
Expected: PASS — 13 tests.

- [ ] **Step 6: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings --lib
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean; the 8 `displays_canvas` tests and the 12 `displays::state` tests still pass byte-identically.

- [ ] **Step 7: Commit**

```bash
git add settings/src/pages/displays/canvas.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): Displays canvas drag through view-layer pointer events

`hit_test`, `compute_view`, `snap` and `canvas_delta_to_layout` are called,
never reimplemented. A miss leaves the selection alone, a release outside
the canvas still ends the drag, and a head that vanished mid-gesture drops
the drag instead of indexing out of bounds.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 4: The per-head control panel — option lists and view

**Files:**
- Create: `settings/src/pages/displays/controls.rs`
- Modify: `settings/src/pages/displays/mod.rs` (add `pub mod controls;`)
- Test: `settings/src/pages/displays/controls.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: from P1 — `state::{DisplaysState, TRANSFORM_LABELS, TRANSFORM_VALUES, distinct_resolutions, refreshes_for, format_refresh}`; `crate::app::{Msg, SettingsModel}`; from `icedtea-ui` — `icedtea_ui::view::builders::{DropDownExt, SpinButtonExt, box_, drop_down, grid, label, spin_button, switch, GridExt}`, `icedtea_ui::widgets::types::{Align, Orientation}`, `icedtea_ui::view::View`.
- Produces:
  ```rust
  pub fn repopulate(st: &mut crate::pages::displays::state::DisplaysState);
  pub fn resolution_labels(st: &crate::pages::displays::state::DisplaysState) -> Vec<String>;
  pub fn refresh_labels(st: &crate::pages::displays::state::DisplaysState) -> Vec<String>;
  pub fn resolution_index(st: &crate::pages::displays::state::DisplaysState) -> usize;
  pub fn refresh_index(st: &crate::pages::displays::state::DisplaysState) -> usize;
  pub fn transform_index(st: &crate::pages::displays::state::DisplaysState) -> usize;
  pub fn position_text(st: &crate::pages::displays::state::DisplaysState) -> String;
  pub fn scale_value(st: &crate::pages::displays::state::DisplaysState) -> f64;
  pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  ```

- [ ] **Step 1: Write the failing tests**

Create `settings/src/pages/displays/controls.rs` with only its test module:

```rust
//! The Displays page's per-head control panel.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, Mode, ModeRequest};
    use crate::pages::displays::state::{DisplaysState, baseline_edit};

    fn mode(w: i32, h: i32, mhz: i32, preferred: bool) -> Mode {
        Mode {
            width: w,
            height: h,
            refresh_mhz: mhz,
            preferred,
        }
    }

    /// A head advertising 1920x1080@60/144 and 1280x720@60.
    fn multi_mode_head() -> Head {
        Head {
            name: "DP-1".to_string(),
            description: "DP-1 display".to_string(),
            enabled: true,
            modes: vec![
                mode(1920, 1080, 60_000, true),
                mode(1920, 1080, 144_000, false),
                mode(1280, 720, 60_000, false),
            ],
            current_mode: Some(mode(1920, 1080, 60_000, true)),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    fn state_with(head: Head) -> DisplaysState {
        let mut st = DisplaysState::new();
        st.edits = vec![baseline_edit(&head)];
        st.heads = vec![head];
        st.selected = Some(0);
        repopulate(&mut st);
        st
    }

    /// Mutation check: have `repopulate` build `refresh_options` from the
    /// head's *whole* mode list instead of `refreshes_for` at the selected
    /// resolution; 1280x720's 60 Hz reappears and the length assertion fails.
    /// Restore.
    #[test]
    fn repopulate_fills_the_option_lists_for_the_selected_head() {
        let st = state_with(multi_mode_head());
        assert_eq!(st.res_options, vec![(1920, 1080), (1280, 720)]);
        assert_eq!(st.refresh_options, vec![144_000, 60_000]);
        assert_eq!(resolution_labels(&st), vec!["1920\u{d7}1080", "1280\u{d7}720"]);
        assert_eq!(refresh_labels(&st), vec!["144.00 Hz", "60.00 Hz"]);
    }

    /// Mutation check: have `resolution_index` return `0` unconditionally;
    /// the 1280x720 assertion fails. Restore.
    #[test]
    fn the_dropdown_indices_follow_the_edit() {
        let mut st = state_with(multi_mode_head());
        assert_eq!(resolution_index(&st), 0);
        assert_eq!(refresh_index(&st), 1, "60 Hz is second, 144 Hz first");
        assert_eq!(transform_index(&st), 0);

        st.edits[0].mode = Some(ModeRequest {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
        });
        st.edits[0].transform = Some(6);
        repopulate(&mut st);
        assert_eq!(resolution_index(&st), 1);
        assert_eq!(refresh_index(&st), 0, "720p offers only 60 Hz");
        assert_eq!(transform_index(&st), 6, "Flipped 180 is TRANSFORM_VALUES[6]");
    }

    /// Mutation check: return `"—"` from `position_text` unconditionally; this
    /// fails on the selected case. Restore.
    #[test]
    fn the_position_text_is_the_edits_position_or_an_em_dash() {
        let mut st = state_with(multi_mode_head());
        assert_eq!(position_text(&st), "0, 0");
        st.edits[0].position = Some((1920, -180));
        assert_eq!(position_text(&st), "1920, -180");
        st.selected = None;
        repopulate(&mut st);
        assert_eq!(position_text(&st), "\u{2014}");
    }

    /// With nothing selected every list empties and the scale falls back to 1.
    ///
    /// Mutation check: drop the `_ =>` arm of `repopulate`; the stale option
    /// lists survive and this fails. Restore.
    #[test]
    fn no_selection_clears_the_option_lists() {
        let mut st = state_with(multi_mode_head());
        st.selected = None;
        repopulate(&mut st);
        assert!(st.res_options.is_empty());
        assert!(st.refresh_options.is_empty());
        assert_eq!(scale_value(&st), 1.0);
    }

    /// A selection index past the end of `heads` is treated as no selection —
    /// never an out-of-bounds index.
    ///
    /// Mutation check: index `st.heads[idx]` directly; this panics. Restore.
    #[test]
    fn a_stale_selection_index_is_treated_as_no_selection() {
        let mut st = state_with(multi_mode_head());
        st.selected = Some(7);
        repopulate(&mut st);
        assert!(st.res_options.is_empty());
        assert_eq!(position_text(&st), "\u{2014}");
    }

    /// Mutation check: drop the `.id("displays_transform")` from `view`; the
    /// id assertion fails. Restore.
    #[test]
    fn the_control_panel_ids_are_the_contract_ids() {
        let m = crate::app::SettingsModel::for_test();
        let ids = icedtea_ui::view::ids_of(&view(&m));
        for id in [
            "displays_enabled",
            "displays_resolution",
            "displays_refresh",
            "displays_scale",
            "displays_transform",
            "displays_position",
        ] {
            assert!(ids.iter().any(|got| got == id), "missing id {id}; got {ids:?}");
        }
    }
```

Close the module with `}`.

`icedtea_ui::view::ids_of(&View<Msg>) -> Vec<String>` is M3's view-tree
introspection helper. If the crate names it differently, use the name it does
export and say so in the commit body; do not add one.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::controls`
Expected: FAIL — `error[E0583]: file not found for module 'controls'` until the module is declared, then `cannot find function 'repopulate'`.

- [ ] **Step 3: Write the implementation**

In `settings/src/pages/displays/mod.rs` add:

```rust
pub mod controls;
```

Then prepend to `settings/src/pages/displays/controls.rs`:

```rust
//! The Displays page's per-head control panel.
//!
//! The GTK page kept a `refresh_controls` closure that both recomputed the
//! option lists *and* wrote them into six widgets. On an Elm loop the writes
//! disappear: [`repopulate`] keeps only the model half — the two option
//! vectors a dropdown index maps back through — and [`view`] renders them.

use icedtea_ui::view::View;
use icedtea_ui::view::builders::{
    DropDownExt, GridExt, SpinButtonExt, box_, drop_down, grid, label, spin_button, switch,
};
use icedtea_ui::widgets::types::{Align, Orientation};

use crate::app::{Msg, SettingsModel};
use crate::pages::displays::state::{
    self, DisplaysState, TRANSFORM_LABELS, TRANSFORM_VALUES, distinct_resolutions, format_refresh,
    refreshes_for,
};

/// The scale spinner's bounds and step — the GTK
/// `SpinButton::with_range(0.5, 4.0, 0.25)` with `set_digits(2)`.
pub const SCALE_MIN: f64 = 0.5;
/// See [`SCALE_MIN`].
pub const SCALE_MAX: f64 = 4.0;
/// See [`SCALE_MIN`].
pub const SCALE_STEP: f64 = 0.25;
/// See [`SCALE_MIN`].
pub const SCALE_DIGITS: u32 = 2;

/// The selected head's index, or `None` when there is no live selection.
fn selection(st: &DisplaysState) -> Option<usize> {
    st.selected.filter(|&i| i < st.heads.len() && i < st.edits.len())
}

/// Recompute `res_options` / `refresh_options` for the current selection.
///
/// Every `update` arm that changes the selection or an edit calls this, exactly
/// where the GTK page called `refresh_controls`.
pub fn repopulate(st: &mut DisplaysState) {
    let Some(idx) = selection(st) else {
        st.res_options.clear();
        st.refresh_options.clear();
        return;
    };
    let modes = st.heads[idx].modes.clone();
    let edit = st.edits[idx].clone();

    let res_options = distinct_resolutions(&modes);
    let (cur_w, cur_h) = edit.mode.map_or((0, 0), |m| (m.width, m.height));
    let res_sel = res_options
        .iter()
        .position(|&(w, h)| w == cur_w && h == cur_h)
        .unwrap_or(0);
    let (sel_w, sel_h) = res_options.get(res_sel).copied().unwrap_or((cur_w, cur_h));
    let refresh_options = refreshes_for(&modes, sel_w, sel_h);

    st.res_options = res_options;
    st.refresh_options = refresh_options;
}

/// `"{w}x{h}"` per distinct resolution, in `res_options` order.
#[must_use]
pub fn resolution_labels(st: &DisplaysState) -> Vec<String> {
    st.res_options
        .iter()
        .map(|(w, h)| format!("{w}\u{d7}{h}"))
        .collect()
}

/// `"{n:.2} Hz"` per refresh rate, in `refresh_options` order.
#[must_use]
pub fn refresh_labels(st: &DisplaysState) -> Vec<String> {
    st.refresh_options
        .iter()
        .map(|&r| format_refresh(r))
        .collect()
}

/// The resolution dropdown's selected index; `0` when the edit names none.
#[must_use]
pub fn resolution_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let (w, h) = st.edits[idx].mode.map_or((0, 0), |m| (m.width, m.height));
    st.res_options
        .iter()
        .position(|&(ow, oh)| ow == w && oh == h)
        .unwrap_or(0)
}

/// The refresh dropdown's selected index; `0` when the edit names none.
#[must_use]
pub fn refresh_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let r = st.edits[idx].mode.map_or(0, |m| m.refresh_mhz);
    st.refresh_options
        .iter()
        .position(|&o| o == r)
        .unwrap_or(0)
}

/// The transform dropdown's selected index, over all eight raw
/// `wl_output.transform` values (state.rs finding #8).
#[must_use]
pub fn transform_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let t = st.edits[idx].transform.unwrap_or(0);
    TRANSFORM_VALUES.iter().position(|&v| v == t).unwrap_or(0)
}

/// The scale spinner's value; `1.0` when the edit names none.
#[must_use]
pub fn scale_value(st: &DisplaysState) -> f64 {
    selection(st).map_or(1.0, |idx| st.edits[idx].scale.unwrap_or(1.0))
}

/// `"{x}, {y}"`, or an em dash when nothing is selected.
#[must_use]
pub fn position_text(st: &DisplaysState) -> String {
    match selection(st) {
        Some(idx) => {
            let (x, y) = st.edits[idx].position.unwrap_or((0, 0));
            format!("{x}, {y}")
        }
        None => "\u{2014}".to_string(),
    }
}

/// One labelled grid row: a start-aligned label in column 0, the control in 1.
fn row<'a>(text: &str, r: u16, control: View<Msg>) -> [View<Msg>; 2] {
    [
        label(text).halign(Align::Start).at(0, r),
        control.at(1, r),
    ]
}

/// The per-head panel: enabled, resolution, refresh, scale, transform,
/// position — the same six controls, in the same order, as the GTK grid.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let st = &m.displays;
    let live = selection(st).is_some();
    let enabled = selection(st).is_some_and(|i| st.edits[i].enabled);

    let res_labels = resolution_labels(st);
    let res_refs: Vec<&str> = res_labels.iter().map(String::as_str).collect();
    let ref_labels = refresh_labels(st);
    let ref_refs: Vec<&str> = ref_labels.iter().map(String::as_str).collect();

    let mut children: Vec<View<Msg>> = Vec::with_capacity(12);
    children.extend(row(
        "Enabled",
        0,
        switch(enabled)
            .id("displays_enabled")
            .halign(Align::Start)
            .sensitive(live)
            .on_toggle(Msg::HeadEnabledToggled),
    ));
    children.extend(row(
        "Resolution",
        1,
        drop_down(&res_refs)
            .selected(resolution_index(st))
            .id("displays_resolution")
            .sensitive(live && !res_refs.is_empty())
            .on_selected(Msg::ResolutionSelected),
    ));
    children.extend(row(
        "Refresh",
        2,
        drop_down(&ref_refs)
            .selected(refresh_index(st))
            .id("displays_refresh")
            .sensitive(live && !ref_refs.is_empty())
            .on_selected(Msg::RefreshSelected),
    ));
    children.extend(row(
        "Scale",
        3,
        spin_button(scale_value(st), SCALE_MIN, SCALE_MAX)
            .step(SCALE_STEP)
            .digits(SCALE_DIGITS)
            .id("displays_scale")
            .sensitive(live)
            .on_value_changed(Msg::HeadScaleChanged),
    ));
    children.extend(row(
        "Transform",
        4,
        drop_down(&TRANSFORM_LABELS)
            .selected(transform_index(st))
            .id("displays_transform")
            .sensitive(live)
            .on_selected(Msg::TransformSelected),
    ));
    children.extend(row(
        "Position",
        5,
        label(&position_text(st))
            .id("displays_position")
            .halign(Align::Start),
    ));

    box_(
        Orientation::Vertical,
        [grid(children).row_spacing(8u32).column_spacing(12u32)],
    )
    .id("displays_controls")
}
```

`state::{self, ...}` is imported for symmetry with `canvas.rs`; drop the `self`
if clippy reports it unused.

- [ ] **Step 4: Close Task 3's forward reference**

Uncomment the `controls::repopulate(&mut m.displays);` call in `app.rs`'s
`Msg::HeadDragBegan` arm and delete the `// Task 4` marker.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays`
Expected: PASS — 13 canvas tests + 6 controls tests + the 12 moved `state` tests.

- [ ] **Step 6: Run the crate gates**

Run:
```bash
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: both clean.

- [ ] **Step 7: Commit**

```bash
git add settings/src/pages/displays/controls.rs settings/src/pages/displays/mod.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): the Displays per-head control panel

`repopulate` keeps the GTK `refresh_controls` closure's model half — the two
option vectors a dropdown index maps back through — and drops its six widget
writes. Selection is validated against both `heads` and `edits`, so a stale
index reads as no selection instead of panicking.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 5: The control mutators

**Files:**
- Modify: `settings/src/pages/displays/controls.rs`
- Modify: `settings/src/app.rs` (the six control arms)
- Test: `settings/src/pages/displays/controls.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 4's `repopulate`/`selection`; from P1 — `state::default_mode_for(head: &Head) -> Option<ModeRequest>`, `state::TRANSFORM_VALUES`; `crate::outputs::ModeRequest`.
- Produces:
  ```rust
  pub fn select_head(st: &mut DisplaysState, index: usize);
  pub fn set_enabled(st: &mut DisplaysState, on: bool);
  pub fn pick_resolution(st: &mut DisplaysState, index: usize);
  pub fn pick_refresh(st: &mut DisplaysState, index: usize);
  pub fn pick_transform(st: &mut DisplaysState, index: usize);
  pub fn set_scale(st: &mut DisplaysState, value: f64);
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/displays/controls.rs`'s `mod tests`:

```rust
    /// A head that advertises modes but whose edit names none — the state a
    /// disabled head arrives in.
    fn mode_less_head() -> Head {
        let mut head = multi_mode_head();
        head.enabled = false;
        head.current_mode = None;
        head
    }

    /// Contract §2.8 / state.rs finding #9.
    ///
    /// Mutation check: delete the `default_mode_for` call from `set_enabled`;
    /// the mode stays `None` and this fails. Restore.
    #[test]
    fn enabling_a_mode_less_head_gives_it_a_default_mode() {
        let mut st = state_with(mode_less_head());
        st.edits[0].mode = None;
        st.edits[0].enabled = false;
        repopulate(&mut st);

        set_enabled(&mut st, true);
        assert!(st.edits[0].enabled);
        assert_eq!(
            st.edits[0].mode,
            Some(ModeRequest {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            }),
            "the preferred advertised mode is adopted so the head is placeable"
        );
        assert!(st.dirty);
        assert!(!st.res_options.is_empty(), "the option lists repopulate");
    }

    /// Mutation check: have `pick_resolution` always take `refreshes.first()`;
    /// the retained-144 assertion fails. Restore.
    #[test]
    fn picking_a_resolution_keeps_the_refresh_when_it_is_offered() {
        let mut st = state_with(multi_mode_head());
        st.edits[0].mode = Some(ModeRequest {
            width: 1920,
            height: 1080,
            refresh_mhz: 144_000,
        });
        repopulate(&mut st);
        // Re-pick 1920x1080: 144 Hz is still offered, so it survives.
        pick_resolution(&mut st, 0);
        assert_eq!(st.edits[0].mode.map(|m| m.refresh_mhz), Some(144_000));
        // Pick 1280x720, which offers only 60 Hz: the highest is taken.
        pick_resolution(&mut st, 1);
        assert_eq!(
            st.edits[0].mode,
            Some(ModeRequest {
                width: 1280,
                height: 720,
                refresh_mhz: 60_000,
            })
        );
        assert!(st.dirty);
    }

    /// state.rs finding #14: a refresh pick on a mode-less head synthesizes a
    /// whole mode from the first resolution rather than being dropped.
    ///
    /// Mutation check: delete the `else if` branch of `pick_refresh`; the mode
    /// stays `None` and this fails. Restore.
    #[test]
    fn picking_a_refresh_on_a_mode_less_head_synthesizes_a_mode() {
        let mut st = state_with(mode_less_head());
        st.edits[0].mode = None;
        repopulate(&mut st);
        // With no mode the refresh list is built for the first resolution.
        pick_refresh(&mut st, 0);
        assert_eq!(
            st.edits[0].mode,
            Some(ModeRequest {
                width: 1920,
                height: 1080,
                refresh_mhz: 144_000,
            })
        );
        assert!(st.dirty);
    }

    /// Mutation check: have `pick_transform` write the *index* instead of
    /// `TRANSFORM_VALUES[index]`; index 6 happens to equal value 6, so use
    /// the out-of-range case below to catch it — an index past the end must
    /// fall back to 0, not write 9. Restore.
    #[test]
    fn picking_a_transform_writes_a_raw_wl_output_value() {
        let mut st = state_with(multi_mode_head());
        pick_transform(&mut st, 5);
        assert_eq!(st.edits[0].transform, Some(TRANSFORM_VALUES[5]));
        pick_transform(&mut st, 9);
        assert_eq!(st.edits[0].transform, Some(0), "out of range falls back");
    }

    /// Mutation check: drop the `dirty = true` from `set_scale`; this fails.
    /// Restore.
    #[test]
    fn setting_a_scale_writes_it_and_dirties_the_page() {
        let mut st = state_with(multi_mode_head());
        set_scale(&mut st, 1.75);
        assert_eq!(st.edits[0].scale, Some(1.75));
        assert!(st.dirty);
    }

    /// Selecting a head repopulates the option lists for *that* head.
    ///
    /// Mutation check: drop the `repopulate` call from `select_head`; the
    /// 720p-only head's option list keeps the first head's entries and this
    /// fails. Restore.
    #[test]
    fn selecting_a_head_repopulates_for_it() {
        let mut st = state_with(multi_mode_head());
        let mut second = multi_mode_head();
        second.name = "HDMI-A-1".to_string();
        second.modes = vec![mode(1280, 720, 60_000, true)];
        second.current_mode = Some(mode(1280, 720, 60_000, true));
        st.edits.push(baseline_edit(&second));
        st.heads.push(second);

        select_head(&mut st, 1);
        assert_eq!(st.selected, Some(1));
        assert_eq!(st.res_options, vec![(1280, 720)]);
        assert!(!st.dirty, "selecting is not an edit");
    }

    /// An out-of-range selection is ignored rather than stored.
    ///
    /// Mutation check: drop the bounds check from `select_head`; the later
    /// `position_text` reads an em dash but `st.selected` is `Some(7)`, and
    /// this fails. Restore.
    #[test]
    fn selecting_a_head_that_does_not_exist_is_ignored() {
        let mut st = state_with(multi_mode_head());
        select_head(&mut st, 7);
        assert_eq!(st.selected, Some(0));
    }

    /// Every mutator with nothing selected is a no-op, never a panic.
    ///
    /// Mutation check: drop any `let Some(idx) = selection(st) else` guard;
    /// this panics. Restore.
    #[test]
    fn every_control_mutator_is_a_no_op_with_no_selection() {
        let mut st = state_with(multi_mode_head());
        st.selected = None;
        repopulate(&mut st);
        set_enabled(&mut st, true);
        pick_resolution(&mut st, 0);
        pick_refresh(&mut st, 0);
        pick_transform(&mut st, 3);
        set_scale(&mut st, 2.0);
        assert!(!st.dirty, "nothing selected means nothing edited");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::controls`
Expected: FAIL — `cannot find function 'set_enabled' in this scope` (and the same for the other five).

- [ ] **Step 3: Write the implementation**

Append to `settings/src/pages/displays/controls.rs`, after `view`:

```rust
/// Select head `index`, if it exists. Selecting is not an edit: `dirty` is
/// untouched.
pub fn select_head(st: &mut DisplaysState, index: usize) {
    if index >= st.heads.len() || index >= st.edits.len() {
        return;
    }
    st.selected = Some(index);
    repopulate(st);
}

/// Toggle the selected head's `enabled` flag.
///
/// Enabling a head whose edit names no mode would leave it unplaceable on the
/// canvas and applied at a mode the user never saw, so it adopts the head's
/// default mode (state.rs finding #9).
pub fn set_enabled(st: &mut DisplaysState, on: bool) {
    let Some(idx) = selection(st) else { return };
    st.edits[idx].enabled = on;
    if on && st.edits[idx].mode.is_none() {
        let default_mode = st.heads.get(idx).and_then(state::default_mode_for);
        if default_mode.is_some() {
            st.edits[idx].mode = default_mode;
        }
    }
    st.dirty = true;
    repopulate(st);
}

/// Pick `index` from the resolution list.
///
/// The current refresh survives when the new resolution offers it; otherwise
/// the resolution's highest is taken.
pub fn pick_resolution(st: &mut DisplaysState, index: usize) {
    let Some(idx) = selection(st) else { return };
    let Some(&(w, h)) = st.res_options.get(index) else {
        return;
    };
    let modes = st.heads[idx].modes.clone();
    let refreshes = refreshes_for(&modes, w, h);
    let cur = st.edits[idx].mode.map(|m| m.refresh_mhz);
    let refresh = cur
        .filter(|r| refreshes.contains(r))
        .or_else(|| refreshes.first().copied())
        .unwrap_or(0);
    st.edits[idx].mode = Some(ModeRequest {
        width: w,
        height: h,
        refresh_mhz: refresh,
    });
    st.dirty = true;
    repopulate(st);
}

/// Pick `index` from the refresh list.
///
/// On a head whose edit names no mode the refresh list was built for the first
/// resolution, so a whole mode is synthesized from it — otherwise the pick
/// would be dropped for want of an existing mode (state.rs finding #14).
pub fn pick_refresh(st: &mut DisplaysState, index: usize) {
    let Some(idx) = selection(st) else { return };
    let Some(&r) = st.refresh_options.get(index) else {
        return;
    };
    if let Some(mode) = st.edits[idx].mode.as_mut() {
        mode.refresh_mhz = r;
        st.dirty = true;
    } else if let Some(&(w, h)) = st.res_options.first() {
        st.edits[idx].mode = Some(ModeRequest {
            width: w,
            height: h,
            refresh_mhz: r,
        });
        st.dirty = true;
    }
    repopulate(st);
}

/// Pick `index` from the transform list, writing the raw `wl_output.transform`
/// value it names. An index past the end falls back to `Normal`.
pub fn pick_transform(st: &mut DisplaysState, index: usize) {
    let Some(idx) = selection(st) else { return };
    st.edits[idx].transform = Some(TRANSFORM_VALUES.get(index).copied().unwrap_or(0));
    st.dirty = true;
}

/// Set the selected head's scale.
pub fn set_scale(st: &mut DisplaysState, value: f64) {
    let Some(idx) = selection(st) else { return };
    st.edits[idx].scale = Some(value);
    st.dirty = true;
}
```

Add `ModeRequest` to the `use crate::outputs::` line at the top of the file
(`use crate::outputs::ModeRequest;`).

- [ ] **Step 4: Wire the six arms in `settings/src/app.rs`**

Replace the stubs P1 left:

```rust
        Msg::HeadSelected(i) => {
            crate::pages::displays::controls::select_head(&mut m.displays, i);
            Cmd::None
        }
        Msg::HeadEnabledToggled(on) => {
            crate::pages::displays::controls::set_enabled(&mut m.displays, on);
            Cmd::None
        }
        Msg::ResolutionSelected(i) => {
            crate::pages::displays::controls::pick_resolution(&mut m.displays, i);
            Cmd::None
        }
        Msg::RefreshSelected(i) => {
            crate::pages::displays::controls::pick_refresh(&mut m.displays, i);
            Cmd::None
        }
        Msg::TransformSelected(i) => {
            crate::pages::displays::controls::pick_transform(&mut m.displays, i);
            Cmd::None
        }
        Msg::HeadScaleChanged(v) => {
            crate::pages::displays::controls::set_scale(&mut m.displays, v);
            Cmd::None
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays::controls`
Expected: PASS — 14 tests.

- [ ] **Step 6: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings --lib
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git add settings/src/pages/displays/controls.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): the Displays control mutators

Six `&mut DisplaysState` functions replacing the GTK `connect_*` handlers,
findings #9 and #14 intact: enabling a mode-less head adopts its default
mode, and a refresh pick on one synthesizes a whole mode instead of being
dropped. Every mutator is a no-op with no selection.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 6: The outputs-message fold

**Files:**
- Modify: `settings/src/pages/displays/mod.rs`
- Modify: `settings/src/app.rs` (the `Msg::Outputs` arm)
- Test: `settings/src/pages/displays/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::outputs::{Head, OutputsMsg}` with variants `HeadsChanged(Vec<Head>)`, `ManagerUnavailable`, `Disconnected`, `ApplySucceeded { is_test: bool }`, `ApplyFailed { is_test: bool }`, `ApplyCancelled`; from P1 — `state::{baseline_edit, reconcile, Reconciled}`; Task 3's `canvas::{CANVAS_W, CANVAS_H}`; Task 4's `controls::repopulate`; `crate::app::SettingsModel` fields `displays`, `displays_status`, `displays_in_flight`, `outputs_available`.
- Produces:
  ```rust
  pub fn on_outputs(m: &mut crate::app::SettingsModel, msg: &crate::outputs::OutputsMsg);
  pub fn reset_edits(st: &mut crate::pages::displays::state::DisplaysState);
  pub fn republish_view(st: &mut crate::pages::displays::state::DisplaysState);
  pub const STATUS_DROPPED: &str;
  pub const STATUS_TEST_OK: &str;
  pub const STATUS_TEST_REJECTED: &str;
  pub const STATUS_APPLIED: &str;
  pub const STATUS_REJECTED: &str;
  pub const STATUS_SUPERSEDED: &str;
  pub const STATUS_TESTING: &str;
  pub const STATUS_APPLYING: &str;
  pub const UNAVAILABLE_TEXT: &str;
  ```

- [ ] **Step 1: Write the failing tests**

Append a `#[cfg(test)] mod tests` to `settings/src/pages/displays/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, Mode, OutputsMsg};
    use crate::pages::displays::state::baseline_edit;

    fn head(name: &str, enabled: bool) -> Head {
        let mode = Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
            preferred: true,
        };
        Head {
            name: name.to_string(),
            description: format!("{name} display"),
            enabled,
            modes: vec![mode],
            current_mode: Some(mode),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    /// A model with one enabled head and a pending, unapplied edit.
    fn dirty_model() -> crate::app::SettingsModel {
        let mut m = crate::app::SettingsModel::for_test();
        let h = head("DP-1", true);
        m.displays.edits = vec![baseline_edit(&h)];
        m.displays.heads = vec![h];
        m.displays.selected = Some(0);
        m.displays.edits[0].position = Some((640, 480));
        m.displays.dirty = true;
        m.outputs_available = true;
        m
    }

    /// Contract §2.8's named unit test.
    ///
    /// Mutation check: have the `ApplySucceeded` arm read
    /// `m.displays_in_flight` to decide whether it was a test, instead of the
    /// reply's own `is_test`; the second assertion fails. Restore.
    #[test]
    fn an_apply_reply_clears_in_flight_by_its_own_is_test_tag() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplySucceeded { is_test: true });
        assert!(!m.displays_in_flight, "a terminal reply clears the latch");
        assert_eq!(m.displays_status, STATUS_TEST_OK);
        assert!(m.displays.dirty, "a preview keeps the edits live to commit");

        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplySucceeded { is_test: false });
        assert!(!m.displays_in_flight);
        assert_eq!(m.displays_status, STATUS_APPLIED);
        assert!(!m.displays.dirty, "a real apply clears the dirty flag");
    }

    /// Mutation check: make the `ApplyFailed { is_test: true }` arm reset the
    /// edits like the `false` arm; the retained-position assertion fails.
    /// Restore.
    #[test]
    fn a_rejected_test_keeps_the_edits_and_a_rejected_apply_resets_them() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyFailed { is_test: true });
        assert_eq!(m.displays.edits[0].position, Some((640, 480)));
        assert_eq!(m.displays_status, STATUS_TEST_REJECTED);

        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyFailed { is_test: false });
        assert_eq!(
            m.displays.edits[0].position,
            Some((0, 0)),
            "a rejected apply re-baselines from the live head"
        );
        assert!(!m.displays.dirty);
        assert_eq!(m.displays_status, STATUS_REJECTED);
    }

    /// A `HeadsChanged` is not a terminal reply: it must not re-enable the
    /// buttons under an outstanding request (state.rs finding #15).
    ///
    /// Mutation check: clear `displays_in_flight` in the `HeadsChanged` arm;
    /// this fails. Restore.
    #[test]
    fn a_heads_changed_does_not_clear_an_outstanding_request() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::HeadsChanged(vec![head("DP-1", true)]));
        assert!(m.displays_in_flight, "only a terminal reply clears the latch");
        assert!(m.outputs_available);
    }

    /// A connector-set change discards pending edits and says so.
    ///
    /// Mutation check: pass `false` for `dirty` into `reconcile`; `dropped`
    /// comes back `false` and the status assertion fails. Restore.
    #[test]
    fn a_hotplug_that_discards_pending_edits_says_so() {
        let mut m = dirty_model();
        on_outputs(
            &mut m,
            &OutputsMsg::HeadsChanged(vec![head("DP-1", true), head("HDMI-A-1", true)]),
        );
        assert_eq!(m.displays_status, STATUS_DROPPED);
        assert_eq!(m.displays.heads.len(), 2);
        assert!(!m.displays.dirty, "a set change resets to the fresh baseline");
        assert!(m.displays.drag.is_none(), "a stale drag index is dropped");
    }

    /// Mutation check: leave `outputs_available` alone in the
    /// `Disconnected` arm; this fails. Restore.
    #[test]
    fn losing_the_manager_marks_output_management_unavailable() {
        for msg in [OutputsMsg::ManagerUnavailable, OutputsMsg::Disconnected] {
            let mut m = dirty_model();
            m.displays_in_flight = true;
            on_outputs(&mut m, &msg);
            assert!(!m.outputs_available, "{msg:?} must mark the page unavailable");
            assert!(!m.displays_in_flight, "and clear any outstanding request");
            assert_eq!(m.displays_status, "");
        }
    }

    /// Mutation check: drop the `ApplyCancelled` arm's `end_request`; the
    /// latch survives and this fails. Restore.
    #[test]
    fn a_cancelled_configuration_clears_the_latch_and_says_superseded() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyCancelled);
        assert!(!m.displays_in_flight);
        assert_eq!(m.displays_status, STATUS_SUPERSEDED);
    }

    /// A `HeadsChanged` republishes the view so a drag started before the
    /// next paint still maps correctly.
    ///
    /// Mutation check: drop the `republish_view` call; the view keeps its
    /// `DisplaysState::new` identity scale and this fails. Restore.
    #[test]
    fn a_heads_changed_republishes_the_canvas_view() {
        let mut m = crate::app::SettingsModel::for_test();
        m.outputs_available = true;
        on_outputs(&mut m, &OutputsMsg::HeadsChanged(vec![head("DP-1", true)]));
        assert!(
            m.displays.view.scale < 1.0,
            "a 1920x1080 head in a 360x240 canvas scales down, got {}",
            m.displays.view.scale
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::tests`
Expected: FAIL — `cannot find function 'on_outputs' in this scope`, `cannot find value 'STATUS_TEST_OK'`.

- [ ] **Step 3: Write the implementation**

Add to `settings/src/pages/displays/mod.rs`, above the test module:

```rust
use crate::app::SettingsModel;
use crate::outputs::OutputsMsg;
use crate::pages::displays::state::{self, DisplaysState};

/// Shown instead of the page when there is no compositor to talk to.
pub const UNAVAILABLE_TEXT: &str =
    "output management unavailable \u{2014} is the icedtea compositor running?";
/// A hotplug discarded unapplied edits.
pub const STATUS_DROPPED: &str = "Displays changed \u{2014} pending edits discarded";
/// A preview was accepted.
pub const STATUS_TEST_OK: &str = "Test succeeded";
/// A preview was rejected.
pub const STATUS_TEST_REJECTED: &str = "Test rejected by the compositor";
/// A real apply landed.
pub const STATUS_APPLIED: &str = "Applied";
/// A real apply was rejected.
pub const STATUS_REJECTED: &str = "Configuration rejected by the compositor";
/// The compositor superseded the request.
pub const STATUS_SUPERSEDED: &str = "Configuration superseded \u{2014} re-reading";
/// A preview is in flight.
pub const STATUS_TESTING: &str = "Testing\u{2026}";
/// A real apply is in flight.
pub const STATUS_APPLYING: &str = "Applying\u{2026}";

/// Reset the pending edits back to the last-known head snapshot.
pub fn reset_edits(st: &mut DisplaysState) {
    st.edits = st.heads.iter().map(state::baseline_edit).collect();
    st.dirty = false;
    if st.selected.is_none_or(|i| i >= st.heads.len()) {
        st.selected = if st.heads.is_empty() { None } else { Some(0) };
    }
}

/// Recompute and publish the canvas view for the current edits.
///
/// The GTK page did this from inside the draw callback; an Elm paint callback
/// may not touch the model (plan P4-D3), so `update` republishes it whenever
/// the head set changes, using the same fixed canvas size the paint uses.
pub fn republish_view(st: &mut DisplaysState) {
    let (_idxs, rects, _enabled) = state::all_rects(st);
    st.view = crate::pages::displays_canvas::compute_view(
        &rects,
        canvas::CANVAS_W,
        canvas::CANVAS_H,
        state::CANVAS_MARGIN,
    );
}

/// Clear an outstanding Test/Apply latch.
fn end_request(m: &mut SettingsModel) {
    m.displays_in_flight = false;
}

/// Fold one `zwlr_output_management_v1` message into the model.
///
/// The arm-for-arm equivalent of the glib source's `match` in the GTK page.
/// Which kind of request a reply answers rides on the reply itself as
/// `is_test`, never on `displays_in_flight` — an overlapped Test/Apply pair
/// can never have one reply read with the other's meaning.
pub fn on_outputs(m: &mut SettingsModel, msg: &OutputsMsg) {
    match msg {
        OutputsMsg::HeadsChanged(heads) => {
            m.outputs_available = true;
            let st = &mut m.displays;
            let r = state::reconcile(&st.heads, &st.edits, st.selected, st.dirty, heads);
            st.heads = heads.clone();
            st.edits = r.edits;
            st.selected = r.selected;
            // Any in-flight drag was indexed against the *old* head list, which
            // the new one may reorder or shorten (state.rs finding #2).
            st.drag = None;
            if !r.compatible {
                st.dirty = false;
            }
            controls::repopulate(st);
            republish_view(st);
            // A `HeadsChanged` is NOT a terminal reply for an outstanding
            // Test/Apply: the compositor still owes a
            // Succeeded/Failed/Cancelled, and clearing the latch here would let
            // a second request overlap the first (state.rs finding #15).
            m.displays_status = if r.dropped {
                STATUS_DROPPED.to_string()
            } else {
                String::new()
            };
        }
        OutputsMsg::ApplySucceeded { is_test } => {
            end_request(m);
            if *is_test {
                // A preview succeeded: keep the edits, and Apply, live.
                m.displays_status = STATUS_TEST_OK.to_string();
            } else {
                m.displays.dirty = false;
                m.displays_status = STATUS_APPLIED.to_string();
            }
        }
        OutputsMsg::ApplyFailed { is_test } => {
            end_request(m);
            if *is_test {
                // A preview was rejected: leave the edits intact to adjust.
                m.displays_status = STATUS_TEST_REJECTED.to_string();
            } else {
                reset_edits(&mut m.displays);
                controls::repopulate(&mut m.displays);
                republish_view(&mut m.displays);
                m.displays_status = STATUS_REJECTED.to_string();
            }
        }
        OutputsMsg::ApplyCancelled => {
            end_request(m);
            m.displays_status = STATUS_SUPERSEDED.to_string();
        }
        OutputsMsg::ManagerUnavailable | OutputsMsg::Disconnected => {
            m.displays_in_flight = false;
            m.outputs_available = false;
            m.displays_status = String::new();
        }
    }
}
```

- [ ] **Step 4: Wire the arm in `settings/src/app.rs`**

```rust
        Msg::Outputs(msg) => {
            crate::pages::displays::on_outputs(m, msg.as_ref());
            Cmd::None
        }
```

`Msg::Outputs` carries `std::sync::Arc<OutputsMsg>` (contract §2.2), so
`as_ref()` hands `on_outputs` a `&OutputsMsg` with no clone.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::displays`
Expected: PASS — 7 new tests plus everything from Tasks 1–5.

- [ ] **Step 6: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings --lib
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git add settings/src/pages/displays/mod.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): fold zwlr_output_management_v1 messages into the model

`reconcile` is called verbatim; findings #2 and #15 survive the port — a
stale drag index is dropped on every HeadsChanged, and only a terminal reply
clears the in-flight latch. Which request a reply answers still rides on its
own `is_test` tag.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 7: The Displays footer — Test, Revert, Apply

**Files:**
- Modify: `settings/src/pages/displays/mod.rs`
- Modify: `settings/src/outputs/pump.rs` (P4-D8: `Result`-returning submits)
- Modify: `settings/src/app.rs` (`SettingsModel.outputs`, the three footer arms)
- Test: `settings/src/pages/displays/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 6's status constants and `reset_edits`/`republish_view`; Task 4's `controls::repopulate`; `crate::outputs::{HeadEdit, OutputsError}`; `crate::outputs::pump::OutputsPump`; `icedtea_ui::view::builders::{button, box_, label}`; `icedtea_ui::widgets::types::{Align, Orientation}`.
- Produces:
  ```rust
  // settings/src/outputs/pump.rs — amended (plan P4-D8)
  impl OutputsPump {
      pub fn test_configuration(&self, edits: &[HeadEdit]) -> Result<(), crate::outputs::OutputsError>;
      pub fn build_and_send_configuration(&self, edits: &[HeadEdit]) -> Result<(), crate::outputs::OutputsError>;
  }

  // settings/src/pages/displays/mod.rs
  pub fn footer(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  pub fn submit(m: &mut crate::app::SettingsModel, is_test: bool);
  pub fn revert(m: &mut crate::app::SettingsModel);

  // settings/src/app.rs
  pub struct SettingsModel { /* … */ pub outputs: Option<crate::outputs::pump::OutputsPump> }  // P1-D2, consumed as-is
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/displays/mod.rs`'s `mod tests`:

```rust
    /// With no pump at all (no compositor, no second connection) a submit is a
    /// status change and nothing else — never a panic, never a stuck latch.
    ///
    /// Mutation check: set `displays_in_flight = true` before the `None` check
    /// in `submit`; this fails. Restore.
    #[test]
    fn a_submit_without_a_pump_never_latches() {
        let mut m = dirty_model();
        assert!(m.outputs.is_none(), "the test model has no pump");
        submit(&mut m, true);
        assert!(!m.displays_in_flight);
        submit(&mut m, false);
        assert!(!m.displays_in_flight);
    }

    /// A second submit while one is outstanding is refused.
    ///
    /// Mutation check: drop the `displays_in_flight` guard from `submit`; the
    /// status is overwritten and this fails. Restore.
    #[test]
    fn a_submit_while_one_is_outstanding_is_refused() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        m.displays_status = STATUS_TESTING.to_string();
        submit(&mut m, false);
        assert_eq!(
            m.displays_status, STATUS_TESTING,
            "the outstanding request's status survives"
        );
    }

    /// Revert re-baselines the edits, clears dirty, and empties the status —
    /// the GTK `revert_btn` handler exactly.
    ///
    /// Mutation check: drop the `republish_view` call from `revert`; the view
    /// keeps the dragged layout's scale and this fails. Restore.
    #[test]
    fn revert_rebaselines_the_edits_and_republishes_the_view() {
        let mut m = dirty_model();
        m.displays.edits[0].position = Some((4000, 4000));
        republish_view(&mut m.displays);
        let dragged_scale = m.displays.view.scale;

        revert(&mut m);
        assert_eq!(m.displays.edits[0].position, Some((0, 0)));
        assert!(!m.displays.dirty);
        assert_eq!(m.displays_status, "");
        assert!(
            m.displays.view.scale > dragged_scale,
            "a smaller layout fits at a larger scale: {} vs {dragged_scale}",
            m.displays.view.scale
        );
    }

    /// Mutation check: drop `.sensitive(...)` from the Apply button; the
    /// footer's id set is unchanged but the interaction gate in Task 11 fails.
    /// Assert the ids here so a rename is caught early.
    #[test]
    fn the_footer_ids_are_the_contract_ids() {
        let m = dirty_model();
        let ids = icedtea_ui::view::ids_of(&footer(&m));
        for id in [
            "displays_status",
            "displays_test",
            "displays_revert",
            "displays_apply",
        ] {
            assert!(ids.iter().any(|got| got == id), "missing id {id}; got {ids:?}");
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::displays::tests`
Expected: FAIL — `cannot find function 'submit' in this scope`, `no field 'outputs' on type 'SettingsModel'`.

- [ ] **Step 3: Amend `OutputsPump` (P4-D8)**

In `settings/src/outputs/pump.rs`, change the two submit methods so the caller
learns about a request that never reached the wire:

```rust
    /// Ship `edits` as a **preview** configuration.
    ///
    /// Non-blocking: it builds the configuration objects and flushes. The
    /// answer arrives later as `Msg::Outputs(ApplySucceeded/ApplyFailed)`.
    ///
    /// Returns the error when the request could not even be attempted — no
    /// manager global, an edit naming a head that is not present, or a flush
    /// failure. `update` needs that synchronously to keep the in-flight latch
    /// honest, which `Cmd::Task` (no return value) cannot deliver (plan P4-D8).
    ///
    /// # Errors
    /// [`crate::outputs::OutputsError`] from `OutputsConnection`.
    pub fn test_configuration(
        &self,
        edits: &[crate::outputs::HeadEdit],
    ) -> Result<(), crate::outputs::OutputsError> {
        let result = self.conn.borrow_mut().test_configuration(edits);
        if result.is_ok() {
            self.flush();
        }
        result
    }

    /// Ship `edits` as a **real** configuration. See
    /// [`OutputsPump::test_configuration`] for the return contract.
    ///
    /// # Errors
    /// [`crate::outputs::OutputsError`] from `OutputsConnection`.
    pub fn build_and_send_configuration(
        &self,
        edits: &[crate::outputs::HeadEdit],
    ) -> Result<(), crate::outputs::OutputsError> {
        let result = self.conn.borrow_mut().build_and_send_configuration(edits);
        if result.is_ok() {
            self.flush();
        }
        result
    }
```

- [ ] **Step 4: Confirm `SettingsModel.outputs` (P1's field — do not redeclare it)**

In `settings/src/app.rs`, confirm `SettingsModel` carries the pump handle
§2.5's `main.rs` snippet passes. P1 ships it under P1-D2 as

```rust
    /// The second wayland connection's pump, when one could be attached.
    /// `None` on a machine with no compositor or no `zwlr_output_manager_v1`,
    /// which is the page's "output management unavailable" state.
    pub outputs: Option<crate::outputs::pump::OutputsPump>,
```

set by `SettingsModel::new(db_path, workers).with_outputs(pump.clone())` — two
constructor arguments plus a builder, not a third argument, and no `Rc`
(`OutputsPump` is `Clone`). P4 uses it as-is: `m.outputs.clone()` in `submit`
works unchanged. If the field is missing or its type differs, **stop and
report** rather than adding a second shape (consistency-check ruling E2).

- [ ] **Step 5: Write the footer and the two actions**

Append to `settings/src/pages/displays/mod.rs`:

```rust
/// Submit the current edit set as a preview (`is_test`) or a real apply.
///
/// Runs on the loop thread, inside `update`: this is a wayland request build
/// plus a flush, not a blocking D-Bus round trip, and its synchronous error is
/// the only thing that can keep the in-flight latch honest (plan P4-D8).
pub fn submit(m: &mut SettingsModel, is_test: bool) {
    if m.displays_in_flight {
        return;
    }
    let Some(pump) = m.outputs.clone() else {
        // No output management at all: the page is already showing its
        // unavailable state and there is nothing to latch.
        return;
    };
    let edits = m.displays.edits.clone();
    let result = if is_test {
        pump.test_configuration(&edits)
    } else {
        pump.build_and_send_configuration(&edits)
    };
    match result {
        Ok(()) => {
            m.displays_in_flight = true;
            m.displays_status = if is_test {
                STATUS_TESTING.to_string()
            } else {
                STATUS_APPLYING.to_string()
            };
        }
        Err(err) => {
            m.displays_status = if is_test {
                format!("Test failed: {err}")
            } else {
                format!("Apply failed: {err}")
            };
        }
    }
}

/// Drop the pending edits back to the last-known head snapshot.
pub fn revert(m: &mut SettingsModel) {
    reset_edits(&mut m.displays);
    controls::repopulate(&mut m.displays);
    republish_view(&mut m.displays);
    m.displays_status = String::new();
}

/// The page's own footer: status, Test, Revert, Apply.
///
/// The shared model footer never touches Displays — this page does not go
/// through the redb working copy at all, the compositor persists applied
/// layouts on its side.
///
/// Apply is held insensitive while a request is outstanding so a
/// `HeadsChanged`-driven repaint cannot re-enable it under one (state.rs
/// finding #15); Test is held for the same reason.
#[must_use]
pub fn footer(m: &SettingsModel) -> View<Msg> {
    let dirty = m.displays.dirty;
    let busy = m.displays_in_flight;
    let live = m.outputs_available;
    box_(
        Orientation::Horizontal,
        [
            label(&m.displays_status)
                .id("displays_status")
                .hexpand(true)
                .halign(Align::Start),
            button("Test")
                .id("displays_test")
                .sensitive(live && !busy)
                .on_click(Msg::DisplaysTest),
            button("Revert")
                .id("displays_revert")
                .sensitive(live && dirty)
                .on_click(Msg::DisplaysRevert),
            button("Apply")
                .id("displays_apply")
                .sensitive(live && dirty && !busy)
                .on_click(Msg::DisplaysApply),
        ],
    )
    .spacing(8u32)
    .id("displays_footer")
}
```

Add to the file's `use` block:

```rust
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{box_, button, label};
use icedtea_ui::widgets::types::{Align, Orientation};

use crate::app::Msg;
```

- [ ] **Step 6: Wire the three arms in `settings/src/app.rs`**

```rust
        Msg::DisplaysTest => {
            crate::pages::displays::submit(m, true);
            Cmd::None
        }
        Msg::DisplaysApply => {
            crate::pages::displays::submit(m, false);
            Cmd::None
        }
        Msg::DisplaysRevert => {
            crate::pages::displays::revert(m);
            Cmd::None
        }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib`
Expected: PASS — the 4 new tests plus everything before them; `settings/tests/outputs_client.rs` unaffected.

- [ ] **Step 8: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 9: Commit**

```bash
git add settings/src/pages/displays/mod.rs settings/src/outputs/pump.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): the Displays Test/Revert/Apply footer

The submit runs inside `update` and the pump's two submit methods return
`Result` (P4-D8): `Cmd::Task` has no return channel, so a request that never
reached the wire could neither report itself nor clear the in-flight latch.
Test and Apply stay insensitive while a request is outstanding.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 8: The page assembly, the initial-page env var, and the `state` probe lines

**Files:**
- Modify: `settings/src/pages/displays/mod.rs` (`view`)
- Modify: `settings/src/main.rs` (`ICEDTEA_SETTINGS_PAGE`)
- Modify: `settings/src/app.rs` (the `state` report lines)
- Test: `settings/src/pages/displays/mod.rs`, `settings/src/app.rs`

**Interfaces:**
- Consumes: Task 2's `canvas::view`, Task 4's `controls::view`, Task 7's `footer`, Task 6's `UNAVAILABLE_TEXT`; `crate::pages::PageId` with `PageId::ALL`, `PageId::name(self) -> &'static str`; M5-D9's `$ICEDTEA_PROBE_REPORT` writer P1 added to `app.rs`.
- Produces:
  ```rust
  pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  // settings/src/pages/mod.rs — no change; PageId is P1's
  pub fn crate::pages::page_from_env() -> Option<crate::pages::PageId>;   // in pages/mod.rs
  pub fn crate::app::state_report_lines(m: &crate::app::SettingsModel) -> Vec<String>;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/displays/mod.rs`'s `mod tests`:

```rust
    /// Mutation check: return the content branch unconditionally from `view`;
    /// the unavailable id disappears and this fails. Restore.
    #[test]
    fn an_unavailable_page_shows_only_its_explanation() {
        let mut m = crate::app::SettingsModel::for_test();
        m.outputs_available = false;
        let ids = icedtea_ui::view::ids_of(&view(&m));
        assert!(ids.iter().any(|id| id == "displays_unavailable"));
        assert!(
            !ids.iter().any(|id| id == "displays_canvas"),
            "no canvas while output management is unavailable; got {ids:?}"
        );
    }

    /// Mutation check: drop `controls::view(m)` from the content branch; the
    /// `displays_enabled` id disappears and this fails. Restore.
    #[test]
    fn an_available_page_shows_canvas_controls_and_footer() {
        let mut m = crate::app::SettingsModel::for_test();
        m.outputs_available = true;
        let ids = icedtea_ui::view::ids_of(&view(&m));
        for id in [
            "displays_canvas",
            "displays_enabled",
            "displays_resolution",
            "displays_refresh",
            "displays_scale",
            "displays_transform",
            "displays_position",
            "displays_status",
            "displays_test",
            "displays_revert",
            "displays_apply",
        ] {
            assert!(ids.iter().any(|got| got == id), "missing id {id}; got {ids:?}");
        }
    }
```

Append to `settings/src/pages/mod.rs`'s test module (creating one if P1 left
none):

```rust
    /// Mutation check: have `page_from_env` return `Some(PageId::Appearance)`
    /// for an unknown name; the `None` assertion fails. Restore.
    #[test]
    fn the_initial_page_env_var_only_accepts_real_page_names() {
        assert_eq!(page_named("displays"), Some(PageId::Displays));
        assert_eq!(page_named("keybindings"), Some(PageId::Keybindings));
        assert_eq!(page_named("Displays"), None, "the match is exact");
        assert_eq!(page_named("nonsense"), None);
        assert_eq!(page_named(""), None);
    }
```

Append to `settings/src/app.rs`'s test module:

```rust
    /// Mutation check: drop the `displays.position` line from
    /// `state_report_lines`; the drag interaction gate (Task 11) has nothing
    /// to read and this fails. Restore.
    #[test]
    fn the_state_report_lines_carry_the_displays_model() {
        let mut m = SettingsModel::for_test();
        m.displays.heads = Vec::new();
        m.displays.edits = Vec::new();
        m.displays.selected = None;
        let none = state_report_lines(&m);
        assert!(none.iter().any(|l| l == "state displays.selected none"));
        assert!(none.iter().any(|l| l == "state displays.dirty false"));
        assert!(none.iter().any(|l| l == "state displays.in_flight false"));
        assert!(
            none.iter().any(|l| l == "state displays.position none"),
            "no selection reports `none`, not a coordinate; got {none:?}"
        );

        let head = crate::outputs::Head {
            name: "DP-1".to_string(),
            description: "DP-1".to_string(),
            enabled: true,
            modes: Vec::new(),
            current_mode: None,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        };
        m.displays.edits = vec![crate::pages::displays::state::baseline_edit(&head)];
        m.displays.heads = vec![head];
        m.displays.selected = Some(0);
        m.displays.edits[0].position = Some((1920, -180));
        m.displays.dirty = true;
        let lines = state_report_lines(&m);
        assert!(lines.iter().any(|l| l == "state displays.position 1920 -180"));
        assert!(lines.iter().any(|l| l == "state displays.selected 0"));
        assert!(lines.iter().any(|l| l == "state displays.dirty true"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib`
Expected: FAIL — `cannot find function 'page_named'`, `cannot find function 'state_report_lines'`, and the `view` id assertions fail against P1's stub body.

- [ ] **Step 3: Write the page assembly**

Append to `settings/src/pages/displays/mod.rs`:

```rust
/// The whole Displays page.
///
/// Either the "output management unavailable" explanation, or the canvas, the
/// per-head panel and the page's own footer — the GTK page's `unavailable`
/// label / `content` box pair, now a branch instead of two visibility flags.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    if !m.outputs_available {
        return box_(
            Orientation::Vertical,
            [label(UNAVAILABLE_TEXT)
                .id("displays_unavailable")
                .halign(Align::Center)
                .valign(Align::Center)
                .vexpand(true)],
        )
        .id("displays");
    }
    box_(
        Orientation::Vertical,
        [
            box_(
                Orientation::Horizontal,
                [canvas::view(m), controls::view(m)],
            )
            .spacing(12u32)
            .vexpand(true)
            .id("displays_content"),
            footer(m),
        ],
    )
    .spacing(8u32)
    .margin(12, 12, 12, 12)
    .id("displays")
}
```

- [ ] **Step 4: Write `page_named` and the env var**

In `settings/src/pages/mod.rs`, beside `PageId`:

```rust
/// The `PageId` whose [`PageId::name`] is `name`, or `None`.
#[must_use]
pub fn page_named(name: &str) -> Option<PageId> {
    PageId::ALL.into_iter().find(|p| p.name() == name)
}

/// The page `$ICEDTEA_SETTINGS_PAGE` names, if it names a real one.
///
/// Test-only ergonomics with a production-safe shape (plan P4-D9): a gate can
/// open the Displays page directly instead of synthesising switcher clicks. An
/// unset, empty or unrecognised value is simply ignored — an environment
/// variable is untrusted input and never panics.
#[must_use]
pub fn page_from_env() -> Option<PageId> {
    page_named(&std::env::var("ICEDTEA_SETTINGS_PAGE").ok()?)
}
```

In `settings/src/main.rs`, where the model is built, replace the initial page
with:

```rust
    let page = crate::pages::page_from_env().unwrap_or(crate::pages::PageId::Appearance);
```

and pass it into `SettingsModel::new`'s `page` field (or assign
`model.page = page;` immediately after construction, whichever shape P1 left).

- [ ] **Step 5: Write the `state` report lines**

In `settings/src/app.rs`, beside the M5-D9 report writer P1 added:

```rust
/// The `state <key> <value…>` lines the probe report carries (plan P4-D10).
///
/// M5-D9's `probe`/`alloc` lines carry geometry only, and the Displays drag
/// gate has to assert the *model's* rectangle. These four lines are what it
/// reads; they are written by the same once-per-changed-frame writer as the
/// other two kinds.
#[must_use]
pub fn state_report_lines(m: &SettingsModel) -> Vec<String> {
    let position = m
        .displays
        .selected
        .and_then(|i| m.displays.edits.get(i))
        .and_then(|e| e.position)
        .map_or_else(
            || "none".to_string(),
            |(x, y)| format!("{x} {y}"),
        );
    let selected = m
        .displays
        .selected
        .map_or_else(|| "none".to_string(), |i| i.to_string());
    vec![
        format!("state displays.position {position}"),
        format!("state displays.selected {selected}"),
        format!("state displays.dirty {}", m.displays.dirty),
        format!("state displays.in_flight {}", m.displays_in_flight),
    ]
}
```

Call it from the report writer, after the `probe`/`alloc` lines of the same
frame, so a reader that has seen a `state` line has also seen that frame's
geometry.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib`
Expected: PASS.

- [ ] **Step 7: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 8: Commit**

```bash
git add settings/src/pages/displays/mod.rs settings/src/pages/mod.rs settings/src/main.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(settings): assemble the Displays page and expose it to the gates

The unavailable/content pair becomes one branch instead of two visibility
flags. `ICEDTEA_SETTINGS_PAGE` selects the initial page (P4-D9) and the probe
report gains `state <key> <value>` lines (P4-D10) so an interaction gate can
assert the model's rectangle rather than a pixel.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 9: `DropDown`'s embedded list is sized to its content, capped

**Files:**
- Modify: `ui/src/widgets/drop_down.rs`
- Test: `ui/src/widgets/drop_down.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DropDownC { items: Rc<[ListItem]>, popover: PopoverC, button: Node, … }`; `PopoverC::open(&mut self, anchor: PopupAnchorPoint, size: (u32, u32), content: Option<Rc<dyn Fn() -> View<Msg>>>, cx: &mut EventCx<'_, Msg>)`.
- Produces:
  ```rust
  pub const DROP_DOWN_ROW_PX: f32 = 34.0;
  pub const DROP_DOWN_MAX_PX: u32 = 240;
  #[must_use] pub fn drop_down_list_height(rows: usize) -> u32;
  ```

**This is the only file under `ui/` that P4 touches** (contract §7's first
discharged forward reference). Everything else in `ui/` is P0's.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/widgets/drop_down.rs`'s `mod tests`:

```rust
    /// M3's P7-D54 embedded the list at a flat 240px, which clips inside a
    /// 420px-tall settings window that also carries a switcher and two
    /// footers. The list is sized to its content and capped instead.
    ///
    /// Mutation check: return `DROP_DOWN_MAX_PX` unconditionally from
    /// `drop_down_list_height`; the two-row and empty cases fail. Restore.
    #[test]
    fn a_drop_down_list_is_sized_to_its_content_and_capped() {
        use super::{DROP_DOWN_MAX_PX, DROP_DOWN_ROW_PX, drop_down_list_height};

        // An empty model still asks for a positive height — a zero-size popup
        // is a protocol error, not an empty list.
        assert_eq!(drop_down_list_height(0), 1);
        // Two rows: 68px, not 240.
        assert_eq!(drop_down_list_height(2), (2.0 * DROP_DOWN_ROW_PX) as u32);
        assert!(drop_down_list_height(2) < DROP_DOWN_MAX_PX);
        // Seven rows is over the cap and scrolls.
        assert_eq!(drop_down_list_height(7), DROP_DOWN_MAX_PX);
        assert_eq!(drop_down_list_height(4000), DROP_DOWN_MAX_PX);
        // The exact boundary: the cap divided by the row height.
        let exact = (f32::from(u16::try_from(DROP_DOWN_MAX_PX).unwrap()) / DROP_DOWN_ROW_PX)
            .floor() as usize;
        assert!(drop_down_list_height(exact) <= DROP_DOWN_MAX_PX);
        assert_eq!(drop_down_list_height(exact + 1), DROP_DOWN_MAX_PX);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widgets::drop_down`
Expected: FAIL — `cannot find function 'drop_down_list_height' in module 'super'`.

- [ ] **Step 3: Write the implementation**

Above `DropDownC`'s `impl Controller` block in `ui/src/widgets/drop_down.rs`:

```rust
/// One embedded list row's height, in device-independent pixels.
///
/// Adwaita's `.menu` popover row: `min-height: 26px` plus the 4px vertical
/// padding either side, hand-resolved the same way `ScrollbarC`'s and
/// `ScaleC`'s intrinsic sizes are — the list rows are controller-owned
/// subnodes taffy never measures.
pub const DROP_DOWN_ROW_PX: f32 = 34.0;

/// The tallest an embedded list may be. Beyond this it scrolls.
///
/// This was M3's flat height for *every* list (P7-D54); it is a ceiling now.
/// A 240px list clips inside settings' 420px window, which also carries a
/// stack switcher and two footers (M5 contract §2.8).
pub const DROP_DOWN_MAX_PX: u32 = 240;

/// The height an embedded list of `rows` items asks for: its content, capped
/// at [`DROP_DOWN_MAX_PX`], never zero.
#[must_use]
pub fn drop_down_list_height(rows: usize) -> u32 {
    let rows = u32::try_from(rows).unwrap_or(u32::MAX);
    // `rows * DROP_DOWN_ROW_PX` in f32 saturates gracefully for a huge model;
    // the clamp is what makes the result meaningful either way.
    let px = (rows as f32 * DROP_DOWN_ROW_PX).ceil();
    let px = if px.is_finite() { px } else { f32::MAX };
    px.clamp(1.0, DROP_DOWN_MAX_PX as f32) as u32
}
```

Then replace the fixed `240` at the `popover.open` call site:

```rust
                self.popover.open(
                    PopupAnchorPoint::Node(self.button.clone()),
                    (
                        rect.width.max(1.0) as u32,
                        drop_down_list_height(self.filtered.len()),
                    ),
                    // The list lives under this widget's own `popover`
                    // node in the parent tree; a popup surface would
                    // duplicate it (P7-D54). `PopoverC::open` reveals that
                    // retained body instead.
                    None,
                    cx,
                );
```

`self.filtered` — not `self.items` — is what is actually shown: a search that
narrows the list narrows the popover with it.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p icedtea-ui --lib widgets::drop_down`
Expected: PASS, including the existing
`opening_a_drop_down_and_clicking_a_row_selects_that_item`.

- [ ] **Step 5: Prove the M3 gates are unchanged**

Run:
```bash
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test gallery_gate
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui --test reconcile_props
cargo test -p icedtea-ui --test window_events
cargo test -p icedtea-ui --test counter_app
```
Expected: all pass; `interaction_gate` is 16/16 and
`opening_a_drop_down_and_picking_an_item_updates_the_button` is green
**unchanged** — the gallery's drop-down sample has three items, whose new
height (102px) is still tall enough to contain every row it clicks.

- [ ] **Step 6: Run the toolkit gates in both feature configurations**

Run:
```bash
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git add ui/src/widgets/drop_down.rs
git commit -m "$(cat <<'MSG'
fix(ui): size a DropDown's embedded list to its content, capped at 240px

M3's P7-D54 opened every embedded list at a flat 240px, which clips inside
settings' 420px window. The height is now `rows * 34px` clamped to
[1, 240] — a two-item list is 68px and a long one scrolls, as it already
could. Contract M5 §2.8 authorises this as P4's single `ui/` edit.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 10: The settings test driver

**Files:**
- Create: `settings/tests/support/displays.rs`
- Modify: `settings/Cargo.toml` (dev-dependencies)
- Test: exercised by Tasks 11 and 12; this task's own gate is that it compiles and its two self-tests pass.

**Interfaces:**
- Consumes: `icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient}`; `icedtea_ui::shm::pixel_rgb(format, bytes, stride, x, y) -> Option<(u8, u8, u8)>`; `icedtea_ui::wayland::{BTN_LEFT}`; `env!("CARGO_BIN_EXE_icedtea-settings")`; Task 8's `ICEDTEA_SETTINGS_PAGE` and `state` lines; M5-D9's `$ICEDTEA_PROBE_REPORT`.
- Produces:
  ```rust
  pub const TEST_THEME: &str;                    // "bundled"
  pub const SCREENCOPY_TOLERANCE: u8;            // 12
  pub const REACT: std::time::Duration;          // 60s
  pub const BOOT: std::time::Duration;           // 60s

  pub struct ProbePoint { pub label: String, pub x: i32, pub y: i32 }
  pub struct Alloc { pub id: String, pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

  pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool;
  pub fn pixel_at(frame: &icedtea_harness::CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)>;
  pub fn paints_something(frame: &icedtea_harness::CapturedFrame, rect: (i32, i32, i32, i32), background: (u8, u8, u8)) -> bool;

  pub struct SettingsDriver { /* … */ }
  impl SettingsDriver {
      pub fn open(theme: &str, page: &str) -> SettingsDriver;
      pub fn background(&self) -> (u8, u8, u8);
      pub fn probe_points(&self) -> Vec<ProbePoint>;
      pub fn point(&self, label: &str) -> (i32, i32);
      pub fn alloc(&self, id: &str) -> Alloc;
      pub fn state(&self, key: &str) -> Option<String>;
      pub fn wait_state(&self, key: &str, value: &str, timeout: std::time::Duration) -> bool;
      pub fn wait_state_change(&self, key: &str, before: Option<String>, timeout: std::time::Duration) -> Option<String>;
      pub fn capture(&mut self) -> icedtea_harness::CapturedFrame;
      pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8);
      pub fn move_to(&mut self, x: i32, y: i32);
      pub fn press(&mut self, x: i32, y: i32);
      pub fn release(&mut self, x: i32, y: i32);
      pub fn click(&mut self, x: i32, y: i32);
      pub fn drag(&mut self, from: (i32, i32), to: (i32, i32));
  }
  ```

- [ ] **Step 1: Confirm the dev-dependencies**

`settings/Cargo.toml`'s `[dev-dependencies]` must read exactly:

```toml
[dev-dependencies]
icedtea-harness = { path = "../harness" }
tempfile = "3"
```

Both are already there per contract §2.1. No new dependency is added by P4.

- [ ] **Step 2: Write the support module**

Create `settings/tests/support/displays.rs`:

```rust
//! Harness scaffolding for the Displays gates.
//!
//! Self-contained on purpose (plan P4-D1): the contract's module map names no
//! shared settings test-support module, so this one carries everything P4's two
//! test binaries need and shares nothing with any other part's.
//!
//! The shape is `ui/tests/support/mod.rs`'s `Driver`, with `icedtea-settings`
//! in place of the gallery: a harness compositor, one settings process against
//! an isolated `XDG_CONFIG_HOME`, a screencopy client and a virtual pointer,
//! and coordinates read out of `$ICEDTEA_PROBE_REPORT` rather than hard-coded.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// The theme every gate pins its colours against. Never the developer's own
/// `gtk.css`.
pub const TEST_THEME: &str = "bundled";

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture — the output goes through a format conversion an
/// offscreen sample does not. `ui/tests/support/mod.rs`'s ceiling, verbatim.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// How long an interaction gets to reach the model and the screen.
///
/// The same generous bound `ui/tests/interaction_gate.rs` documents at length:
/// one anti-aliased `clip_path` in `skia-rs-canvas` 0.4.0 rasterises its mask
/// over the whole canvas, so a single repaint costs seconds on a debug build.
/// A complexity bound, not a wall-clock pin.
pub const REACT: Duration = Duration::from_secs(60);

/// How long the settings process gets to map its first frame.
pub const BOOT: Duration = Duration::from_secs(60);

/// How long a polling loop sleeps between samples.
const POLL: Duration = Duration::from_millis(25);

/// One `probe <label> <x> <y>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// One `alloc <id> <x> <y> <w> <h>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alloc {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Whether two channel values are the same within [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= u32::from(SCREENCOPY_TOLERANCE)
}

/// Whether an RGB triple matches `expected` on every channel.
#[must_use]
pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

/// The RGB at `(x, y)` in a captured frame, or `None` outside it.
#[must_use]
pub fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// Whether any pixel of `rect` differs from `background`.
///
/// A full scan, not a 5x5 inset grid: the whole page is a few tens of
/// rectangles and one screencopy round trip dwarfs the scan.
#[must_use]
pub fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h).any(|py| {
        (x..x + w).any(|px| {
            px >= 0
                && py >= 0
                && pixel_at(frame, px as u32, py as u32).is_some_and(|got| !matches(got, background))
        })
    })
}

/// Kill the child however the test ends.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A harness compositor plus one `icedtea-settings` process, its probe report
/// and its injectors.
pub struct SettingsDriver {
    _compositor: Compositor,
    _child: Reaper,
    /// Kept alive: dropping it deletes the isolated config store and report.
    _home: tempfile::TempDir,
    report: PathBuf,
    screencopy: ScreencopyClient,
    pointer: VirtualPointerClient,
    output: (u32, u32),
    background: (u8, u8, u8),
    background_probe: (u32, u32),
}

impl SettingsDriver {
    /// Boot a compositor and open `icedtea-settings` on `page` under `theme`.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output, screencopy or virtual pointer,
    /// if the binary cannot be spawned, or if it never paints within [`BOOT`].
    #[must_use]
    pub fn open(theme: &str, page: &str) -> SettingsDriver {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let (w, h) = compositor.output_size();
        let mut screencopy = ScreencopyClient::spawn(&socket);
        // A column the settings window never covers: the far right edge.
        let background_probe = (w as u32 - 3, h as u32 / 2);
        let empty = screencopy.capture();
        let background = pixel_at(&empty, background_probe.0, background_probe.1)
            .expect("the background probe is inside the frame");

        let home = tempfile::tempdir().expect("temp XDG_CONFIG_HOME");
        let report = home.path().join("probe.txt");
        let child = Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
            .env("WAYLAND_DISPLAY", &socket)
            .env("XDG_CONFIG_HOME", home.path())
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_SETTINGS_PAGE", page)
            .env("ICEDTEA_PROBE_REPORT", &report)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn icedtea-settings");

        let pointer = VirtualPointerClient::spawn(&socket);
        let mut driver = SettingsDriver {
            _compositor: compositor,
            _child: Reaper(child),
            _home: home,
            report,
            screencopy,
            pointer,
            output: (w as u32, h as u32),
            background,
            background_probe,
        };
        driver.wait_for_first_frame();
        driver
    }

    /// Block until the window covers the centre of the output.
    fn wait_for_first_frame(&mut self) {
        let centre = (self.output.0 / 2, self.output.1 / 2);
        let started = Instant::now();
        while started.elapsed() < BOOT {
            let frame = self.screencopy.capture();
            if pixel_at(&frame, centre.0, centre.1)
                .is_some_and(|px| !matches(px, self.background))
            {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!("icedtea-settings never painted within {BOOT:?}");
    }

    /// The output's empty background colour.
    #[must_use]
    pub fn background(&self) -> (u8, u8, u8) {
        self.background
    }

    /// Every line the settings process has reported so far.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Every `probe` line, most recent first — the report appends per frame, so
    /// a later line supersedes an earlier one with the same label.
    #[must_use]
    pub fn probe_points(&self) -> Vec<ProbePoint> {
        let mut out: Vec<ProbePoint> = Vec::new();
        for line in self.lines() {
            let mut f = line.split_whitespace();
            if f.next() != Some("probe") {
                continue;
            }
            let (Some(label), Some(x), Some(y)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x.parse(), y.parse()) else {
                continue;
            };
            let label = label.to_string();
            out.retain(|p| p.label != label);
            out.push(ProbePoint { label, x, y });
        }
        out
    }

    /// The most recent centre reported for `label`.
    ///
    /// # Panics
    ///
    /// If no probe line ever named `label`; the message lists what was there.
    #[must_use]
    pub fn point(&self, label: &str) -> (i32, i32) {
        let points = self.probe_points();
        points
            .iter()
            .find(|p| p.label == label)
            .map(|p| (p.x, p.y))
            .unwrap_or_else(|| {
                let labels: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
                panic!("no probe point {label:?}; got {labels:?}")
            })
    }

    /// The most recent allocation reported for `id`.
    ///
    /// # Panics
    ///
    /// If no `alloc` line ever named `id`.
    #[must_use]
    pub fn alloc(&self, id: &str) -> Alloc {
        let mut found = None;
        for line in self.lines() {
            let mut f = line.split_whitespace();
            if f.next() != Some("alloc") || f.next() != Some(id) {
                continue;
            }
            let nums: Vec<i32> = f.filter_map(|n| n.parse::<f32>().ok().map(|v| v as i32)).collect();
            if let [x, y, w, h] = nums[..] {
                found = Some(Alloc {
                    id: id.to_string(),
                    x,
                    y,
                    w,
                    h,
                });
            }
        }
        found.unwrap_or_else(|| panic!("no allocation line for {id:?}"))
    }

    /// The most recent `state <key> <value…>` value, if any.
    #[must_use]
    pub fn state(&self, key: &str) -> Option<String> {
        let mut found = None;
        for line in self.lines() {
            let Some(rest) = line.strip_prefix("state ") else {
                continue;
            };
            let Some(value) = rest.strip_prefix(key) else {
                continue;
            };
            let Some(value) = value.strip_prefix(' ') else {
                continue;
            };
            found = Some(value.to_string());
        }
        found
    }

    /// Poll until `key` reads `value`.
    #[must_use]
    pub fn wait_state(&self, key: &str, value: &str, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if self.state(key).as_deref() == Some(value) {
                return true;
            }
            std::thread::sleep(POLL);
        }
        false
    }

    /// Poll until `key` reads something other than `before`, and return it.
    #[must_use]
    pub fn wait_state_change(
        &self,
        key: &str,
        before: Option<String>,
        timeout: Duration,
    ) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            let now = self.state(key);
            if now != before && now.is_some() {
                return now;
            }
            std::thread::sleep(POLL);
        }
        None
    }

    /// One screencopy frame.
    pub fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }

    /// The RGB at `(x, y)` on the output.
    ///
    /// # Panics
    ///
    /// If the point is outside the captured frame.
    pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let frame = self.capture();
        pixel_at(&frame, x as u32, y as u32).expect("sample inside the frame")
    }

    /// Move the pointer and settle.
    ///
    /// The settle is `ui/tests/support/mod.rs`'s: a button sent back to back
    /// with the motion that first entered a surface can reach the seat before
    /// focus is assigned, and `wlr_seat_pointer_notify_button` drops a button
    /// with no focused surface silently.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(POLL);
            self.pointer.pump();
        }
    }

    /// Press the left button at `(x, y)`.
    pub fn press(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Release the left button at `(x, y)`.
    pub fn release(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release at `(x, y)`.
    pub fn click(&mut self, x: i32, y: i32) {
        self.press(x, y);
        self.release(x, y);
    }

    /// Press at `from`, move through the midpoint, release at `to`.
    ///
    /// The intermediate motion is what makes it a drag; a teleport looks like a
    /// click somewhere else.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.press(from.0, from.1);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release(to.0, to.1);
    }
}
```

- [ ] **Step 3: Write the driver's two self-tests**

Create `settings/tests/displays.rs` with just the module declaration and the
driver's own smoke tests, so this task has a runnable gate:

```rust
//! The Displays page's gates: rest-state paint, drag-to-snap, drop-down fit.

#[path = "support/displays.rs"]
mod support;

use std::time::Duration;

use support::{REACT, SettingsDriver, TEST_THEME};

/// The driver itself: the process boots on the page it was told to, and the
/// report carries both geometry and model state for it.
///
/// Mutation check: pass `"appearance"` as the page; the `displays_canvas`
/// probe point never appears and this fails. Restore.
#[test]
fn the_driver_opens_the_displays_page_and_reads_its_report() {
    let driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(
        driver
            .wait_state("displays.dirty", "false", REACT)
            .then_some(())
            .is_some(),
        "the report never carried a displays.dirty line"
    );
    let labels: Vec<String> = driver
        .probe_points()
        .into_iter()
        .map(|p| p.label)
        .collect();
    assert!(
        labels.iter().any(|l| l == "displays_canvas"),
        "no displays_canvas probe point; got {labels:?}"
    );
}

/// The harness advertises `zwlr_output_manager_v1` and at least one head
/// (`settings/tests/outputs_client.rs` proves both), so the page must come up
/// live rather than in its unavailable state.
///
/// Mutation check: force `outputs_available = false` in
/// `SettingsModel::new`; the canvas disappears and this fails. Restore.
#[test]
fn the_displays_page_comes_up_live_under_the_harness() {
    let driver = SettingsDriver::open(TEST_THEME, "displays");
    let labels: Vec<String> = driver
        .probe_points()
        .into_iter()
        .map(|p| p.label)
        .collect();
    assert!(
        !labels.iter().any(|l| l == "displays_unavailable"),
        "output management should be available under the harness; got {labels:?}"
    );
    assert!(labels.iter().any(|l| l == "displays_apply"));
    let _ = Duration::from_secs(0); // keep the import honest if REACT is unused
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p icedtea-settings --test displays`
Expected: PASS — 2 tests. A failure that says "never painted" means the
`ICEDTEA_UI_THEME`/`ICEDTEA_SETTINGS_PAGE` wiring from Task 8 is missing, not
that the driver is wrong.

- [ ] **Step 5: Run the crate gates**

Run:
```bash
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: both clean.

- [ ] **Step 6: Commit**

```bash
git add settings/tests/support/displays.rs settings/tests/displays.rs
git commit -m "$(cat <<'MSG'
test(settings): a harness driver for the Displays gates

`ui/tests/support/mod.rs`'s Driver shape with `icedtea-settings` in place of
the gallery: an isolated XDG_CONFIG_HOME, coordinates read out of the probe
report instead of hard-coded, and the same pointer settle the seat's focus
assignment needs.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 11: The Displays rest-state gate

**Files:**
- Modify: `settings/tests/displays.rs`

**Interfaces:**
- Consumes: Task 10's `SettingsDriver`, `paints_something`, `matches`, `TEST_THEME`, `REACT`; Task 8's page ids.
- Produces: `displays_page_paints_every_probe_point_at_rest_in_the_{light,dark,high_contrast}_theme`.

**Mutation check for the whole gate:** make `canvas::draw` return without
issuing a single `draw_rect`; the canvas probe point stops painting and all
three theme tests fail. Restore.

- [ ] **Step 1: Write the failing tests**

Append to `settings/tests/displays.rs`:

```rust
/// Every id the Displays page is contractually required to expose
/// (M5 contract §2.3's widget-id list, the `displays.*` row).
const DISPLAYS_IDS: &[&str] = &[
    "displays_canvas",
    "displays_enabled",
    "displays_resolution",
    "displays_refresh",
    "displays_scale",
    "displays_transform",
    "displays_position",
    "displays_status",
    "displays_test",
    "displays_revert",
    "displays_apply",
];

/// The rest-state gate: at rest, in `theme`, every probe point of the Displays
/// page paints something. No `KNOWN_BLANK` exemptions — the spec is explicit
/// that app widgets get none (§7).
fn displays_page_paints_at_rest(theme: &str) {
    let mut driver = SettingsDriver::open(theme, "displays");
    assert!(
        driver.wait_state("displays.dirty", "false", REACT),
        "the settings process never reported its model"
    );

    // Every contractual id must have reported an allocation…
    for id in DISPLAYS_IDS {
        let a = driver.alloc(id);
        assert!(
            a.w > 0 && a.h > 0,
            "{id} has a zero-area allocation {a:?} in the {theme} theme"
        );
    }

    // …and paint inside it.
    let background = driver.background();
    let frame = driver.capture();
    let mut blank = Vec::new();
    for id in DISPLAYS_IDS {
        let a = driver.alloc(id);
        if !support::paints_something(&frame, (a.x, a.y, a.w, a.h), background) {
            blank.push(*id);
        }
    }
    assert!(
        blank.is_empty(),
        "these Displays widgets paint nothing at rest in the {theme} theme: {blank:?}"
    );
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    displays_page_paints_at_rest("light");
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    displays_page_paints_at_rest("dark");
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    displays_page_paints_at_rest("hc");
}

/// The canvas specifically: its own backdrop must differ from the window's, so
/// a canvas that painted nothing but inherited the page background could not
/// pass the gate above by accident.
///
/// Mutation check: make `CANVAS_BG` equal the theme's window background; this
/// fails. Restore.
#[test]
fn the_canvas_paints_its_own_backdrop() {
    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));
    let canvas = driver.alloc("displays_canvas");
    let footer = driver.alloc("displays_footer");
    // A point inside the canvas, and one inside the footer's chrome.
    let inside = driver.pixel(canvas.x + canvas.w / 2, canvas.y + 4);
    let page = driver.pixel(footer.x + 2, footer.y + footer.h / 2);
    assert!(
        !support::matches(inside, page),
        "the canvas backdrop {inside:?} is indistinguishable from the page {page:?}"
    );
}
```

Add `"displays_footer"` handling: `footer(m)` already carries that id (Task 7),
so `driver.alloc("displays_footer")` resolves. It is deliberately not in
`DISPLAYS_IDS` — the footer is a container whose own box may legitimately be
untinted; its four children are what the gate checks.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --test displays`
Expected: FAIL — before Tasks 2/4/7/8 landed the ids do not exist; on the
completed tree they must pass, so run this **first against a deliberately
broken canvas** to confirm the gate has teeth: comment out the body of
`canvas::draw`'s closure, run, see
`the_canvas_paints_its_own_backdrop` and the three theme tests fail, restore.

- [ ] **Step 3: Restore and run to verify they pass**

Run: `cargo test -p icedtea-settings --test displays`
Expected: PASS — 6 tests.

- [ ] **Step 4: Run the crate gates**

Run:
```bash
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/displays.rs
git commit -m "$(cat <<'MSG'
test(settings): the Displays rest-state gate, light/dark/hc

Every contractual `displays.*` id reports a non-zero allocation and paints
inside it, in all three themes, with no KNOWN_BLANK exemptions. A separate
assertion pins the canvas backdrop as distinct from the page so a blank
canvas cannot pass by inheriting the window colour.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 12: The drag interaction gate and the drop-down fit gate

**Files:**
- Modify: `settings/tests/displays.rs`

**Interfaces:**
- Consumes: Task 10's `SettingsDriver` (`drag`, `click`, `alloc`, `point`, `state`, `wait_state_change`); Task 8's `state displays.position` line; Task 9's `drop_down_list_height`.
- Produces: `dragging_a_head_snaps_it_and_updates_the_model`, `a_drop_down_list_fits_inside_the_settings_window`.

- [ ] **Step 1: Write the failing tests**

Append to `settings/tests/displays.rs`:

```rust
/// Contract §2.8's named interaction gate: a drag across the canvas moves a
/// head and the model's rectangle follows, read out of the probe report rather
/// than off a pixel.
///
/// The harness advertises a single head, so there is no neighbour to snap
/// against — which makes the origin the binding candidate, and that is exactly
/// what `snap` is asked to prove here: dragged near `(0, 0)` the head lands
/// *on* it, and dragged far away it lands where it was dropped.
///
/// Mutation check: delete the `PointerMotion` forwarding arm from
/// `DrawingAreaC::on_event` (P0's M5-D5 §4); `displays.position` never changes
/// and this fails on the first `wait_state_change`. Restore.
#[test]
fn dragging_a_head_snaps_it_and_updates_the_model() {
    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));

    let canvas = driver.alloc("displays_canvas");
    let start = driver.state("displays.position");
    assert_eq!(
        start.as_deref(),
        Some("0 0"),
        "the harness head starts at the origin"
    );

    // Press on the head's tile — the canvas centre, where the only head is
    // drawn — and drag it well clear of the origin, staying inside the canvas.
    let from = (canvas.x + canvas.w / 2, canvas.y + canvas.h / 2);
    let to = (canvas.x + canvas.w - 8, canvas.y + canvas.h - 8);
    driver.drag(from, to);

    let moved = driver
        .wait_state_change("displays.position", start.clone(), REACT)
        .expect("the drag never reached the model");
    assert_ne!(
        moved.as_str(),
        "0 0",
        "the head must have left the origin; report said {moved:?}"
    );
    assert!(
        driver.wait_state("displays.dirty", "true", REACT),
        "a move must dirty the page"
    );
    // The position is two integers, so the snap ran and rounded.
    let parts: Vec<i32> = moved
        .split_whitespace()
        .map(|n| n.parse().expect("an integer position"))
        .collect();
    assert_eq!(parts.len(), 2, "position is `<x> <y>`, got {moved:?}");

    // Drag it back to within the snap threshold of the origin: it snaps flush.
    let back = (canvas.x + canvas.w / 2, canvas.y + canvas.h / 2);
    driver.drag(to, back);
    assert!(
        driver.wait_state("displays.position", "0 0", REACT),
        "a head dropped near the origin must snap to it; report said {:?}",
        driver.state("displays.position")
    );

    // Selecting the head is part of the same gesture (contract §2.8).
    assert_eq!(driver.state("displays.selected").as_deref(), Some("0"));
}

/// Contract §2.8's P7-D54 regression: an embedded `DropDown` list must fit
/// inside settings' window rather than being opened at a flat 240px.
///
/// The transform dropdown has all eight variants and is the tallest list on
/// the page; the resolution list is whatever the harness head advertises.
/// Both must open with their whole body inside the window.
///
/// Mutation check: restore the literal `240` in
/// `ui/src/widgets/drop_down.rs`'s `popover.open` call; the assertion on the
/// resolution list's height fails (the harness head advertises one or two
/// modes, so 240px is far taller than its content). Restore the fix.
#[test]
fn a_drop_down_list_fits_inside_the_settings_window() {
    use icedtea_ui::widgets::drop_down::{DROP_DOWN_MAX_PX, drop_down_list_height};

    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));

    let root = driver.alloc("root");
    let resolution = driver.alloc("displays_resolution");
    let (x, y) = (
        resolution.x + resolution.w / 2,
        resolution.y + resolution.h / 2,
    );
    driver.click(x, y);

    // The retained list body reports its own allocation once it is revealed.
    let list = {
        let mut found = None;
        let deadline = std::time::Instant::now() + REACT;
        while std::time::Instant::now() < deadline {
            if let Ok(a) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                driver.alloc("displays_resolution_list")
            })) {
                if a.h > 0 {
                    found = Some(a);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        found.expect("the resolution list never reported an allocation")
    };

    assert!(
        list.h <= i32::try_from(DROP_DOWN_MAX_PX).unwrap(),
        "the list is {}px tall, past the {DROP_DOWN_MAX_PX}px cap",
        list.h
    );
    assert!(
        list.y + list.h <= root.y + root.h,
        "the list runs past the bottom of the window: list {list:?}, root {root:?}"
    );
    // And it is content-sized, not flat: a short model gets a short list.
    let expected = i32::try_from(drop_down_list_height(2)).unwrap();
    assert!(
        list.h <= i32::try_from(DROP_DOWN_MAX_PX).unwrap() && list.h > 0,
        "a content-sized list is somewhere in (0, {DROP_DOWN_MAX_PX}]; \
         a two-row model would be {expected}px, got {}",
        list.h
    );
}
```

The `displays_resolution_list` id is `DropDownC`'s own `listview` subnode,
labelled by the probe writer as `<id>_list` when the parent has an id. If the
crate labels it by CSS node name instead, read the label the report actually
carries (`driver.probe_points()` lists them all) and use that; do not add an id
to `drop_down.rs` beyond the height fix.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --test displays`
Expected: on the tree before Task 9, `a_drop_down_list_fits_inside_the_settings_window`
FAILS with a list height of 240; after Task 9 it passes. Confirm the drag gate
by the mutation named in its doc comment.

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --test displays`
Expected: PASS — 8 tests.

- [ ] **Step 4: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/displays.rs
git commit -m "$(cat <<'MSG'
test(settings): drag-to-snap and drop-down-fit gates for Displays

The drag gate asserts the *model's* rectangle through the probe report's
`state displays.position` line, in both directions: away from the origin,
then back inside the snap threshold so it lands flush. The fit gate is
P7-D54's regression, now that a list is sized to its content.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 13: The cross-page gate §2.8 assigns to P4

**Files:**
- Create: `settings/tests/interaction.rs`

**Interfaces:**
- Consumes: Task 10's `SettingsDriver` and helpers; `zbus` 5 (already a normal dependency of `icedtea-settings`, so available to its tests).
- Produces: `apply_reaches_reload_config_on_the_mock`.

Contract §2.8 lists this gate under P4 although it exercises the footer P1 owns
(plan P4-D12). P4 adds it as a **test only** and edits no page source outside
`pages/displays/`. **`a_colour_pick_changes_the_swatch` is not written here** —
it is P2's gate (P2-D4), lands in `settings/tests/appearance.rs`, and asserts
against P2's `button_from(drawing_area)` swatches (P2-D11), not against
`ColorDialogButtonC` (consistency-check ruling E4). The "recording `ReloadClient` seam" is a test-owned D-Bus
service, not a source seam (plan P4-D13) — `compositor_reload.rs` is a file no
part may touch.

- [ ] **Step 1: Write the failing tests**

Create `settings/tests/interaction.rs`:

```rust
//! The cross-page interaction gate M5 contract §2.8 assigns to P4.
//!
//! It drives the real `icedtea-settings` binary under `icedtea_harness`,
//! stands up its own
//! `org.icedtea.Compositor` service on the session bus and records the
//! `ReloadConfig` calls it receives (plan P4-D13); it skips visibly when the
//! bus is unavailable or the name is already owned, the same posture
//! `settings/tests/live_apply.rs` takes with the real compositor.

#[path = "support/displays.rs"]
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH};
use support::{REACT, SettingsDriver, TEST_THEME};

/// Whether `name` already has an owner on the session bus.
fn name_owned(name: &str) -> Option<bool> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(name),
        )
        .ok()?;
    reply.body().deserialize::<bool>().ok()
}

/// The recording stand-in for a running compositor: it answers `ReloadConfig`
/// and counts the calls. Method names are PascalCase on the wire, which is
/// exactly the regression inherited decision 10 exists for.
struct RecordingCompositor {
    calls: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.icedtea.Compositor")]
impl RecordingCompositor {
    fn reload_config(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Contract §2.8: Apply reaches `ReloadConfig`.
///
/// Mutation check: change the `"ReloadConfig"` member in
/// `settings/src/compositor_reload.rs` to `"reload_config"`; the recorder
/// never sees a call and this fails. Restore. (That file is read-only for P4 —
/// perform the mutation, observe, and `git checkout` it.)
#[test]
fn apply_reaches_reload_config_on_the_mock() {
    let Some(owned) = name_owned(COMPOSITOR_BUS_NAME) else {
        eprintln!("skipping: no session bus");
        return;
    };
    if owned {
        eprintln!("skipping: {COMPOSITOR_BUS_NAME} is already owned");
        return;
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let _service = zbus::blocking::connection::Builder::session()
        .expect("session bus")
        .name(COMPOSITOR_BUS_NAME)
        .expect("claim the bus name")
        .serve_at(
            COMPOSITOR_PATH,
            RecordingCompositor {
                calls: Arc::clone(&calls),
            },
        )
        .expect("serve the interface")
        .build()
        .expect("build the service");

    let mut driver = SettingsDriver::open(TEST_THEME, "behavior");

    // Dirty the model: toggle a switch, which enables Revert and Apply.
    let switch = driver.alloc("behavior_raise_on_focus");
    driver.click(switch.x + switch.w / 2, switch.y + switch.h / 2);

    let apply = driver.alloc("apply");
    driver.click(apply.x + apply.w / 2, apply.y + apply.h / 2);

    let deadline = Instant::now() + REACT;
    while Instant::now() < deadline && calls.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "Apply must reach ReloadConfig exactly once"
    );
}
```

The `appearance_accent_dialog` label is the chooser body's node as the probe
writer labels it. If the report carries a different label, read
`driver.probe_points()` and use the one it actually reports — do not add an id
to a P2-owned page.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --test interaction`
Expected: on a tree where `ColorDialogButtonC` ignores its colour prop, or
where the wire member is misspelled, both fail; on the completed tree both
pass. Perform each named mutation, observe the failure, restore.

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --test interaction`
Expected: PASS — 2 tests (the second may print a visible skip on a machine
with no session bus, which is the documented posture).

- [ ] **Step 4: Run the crate gates**

Run:
```bash
cargo test -p icedtea-settings
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: all clean.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/interaction.rs
git commit -m "$(cat <<'MSG'
test(settings): the Apply-reaches-ReloadConfig gate

The cross-page gate contract §2.8 assigns to P4 (P4-D12), added as a
test only; the colour-pick gate is P2's (P2-D4). The "recording ReloadClient seam" is a test-owned
org.icedtea.Compositor service that counts ReloadConfig calls (P4-D13),
since compositor_reload.rs is read-only for every part.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 14: Close-out — moved tests, full gates, contract amendments

**Files:**
- Modify: `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6)

**Interfaces:**
- Consumes: everything in Tasks 1–13.
- Produces: the P4 entries of the contract's amendment ledger, which P6's §4.3
  sweep expects to find already present.

- [ ] **Step 1: Prove the twenty moved tests still pass byte-identically**

Run:
```bash
git diff --stat develop -- settings/src/pages/displays_canvas.rs settings/src/pages/displays/state.rs
cargo test -p icedtea-settings --lib pages::displays_canvas
cargo test -p icedtea-settings --lib pages::displays::state
```
Expected: the `git diff --stat` prints **nothing** for either file — P4 calls
them and never edits them — and the two runs report 8 and 12 passing tests
respectively.

- [ ] **Step 2: Prove the M3 gates are untouched**

Run:
```bash
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test gallery_gate
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui --test reconcile_props
cargo test -p icedtea-ui --test window_events
cargo test -p icedtea-ui --test counter_app
cargo test -p icedtea-ui --test layer_shell_screencopy
cargo test -p icedtea-ui --test transition_screencopy
cargo test -p icedtea-ui --test themed_button_offscreen
cargo test -p icedtea-ui --test adwaita_coverage
cargo test -p icedtea-ui --test gtk4_property_reference
```
Expected: all green; `interaction_gate` reports 16 passing tests.

- [ ] **Step 3: Prove no GTK crept in**

Run:
```bash
cargo tree -p icedtea-settings -e normal | grep -Ei '\b(gtk4|gtk4-layer-shell|glib|gio|gdk|pango|cairo)\b' || echo "clean"
grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests || echo "clean"
```
Expected: `clean` from both.

- [ ] **Step 4: Run the full part gate**

Run:
```bash
cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo test -p icedtea-settings
cargo test -p icedtea-ui
```
Expected: all clean. `settings/tests/live_apply.rs` and
`settings/tests/outputs_client.rs` pass unmodified — `git diff develop --
settings/tests/live_apply.rs settings/tests/outputs_client.rs` prints nothing.

- [ ] **Step 5: Append the amendments to the contract**

Under `## 6. Amendments` in
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md`, append one block per
deviation in the M3 §10 shape — **Carried out by / Added / Contract says / As
shipped / Ruling**. Write all thirteen, `P4-D1` … `P4-D13`, copying the ruling
text from this plan's "Contract deviations" section verbatim and adding the
commit hash each landed in. For example:

```markdown
### P4-D8 — the Displays submit runs inside `update`, and the pump returns `Result`

**Carried out by:** P4 (`settings/src/outputs/pump.rs`,
`settings/src/pages/displays/mod.rs`), commit `<hash>`.

**Contract says** (§2.5): `pub fn test_configuration(&self, edits: &[HeadEdit]);`
and `pub fn build_and_send_configuration(&self, edits: &[HeadEdit]);` —
"Fire-and-forget, from `Cmd::Task`."

**As shipped:** both return `Result<(), crate::outputs::OutputsError>`, and
`displays::submit` calls them from `update`.

**Ruling:** `Cmd::Task` has no return channel (M5-D3: "Anything whose answer
matters comes back through the inbox as a `Msg`, never as a return value"), and
the pump holds no `InboxSender`. A submit that fails before it reaches the wire
would therefore neither report itself nor clear `displays_in_flight`, leaving
Test and Apply dead for the rest of the session. The call is a wayland request
build plus a `flush` on the loop thread — precisely what the GTK click handler
did — not a blocking D-Bus round trip, so spec D8's "outbound calls run on the
worker via `Cmd::Task`, never inside `update`" is not engaged.
```

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'MSG'
docs(m5): record P4's thirteen contract amendments

P4-D1..P4-D13 in the M3 §10 shape, so P6's close-out sweep finds them
already in the ledger.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Task |
|---|---|
| Spec §5.4 "P4 Displays": `DrawingArea` canvas with `displays_canvas::{compute_view, hit_test, snap}` **verbatim** | 1, 2, 3 |
| Spec §5.4: `PointerDown`/`Motion`/`Up` on the canvas | 2 (registration), 3 (behaviour) |
| Spec §5.4: per-head panel — enabled `Switch`, resolution / refresh / transform / scale controls | 4, 5 |
| Spec §5.4: "its own Test/Revert/Apply footer as today" | 7 |
| Spec §5.4 / §10 risk: "an embedded `DropDown` list is a fixed 240 px … P4 sizes lists to content with a max-height scroll" | 9, 12 |
| Spec D4: `displays_canvas.rs` reused verbatim | 14 Step 1 (proved by `git diff --stat`) |
| Spec D6: rest-state screencopy + interaction gates per page | 11, 12 |
| Spec §7 "New gates": rest-state per page, light/dark/hc, no `KNOWN_BLANK` for app widgets | 11 |
| Spec §7: "displays drag-to-snap moves a head and the model's rect updates" | 12 |
| Spec §7: "a colour pick changes a swatch" | 13 |
| Spec §7: "Apply reaches `ReloadConfig` on the mock" | 13 |
| Contract §2.8 canvas: `pub fn draw(state: DisplaysSnapshot) -> impl Fn(&mut Canvas, Rect, &mut PaintCx)` | 1 (`DisplaysSnapshot`), 2 (`draw`) |
| Contract §2.8: eight literal RGB triples and two label offsets unchanged | 2 (`the_canvas_palette_is_the_gtk_palette`, P4-D11 for the baseline conversion) |
| Contract §2.8 drag rules — began / dragged / ended, grab semantics | 3 |
| Contract §2.8 controls: all eight transform variants (finding #8) | 4 (`TRANSFORM_LABELS`), 5 (`picking_a_transform_writes_a_raw_wl_output_value`) |
| Contract §2.8: `default_mode_for` on enable and on a refresh pick (findings #9/#14) | 5 |
| Contract §2.8: `Test`/`Apply` set `displays_in_flight` | 7 |
| Contract §2.8 unit test `a_drag_that_hits_nothing_leaves_the_selection_alone` | 3 |
| Contract §2.8 unit test `a_drag_snaps_the_dragged_head_against_its_neighbour` | 3 |
| Contract §2.8 unit test `a_release_outside_the_canvas_ends_the_drag` | 3 |
| Contract §2.8 unit test `enabling_a_mode_less_head_gives_it_a_default_mode` | 5 |
| Contract §2.8 unit test `an_apply_reply_clears_in_flight_by_its_own_is_test_tag` | 6 |
| Contract §2.8 gate `displays_page_paints_every_probe_point_at_rest` | 11 |
| Contract §2.8 gate `dragging_a_head_snaps_it_and_updates_the_model` | 12 |
| Contract §2.8 gate `a_drop_down_list_fits_inside_the_settings_window` | 12 |
| Contract §2.3 widget ids `displays.{canvas, enabled, resolution, refresh, transform, scale, position, test, revert, apply, status, unavailable}` | 4, 7, 8 (asserted in 4, 7, 8, 11) |
| Contract §2.5: `OutputsMsg` variants handled exactly as the glib source's `match` | 6 |
| Contract §2.5: `ConfigData { is_test }` tags the reply; `displays_in_flight` never infers it | 6 |
| Contract §5 P4 "Must not touch: `displays_canvas.rs`, `displays/state.rs`, any other `ui/` file" | 14 Step 1, and P4 touches only `drop_down.rs` under `ui/` |
| Contract §5 P4 gate: "`interaction_gate.rs` still 16/16 after the drop-down change" | 9 Step 5, 14 Step 2 |
| Contract §5 P4 gate: "the twenty moved tests still pass byte-identically" | 14 Step 1 |
| Inherited decision 11: `live_apply.rs` keeps the real binary | 14 Step 4 (asserted by an empty `git diff`) |

No gap found. Every item §2.8 names has a task; every task ends in a runnable
gate.

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `implement later`, `fill in details`,
`add appropriate error handling`, `handle edge cases`, `write tests for the
above`, `similar to Task`. None present. Three places carry a *conditional*
instruction rather than a placeholder, and each names the exact fallback and
forbids inventing new API:

- Task 2 Step 1 and Task 4 Step 1: the `View` introspection helpers
  (`children`, `handler_kinds`, `id_of`, `ids_of`). The instruction is "use the
  name the crate exports, do not add one", with the reason (P4 owns exactly one
  `ui/` file).
- Task 7 Step 4: `SettingsModel.outputs` — "confirm it exists; add it if P1 did
  not", with the exact declaration and the `SettingsModel::new` threading.
- Task 12 / Task 13: the `displays_resolution_list` and
  `appearance_accent_dialog` probe labels — "read `driver.probe_points()` and
  use the label the report carries", again with no new id permitted.

Each is a *verification* step with a determined outcome, not deferred design.

### 3. Type consistency

Checked every name that crosses a task boundary:

- `DisplaysSnapshot` — defined Task 1, consumed Tasks 2 (`draw`, `view`) and 3
  (`tile_centre` helper). Field names `rects`/`enabled`/`selected`/`names`/
  `details` identical throughout.
- `scene(&DisplaysSnapshot, f64, f64) -> CanvasScene` — one signature, used in
  Tasks 1, 2, 3.
- `CANVAS_W` / `CANVAS_H` / `CANVAS_FONT_PX` — defined Task 1, used in Tasks 2,
  3, 6 (`republish_view`).
- `drag_began` / `dragged` / `drag_ended` — defined Task 3, wired in Task 3's
  `app.rs` arms only.
- `repopulate` — defined Task 4, called from Task 3's `HeadDragBegan` arm and
  Tasks 5, 6, 7. One signature: `fn repopulate(st: &mut DisplaysState)`.
- `selection(st) -> Option<usize>` — private to `controls.rs`, used by all six
  mutators and all four index helpers; never leaks.
- `select_head` / `set_enabled` / `pick_resolution` / `pick_refresh` /
  `pick_transform` / `set_scale` — declared once in Task 5's Produces and used
  under exactly those names in Task 5's `app.rs` arms.
- `on_outputs(&mut SettingsModel, &OutputsMsg)` — Task 6; `Msg::Outputs` carries
  `Arc<OutputsMsg>` so the arm passes `msg.as_ref()`. Matches contract §2.2.
- `reset_edits` / `republish_view` — defined Task 6, reused by Task 7's
  `revert` and Task 6's `ApplyFailed { is_test: false }` arm.
- `submit(&mut SettingsModel, bool)` / `revert(&mut SettingsModel)` /
  `footer(&SettingsModel)` — Task 7, wired in Task 7's arms and Task 8's `view`.
- `OutputsPump::{test_configuration, build_and_send_configuration}` — one
  amended signature (Task 7), used only by `submit`.
- `drop_down_list_height(usize) -> u32`, `DROP_DOWN_ROW_PX`, `DROP_DOWN_MAX_PX`
  — defined Task 9, used in Task 9's test, Task 9's call site, and Task 12's
  fit gate.
- `SettingsDriver` — defined Task 10; every method Tasks 11–13 call
  (`open`, `background`, `probe_points`, `point`, `alloc`, `state`,
  `wait_state`, `wait_state_change`, `capture`, `pixel`, `click`, `drag`) is in
  its `impl`. `support::{matches, paints_something}` are free functions, and
  both are used as such.
- `state_report_lines` / `page_named` / `page_from_env` — defined Task 8,
  consumed by Task 10's driver (through the report file) and Task 12.
- Status constants `STATUS_*` and `UNAVAILABLE_TEXT` — defined Task 6, asserted
  in Tasks 6 and 7, rendered in Tasks 7 and 8.

One naming hazard checked and resolved: the plan uses `Msg::HeadDragged` (the
contract's variant) everywhere, never `Msg::HeadDragMotion`. Two `Rect` types
coexist — `displays_canvas::Rect` (f64, layout and canvas space) and
`icedtea_ui::layout::Rect` (f32, the paint callback's box, imported as
`UiRect` in `canvas.rs`) — and Task 2's `draw` is the only place both appear,
where the conversion is explicit.
