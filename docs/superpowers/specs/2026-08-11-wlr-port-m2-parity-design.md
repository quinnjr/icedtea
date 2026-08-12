# icedtea-wm Compositor Port to `wlr` — Milestone 2 (Parity)

Date: 2026-08-11

## Overview

Milestone 1 (vertical slice, `docs/superpowers/specs/2026-08-11-wlr-port-design.md`)
is merged: a nested session boots on `wlr` 0.20.4, xdg toplevels render
through `wlr_scene` at model-dictated geometry, seat focus follows the model,
and shutdown is clean. Milestone 2 restores **feature parity with the smithay
baseline** (merged to `main` at `8577ff8`, the behavioral reference) on the
`wlr` backend, and closes the residuals M1 parked.

Parity scope (the full ledgered list — nothing deferred to an M3):

1. SSD proper decorations: title text + close/maximize/minimize buttons.
2. Client-driven maximize/fullscreen/move/resize (acknowledged regression
   vs the baseline).
3. DBus control plane + config-reload feature parity (gap-close; the wake
   pipes and `handle_command` dispatch already landed in M1).
4. Wallpaper actually drawn (decode plumbing is live; nothing renders the
   image yet).
5. Snapping visuals (snap-target overlay during drags).
6. Model-level "unmapped" concept — focus must never sit on an unmapped
   window.
7. Multi-output and hotplug (monotonic output ids landed in M1).
8. Layer-shell.

Plus the queued `wlr` 0.20.5 API items and the ledgered deferral that M1's
spec assigned to parity: a client-driven protocol test harness.

## Decisions of record

- **Structure: feature-vertical slices.** M2 is ordered by user-visible
  feature. Each slice publishes only the `wlr` API it needs, immediately
  before its icedtea consumer lands. (M1 was ordered by subsystem; M2
  trades that for visible progress per step.)
- **API review gate is unchanged and per-publish.** Feature-vertical
  ordering produces more, smaller publishes (~6); every one still goes
  design → example → headless tests → explicit API review gate → publish.
  The gate does not scatter: no `wlr` API lands unreviewed, and the
  additive-only 0.20.x policy holds (a misdesigned API gets a deprecation
  shim, never a break).
- **Text stack: cosmic-text.** Full shaping and font fallback (fontdb /
  swash / rustybuzz underneath), so CJK/emoji/RTL titles render correctly.
  Pinned as a normal icedtea dependency; `wlr` stays text-agnostic — it
  only ever sees finished RGBA pixels.
