# icedtea-wm — Track A2: Client-Compatibility Protocol Batch — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Roadmap slot:** Track A, item **A2**
(`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`)
**Branch:** target `develop` (compositor/system track; toolkit-independent).
**Companion tracks:** A1 (XWayland) and the wlr-port M5 scene work run in
parallel wlroots-sys sessions — the publish cadence here must be coordinated
with them exactly as M4.4/M4.5 coordinated (shared `wlroots-sys` repo, `[patch]`
resolves to the checked-out branch).

## Goal

Close the "client-compat" row of the roadmap audit: add the standard Wayland
protocols that real applications, toolkits, docks, and pagers expect a modern
compositor to speak, so that (a) apps stop silently degrading (wrong cursors,
blurry HiDPI, focus-steal, no attention hints), (b) standard docks/pagers/tray
hosts — **and icedtea's own future taskbar** — can enumerate and act on windows
through a real protocol instead of only the bespoke `org.icedtea.WM` D-Bus
surface, and (c) the night-light feature (`shell-applets`/`nightlight` design)
gets its compositor backend. Eight protocols, delivered as a small number of
additive `wlr` publishes (0.20.23+) plus their compositor wiring:

- `xdg_activation_v1` — focus-steal prevention / request-attention.
- `wp_cursor_shape_manager_v1` (cursor-shape-v1) — named cursors from the theme.
- `wp_fractional_scale_manager_v1` (fractional-scale-v1) **+ `wp_viewporter`
  (viewporter)** — crisp HiDPI at fractional scales.
- `wp_content_type_manager_v1` (content-type-v1) — surface content-type hints.
- `wp_single_pixel_buffer_manager_v1` (single-pixel-buffer-v1) — cheap solid
  buffers.
- `wp_presentation` (presentation-time) — real presentation feedback.
- `zwlr_foreign_toplevel_manager_v1` (wlr-foreign-toplevel-management) —
  window enumeration/activation for docks, pagers, tray hosts, and the taskbar.
- `zwlr_gamma_control_manager_v1` (wlr-gamma-control) — night-light backend.

This design is compositor + wlr-crate only. It does **not** build any UI
consumer (the taskbar/pager/night-light UI are Track B / cross-cutting); it only
exposes the protocols and proves them with harness clients.

## Grounding in the current codebase

The wlr crate is a thin, hand-audited wrapper over wlroots 0.20 (repo
`wlroots-sys`, crate `wlr` on crates.io, currently `0.20.22`). The established,
proven pattern — used by every protocol M4.1–M4.5 landed — is:

1. wlroots already implements the protocol in C; the `wlr` crate adds a
   `Runtime::create_<x>_manager(&self, display: &Display) -> Result<()>` that
   calls the wlroots `wlr_<x>_manager_v1_create(display)` (or richer, see below),
   stores the returned `NonNull` in a `RuntimeInner` cell with a double-create
   guard, and — for protocols that raise per-object requests — installs a
   listener that fans events out to the compositor's handler traits.
2. The compositor creates the global at boot in `compositor/src/lib.rs`, as a
   **non-fatal** `if let Err(err) = runtime.create_<x>_manager(&display)` block
   beside the existing ones (`lib.rs:72–138`: xdg-decoration, layer-shell,
   primary-selection, data-control, virtual-keyboard, virtual-pointer,
   screencopy, session-lock, idle-notifier, idle-inhibit, pointer-constraints,
   relative-pointer, output-manager). The degraded-not-dead comment tone is the
   house style.
3. Rendering is wlroots' scene graph, owned by `wlr::Runtime` (`lib.rs:59`
   `Runtime::new()` "the scene graph"; the background rect is a scene node at
   `lib.rs:157`). Surfaces are scene nodes; **wlroots' `wlr_scene` handles all
   per-surface buffer transform/scale/crop internally** — this is why viewporter,
   fractional-scale, single-pixel-buffer, content-type and presentation-time are
   nearly free on the compositor side (the scene does the work once the global
   exists), whereas cursor-shape, xdg-activation, gamma-control and
   foreign-toplevel carry real compositor logic (request handlers / per-window
   handles).
