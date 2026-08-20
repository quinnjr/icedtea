# icedtea-wm — XWayland (Track A, item A1) — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Roadmap slot:** Track A, item **A1** (`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`), flagged "early / biggest lift".
**Branches:** `wlr` additions in `wlroots-sys` (`develop` → PR, new `0.20.x` publish); compositor consumption on `icedtea-wm` `develop`.

## Goal

Run X11 applications on icedtea by wrapping wlroots' XWayland support in the
`wlr` crate and integrating it into the compositor's existing window model, so
X11 clients get **first-class parity** with native Wayland toplevels: server-side
decorations, focus and stacking, interactive move/resize, workspaces, snap/
maximize/fullscreen/minimize, correct clipboard/primary/DND across the X11↔
Wayland boundary, and correct HiDPI scaling — with override-redirect surfaces
(menus, tooltips, drag icons) handled as unmanaged pop-ups. Without this a large
share of real software (Electron-with-X11-fallback, Steam and its games, JetBrains
IDEs, GIMP, many Qt/Java/Tk apps) will not run, which caps how daily-driver
icedtea can be.

Because the lift is large and the `wlr`-crate cost is the dominant unknown, the
first milestone is a **spike** that proves the wrap end-to-end with a single X11
window, before any widening work is committed.

## Key finding — the sys layer is already bound

`wlr-sys` (`crates/wlr-sys`) **already binds the XWayland C API**. `build.rs`'s
`SUBSYSTEMS` table carries an `xwayland` subsystem (feature `xwayland`, `pc_var`
`have_xwayland`, `cfg` `wlr_has_xwayland`, `extra_pc: ["xcb"]`) whose headers are
`wlr/xwayland.h`, `wlr/xwayland/server.h`, `wlr/xwayland/shell.h`,
`wlr/xwayland/xwayland.h` — and `xwayland` is in the crate's **default** feature
set. `crates/wlr/src/coverage.rs` already groups `wlr_xwayland` and `wlr_xwm`
symbols into an `"xwayland"` burn-down area that is currently **unwrapped**
(waived, no safe surface). So A1 is **not** a bindgen/FFI task: the raw
`sys::wlr_xwayland_*` symbols exist. The work is (1) the safe `wlr`-crate wrapper
over them, and (2) compositor integration. This materially lowers the sys-side
risk versus a from-scratch protocol addition (contrast M4.3/M4.4, which added new
`pc_var`/headers). It does add a **build/runtime dependency on the `Xwayland`
binary + `xcb`** at the packaging layer.

## Decisions of record

1. **Wrap the modern split API, `lazy` by default.** Use
   `wlr_xwayland_create(display, compositor, lazy=true)` (the convenience
   constructor that internally owns a `wlr_xwayland_server` + `wlr_xwayland_shell_v1`
   + the `xwm`). Lazy start means the `Xwayland` process is only spawned when the
   first X11 client connects to the advertised `DISPLAY`, keeping cold sessions
   cheap. The lower-level `wlr_xwayland_server` split is **not** exposed in the
   first pass (see Out of scope).
2. **X11 surfaces reuse the existing window model, they do not fork it.** A
   managed (non-override-redirect) `wlr_xwayland_surface` maps to the same
   `window::Window` row and the same `wayland::ToplevelKey`→`WindowId` binding
   machinery, so SSD, snap/maximize/fullscreen/minimize, workspaces, focus-MRU,
   alt-tab, and interactive move/resize all work through one code path. The
   decoration answer reuses `decoration::has_ssd`/`content_rect` keyed on the X11
   `class`/`instance` (mapped to `app_id`).
3. **The safe crate owns the XWayland state machine; the compositor sees handler
   callbacks and a small surface accessor set** — mirroring how `ToplevelHandler`
   already abstracts xdg-shell. The compositor never touches `xcb` or `xwm`.
