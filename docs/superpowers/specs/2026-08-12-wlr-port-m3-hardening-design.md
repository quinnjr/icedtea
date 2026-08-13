# icedtea-wm Compositor — Milestone 3 (Hardening)

Date: 2026-08-12

## Overview

Milestone 2 (parity, `docs/superpowers/specs/2026-08-11-wlr-port-m2-parity-design.md`)
is merged: icedtea-wm reaches smithay-baseline feature parity on the `wlr`
backend, consuming published `wlr` 0.20.5–0.20.11. M3 **hardens** the
compositor into a real daily-driver by closing the correctness gaps,
recorded limitations, and additive `wlr` obligations that M2 ledgered as
deferred, plus the D-Bus observability and SSD-button polish that make the
result usable by panels and by a person.

M3 is scoped from the M2 SDD ledger's deferred/parked/KNOWN-LIMITATION
lines and the M2 final + lex-review findings. It is a hardening milestone,
not a feature-expansion one: no new Wayland protocols, no new subsystems
(fm/clipboard/wallet/applets remain their own future specs). The one new
`wlr` release (0.20.12) is small and purely additive.

## Decisions of record

- **`wlr` 0.20.12 is published once, up front.** M2 interleaved a release
  per feature because each API was large and needed a real consumer to
  validate its shape before freezing. M3's `wlr` surface is three small,
  well-understood additive items — `add_rect_in_band` mirrors the
  already-shipped `add_rect_in_toplevel`, `set_layer_surface_output` is a
  one-call wrapper, and the `Display`-pinning `debug_assert` is internal.
  None needs consumer-driven iteration, so one release + one API-review
  gate replaces M2's six.
- **Additive-only within 0.20.x still holds.** 0.20.12 adds items; it
  changes no published signature or semantics. The 0.20.11 banded-tree
  stacking exception is already recorded and is not revisited.
- **Config reload preserves clients.** The recorded KNOWN LIMITATION
  (`state.rs` `apply_config` destroys every window row on reload,
  abandoning clients as invisible-but-alive) is fixed, not re-recorded.
  This is the milestone's highest-value and highest-risk item; it is split
  into two tasks (row preservation, then appearance/keybinding/wallpaper
  resync) so each is independently reviewable.