4. Output scale is already a first-class, **fractional-capable** value:
   `state.rs:2587` `output.set_scale(Self::guarded_scale(cfg.scale))` where
   `guarded_scale` (`state.rs:2607`) accepts any finite `> 0` `f64` (tests at
   `state.rs:8218–8224` cover `2.5`), and the Displays settings page already
   round-trips a `scale: f64`. Fractional *output* scale is therefore already
   plumbed; what is missing is telling **clients** the fractional scale so they
   render sharp — that is exactly what fractional-scale-v1 + viewporter add.

## Decisions of record

1. **Split by compositor-integration nature into three additive publishes**, not
   one per protocol and not one giant drop. The natural seam is *how much
   compositor code each needs*, which also matches how each is reviewed and
   tested:
   - **Batch 1 — passive/scene-internal** (create the global; wlroots' scene or
     renderer does the rest): viewporter, fractional-scale, single-pixel-buffer,
     content-type, presentation-time.
   - **Batch 2 — request-driven handlers** (the crate must expose an event; the
     compositor must act): cursor-shape, xdg-activation, gamma-control.
   - **Batch 3 — foreign-toplevel-management** on its own: it is the only one
     with per-window *handle* lifecycle (create/update/destroy mirrored onto
     every managed window) plus a rich inbound request set. Sizing it alone keeps
     the publish additive-review tractable, exactly as each M4.x got its own.
   Every publish is additive-only within `0.20.x` (semver rule the port has held
   since M1). Batches 1 and 2 *could* collapse into a single publish if the user
   prefers fewer hard-stops; the milestone breakdown below keeps them separate
   because Batch 2 needs crate-exposed events and Batch 1 does not, and because
   Batch 1 is the higher-leverage, lower-risk half to ship first.
2. **Each publish is a hard-stop for consent** (project rule): API-review verdict
   held before the mechanical `cargo publish`, then the compositor bumps and
   consumes. Never auto-merged; a merge window is presented per branch.
3. **No new crates.** All eight are compositor-side protocols; harness clients
   bind them from `wayland-protocols` / `wayland-protocols-wlr` (both already
   deps — `compositor/Cargo.toml:79`), as the screencopy and pointer-constraint
   harness clients already do.
4. **cursor-shape uses the compositor's xcursor theme, not client pixmaps.**
   The handler maps the shape enum to a named xcursor and sets it on the seat's
   cursor; this unifies with the existing cursor rendering rather than adding a
   second cursor path.
5. **xdg-activation implements a real focus-steal policy**, not blind activation:
   a token is honored (raise+focus) only when it carries a serial/seat from a
   still-recent user interaction; otherwise the target window is marked
   *urgent/attention* and surfaced through the existing `org.icedtea.WM`
   `WindowUpdated` event rather than stealing focus. This is the whole point of
   the protocol per the roadmap ("focus-steal prevention/attention").
6. **foreign-toplevel handles are owned by the window model.** A handle is
   created when a window maps and destroyed when it unmaps/closes, and every
   `title`/`app_id`/state/output change already tracked by `WindowManager`
   (which already emits `WindowUpdated`) also pushes to the handle. Inbound
   requests (activate/close/maximize/minimize/fullscreen/set_rectangle) route
   into the **same** internal operations the `org.icedtea.WM` methods already
   call (`focus`/`close`/`min`/`max`/`fullscreen`), so the protocol and the D-Bus
   surface stay behaviorally identical — foreign-toplevel becomes a second
   front-end onto one WM core, which is what lets a standard pager and icedtea's
   own taskbar drive the same windows.
7. **gamma-control is the night-light backend only.** This milestone exposes the
   protocol so an external agent (e.g. `wlsunset`, or a future icedtea
   night-light daemon) can push gamma ramps; it does **not** build the
   day/night scheduling UI (that is the `nightlight` design / Track B).

## Architecture — per protocol