4. **override-redirect (OR) surfaces are unmanaged pop-ups.** They bypass the WM:
   positioned at their client-requested absolute coordinates, no SSD, stacked
   above managed windows, never entered into the `Window` model, never focus
   candidates for alt-tab. Keyboard focus follows X11 semantics (an OR surface may
   still take keyboard focus for e.g. a combo-box list). This is the classic
   wlroots split and the single most bug-prone area.
5. **Clipboard/primary/DND come for free from `wlr_xwm` once the seat is set** —
   they are **not** new compositor code. wlroots' `xwm` bridges the X11 CLIPBOARD/
   PRIMARY selections and XDND to the Wayland `wlr_seat` selections the M4.1
   selection stack + `data-control`/`primary-selection` already drive. The crate
   must call `wlr_xwayland_set_seat(xwayland, seat)` after `create_seat`; the
   compositor's job is limited to wiring + verification.
6. **Split-phase publish → consume, as in M1–M4.6.** One additive `wlr` `0.20.x`
   release (**0.20.23**), API-review verdict held before the mechanical
   `cargo publish`; the compositor then bumps and consumes. Additive-only within
   `0.20.x`. Development uses a temporary `[patch.crates-io]` resolving to the
   checked-out `wlroots-sys` branch, swapped to the published pin at the end.
   **Coordinate with any parallel `wlroots-sys` sessions** (the user runs parallel
   sessions sharing that repo) before the version bump and PR.
7. **Execution: SDD** (`superpowers:subagent-driven-development`). Per-task review,
   final whole-branch review, **merge window presented — never auto-merged**.
   wlroots-sys integrates via a PR into `develop`.
8. **First milestone is a spike.** M1 proves: crate wrapper compiles + publishes-
   shape, `Xwayland` starts lazily, a single managed X11 window maps into the
   `Window` model and renders/focuses. Only on a green spike do M2–M5 widen.

## Architecture

### wlroots XWayland object model (what the crate wraps)

- **`wlr_xwayland`** — the manager. Events: `ready` (the X server is up and the
  `xwm` is running — the earliest point `set_seat` and cursor setup are valid),
  `new_surface` (a new `wlr_xwayland_surface` appeared), `remove` (teardown).
  Helpers: `wlr_xwayland_set_seat`, `wlr_xwayland_set_cursor`,
  `wlr_xwayland_destroy`. Field `display_name` (e.g. `:1`) — the value to export
  as `DISPLAY`.
- **`wlr_xwayland_surface`** — one X11 window. Crucially, an X11 window **exists
  before it has a `wlr_surface`** (X11 creates the window, then later a client
  attaches content), so the lifecycle is two-phase:
  - `associate` — a `wlr_surface` is now attached (build the scene node here).
  - `unassociate` — the `wlr_surface` went away (tear the scene node down; the
    `wlr_xwayland_surface` may live on and re-associate).
  - `map` / `unmap` — the associated surface gained/lost a committed buffer
    (create/hide the `Window` row here, exactly like xdg `mapped`/`unmapped`).
  - `destroy` — the X11 window is gone.
  - Request events: `request_configure` (x/y/w/h — X11 clients self-position),
    `request_move`, `request_resize` (with `edges`), `request_maximize`,
    `request_fullscreen`, `request_minimize`, `request_activate`.
  - Property events: `set_title`, `set_class` (WM_CLASS: instance + class),
    `set_role`, `set_window_type` (`_NET_WM_WINDOW_TYPE_*`), `set_parent`,
    `set_pid`, `set_hints` (urgency), `set_override_redirect`,
    `set_geometry`.
  - Fields: `override_redirect: bool`, `mapped`, `x/y/width/height`, `title`,
    `class`, `instance`, `role`, `window_type` (atom list), `parent`, `pid`,
    `modal`, `size_hints`, `hints`, the backing `surface: *wlr_surface`.
  - Helpers: `wlr_xwayland_surface_configure(xs, x, y, w, h)`,
    `wlr_xwayland_surface_activate(xs, bool)` (X11 focus in/out),
    `wlr_xwayland_surface_restack(xs, sibling, mode)`,
    `wlr_xwayland_surface_set_maximized/_fullscreen/_minimized`,
    `wlr_xwayland_surface_close(xs)`,
    `wlr_xwayland_surface_from_wlr_surface(surface)` (surface → xsurface, for the
    scene/seat path).

