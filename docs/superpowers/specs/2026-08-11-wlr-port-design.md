# icedtea-wm Compositor Port to `wlr` — Milestone 1 (Vertical Slice)

Date: 2026-08-11

## Overview

Port the icedtea-wm compositor from smithay 0.7 to `wlr` (safe wlroots 0.20
bindings, crates.io `wlr`, repo `quinnjr/wlroots-sys`), growing `wlr`'s safe
API subsystem-by-subsystem with icedtea as the driving consumer. The smithay
baseline (Plan 1) is merged to `main` at `8577ff8` and remains the reference
for behavior.

Milestone 1 is a **vertical slice**: a nested session boots, xdg toplevels
render through `wlr_scene` at model-dictated geometry, keyboard and pointer
focus work (click-to-focus; seat focus follows the model), and shutdown is
clean (SIGINT/SIGTERM via a signalfd registered as a `wlr` fd source — the
slice's live consumer of the fd-source API, so that API too freezes with a
real user). Milestone 2 (separate spec and plan) is parity with the smithay
baseline: DBus control plane, config reload, decorations, snapping, alt-tab,
workspaces beyond the slice's minimum.

Out of scope for milestone 1: DBus wiring, config reload, SSD decoration
drawing, snapping, alt-tab, layer-shell, xdg-decoration, DRM/libinput session
testing (the code path exists via `autocreate` but only the nested and
headless backends are exercised).

## Decisions of record

- **Milestone shape:** vertical slice first; parity is milestone 2.
- **Renderer:** wrap `wlr_scene` (retained scene graph; damage tracking and
  output commit come from wlroots). No manual render pass.
- **Event loop:** single loop owned by wlroots. `wlr` grows a safe fd-source
  API (`wl_event_loop_add_fd`); icedtea keeps message-passing-only threading
  via crossbeam channels signalled through an eventfd registered as an fd
  source. calloop is removed.
- **Dependency mechanics:** icedtea consumes **published crates.io versions
  only** — no path or git dependencies at any point. Every `wlr` API addition
  is published before its icedtea consumer lands.
- **Version policy:** `wlr` stays on **0.20.x, additive-only**. The safe API
  is designed to be right the first time; a misdesigned API gets a
  deprecation shim, never a break. Consequence: every publish is preceded by
  an explicit API review gate, and each API's first real consumer (icedtea)
  is built against it before the publish freezes the shape (interleaved
  workflow below).
- **Approach:** interleaved in-place port. One icedtea branch (`wlr-port`);
  the compositor crate is rewritten in place. Work alternates between repos
  in subsystem-sized steps.

## Cross-repo workflow

Per subsystem: design the safe API in `~/Projects/wlroots-sys` → example
under `crates/wlr/examples/` + headless tests → API review gate (additive-only
makes the shape permanent) → `cargo publish` → icedtea task consumes the
published version. Both repos keep their own commit history and gates
(`cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`).

Three `wlr` publishes:

1. **Loop + scene.** fd-source registration on `EventLoop`; scene creation,
   per-output `SceneOutput` attachment, `commit()`, solid-color rect nodes.
2. **xdg-shell.** Toplevel lifecycle handler trait, borrow-scoped
   `Toplevel<'_>` + stored `ToplevelId`, automatic scene-tree insertion,
   `set_size`/`set_activated`/`send_close`, configure/ack surface.
3. **Seat + input.** Seat creation; keyboard/pointer handler traits with
   xkb-translated keysyms from `wlr_keyboard`; `set_keyboard_focus`/clear;
   pointer motion/button with scene-based hit test (`wlr_scene_node_at`).

## `wlr` safe-API design

- **Ownership model extends, never bends.** Every new object kind follows the
  crate's established pattern: borrow-scoped handles (`Toplevel<'_>`,
  `Pointer<'_>`) that cannot escape the handler call that provided them, and
  stored ids (`ToplevelId`, `SourceId`) that are stable, comparable, and
  hashable. Ids map 1:1 onto icedtea's `WindowId`-keyed model.
- **Handler traits, additively.** `Backend::run<S: OutputHandler>` is
  published and its bound cannot tighten. Each subsystem adds its own
  fully-defaulted trait (`ToplevelHandler`, `SeatHandler`, `FdHandler`).
  Release 2 adds one new entry point — `Backend::run_all<S: Handlers>` —
  where `Handlers` is a blanket-implemented supertrait of all handler traits,
  so an `OutputHandler`-only consumer qualifies automatically. `run` remains
  forever as the output-only path.