For each: **wlr crate** = what `wlroots-sys`/`wlr` must expose; **compositor** =
what `lib.rs`/`state.rs` must do; **nature** = pure-additive vs. handler vs.
render/scale-touching.

### Batch 1 — passive / scene-internal (wlr 0.20.23)

- **viewporter (`wp_viewporter`).**
  - *wlr crate:* `Runtime::create_viewporter(&display)` →
    `wlr_viewporter_create(display)`; `NonNull` cell + double-create guard. No
    listener (buffer src/dst crop-scale is a scene property wlroots reads at
    render time).
  - *compositor:* one boot line in `lib.rs`. No `state.rs` change — the scene
    already renders every surface and now honors its viewport.
  - *nature:* additive; **touches scaling** only in the sense that the scene now
    applies client-declared src-crop/dst-scale. No compositor math changes.

- **fractional-scale (`wp_fractional_scale_manager_v1`).**
  - *wlr crate:* `Runtime::create_fractional_scale_manager(&display)` →
    `wlr_fractional_scale_manager_v1_create(display, version)`. **Verify** (see
    Verification) whether `wlr_scene` auto-sends the preferred fractional scale
    per surface (it does in wlroots ≥0.17 via `wlr_scene_surface`), or whether
    the crate must additionally expose
    `wlr_fractional_scale_v1_notify_scale(surface, scale)` for the compositor to
    call from the output-enter path. Design assumes scene auto-send; the notify
    wrapper is the fallback and is itself additive.
  - *compositor:* boot line. If the scene auto-sends, **no** `state.rs` change;
    if not, send preferred scale = the window's current output scale
    (`OutputSurface`, `state.rs:506`) on map / output-migration.
  - *nature:* **render/scale-touching by intent** — this is the protocol that
    makes fractional output scales (e.g. `1.5`, already accepted by
    `guarded_scale`) produce *sharp* client buffers. Pairs with viewporter (the
    client scales a hi-res buffer down through a viewport). Ship the two together.

- **single-pixel-buffer (`wp_single_pixel_buffer_manager_v1`).**
  - *wlr crate:* `Runtime::create_single_pixel_buffer_manager(&display)` →
    `wlr_single_pixel_buffer_manager_v1_create(display)`; cell + guard, no
    listener.
  - *compositor:* boot line. The renderer/scene consume the buffer type
    automatically.
  - *nature:* pure-additive.

- **content-type (`wp_content_type_manager_v1`).**
  - *wlr crate:* `Runtime::create_content_type_manager(&display)` →
    `wlr_content_type_manager_v1_create(display, version)`; cell + guard.
  - *compositor:* boot line. The content-type is surface metadata wlroots
    attaches; the scene can use it later for direct-scanout/tearing hints. No
    handler required now.
  - *nature:* pure-additive.

- **presentation-time (`wp_presentation`).**
  - *wlr crate:* `Runtime::create_presentation(&display)` →
    `wlr_presentation_create(display, backend)` **and** a
    `Runtime::set_scene_presentation()` wrapping
    `wlr_scene_set_presentation(scene, presentation)` — the one wiring call that
    makes the scene emit per-surface presentation feedback on each output commit.
  - *compositor:* boot line to create, plus one call to hand the presentation to
    the scene after `init_graphics` (the scene already commits outputs on the
    frame handler; feedback rides that path for free once set).
  - *nature:* additive + one wiring line; no render math.

### Batch 2 — request-driven handlers (wlr 0.20.24)

- **cursor-shape (`wp_cursor_shape_manager_v1`).**
  - *wlr crate:* `Runtime::create_cursor_shape_manager(&display)` →
    `wlr_cursor_shape_manager_v1_create(display, version)`, **plus** a listener
    on the manager's `request_set_shape` signal that fans out to a new
    `CursorShapeHandler::request_set_shape(device, serial, shape)` on the
    compositor's handler trait (the crate already owns the seat/cursor; it must
    expose the shape enum and the requesting device so the compositor can honor
    only the currently-focused pointer/tablet). The crate should also expose the
    xcursor-name lookup (`wlr_cursor_shape_v1_name(shape)`) or accept the enum
    directly on a `Runtime::set_cursor_shape(...)` helper.
  - *compositor:* handle `request_set_shape` by loading the named cursor from the
    active xcursor theme and setting it on the seat cursor — routed through
    whatever `input.rs` already uses to set the pointer image (the compositor
    already manages a cursor; this replaces the default arrow with the requested
    shape while the pointer is over that client's surface, and reverts on focus
    change). Guard on serial/focus so a background client cannot change the
    cursor.
  - *nature:* handler; touches `input.rs` cursor path. No scaling.