- **Validation bar: automated only.** The client protocol harness lands
  early in M2 and gates every later slice. The manual daily-drive checklist
  (task-11-report steps 1–7 plus the final review's 9 additions) and
  VM/DRM validation remain the user's own post-M2 activity, not an M2 exit
  criterion.
- **Dependency mechanics unchanged:** icedtea consumes published crates.io
  versions only; no path or git dependencies at any point.

## Slices

Each slice is: (`wlr` work, if any) → API review gate → publish → icedtea
consumption, with both repos' gates green per commit. Version numbers are
the expected sequence; if a slice needs an extra publish the numbers shift,
the policy does not.

### Slice 0 — M1 residuals (`wlr` 0.20.5)

The queued small items; several fix parked defects, so they precede new
features.

`wlr` additions:

- Rect-in-toplevel-tree (or `raise_rect`): rects can be attached to /
  raised with a toplevel's scene tree — fixes the parked M1 residual where
  the SSD interim band is not raised with its toplevel.
- `remove_fd(SourceId)`: fd sources removable by id.
- EINTR retry in `run_inner`.
- `KeyEvent::for_test` — closes the `SeatHandler` direct-coverage gap.
- `Toplevel`: `Debug` impl + current-size accessor.
- `#[doc]` steering on `run()` toward `run_all`.

icedtea consumption: pin 0.20.5; the SSD band raises with its toplevel;
`SeatHandler` paths gain direct tests via `KeyEvent::for_test`.

### Slice 1 — client protocol test harness

A headless icedtea compositor plus a real Wayland client (`wayland-client`
as a dev-dependency) driving automated protocol tests: connect, create a
toplevel, observe configure/geometry/activation, exercise close. Lands
early so every subsequent slice ships client-driven tests. Tests follow
the existing headless pattern (`WLR_BACKENDS=headless`, no display). No
new `wlr` API expected; if observation requires one small hook, it rides
the next slice's publish rather than earning its own.

### Slice 2 — wallpaper drawn (`wlr` 0.20.6: buffer scene nodes)

`wlr` addition: an RGBA pixel-buffer scene node — create from owned pixels
(width, height, stride), update contents, position/raise/destroy by stored
id, following the crate's borrow-handle + stored-id ownership model.

icedtea consumption: `render.rs` uploads the decoded wallpaper image
(plumbing already live from M1) into a buffer node per output, scaled per
the existing config semantics; the solid `wallpaper_color` remains the
fallback before decode completes or on decode failure, exactly as today.

### Slice 3 — client-driven maximize/fullscreen/move/resize (`wlr` 0.20.7)

The acknowledged regression vs the baseline.

`wlr` addition: fully-defaulted `ToplevelHandler` client-request methods —
`request_maximize`, `request_fullscreen`, `request_move`, `request_resize`
(with edge), carrying what the client sent; default bodies ignore, so
existing consumers are unaffected.

icedtea consumption: route the requests into the existing
`reconcile_maximized` / `reconcile_fullscreen` seams (already written and
waiting), and drive interactive move/resize through the existing
`input.rs` state machines with pointer-position updates each motion frame.
Protocol tests: a client requesting maximize observes the maximized
configure; move/resize alter model geometry.

### Slice 4 — SSD proper decorations (`wlr` 0.20.8: xdg-decoration)

`wlr` addition: `zxdg_decoration_manager_v1` negotiation surfaced as
toplevel decoration-mode events + a server-side set-mode call, so clients
that insist on CSD keep their own decorations.

icedtea consumption:

- cosmic-text rasterizes the title into RGBA and displays it via a slice-2
  buffer node inside the title bar; re-rasterize on title change, focus
  change (palette), and config reload. This closes the long-ledgered SSD
  title-text deferral.
- Close/maximize/minimize buttons as hit-tested sub-rects in the bar,
  routed through the existing pointer machinery; actions reuse the same
  model paths the DBus commands use.
- Decoration mode: SSD only for clients that accept it; `window.rs`'s
  existing ssd flag reflects the negotiated mode.

### Slice 5 — snapping visuals, unmapped concept, DBus/config gap-close

icedtea-only (no new `wlr` API expected):

- Snap-target overlay rect shown during drags — `scene_order` already
  models `snap_active`; this wires an actual translucent rect to the
  computed snap zone.
- Model-level "unmapped" window state: an unmapped window leaves the
  focus/alt-tab candidate set, focus falls to the model's next candidate,
  and remap restores eligibility. Extends the M1 unmap seat-trap fix from
  the seat layer into the model.
- DBus + config-reload parity audit: walk the baseline's DBus surface and
  reload behaviors against the current implementation, close what's
  missing, and add protocol/model tests for each closed gap.

### Slice 6 — multi-output + hotplug (`wlr` 0.20.9: output layout)

`wlr` addition: wrap `wlr_output_layout` — outputs added/removed with
positions, layout-box queries by output id, scene-output coordinate
mapping.

icedtea consumption: workspaces and wallpaper become per-output; windows
on a hot-removed output migrate to a surviving output (baseline
semantics); hot-add creates a scene output and wallpaper node. Headless
tests drive add/remove via the multi-output headless backend.

### Slice 7 — layer-shell (`wlr` 0.20.10)

`wlr` addition: wrap `wlr_layer_shell_v1` — layer surface lifecycle
handler trait (borrow handle + stored id), anchor/exclusive-zone/layer
data, configure/ack, scene insertion at the correct layer.

icedtea consumption: arrange layer surfaces per output at their anchors;
shrink the usable area by exclusive zones so `layout.rs` tiles inside it;
keyboard-interactivity handling for panels. Protocol tests with a
layer-shell client.

## Error handling

Unchanged from M1: `wlr::Error` on creation paths bubbles to `main`;
handler bodies are panic-free by policy (abort semantics under
`extern "C"`); model-layer semantics (`Option`-returning mutations,
ignored invalid commands) are untouched. cosmic-text rasterization
failures degrade to the interim band (rect without text), never panic.

## Testing

- Model/layout/input/config/contract tests carry forward untouched.
- Each `wlr` publish ships examples + headless integration tests in
  wlroots-sys, as in M1.
- New in M2: the slice-1 client protocol harness; every slice from 2 on
  adds client-driven tests for its feature where a client can observe it.
- Gates in both repos per commit: `cargo test --workspace`,
  `cargo clippy --all-targets -- -D warnings`.

## Success criteria (milestone 2)

1. All eight parity features work in a nested session, matching baseline
   behavior.
2. The client protocol harness runs headless in CI conditions and covers
   toplevel lifecycle, client-driven state requests, decoration mode,
   and layer-shell.
3. The M1 parked SSD z-order residual is closed (slice 0).
4. ~6 `wlr` 0.20.x publishes, each with examples, headless tests, and a
   passed API review; icedtea pins the last.
5. Both repos' gates green; all carried-over tests pass unchanged.
6. The manual VM/daily-drive checklist remains open as a user activity —
   explicitly not an M2 criterion.