### `wlr`-crate API surface to add (one additive 0.20.23 release)

A new module `crates/wlr/src/xwayland.rs` plus additive `Runtime` methods and a
new handler trait, following the shape of `toplevel.rs`/`ToplevelHandler`/`layer.rs`:

- **Types**
  - `XwaylandSurfaceId(u64)` — stable id in the crate's id scheme (mirrors
    `ToplevelId`/`LayerSurfaceId`; `dangling_for_test`/`dangling_nth_for_test` for
    unit tests), so the compositor keys on an opaque id, not a raw pointer.
  - `XwaylandSurface<'h>` — a borrowed accessor, mirroring `Toplevel<'h>`:
    `id()`, `title() -> Option<String>`, `class() -> Option<String>`,
    `instance() -> Option<String>`, `role() -> Option<String>`, `pid()`,
    `geometry() -> Rectangle` (client-requested x/y/w/h),
    `override_redirect() -> bool`, `is_modal() -> bool`,
    `window_type() -> XwaylandWindowType` (a wrapped enum over the common atoms:
    Normal, Dialog, Menu, Tooltip, Utility, Splash, DropdownMenu, PopupMenu,
    Combo, Dnd, Notification, Desktop, Dock — the ones that drive placement/SSD),
    `parent() -> Option<XwaylandSurfaceId>`, `wants_decorations() -> bool`
    (from `_MOTIF_WM_HINTS`/`_GTK` where wlroots surfaces it, else true).
  - `XwaylandWindowType` enum + `Edges` reuse (already in `toplevel.rs`).
- **`Runtime` methods**
  - `create_xwayland(&self, display: &Display, compositor, lazy: bool) -> Result<()>`
    — creates `wlr_xwayland`, links `ready`/`new_surface`/`remove` and, per
    surface, the full listener set above; double-create guarded like the other
    `create_*`; manager is display/runtime-owned (no `Drop`, torn down with the
    display, matching the session-lock/idle managers).
  - `xwayland_display_name(&self) -> Option<String>` — the `:N` to export.
  - `set_xwayland_seat(&self)` — call `wlr_xwayland_set_seat` with the runtime's
    own seat; invoked internally on `ready` (and re-entrant-safe), plus exposed
    so the compositor can re-assert after a seat change.
  - `configure_xwayland_surface(&self, id, Rectangle)` — position + size an X11
    window (SSD content rect for managed; raw geometry for OR).
  - `activate_xwayland_surface(&self, id, bool)` — X11 focus in/out; paired with
    the seat keyboard-focus path.
  - `restack_xwayland_surface(&self, id, RestackMode)` — top/bottom, for stacking
    parity when a managed X11 window is raised.
  - `set_xwayland_surface_maximized/_fullscreen/_minimized(&self, id, bool)` —
    reflect model state back to the client's `_NET_WM_STATE`.
  - `close_xwayland_surface(&self, id)` — `WM_DELETE_WINDOW`/kill, backing the
    existing `request_close` path.
  - Scene: on `associate`, build the surface's scene node with
    `wlr_scene_subsurface_tree_create(band_tree, xsurface->surface)` — the same
    primitive M4.4's lock surfaces use (`wlr_scene_subsurface_tree_create` in
    `backend.rs`), since there is **no** `wlr_scene_xdg_surface_create` analogue
    for X11. Managed surfaces parent into a toplevel-owned tree (so SSD rects and
    z-order ride with the window, like `add_buffer_in_toplevel`); OR surfaces
    parent into a band **above** `Band::Toplevel` (a dedicated unmanaged/OR tree,
    or `Band::Top`) so pop-ups float over managed content but below `Band::Lock`/
    `Band::Overlay`.