- **xdg-activation (`xdg_activation_v1`).**
  - *wlr crate:* `Runtime::create_xdg_activation_manager(&display)` →
    `wlr_xdg_activation_v1_create(display)`, plus a listener on `request_activate`
    fanning out to `ActivationHandler::request_activate(surface, token)` with the
    token's recorded seat/serial/requesting-surface so the compositor can apply
    policy. (wlroots tracks token issuance/validation internally; the compositor
    receives only validated activation requests.)
  - *compositor:* implement the focus-steal policy (Decision 5): if the token
    derives from a recent user interaction on a currently-focused client, raise +
    focus the target through the existing focus path
    (`WindowManager::focus`); otherwise mark the target *urgent* and emit it via
    the existing `WindowUpdated` D-Bus signal (add an `attention`/`urgent` bit to
    the window state the taskbar/pager already read). Reuses the WM core; no new
    focus machinery.
  - *nature:* handler; touches focus + `org.icedtea.WM` state (additive field).

- **gamma-control (`zwlr_gamma_control_manager_v1`).**
  - *wlr crate:* `Runtime::create_gamma_control_manager(&display)` →
    `wlr_gamma_control_manager_v1_create(display)`, plus a listener on
    `set_gamma` that fans out to `GammaHandler::set_gamma(output, ramp)` and a
    `Runtime`/`Output` helper wrapping
    `wlr_gamma_control_v1_apply(...)` +
    `wlr_output_state_set_gamma_lut(state, size, r, g, b)` +
    commit, or (simpler and what wlroots recommends) a helper that, on
    `set_gamma`, pulls the pending ramp with `wlr_gamma_control_v1_apply` into
    the next output commit. The crate must also expose the per-output gamma
    **size** (`wlr_output_get_gamma_size`) so a client can size its ramp, and
    surface the "gamma failed" path so the crate can send
    `wlr_gamma_control_v1_send_failed` when a commit rejects the LUT.
  - *compositor:* on `set_gamma`, stash the ramp for the named output and apply
    it in that output's next `commit` (the output frame/commit path already
    exists for scale/mode/transform at `state.rs:2581–2607`). On commit failure,
    signal failed. Clear the ramp when the controlling client goes away
    (wlroots tracks the controller; the crate forwards the destroy).
  - *nature:* handler; touches the output commit path. No scene change.

### Batch 3 — foreign-toplevel-management (wlr 0.20.25)