- **fd sources.** `EventLoop::add_fd(RawFd, Interest) -> SourceId`;
  `FdHandler::fd_ready(&mut self, SourceId)`; removal by id. Callbacks run on
  the loop like every other handler.
- **Scene.** `Scene` created before `autocreate`; `SceneOutput` attached in
  `new_output`; `OutputHandler::frame` body becomes `scene_output.commit()`.
  Toplevels are inserted into the scene tree by the library; the consumer
  positions them by id (`scene.set_position(ToplevelId, x, y)`). Solid-color
  rect nodes ship in release 1 (background in the slice; SSD strips reuse
  them at parity).
- **Panic policy.** Handler methods run under an `extern "C"` frame; an
  escaping panic aborts the process. Consumer handler bodies must be
  panic-free.

## icedtea-side changes

- `compositor/Cargo.toml`: `smithay` and `calloop` out; `wlr` (strict
  crates.io version) in.
- `backend.rs`: shrinks to `Display`/`Scene` creation, `Backend::autocreate`,
  `run_all(state)`. The winit path and the loud DRM stub are deleted —
  `autocreate` provides nested-under-Wayland, X11, headless, and DRM from one
  call. `--nested` survives as a CLI alias for `WLR_BACKENDS=wayland`.
- `state.rs`: the model half is untouched (`apply_action`, `handle_command`,
  `emit_pending`, saved-geometry maps, focus-transition helpers). The
  smithay half is replaced: `sync_window_to_space` becomes
  `sync_window_to_scene` with the same contract and the same frame-space vs
  content-space invariant, implemented as scene-node positioning plus
  `set_size`/`set_activated` staging on a `ToplevelId`. `surface_to_window`
  becomes a `ToplevelId → WindowId` map. smithay delegate macros and handler
  impls are replaced by `wlr` handler-trait impls calling the same model
  functions.
- `render.rs`: reduces to scene setup + background rect. SSD strip drawing
  returns at parity (as scene rects).
- `window.rs`, `layout.rs`, `decoration.rs`, `input.rs` (state machines),
  `contract`, `config`: unchanged.
- **Unsafe policy:** exception (b) (calloop `Generic::get_mut`) is deleted
  with calloop. Exceptions (a) (redb, still unused) and (c)
  (`env::set_var("WAYLAND_DISPLAY")` at single-threaded startup) remain. All
  wlroots FFI unsafe lives inside the `wlr` crate, not in icedtea.

## Error handling

- `wlr::Error` on creation paths; startup failures bubble to `main` exactly
  as today.
- Handler bodies are panic-free by policy (abort semantics under
  `extern "C"`); icedtea's existing never-panic discipline (config crate
  contract) extends to every handler impl.
- Model-layer error semantics (`Option`-returning mutations, ignored invalid
  commands) are unchanged.

## Testing

- Model/layout/input/config/contract tests carry over untouched — they never
  depended on smithay.
- Each `wlr` release ships headless integration tests in wlroots-sys using
  wlroots' headless backend (`WLR_BACKENDS=headless`): real `autocreate`,
  real loop, real scene, no display required.
- icedtea gains a headless boot smoke test (create state, run N loop
  iterations, clean shutdown) that the smithay baseline never had.
- Client-driven protocol tests (spawning a real toplevel against the headless
  compositor) are a parity-milestone goal, not part of the slice.
- Gates in both repos: `cargo test --workspace` and
  `cargo clippy --all-targets -- -D warnings`, both green before any commit
  lands.

## Success criteria (milestone 1)

1. `icedtea-compositor` builds with no smithay or calloop dependency, from
   crates.io deps only.
2. SIGINT/SIGTERM terminate the loop cleanly through the fd-source path.
3. Running nested under an existing Wayland session: a spawned client's
   toplevel appears at the model's cascade position; a second client's
   toplevel appears cascaded and focused; clicking the first refocuses it
   (model + seat + `Activated` all agree, both ends of the transition).
4. Keyboard input reaches the focused client; focus-follows-model works via
   the same `sync_focus_change` semantics the baseline established.
5. Headless boot smoke test passes in CI conditions (no display).
6. Three `wlr` 0.20.x releases published, each with examples, headless tests,
   and a passed API review; icedtea pins the third.
7. All carried-over model tests pass unchanged; both repos' gates green.