- **Oversized-window migration centers.** M2's Task-17 "product question"
  (a hot-removed window wider/taller than the survivor pins to the
  survivor's corner) is decided: center it in the survivor. Corner-pinning
  is visually jarring; centering matches how maximize/fullscreen already
  place oversized content.
- **Automated-only validation bar, unchanged.** New load-bearing behaviors
  gain client-protocol harness tests. The manual VM daily-drive checklist
  stays the user's own activity, not an M3 exit criterion.
- **Execution: SDD.** Subagent-per-task, per-task review + fix loops, a
  controller-held split-phase gate (review verdict in hand before the
  publish step is dispatched) for the 0.20.12 release, and a final
  whole-branch review before the merge window.

## Work groups

Grouped and sequenced; the plan turns each into one or more tasks. The
`wlr` release and its icedtea consumers come first because two icedtea
fixes depend on the new APIs; the rest is icedtea-only and independent.

### Group A — `wlr` 0.20.12 (one release)

- **`add_rect_in_band(band, w, h, color) -> RectId`** — create a solid rect
  parented into a named scene band (Background/Bottom/Toplevel/Top/Overlay),
  not the scene root. Root rects/buffers currently sit above every toplevel
  AND intercept pointer hit-tests over their area (M2 finding I6); a
  band-parented rect participates in z-order and input correctly. Mirrors
  `add_rect_in_toplevel`'s ownership shape; `remove_rect` already handles it.
- **`set_layer_surface_output(id, output) -> Option<()>`** — assign a layer
  surface to an output the client left unset (the compositor's protocol
  responsibility; M2 debt finding). `None` on unknown/stale id.
- **`Display`-pinning `debug_assert`** — the internal manager-pointer cache
  (`create_xdg_shell`/`create_seat`/etc.) is unsound if a `Runtime` is
  reused across a second `Display` (M2 finding I7; unreachable for icedtea,
  which builds one `Display`). Add a `debug_assert!` that the `Display`
  matches the one the manager was created against. No signature change.
- Gate (four conventions, panic sweep, additive-only), example/tests,
  publish, tag, consume-check. README obligations list updated (remove the
  three now-shipped items).

### Group B — icedtea consumes 0.20.12

- **Snap-preview & inert root nodes.** Move the snap-preview rect into the
  Overlay band via `add_rect_in_band` so it renders above windows AND does
  not swallow the clicks underneath it during a drag (M2 finding I6). Audit
  any other root-level rect/buffer that should be input-inert (the boot
  background rect is below everything and fine; confirm). Harness test:
  a click lands on the window under an active snap preview.
- Pin `wlr = "0.20.12"`.

### Group C — config-reload rewrite (two tasks)

- **C1 — row preservation.** Rework `apply_config` so a live reload diffs
  the new config against live state and **keeps existing window rows
  mapped** instead of destroying them. Preserve `WindowId`s, focus, MRU,
  workspace assignment, geometry, and the `mapped`/minimized/maximized/
  fullscreen state. The `next_id`/`seq` floor machinery and workspace-index
  reconciliation must survive without emitting a `WindowClosed` burst.
  Remove the KNOWN LIMITATION block.
- **C2 — appearance/keybinding/wallpaper resync.** On reload: recolor the
  background rect + every SSD band/button/title (palette-keyed raster cache
  already invalidates on foreground change), re-run keybinding validation
  (bad bindings non-fatal, same warn path as load), and re-spawn the
  wallpaper decode only when the path changed (clear nodes first, per the
  Task-7-carried contract). Every window re-syncs to the new appearance.
- Harness test: a client stays mapped and re-themed across a reload that
  changes palette, a keybinding, and the wallpaper path.

### Group D — focus-invariant pass

- One consolidated fix making **"the model never reports a hidden
  (unmapped or minimized) window as `focused`"** a tested invariant in
  every workspace state. Closes: inactive-workspace unmap (M2 I2 residual),
  inactive-workspace `DbCommand::Minimize` (M2 Task-21 residual), and
  last-mapped-window-with-no-candidate (leaves a stale pointer today). The
  fix mirrors `remove_window`'s no-successor pointer-clear, applied
  uniformly across the unmap/minimize/close/workspace-switch paths.
- Tests: focus after unmapping/minimizing the focused window on an inactive
  workspace, and after the sole window on a workspace becomes hidden.

### Group E — layer-shell arrangement completion

- **N7** — corner-anchored (or single-edge) surfaces honor `desired_size`
  on the free axis instead of over-carving/full-spanning; align the
  exclusive-edge choice with wlroots' `get_exclusive_edge`.
- **N8** — honor exclusive-zone **margins** in placement.
- **N9** — re-evaluate `keyboard_interactive` on post-map commits, not only
  at map, so a surface that becomes interactive later (a menu opening)
  takes keyboard focus.
- **N11** — skip inactive-workspace and minimized windows in
  `arrange_layers`' re-sync (perf + avoids spurious configures).
- **M5** — hot-remove layer re-homing resolves to the geometrically-correct
  survivor (frame/output containment), not `output_for_pointer`.
- Consume **`set_layer_surface_output`** for surfaces the client left
  output-unset.
- Tests: an interactive-on-demand menu takes keyboard post-map; a
  margined/corner panel places correctly.

### Group F — multi-output migration

- Center an oversized migrated window in the survivor (decision above).
  Test: a window larger than the survivor migrates centered, not
  corner-pinned, without panic.

### Group G — D-Bus observability

- **contract crate**: `WindowUpdate` gains a `mapped: bool` field; the
  seq-locked signal signature tests bump accordingly.
- Map/unmap already emit one `WindowUpdated` (M2's sanctioned
  `set_mapped` emission) — the change here is only that its payload now
  carries `mapped`, so subscribers can read it. Decoration-mode changes
  (`set_client_decorations_requested`) are today D-Bus-invisible and gain
  a new sanctioned `WindowUpdated` emission (one per transition, listed in
  the plan). The exactly-one-event-per-mutation invariant is preserved.
- Tests: a D-Bus `GetState`/signal round-trip observes a window's mapped
  and decoration state changing.

### Group H — cosmetic

- **SSD button glyphs** — render minimize/maximize/close glyphs into the
  button rects via the existing cosmic-text rasterizer (buttons are flat
  color chips today), and add hover/press visual feedback (color shift on
  the pointer-over / pressed button, driven by the existing pointer
  routing).
- **Unfocused-title dimming** — dim inactive-window titles via alpha rather
  than the current RGB approximation, now that the rasterizer path is in
  place (M2 Task-13 deviation 3).

### Group I — milestone close

- Full gates both repos; success-criteria sweep with named evidence per
  criterion; present the merge window (no auto-merge). The manual VM
  checklist is restated as the user's activity.

## icedtea-side changes

- `wayland.rs` seam: snap-preview via band rect; SSD buttons gain glyphs +
  a pressed/hover state parameter.
- `state.rs`: `apply_config` rewritten (row-preserving diff); the focus
  pointer-clear consolidated across hide paths; `arrange_layers` honors
  margins/desired/interactivity and skips hidden windows; migration
  centering; new D-Bus emissions on map/decoration transitions.
- `render.rs`/`text.rs`: button-glyph rasterization; alpha dimming.
- `contract` crate: `WindowUpdate.mapped`.
- `dbus.rs`: no new commands; the emitter observes the new events.
- `Cargo.toml`: `wlr = "0.20.12"`.

## Error handling

Unchanged from M2. Handler bodies stay panic-free; model mutations return
`Option`; creation-path `wlr::Error` bubbles to `main`; the reload rewrite
must itself never panic on a malformed reloaded config (the config crate's
never-panic contract already guarantees a parseable `Config`; invalid
keybindings warn and are skipped, as at load).

## Testing

- All carried-over model/layout/input/config/contract tests stay green.
- New client-protocol harness tests for the load-bearing behaviors: reload
  keeps a client mapped + re-themed; snap preview doesn't eat clicks; an
  interactive layer surface takes keyboard post-map; focus after hiding the
  last window on an inactive workspace; a D-Bus round-trip observes mapped/
  decoration state.
- `wlr` 0.20.12 ships an example + headless tests for the three new items.
- Gates in both repos per commit: `cargo test --workspace` and
  `cargo clippy --all-targets -- -D warnings` (wlroots-sys also `fmt` and
  `RUSTDOCFLAGS="-D warnings" cargo doc`).

## Success criteria (milestone 3)

1. A live config reload that changes palette, a keybinding, and the
   wallpaper path keeps every client mapped and re-themed — no client
   abandoned, no `WindowClosed` burst. (Harness test.)
2. No workspace state leaves a hidden window reported as `focused`:
   inactive-workspace unmap, inactive-workspace minimize, and
   last-window-hidden all clear the pointer. (Model + harness tests.)
3. Layer surfaces honor margins and `desired_size`, re-evaluate keyboard
   interactivity after map, and never hang or mis-home on hotplug.
   (Harness + model tests.)
4. The snap preview renders above windows and does not intercept pointer
   clicks. (Harness test.)
5. D-Bus subscribers observe a window's mapped state and decoration mode
   changing. (Round-trip test.)
6. SSD buttons show minimize/maximize/close glyphs and press feedback;
   unfocused titles dim via alpha.
7. `wlr` 0.20.12 published through the API-review gate with an example and
   headless tests; icedtea pins it; both repos' gates green; all
   carried-over tests pass unchanged.
8. The manual VM/daily-drive checklist remains open as a user activity —
   explicitly not an M3 criterion.