- **`XwaylandHandler` trait** (new, all methods defaulted; folded into the
  `Handlers` supertrait set in `handler.rs` exactly as `SessionLockHandler` was —
  a truly additive change, gated by the `every_handler_method_is_defaulted` /
  blanket-impl tests already in `handler.rs`):
  - `xwayland_ready(&mut self, display_name: &str)` — export `DISPLAY`, set seat/
    cursor (crate does the seat/cursor itself; the compositor uses this to publish
    the env for children).
  - `new_xwayland_surface(&mut self, surface: &XwaylandSurface<'_>)` — analogous to
    `new_toplevel`; nothing modelled yet.
  - `xwayland_surface_associate` / `xwayland_surface_unassociate(&mut self, id)`.
  - `xwayland_surface_mapped(&mut self, surface: &XwaylandSurface<'_>)` /
    `xwayland_surface_unmapped(&mut self, id)`.
  - `xwayland_surface_destroyed(&mut self, id)`.
  - `xwayland_title_changed` / `xwayland_class_changed(&mut self, surface)`.
  - `xwayland_request_configure(&mut self, id, geometry: Rectangle)`.
  - `xwayland_request_move(&mut self, id)`.
  - `xwayland_request_resize(&mut self, id, edges: Edges)`.
  - `xwayland_request_maximize/_fullscreen/_minimize(&mut self, id, bool)`.
  - `xwayland_request_activate(&mut self, id)` — focus-steal request (ties to the
    future `xdg-activation` A2 policy; for now honor-or-mark-urgent).
  - `xwayland_override_redirect_changed(&mut self, id, override_redirect: bool)` —
    a surface can flip OR at runtime; the compositor must move it between the
    managed model and the unmanaged pop-up path.
- **Coverage:** the `wlr_xwayland`/`wlr_xwm` ledger rows in `coverage.rs` move
  from waived → wrapped for the symbols the wrapper reaches; the coverage audit
  (`crates/wlr/tests/coverage_audit.rs`) enforces no silent drift, and any symbol
  we deliberately do not wrap (e.g. the low-level `wlr_xwayland_server_*` split)
  stays explicitly waived with a reason, per the existing ledger discipline.

### Compositor integration (`icedtea-wm`)

- **Boot** (`compositor/src/lib.rs` and `harness/src/lib.rs`, both boot paths):
  after `create_seat`, add `runtime.create_xwayland(&display, compositor, true)`
  beside the existing `create_*` blocks — **non-fatal** in production (log and
  continue if `Xwayland` is absent, mirroring the screencopy/session-lock error
  handling), `.expect` in the harness only when the harness opts into an X11 test.
- **State** (`compositor/src/state.rs`): implement `XwaylandHandler for State`
  alongside `impl wlr::ToplevelHandler for State`. Reuse:
  - The `wayland` binding maps (`ToplevelKey`↔`WindowId` in `wayland.rs`) — add a
    parallel `XwaylandKey`/`XwaylandSurfaceId` binding, or generalize the existing
    key to a `SurfaceKey { Xdg | X11 }` sum so `window_for`/`bind`/`unbind` and the
    decoration-node map serve both. **Decision:** generalize the key (one focus/
    stacking/SSD path is the whole point of Decision 2); the enum is additive.
  - `new_toplevel(...)` → a shared `add_managed_window(key, app_id, title, pid)`
    that both `mapped` and `xwayland_surface_mapped` call; `app_id` for X11 is
    `class` (falling back to `instance`), which feeds `decoration::has_ssd`.
  - `sync_window_to_scene`, `sync_seat_focus`, `sync_focus_change`, the
    layout/snap/maximize logic, `emit_pending` — all unchanged; they operate on
    `WindowId`.
  - `initial_commit`/`content_rect` SSD sizing: X11 has no `initial_commit`; size
    on `associate`/first `request_configure` via `configure_xwayland_surface`
    with the SSD `content_rect`, and re-configure on move/resize/maximize.
- **Focus/activation:** `sync_seat_focus` additionally calls
  `activate_xwayland_surface(id, focused)` for X11 windows so the client sees X11
  focus-in/out; the crate's seat keyboard-enter still drives the actual key
  routing (one seat, shared with native clients).