- **wlr-foreign-toplevel-management (`zwlr_foreign_toplevel_manager_v1`).**
  - *wlr crate:* the richest addition —
    - `Runtime::create_foreign_toplevel_manager(&display)` →
      `wlr_foreign_toplevel_manager_v1_create(display)`, cell + guard.
    - A per-window **handle** type wrapping `wlr_foreign_toplevel_handle_v1`:
      `create(manager) -> handle`, and setters mirroring the wlroots API —
      `set_title`, `set_app_id`, `set_maximized/minimized/activated/fullscreen`,
      `output_enter/output_leave`, `set_parent`, `done` (coalesce), and `closed`
      (on unmap/destroy). These are thin `wlr_foreign_toplevel_handle_v1_*`
      wrappers.
    - A listener per handle fanning the inbound requests
      (`request_activate`, `request_close`, `request_maximize`,
      `request_minimize`, `request_fullscreen`, `set_rectangle`) out to a
      `ForeignToplevelHandler` trait keyed by the compositor's window id.
    This is additive but sizable; it is why Batch 3 is its own publish.
  - *compositor (`state.rs`/`window.rs`):* give each managed window an optional
    `foreign_handle` alongside its existing bookkeeping. Create it when the
    window maps (beside the point `WindowManager` first tracks a window), destroy
    it on unmap/close. Drive the setters from the **same** places that already
    emit `WindowUpdated`: title/app-id changes, maximize/minimize/fullscreen/
    activate transitions, and output migration (the hot-unplug migration path
    already exists). Route inbound requests into the existing internal ops the
    `org.icedtea.WM` D-Bus methods call — `focus`/`close`/`min`/`max`/
    `fullscreen` — so a `wlr`-pager and the D-Bus surface converge on one WM
    core. `set_rectangle` (the taskbar's icon rect, for minimize animations)
    is stored per-handle for later use and is otherwise a no-op now.
  - *nature:* handler + **per-window handle lifecycle** — the only protocol here
    with state mirrored onto every window. No scaling/render change.

## Verification — what to confirm before writing code (per the roadmap ask)

The roadmap explicitly flags "confirm `xdg-output`/`presentation-time` coverage
from the wlr core." Do these checks first; each is a `grep` in the checked-out
`wlroots-sys` + `wlr` source and a boot-time global dump:

1. **xdg-output (`zxdg_output_manager_v1`).** This is *distinct* from the
   already-present wlr-output-**management** (`zwlr_output_manager_v1`,
   `create_output_manager`, `lib.rs:136`) — xdg-output is the read-only
   per-output logical-geometry protocol many panels/`wlr-randr`-style tools and
   toolkits read. Confirm whether `init_graphics` (`lib.rs:62`, "the renderer and
   the **core protocol globals**") already creates
   `wlr_xdg_output_manager_v1` against the output layout. If **not** present,
   add `Runtime::create_xdg_output_manager(&display)` →
   `wlr_xdg_output_manager_v1_create(display, output_layout)` to **Batch 1** (it
   needs the scene's output layout; pure-additive, no handler). If already
   present, note it and drop it. *Check:* `grep -ri xdg_output` in `wlroots-sys`
   and `wlr`, and dump `advertised_globals` in the harness.

2. **presentation-time.** Confirm `wlr_presentation` is **not** already created
   by `init_graphics`. Some compositors create it there; icedtea's boot list does
   not mention it, so it is presumed absent — but verify via `grep -ri
   presentation` and the global dump before adding it. If absent, Batch 1 adds it
   (Decision above). If present, only `wlr_scene_set_presentation` wiring may be
   missing — add just that.

3. **fractional-scale auto-send.** Confirm whether the `wlr_scene` version in
   `wlroots-sys` calls `wlr_fractional_scale_v1_notify_scale` internally on
   surface output-enter. Determines whether Batch 1 needs the notify wrapper
   (fallback) or nothing beyond global creation.

4. **cursor-shape ↔ existing cursor.** Confirm how `input.rs` currently sets the
   pointer image (xcursor theme handle, cursor manager) so the cursor-shape
   handler reuses it rather than introducing a parallel cursor.

5. **Global-advertisement harness hook.** The harness already asserts on
   `advertised_globals` (screencopy spec, criterion 1). Each protocol below gets
   the same "global is advertised" assertion; confirm the harness registry can
   bind each new global from `wayland-protocols`/`-wlr`.

## Decomposition into milestones (by publish batch)

Each milestone = one additive `wlr` `0.20.x` publish + the compositor bump that
consumes it, following the split-phase publish discipline M4.3–M4.5 used
(freeze → API review verdict held → `cargo publish` → compositor consumes),
with a merge window presented per branch — never auto-merged.

- **M-A2.0 — Verification spike (no publish).** Run the five Verification checks;
  record which of xdg-output / presentation-time / fractional-scale-notify the
  core already provides. Output: the exact global list Batch 1 must add. Small,
  read-only; may be folded into M-A2.1's first task.

- **M-A2.1 — Batch 1, wlr 0.20.23 (passive/scene-internal).** viewporter,
  fractional-scale (+ notify wrapper iff the spike says so), single-pixel-buffer,
  content-type, presentation-time (+ `set_scene_presentation`), and xdg-output
  **iff** the spike found it absent. All create-global (+ one presentation wiring
  line). Compositor: boot lines in `lib.rs`; no `state.rs` logic beyond the
  presentation wiring. Highest leverage / lowest risk; ship first. Harness:
  bind each global, assert advertised; a fractional-scale client asserts it
  receives a `preferred_scale` event equal to the output scale; a viewporter
  client maps a viewport-cropped buffer and screencopy (already present)
  confirms the cropped region renders; a presentation-feedback client asserts it
  gets `presented`/`discarded`.

- **M-A2.2 — Batch 2, wlr 0.20.24 (request handlers).** cursor-shape,
  xdg-activation, gamma-control. wlr crate exposes the three request events +
  the small helpers (xcursor-name, gamma size/apply). Compositor implements the
  three handlers (`input.rs` cursor set; focus/urgent policy + an additive
  `attention` bit on WM window state; per-output gamma ramp applied on commit).
  Harness: a virtual client sets a shape and asserts the seat cursor changed; an
  activation client requests activation with/without a fresh serial and asserts
  focus-change vs. urgent-flag; a gamma client sets a ramp and asserts the output
  commit carried a gamma LUT (or the failed path fires).

- **M-A2.3 — Batch 3, wlr 0.20.25 (foreign-toplevel-management).** The handle
  type + per-handle request listener in the crate; per-window handle lifecycle
  and request routing in `state.rs`/`window.rs`. Harness: a foreign-toplevel
  client enumerates mapped windows and sees title/app-id/state; it drives
  `activate`/`close`/`maximize`/`minimize`/`fullscreen` and asserts the window
  changed identically to the matching `org.icedtea.WM` call; it observes
  `closed` on unmap. This is the milestone that lets a stock pager/dock — and
  icedtea's own taskbar — drive windows.

## Testing

Follow the harness-client + `compositor/tests/client_protocol.rs` pattern the
protocol milestones established (screencopy/pointer-constraints), automated-only:

1. **Global advertised** for all eight (nine with xdg-output) —
   `advertised_globals` assertions, one per protocol.
2. **fractional-scale + viewporter (load-bearing):** with an output at a
   fractional scale (`1.5`, already legal per `guarded_scale`), a client binding
   `wp_fractional_scale_v1` receives `preferred_scale = 1.5×120 = 180`; a
   viewporter client with a source-crop/dest-size viewport, captured via the
   existing screencopy client, shows the cropped-and-scaled region — proving the
   scene honors the viewport, not just that the global exists.
3. **presentation-time:** a client committing a frame receives a `presented`
   (or `discarded`) feedback event after an output commit.
4. **cursor-shape:** a client with pointer focus calls `set_shape`; assert the
   seat's active cursor became the mapped named cursor, and reverts when focus
   leaves (guards the background-client case).
5. **xdg-activation (both branches):** activation carrying a fresh
   interaction serial ⇒ target gains focus; a stale/token-less activation ⇒
   target does **not** steal focus but is flagged urgent (assert via the WM state
   the test reads). Both non-vacuous — the two inputs produce different outcomes.
6. **gamma-control:** a client sets a ramp of the output's advertised gamma size;
   assert the output's next commit applied a gamma LUT (and that a wrong-size
   ramp triggers `failed`).
7. **foreign-toplevel:** enumerate ⇒ correct title/app-id/state; each inbound
   request ⇒ the window transitions identically to the corresponding
   `org.icedtea.WM` method (assert against `get_state`); unmap ⇒ `closed`.
8. **Gates** (both repos, on the freeze commit before each publish):
   `cargo test --workspace` + `cargo clippy --all-targets -- -D warnings`
   (icedtea); `cargo test -p wlr` + clippy + `RUSTDOCFLAGS="-D warnings" cargo
   doc -p wlr --no-deps` + `cargo fmt --all --check` (wlroots-sys — the PR CI
   gate the port PRs enforce). Unit tests in the crate are deletion/mutation
   verified (double-create guard returns the `Error::Operation` on the second
   call), matching every prior `create_*_manager` addition.

## Risks

- **fractional-scale is the one with real render consequences.** If the scene
  does *not* auto-send preferred scale, the compositor must send it on the
  correct output-enter/leave and migration edges (the hot-unplug migration path
  already exists and is a known-tricky area). Mitigation: the Verification spike
  settles auto-send vs. manual before any code; the load-bearing test (2) fails
  loudly if a client gets the wrong scale.
- **cursor-shape ↔ existing cursor path.** Introducing a second cursor image
  source risks flicker/stale cursors on focus change. Mitigation: route through
  the *existing* `input.rs` cursor and revert on focus-leave; the test asserts
  the revert.
- **xdg-activation policy is a judgment call.** Too permissive re-introduces
  focus-steal (the very thing the protocol prevents); too strict annoys
  legitimate launchers. Mitigation: key strictly on wlroots-validated token
  seat/serial recency, default to *urgent-not-focus*, and expose the urgent bit
  so the taskbar can show it — behavior is testable both ways (test 5).
- **gamma-control touches the output commit.** A bad LUT or a commit rejection
  must not wedge the output. Mitigation: apply in the normal commit path used by
  scale/mode/transform, send `failed` on rejection, and drop the ramp when the
  controller disconnects (wlroots forwards the destroy).
- **foreign-toplevel handle lifecycle must exactly track windows.** A leaked or
  stale handle shows a ghost window in every dock/pager. Mitigation: create/
  destroy the handle at the *same* points `WindowManager` starts/stops tracking a
  window, drive setters from the existing `WindowUpdated` emission sites (single
  source of truth), and test enumerate-then-unmap-then-`closed`.
- **Publish cadence collides with parallel sessions.** A1 (XWayland) and M5
  (scene) share `wloots-sys`. Mitigation: coordinate via `SendMessage` as the
  M4.4/M4.5 sessions did; each of the three batches is an independent additive
  `0.20.x`, so they can interleave with the other sessions' publishes as long as
  version numbers are claimed in order.
- **Batch-1/Batch-2 merge choice.** Collapsing them to one publish saves a
  hard-stop but mixes passive globals with handler logic in one review. Left to
  the user at plan handoff; the default is separate (safer review, ship the
  passive half first).

## Out of scope (explicit)

- **XWayland** (roadmap A1) — its own large wlr-crate lift and spec.
- The **UI consumers**: taskbar/pager rendering, the night-light day/night
  scheduler + Settings page (`nightlight` design), the urgent/attention
  *indicator* in the bar, and any cursor-theme picker — all Track B / cross-cutting.
- **tablet-v2** and **text-input-v3 / input-method-v2** — roadmap A6, a separate
  batch.
- The **`ext-*` successors** (`ext-foreign-toplevel-list-v1`,
  `ext-image-capture-source`) — the `zwlr-` variants are what current docks/
  pagers and tooling bind; `ext-` equivalents are a later milestone only if
  newer consumers require them (mirrors the screencopy spec's `ext-image-copy`
  deferral).
- **content-type-driven direct scanout / tearing-control** — content-type is
  exposed as metadata now; acting on it (scanout/tearing) is a later perf task.
- **dmabuf/zero-copy** paths — orthogonal renderer work.
- **A night-light scheduler daemon** — gamma-control only exposes the protocol;
  an external tool (`wlsunset`) or a future icedtea daemon drives it.

## Execution

Three `wlr` releases (0.20.23 → 0.20.25), one per batch, each on a `wlr-a2.N`
branch off `develop` (icedtea) and a `feature/wlr-a2.N` branch off `develop`
(wlroots-sys, up as a PR with the test/miri/msrv/fmt CI the port PRs run).
Split-phase publish discipline (review verdict held before `cargo publish`);
per-task review; final whole-branch review; merge window presented for the
user's go/no-go — never auto-merged. Coordinate version-number claims with the
parallel A1/M5 wlroots-sys sessions via `SendMessage`.