- **Move/resize:** `request_move`/`request_resize` reuse the existing interactive
  grab in `input.rs` (`resize_edges_from_wlr` already converts `wlr::Edges`); the
  grab writes geometry back through `configure_xwayland_surface` instead of the
  xdg configure. Client self-positioning (`request_configure`) is honored for OR
  and for managed dialogs' initial placement, then constrained by the WM
  (cascade/output-clamp) exactly as native windows are placed.
- **OR surfaces:** kept out of `WindowManager` entirely; tracked in a small
  side-table keyed by `XwaylandSurfaceId`, positioned from `geometry()`, parented
  above `Band::Toplevel`. `xwayland_override_redirect_changed` migrates a surface
  between the two paths.
- **D-Bus:** X11 windows flow through the existing `org.icedtea.WM` surface
  (`WindowOpened`/`Closed`/`Updated`, focus/close/min/max/fullscreen) with no new
  interface — they are `Window` rows. `app_id` = X11 class, `title` = `_NET_WM_NAME`.

### Clipboard / primary / DND bridge

No new compositor code. `wlr_xwm` bridges automatically once
`wlr_xwayland_set_seat(seat)` is set (done by the crate on `ready`):

- **CLIPBOARD ↔ Wayland selection** and **PRIMARY ↔ primary-selection** are
  synced by `xwm` against the same `wlr_seat` the M4.1 selection stack drives, so
  the clipboard daemon (`org.icedtea.Clipboard`, `wlr-data-control`) sees X11
  copies and X11 apps see Wayland copies, transparently.
- **XDND ↔ `wl_data_device` drag-and-drop** is bridged by `xwm` against the same
  seat the M4.2 DND work drives (drag icon, `drag_icon_position`).
- Compositor work is limited to: ensure `set_xwayland_seat` runs after
  `create_seat` (crate-internal on `ready`, re-asserted if the seat is recreated)
  and **verify** end-to-end (see Testing). MIME-type edge cases (X11 `TARGETS`
  atoms ↔ MIME strings) are wlroots' responsibility; we assert, not reimplement.

### HiDPI / scale

wlroots XWayland is **single-scale**: X11 has no per-window fractional scale, so
wlroots renders X11 surfaces at an integer scale and the compositor is responsible
for choosing it and telling Xwayland via `wlr_xwayland` scale/DPI hints
(`Xwayland -scale` / the `xwm`'s scale support where the wlroots version exposes
it). **Decision for A1:** target the current single-output/global-scale reality —
apply the output's integer scale to X11 surfaces uniformly (scene-node scale +
`_XWAYLAND` scale hint), and set X11 DPI (`Xft.dpi`/`RESOURCE_MANAGER`) to match
so toolkits size fonts correctly. Mixed-DPI multi-monitor for X11 is a known
wlroots limitation (X11 apps blur on the non-primary scale); documented as a
follow-up, not solved here. Fractional-scale/viewporter (A2) is native-only and
does not apply to X11.

### DISPLAY / environment wiring

- On `xwayland_ready(display_name)` the compositor exports `DISPLAY=:N` into its
  own environment **and** into the environment used to spawn session children
  (the same place `WAYLAND_DISPLAY` is set in `lib.rs` — note the existing comment
  there about `set_var` being single-threaded-with-respect-to-the-environment;
  the `DISPLAY` export must obey the same ordering, set before worker threads that
  inherit the env are spawned, or pushed through the child-spawn env explicitly
  rather than the process env if that ordering can't be guaranteed).
- Because start is **lazy**, `DISPLAY` is only valid after `ready`; children
  launched before first X activity get `DISPLAY` once `ready` fires. Document that
  autostart/launcher-spawned processes should read `DISPLAY` from the compositor's
  published environment (D-Bus `org.icedtea.WM` could later expose it; for A1 the
  process-env export suffices since children are forked from the compositor
  session).
- `XCURSOR_*` and `_XWAYLAND` cursor: the crate calls `wlr_xwayland_set_cursor`
  with the runtime's xcursor image on `ready`, so X11 apps get the same pointer.

## Decomposition into milestones

**M1 — SPIKE: wrap + one window end-to-end (the gate).** Additive `wlr` `xwayland`
module with `create_xwayland` (lazy), the `ready`/`new_surface`/`associate`/`map`/
`unmap`/`destroy` listeners, the `XwaylandSurface` accessor (id/title/class/pid/
geometry/override_redirect), the scene-node build on `associate`, and a minimal
`XwaylandHandler`. Compositor: boot `create_xwayland`, implement the handler to
add a **managed** X11 window to the `Window` model on `map` and remove on
`unmap`/`destroy`, export `DISPLAY`, set seat on `ready`. **Proof:** an automated
test spawns `Xwayland` under the harness, connects a trivial xcb client that
creates+maps one top-level window, and asserts (a) `wlr_xwayland` advertised /
`ready` fired, (b) a `Window` row appears with the right class/title, (c) it
renders in the scene and can receive keyboard focus. If the wrap or the headless
`Xwayland` handshake proves disproportionately costly, **stop and re-scope** here
— that is the point of the spike. Publish is **not** done at M1; development stays
on the `[patch]`.

**M2 — Managed-window parity.** SSD via `decoration::has_ssd`/`content_rect`
(title bar, min/max/close buttons hit-testing through the shared path);
interactive + client-initiated move/resize (`request_move`/`request_resize` →
`input.rs` grab → `configure_xwayland_surface`); maximize/fullscreen/minimize
reflected back with `set_xwayland_surface_*`; workspaces, snap, cascade placement,
alt-tab, focus-MRU — all inherited by routing X11 windows through the shared model
path. `set_title`/`set_class` live updates → `WindowUpdate`. Focus activation
(`activate_xwayland_surface`) wired into `sync_seat_focus`. Restack parity on
raise.

**M3 — override-redirect + window-type placement.** OR surfaces as unmanaged
pop-ups (side-table, above-toplevel band, client coords, no SSD, correct keyboard
focus, correct stacking relative to their parent). Runtime OR flips
(`override_redirect_changed`) migrate between paths. `window_type`/`modal`/`parent`
drive placement: dialogs centered/parented, utility/splash undecorated-or-simple,
menus/tooltips as OR. `_NET_WM_STATE` fullscreen/maximize requests honored.

**M4 — Clipboard / primary / DND bridge + HiDPI + env.** Verify (not build)
CLIPBOARD↔selection, PRIMARY↔primary-selection, XDND↔`wl_data_device` end-to-end
against the existing clipboard daemon and DND stack. Apply integer output scale +
DPI hint to X11 surfaces; export `DISPLAY`/cursor robustly. Confirm the clipboard
daemon captures an X11 copy and an X11 app pastes a Wayland copy.

**M5 — Publish + consume + harden.** Freeze the `wlr` branch green (crate tests +
clippy + `RUSTDOCFLAGS="-D warnings" cargo doc` + fmt + coverage audit), hold the
API-review verdict, **present the publish window** (project rule — never auto-
publish), `cargo publish` **0.20.23**, swap the compositor off the `[patch]` to
the published pin, bump and consume, final whole-branch review, **present the
merge window**. wlroots-sys lands via a PR into `develop`.

## Testing

XWayland cannot be exercised by the existing pure-`wayland-client` `TestClient`;
tests need a real `Xwayland` binary and an X11 client. Approach, gated so CI
without `Xwayland` skips cleanly:

- **Harness addition:** an `XwaylandTestClient` that (a) waits for the crate's
  `ready`, reads `DISPLAY` via a new `DbCommand::XwaylandDisplay` reply-channel
  accessor (mirroring `DbCommand::SessionLocked`/`DragIconPosition`), (b) connects
  with a minimal `xcb`/`x11rb` client, and (c) creates/maps/configures/copies from
  a top-level and an OR window. Guard the whole X11 test module behind a runtime
  check for the `Xwayland` binary (`which Xwayland`); absent ⇒ `eprintln!` skip,
  not a failure, so the workspace test gate stays green on machines without it
  (document `Xwayland` as a test dependency).
- **Observability hooks** (`#[doc(hidden)]`/test-only): `DbCommand::XwaylandDisplay`
  (the `:N`), and a way to read whether a given `WindowId` is X11-backed and its
  OR-ness (extend the snapshot or a dedicated command) so assertions are on model
  state, not pixels.

Load-bearing tests, most non-vacuous:

1. **Global + lazy start.** `wlr_xwayland` is created; `ready` fires only after a
   client connects (lazy); `DisplayName` is a valid `:N`.
2. **Managed map round-trip.** An xcb client maps one top-level; assert a `Window`
   row with the right `app_id`(=class)/`title`/`pid`, mapped, rendered, focusable.
   Unmap/destroy removes it and reseats focus (reuse the
   `unmapping_the_focused_window_moves_focus_to_the_next_candidate` pattern).
3. **SSD + move/resize parity.** A decorated X11 window gets an SSD strip;
   injected title-bar drag moves it; a `request_resize` grab resizes it; assert
   the geometry the client is configured to.
4. **Maximize/fullscreen/minimize.** Drive each through `org.icedtea.WM` and
   assert both the model state and that `set_xwayland_surface_*` was applied.
5. **override-redirect pop-up.** An OR window maps at client coords, is **not** in
   `WindowManager`, is **not** an alt-tab candidate, stacks above managed windows,
   and (for a focus-taking OR) receives keyboard input. A runtime OR flip migrates
   correctly.
6. **Clipboard bridge.** X11 client sets CLIPBOARD; assert the clipboard daemon /
   Wayland selection sees it. Wayland client sets a selection; assert the X11
   client reads it. Same shape for PRIMARY.
7. **DND bridge.** An X11→Wayland (and Wayland→X11) drag delivers the payload;
   assert the drop data and that `drag_icon_position` tracks (reuse M4.2 hooks).
8. **HiDPI hint.** With output scale 2, assert the X11 surface is configured/
   scaled at integer 2 and the DPI hint is set.
9. **Crate unit tests.** `XwaylandSurface` accessors over a `dangling_*` id;
   `XwaylandWindowType` atom mapping; the handler blanket-impl/defaulted-method
   tests in `handler.rs` still hold with the new trait folded in.

**Gates:** `cargo test --workspace` + `cargo clippy --all-targets -- -D warnings`
(icedtea); `cargo test -p wlr` + `cargo test -p wlr-sys` (the `xwayland` FFI
interop) + clippy + `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps` +
`cargo fmt --all --check` + the coverage audit (`coverage_audit.rs`) — green on
the freeze commit before publish.

## Risks

- **Biggest single lift on the roadmap; high uncertainty.** Mitigation: the M1
  spike is an explicit stop-and-rescope gate; nothing publishes until the wrap is
  proven end-to-end.
- **override-redirect handling is the classic footgun** (focus, stacking, runtime
  OR flips, menus that never unmap cleanly). Mitigation: M3 is its own milestone
  with dedicated tests; keep OR entirely out of the managed model.
- **The two-phase X11 lifecycle** (surface exists before `wlr_surface`;
  associate/unassociate distinct from map/unmap; a window can re-associate) is
  subtly different from xdg-shell and easy to get wrong (leaked scene nodes, stale
  `Window` rows). Mitigation: model the crate state machine explicitly, one
  ownership point; test associate-without-map and re-associate.
- **Headless test harness for X11.** Driving `Xwayland` + an xcb client under the
  existing headless harness is new territory; the handshake may be finicky and the
  `Xwayland` binary may be absent in CI. Mitigation: skip-if-absent guard;
  `x11rb`/`xcb` minimal client; assert on model state via `DbCommand`, not pixels.
- **Clipboard/DND is "free" only if the seat wiring is exactly right.** A missed
  `set_seat` (or a seat recreated after `ready`) silently breaks the bridge.
  Mitigation: crate asserts seat on `ready` and exposes `set_xwayland_seat` for
  re-assertion; end-to-end clipboard/DND tests (6,7) catch regressions.
- **HiDPI is fundamentally limited for X11** (single global integer scale; mixed-
  DPI multi-monitor blurs). Accepted; scoped to global integer scale + DPI hint,
  with mixed-DPI as documented follow-up.
- **Focus-steal / activation** (`request_activate`, `_NET_ACTIVE_WINDOW`) needs a
  policy; without one, X11 apps can grab focus. Mitigation: honor-or-mark-urgent
  now; a real policy ties into `xdg-activation` (A2), noted as the integration
  point, not solved here.
- **`Xwayland` becomes a hard runtime dependency** for X11 support (plus `xcb` at
  build). Mitigation: `create_xwayland` is non-fatal; a session without `Xwayland`
  runs Wayland-only, exactly as today.
- **Publish coordination.** The user runs parallel `wlroots-sys` sessions sharing
  the repo. Mitigation: coordinate via `SendMessage` before the `0.20.23` bump and
  PR; `[patch]` resolves to the checked-out branch during development (per the
  established M4/M5 discipline).

## Effort / risk read (honest)

The **largest** Track-A sub-project and the highest-uncertainty one, but with one
big de-risker already in place: **the sys/FFI layer is already bound and default-
on** (`wlr-sys` `xwayland` subsystem), so this is a *safe-wrapper + integration*
job, not a bindgen expedition. Rough shape, in the units of the shipped M4.x
milestones (each a multi-day milestone):

- M1 spike: comparable to a normal wlr-wrap milestone, front-loaded with the
  X11-harness unknown — **the milestone most likely to surprise**.
- M2 parity: moderate — mostly *reuse* of the existing model, focus, decoration,
  and grab code through a shared path; the risk is the plumbing generalization
  (`SurfaceKey` sum), not new algorithms.
- M3 OR: small-but-fiddly; bug-density high, code volume low.
- M4 clipboard/DND/HiDPI: mostly **verification** (wlroots does the bridge), plus
  the scale/DPI wiring; low code, real test effort.
- M5 publish/consume/harden: mechanical, gated by the two consent windows.

Net: plan for it to run **longer than a typical M4.x milestone** and to spend its
uncertainty budget in M1 (harness) and M3 (OR). It gates a large, concrete slice
of real-app compatibility, so the spike-first structure buys the option to
re-scope cheaply if the wrap surprises.

## Out of scope (explicit)

- The low-level `wlr_xwayland_server` / `wlr_xwayland_shell_v1` split constructor
  (we wrap the `wlr_xwayland_create` convenience path); staying explicitly waived
  in the coverage ledger.
- Rootful Xwayland / a separate X screen — only rootless per-window integration.
- Mixed-DPI fractional scaling for X11 (wlroots limitation; global integer scale
  only). Native fractional-scale/viewporter is A2 and native-only.
- A real focus-steal-prevention policy — deferred to `xdg-activation` (A2); A1
  only honors-or-marks-urgent.
- X11 window-manager niceties beyond parity with our native WM: no `_NET_WM`
  pager/desktop hints beyond what maps to our 4 global workspaces, no X11-specific
  keybinding/hotkey grabs, no legacy ICCCM corner cases beyond what wlroots' `xwm`
  handles.
- Primary-selection/DND *semantics* beyond wlroots' bridge — we verify, we do not
  reimplement MIME/atom translation.
- The pure-Rust-toolkit shell surfaces (Track B) — unrelated; X11 apps render
  through the compositor's existing scene path.

## Next step

On approval, turn **M1 (the spike)** into an implementation plan
(`superpowers:writing-plans`): the additive `wlr` `xwayland` module + `create_xwayland`
+ the map-one-window handler + the headless `Xwayland` harness test — the concrete,
gated first deliverable that proves the wrap before any widening is committed.
