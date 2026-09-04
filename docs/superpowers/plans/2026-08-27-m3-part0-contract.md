# M3 Part 0 — Shared Interface Contract

**Date:** 2026-08-27
**Spec:** `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
**Previous contract:** `docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§12 E1–E15 are still binding)
**Branch:** `rebuild/pure-rust-gtk-m3` (off `develop` @ `8df998e`) · **Crates:** `ui/` (`icedtea-ui`), `compositor/`, `harness/`, `wlr` (wloots-sys `develop` @ `d072c96` = 0.20.27 → 0.20.28)
**Research notes:** `.superpowers/m3-plan-notes/{wlr-popups,compositor-harness,current-ui-crate,gtk-widget-nodes,icons-svg,xkb-wayland-client}.md`

This is the frozen interface eight independently-authored part-plans code against.
Every signature below is normative: a part may add private items freely, but may
not change a signature here without a contract amendment recorded in §10.
One-line semantics follow each item.

Rules carried forward from M2 and still binding: nothing outside
`css::registry` names a CSS property string; `Node`/`PseudoStates` from
`css::node` are the only node/state types; `Button::render`'s M2-final
signature (E5/E6) is the paint template, not the pre-ruling one; property
counts are 114/95/19 (E14); `anim::keyframes::resolve_segment`, not
`Keyframes::segment` (E9); `Stylesheet::append_layer` stays live (E8).

**Crate deps to add (P3 owns the `ui/Cargo.toml` edit; P1 owns `wlr`'s version bump):**

```toml
# ui/Cargo.toml
xkbcommon = "0.9"                                     # links libxkbcommon; default feature "wayland" required (Keymap::new_from_fd)
wayland-protocols = { version = "0.32", features = ["client", "staging", "unstable"] }
memmap2 = "0.9"                                       # only if P3 needs its own mapping; xkbcommon's fd path already mmaps
```

`client` gives `xdg::shell::client` (xdg-shell); `staging` gives
`wp::cursor_shape::v1::client`; `unstable` gives
`wp::primary_selection::zv1::client`. `wayland-client` 0.31,
`wayland-protocols-wlr` 0.3, `skia-rs-safe` 0.4.0 (features
`std,text,codec,codec-png,svg` — already enabled), `taffy` 0.14, `cssparser`
0.37, `selectors` 0.40, `fontconfig` 0.11 (optional) are unchanged.

---

## 0. Module map (owner in brackets)

```
wloots-sys/crates/wlr/src/
  popup.rs          [P1]  PopupId, Popup<'h>, PopupParent, PositionerRules
  backend.rs        [P1]  on_new_popup/_commit/_map/_unmap/_destroy/_reposition, link_popup, Bound.popup
  runtime.rs        [P1]  PopupEntry, record/forget/popup_entry/drop_all_popups, Runtime popup methods
  handler.rs        [P1]  six defaulted ToplevelHandler methods
  dispatch.rs       [P1]  six Event variants
  lib.rs, coverage/{wrapped,waived}.toml, README.md   [P1]

compositor/src/
  state.rs          [P2]  PopupEntry, popup registry, placement, z-order, focus stack
  wayland.rs        [P2]  PopupKey newtype (wlr::PopupId wrapper), handler impls
compositor/tests/popups.rs  [P2]
harness/src/lib.rs  [P2]  PopupSpec, TestClient popup API, LayerPanelClient::open_popup

ui/src/
  window/mod.rs     [P3]  Surface, Window, InputEvent, SurfaceError, SurfaceStates
  window/toplevel.rs[P3]  Toplevel (xdg_toplevel)
  window/layer.rs   [P3]  Layer (zwlr_layer_surface_v1) + LayerWindow compat shim
  window/popup.rs   [P3]  Popup (xdg_popup + xdg_positioner), PopupKey, Positioner
  window/keyboard.rs[P3]  Keymap, KeyEvent, Mods, Repeat
  window/pointer.rs [P3]  hit_test, Hit, ImplicitGrab, Scroll, Kinetic, CursorShape mapping
  window/focus.rs   [P3]  FocusRing, FocusDirection, navigate, focus_sort
  window/selection.rs[P3] Clipboard (wl_data_device + primary)
  view/mod.rs       [P4]  View, Kind, Key, Props, Prop, PropName, Handlers, EventKind
  view/builders.rs  [P4]  free-function builders (P4 ships the frame; P5/P6 fill it)
  view/reconcile.rs [P4]  Instance, reconcile, Op, BuildCx
  view/controller.rs[P4]  Controller, EventCx, Event
  view/app.rs       [P4]  App, AppError, Script, Frames
  view/cmd.rs       [P4]  Cmd
  widgets/*.rs      [P5/P6] one file per widget; see §5
  icons/{mod,theme,lookup,render,builtin,symbolic}.rs  [P7]
  bin/gallery.rs    [P8]
  paint/icon.rs     [P7]  paint_icon; the only new file under paint/
  layout.rs         [P6]  Container::Grid/Center + per-child align/expand (see §3.6)
  text.rs           [P5]  TextLayout: wrap/ellipsize/cursor/selection (see §3.7)

ui/tests/
  fixtures/gtk4.22-node-trees/<widget>.txt   [P5/P6]
  fixtures/mini-icon-theme/**                [P7]
  fixtures/keymaps/us.xkb                    [P3]
  node_trees.rs      [P5/P6]  node_tree_of() conformance gate
  gallery_gate.rs    [P8]     rest-state screencopy gate
  interaction_gate.rs[P8]     one interaction per widget class
  support/mod.rs     [P8]     grown, not duplicated
```

**Execution order: P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8.** P7 may start
against P4's `IconRef` resolution point as soon as P4 lands; its gate needs P5.

---

## 1. `wlr` 0.20.28 — xdg-popup [P1]

Repo: `/home/joseph/Projects/wlroots-sys`. **Do not create branches there from
any other session** — P1's executor owns the branch. Crate version
`0.20.27` → `0.20.28` (additive minor-patch; every handler method is
defaulted, so `impl ToplevelHandler for S {}` written against 0.20.27 still
compiles).

### 1.1 `popup.rs` — ids, parents, rules

```rust
/// Identifies one live `wlr_xdg_popup`. Minted on the popup's `wlr_surface`
/// addon set with `ensure_id_raw`, from the same process-wide counter as
/// `ToplevelId`/`LayerSurfaceId`/`NodeId`, so ids never collide across kinds.
/// Deliberately no `PartialOrd`/`Ord` (see `id.rs:20-24`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupId(pub(crate) u64);

/// What a popup hangs off. A layer-shell popup is created with a NULL xdg
/// parent and reparented by `zwlr_layer_surface_v1.get_popup`, so the parent
/// is only knowable from the parent-scoped `new_popup` signal (§1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupParent {
    Toplevel(ToplevelId),
    Layer(LayerSurfaceId),
    Popup(PopupId),
}

impl PopupParent {
    /// The chain root: walks `Popup(_)` links to the `Toplevel`/`Layer` at the
    /// bottom. `None` if a link is already dead.
    pub fn root(self, rt: &Runtime) -> Option<PopupParent>;
    pub fn is_popup(self) -> bool;
}

/// A snapshot of `wlr_xdg_positioner_rules`, copied out (never borrowed) so it
/// survives re-entry into wlroots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionerRules {
    pub anchor_rect: Box2D,
    pub anchor: PositionerAnchor,
    pub gravity: PositionerGravity,
    pub constraint_adjustment: ConstraintAdjustment,
    pub size: (i32, i32),
    /// `Some` only when the client sent `set_parent_size` (protocol v3+).
    pub parent_size: Option<(i32, i32)>,
    pub offset: (i32, i32),
    pub reactive: bool,
    /// `Some` only when the client sent `set_parent_configure`.
    pub parent_configure_serial: Option<u32>,
}

impl PositionerRules {
    /// `wlr_xdg_positioner_rules_get_geometry` — the unconstrained geometry in
    /// the parent surface's coordinate system.
    #[must_use] pub fn geometry(&self) -> Box2D;
    /// `wlr_xdg_positioner_rules_unconstrain_box` — rules applied against a
    /// constraint box, without touching any live popup.
    #[must_use] pub fn unconstrain_box(&self, constraint: &Box2D) -> Box2D;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PositionerAnchor { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PositionerGravity { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ConstraintAdjustment: u32 {
        const SLIDE_X  = 1;
        const SLIDE_Y  = 2;
        const FLIP_X   = 4;
        const FLIP_Y   = 8;
        const RESIZE_X = 16;
        const RESIZE_Y = 32;
    }
}
```

`PositionerAnchor`/`PositionerGravity` are `#[non_exhaustive]` and convert from
the raw `xdg_positioner_anchor`/`_gravity` C enums; an unknown value maps to
`None` and is logged once, never panics (untrusted-client rule, spec §7).

### 1.2 `Popup<'h>` — the borrowed handle

Mirrors `Toplevel<'h>` (`toplevel.rs:89`) exactly: `NonNull` raw + id +
`PhantomData<&'h ()>`, hand-written `Debug` (named fields only, no raw
pointers — `toplevel.rs:99`), `pub(crate) unsafe fn from_raw_with_id`.

```rust
pub struct Popup<'h> { /* raw: NonNull<sys::wlr_xdg_popup>, id: PopupId, _scope: PhantomData<&'h ()> */ }

impl Popup<'_> {
    #[must_use] pub fn id(&self) -> PopupId;

    /// The parent recorded when this popup was created. Never re-derived from
    /// `(*popup).parent` at call time (a layer popup's is NULL at creation).
    #[must_use] pub fn parent(&self) -> PopupParent;

    /// `scheduled.rules.anchor_rect`, in the parent's window-geometry space.
    #[must_use] pub fn anchor_rect(&self) -> Box2D;

    /// The client's full positioner, copied out.
    #[must_use] pub fn positioner_rules(&self) -> PositionerRules;

    /// `current.geometry` — where the popup actually is, once configured.
    #[must_use] pub fn geometry(&self) -> Box2D;

    /// `current.reactive`: the client wants re-unconstraining when the parent moves.
    #[must_use] pub fn is_reactive(&self) -> bool;

    /// `(*popup).seat != NULL` — the client sent `xdg_popup.grab`. wlroots owns
    /// the grab's lifetime; the crate only observes it (§1.6).
    #[must_use] pub fn grab_requested(&self) -> bool;

    /// `wlr_xdg_popup_unconstrain_from_box`. **`constraint` is in the ROOT
    /// TOPLEVEL PARENT surface's coordinate system**, not layout/output space —
    /// the compositor translates. Rewrites `scheduled.geometry` from the rules
    /// AND schedules a configure — see E20; it is guarded on `initialized` and
    /// does nothing on a popup that has not committed yet.
    pub fn unconstrain(&self, constraint: &Box2D);

    /// `wlr_xdg_popup_get_position` — position in the PARENT SURFACE's coords.
    #[must_use] pub fn position(&self) -> (f64, f64);

    /// `wlr_xdg_popup_get_toplevel_coords` — surface-local point mapped into
    /// the root toplevel's surface coordinates.
    #[must_use] pub fn toplevel_coords(&self, popup_sx: i32, popup_sy: i32) -> (i32, i32);

    /// `wlr_xdg_surface_schedule_configure(base)`. Returns the configure serial,
    /// or `0` when the surface is not `initialized` yet — **the call is skipped
    /// in that case**: the C function asserts `initialized` and Arch ships
    /// wlroots without `NDEBUG`, so an early call is a hard `abort()` of the
    /// whole compositor (the hazard `layer.rs:5-47` already documents).
    pub fn send_configure(&self) -> u32;

    /// `scheduled.reposition_token` when
    /// `scheduled.fields & WLR_XDG_POPUP_CONFIGURE_REPOSITION_TOKEN` is set.
    /// wlroots sends `xdg_popup.repositioned` itself; never forge one.
    #[must_use] pub fn reposition_token(&self) -> Option<u32>;

    /// `wlr_xdg_popup_destroy` — sends `xdg_popup.popup_done`, destroys the
    /// role object, makes the resource inert. Does **not** touch the scene tree.
    pub fn destroy(&self);
}
```

### 1.3 `ToplevelHandler` additions — all defaulted

Added to `ToplevelHandler` (`handler.rs:378`), **not** a new trait and **not**
a supertrait on the sealed `Handlers` (`handler.rs:961`) — that would be a
breaking change.

```rust
fn new_popup(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_initial_commit(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_mapped(&mut self, id: PopupId) { let _ = id; }
fn popup_unmapped(&mut self, id: PopupId) { let _ = id; }
fn popup_reposition(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_destroyed(&mut self, id: PopupId) { let _ = id; }
```

`popup_destroyed`'s doc carries the **"`id` may be one you were never told
about"** caveat word for word as `toplevel_destroyed` (`handler.rs:434-441`)
and `layer_surface_destroyed` (`handler.rs:598-603`) do.

### 1.4 Listeners, ids, session table

Per the research ruling, listen on the **parent-scoped** `new_popup`, not the
shell-level one:

| Signal | Registered in | Bound id |
|---|---|---|
| `(*toplevel).base->events.new_popup` | `on_new_toplevel` (`backend.rs:5043`), a 10th registration | `ToplevelId` |
| `(*layer_surface).events.new_popup` | `on_new_layer_surface` (`backend.rs:5649`), a 5th registration | `LayerSurfaceId` |
| `(*popup).base->events.new_popup` | the new `on_new_popup` | `PopupId` |
| `(*popup).base->surface->events.{commit,map,unmap}` | `on_new_popup` | `PopupId` |
| `(*popup).events.{destroy,reposition}` | `on_new_popup` | `PopupId` |

`wlr_xdg_shell.events.new_popup` is left **unused**: at shell level a
layer-shell popup still has `parent == NULL` and cannot be classified.

```rust
// backend.rs — Bound gains one slot; `link` is NOT widened (its slot list is
// frozen at five; follow `link_xwayland`'s precedent at backend.rs:351 and
// build the boxed Bound directly).
struct Bound { /* … */ popup: Option<PopupId> }

unsafe fn link_popup(
    signal: *mut sys::wl_signal,
    notify: sys::wl_notify_func_t,
    session: *const (),
    popup: PopupId,
) -> Registration;
```

Every popup listener is dropped from inside the popup's own destroy emission
while the object is still alive, so `alive`/`flag` are null — the same
"stronger claim" `link_node` (`backend.rs:552-560`) documents.

`PopupId` is minted on the popup's **`wlr_surface`** addon set:
`PopupId(ensure_id_raw(&raw mut (*(*popup).base).surface->addons))`.
`toplevel_id_of_surface` (`backend.rs:5197`) must gain a role check
(`wlr_xdg_popup_try_from_wlr_surface(surface).is_null()`), because a popup
surface now carries an id addon and would otherwise be handed back mislabelled
as a `ToplevelId`.

```rust
// backend.rs Session<'r, S>
popups: RefCell<HashMap<PopupId, PopupListeners>>,   // removed, and so unlinked,
                                                     // from on_popup_destroy —
                                                     // before the popup is freed

struct PopupListeners { _commit: Registration, _map: Registration, _unmap: Registration,
                        _destroy: Registration, _reposition: Registration, _new_popup: Registration }
```

**The event carried in `Bound` is authoritative.** Never read the id back out
of a signal's `data`: `wlr_surface.events.map`/`.unmap` and
`wlr_xdg_toplevel.events.destroy` already fire with NULL `data`
(`backend.rs:5091-5106`), and whether `wlr_xdg_popup.events.{destroy,reposition}`
do was not verifiable in-session.

### 1.5 Scene subtree and runtime entry

```rust
// runtime.rs
pub(crate) struct PopupEntry {
    pub raw: NonNull<sys::wlr_xdg_popup>,
    pub tree: NonNull<sys::wlr_scene_tree>,
    pub parent: PopupParent,
    pub configured: Cell<bool>,
}
```

The subtree is `wlr_scene_xdg_surface_create(parent_tree, (*popup).base)`,
where `parent_tree` is `ToplevelEntry.tree` / `LayerSurfaceEntry.scene_tree` /
the parent popup's own `tree`, so popups stack with their parent for free and
`Runtime::leaf_surface_at` (`runtime.rs:8170`) already resolves clicks on them
— including the session-lock isolation gate (`runtime.rs:8185-8197`). **P1 adds
no second lock gate and destroys no popup scene tree** (`forget_toplevel`,
`runtime.rs:6842`, explains why: wlroots frees a tree's children recursively).

```rust
impl Runtime {
    /// Borrowed handle; `None` once the popup is gone.
    pub fn popup(&self, id: PopupId) -> Option<Popup<'_>>;
    pub fn popup_parent(&self, id: PopupId) -> Option<PopupParent>;
    /// Direct children only, creation-ordered.
    pub fn popups_of(&self, parent: PopupParent) -> Vec<PopupId>;
    /// The whole subtree under `parent`, deepest last.
    pub fn popup_chain(&self, parent: PopupParent) -> Vec<PopupId>;
    /// `unconstrain(constraint)` then `send_configure()`; `false` if the popup
    /// is gone or not yet `initialized`. `constraint` is root-toplevel space.
    pub fn configure_popup(&self, id: PopupId, constraint: &Box2D) -> bool;
    pub fn popup_position(&self, id: PopupId) -> Option<(f64, f64)>;
    /// `Popup::destroy` for `id` and, deepest-first, every popup under it.
    pub fn dismiss_popup(&self, id: PopupId) -> usize;
    pub fn dismiss_popups_of(&self, parent: PopupParent) -> usize;
    /// `(*popup).seat != NULL`.
    pub fn popup_is_grabbing(&self, id: PopupId) -> bool;
    /// `wlr_seat_pointer_has_grab(seat) || wlr_seat_keyboard_has_grab(seat)` —
    /// whether *some* explicit seat grab is in force right now.
    pub fn seat_has_explicit_grab(&self) -> bool;
}
```

Every accessor **copies the field out and releases the `RefCell` borrow before
returning**, because the caller re-enters wlroots, which can emit a signal,
which can take the same borrow mutably (`runtime.rs:6890`, non-negotiable).
`record_popup`/`forget_popup`/`popup_entry`/`drop_all_popups` mirror the
toplevel quartet; `drop_all_popups` runs from `run_inner` when `run_all` returns.

### 1.6 Grabs and keyboard focus — what P1 must NOT do

wlroots builds `wlr_xdg_popup_grab` itself on `xdg_popup.grab` and installs
three seat grabs (pointer, keyboard, touch), routes delivery to the chain,
dismisses the chain on a press outside it, and restores the pre-grab keyboard
focus when the grab ends. There is no API to create, inspect or end one, and no
grab-start/grab-end signal.

* P1 **must not** call `wlr_seat_pointer_start_grab` / `_keyboard_start_grab` /
  `_end_grab` for a popup. A second `start_grab` displaces wlroots' own and
  breaks chain dismissal.
* P1 **must not** add a "focus the popup" call.
  `wlr_seat_keyboard_notify_enter` routes *through* the active keyboard grab,
  so `Runtime::focus_toplevel_keyboard` (`runtime.rs:7909`) already does the
  right thing by construction.
* The implicit-pointer-grab code (`grab_still_applies` `backend.rs:6834`,
  `pointer_motion_to_focus` `:6897`, `on_pointer_button` `:7171`) is **not
  touched**. An xdg-popup grab is exactly the "explicit grab" those three sites
  already name. `mod implicit_grab_tests` (`backend.rs:8517`), in particular
  `an_explicit_grab_drops_the_implicit_one` (`:8574`), must stay green
  **unmodified** — that is the regression signal that popups integrate rather
  than fight.
* Non-grabbing popups (`grab_requested() == false`: tooltips, non-modal
  popovers) get no grab and no focus restore; P2 decides their focus explicitly.
* If P1 adds `Runtime::focus_popup_keyboard`, it mirrors
  `focus_toplevel_keyboard`'s two gates verbatim (`is_session_locked()`
  `runtime.rs:7915`, `(*surface).mapped` `:7936`).

### 1.7 Events

`dispatch::Event` is `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` and carries
ids, never handles. Six variants:

```rust
NewPopup(PopupId), PopupInitialCommit(PopupId), PopupMapped(PopupId),
PopupUnmapped(PopupId), PopupReposition(PopupId), PopupDestroyed(PopupId),
```

`deliver_all` (`backend.rs:2260`) gains six arms next to the layer ones
(`:2335-2344`) using a `with_popup` helper modelled on `with_layer_surface`
(`backend.rs:2509`). **Compile-required:** the six names must also be added to
`deliver`'s `|`-chained unreachable arm (`backend.rs:7280-7310`) or the match
stops being exhaustive.

`on_popup_commit` gates on `(*(*popup).base).initial_commit`, emits
`PopupInitialCommit`, then calls `send_configure()` unconditionally — xdg-shell
requires the compositor to answer a popup's first commit or it never maps.

### 1.8 `LayerId`

**Already exists** — `LayerSurfaceId` (`layer.rs:115`). P1 adds no new layer id
and does not rename it; `PopupParent::Layer` takes `LayerSurfaceId`.

### 1.9 Coverage ledger — moves, in the same commit as the wrapper

The audit fails a `not-yet` row whose symbol appears in a `sys::` use, so the
ledger moves land with the code. Files:
`crates/wlr/coverage/{wrapped,waived}.toml`; gate
`cargo test -p wlr --test coverage_audit`, burn-down `cargo xtask coverage`.
Set `WLR_COVERAGE_BINDINGS` to an exact path and `WLR_COVERAGE_ALL_FEATURES=1`
after an all-features build.

| waived.toml line | symbol | disposition | module / item |
|---|---|---|---|
| 4445 | `wlr_xdg_popup` | → wrapped | `popup` / `Popup` |
| 4451 | `wlr_xdg_popup_configure` | → wrapped | `popup` / `Popup::reposition_token` |
| 4457 | `wlr_xdg_popup_configure_field` | → wrapped | `popup` / `Popup::reposition_token` |
| 4463 | `wlr_xdg_popup_destroy` | → wrapped | `popup` / `Popup::destroy` |
| 4469 | `wlr_xdg_popup_from_resource` | **stays waived** `not-yet`/M9 | resource-only, unreachable from the safe API |
| 4475 | `wlr_xdg_popup_get_position` | → wrapped | `popup` / `Popup::position` |
| 4481 | `wlr_xdg_popup_get_toplevel_coords` | → wrapped | `popup` / `Popup::toplevel_coords` |
| 4487 | `wlr_xdg_popup_grab` | **stays waived, re-reasoned** `internal` (no milestone) | note points at `wlr_seat_pointer_has_grab` / `popup->seat` as the observable; no `wlr_xdg_popup_grab_*` symbol exists |
| 4493 | `wlr_xdg_popup_state` | → wrapped | `popup` / `Popup::geometry` |
| 4499 | `wlr_xdg_popup_try_from_wlr_surface` | → wrapped | `backend` / `toplevel_id_of_surface` role check (§1.4) |
| 4505 | `wlr_xdg_popup_unconstrain_from_box` | → wrapped | `popup` / `Popup::unconstrain` |
| 4529 | `wlr_xdg_positioner_rules` | → wrapped | `popup` / `PositionerRules` |
| 4535 | `wlr_xdg_positioner_rules_get_geometry` | → wrapped | `popup` / `PositionerRules::geometry` |
| 4541 | `wlr_xdg_positioner_rules_unconstrain_box` | → wrapped | `popup` / `PositionerRules::unconstrain_box` |
| 2737 | `wlr_seat_keyboard_has_grab` | → wrapped | `runtime` / `Runtime::seat_has_explicit_grab` |
| 4511, 4517, 4523 | `wlr_xdg_positioner`, `_from_resource`, `_is_complete` | stay waived | resource-only |
| 4553, 4577, 1576, 1600 | `wlr_xdg_surface_for_each_popup_surface`, `_popup_surface_at`, `wlr_layer_surface_v1_for_each_popup_surface`, `_popup_surface_at` | stay waived | the scene graph covers them |

Reused unchanged (no ledger edit): `wlr_scene_xdg_surface_create`
(`wrapped.toml:1377`), `wlr_scene_node_set_position` (`:1222`),
`wlr_xdg_surface_schedule_configure` (`:1822`), `wlr_seat_pointer_has_grab`
(`:1427`), `wlr_seat_keyboard_notify_enter` (`:1412`), `wlr_box_empty` (`:97`).

### 1.10 Docs, version, publish

`crates/wlr/README.md` gains an xdg-popup section (parent-scoped listeners, the
"wlroots owns the grab" rule, the root-toplevel-space constraint box);
`lib.rs`'s `pub use` block (`:101-139`) gains
`pub use popup::{ConstraintAdjustment, Popup, PopupId, PopupParent, PositionerAnchor, PositionerGravity, PositionerRules};`
beside `pub use toplevel::{Edges, Toplevel, ToplevelId};`. `Cargo.toml` →
`0.20.28`. **Publishing is a consent stop**; `cargo publish --dry-run`,
`clippy -D warnings`, `cargo doc -D warnings` and the coverage audit run first.
`icedtea` consumes the published `0.20.28`, not a `[patch]`.

---

## 2. Compositor + harness [P2]

### 2.1 `compositor/src/wayland.rs`

```rust
/// Transparent, hashable wrapper over `wlr::PopupId` — the only file that names
/// `wlr` types stays `wayland.rs` (module doc, wayland.rs:1-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) wlr::PopupId);

impl PopupKey {
    pub(crate) fn new(id: wlr::PopupId) -> Self;
    /// A dangling id that never resolves to a live popup; for `State` unit tests.
    pub fn for_test(n: u64) -> Self;
}
```

### 2.2 `compositor/src/state.rs` — registry

```rust
/// What a popup hangs off, in model terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupHost {
    Window(WindowId),
    Layer(wlr::LayerSurfaceId),
    Popup(PopupKey),
}

/// The chain root — never a popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupRoot { Window(WindowId), Layer(wlr::LayerSurfaceId) }

pub struct PopupEntry {
    pub host: PopupHost,
    pub root: PopupRoot,
    /// Output index the root sits on, or `NO_OUTPUT` (`state.rs:43`).
    pub output: u32,
    /// Total creation order across all popups; the z-order tiebreak.
    pub sequence: u64,
    /// The client sent `xdg_popup.grab`.
    pub grabbing: bool,
    pub mapped: bool,
    /// Last configured geometry, in root-surface coordinates.
    pub geometry: Rectangle,
}

// State fields, alongside `layers` (state.rs:865) and `layer_focus` (:872):
popups: HashMap<PopupKey, PopupEntry>,
/// Creation-ordered; the tail is the topmost popup. Cleared root-first on dismiss.
popup_stack: Vec<PopupKey>,
next_popup_sequence: u64,
/// Whom keyboard focus returns to when a grabbing chain ends and wlroots'
/// own restore does not apply (non-grabbing chains, or a root that died).
focus_before_popup: Option<PopupRoot>,
```

```rust
impl State {
    /// The constraint box handed to `Runtime::configure_popup`, **in the root
    /// toplevel/layer surface's coordinate system** (not layout space): the
    /// root's output `usable` rect (`OutputSurface::usable`, state.rs:54)
    /// translated by minus the root's frame origin. `None` if the root or its
    /// output is gone. For a layer root the same output's `usable` is used, so
    /// a panel's menu is clamped to the same working area windows get.
    pub fn popup_constraint_box(&self, popup: PopupKey) -> Option<Rectangle>;

    pub(crate) fn record_popup(&mut self, popup: PopupKey, host: PopupHost, grabbing: bool);
    pub(crate) fn forget_popup(&mut self, popup: PopupKey);
    pub fn popup_root(&self, popup: PopupKey) -> Option<PopupRoot>;
    /// Whole subtree, deepest last.
    pub fn popup_chain(&self, root: PopupRoot) -> Vec<PopupKey>;
    /// Topmost popup whose last configured geometry contains `point` (frame
    /// space), searched `popup_stack` back to front. `None` while locked.
    pub fn popup_at_point(&self, point: (i32, i32)) -> Option<PopupKey>;
    /// Re-runs unconstrain+configure for every reactive popup under `root`.
    /// Called from the same places that move a window or re-arrange layers.
    pub fn reconstrain_popups(&mut self, root: PopupRoot);
    /// Called from `popup_destroyed` once the chain is empty: restores focus to
    /// `focus_before_popup`'s root (the parent), **not** to the pointer position.
    fn restore_focus_after_popups(&mut self);
}
```

**Z-order rule (normative).** A popup is painted above its own root and below
any layer surface in a band above the root's band; among siblings, higher
`sequence` wins. This falls out of the scene subtree (§1.5), so the model's
`popup_stack` exists for hit-testing and dismissal order only — the compositor
never calls `wlr_scene_node_place_*` for popups.

**Focus rules (normative).**
1. `grabbing == true`: the compositor does nothing. wlroots' popup grab takes
   keyboard focus, dismisses the chain on an outside press, and restores the
   pre-grab focus. `sync_seat_focus` (`state.rs:2288`) gains a **fourth** early
   return, before the layer check: `if self.runtime_has_explicit_grab() { return; }`.
2. `grabbing == false`: keyboard focus does not move at all; the popup is a
   visual overlay that takes pointer focus normally.
3. On the last popup of a chain being destroyed, `restore_focus_after_popups`
   points focus at the chain's root (window or layer), then
   `sync_focus_change`.
4. Session lock: no new gate. `Runtime::leaf_surface_at`'s lock isolation
   (§1.5) already denies input to popups on hidden windows; `sync_seat_focus`'s
   existing `session_locked` return (`state.rs:2291`) stands.
5. Escape is client-side. The compositor synthesises no `popup_done`.

**Handler impl.** `new_popup` → resolve the host through
`Wayland::window_for`/`self.layers`, `record_popup`, compute
`popup_constraint_box`, `Runtime::configure_popup`. `popup_reposition` →
recompute the box, `configure_popup` again. `popup_mapped`/`_unmapped` update
`mapped`. `popup_destroyed` → `forget_popup`, then rule 3 if the chain emptied
(and tolerate an id never seen, per §1.3's caveat).

### 2.3 `harness/src/lib.rs`

```rust
/// Everything an `xdg_positioner` needs, in one value, so a test reads as one
/// statement. `anchor`/`gravity`/`constraint_adjustment` are the protocol's own
/// enum/bitfield types re-exported by the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupSpec {
    pub anchor_rect: (i32, i32, i32, i32),
    pub size: (i32, i32),
    pub anchor: PopupAnchor,
    pub gravity: PopupGravity,
    pub constraint_adjustment: PopupConstraint,
    pub offset: (i32, i32),
    /// `Some(serial)` sends `xdg_popup.grab(seat, serial)` **before** the first
    /// commit, as xdg-shell requires. The serial must come from
    /// `last_pointer_serial()` after a button press.
    pub grab: Option<u32>,
    pub reactive: bool,
}

impl PopupSpec {
    /// 64×48 at (0,0,1,1), anchor+gravity BottomLeft, no constraint adjustment,
    /// no grab, not reactive — the shape most tests want.
    pub fn new(w: i32, h: i32) -> Self;
    pub fn anchor_rect(self, x: i32, y: i32, w: i32, h: i32) -> Self;
    pub fn anchor(self, a: PopupAnchor) -> Self;
    pub fn gravity(self, g: PopupGravity) -> Self;
    pub fn constraint(self, c: PopupConstraint) -> Self;
    pub fn grab(self, serial: u32) -> Self;
    pub fn reactive(self, on: bool) -> Self;
}

impl TestClient {
    /// Opens an `xdg_popup` on this client's toplevel and drives it to mapped:
    /// positioner → get_popup → [grab] → commit(no buffer) → wait configure →
    /// ack → attach → commit. Panics if a popup from this client is already
    /// live, or on a non-positive size.
    pub fn open_popup(&mut self, spec: PopupSpec);
    /// Opens a child popup of the current topmost popup (nested chains).
    pub fn open_popup_from_popup(&mut self, spec: PopupSpec);
    /// `(x, y, width, height)` from the last `xdg_popup.configure`, in the
    /// parent's window-geometry coordinates. `None` until one arrives.
    pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)>;
    /// Per-depth: `popup_configured_at(0)` is the outermost popup.
    pub fn popup_configured_at(&self, depth: usize) -> Option<(i32, i32, i32, i32)>;
    /// Whether *any* popup of this client has received `xdg_popup.popup_done`.
    pub fn popup_done(&self) -> bool;
    pub fn popup_depth(&self) -> usize;
    /// `xdg_popup.reposition(positioner, token)`.
    pub fn reposition_popup(&mut self, spec: PopupSpec, token: u32);
    /// The token echoed by `xdg_popup.repositioned`.
    pub fn popup_repositioned(&self) -> Option<u32>;
    /// Destroys the topmost popup (protocol requires reverse-creation order).
    pub fn destroy_popup(&mut self);
    /// Kept for `client_protocol.rs`: `open_popup(PopupSpec::new(w, h).grab(serial))`.
    pub fn open_grabbing_popup(&mut self, serial: u32, w: i32, h: i32);
}

impl LayerPanelClient {
    /// `zwlr_layer_surface_v1.get_popup` after an `xdg_surface.get_popup` with a
    /// NULL parent, per the layer-shell protocol.
    pub fn open_popup(&mut self, spec: PopupSpec);
    pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)>;
    pub fn popup_done(&self) -> bool;
}
```

`PopupHandles` (`lib.rs:1850`) becomes a `Vec<PopupHandles>` chain on
`TestClient`, each with its own `wl_surface`/`BufferPool`; `TestClient::detach`
destroys them topmost-first. The 0.31-proxies-have-no-Drop caveat
(`lib.rs:1839-1844`) is preserved verbatim.

`TestClient::popup_dismissed()` is **renamed** to `popup_done()`. It is the
only rename in the harness; its one call site is
`compositor/tests/client_protocol.rs:2765`.

### 2.4 `compositor/tests/popups.rs` — exact test names

```
a_popup_under_a_toplevel_is_configured_at_the_positioner_geometry
a_popup_that_would_leave_the_output_is_flipped_by_the_constraint_adjustment
a_popup_that_cannot_flip_is_slid_back_inside_the_usable_area
a_popup_under_a_layer_panel_is_constrained_to_the_same_output
a_nested_popup_chain_configures_every_level_relative_to_its_parent
a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it
keyboard_focus_returns_to_the_parent_when_a_grabbing_chain_ends
a_non_grabbing_popup_never_moves_keyboard_focus
a_reactive_popup_is_reconfigured_when_its_parent_moves
a_reposition_request_reconfigures_and_echoes_the_token
a_popup_on_a_hidden_window_receives_no_input_while_the_session_is_locked
destroying_a_parent_destroys_its_popup_chain_without_a_double_free
```

Each runs deterministically ×3. `compositor/src/state.rs`'s own `mod tests`
gains unit coverage for `popup_constraint_box`, `popup_at_point` and
`popup_chain` over `PopupKey::for_test` ids.

---

## 3. Window & event layer [P3]

### 3.1 `Surface`, `Window`, `InputEvent`

```rust
pub enum Surface {
    Toplevel(toplevel::Toplevel),
    Layer(layer::Layer),
    Popup(popup::Popup),
}

impl Surface {
    pub fn wl_surface(&self) -> &wl_surface::WlSurface;
    pub fn size(&self) -> (u32, u32);
    pub fn scale(&self) -> i32;
    pub fn states(&self) -> SurfaceStates;
    /// Ink-rect sizing: `resize` is honoured for Toplevel/Layer, ignored for a
    /// Popup (its size is fixed by the positioner until a reposition).
    pub fn resize(&mut self, size: (u32, u32)) -> Result<(), SurfaceError>;
    pub fn set_cursor_shape(&mut self, shape: CursorShape, serial: u32);
    pub fn commit_buffer(&mut self, skia: &mut skia_rs_safe::canvas::Surface) -> Result<(), SurfaceError>;
}

bitflags::bitflags! {
    /// `xdg_toplevel.configure` states, plus the layer/popup analogues.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct SurfaceStates: u16 {
        const MAXIMIZED = 1 << 0;
        const FULLSCREEN = 1 << 1;
        const RESIZING = 1 << 2;
        const ACTIVATED = 1 << 3;     // clear ⇒ :backdrop on the root node
        const TILED_LEFT = 1 << 4;
        const TILED_RIGHT = 1 << 5;
        const TILED_TOP = 1 << 6;
        const TILED_BOTTOM = 1 << 7;
        const SUSPENDED = 1 << 8;
    }
}

/// One window = one `Surface` + one retained tree + its layout/animation state
/// + the frame pump. Child popups are separate `Window`s parented to this one.
pub struct Window { /* private */ }

impl Window {
    pub fn open(surface_spec: SurfaceSpec, sheet: CompiledSheet, fonts: FontDatabase)
        -> Result<Self, SurfaceError>;
    pub fn root(&self) -> &Node;
    pub fn layout(&mut self) -> &mut LayoutTree;
    pub fn animations(&mut self) -> &mut AnimationState;
    pub fn clock(&self) -> &Rc<dyn Clock>;
    pub fn set_clock(&mut self, clock: Rc<dyn Clock>);
    pub fn sheet(&self) -> &CompiledSheet;
    pub fn fonts(&mut self) -> &mut FontDatabase;
    /// Restyle the dirty subtree, relayout, repaint, attach, commit. No-op when
    /// nothing is dirty and no animation is live.
    pub fn render(&mut self) -> Result<bool, SurfaceError>;
    pub fn mark_dirty(&mut self, node: &Node);
    /// Bounded wait: dispatch pending, `prepare_read`, `poll(2)` for the
    /// remaining time, read. Generalises M2's private `wait_bounded`
    /// (`wayland.rs:377`). `None` blocks until an event or `close()`.
    pub fn pump(&mut self, timeout: Option<Duration>) -> Result<Vec<InputEvent>, SurfaceError>;
    /// The animation clock's next deadline folded with the keyboard repeat
    /// deadline and every controller's `next_deadline`. `Duration::ZERO` is a
    /// legitimate "now", never "spin".
    pub fn next_deadline(&self) -> Option<Duration>;
    pub fn open_popup(&mut self, parent: PopupAnchorPoint, positioner: Positioner)
        -> Result<PopupKey, SurfaceError>;
    pub fn close_popup(&mut self, key: PopupKey);
    pub fn clipboard(&mut self) -> &mut Clipboard;
    pub fn is_closed(&self) -> bool;
}

/// Everything the window layer hands upward. One flat enum; the reactive layer
/// (§4) is the only consumer.
#[derive(Debug, Clone)]
pub enum InputEvent {
    PointerEnter { x: f64, y: f64, serial: u32 },
    PointerMotion { x: f64, y: f64, time_ms: u32 },
    PointerLeave,
    PointerButton { button: u32, pressed: bool, serial: u32, time_ms: u32 },
    Scroll(Scroll),
    TouchDown { id: i32, x: f64, y: f64, serial: u32, time_ms: u32 },
    TouchMotion { id: i32, x: f64, y: f64, time_ms: u32 },
    TouchUp { id: i32, serial: u32, time_ms: u32 },
    KeyboardEnter { serial: u32 },
    KeyboardLeave,
    Key(KeyEvent),
    Configure { size: (u32, u32), states: SurfaceStates },
    ScaleChanged(i32),
    Frame { now: Duration },
    Close,
    PopupDone(PopupKey),
    Repositioned { key: PopupKey, token: u32 },
    SelectionChanged,
    PrimaryChanged,
}

#[derive(Debug)]
pub enum SurfaceError {
    Connect, MissingGlobal(&'static str), Shm, Socket,
    Render(&'static str), NoFont, Dispatch, Closed,
    Timeout(Duration), Protocol(&'static str), Keymap,
}
```

`SurfaceError` is a superset of M2's `LayerWindowError` (same variant names for
the nine that carry over) — `LayerWindowError` becomes
`pub type LayerWindowError = SurfaceError;` so M2's tests keep compiling
(§8).

### 3.2 Roles — request/event mapping

| Role | Requests it may send | Events it consumes | Maps to |
|---|---|---|---|
| `Toplevel` | `set_title`, `set_app_id`, `set_min_size`, `set_max_size`, `set_maximized`/`unset_`, `set_fullscreen`/`unset_`, `set_minimized`, `move`, `resize`, `show_window_menu`, `xdg_surface.set_window_geometry`, `ack_configure` | `xdg_toplevel.Configure{width,height,states}`, `.Close`, `.ConfigureBounds`, `.WmCapabilities`, `xdg_surface.Configure{serial}` | `InputEvent::Configure` (states → `SurfaceStates`; `!ACTIVATED` sets `PseudoStates::BACKDROP` on the root), `InputEvent::Close` |
| `Layer` | `set_anchor`, `set_size`, `set_exclusive_zone`, `set_margin`, `set_keyboard_interactivity`, `set_layer`, `get_popup`, `ack_configure` | `zwlr_layer_surface_v1.Configure{serial,w,h}`, `.Closed` | `InputEvent::Configure` (states always empty), `InputEvent::Close` |
| `Popup` | `xdg_positioner.{set_size,set_anchor_rect,set_anchor,set_gravity,set_constraint_adjustment,set_offset,set_reactive,set_parent_size,set_parent_configure}`, `xdg_popup.{grab,reposition,destroy}`, `ack_configure` | `xdg_popup.Configure{x,y,w,h}`, `.PopupDone`, `.Repositioned{token}`, `xdg_surface.Configure` | `InputEvent::Configure` (size only; x/y stored on the `Popup`), `InputEvent::PopupDone`, `InputEvent::Repositioned` |

Sequencing invariant every role honours: `xdg_surface` → role object → role
state → **first `commit` with no buffer** → wait `configure` →
`ack_configure` → attach → commit. A positioner needs a non-zero `set_size`
**and** `set_anchor_rect` before `get_popup` or the compositor raises
`invalid_positioner`. `xdg_wm_base::Event::Ping` is answered with `pong`
inside `pump` and never surfaces as an `InputEvent`. Nested popups are
destroyed strictly topmost-first.

```rust
/// The toolkit's positioner, converted to `xdg_positioner` requests on use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Positioner {
    pub anchor_rect: layout::Rect,   // in the parent window's frame space
    pub size: (u32, u32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    pub constraint: ConstraintAdjustment,
    pub offset: (i32, i32),
    pub reactive: bool,
}

/// Where a popup hangs: a node of the parent window (its allocation becomes the
/// anchor rect) or an explicit rect.
#[derive(Debug, Clone)]
pub enum PopupAnchorPoint { Node(Node), Rect(layout::Rect) }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) u64);
```

### 3.3 `window/keyboard.rs`

```rust
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Mods: u8 {
        const SHIFT = 1 << 0;
        const CTRL  = 1 << 1;
        const ALT   = 1 << 2;   // Mod1
        const LOGO  = 1 << 3;   // Mod4
        const CAPS  = 1 << 4;   // Lock
        const NUM   = 1 << 5;   // Mod2
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// The raw `wl_keyboard.key` evdev code. XKB's is this + 8.
    pub keycode: u32,
    pub keysym: xkbcommon::xkb::Keysym,
    /// `None` for a key with no text (modifiers, arrows, F-keys).
    pub utf8: Option<String>,
    pub mods: Mods,
    pub pressed: bool,
    pub repeat: bool,
    pub serial: u32,
    pub time_ms: u32,
}

impl KeyEvent {
    /// `mods` with the modifiers this keysym consumed removed — the mask to
    /// compare an accelerator against (`xkb_state_mod_mask_remove_consumed`).
    #[must_use] pub fn effective_mods(&self) -> Mods;
    #[must_use] pub fn matches(&self, keysym: xkbcommon::xkb::Keysym, mods: Mods) -> bool;
}

pub struct Keymap { /* xkb::Context, xkb::Keymap, xkb::State, Option<xkb::compose::State>, repeat */ }

impl Keymap {
    /// From `wl_keyboard.keymap`. `MAP_PRIVATE` is required from protocol v7 on
    /// and is what `xkb::Keymap::new_from_fd` already does — never hand-roll an
    /// `mmap` with `MAP_SHARED`.
    pub fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, KeymapError>;
    /// RMLVO, for hermetic tests: `Keymap::from_names("us", "")`.
    pub fn from_names(layout: &str, variant: &str) -> Result<Self, KeymapError>;
    /// From a vendored keymap string (`ui/tests/fixtures/keymaps/us.xkb`).
    pub fn from_string(text: &str) -> Result<Self, KeymapError>;
    /// From `wl_keyboard.modifiers`; `group` fills all three layout components.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32);
    /// From `wl_keyboard.key`, before `translate`.
    pub fn update_key(&mut self, keycode: u32, pressed: bool);
    /// Translate, running the compose table (locale from `LC_ALL`/`LC_CTYPE`/
    /// `LANG`) so a dead-key sequence yields one composed `utf8`.
    pub fn translate(&mut self, keycode: u32, pressed: bool, serial: u32, time_ms: u32) -> KeyEvent;
    #[must_use] pub fn mods(&self) -> Mods;
    /// `xkb_keymap_key_repeats` — modifiers say `false`.
    #[must_use] pub fn repeats(&self, keycode: u32) -> bool;
    /// From `wl_keyboard.repeat_info`. `rate == 0` disables repeat entirely.
    pub fn set_repeat_info(&mut self, rate: i32, delay: i32);
    /// Arm/disarm the repeat timer for the held key. Cleared by the matching
    /// key-up and by `wl_keyboard.leave`.
    pub fn arm_repeat(&mut self, ev: &KeyEvent, now: Duration);
    pub fn clear_repeat(&mut self);
    /// Next repeat fire time on the animation clock, folded into
    /// `Window::next_deadline`.
    #[must_use] pub fn repeat_deadline(&self, now: Duration) -> Option<Duration>;
    /// The synthetic repeat `KeyEvent` due at `now`, if any.
    pub fn repeat_due(&mut self, now: Duration) -> Option<KeyEvent>;
}

#[derive(Debug)] pub enum KeymapError { Context, Compile, Mmap(std::io::Error) }
```

xkbcommon has no repeat scheduler; the timer is ours, on `anim::Clock`.
The compose types must be spelled `xkb::compose::{State, Table}` — the glob
re-export loses `State` to the keyboard state.

### 3.4 `window/pointer.rs`

```rust
#[derive(Debug, Clone)]
pub struct Hit { pub node: Node, /// point in the node's border-box space
                 pub local: (f32, f32) }

/// Topmost-painted node containing `point` (window frame space). Skips a
/// subtree whose computed `opacity` is 0, whose `visibility` is `hidden`, or
/// whose node carries `PseudoStates::DISABLED` when `respect_sensitive` — GTK
/// delivers no events to insensitive or unmapped widgets.
pub fn hit_test(root: &Node, tree: &LayoutTree, styles: &StyleMap,
                point: (f32, f32), respect_sensitive: bool) -> Option<Hit>;

/// The full capture→target chain, outermost first. Controller dispatch is
/// capture → target → bubble, mirroring GTK.
pub fn hit_chain(root: &Node, tree: &LayoutTree, styles: &StyleMap,
                 point: (f32, f32)) -> Vec<Hit>;

/// Client-side mirror of the compositor's implicit grab: once a button goes
/// down on a node, motion and the release go to that node regardless of what is
/// under the pointer, until the last button is released.
#[derive(Debug, Default)]
pub struct ImplicitGrab { /* target: Option<Node>, held: u32 (bitmask) */ }

impl ImplicitGrab {
    /// `true` if this press began the grab.
    pub fn press(&mut self, button: u32, target: &Node) -> bool;
    /// `true` if this release ended it.
    pub fn release(&mut self, button: u32) -> bool;
    #[must_use] pub fn target(&self) -> Option<Node>;
    #[must_use] pub fn is_held(&self) -> bool;
    pub fn clear(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scroll {
    pub dx: f32, pub dy: f32,
    pub source: ScrollSource,
    /// `wl_pointer.axis_stop` — ends a kinetic gesture.
    pub stop: bool,
    pub time_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSource { Wheel, Finger, Continuous, WheelTilt }

/// Kinetic deceleration on the animation clock, for `Finger` scrolls after a
/// `stop`. Exponential decay; stops below `MIN_VELOCITY`.
#[derive(Debug, Default)]
pub struct Kinetic { /* velocity: (f32, f32), last_ms: f64 */ }

impl Kinetic {
    pub fn feed(&mut self, scroll: &Scroll, now: Duration);
    /// Delta to apply this frame; `None` once stopped.
    pub fn sample(&mut self, now: Duration) -> Option<(f32, f32)>;
    pub fn cancel(&mut self);
}

/// CSS `cursor` keyword → `wp_cursor_shape_device_v1::Shape`. Unknown or
/// `url()` values fall back to `Default` (client-side cursor themes are out of
/// scope; icedtea supports `wp_cursor_shape_v1`).
pub fn cursor_shape_for(style: &ComputedStyle) -> CursorShape;
```

`StyleMap` is `P4`'s per-node computed-style cache
(`HashMap<NodeAddr, Rc<ComputedStyle>>`, keyed by `Node::addr()`); P3 takes it
by reference and never builds one.

### 3.5 `window/focus.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection { TabForward, TabBackward, Up, Down, Left, Right }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCause { Keyboard, Pointer, Programmatic }

/// One focus owner per window; popups form a stack of rings.
#[derive(Debug, Default)]
pub struct FocusRing { /* focus: Option<Node>, focus_visible: bool, default: Option<Node> */ }

impl FocusRing {
    #[must_use] pub fn focus(&self) -> Option<Node>;
    /// Moves `PseudoStates::FOCUS` (M2 derives `:focus-within`/`:focus-visible`
    /// up the chain from it) and updates `focus_visible` per §3.5's rule.
    pub fn set_focus(&mut self, node: Option<&Node>, cause: FocusCause);
    /// GTK's rule, not CSS's: defaults to `true`; a key **press** remembers the
    /// focus; the matching **release** clears it only if the focus did not move
    /// while the key was down, and sets it otherwise; `Alt` alone forces `true`.
    #[must_use] pub fn focus_visible(&self) -> bool;
    pub fn note_key(&mut self, ev: &KeyEvent);
    pub fn set_default(&mut self, node: Option<&Node>);
    #[must_use] pub fn default(&self) -> Option<Node>;
}

/// Focusable, mapped, sensitive nodes under `root`, in **geometric** order for
/// `dir` — GTK's `gtk_widget_focus_sort`, not tree order: seed with the
/// container's mapped+sensitive direct children, then sort by y-centre and, on
/// a tie, x-centre (x reversed under RTL); `TabBackward` reverses the result.
/// A `BoxLayout`-shaped container delegates to the directional sort along its
/// own orientation.
pub fn focus_sort(parent: &Node, tree: &LayoutTree, dir: FocusDirection) -> Vec<Node>;

/// Next focus from `from` in `dir`, recursing into containers. `None` at the
/// end of the ring (the caller wraps, or lets the popup stack pop).
pub fn navigate(root: &Node, tree: &LayoutTree, from: Option<&Node>,
                dir: FocusDirection) -> Option<Node>;

/// Whether a node participates in the ring at all — `focusable` prop, not
/// disabled, non-zero allocation. `WindowControls`' three buttons are
/// `focusable = false` and are never candidates.
pub fn is_focusable(node: &Node, tree: &LayoutTree) -> bool;
```

Window-level key bindings P3 installs before the focused node sees the event:
`Tab`/`KP_Tab` (± `Ctrl`) → `TabForward`; `Shift+Tab`/`Ctrl+Shift+Tab` →
`TabBackward`; arrows and `KP_` variants, plain and with `Ctrl` → the four
directions; `Space`/`KP_Space` → activate the focused node;
`Return`/`ISO_Enter`/`KP_Enter` → activate the window default. `Escape` in a
popup window sends `xdg_popup.destroy` client-side. `Ctrl+Shift+I`/`D` are
**not** implemented.

### 3.6 Layout additions [P6 owns the edit; P3/P5 consume]

`layout::Container` gains the variants M2's own doc comment defers to M3
(`layout.rs:163`), and `set_style` stops hard-coding
`AlignItems::CENTER`/`JustifyContent::CENTER` (`layout.rs:423`):

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

/// Per-child placement, read from the node's props by `set_style`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ChildLayout {
    pub halign: Align, pub valign: Align,
    pub hexpand: bool, pub vexpand: bool,
    pub margin: [f32; 4],           // top, right, bottom, left
    pub grid: Option<GridPlacement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPlacement { pub column: u16, pub row: u16, pub column_span: u16, pub row_span: u16 }

impl LayoutTree {
    pub fn set_container(&mut self, node: &Node, container: Container);
    pub fn set_child_layout(&mut self, node: &Node, child: ChildLayout);
    /// A closure the tree calls to measure a leaf (text, icon, drawing area).
    pub fn set_measure(&mut self, node: &Node, measure: Rc<dyn Measure>);
}
```

### 3.7 Text additions [P5 owns the edit; P6 consumes]

M2's `text.rs` is single-line/single-run. M3 adds one type; `ShapedText`,
`FontDatabase`, `TextStyle` and every M2 signature are unchanged.

```rust
/// A wrapped, ellipsized, cursor-aware paragraph over `FontDatabase::shape`.
pub struct TextLayout { /* lines: Vec<ShapedLine>, … */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ellipsize { None, Start, Middle, End }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode { None, Word, Char, WordChar }

impl TextLayout {
    pub fn build(text: &str, style: &TextStyle, fonts: &mut FontDatabase,
                 width: Option<f32>, wrap: WrapMode, ellipsize: Ellipsize) -> Self;
    #[must_use] pub fn size(&self) -> (f32, f32);
    #[must_use] pub fn line_count(&self) -> usize;
    /// Byte offset ↔ caret rect, both directions; the basis for every cursor,
    /// selection and hit-test in the entry family and `TextView`.
    #[must_use] pub fn caret_rect(&self, byte: usize) -> layout::Rect;
    #[must_use] pub fn byte_at(&self, point: (f32, f32)) -> usize;
    /// Grapheme-cluster and word boundaries for arrow/Ctrl-arrow movement.
    #[must_use] pub fn next_grapheme(&self, byte: usize) -> usize;
    #[must_use] pub fn prev_grapheme(&self, byte: usize) -> usize;
    #[must_use] pub fn next_word(&self, byte: usize) -> usize;
    #[must_use] pub fn prev_word(&self, byte: usize) -> usize;
    /// Selection rectangles, one per covered line, for the `selection` node.
    #[must_use] pub fn selection_rects(&self, range: Range<usize>) -> Vec<layout::Rect>;
    pub fn draw(&self, canvas: &mut Canvas<'_>, origin: (f32, f32), color: Rgba);
}

/// The `<b><i><span foreground=… weight=… size=…>` subset, and nothing else.
/// An unknown tag or attribute is dropped and logged once; never panics.
pub fn parse_markup(text: &str) -> (String, Vec<MarkupSpan>);

#[derive(Debug, Clone, PartialEq)]
pub struct MarkupSpan { pub range: Range<usize>, pub bold: bool, pub italic: bool,
                        pub color: Option<Rgba>, pub weight: Option<f32>, pub size_px: Option<f32> }
```

### 3.8 `window/selection.rs`

```rust
pub struct Clipboard { /* wl_data_device_manager, wl_data_device,
                          zwp_primary_selection_device_manager_v1, devices, offers */ }

impl Clipboard {
    /// Offers `text/plain;charset=utf-8`. `serial` must come from a real input
    /// event. AMENDED (M3 review-fix wave, 2026-09-03): a compositor that never
    /// advertised `wl_data_device_manager` no longer panics — the clipboard
    /// degrades to the in-process offscreen transport with a warning, matching
    /// what the primary-selection half always did for its own missing manager.
    /// The original fail-fast ruling cost a legal minimal compositor the whole
    /// app; the review judged the asymmetry a defect.
    pub fn copy(&mut self, text: &str, serial: u32);
    /// Reads the current selection with a deadline; `None` on no offer, no
    /// matching mime type, or timeout.
    pub fn paste(&mut self, deadline: Duration) -> Option<String>;
    pub fn set_primary(&mut self, text: &str, serial: u32);
    pub fn primary(&mut self, deadline: Duration) -> Option<String>;
    #[must_use] pub fn has_selection(&self) -> bool;
    #[must_use] pub fn has_primary(&self) -> bool;
}
```

Bound at `wl_data_device_manager` `version.min(3)`; the offer child object is
wired with `event_created_child!`, exactly as `harness/src/lib.rs:1237-1296`
does. DnD is M6 and is not wired here.

### 3.9 P3 tests

Unit (hermetic, no compositor): key translation against
`ui/tests/fixtures/keymaps/us.xkb` (plain letter, `Shift`, `Ctrl` consumed-mod
masking, a dead-key compose sequence, a repeat-ineligible modifier);
`focus_sort` order for a Box, a Grid, a HeaderBar with end packs and an
Overlay, including RTL; `hit_test` with three overlapping siblings, an
`opacity: 0` subtree and a disabled node; `ImplicitGrab` press/drag-off/release;
`Kinetic` decay on a `ManualClock`.

E2E (harness compositor): a toplevel maps and is SSD-decorated; a `configure`
resize relayouts and repaints; `VirtualKeyboardClient` types into an `entry`
and screencopy shows the glyphs; `Tab` moves the focus ring and the ring is
only visible after the key; a popup opened from a `menubutton` receives the
grab and is dismissed by a click outside.

---

## 4. Reactive framework [P4]

### 4.1 `View`

```rust
pub struct View<Msg> {
    pub kind: Kind,
    pub key: Option<Key>,
    pub props: Props,
    pub children: Vec<View<Msg>>,
    pub handlers: Handlers<Msg>,
}

impl<Msg: 'static> View<Msg> {
    pub fn new(kind: Kind) -> Self;
    pub fn key(self, key: impl Into<Key>) -> Self;
    pub fn child(self, child: View<Msg>) -> Self;
    pub fn children(self, children: impl IntoIterator<Item = View<Msg>>) -> Self;
    pub fn prop(self, name: PropName, value: impl Into<Prop>) -> Self;
    pub fn on(self, event: EventKind, handler: Handler<Msg>) -> Self;
    /// Every widget gets these, from `GtkWidget`'s own property set.
    pub fn class(self, class: &str) -> Self;
    pub fn classes(self, classes: &[&str]) -> Self;
    pub fn id(self, id: &str) -> Self;
    pub fn visible(self, on: bool) -> Self;
    pub fn sensitive(self, on: bool) -> Self;
    pub fn focusable(self, on: bool) -> Self;
    pub fn tooltip(self, text: &str) -> Self;
    pub fn halign(self, a: Align) -> Self;
    pub fn valign(self, a: Align) -> Self;
    pub fn hexpand(self, on: bool) -> Self;
    pub fn vexpand(self, on: bool) -> Self;
    pub fn margin(self, top: i32, right: i32, bottom: i32, left: i32) -> Self;
    pub fn width_request(self, px: i32) -> Self;
    pub fn height_request(self, px: i32) -> Self;
    pub fn cursor(self, name: &str) -> Self;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key { Index(usize), Id(u64), Name(Rc<str>) }
impl From<usize> for Key {} impl From<u64> for Key {} impl From<&str> for Key {}
```

### 4.2 `Kind`

One variant per in-scope widget, plus the sub-kinds GTK renders as their own
node (`ListBoxRow`, `FlowBoxChild`, `ColumnViewColumn`, `NotebookTab`,
`StackPage`, `PopoverMenuItem`). Order is the §5 catalogue order.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    // P5 · display
    Label, Spinner, Statusbar, LevelBar, ProgressBar, InfoBar, Scrollbar,
    Image, Picture, Separator, TextView, Scale, DrawingArea, WindowControls,
    Calendar, Popover,
    // P5 · buttons
    Button, ToggleButton, LinkButton, CheckButton, MenuButton, Switch,
    DropDown, ColorDialogButton, ColorDialog, FontDialogButton, FontDialog,
    // P5 · entries
    Entry, SearchEntry, PasswordEntry, SpinButton, EditableLabel,
    // P6 · containers
    Box, Grid, CenterBox, ScrolledWindow, Paned, Frame, Expander, SearchBar,
    ActionBar, HeaderBar, Notebook, NotebookTab, Overlay, Stack, StackPage,
    StackSwitcher, StackSidebar,
    // P6 · lists
    ListBox, ListBoxRow, FlowBox, FlowBoxChild, ListView, GridView,
    ColumnView, ColumnViewColumn,
    // P6 · menus
    PopoverMenu, PopoverMenuBar, PopoverMenuItem,
    // P6 · windows
    Window, ShortcutsWindow, AboutDialog, AlertDialog,
}

impl Kind {
    /// The CSS node name this kind's ROOT node carries. Several kinds share one
    /// (`Button`/`ToggleButton`/`LinkButton` → `button`; `Box`/`CenterBox` →
    /// `box`) — fixtures key on `Kind`, never on the name.
    #[must_use] pub fn css_name(self) -> &'static str;
    /// Style classes the kind always adds (`ToggleButton` → `["toggle"]`).
    #[must_use] pub fn base_classes(self) -> &'static [&'static str];
    #[must_use] pub fn is_focusable_by_default(self) -> bool;
    #[must_use] pub fn all() -> &'static [Kind];
}
```

### 4.3 `Props`

```rust
/// A typed name, never a string — the §0 "nothing names a property string"
/// rule extended to widget props.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PropName {
    // GtkWidget-universal
    Classes, Id, Visible, Sensitive, Focusable, Tooltip, Halign, Valign,
    Hexpand, Vexpand, Margin, WidthRequest, HeightRequest, Cursor, Opacity,
    // shared widget props
    Label, Text, Placeholder, Icon, IconSize, Active, Checked, Indeterminate,
    Value, Lower, Upper, StepIncrement, PageIncrement, Digits, Wrap, Ellipsize,
    Xalign, Yalign, Markup, Selectable, Uri, Group, Orientation, Spacing,
    Homogeneous, RowSpacing, ColumnSpacing, Position, Ratio, Expanded,
    Title, Subtitle, Fraction, Pulse, Inverted, ShowText, MaxLength,
    Visibility, Editable, EnableUndo, SelectionMode, Model, Selected,
    EnableSearch, ShowArrow, Modal, Autohide, Transition, TransitionDuration,
    Reveal, Decoration, Side, Anchor, Gravity, Constraint, Offset, Reactive,
    Columns, Rows, Column, Row, ColumnSpan, RowSpan, Fit, Paintable,
    DrawFn, ItemFactory, Page, Sortable, Resizable, MinContentWidth,
    MinContentHeight, OverlayScrolling, Kinetic, ShowSeparators, MarksTop,
    MarksBottom, FillLevel, Message, Detail, Buttons, MessageType,
}

#[derive(Clone)]
pub enum Prop {
    Str(Rc<str>),
    Bool(bool),
    Int(i64),
    Float(f64),
    Icon(IconRef),
    Classes(Rc<[Rc<str>]>),
    Edges([i32; 4]),
    Align(Align),
    Enum(u16),                                  // widget-local enums, via `#[repr(u16)]`
    Items(Rc<[ListItem]>),                      // list models
    Draw(Rc<dyn Fn(&mut Canvas<'_>, layout::Rect)>),
    None,
}

impl PartialEq for Prop { /* `Draw` compares by `Rc::ptr_eq`; everything else by value */ }

/// Small sorted map — most widgets carry under a dozen props, and `set_prop`
/// only fires on an actual change (generation-driven restyle).
#[derive(Clone, Default, PartialEq)]
pub struct Props(Vec<(PropName, Prop)>);

impl Props {
    pub fn set(&mut self, name: PropName, value: Prop);
    #[must_use] pub fn get(&self, name: PropName) -> Option<&Prop>;
    #[must_use] pub fn str(&self, name: PropName) -> Option<&str>;
    #[must_use] pub fn bool(&self, name: PropName, default: bool) -> bool;
    #[must_use] pub fn int(&self, name: PropName, default: i64) -> i64;
    #[must_use] pub fn float(&self, name: PropName, default: f64) -> f64;
    /// Props in `self` that differ from `prev`, plus props removed since —
    /// the reconciler's `SetProp` op set.
    pub fn diff(&self, prev: &Props) -> Vec<PropName>;
    pub fn iter(&self) -> impl Iterator<Item = (PropName, &Prop)>;
}
```

### 4.4 `Handlers`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum EventKind {
    Click, Activate, Toggle, Change, Selected, ValueChanged, PageChanged,
    Expanded, Scrolled, Close, Response, FocusIn, FocusOut, KeyPressed,
    ActivateLink, Reordered, Search, DateSelected,
}

pub enum Handler<Msg> {
    Unit(Msg),                                  // Msg: Clone
    Text(Rc<dyn Fn(&str) -> Msg>),
    Bool(Rc<dyn Fn(bool) -> Msg>),
    Index(Rc<dyn Fn(usize) -> Msg>),
    Float(Rc<dyn Fn(f64) -> Msg>),
    Key(Rc<dyn Fn(&KeyEvent) -> Option<Msg>>),
}

#[derive(Default)]
pub struct Handlers<Msg>(Vec<(EventKind, Handler<Msg>)>);

impl<Msg: Clone + 'static> Handlers<Msg> {
    pub fn set(&mut self, kind: EventKind, handler: Handler<Msg>);
    /// The controller's only way to emit; `None` when nothing is bound.
    #[must_use] pub fn fire_unit(&self, kind: EventKind) -> Option<Msg>;
    #[must_use] pub fn fire_text(&self, kind: EventKind, value: &str) -> Option<Msg>;
    #[must_use] pub fn fire_bool(&self, kind: EventKind, value: bool) -> Option<Msg>;
    #[must_use] pub fn fire_index(&self, kind: EventKind, value: usize) -> Option<Msg>;
    #[must_use] pub fn fire_float(&self, kind: EventKind, value: f64) -> Option<Msg>;
}
```

**Builder naming rule (normative, P5/P6 must follow it exactly).**
A widget `Foo` has one free constructor `fn foo(..) -> View<Msg>` in
`view::builders`, named in `snake_case` from the GTK class name with the `Gtk`
prefix dropped (`GtkCheckButton` → `check_button`, `GtkColorDialogButton` →
`color_dialog_button`). Its required content is a positional argument
(`button("Ok")`, `label("Hi")`, `scale(0.0, 100.0)`); everything else is a
chained `self`-consuming setter named after the **GTK property**, in
`snake_case`, with no `set_` prefix (`.wrap(true)`, `.show_text(true)`,
`.max_length(32)`). Handlers are `on_<eventkind snake_case>`
(`.on_click(Msg::Ok)`, `.on_change(Msg::Name)`, `.on_selected(Msg::Pick)`),
taking a `Msg` for `Handler::Unit` and a closure otherwise. Every builder
returns `View<Msg>` so `.class()/.margin()/.key()` from §4.1 chain after.

### 4.5 Reconciler

```rust
/// One retained widget: the M2 node it owns, plus the controller that owns the
/// behaviour state the model must not carry.
pub struct Instance<Msg> {
    pub node: Node,
    pub kind: Kind,
    pub key: Option<Key>,
    pub props: Props,
    pub handlers: Handlers<Msg>,
    pub controller: Box<dyn Controller<Msg>>,
    pub children: Vec<Instance<Msg>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Insert { index: usize },
    Remove { index: usize },
    Move { from: usize, to: usize },
    SetProp { index: usize, name: PropName },
    SetHandlers { index: usize },
    Recurse { index: usize },
}

/// Keyed LCS with moves.
/// - same `key` + same `kind` → keep the `Instance`; `props.diff` drives
///   `SetProp`, which drives `Controller::set_prop`, which mutates the `Node`
///   (generation bump → M2 restyles the dirty subtree only);
/// - new → build `Node` + controller (`Insert`);
/// - gone → drop the `Instance` (controller drops; running animations finish
///   per `fill-mode` before the node is detached);
/// - reordered → `Move` (`Node::insert_child` reparents in place);
/// - unkeyed children match **positionally, within their kind**.
/// Identity survives every one of these: animation state, focus, shaping
/// caches, tree ids, attached popups.
/// Returns the ops actually applied, minimal — no `SetProp` for an unchanged
/// value, no `Move` for an element already at its destination.
pub fn reconcile<Msg: Clone + 'static>(
    parent: &Node,
    prev: &mut Vec<Instance<Msg>>,
    next: Vec<View<Msg>>,
    cx: &mut BuildCx<'_>,
) -> Vec<Op>;

/// What building a controller needs. Threaded down the whole reconcile.
pub struct BuildCx<'a> {
    pub sheet: &'a CompiledSheet,
    pub fonts: &'a mut FontDatabase,
    pub icons: &'a mut IconTheme,
    pub clock: &'a Rc<dyn Clock>,
    pub env: &'a ResolveEnv,
}
```

### 4.6 `Controller`

```rust
/// Behaviour and the state the model should not own: press state, entry cursor
/// / selection / undo stack, scroll offset, expander progress, dropdown open
/// state, spin repeat timer.
pub trait Controller<Msg>: 'static {
    fn kind(&self) -> Kind;

    /// Build the kind's subnodes under `node` and seed state from `props`.
    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self where Self: Sized;

    /// One prop changed. Must update `node` (classes, states, subnode text) —
    /// this is the only place a controller touches the retained tree outside
    /// `on_event`/`tick`.
    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>);

    /// The whole behaviour. Returns the messages this event produced, in order.
    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg>;

    /// Clock-driven behaviour: spin repeat, kinetic scroll, expander animation,
    /// spinner rotation. Default: nothing.
    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let _ = (now, cx);
        Vec::new()
    }

    /// Lower bound on the next `tick` that would do something. `Duration::ZERO`
    /// is a legitimate "now", never "spin". Default: `None`.
    fn next_deadline(&self, now: Duration) -> Option<Duration> { let _ = now; None }

    /// Intrinsic size for a leaf the layout tree cannot measure itself
    /// (text, icon, drawing area). Default: `None` (taffy measures the box).
    fn measure(&mut self, available: (Option<f32>, Option<f32>), cx: &mut BuildCx<'_>)
        -> Option<(f32, f32)> { let _ = (available, cx); None }

    /// Extra painting inside the node's own layer, after M2's box painting and
    /// before children. `true` if anything was drawn. Default: `false`.
    fn paint(&mut self, canvas: &mut Canvas<'_>, alloc: &Allocation,
             style: &ComputedStyle, cx: &mut PaintCx<'_>) -> bool {
        let _ = (canvas, alloc, style, cx); false
    }
}

/// What a controller sees. `InputEvent` narrowed to this node, plus the
/// lifecycle events the framework synthesises.
#[derive(Debug, Clone)]
pub enum Event {
    PointerEnter { local: (f32, f32) },
    PointerMotion { local: (f32, f32) },
    PointerLeave,
    PointerDown { button: u32, local: (f32, f32), serial: u32 },
    PointerUp { button: u32, local: (f32, f32), serial: u32 },
    Scroll(Scroll),
    Key(KeyEvent),
    FocusIn { cause: FocusCause },
    FocusOut,
    /// Space/Enter, or a click that completed inside — the "activate" GTK means.
    Activate,
    /// A popup this controller opened was dismissed by the compositor.
    PopupDone,
    /// The window's size or `SurfaceStates` changed.
    Configure { size: (u32, u32), states: SurfaceStates },
}

pub struct EventCx<'a, Msg> {
    pub node: &'a Node,
    pub handlers: &'a Handlers<Msg>,
    pub tree: &'a LayoutTree,
    pub styles: &'a StyleMap,
    pub focus: &'a mut FocusRing,
    pub clipboard: &'a mut Clipboard,
    pub icons: &'a mut IconTheme,
    pub fonts: &'a mut FontDatabase,
    pub clock: &'a Rc<dyn Clock>,
    /// Side effects a controller may request without going through the model.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
    /// `true` if the event was handled here and must not bubble further.
    pub handled: bool,
}
```

Dispatch order is GTK's: capture (root → target), target, bubble
(target → root). A controller that sets `cx.handled = true` stops the phase it
is in. Unhandled keys fall through to the window's focus navigation (§3.5).

### 4.7 `Cmd` and `App`

```rust
pub enum Cmd<Msg> {
    None,
    Batch(Vec<Cmd<Msg>>),
    /// Fires once, on the animation clock.
    After(Duration, Rc<dyn Fn() -> Msg>),
    Copy(String),
    Paste(Rc<dyn Fn(Option<String>) -> Msg>),
    SetPrimary(String),
    Primary(Rc<dyn Fn(Option<String>) -> Msg>),
    OpenPopup { anchor: PopupAnchorPoint, positioner: Positioner, view: Rc<dyn Fn() -> View<Msg>> },
    ClosePopup(PopupKey),
    Focus(Node),
    SetTitle(String),
    Minimize, ToggleMaximized, CloseWindow,
    Quit,
}

pub struct App<M, Msg> { /* model, update, view, instances, focus, popups, window */ }

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    pub fn new(
        model: M,
        update: fn(&mut M, Msg) -> Cmd<Msg>,
        view: fn(&M) -> View<Msg>,
    ) -> Self;

    /// The loop: `pump` → route to controllers → collect `Msg`s → `update` each
    /// in order → `view` → `reconcile` → restyle/relayout/repaint dirty nodes →
    /// `AnimationState` ticks feed frames. Messages are **queued, never
    /// nested**: an `update` that produces a `Cmd` producing a `Msg` enqueues
    /// it; the fold never re-enters.
    pub fn run(self, surface: Surface) -> Result<(), AppError>;

    /// Same loop against an offscreen skia surface with no Wayland connection,
    /// driven by a scripted event list on a `ManualClock`. This is how the
    /// counter-app test and every controller unit test run.
    pub fn run_offscreen(self, size: (u32, u32), clock: Rc<ManualClock>,
                         script: Vec<ScriptStep<Msg>>) -> Result<Frames, AppError>;
}

pub enum ScriptStep<Msg> {
    Event(InputEvent),
    Advance(Duration),
    Message(Msg),
    Capture,
}

/// One RGBA frame per `ScriptStep::Capture`, plus the final model-visible state.
pub struct Frames { /* frames: Vec<(u32, u32, Vec<u8>)> */ }

impl Frames {
    #[must_use] pub fn len(&self) -> usize;
    #[must_use] pub fn is_empty(&self) -> bool;
    #[must_use] pub fn pixel(&self, frame: usize, x: u32, y: u32) -> Option<(u8, u8, u8, u8)>;
}

#[derive(Debug)]
pub enum AppError { Surface(SurfaceError), Layout(LayoutError) }
```

**`Msg` wiring, end to end.** A builder stores a `Handler<Msg>` in the `View`'s
`Handlers`. `reconcile` copies the `Handlers` onto the `Instance` (a
`SetHandlers` op — handlers are replaced wholesale every frame, never diffed;
they are closures over the current model). A controller reacting to an `Event`
calls `cx.handlers.fire_*` and returns whatever came back. `App::run` drains
those into a FIFO and folds them with `update`, one at a time, then re-runs
`view` **once** per drained batch.

### 4.8 P4 tests

Reconciler property tests: for random keyed edit sequences, every surviving key
maps to a `Node` with the same `tree_id`; the op list contains no redundant
`Move`/`SetProp`; a removed key's controller is dropped exactly once.
Controller units on a `ManualClock`. An offscreen "counter app" driven by
`run_offscreen` with pixel assertions on the label. Mutation checks on the LCS
and on `Props::diff`.

---

## 5. Widget catalogue [P5 · P6]

**Fixtures.** Each widget vendors its GTK 4.22.4 "CSS nodes" block verbatim
(box-drawing art included) at
`ui/tests/fixtures/gtk4.22-node-trees/<kind_snake_case>.txt`, sourced from
`.superpowers/m3-plan-notes/gtk-widget-nodes.md`, which extracted them from the
installed `gtk4 1:4.22.4-1` sources. `ui/tests/node_trees.rs` renders the
retained tree with

```rust
/// Renders a built widget's retained `Node` subtree in GTK's own notation:
/// `name.class`, `[optional]`, `├──`/`╰──`, `┊` for repetition, `<child>` for
/// an application child. Nested widget subtrees are rendered through, not
/// stopped at (a `ColumnView` shows its `listview`; a `PopoverMenuBar` shows
/// its `item`s).
pub fn node_tree_of(kind: Kind, props: &Props) -> String;
```

and diffs it against the fixture. Notation legend, GTK's own: `name` = node,
`name.class` = always-present class, `name[.class]` = state-dependent class,
`[name]` = configuration-dependent subnode, `<child>` = app child subtree,
`┊`/`⋮` = repetition, `╰──` = last child.

**Fixtures key on `Kind`, never on the node name** — `Button`/`ToggleButton`/
`LinkButton` all render `button`, `Box`/`CenterBox` both render `box`.

**Per-widget gate:** one Adwaita pixel test at rest and one in an interaction
state, plus the node-tree conformance test. Both gates are in
`ui/tests/gallery_gate.rs`/`interaction_gate.rs` (P8), not per widget file.

**Universal props and handlers.** Every widget below inherits §4.1's
`GtkWidget`-universal set (`class`, `id`, `visible`, `sensitive`, `focusable`,
`tooltip`, `halign`, `valign`, `hexpand`, `vexpand`, `margin`,
`width_request`, `height_request`, `cursor`, `opacity`) and
`.on_focus_in`/`.on_focus_out`. Only widget-specific additions are listed.

**Every controller struct additionally carries** `pressed: bool` (for `:active`)
and `hovered: bool` (for `:hover`) unless it has no pointer behaviour at all;
those two are omitted from the listings for brevity.

### 5.1 P5 · Display — `ui/src/widgets/{label,spinner,…}.rs`

#### Label — `Kind::Label` — node name `label`
```
label
├── [selection]
├── [link]
┊
╰── [link]
```
- builder `label(text: &str)`; `.wrap(bool) .wrap_mode(WrapMode) .ellipsize(Ellipsize) .lines(i32) .xalign(f32) .yalign(f32) .markup(bool) .selectable(bool) .use_underline(bool) .width_chars(i32) .max_width_chars(i32)`; handlers `.on_activate_link(|uri| Msg)`
- controller `struct LabelC { layout: TextLayout, spans: Vec<MarkupSpan>, selection: Option<Range<usize>>, links: Vec<(Range<usize>, Rc<str>)>, hovered_link: Option<usize>, selection_node: Option<Node>, link_nodes: Vec<Node> }`

#### Spinner — `Kind::Spinner` — node name `spinner`
```
spinner
```
`:checked` while spinning (GTK's own divergence).
- builder `spinner()`; `.spinning(bool)`
- controller `struct SpinnerC { spinning: bool, phase: f32, started: Duration }` — `tick` advances `phase`; `next_deadline` is one frame while spinning, `None` otherwise.

#### Statusbar — `Kind::Statusbar` — node name `statusbar`
```
statusbar
```
Deprecated upstream (4.10); kept — Adwaita still styles it and spec §5 lists it (ruling R1).
- builder `statusbar()`; `.text(&str)`
- controller `struct StatusbarC { stack: Vec<(u32, String)>, label: Node }`

#### LevelBar — `Kind::LevelBar` — node name `levelbar`
```
levelbar[.discrete]
╰── trough
    ├── block.filled.level-name
    ┊
    ├── block.empty
    ┊
```
- builder `level_bar(value: f64)`; `.min_value(f64) .max_value(f64) .mode(LevelBarMode) .offset(&str, f64) .inverted(bool)`
- controller `struct LevelBarC { value: f64, min: f64, max: f64, discrete: bool, offsets: Vec<(Rc<str>, f64)>, trough: Node, blocks: Vec<Node> }`

#### ProgressBar — `Kind::ProgressBar` — node name `progressbar`
```
progressbar[.osd]
├── [text]
╰── trough[.empty][.full]
    ╰── progress[.pulse]
```
- builder `progress_bar(fraction: f64)`; `.text(&str) .show_text(bool) .inverted(bool) .pulse_step(f64) .ellipsize(Ellipsize)`
- controller `struct ProgressBarC { fraction: f64, pulsing: bool, pulse_pos: f64, trough: Node, progress: Node, text: Option<Node> }` — `tick` drives the pulse block.

#### InfoBar — `Kind::InfoBar` — node name `infobar`
```
infobar[.info][.warning][.error][.question]
```
Deprecated upstream (4.10); kept (R1). Close button carries `.close`.
- builder `info_bar()`; `.message_type(MessageType) .revealed(bool) .show_close_button(bool)`; handlers `.on_response(|usize| Msg) .on_close(Msg)`
- controller `struct InfoBarC { revealed: bool, reveal_progress: f32, close_button: Option<Node> }`

#### Scrollbar — `Kind::Scrollbar` — node name `scrollbar`
```
scrollbar
╰── range[.fine-tune]
    ╰── trough
        ╰── slider
```
Positional classes `.horizontal`/`.vertical`, and `.overlay-indicator`/`.dragging`/`.hovering` set by a parent `ScrolledWindow`.
- builder `scrollbar(orientation: Orientation)`; `.value(f64) .lower(f64) .upper(f64) .page_size(f64) .step_increment(f64) .page_increment(f64) .inverted(bool)`; handlers `.on_value_changed(|f64| Msg)`
- controller `struct ScrollbarC { value: f64, adj: Adjustment, drag: Option<f32>, fine_tune: bool, range: Node, trough: Node, slider: Node }` — Shift-drag or long-press enables fine-tune (`GtkRange`'s rule).

#### Image — `Kind::Image` — node name `image`
```
image[.normal-icons][.large-icons]
```
- builder `image(icon: IconRef)`; `.icon_name(&str) .file(&Path) .pixel_size(i32) .icon_size(IconSize) .use_fallback(bool)`
- controller `struct ImageC { icon: IconRef, resolved: Option<icons::Handle>, pixel_size: i32 }` — `paint` delegates to `paint::icon::paint_icon`.

#### Picture — `Kind::Picture` — node name `picture`
```
picture
```
- builder `picture(path: &Path)`; `.content_fit(ContentFit) .can_shrink(bool) .alternative_text(&str)`
- controller `struct PictureC { source: PictureSource, decoded: Option<Rc<DecodedImage>>, fit: ContentFit }`

#### Separator — `Kind::Separator` — node name `separator`
```
separator.horizontal
separator.vertical
```
- builder `separator(orientation: Orientation)`
- controller `struct SeparatorC { orientation: Orientation }` (no pointer state)

#### TextView — `Kind::TextView` — node name `textview`
```
textview.view
├── border.top
├── border.left
├── text
│   ╰── [selection]
├── border.right
├── border.bottom
╰── [window.popup]
```
- builder `text_view(text: &str)`; `.editable(bool) .wrap_mode(WrapMode) .monospace(bool) .cursor_visible(bool) .left_margin(i32) .right_margin(i32) .top_margin(i32) .bottom_margin(i32) .enable_undo(bool)`; handlers `.on_change(|&str| Msg)`
- controller `struct TextViewC { buffer: String, layout: TextLayout, cursor: usize, anchor: Option<usize>, scroll: (f32, f32), undo: UndoStack, kinetic: Kinetic, text_node: Node, selection_node: Option<Node> }` — plain text only; editing keys per `GtkText`'s table (§5.3).

#### Scale — `Kind::Scale` — node name `scale`
```
scale[.fine-tune][.marks-before][.marks-after]
├── [value][.top][.right][.bottom][.left]
├── marks.top
│   ├── mark
│   ┊    ├── [label]
│   ┊    ╰── indicator
┊   ┊
│   ╰── mark
├── marks.bottom
│   ├── mark
│   ┊    ├── indicator
│   ┊    ╰── [label]
┊   ┊
│   ╰── mark
╰── trough
    ├── [fill]
    ├── [highlight]
    ╰── slider
```
- builder `scale(lower: f64, upper: f64)`; `.value(f64) .orientation(Orientation) .digits(i32) .draw_value(bool) .value_pos(Position) .mark(f64, Position, Option<&str>) .show_fill_level(bool) .fill_level(f64) .inverted(bool)`; handlers `.on_value_changed(|f64| Msg)`
- controller `struct ScaleC { value: f64, adj: Adjustment, marks: Vec<Mark>, drag: Option<f64>, fine_tune: bool, trough: Node, slider: Node, highlight: Node, fill: Option<Node>, value_node: Option<Node>, mark_nodes: Vec<Node> }`

#### DrawingArea — `Kind::DrawingArea` — node name `widget`
```
widget
```
GTK sets no CSS name; the node is `GtkWidget`'s default `widget`. Style it with a class you add.
- builder `drawing_area(draw: impl Fn(&mut Canvas<'_>, layout::Rect) + 'static)`; `.content_width(i32) .content_height(i32)`; handlers `.on_resize(|(i32, i32)| Msg)` via `EventKind::Change`
- controller `struct DrawingAreaC { draw: Rc<dyn Fn(&mut Canvas<'_>, layout::Rect)>, content: (i32, i32), last_size: (f32, f32) }` — `paint` calls `draw`; `measure` returns `content`.

#### WindowControls — `Kind::WindowControls` — node name `windowcontrols`
```
windowcontrols[.start][.end][.empty][.native]
├── [image.icon]
├── [button.minimize]
├── [button.maximize]
╰── [button.close]
```
Layout string rule, verbatim from `update_window_buttons`: `side` picks the half of `gtk-decoration-layout` before (`start`) or after (`end`) the colon; the half is split on `,` and walked **in order**; `icon` only when sovereign (`!modal && transient_for.is_none()`) and an icon exists; `minimize` sovereign only; `maximize` only when `resizable` **and** sovereign, icon `window-maximize-symbolic` or `window-restore-symbolic` when maximized; `close` only when `deletable`; **`menu` produces no child in 4.22.4**. Nothing emitted ⇒ add `.empty`. All three buttons are `focusable = false` and never enter the Tab ring. Actions route to `xdg_toplevel` requests, not to a GTK window: `Cmd::Minimize`, `Cmd::ToggleMaximized`, `Cmd::CloseWindow`.
- builder `window_controls(side: Side)`; `.decoration_layout(&str)` (overrides the setting)
- controller `struct WindowControlsC { side: Side, layout: Rc<str>, buttons: Vec<(WindowButton, Node)>, empty: bool }` — rebuilt on a `Configure` whose `SurfaceStates` changed `MAXIMIZED`, and on a settings change.

#### Calendar — `Kind::Calendar` — node name `calendar`
```
calendar.view
├── header
│   ├── button
│   ├── stack.month
│   ├── button
│   ├── button
│   ├── label.year
│   ╰── button
╰── grid
    ╰── label[.day-name][.week-number][.day-number][.other-month][.today]
```
- builder `calendar(year: i32, month: u32, day: u32)`; `.show_day_names(bool) .show_heading(bool) .show_week_numbers(bool) .mark_day(u32)`; handlers `.on_date_selected(|(i32, u32, u32)| Msg)`
- controller `struct CalendarC { shown: (i32, u32), selected: (i32, u32, u32), marks: u32, header: Node, grid: Node, day_nodes: Vec<Node> }` — arrow keys move the selection within the grid.

#### Popover — `Kind::Popover` — node name `popover`
```
popover.background[.menu]
├── arrow
╰── contents
    ╰── <child>
```
**Ruling R4:** `Popover` is a **P5** deliverable although spec §1 files popovers
under P6, because `MenuButton`, `DropDown`, `ColorDialogButton` and
`FontDialogButton` (all P5) embed one. P6 owns `PopoverMenu`/`PopoverMenuBar`
on top of it.
A popover is backed by a real `Surface::Popup` (§3.2) when `autohide` (GTK's
"modal") is set, taking `xdg_popup.grab`; a non-autohide popover renders into
the parent window's own tree instead, with no grab.
- builder `popover(child: View<Msg>)`; `.autohide(bool) .has_arrow(bool) .position(Position) .offset(i32, i32) .pointing_to(layout::Rect)`; handlers `.on_close(Msg)`
- controller `struct PopoverC { open: bool, autohide: bool, has_arrow: bool, position: Position, popup: Option<PopupKey>, arrow: Node, contents: Node }`

### 5.2 P5 · Buttons

#### Button — `Kind::Button` — node name `button`
```
button[.image-button][.text-button][.flat][.keyboard-activating]
```
`.image-button`/`.text-button` are set from the actual content;
`.keyboard-activating` is added for the duration of a Space/Enter activation.
`.suggested-action`, `.destructive-action`, `.circular` are app-supplied.
- builder `button(label: &str)` / `button_from(child: View<Msg>)`; `.label(&str) .icon(IconRef) .has_frame(bool) .use_underline(bool)`; handlers `.on_click(Msg)` (= `EventKind::Click`), `.on_activate(Msg)`
- controller `struct ButtonC { label: Option<Node>, image: Option<Node>, activating_until: Option<Duration> }`

#### ToggleButton — `Kind::ToggleButton` — node name `button`, class `.toggle`
```
button.toggle
```
- builder `toggle_button(label: &str)`; `.active(bool) .group(&str)`; handlers `.on_toggle(|bool| Msg)`
- controller `struct ToggleButtonC { active: bool, group: Option<Rc<str>>, label: Option<Node> }` — a grouped toggle behaves radio-like: activating one clears its siblings.

#### LinkButton — `Kind::LinkButton` — node name `button`, class `.link`
```
button.link
```
Actions: `clipboard.copy` (copies the uri), `menu.popup`.
Shortcut: `Shift+F10` or `Menu` opens the context menu.
- builder `link_button(uri: &str, label: &str)`; `.visited(bool)`; handlers `.on_activate_link(|uri| Msg)`
- controller `struct LinkButtonC { uri: Rc<str>, visited: bool, label: Node, menu: Option<PopupKey> }`

#### CheckButton — `Kind::CheckButton` — node name `checkbutton`
```
checkbutton[.text-button][.grouped]
├── check
╰── [label]
```
The `check` subnode uses `-gtk-icon-source: builtin` (`Builtin::Check` /
`Builtin::CheckIndeterminate`); a grouped check button renders `Builtin::Radio`
under the same `check` node, per GTK.
- builder `check_button(label: &str)`; `.active(bool) .inconsistent(bool) .group(&str) .use_underline(bool)`; handlers `.on_toggle(|bool| Msg)`
- controller `struct CheckButtonC { active: bool, inconsistent: bool, group: Option<Rc<str>>, check: Node, label: Option<Node> }`

#### MenuButton — `Kind::MenuButton` — node name `menubutton`
```
menubutton
╰── button.toggle
    ╰── <content>
         ╰── [arrow]
```
- builder `menu_button(label: &str)`; `.icon(IconRef) .always_show_arrow(bool) .direction(ArrowDirection) .has_frame(bool) .primary(bool)`; children = the popover content
- controller `struct MenuButtonC { open: bool, button: Node, arrow: Option<Node>, popover: PopoverC }`

#### Switch — `Kind::Switch` — node name `switch`
```
switch
├── image
├── image
╰── slider
```
- builder `switch(active: bool)`; `.state(bool)`; handlers `.on_toggle(|bool| Msg)`
- controller `struct SwitchC { active: bool, drag: Option<f32>, slide: f32, slider: Node, on_image: Node, off_image: Node }` — drag past the midpoint toggles; `slide` animates on the clock.

#### DropDown — `Kind::DropDown` — node name `dropdown`
```
dropdown
├── button.toggle
│   ╰── <content>
│        ╰── [arrow]
╰── popover.menu
    ╰── contents
        ├── [entry.search]
        ╰── listview
            ╰── row[.activatable]
            ┊
```
GTK's own block says only "a single node `dropdown`, with the button and popover
nodes as children"; the expansion above is the tree it actually renders and is
what the fixture holds.
- builder `drop_down(items: &[&str])` / `drop_down_from(items: Rc<[ListItem]>)`; `.selected(usize) .enable_search(bool) .show_arrow(bool) .search_match_mode(MatchMode)`; handlers `.on_selected(|usize| Msg)`
- controller `struct DropDownC { items: Rc<[ListItem]>, selected: usize, open: bool, search: String, filtered: Vec<usize>, button: Node, popover: PopoverC, list: ListViewC }`

#### Switch group note
`ToggleButton`/`CheckButton` groups are keyed by the `Group` prop (an `Rc<str>`
name), not by object identity — the reactive layer has no stable widget handles
to group by.

#### ColorDialogButton — `Kind::ColorDialogButton` — node name `colorbutton`
```
colorbutton
╰── button.color
    ╰── [content]
```
- builder `color_dialog_button(rgba: Rgba)`; `.with_alpha(bool) .dialog(ColorDialogSpec)`; handlers `.on_change(|Rgba| Msg)` via `EventKind::ValueChanged`
- controller `struct ColorDialogButtonC { rgba: Rgba, dialog_open: bool, button: Node, swatch: Node }`

#### ColorDialog — `Kind::ColorDialog` — node name `window`, classes `.dialog`
```
window.dialog
╰── colorchooser
    ╰── colorswatch
    ┊
```
Not a widget in GTK (a `GObject` that launches a deprecated `GtkColorChooserDialog`); icedtea builds the chooser body itself under `window.dialog`.
- builder `color_dialog(rgba: Rgba)`; `.title(&str) .modal(bool) .with_alpha(bool)`; handlers `.on_response(|usize| Msg) .on_change(|Rgba| Msg)`
- controller `struct ColorDialogC { rgba: Rgba, palette: Vec<Rgba>, custom: Option<Rgba>, swatches: Vec<Node> }`

#### FontDialogButton — `Kind::FontDialogButton` — node name `fontbutton`
```
fontbutton
╰── button.font
    ╰── [content]
```
- builder `font_dialog_button(desc: &str)`; `.use_font(bool) .use_size(bool) .level(FontLevel)`; handlers `.on_change(|&str| Msg)`
- controller `struct FontDialogButtonC { desc: Rc<str>, dialog_open: bool, button: Node, label: Node }`

#### FontDialog — `Kind::FontDialog` — node name `window`, classes `.dialog`
```
window.dialog
╰── fontchooser
```
Same note as `ColorDialog`.
- builder `font_dialog(desc: &str)`; `.title(&str) .modal(bool) .language(&str)`; handlers `.on_response(|usize| Msg) .on_change(|&str| Msg)`
- controller `struct FontDialogC { families: Vec<Rc<str>>, selected: Option<usize>, size: f32, preview: String, list: ListViewC }`

### 5.3 P5 · Entries

All five embed GTK's `GtkText` engine. **`GtkText`'s own tables are the single
source for editing behaviour**, not the per-entry pages.

Shortcuts (verbatim from `gtk/gtktext.c:105`): `Shift+F10` / `Menu` opens the
context menu; `Ctrl+A` or `Ctrl+/` selects all; `Ctrl+Shift+A` or `Ctrl+\`
unselects; `Ctrl+Z` undo; `Ctrl+Y` or `Ctrl+Shift+Z` redo;
`Ctrl+Shift+T` toggles text direction; `Clear` clears.
Actions: `clipboard.{copy,cut,paste}`, `menu.popup`, `misc.toggle-visibility`,
`misc.toggle-direction`, `selection.{delete,select-all}`,
`text.{undo,redo,clear}`. (`misc.insert-emoji` is M6.)

The shared `text` subnode's own tree:
```
text[.read-only]
├── placeholder
├── undershoot.left
├── undershoot.right
├── [selection]
├── [block-cursor]
╰── [window.popup]
```
(Touch `cursor-handle` subnodes are recorded in the fixture and rendered only
when touch selection is active.)

Shared editing state, embedded by all five controllers:

```rust
pub struct TextEditState {
    pub buffer: String,
    pub layout: TextLayout,
    pub cursor: usize,          // byte offset
    pub anchor: Option<usize>,  // selection anchor; None ⇒ no selection
    pub scroll_offset: f32,
    pub undo: UndoStack,
    pub overwrite: bool,
    pub visibility: bool,       // PasswordEntry / `misc.toggle-visibility`
    pub max_length: Option<usize>,
    pub text_node: Node,
    pub placeholder_node: Node,
    pub selection_node: Option<Node>,
}

pub struct UndoStack { /* Vec<Edit>, cursor: usize, coalesce_until: Duration */ }
```

#### Entry — `Kind::Entry` — node name `entry`
```
entry[.flat][.warning][.error]
├── text[.readonly]
├── image.left
├── image.right
╰── [progress[.pulse]]
```
- builder `entry(text: &str)`; `.placeholder(&str) .editable(bool) .max_length(i32) .visibility(bool) .icon_left(IconRef) .icon_right(IconRef) .progress_fraction(f64) .progress_pulse_step(f64) .activates_default(bool) .enable_undo(bool) .xalign(f32) .width_chars(i32)`; handlers `.on_change(|&str| Msg) .on_activate(Msg) .on_icon_press(|usize| Msg)`
- controller `struct EntryC { edit: TextEditState, icons: [Option<Node>; 2], progress: Option<Node>, menu: Option<PopupKey> }`

#### SearchEntry — `Kind::SearchEntry` — node name `entry`, class `.search`
```
entry.search
╰── text
```
- builder `search_entry(text: &str)`; `.placeholder(&str) .search_delay(u32)`; handlers `.on_search(|&str| Msg) .on_change(|&str| Msg) .on_activate(Msg)`
- controller `struct SearchEntryC { edit: TextEditState, delay_ms: u32, pending_since: Option<Duration> }` — `tick` fires `EventKind::Search` once `delay_ms` has elapsed with no further edit.

#### PasswordEntry — `Kind::PasswordEntry` — node name `entry`, class `.password`
```
entry.password
╰── text
    ├── image.caps-lock-indicator
    ┊
```
- builder `password_entry(text: &str)`; `.placeholder(&str) .show_peek_icon(bool) .activates_default(bool)`; handlers `.on_change(|&str| Msg) .on_activate(Msg)`
- controller `struct PasswordEntryC { edit: TextEditState, peek: bool, caps_lock: bool, caps_node: Option<Node>, peek_node: Option<Node> }` — `caps_lock` comes from `Mods::CAPS`.

#### SpinButton — `Kind::SpinButton` — node name `spinbutton`
```
spinbutton.horizontal
├── text
│    ├── undershoot.left
│    ╰── undershoot.right
├── button.down
╰── button.up
```
Step buttons paint `Builtin::SpinPlus`/`Builtin::SpinMinus`.
- builder `spin_button(value: f64, lower: f64, upper: f64)`; `.step(f64) .page(f64) .digits(u32) .wrap(bool) .numeric(bool) .snap_to_ticks(bool) .climb_rate(f64) .orientation(Orientation)`; handlers `.on_value_changed(|f64| Msg)`
- controller `struct SpinButtonC { edit: TextEditState, adj: Adjustment, digits: u32, wrap: bool, repeat: Option<RepeatTimer>, up: Node, down: Node }` — `tick` drives the held-button repeat with `climb_rate` acceleration; Up/Down and Page Up/Down step from the keyboard; scroll steps.

#### EditableLabel — `Kind::EditableLabel` — node name `editablelabel`
```
editablelabel[.editing]
╰── stack
    ├── label
    ╰── text
```
- builder `editable_label(text: &str)`; `.editing(bool)`; handlers `.on_change(|&str| Msg)`
- controller `struct EditableLabelC { edit: TextEditState, editing: bool, committed: String, stack: Node, label: Node }` — Enter commits and leaves editing; Escape reverts to `committed`.

### 5.4 P6 · Containers

#### Box — `Kind::Box` — node name `box`
```
box
```
- builder `box_(orientation: Orientation, children)`; `.spacing(i32) .homogeneous(bool) .baseline_position(BaselinePosition)`
- controller `struct BoxC { orientation: Orientation, spacing: f32, homogeneous: bool }` — sets `Container::Box`.

#### Grid — `Kind::Grid` — node name `grid`
```
grid
```
Children carry `ChildLayout::grid` (`column`, `row`, `column_span`, `row_span`); `border-spacing` maps to `row_spacing`/`column_spacing` when the props are unset.
- builder `grid(children)`; `.row_spacing(i32) .column_spacing(i32) .row_homogeneous(bool) .column_homogeneous(bool) .baseline_row(i32)`; per-child `.at(col, row) .span(cols, rows)`
- controller `struct GridC { columns: u16, rows: u16, spacing: (f32, f32), homogeneous: (bool, bool) }`

#### CenterBox — `Kind::CenterBox` — node name `box`
```
box
```
Exactly three children (start, center, end); the **first** child is allocated per text direction (left under LTR, right under RTL).
- builder `center_box(start, center, end)`; `.orientation(Orientation) .shrink_center_last(bool) .baseline_position(BaselinePosition)`
- controller `struct CenterBoxC { orientation: Orientation, shrink_center_last: bool }` — sets `Container::Center`.

#### ScrolledWindow — `Kind::ScrolledWindow` — node name `scrolledwindow`
```
scrolledwindow[.frame]
├── <child>
├── [overshoot.top][.bottom][.left][.right]
├── [undershoot.top][.bottom][.left][.right]
├── [scrollbar.horizontal[.overlay-indicator][.dragging][.hovering]]
├── [scrollbar.vertical[.overlay-indicator][.dragging][.hovering]]
╰── [junction]
```
GTK's block names the subnodes prosaically (`overshoot`, `undershoot`, `junction`, plus positional and overlay classes it sets on its scrollbars); the tree above is the fixture.
- builder `scrolled_window(child)`; `.hscrollbar_policy(Policy) .vscrollbar_policy(Policy) .has_frame(bool) .min_content_width(i32) .min_content_height(i32) .max_content_width(i32) .max_content_height(i32) .propagate_natural_width(bool) .propagate_natural_height(bool) .kinetic_scrolling(bool) .overlay_scrolling(bool)`; handlers `.on_scrolled(|(f64, f64)| Msg)`
- controller `struct ScrolledWindowC { offset: (f32, f32), extent: (f32, f32), kinetic: Kinetic, overshoot: (f32, f32), hbar: Option<ScrollbarC>, vbar: Option<ScrollbarC>, hovering_until: Option<Duration>, junction: Option<Node> }`

#### Paned — `Kind::Paned` — node name `paned`
```
paned
├── <child>
├── separator[.wide]
╰── <child>
```
- builder `paned(orientation: Orientation, start, end)`; `.position(i32) .position_set(bool) .wide_handle(bool) .resize_start(bool) .resize_end(bool) .shrink_start(bool) .shrink_end(bool)`; handlers `.on_value_changed(|f64| Msg)`
- controller `struct PanedC { position: f32, drag: Option<f32>, wide: bool, separator: Node }` — the separator is focusable; arrows move it by one step, Home/End to the extremes.

#### Frame — `Kind::Frame` — node name `frame`
```
frame
├── <label widget>
╰── <child>
```
- builder `frame(child)`; `.label(&str) .label_xalign(f32) .label_widget(View<Msg>)`
- controller `struct FrameC { label: Option<Node>, label_xalign: f32 }`

#### Expander — `Kind::Expander` — node name `expander-widget`
```
expander-widget
╰── box
    ├── title
    │   ├── expander
    │   ╰── <label widget>
    ╰── <child>
```
**The widget's node is `expander-widget`; the `expander` node inside it is the arrow** (easy to get wrong). The arrow paints `Builtin::Expander`, rotated by `-gtk-icon-transform` when expanded.
- builder `expander(label: &str, child)`; `.expanded(bool) .use_underline(bool) .resize_toplevel(bool)`; handlers `.on_expanded(|bool| Msg)`
- controller `struct ExpanderC { expanded: bool, progress: f32, title: Node, arrow: Node, content: Node }` — `tick` animates `progress`; Space/Enter toggles.

#### SearchBar — `Kind::SearchBar` — node name `searchbar`
```
searchbar
╰── revealer
    ╰── box
         ├── [child]
         ╰── [button.close]
```
- builder `search_bar(child)`; `.search_mode(bool) .show_close_button(bool) .key_capture(bool)`; handlers `.on_toggle(|bool| Msg)`
- controller `struct SearchBarC { revealed: bool, progress: f32, revealer: Node, close: Option<Node> }` — Escape closes; typing while `key_capture` opens it and forwards the key.

#### ActionBar — `Kind::ActionBar` — node name `actionbar`
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
- builder `action_bar()`; `.revealed(bool)`; `.pack_start(View) .pack_end(View) .center(View)`
- controller `struct ActionBarC { revealed: bool, progress: f32, start: Node, center: Option<Node>, end: Node }`

#### HeaderBar — `Kind::HeaderBar` — node name `headerbar`
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
`decoration_layout` overrides `gtk-decoration-layout` and is bound down into both `WindowControls`; `.native` is added when the window is not client-decorated (icedtea's compositor draws SSD, so this is the common case for real windows and the *un*common case in the gallery).
- builder `header_bar()`; `.title(&str) .subtitle(&str) .title_widget(View) .show_title_buttons(bool) .decoration_layout(&str)`; `.pack_start(View) .pack_end(View)`
- controller `struct HeaderBarC { start: Node, end: Node, title: Node, controls: [Option<WindowControlsC>; 2], layout: Rc<str> }` — double-click on the handle runs `gtk-titlebar-double-click` (default `toggle-maximize`); middle-click runs `gtk-titlebar-middle-click` (default `none`).

#### Notebook — `Kind::Notebook` / `Kind::NotebookTab` — node name `notebook`
```
notebook
├── header.top
│   ├── [<action widget>]
│   ├── tabs
│   │   ├── [arrow]
│   │   ├── tab
│   │   │   ╰── <tab label>
┊   ┊   ┊
│   │   ├── tab[.reorderable-page]
│   │   │   ╰── <tab label>
│   │   ╰── [arrow]
│   ╰── [<action widget>]
│
╰── stack
    ├── <child>
    ┊
    ╰── <child>
```
`header` carries `.top`/`.bottom`/`.left`/`.right` from the tab position.
- builder `notebook(pages)`; `.page(usize) .tab_pos(Position) .scrollable(bool) .show_tabs(bool) .show_border(bool)`; per-tab `notebook_tab(label, child).reorderable(bool).detachable(bool)`; handlers `.on_page_changed(|usize| Msg) .on_reordered(|(usize, usize)| Msg)`
- controller `struct NotebookC { page: usize, tabs: Vec<Node>, scroll: f32, drag: Option<(usize, f32)>, header: Node, stack: Node, arrows: [Option<Node>; 2] }` — `Ctrl+PageUp/Down` and `Alt+1..9` switch; drag reorders when `reorderable`.

#### Overlay — `Kind::Overlay` — node name `overlay`
```
overlay
├── <child>
╰── <overlay child>[.left][.right][.top][.bottom]
```
An overlay child whose alignment puts it at an edge gets that positional class.
- builder `overlay(child)`; `.overlay(View<Msg>) .measure_overlay(bool) .clip_overlay(bool)`
- controller `struct OverlayC { overlays: Vec<(Node, bool, bool)> }`

#### Stack — `Kind::Stack` / `Kind::StackPage` — node name `stack`
```
stack
```
- builder `stack(pages)`; `.visible_child(&str) .transition_type(StackTransition) .transition_duration(u32) .hhomogeneous(bool) .vhomogeneous(bool) .interpolate_size(bool)`; per-page `stack_page(name, title, child).icon(IconRef).needs_attention(bool)`; handlers `.on_change(|&str| Msg)`
- controller `struct StackC { visible: usize, outgoing: Option<usize>, progress: f32, transition: StackTransition, duration_ms: u32, pages: Vec<StackPageState> }` — the transition runs on M2's `AnimationState`, not a bespoke timer.

#### StackSwitcher — `Kind::StackSwitcher` — node name `stackswitcher`, class `.stack-switcher`
```
stackswitcher.stack-switcher
├── button[.needs-attention]
┊
╰── button[.needs-attention]
```
- builder `stack_switcher(pages: Rc<[StackPageInfo]>)`; `.selected(usize) .orientation(Orientation)`; handlers `.on_selected(|usize| Msg)`
- controller `struct StackSwitcherC { selected: usize, buttons: Vec<Node> }`

#### StackSidebar — `Kind::StackSidebar` — node name `stacksidebar`, class `.sidebar`
```
stacksidebar.sidebar
╰── scrolledwindow
    ╰── list.navigation-sidebar
        ╰── row[.needs-attention]
        ┊
```
- builder `stack_sidebar(pages: Rc<[StackPageInfo]>)`; `.selected(usize)`; handlers `.on_selected(|usize| Msg)`
- controller `struct StackSidebarC { selected: usize, list: ListBoxC }`

### 5.5 P6 · Lists

#### ListBox / ListBoxRow — `Kind::ListBox` / `Kind::ListBoxRow` — node names `list` / `row`
```
list[.separators][.rich-list][.navigation-sidebar][.boxed-list]
╰── row[.activatable]
```
- builder `list_box(rows)`; `.selection_mode(SelectionMode) .show_separators(bool) .activate_on_single_click(bool)`; per-row `list_box_row(child).activatable(bool).selectable(bool)`; handlers `.on_selected(|usize| Msg) .on_activate(|usize| Msg)`
- controller `struct ListBoxC { rows: Vec<Node>, selection: Selection, cursor: Option<usize>, mode: SelectionMode, activate_single: bool }` — arrows move the cursor, Space toggles under multi-selection, Enter activates; `Ctrl+A` selects all under `Multiple`.

#### FlowBox / FlowBoxChild — `Kind::FlowBox` / `Kind::FlowBoxChild` — node names `flowbox` / `flowboxchild`
```
flowbox
├── flowboxchild
│   ╰── <child>
├── flowboxchild
│   ╰── <child>
┊
╰── [rubberband]
```
- builder `flow_box(children)`; `.selection_mode(SelectionMode) .min_children_per_line(u32) .max_children_per_line(u32) .row_spacing(u32) .column_spacing(u32) .homogeneous(bool) .activate_on_single_click(bool)`; handlers `.on_selected(|usize| Msg) .on_activate(|usize| Msg)`
- controller `struct FlowBoxC { children: Vec<Node>, selection: Selection, cursor: Option<usize>, mode: SelectionMode, rubberband: Option<(layout::Rect, Node)> }`

#### ListView — `Kind::ListView` — node name `listview`
```
listview[.separators][.rich-list][.navigation-sidebar][.data-table]
├── row[.activatable]
│
├── row[.activatable]
│
┊
╰── [rubberband]
```
Rows are **recycled**: the controller keeps a pool of `row` nodes sized to the
viewport plus overscan and rebinds them on scroll; a rebind is a
`Controller::set_prop` on the row's child instance, never a `reconcile`
insert/remove, so identity (and animation) survives scrolling.
- builder `list_view(model: Rc<[ListItem]>, factory)`; `.selection_mode(SelectionMode) .selected(usize) .show_separators(bool) .single_click_activate(bool) .enable_rubberband(bool)`; handlers `.on_selected(|usize| Msg) .on_activate(|usize| Msg)`
- controller `struct ListViewC { model: Rc<[ListItem]>, pool: Vec<Node>, first_visible: usize, offset: f32, row_height: f32, selection: Selection, cursor: Option<usize>, kinetic: Kinetic, rubberband: Option<(layout::Rect, Node)> }`

#### GridView — `Kind::GridView` — node name `gridview`
```
gridview
├── child[.activatable]
│
├── child[.activatable]
│
┊
╰── [rubberband]
```
- builder `grid_view(model, factory)`; `.min_columns(u32) .max_columns(u32) .selection_mode(SelectionMode) .single_click_activate(bool) .enable_rubberband(bool)`; handlers as `ListView`
- controller `struct GridViewC { model: Rc<[ListItem]>, pool: Vec<Node>, columns: u32, first_visible: usize, offset: f32, cell: (f32, f32), selection: Selection, cursor: Option<usize>, kinetic: Kinetic }`

#### ColumnView / ColumnViewColumn — `Kind::ColumnView` / `Kind::ColumnViewColumn` — node name `columnview`
```
columnview[.column-separators][.rich-list][.navigation-sidebar][.data-table]
├── header
│   ├── <column header>
┊   ┊
│   ╰── <column header>
│
├── listview
│
┊
╰── [rubberband]
```
Contains a real `listview` node — `node_tree_of` renders through it.
- builder `column_view(model, columns)`; `.show_row_separators(bool) .show_column_separators(bool) .reorderable(bool) .sort_column(usize, SortOrder)`; per-column `column_view_column(title, factory).resizable(bool).expand(bool).sorter(Sorter)`; handlers `.on_selected(|usize| Msg) .on_activate(|usize| Msg) .on_change(|&str| Msg)` (sort changes)
- controller `struct ColumnViewC { columns: Vec<ColumnState>, list: ListViewC, sort: Option<(usize, SortOrder)>, drag: Option<(usize, f32)>, header: Node }`

### 5.6 P6 · Menus

#### PopoverMenu — `Kind::PopoverMenu` / `Kind::PopoverMenuItem` — node name `popover`, classes `.background .menu`
```
popover.background.menu
├── arrow
╰── contents
    ╰── [box.horizontal.<display-hint>]
        ├── button.model
        │   ╰── label
        ┊
        ╰── button.model
            ╰── label
```
Menu items are `button.model`; a section with a display hint gets a
`box.horizontal.<hint>` that **may not be the item's direct parent** (so a
theme selects `.inline-buttons button.model`). Mnemonics inside a
`PopoverMenu` trigger **without** the `Alt` modifier. `Space` activates the
default widget.
- builder `popover_menu(items)`; `.flags(MenuFlags) .visible_submenu(&str)`; per-item `popover_menu_item(label).icon(IconRef).accel(&str).section(&str, DisplayHint).submenu(&str)`; handlers `.on_activate(|usize| Msg)`
- controller `struct PopoverMenuC { popover: PopoverC, items: Vec<Node>, cursor: Option<usize>, submenu: Option<PopupKey>, sections: Vec<(Rc<str>, Node)> }`

#### PopoverMenuBar — `Kind::PopoverMenuBar` — node name `menubar`
```
menubar
├── item[.active]
┊   ╰── popover
╰── item
    ╰── popover
```
- builder `popover_menu_bar(menus)`; `.menu(&str, View<Msg>)`; handlers `.on_activate(|usize| Msg)`
- controller `struct PopoverMenuBarC { items: Vec<Node>, open: Option<usize>, menus: Vec<PopoverMenuC> }` — once one menu is open, hovering a sibling `item` switches to it without a second click; Left/Right move between items.

### 5.7 P6 · Windows & dialogs

#### Window — `Kind::Window` — node name `window`, class `.background`
```
window.background [.csd / .solid-csd / .ssd] [.maximized / .fullscreen / .tiled]
├── <child>
╰── <titlebar child>.titlebar [.default-decoration]
```
The decoration class comes from the surface: icedtea's compositor draws SSD, so
a real toplevel gets `.ssd` unless the app opts into CSD; the gallery renders
`.csd` to exercise `HeaderBar`/`WindowControls`. `SurfaceStates` drives
`.maximized`/`.fullscreen`/`.tiled` and `PseudoStates::BACKDROP`.
Key bindings, `gtk/gtkwindow.c:1307-1359`: `default.activate`,
`window.minimize`, `window.toggle-maximized`, `window.close`;
`Space` → activate focus; `Return`/`ISO_Enter`/`KP_Enter` → activate default.
- builder `window(child)`; `.title(&str) .titlebar(View<Msg>) .default_widget(&str) .resizable(bool) .modal(bool) .deletable(bool) .decorated(bool) .default_size(i32, i32) .icon_name(&str)`; handlers `.on_close(Msg)`
- controller `struct WindowC { states: SurfaceStates, titlebar: Option<Node>, focus: FocusRing, default_widget: Option<Node>, decoration: Decoration }`

#### ShortcutsWindow — `Kind::ShortcutsWindow` — node name `window`, class `.shortcuts`
```
window.shortcuts
```
Deprecated upstream (4.18); kept (R1). A `Window` preset.
- builder `shortcuts_window(sections)`; `.section_name(&str) .view_name(&str)`; handlers `.on_close(Msg) .on_search(|&str| Msg)`
- controller `struct ShortcutsWindowC { window: WindowC, sections: Vec<Node>, search: String }` — Escape closes; `Ctrl+F` focuses search.

#### AboutDialog — `Kind::AboutDialog` — node name `window`, class `.aboutdialog`
```
window.aboutdialog
```
A `Window` preset. Escape closes.
- builder `about_dialog(program_name: &str)`; `.version(&str) .comments(&str) .copyright(&str) .license(&str) .license_type(LicenseType) .website(&str) .website_label(&str) .authors(&[&str]) .artists(&[&str]) .documenters(&[&str]) .translator_credits(&str) .logo_icon_name(&str) .wrap_license(bool)`; handlers `.on_activate_link(|uri| Msg) .on_close(Msg)`
- controller `struct AboutDialogC { window: WindowC, pages: StackC }`

#### AlertDialog — `Kind::AlertDialog` — node name `window`, classes `.dialog .message`
```
window.dialog.message
├── label.title
├── [label]
╰── box
    ├── button
    ┊
    ╰── button
```
Not a widget in GTK (a `GObject` launching a deprecated `GtkMessageDialog`); the
tree above is the one `GtkMessageDialog` renders (`.dialog` at
`gtk/deprecated/gtkdialog.c:601`, `.message` at `gtk/gtkmessagedialog.c:463`,
the primary label's `.title` at `:342`) and is the fixture.
- builder `alert_dialog(message: &str)`; `.detail(&str) .buttons(&[&str]) .default_button(i32) .cancel_button(i32) .modal(bool)`; handlers `.on_response(|usize| Msg)`
- controller `struct AlertDialogC { window: WindowC, buttons: Vec<Node>, default_button: Option<usize>, cancel_button: Option<usize> }` — Escape fires `cancel_button`, Enter fires `default_button`.

---

## 6. Icons [P7]

```rust
// ui/src/icons/theme.rs
pub struct IconTheme { /* name, roots, chain: Vec<ThemeDir>, cache */ }

impl IconTheme {
    /// `gtk-icon-theme-name` from `$XDG_CONFIG_HOME/gtk-4.0/settings.ini`
    /// (`[Settings]`), default `Adwaita`. Roots, in order:
    /// `$XDG_DATA_HOME/icons`, `$HOME/.icons`, each `$XDG_DATA_DIRS/icons`,
    /// then `/usr/share/pixmaps` (flat, non-themed, last resort only).
    pub fn from_env() -> Self;
    /// Hermetic construction for tests: no env reads at all.
    pub fn with_name_and_roots(name: &str, roots: Vec<PathBuf>) -> Self;
    pub fn name(&self) -> &str;
    /// The resolved inheritance chain, base theme first, `hicolor` last —
    /// implicitly appended when a theme neither is `hicolor` nor names it.
    pub fn chain(&self) -> &[Rc<str>];

    /// The spec's `FindIcon`: per theme in the chain, try each `Directories`
    /// (+ `ScaledDirectories`) subdir for `name.{png,svg,xpm}` where
    /// `DirectoryMatchesSize(subdir, size, scale)`; if none matched, take that
    /// theme's minimum `DirectorySizeDistance` candidate **before** moving to
    /// the next theme; finally `/usr/share/pixmaps`.
    /// `symbolic` prefers `<name>-symbolic` and falls back to `<name>`;
    /// a total miss falls back to `image-missing`, and only then to `None`.
    pub fn lookup(&mut self, name: &str, size: u32, scale: u32, symbolic: bool)
        -> Option<IconFile>;

    /// `lookup` + decode + recolour + rasterize, memoized. This is what paint
    /// calls; `lookup` alone is for tests and diagnostics.
    pub fn render(&mut self, name: &str, size: u32, scale: u32,
                  symbolic: bool, palette: &Palette) -> Option<Rc<Image>>;
    pub fn clear_caches(&mut self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconFile {
    pub path: PathBuf,
    pub format: IconFormat,
    /// The subdir's declared `Size`.
    pub nominal_size: u32,
    pub scale: u32,
    pub symbolic: bool,
    pub kind: DirKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum IconFormat { Svg, Png, Xpm }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirKind { Fixed { size: u32 }, Scalable { min: u32, max: u32 }, Threshold { size: u32, threshold: u32 } }

impl DirKind {
    /// The spec's `DirectoryMatchesSize`.
    #[must_use] pub fn matches(self, dir_scale: u32, size: u32, scale: u32) -> bool;
    /// The spec's `DirectorySizeDistance`.
    #[must_use] pub fn distance(self, dir_scale: u32, size: u32, scale: u32) -> u32;
}

/// The four logical slots GTK passes positionally as
/// `[foreground, success, warning, error]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette { pub foreground: Rgba, pub success: Rgba, pub warning: Rgba, pub error: Rgba }

impl Palette {
    /// `foreground` = the painting node's computed `color`; the other three
    /// come from `-gtk-icon-palette` on that node, falling back to the theme's
    /// `@success_color`/`@warning_color`/`@error_color`.
    pub fn from_style(style: &ComputedStyle, colors: &ColorTable) -> Self;
}

// ui/src/icons/render.rs
/// SVG via `skia_rs_safe::svg::{parse_svg, render_svg_in_container}` (which
/// honours `preserveAspectRatio`, i.e. GTK's icon-scaling contract); PNG via
/// `skia_rs_safe::codec::decode_image` + `Canvas::draw_image_rect`.
/// A decode failure resolves to `image-missing` and is **logged once per path**.
pub fn render(file: &IconFile, size: u32, scale: u32, palette: &Palette) -> Option<Image>;

// ui/src/icons/symbolic.rs
/// Recolour by building a synthetic stylesheet (`.success{fill:…}`,
/// `.warning{…}`, `.error{…}`, plus a lowest-specificity catch-all for the
/// foreground) and running `apply_stylesheet` on a **cloned** `SvgDom` — never
/// mutate a cached parse in place, and never string-patch the SVG source.
pub fn recolour(dom: &SvgDom, palette: &Palette) -> SvgDom;
/// `true` for a `-symbolic.svg` name or a file under a `symbolic/` subdir.
pub fn is_symbolic(file: &IconFile) -> bool;

// ui/src/icons/builtin.rs
/// `-gtk-icon-source: builtin` shapes, as skia paths in a unit box scaled to
/// `size`. Geometry follows `gtkcssimagebuiltin`; the pixel tests are
/// tolerance-based against a real GTK render, not hard-coded constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Check, CheckIndeterminate, Radio, RadioIndeterminate,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Expander, SpinPlus, SpinMinus,
}

impl Builtin {
    /// The shape, in a `size × size` box at the origin.
    #[must_use] pub fn path(self, size: f32) -> skia_rs_safe::core::Path;
    /// Stroke width for the outline shapes, ~size/8.
    #[must_use] pub fn stroke_width(self, size: f32) -> f32;
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: layout::Rect, color: Rgba);
    /// Parsed from a `builtin(<name>)` CSS value.
    #[must_use] pub fn from_css_name(name: &str) -> Option<Self>;
}
```

**Cache keys.** `IconTheme::lookup` memoizes on
`(theme_chain_hash, name, size, scale, symbolic)` → `Option<IconFile>`;
`IconTheme::render` memoizes on
`(IconFile.path, mtime, size, scale, palette)` → `Rc<Image>`; parsed `SvgDom`s
are cached on `(path, mtime)` and cloned per render, so a palette change costs
a re-render but not a re-parse. Every cache is bounded and cleared by
`clear_caches`.

**How `IconRef` resolves.** M2 parses and stores icon properties but draws
nothing (`css/value/image.rs:229`, `paint/background.rs:128`). P7 adds
`ui/src/paint/icon.rs`:

```rust
/// The single entry point every icon-shaped paint goes through: CSS
/// `-gtk-icon-source`, `background-image: -gtk-icontheme(…)`, and the `Image`
/// and `Picture` widgets. Reads `-gtk-icon-size`, `-gtk-icon-transform`,
/// `-gtk-icon-shadow`, `-gtk-icon-filter`, `-gtk-icon-style` and
/// `-gtk-icon-palette` off `style`; HiDPI comes from `-gtk-scaled()` and the
/// output scale on `cx`.
pub fn paint_icon(canvas: &mut Canvas<'_>, icon: &IconRef, rect: layout::Rect,
                  style: &ComputedStyle, cx: &mut PaintCx<'_>);
```

`PaintCx` (`paint/mod.rs:44`) gains one field: `pub icons: &'a mut IconTheme`.
That is the **only** change to an M2 paint signature; `paint_node`,
`paint_node_with_children`, `paint_backgrounds` and the rest keep their
signatures byte for byte.

**Cursors are out of scope.** The `cursor` property maps to
`wp_cursor_shape_v1` names (§3.4); no client-side cursor theme is loaded.

**Tests.** `ui/tests/fixtures/mini-icon-theme/` — an `index.theme` with two
`Directories` (one `Fixed` 16, one `Scalable` symbolic), one regular SVG, one
`-symbolic.svg`, one PNG, and a one-entry `Inherits` parent — drives lookup,
inheritance, closest-size and fallback hermetically. A recolour pixel test
renders the symbolic icon under `color: #3584e4` and asserts the fill. The
installed Adwaita theme (`Inherits=AdwaitaLegacy,hicolor`; `16x16/*` `Fixed`,
`scalable/*` and `symbolic/*` `Scalable` 8..512; **no** `ScaledDirectories`,
so a `scale=2` lookup legitimately lands on a `Scale=1` SVG) is exercised only
when present. `cursors/` is not in `Directories=` and must never be treated as
an icon subdir; `icon-theme.cache` is never parsed.

---

## 7. Gallery and gates [P8]

```
cargo run -p icedtea-ui --bin gallery -- [OPTIONS]

  --theme <light|dark|hc>     Which bundled sheet to compile. Default: light.
  --theme-file <PATH>         A sheet on disk instead of a bundled one.
  --widget <NAME>             Render exactly one widget, alone, at the origin.
                              NAME is the Kind's snake_case name.
  --list                      Print every widget name, one per line, and exit.
  --probe-points              Print `<widget> <label> <x> <y>` for every probe
                              point of the current layout, and exit. This is how
                              the gate learns coordinates — it never hard-codes.
  --size <WxH>                Surface size. Default: 1280x800.
  --scroll <PX>               Scroll the page before the first frame.
  --scale <N>                 Output scale, for HiDPI probes. Default: 1.
  --print-allocation          As the M2 `themed-button` bin: one allocation per
                              line, for tests that need geometry without pixels.
```

The gallery is one scrollable page laid out with the toolkit's own containers,
one `frame` per widget carrying the widget's name as its label, in `Kind::all()`
order. **A widget missing from the page fails the gate** — the page is built by
iterating `Kind::all()`, so adding a `Kind` without a gallery entry is a
compile-time hole, not a silently missing test.

**Probe-point convention (normative).** Every widget exposes its probe points as
`(label, node)` pairs; the gallery turns each into a window-space point at the
**centre of that node's border box**, and `--probe-points` prints them. Labels
are stable, lowercase, and name the CSS subnode they sit on (`"root"`,
`"check"`, `"slider"`, `"trough"`, `"text"`, `"arrow"`, `"tab0"`, `"row0"`).
Every widget has at least `"root"`. The gate reads coordinates from
`--probe-points`, never from a literal — the M2 precedent
(`support::allocation_of` parsing `--print-allocation`) extended.

```rust
// ui/tests/support/mod.rs — grown, not duplicated
pub struct ProbePoint { pub widget: String, pub label: String, pub x: u32, pub y: u32 }
pub fn probe_points(theme: &str) -> Vec<ProbePoint>;
pub fn spawn_gallery(socket: &str, theme: &str) -> Reaper;
pub fn spawn_gallery_widget(socket: &str, theme: &str, widget: &str) -> Reaper;
// existing, unchanged: pixel_at, close, matches, capture_until, allocation_of,
// spawn_themed_button, spawn_themed_button_with_theme
```

`ui/tests/gallery_gate.rs` — rest state:

```
every_kind_appears_in_the_gallery
every_widget_renders_at_rest_in_the_light_theme
every_widget_renders_at_rest_in_the_dark_theme
every_widget_renders_at_rest_in_the_high_contrast_theme
every_probe_point_differs_between_light_and_dark
the_node_tree_of_every_widget_matches_its_gtk_fixture
```

`ui/tests/interaction_gate.rs` — one interaction per widget class, driven by
`VirtualPointerClient`/`VirtualKeyboardClient` with screencopy or model
assertions:

```
clicking_a_button_fires_its_message_and_paints_the_active_state
toggling_a_toggle_button_paints_the_checked_state
checking_a_check_button_paints_the_builtin_check
flipping_a_switch_animates_the_slider_to_the_other_end
typing_into_an_entry_shows_the_glyphs_and_moves_the_caret
typing_into_a_search_entry_fires_one_search_after_the_delay
peeking_a_password_entry_reveals_the_text
stepping_a_spin_button_repeats_while_the_button_is_held
opening_a_drop_down_and_picking_an_item_updates_the_button
dragging_a_scale_moves_the_slider_and_reports_the_value
scrolling_a_list_view_recycles_rows_without_losing_selection
expanding_an_expander_animates_and_reveals_the_child
switching_a_stack_page_runs_the_transition
opening_a_menu_button_popover_takes_the_grab_and_dismisses_outside
tabbing_through_a_form_moves_the_focus_ring_in_geometric_order
a_pointer_click_focuses_without_showing_the_focus_ring
```

Both gates run against the harness compositor. `ui/tests/node_trees.rs` is
hermetic (no compositor) and owned jointly by P5/P6; P8 only wires it into the
gallery gate's `the_node_tree_of_every_widget_matches_its_gtk_fixture`.

P8 also owns the M3 documentation pass: `ui/README.md`'s widget table and
module map, and the spec's status line.

---

## 8. Migration from M2

### 8.1 Replaced and re-homed items

| M2 | M3 | note |
|---|---|---|
| `widget::button::Button` (concrete struct) | `Kind::Button` + `ButtonC` controller + `builders::button` | `Button` itself **survives unchanged** as the M1/M2 gate's subject (§8.2); the toolkit does not build on it |
| `wayland::LayerWindow` | `window::Surface::Layer` + `window::Window` | `pub type LayerWindow = …` compat shim, §8.2 |
| `wayland::LayerWindowError` | `window::SurfaceError` | `pub type LayerWindowError = SurfaceError;`, same nine variant names |
| `wayland.rs`'s private `wait_bounded` | `Window::pump` | same shape (dispatch pending → `prepare_read` → `poll(2)` → read), generalised over surface kind |
| `Button::contains(origin, x, y)` | `pointer::hit_test(root, tree, styles, point, sensitive)` | the one-rect check becomes a real tree walk |
| `layout::Container::{Box, Leaf}` | `+ Grid`, `+ Center` (§3.6) | closes `layout.rs:163`'s own "M3" note |
| `LayoutTree::set_style`'s hard-coded `CENTER`/`CENTER` | `ChildLayout` read per child | closes `layout.rs:423`'s own "M3" note |
| `paint::paint_node`'s unused `node` param | read by `view::app`'s recursive walker | closes `paint/mod.rs:245`'s `#[allow(unused_variables)]`; the attribute is **removed** |
| icons "drawn in M4" (`computed.rs`, `paint/background.rs:128`, `value/image.rs:229`) | `paint::icon::paint_icon` | those three comments are rewritten to point at P7, not deleted-and-forgotten |
| `Button::tick`'s "no relayout on animated layout properties" | `App::run` relayouts when a sampled `Overrides` touches a layout-affecting prop | closes `widget/button.rs:335-343`'s own "M3" note |
| `harness::TestClient::popup_dismissed()` | `popup_done()` | the only harness rename; one call site |
| `harness::TestClient::popup_configured() -> bool` | `-> Option<(i32,i32,i32,i32)>` | forced: the old one was "the executable record of the gap" |
| `PaintCx { env, colors, fonts, images, text }` | `+ icons: &mut IconTheme` | the only M2 paint-signature change in all of M3 |

`Button` is **not** deleted and **not** reimplemented in terms of `ButtonC`.
It stays exactly as M2 left it (E5/E6 signatures) so the M1 pixel gate keeps
measuring the same code, and `bin/themed-button.rs` keeps working. P5's
`ButtonC` is a parallel implementation over the same paint primitives.

### 8.2 THE GATE — M2 tests that must stay byte-identical

No edit of any kind, including imports, is permitted in these:

- `ui/tests/themed_button_offscreen.rs` — all **4** (**the M1 pixel gate**, per
  M2 contract §10.2/E13): height `34.0`; `y = height-2` band `0xFFF6F5F4`;
  gutter column `bx = 4`; corner (0,0) transparent; border pixel `0xFFCDC7C2`;
  hover `0xFFE8E6E3`; active `0xFFDAD6D2`; suggested-action
  `#2c7fe3`→`#3584e4`, white text, border `0xFF15539E`; empty button 36×34;
  >20 dark label pixels. **A diff that changes a number here fails review.**
- `ui/tests/adwaita_coverage.rs` — all **9** (0 unknown properties on all three
  sheets, 37/37 `@define-color`s, cycle safety, sheet identity).
- `ui/tests/gtk4_property_reference.rs` — all **4** (114/95/19 per E14).
- `ui/tests/transition_screencopy.rs` — the **1** end-to-end transition test.
- `ui/src/css/**` — every test in `registry.rs`, `tokens.rs`, `parse.rs`,
  `value/**`, `node.rs`, `select.rs`, `cascade.rs`, `computed.rs`. M3 adds no
  CSS-engine behaviour; P7 only **reads** icon properties the registry already
  parses.
- `ui/src/anim/**` — every test.
- `ui/src/shm.rs` — all **11**. `BufferPool`/`ShmBuffer`/`SlotPool` are reused
  by every `Surface` kind unchanged.
- `compositor/**` and `harness/**` outside the popup paths — every existing
  test. In particular `backend.rs`'s `mod implicit_grab_tests` in the `wlr`
  crate stays green **unmodified** (§1.6).

Pinned constants that must not move: 900 compiled Adwaita rules; 1941 lines /
37 `@define-color`s; `POOL_INITIAL_BUFFERS=2`, `POOL_MAX_BUFFERS=3`,
`CONFIGURE_TIMEOUT=5s`, `MARGIN=0`, `BTN_LEFT=0x110`; `#3584e4`, `#1c6fd4`,
`#1961b9`.

### 8.3 M2 tests rewritten, with reason — the exhaustive list

| file | n | reason | invariant |
|---|---|---|---|
| `ui/tests/layer_shell_screencopy.rs` | 3 | `LayerWindow::open` moves behind `Surface::Layer`; the test's own `use icedtea_ui::wayland::LayerWindow` resolves through the §8.1 type alias | **imports only.** Every constant, every assertion and the `close()` ±12 tolerance derivation stay byte-identical, including the drag-off-and-back `:active` gesture |
| `ui/src/wayland.rs` | 11 | the file is replaced by `ui/src/window/**`; its 11 M1-era `#[test]`s move to `ui/src/window/layer.rs` | every assertion, constant and tolerance preserved verbatim; the `state()` helper is re-expressed against `window::Layer` |
| `ui/src/widget/button.rs` | 4 | **not rewritten.** Listed here only to say so explicitly | unchanged |
| `ui/src/layout.rs` | 6 | `Container` gains two variants and `set_style` reads `ChildLayout`; the six M2 tests construct `Container::Box`/`Leaf` and a `Style` with the old hard-coded alignment | **every number is preserved**: 80×34, label at (10, 8), 36×34 empty, 220×50 intrinsic, borderless == label, reused-tree equality. The default `ChildLayout` must reproduce M2's `CENTER`/`CENTER` exactly, and a test pins that |
| `ui/src/paint/mod.rs` | all | `PaintCx` gains `icons` | mechanical: every fixture gains an `IconTheme::with_name_and_roots("hicolor", vec![])`. No pixel constant changes |
| `ui/src/app.rs` | 0 | `themed_button*` helpers survive; `App::run` is a parallel path, not a generalisation of them (M2 E7) | unchanged |
| `compositor/tests/client_protocol.rs` | 1 | `a_popup_grab_still_dismisses_on_a_click_outside_it` asserts `!popup_configured()` **within 300 ms** as "the executable record of the gap", and was written to fail the day `wlr` grows popup support | rewritten as `a_popup_grab_still_dismisses_on_a_click_outside_it`, keeping its press/serial/click-outside sequence and its `popup_done()` assertion verbatim, with the negative configure assertion **inverted**: the popup is now configured and mapped. The mutation check (skip `notify_button` under an explicit grab ⇒ no `popup_done`) is re-run and recorded |
| `harness/src/lib.rs` docs | — | the "`wlr` has no xdg-popup support" paragraph (`lib.rs:2668-2676`) and `popup_configured`'s "the executable record of the gap" doc (`:2683`) are now false | rewritten to describe the working API; the `PopupHandles` leak caveat (`:1839-1844`) is preserved verbatim |
| `compositor/src/state.rs` docs | — | `state.rs:912` and `:6497` describe popups as future work | rewritten to point at the registry |

Nothing else in the workspace is rewritten. Any further M2 test that must
change requires a §10 amendment naming it and its reason.

### 8.4 Rulings that amend the M3 spec

- **R1 — the three deprecated widgets stay in scope.** `Statusbar`, `InfoBar`
  and `ShortcutsWindow` are deprecated in GTK 4.22 and so collide with spec
  §5's own "except deprecated" rule, but Adwaita still styles `statusbar` and
  `infobar`, all three are cheap, and the spec names them explicitly. In-scope
  count is **~59 `Kind` variants** covering the spec's ~48 widgets plus
  sub-kinds.
- **R2 — Tab order is geometric, not tree order.** Spec §3 says "tree order";
  GTK's `gtk_widget_focus_sort` sorts by y-centre then x-centre (x reversed
  under RTL), delegating to a directional sort for box-shaped containers. P3
  implements the geometric sort (§3.5). Tree order would visibly diverge for
  Grid, CenterBox, HeaderBar packs and Overlay, and the "Tab moves the focus
  ring" e2e would assert the wrong order.
- **R3 — `:focus-visible` is not "any keyboard navigation".** Spec §3's wording
  is amended to GTK's real rule (`_gtk_window_update_focus_visible`): defaults
  **true**; a key press remembers the focus; the release clears it only if the
  focus did not move, and sets it otherwise; `Alt` alone forces it true.
- **R4 — `Popover` ships in P5, not P6.** Spec §1's part table puts popovers in
  P6, but four P5 widgets embed one. `Kind::Popover` and
  `ui/src/widgets/popover.rs` are P5; `PopoverMenu`, `PopoverMenuBar` and
  `Kind::PopoverMenuItem` stay P6.
- **R5 — parent-scoped `new_popup`.** Spec §2 says listeners on
  `wlr_xdg_shell.events.new_popup` and `wlr_layer_surface_v1.events.new_popup`.
  P1 listens on `wlr_xdg_surface.events.new_popup` (per toplevel base, per
  popup base) plus the layer one, and leaves the shell-level signal unused: a
  layer popup has `parent == NULL` at shell-level emission and cannot be
  classified there.
- **R6 — `LayerId` already exists** as `wlr::LayerSurfaceId`; §1.8.
- **R7 — the constraint box is root-toplevel-surface space**, not layout or
  output space. `wlr_xdg_popup_unconstrain_from_box`'s own header says so; P2
  translates (§2.2).

---

## 9. Part boundaries

Dependency order: **P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8.** Strictly
sequential; per-task reviews, whole-part reviews and fix waves, then a
whole-milestone review. P1's publish and the final merge are consent stops.

### P1 — `wlr` 0.20.28: xdg-popup
**Owns (wloots-sys):** `crates/wlr/src/popup.rs` (new),
`crates/wlr/src/{backend,runtime,handler,dispatch,lib}.rs` popup paths,
`crates/wlr/coverage/{wrapped,waived}.toml`, `crates/wlr/README.md`,
`crates/wlr/Cargo.toml`.
**Implements:** §1 (all).
**Consumes:** nothing.
**Must not touch:** the implicit-grab code (`grab_still_applies`,
`pointer_motion_to_focus`, `on_pointer_button`) or its tests; any seat
`start_grab`/`end_grab`; the scene tree of a popup whose parent is dying.
**Gate:** crate unit tests; `cargo test -p wlr --test coverage_audit` and
`cargo xtask coverage` (both with `WLR_COVERAGE_BINDINGS` pinned);
`clippy -D warnings` and `cargo doc -D warnings`; `cargo publish --dry-run`;
`mod implicit_grab_tests` green unmodified. **Publish is a consent stop.**

### P2 — compositor popups + harness popup client
**Owns (icedtea):** `compositor/src/state.rs` popup paths,
`compositor/src/wayland.rs` (`PopupKey`), `compositor/tests/popups.rs` (new),
the popup half of `harness/src/lib.rs`, the one rewritten test in
`compositor/tests/client_protocol.rs`, `compositor/Cargo.toml`/`harness/Cargo.toml`
`wlr = "0.20.28"`.
**Implements:** §2 (all).
**Consumes:** §1.
**Must not touch:** `ui/`; any non-popup compositor path; `Snapshot`/
`WindowInfo` (they stay toplevel-only — popup assertions go through the
client-side harness accessors, not D-Bus).
**Gate:** all twelve `popups.rs` tests deterministic ×3; every existing
compositor and harness test green; the rewritten `client_protocol.rs` test's
mutation check re-run and recorded.

### P3 — window & event layer
**Owns (ui):** `ui/src/window/**` (new, replacing `ui/src/wayland.rs`),
`ui/Cargo.toml` (the four dep lines), `ui/tests/fixtures/keymaps/us.xkb`,
the `LayerWindow`/`LayerWindowError` compat aliases.
**Implements:** §3.1–§3.5, §3.8, §3.9.
**Consumes:** §1, §2 (for its e2e), M2's `Node`, `LayoutTree`,
`AnimationState`, `BufferPool`, `CompiledSheet`, `FontDatabase`.
**Must not touch:** `css/**`, `anim/**`, `shm.rs`, `paint/**`,
`widget/button.rs`, `app.rs`.
**Gate:** §3.9's unit and e2e lists; `ui/tests/layer_shell_screencopy.rs`
diffs by imports only; the 11 relocated `wayland.rs` tests keep every
assertion; `--no-default-features` builds.

### P4 — reactive framework
**Owns (ui):** `ui/src/view/**` (new), the recursive paint walker that finally
reads `paint_node`'s `node` argument, `StyleMap`.
**Implements:** §4 (all).
**Consumes:** §3 (`Surface`, `Window`, `InputEvent`, `FocusRing`, `hit_test`,
`Clipboard`), M2's cascade/computed/anim/layout/paint.
**Must not touch:** `window/**` internals (it consumes the public API),
`css/**`, `widget/button.rs`.
**Gate:** §4.8; mutation checks on the LCS and `Props::diff`; the counter app
renders identical pixels for identical models.

### P5 — widgets: display, buttons, entries, popover
**Owns (ui):** `ui/src/widgets/` for the 32 kinds in §5.1–§5.3,
`ui/src/text.rs`'s §3.7 additions, `ui/tests/fixtures/gtk4.22-node-trees/` for
those kinds, `ui/tests/node_trees.rs` (created here, extended by P6).
**Implements:** §3.7, §5.1, §5.2, §5.3.
**Consumes:** §4 (all), §3, §7's `Builtin` **by name only** — P5 stubs
`Builtin::path` behind P7's signature and P7 fills it in; a P5 pixel test that
depends on real builtin geometry is deferred to P7's fix wave and named as such.
**Must not touch:** `layout.rs` (P6 owns the `Container` widening), `view/**`,
`window/**`.
**Gate:** per widget, one Adwaita rest-state pixel test and one interaction
state; `node_tree_of` matches every fixture; `TextLayout` unit tests for wrap,
ellipsize, caret round-tripping and grapheme/word movement, including
non-ASCII.

### P6 — widgets: containers, lists, menus, windows
**Owns (ui):** `ui/src/widgets/` for the 27 kinds in §5.4–§5.7,
`ui/src/layout.rs` (§3.6), the remaining node-tree fixtures.
**Implements:** §3.6, §5.4, §5.5, §5.6, §5.7.
**Consumes:** §4, §5's `PopoverC`/`ListViewC`/`ScrollbarC`, §3.
**Must not touch:** P5's widget files except to call their controllers.
**Gate:** the same per-widget pair; `layout.rs`'s 6 M2 tests keep every number
and a new test pins that the default `ChildLayout` reproduces M2's
`CENTER`/`CENTER`; row recycling in `ListView`/`GridView`/`ColumnView` is
proven not to lose selection or animation state across a scroll.

### P7 — icons
**Owns (ui):** `ui/src/icons/**` (new), `ui/src/paint/icon.rs` (new),
`PaintCx`'s one new field, `ui/tests/fixtures/mini-icon-theme/**`, and the
three "drawn in M4" comments in `computed.rs`/`paint/background.rs`/
`value/image.rs`.
**Implements:** §6 (all).
**Consumes:** §4's `IconRef` resolution point, M2's `ComputedStyle`/`ColorTable`.
**Must not touch:** widget behaviour; `paint_node`'s signature.
**Gate:** hermetic mini-theme lookup/inheritance/closest-size/fallback tests;
the symbolic recolour pixel test; builtin shapes cross-checked against a real
GTK render with a stated tolerance; Adwaita exercised only when present; the
`PaintCx` change is mechanical in every M2 paint test and changes no pixel
constant.

### P8 — gallery, gates, docs
**Owns (ui):** `ui/src/bin/gallery.rs` (new), `ui/tests/gallery_gate.rs` (new),
`ui/tests/interaction_gate.rs` (new), `ui/tests/support/mod.rs` (grown),
`ui/README.md`, the spec's status line, `ui/Cargo.toml`'s `[[bin]]` entry.
**Implements:** §7 (all).
**Consumes:** everything.
**Must not touch:** any widget implementation — a gate failure is fixed in the
owning part's fix wave, not in the gate.
**Gate:** all six `gallery_gate.rs` tests and all sixteen
`interaction_gate.rs` tests, in light, dark and hc; `cargo test --workspace`;
`clippy -D warnings` with default features **and** `--no-default-features`;
`cargo fmt --all --check` (a hard gate since M2's PR #20/#21 made the tree
rustfmt-clean); `cargo doc -D warnings`.

### Cross-cutting rules (every part)

- Every load-bearing test records a mutation check: break the code the test
  claims to cover, confirm the test fails, restore.
- Timing assertions are generous complexity bounds, never wall-clock pins.
- Untrusted input — themes, icon files, SVGs, keymaps, popup positioners,
  markup, list models — **never panics**. A malformed value is dropped, logged
  once, and replaced by the initial/fallback.
- `Duration::ZERO` from any `next_deadline`/`next_frame_in` means "now", never
  "spin".
- No part changes a signature in this contract without a §10 amendment.

---

## 10. Amendments

*(Empty at freeze. Every deviation a part discovers is appended here as
`### E<n> — <one-line ruling>` with the conflicting texts quoted, the ruling,
and which part carries it out — the same shape as the M2 contract's §12.)*

### P3-A — `Surface::commit_buffer` is `pub(crate)` and borrows the pool, the shm and the queue handle

**Carried out by:** P3 (`ui/src/window/mod.rs`). **Added:** 2026-08-31, in the
Part 3 whole-part fix wave.

**Contract §3.1 says:**

```rust
pub fn commit_buffer(&mut self, skia: &mut skia_rs_safe::canvas::Surface) -> Result<(), SurfaceError>;
```

**As shipped:**

```rust
pub(crate) fn commit_buffer(
    &mut self,
    skia: &mut skia_rs_safe::canvas::Surface,
    buffers: &mut BufferPool,
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<WindowState>,
) -> Result<(), SurfaceError>;
```

**Ruling.** Two departures, both forced, both kept:

1. **The three extra parameters.** The contract's own §3.1 note on `WindowState`
   settles where the pools live — "the pools live on `Window`, and one window
   has several" — and P3's `Window` does exactly that: one `BufferPool` for the
   window's own surface plus one per open popup, reallocated together by
   `Window::render`'s `resize_backing`. A `Surface` that owned its pool would
   have to own a `wl_shm` and a `QueueHandle<WindowState>` as well, in all
   three role structs, and `PopupWindow` would then hold the pool twice. The
   `Surface` borrows the pool for the duration of one commit instead.
2. **`pub(crate)` rather than `pub`.** `QueueHandle<WindowState>` names a
   crate-private type, so the signature above is unnameable outside the crate
   whatever its visibility marker says. P4 lives in the same crate
   (`ui/src/view/**`) and can call it; no out-of-crate consumer could have.

`Surface::set_cursor_shape` had drifted the same way (a third
`device: Option<&WpCursorShapeDeviceV1>` parameter) and is **not** amended: it
was restored to the contract's `(&mut self, shape, serial)` in the same fix
wave, by giving each role — including `Popup` — a handle on the window's one
`wp_cursor_shape_device_v1` so the method can read the device off `self`. Only
the window's own role object destroys that device.

**Everything else in §3.1 is delivered verbatim,** including
`Surface::Popup(popup::Popup)`, which the window layer itself uses:
`PopupWindow` holds its role as a `Surface`, so a popup goes through the same
`wl_surface`/`size`/`scale`/`states`/`commit_buffer` accessors the other two
roles do.

### P4-D19 — `cx.handled` ends the whole dispatch, not just the phase it was set in

**Carried out by:** P4 (`ui/src/view/app.rs::deliver`). **Added:** 2026-08-31,
in the Part 4 whole-part fix wave.

**Contract §4.6 says** a controller that sets `cx.handled = true` "stops the
phase it is in".

**As shipped:** the node that sets `handled` is the last node the event
reaches. A handled capture suppresses both target and bubble; a handled target
suppresses bubble; a handled bubble stops climbing.

**Ruling.** The contract's wording is self-contradictory for the capture leg.
Capture, target and bubble run over one path, so "capture stops descending" and
"the target still receives the event" cannot both hold: a capture that stopped
only its own phase would hand the event straight to the node that just
intercepted it, which is the opposite of what intercepting means. Whole-
dispatch stopping is also what P4's own literal dispatch test asserts and what
GTK's `GTK_EVENT_SEQUENCE_CLAIMED` does. `EventCx::phase` (P4 D5) is
unaffected — a controller still sees which leg it is in. `deliver`'s rustdoc
states the shipped rule; P5 and P6 write their controllers against that
rustdoc, not against §4.6's sentence.

---

### P4-D20 — `CompiledRule`, `CompiledSheet` and `RuleBuckets` gain `#[derive(Clone)]`

**Carried out by:** P4 (`ui/src/css/cascade.rs`, `ui/src/css/select.rs`).
**Added:** 2026-08-31, in the Part 4 whole-part fix wave.

**Contract §9 says** P4 "must not touch: `window/**` internals …, `css/**`,
`widget/button.rs`".

**As shipped:** three `#[derive(Debug)]` became `#[derive(Debug, Clone)]`.
Nothing else in `css/**` changed — no field, no signature, no behaviour, no
test.

**Ruling.** Declared, not reverted. `App::run` must own a `CompiledSheet`
(`self.sheet.take().unwrap_or_else(|| window.sheet().clone())`) because the
same loop body holds `window` mutably borrowed for `pump`, `clipboard` and
`paint_with`; borrowing the sheet out of the window instead does not borrow-
check. The other alternative — handing out an `Rc<CompiledSheet>` from
`Window::sheet` — would change an existing P3 signature, which P4 D2 forbids
in as many words. Three additive derives are the smallest change that works,
and this amendment is the ledger entry §9 exists to demand. P5–P8 keep the
`css/**` prohibition in full: this is a closed, enumerated exception, not a
precedent for further edits.

---

### P4-D21 — `ui/src/window/mod.rs` gains four more `Window` methods and one re-export

**Carried out by:** P4 (`ui/src/window/mod.rs`). **Added:** 2026-08-31, in the
Part 4 whole-part fix wave.

**P4 D2 declared** one additive item on this P3-owned file,
`Window::paint_with`.

**As shipped,** five:

```rust
impl Window {
    pub fn paint_with(...);          // D2
    pub fn size(&self) -> (u32, u32);
    pub fn set_title(&mut self, title: &str);
    pub fn minimize(&mut self);
    pub fn toggle_maximized(&mut self);
}
pub use layer::BTN_LEFT;
```

**Ruling.** Kept and declared. `App::run` needs `size` for the layout pass and
the other three to execute `Cmd::{SetTitle, Minimize, ToggleMaximized}`; §3.1
exposes no other route to any of them, and the loop is §4.7's, so the methods
have to exist somewhere. `BTN_LEFT` is re-exported at `window`'s root because
`view::controller` and P5's controllers name the left mouse button constantly
and `window::layer` is a role module, not a button vocabulary. All five are
additive: no existing signature is touched, which is the same rule D2 stated.
The file stays P3-owned; a later part adding to it needs its own amendment.

---

### P5-D22 — `DropDownC.list` is a `Node`, not a `ListViewC`

**Carried out by:** P5 (Task 21, `ui/src/widgets/drop_down.rs`). **Added:**
2026-08-31, fixing Task 21's review round 1.

**Contract §5.2 says:**

```rust
struct DropDownC { items: Rc<[ListItem]>, selected: usize, open: bool, search: String, filtered: Vec<usize>, button: Node, popover: PopoverC, list: ListViewC }
```

`ListViewC` is a P6 kind (`Kind::ListView`) landing after P5.

**As shipped:** `DropDownC.list` is a plain `Node` — the `listview` subnode
and its `row` children are built and reconciled directly by `DropDownC`
itself, with no `ListViewC` controller involved.

**Ruling.** Declared, not reverted, matching Task 21's own Interfaces section
and the part plan's D10 (§ "Contract deviations"). P5 has no `ListViewC` to
construct — `Kind::ListView` is one of P6's 32 kinds — so `DropDownC` builds
the node and rows itself for this milestone. P6's Task 19, when it creates
`ui/src/widgets/list_view.rs` and lands `Kind::ListView`, replaces this field
with a real `ListViewC` and migrates `DropDownC`'s row-building logic onto it.
`FontDialogC.list` (Task 22) carries the identical deviation and the identical
P6 migration path; contract §11 E10 already covers the cross-part half of this
ruling (P6's `on_scrolled`/`on_reordered` two-argument handlers), so this entry
records the P5-side half that §11 assumed but that had not yet been written
into §10 itself.

---

### P5-D23 — `ColorDialogButton`'s colour rides `Handler::Float` as a packed `0xAARRGGBB`; all four dialog kinds gain a `sink` node

**Carried out by:** P5 (Task 22, `ui/src/widgets/color_dialog.rs`,
`ui/src/widgets/font_dialog.rs`). **Added:** 2026-08-31.

**Contract §5.2 types** `ColorDialogButton`'s change handler as
`on_change(|Rgba| Msg)` and `FontDialogC.list` as `ListViewC`.

**As shipped:**

1. `Handler` has no `Rgba` variant (only `Unit`/`Text`/`Bool`/`Index`/`Float`/
   `Key`), so `ColorDialogButtonExt::on_change` and `ColorDialogExt::on_change`
   both take `impl Fn(f64) -> Msg`, fired via `Handler::Float` with the colour
   packed 8 bits per channel into an `f64` (`ColorDialogC::pack`/`unpack`,
   exact up to `2^32`, which every `f64` represents losslessly). Task 22's own
   Interfaces section specified this, matching Task 21's `FontDialogC.list`
   precedent below.
2. `FontDialogC.list` is a plain `Node` (the `fontchooser` subnode), not a
   `ListViewC` — `Kind::ListView` is a P6 kind. Identical deviation and
   identical P6 migration path to P5-D22's `DropDownC.list`.
3. (Not anticipated by Task 22's text.) `ColorDialogButtonC`, `ColorDialogC`,
   `FontDialogButtonC` and `FontDialogC` each gained a `pub sink: Node` field,
   wired into `widgets::child_slot`. None of the four take application
   children, so `reconcile.rs`'s no-application-children cleanup — which walks
   `child_slot(kind, controller).unwrap_or(root)` and detaches everything past
   position zero — was detaching each controller's own `button`/`colorchooser`/
   `fontchooser` subnode on every single reconcile, leaving every one of the
   four kinds a permanently zero-sized, unstyled, unpaintable root. `sink` is
   the identical fix `DropDownC` (Task 21) already carries for the same
   reason; `widgets::mod.rs`'s own doc comment on `child_slot` names this
   exact failure mode as "fatal" and prescribes this exact fix.

**Ruling.** All three declared, not reverted. (1) and (2) are exactly what
Task 22's Interfaces section instructed recording here. (3) is a correctness
fix, not a scope expansion: without it every one of this task's four `Kind`s
renders nothing and hit-tests nothing, which is not an alternate valid
implementation of the plan's node trees, just a bug the plan's code sample
did not have to face because it was written before Task 21 discovered
`child_slot`. P6 tasks building further no-application-children widgets whose
chrome lives on a subnode (any future `menubutton`/`dropdown`/`colorbutton`-
shaped kind) should check `child_slot` from the start rather than rediscover
this by a zero-sized widget in a pixel test.

**Also recorded here:** `clicking_a_colour_button_opens_its_dialog_and_repaints_the_swatch`
in `ui/tests/widget_pixels.rs` is `#[ignore]`d for the same framework bug
already covered by `opening_a_drop_down_and_picking_an_item_updates_the_button`
(P5-D22's neighbour, `route`'s `aim` in `ui/src/view/app.rs` never resolving to
the innermost `Instance` when a kind's chrome lives on a nested, non-zero-sized
subnode). Confirmed directly: an inline `eprintln!` at the top of
`ColorDialogButtonC::on_event` never fires under the test's click script, even
once (3) above gave the subnode real layout geometry. Not fixable inside P5's
boundary (`view/app.rs` is D5 File Structure); unignore alongside the
`DropDown` test once `aim` is fixed.

**Superseded 2026-08-31 by §10 P5-D33:** `aim` is fixed inside P5 and both
tests are un-ignored and green. The "not fixable inside P5's boundary"
rationale is withdrawn — see P5-D32 for why the boundary claim did not hold.
The description of `child_slot` in (3) above as a pre-existing walk is also
corrected there: P5 added it to `reconcile.rs`.

---

### P5-D24 — `ui/Cargo.toml` gains `unicode-segmentation` and `unicode-linebreak`

**Carried out by:** P5 (Task 1, `ui/Cargo.toml`). **Added:** 2026-08-31, in the
P5 close-out.

**Contract §0 gives** `ui/Cargo.toml` to P3 and lists three dependencies.

**As shipped:** P5 adds `unicode-segmentation = "1.13"` and
`unicode-linebreak = "0.1"`.

**Ruling.** Declared, not reverted. §3.7's `TextLayout` needs UAX #29
cluster/word boundaries and UAX #14 line-break opportunities, neither of which
the three P3 dependencies provide. Both crates already resolve in the
workspace lockfile through `cosmic-text`, so this pins direct, versioned
dependencies on crates the build already pulls in transitively rather than
reaching into `cosmic-text`'s internals.

---

### P5-D25 — `ui/src/icons/builtin.rs` is created by P5 with placeholder geometry

**Carried out by:** P5 (Task 1, `ui/src/icons/builtin.rs`). **Added:**
2026-08-31, in the P5 close-out.

**Contract §9 assigns** `ui/src/icons/**` to P7.

**As shipped:** P5 creates `ui/src/icons/builtin.rs` with P7's exact
`Builtin` enum and `path`/`stroke_width` signatures, bodies filled with
placeholder geometry, so `CheckButton`'s check node and `SpinButton`'s
steppers have something to call.

**Ruling.** Declared, not reverted. P5's own widgets (`CheckButton`,
`SpinButton`, `MenuButton`'s arrow) need `Builtin` to exist before P7 lands;
the file's own module doc says as much. P7 replaces the two placeholder
bodies and un-ignores `a_check_button_paints_the_builtin_check_glyph` and
`an_image_paints_its_resolved_icon` (P5-D31 below, which owns those two
`#[ignore]`s; P5-D28 is the builders-file record and was cited here in
error); it must not rename the
enum or its two methods, since P5's `CheckButtonC`/`SpinButtonC` already call
them by these exact names.

---

### P5-D26 — `ImageC.resolved` is `Option<Rc<skia_rs_safe::codec::Image>>`, not `icons::Handle`

**Carried out by:** P5 (`ui/src/widgets/image.rs`). **Added:** 2026-08-31, in
the P5 close-out.

**Contract §5.1 names** `icons::Handle` as `ImageC`'s resolved-icon field
type; §6 never defines a type by that name — `IconTheme::render` there
returns `Option<Rc<Image>>` (`Image` = `skia_rs_safe::codec::Image`).

**Ruling.** Follow §6, the more specific and more recently written section.
`ImageC.resolved: Option<Rc<skia_rs_safe::codec::Image>>` matches
`IconTheme::render`'s return type exactly, so `ImageC::build` can store it
without a conversion; it stays `None` until P7 wires icon resolution through
(P5-D31).

---

### P5-D27 — eighteen widget-local types live in `widgets/mod.rs` and `widgets/edit.rs`, behind a `WidgetEnum` trait

**Carried out by:** P5 (`ui/src/widgets/mod.rs`, `ui/src/widgets/edit.rs`).
**Added:** 2026-08-31, in the P5 close-out.

**§5 names** `Orientation`, `Position`, `Side`, `IconSize`, `MessageType`,
`LevelBarMode`, `ContentFit`, `MatchMode`, `ArrowDirection`, `FontLevel`,
`WindowButton`, `ColorDialogSpec`, `PictureSource`, `Adjustment`, `Mark`,
`RepeatTimer`, `TextEditState` and `UndoStack` across widget signatures but
never says which module declares them.

**As shipped:** the eleven simple enums (`Orientation` through `WindowButton`)
are generated by one `widget_enum!` macro in `widgets/mod.rs`, each backed by
a CSS-class string per variant and reached through a shared `WidgetEnum`
trait (`to_prop`/`from_prop`/`all`) so every one of them round-trips through
`Prop::Enum(u16)` the same way. `ColorDialogSpec`, `PictureSource`,
`Adjustment`, `Mark` and `RepeatTimer` are plain structs, also in
`widgets/mod.rs`. `TextEditState` and `UndoStack` are in `widgets/edit.rs`,
the shared `GtkText` engine `Entry`/`SearchEntry`/`PasswordEntry`/
`SpinButton` all build on. `ListItem` — the eighteenth name §5 mentions
alongside these — is **not** among them: it is P4's, declared in
`view/mod.rs` and already covered by §11 E5; P5 only consumes it.

**Ruling.** Declared. The `WidgetEnum` trait is the mechanism, not named by
§5, that lets seventeen unrelated enums share one encoding path into
`Prop::Enum` instead of each widget hand-rolling a `to_u16`/`from_u16` pair.

---

### P5-D28 — `view/builders.rs` gains only `pub use` lines; builder bodies live under `widgets/`

**Carried out by:** P5 (`ui/src/view/builders.rs`, `ui/src/widgets/*.rs`).
**Added:** 2026-08-31, in the P5 close-out.

**Contract §9's P5 boundary says** P5 "must not touch `view/**`"; §0 and
§4.4 say `view/builders.rs` is where P5/P6 fill the builder frame P4 ships,
naming every builder `view::builders::<name>`.

**As shipped:** every one of P5's 30-odd builder functions and `*Ext` setter
traits is defined in its own `widgets/<name>.rs`; `view/builders.rs` gains
only a `pub use crate::widgets::<name>::{...};` line per widget (30 lines, one
per builder module), plus the doc comment recording the naming rule.

**Ruling.** Declared, not reverted. This is the smallest edit to `view/**`
that satisfies §4.4's naming requirement without moving widget
implementations — which line up with `view::{Kind, Prop, PropName}` and the
reconciler far more than with `view`'s own module boundary — into a file
`view/**`'s prohibition was written to keep P5 out of.

---

### P5-D29 — `widgets::build_controller` is the reconciler's whole `Kind` dispatch table

**Carried out by:** P5 (`ui/src/widgets/mod.rs`). **Added:** 2026-08-31, in
the P5 close-out.

**§4.5 has** the reconciler build a `Box<dyn Controller>` from a `Kind`
without saying how, and `Controller::build` is `fn build() -> Self where Self:
Sized`, which a `Kind`-indexed dispatcher cannot call generically.

**As shipped:** `widgets::build_controller(kind: Kind) -> Box<dyn
Controller>` is one `match` over every `Kind` variant, each arm calling that
kind's own `Controller::build`; `Kind`s P6 has not yet implemented fall
through to the `Unimplemented` controller (`no_p5_kind_falls_through_to_the_
unimplemented_controller`, this close-out's Step 1, checks the P5 half of
that table has no such fallthrough).

**Ruling.** Declared, not reverted, matching Task 6's own Interfaces section.
P5 owns the table beside the widgets it dispatches to; P4's reconciler calls
it by name; P6 replaces the `Unimplemented` arms for its own 32 kinds as they
land.

---

### P5-D30 — `node_tree_of`'s fixtures are matched structurally, not string-diffed

**Carried out by:** P5 (`ui/src/widgets/mod.rs::fixture_matches`). **Added:**
2026-08-31, in the P5 close-out.

**§5 says** the rendered tree is diffed against the fixture block vendored
from GTK's sources, but the fixture blocks carry `[optional]` nodes,
`[.optional-class]` markers, `┊`/`⋮` repetition and `<child>` wildcards that a
literal string diff cannot interpret.

**As shipped:** `widgets::fixture_matches` parses both trees into paths with
per-node required classes, then enforces both directions: every rendered node
sits at a fixture-named path (or under a `<child>` wildcard) and carries every
always-present class the fixture names at that path; every fixture node that
is not `[optional]`, and whose parent is present, must appear in the rendered
tree. `┊`/`⋮` lines are dropped during parsing — repetition does not change
which paths are legal, only how many rendered nodes may sit at one.

**Ruling.** Declared, not reverted, matching Task 6's own Interfaces section.
This close-out's exhaustive sweep (Step 1) found one fixture, not the
matcher, out of step with this rule: `drop_down.txt`'s `row[.activatable]`
line named a required-but-childless node, when a `GtkDropDown` with an empty
model renders no rows at all. The fixture was corrected to
`[row.activatable]` (an optional node with a required class, the same shape
`password_entry.txt`'s `[image.caps-lock-indicator]` already uses) rather
than weakening the matcher.

---

### P5-D31 — two P5 pixel assertions stay `#[ignore]`d for P7's fix wave

**Carried out by:** P5 (`ui/tests/widget_pixels.rs`). **Added:** 2026-08-31,
in the P5 close-out.

**As shipped:** `a_check_button_paints_the_builtin_check_glyph` and
`an_image_paints_its_resolved_icon` are written now, against real
`Builtin`/`icons` call sites, and `#[ignore]`d until P7 fills `Builtin::path`
and `IconTheme::render` (P5-D25, P5-D26).

**Ruling.** Declared, not reverted. §9's P5 boundary already sanctions
deferring icon-dependent pixel assertions to P7; recording the two names here
means P7 cannot close its own tasks without finding and un-ignoring them.
`ui/tests/widget_pixels.rs` carries two further `#[ignore]`s this close-out
did **not** add and does not own —
`opening_a_drop_down_and_picking_an_item_updates_the_button` and
`clicking_a_colour_button_opens_its_dialog_and_repaints_the_swatch` — both
already recorded under P5-D23 as blocked on `view/app.rs::route`'s `aim`, a
framework bug outside every P5 file. The close-out's own gate run therefore
sees four ignored tests in this file, not two; the discrepancy is P5-D23
predating this entry, not a new gap.

**Superseded 2026-08-31 by the P5 whole-part fix wave.** `aim` is fixed
(P5-D33) and both of those tests are un-ignored. `ui/tests/widget_pixels.rs`
now carries exactly the two `#[ignore]`s this entry owns, and P7 is the only
part that may remove them.

---

### P5-D32 — P5 edits four files under `view/`, not one; E4's "no edit except `builders.rs`" is amended

**Carried out by:** P5 (`ui/src/view/controller.rs`, `ui/src/view/reconcile.rs`,
`ui/src/view/app.rs`, plus D5's `ui/src/view/builders.rs`). **Added:**
2026-08-31, in the P5 whole-part fix wave, after a review found the drift
undeclared.

§11 E4 states verbatim that "P5 now makes **no** edit under `view/` except
D5's `pub use` lines in `builders.rs`", and the part plan's File Structure
lists `view/app.rs` as "Untouched". Both were true when written and are false
as shipped. The four changes, none of them reverted:

1. **`Controller<Msg>` gained `std::any::Any` as a supertrait**
   (`controller.rs`). `widgets::child_slot` has to downcast a
   `&dyn Controller<Msg>` to a concrete controller (`PopoverC`,
   `MenuButtonC`, …) to find the node an application child attaches under.
   `Any` as a supertrait, rather than a defaulted `as_any` method, means
   trait-object upcasting does it at the call site with no per-implementor
   boilerplate. This is a change to a P4 public trait: it adds a `'static`
   bound that `Controller<Msg>: 'static` already implied, so every existing
   implementor still compiles, but P6/P7 controllers must remain `'static`
   concrete types (no borrowed fields) — which the trait already required.
2. **`view::build_controller` now delegates to `widgets::build_controller`,
   and `view::generic_controller` was split out of it** (`controller.rs`).
   P5-D29 declared the widgets-side table but not this side of the seam.
   `view::build_controller` keeps its P4 signature and stays the name
   `reconcile`'s `Insert` arm calls; `generic_controller` is the new public
   catch-all the widgets table falls back to, and it no longer runs the props
   loop (its caller does). Contract §11 E1's "one definition, one fallback"
   is preserved: a kind with no controller still lands on `GenericC`.
3. **`reconcile.rs` routes application children through
   `crate::widgets::child_slot(..).unwrap_or_else(|| node.clone())`** in both
   `build_instance` and the update path. P5-D23 describes this walk as if it
   pre-existed P5; it did not. Semantics: a widget whose chrome owns a child
   slot (`Popover`'s `contents`, `MenuButton`'s popover) receives its `View`
   children under that slot instead of directly under its root node.
   `unwrap_or_else` preserves the P4 behaviour exactly for every kind that
   declares no slot.
4. **Four tests were rewritten** — three in `controller.rs`
   (`Kind::Button` → `Kind::MenuButton` in the two tests that mean to
   exercise `GenericC`'s own fallback behaviour, now that `Kind::Button` has
   a real controller; and two class-order assertions flipped, because
   `ButtonC`/`ToggleButtonC` apply `Universal` incrementally where `GenericC`
   rewrites the whole class list) and one in `app.rs` (the same
   `Kind::Button` → `Kind::MenuButton` substitution).

**Ruling.** All four declared, not reverted. (1)–(3) are load-bearing: without
them the shared-surface widgets (`Popover` and the four kinds embedding it)
have no way to receive children and no way to find their own chrome, which is
not an alternate valid implementation of §5.2 but a missing one. (4) is a
consequence of P5 doing its job — the tests still test what they were written
to test, on a kind that is still `GenericC`. **§11 E4 is amended accordingly**
(see its own entry). P6 and P7 may edit `controller.rs`/`reconcile.rs` only to
add arms to the same seams, and must record any further `view/**` change here
rather than relying on E4's original wording.

---

### P5-D33 — `route`'s `aim` resolves to the innermost `Instance`, not the deepest hit node

**Carried out by:** P5 (`ui/src/view/app.rs`, `ui/tests/widget_pixels.rs`).
**Added:** 2026-08-31, in the P5 whole-part fix wave.

**The bug.** `route`'s `aim` closure took `hit_chain(..).last()` — the
geometrically deepest node under the pointer — with no regard for `Instance`
identity. For a widget whose chrome lives on a controller-owned, non-zero-sized
subnode (`DropDown`'s and `MenuButton`'s `button.toggle`,
`ColorDialogButton`'s `button.color`), that node belongs to no `Instance`,
`path_to` returns an empty path, and `deliver` silently drops the event: the
widget never sees its own click. Flat widgets hid this for the whole of P1–P4,
because their content nodes measure to (0, 0) and their deepest hit *is* their
own `Instance` root.

**As shipped:** `aim` now walks the chain innermost-outward and takes the
first node `path_to` can reach:

```rust
chain.iter().rev()
    .find(|hit| !path_to(&rt.instances, &hit.node).is_empty())
    .map(|hit| (hit.node.clone(), hit.local))
```

`hit.local` is already that node's own local point, so a controller's
`local_rect`/`shift_event` re-mapping is unchanged. For every case the old
code delivered, this selects the identical node; it only adds delivery where
there was none.

**Ruling.** Fixed inside P5, not waived, and not deferred. P5-D23 and P5-D31
both recorded this as "outside P5's boundary" — that rationale does not
survive P5-D32, which shows P5 already editing three `view/**` files, and the
alternative (waiving two acceptance criteria) would have let P6 build every
further nested-chrome kind on a dead click path. Both blocked tests
(`opening_a_drop_down_and_picking_an_item_updates_the_button`,
`clicking_a_colour_button_opens_its_dialog_and_repaints_the_swatch`) are
un-ignored and green, each with a re-verified `// mutation:` comment: the
verified mutation is `return Vec::new()` at the top of the controller's
`on_event`, since the narrower state mutations the original comments named do
not move the asserted pixels. Whole-workspace gates re-run green (1418
passing), so no compositor- or harness-side test depended on the old aim.

---

### P5-D34 — `Cmd::OpenPopup` discards its view payload; owned by P6, not P5

**Carried out by:** P6 (`ui/src/view/app.rs`). **Added:** 2026-08-31, in the
P5 whole-part fix wave, to give a previously-informal deferral a named owner.

**The gap.** `Cmd::OpenPopup { positioner, .. }` drops its `view` field on the
floor in `view/app.rs`. `PopoverC::open` therefore maps an autohide popover to
a real, grabbed `Surface::Popup` that paints nothing at all. The P5 close-out
expected Task 18 (`Popover`) to close this; it did not, and could not — the
fix is in `view/app.rs`'s command loop, which has to build and render a second
instance tree against the popup surface, not in any `widgets/**` file.
`window_events.rs::a_popup_opened_from_a_menubutton_takes_the_grab_and_is_dismissed_outside_it`
stays green because it only exercises grab and dismissal, never content.

**Ruling.** Assigned to **P6**, which owns the menus and the popup-hosted
widget kinds that make an empty popup visibly wrong, and which already edits
`view/app.rs` under P5-D32's amended E4. P6 may not close its popup tasks
while `Cmd::OpenPopup`'s `view` is still discarded, and must add a pixel test
that a popup's *content* inks. Until then, `Popover` and the four P5 kinds
embedding it (`MenuButton`, `DropDown`, `ColorDialogButton`,
`FontDialogButton`) ship with correct grab, dismissal and chrome, and no
rendered popup body — a known, recorded limitation of the P5 milestone, not a
regression P6 introduces.

**PARTLY CLOSED 2026-09-01 (§10 P6-D39).** P6's whole-part fix wave wires the
payload: `Runtime` keeps a `PopupSurface` per open popup, `Cmd::OpenPopup`
builds its `view` into it, and
`widget_pixels.rs::a_popup_paints_the_view_its_open_command_carried` is the
required content pixel test. What remains is in two files P6 may not edit —
`PopoverC::open`'s own stub payload (`widgets/popover.rs`, P5) and a
popup-paint seam on `Window` (`window/mod.rs`, P3) — and is assigned to P7 in
P6-D39.

---

### P6-D35 — P6 edits four files under `view/` beyond `builders.rs`; recorded per E4's amendment

**Carried out by:** P6 (`ui/src/view/controller.rs`, `ui/src/view/reconcile.rs`,
`ui/src/view/app.rs`, `ui/src/view/render.rs`). **Added:** 2026-09-01, in the
P6 whole-part fix wave, after a review found the drift undeclared.

The part-6 plan's File Structure table lists only `view/builders.rs` as
P6-editable under `view/`, and §11 E4 (as amended by P5-D32) instructs P6 to
"record any further `view/**` change in §10 the same way". The changes, none
of them reverted, all additive, no gate regressed:

1. **`Controller::child_index` / `Controller::reserved_total`**
   (`controller.rs`, defaulted trait methods). A controller whose chrome
   shares its root node with the application children maps a *logical* child
   index onto a real node index, and tells reconcile's trim step how many
   node children to expect. The defaults (`view_index`, `view_count`) are
   exactly the pre-P6 behaviour, so every P4/P5 controller is unchanged.
2. **`reconcile_reserved`** (`reconcile.rs`), the `reserve`-aware body
   `reconcile` now delegates to. Without it, the first real view child
   reconciled into a widget whose controller had attached chrome to the same
   node detached that chrome (`Frame`'s label, `Paned`'s separator).
3. **`Frames::width` / `Frames::height`** (`app.rs`), so a pixel test can
   derive a coordinate from the frame instead of hard-coding it.
4. **`crate::widgets::flush_layout(tree)`** (`render.rs`), called from
   `layout_tree` immediately after `write_styles`. A widget controller has no
   `&mut LayoutTree`, so its container/gap/homogeneous/child-placement
   decisions are recorded on node-keyed side tables and folded in here.

Three further `view/**` changes were made in the P6 whole-part fix wave and
are recorded in **P6-D36**, **P6-D37** and **P6-D38** below.

**Ruling.** All four declared, not reverted. P7 and P8 may edit these same
seams to add arms, and must record any further `view/**` change here.

---

### P6-D36 — `reconcile`'s `Remove` evicts the widget side tables; the tables are `Weak`-tagged

**Carried out by:** P6 (`ui/src/widgets/mod.rs`, `ui/src/view/reconcile.rs`).
**Added:** 2026-09-01, in the P6 whole-part fix wave.

**The bug.** The six thread-local side tables in `widgets` (`PENDING`,
`CONTAINERS`, `NODE_PROPS`, `NODE_CHILD`, `TRANSITIONS`, `ROW_BINDING`) are
keyed on `node.opaque()` — the `NodeInner` allocation's address — and had no
removal path. Five of them hold no reference to the node they describe, so a
torn-down node's address was handed straight back to the next node built and
that node inherited the dead one's grid cell, container variant,
stack-transition state or pooled row binding (measured: 512 of 512 freshly
built nodes landed on the dead node's address). `PENDING` holds a strong
`Node`, so it leaked instead of aliasing. Reachable in ordinary use: destroy
a `Grid`/`Stack`/`ListBox` child and build another.

**Ruling.** Both halves are fixed together, and the fix crosses the `view/`
boundary, hence this record. Every entry carries a `Weak<NodeInner>` tag: a
stale entry reads as absent, and — because a live `Weak` keeps the allocation
alive — the address cannot be reused while one is held. `reconcile`'s
`Remove` arm calls the new `widgets::forget_subtree` on the instance's node,
and `flush_layout` sweeps dead entries once per frame for whatever a
controller dropped on its own.

---

### P6-D37 — E6 is honoured by converging on P6's fixture slack, not by discarding it

**Carried out by:** P6 (`ui/src/widgets/node_tree.rs`, `ui/src/widgets/mod.rs`,
`ui/tests/node_trees.rs`). **Added:** 2026-09-01, in the P6 whole-part fix
wave.

**The gap.** §11 E6 rules that P6's Task 3 relocates P5's `node_tree_of`,
`render_node_tree` and `fixture_matches` into `widgets/node_tree.rs`
unchanged, re-exports all three, and adds `matches_fixture` as a thin
node-first wrapper — "there is exactly one matcher implementation". P6 left
P5's three functions in `widgets/mod.rs` and wrote a second, independent
backtracking matcher, so P5's 32 fixtures and P6's 27 were validated by two
matchers with different slack semantics.

**Ruling.** E6 is carried out as written: the three functions moved verbatim,
`matches_fixture` renders and delegates, and the second implementation
(`Pattern`, `parse_fixture`, `match_list`, `render_tree`) is deleted. Two
generalisations were needed for P5's matcher to accept P6's vendored blocks,
both of rules it already had, and both now apply to all 59 fixtures:

* any line opening with a `<...>` token is an application-supplied subtree
  (`<overlay child>[.left]`, `<column header>`, not just `<child>`);
* a fixture line with no children of its own describes a subtree GTK's block
  elides, so that node's rendered descendants are not checked.

The surviving matcher is path-based and therefore does **not** check sibling
order; the two order-sensitive facts P6 depends on carry their own
assertions (`frame::tests::a_titled_frame_is_a_label_then_the_child`,
`scrolled_window::tests::chrome_follows_the_application_child_in_gtks_declared_order`).
`node_tree_of` also builds through `build_widget` now, so the fixture gate
and the widget modules share one `Headless` environment (bundled Adwaita
light) instead of diverging on an empty sheet. **This matcher is normative
for P8's gallery gate.**

---

### P6-D38 — E8's extension-trait convention, applied to P6's containers; argument types stay `impl Into<Prop>`

**Carried out by:** P6 (`ui/src/view/builders.rs`, `ui/src/widgets/expander.rs`,
and the six widget modules whose tests import the new traits). **Added:**
2026-09-01, in the P6 whole-part fix wave.

**The gap.** §11 E8 withdraws part-6 plan deviation 13 and rules that P6
ships per-widget extension traits, warning that "a trait method is shadowed
by an inherent method of the same name". P6 shipped both conventions, and the
predicted collision landed: the inherent `View::position` shadowed P5's
`PopoverExt::position` and the inherent `View::label` shadowed
`ButtonExt::label`, making both unreachable public API.

**Ruling.** Ten inherent `impl<Msg> View<Msg>` blocks in `builders.rs` become
thirteen traits — `BoxExt`, `GridExt`, `CenterBoxExt`, `FrameExt`,
`OverlayExt`, `PanedExt`, `PackExt`, `ScrolledWindowExt`, `StackExt`,
`ListBoxExt`, `ListViewExt`, `FlowBoxExt`, `GridViewExt` — plus `expanded`
folded into the existing `ExpanderExt`. Three consequences are recorded
rather than hidden:

1. **Argument types stay `impl Into<Prop>`** except where E8's own worked
   example demands otherwise (`PanedExt::position(i32)`, against
   `PopoverExt::position(Position)`). Narrowing the rest would change which
   `Prop` variant reaches a controller that already reads `Int` or `Float`
   from the same setter, which is a behaviour change no review asked for.
2. **There is no `FrameExt::label`.** Two traits in scope defining one method
   name make every call site ambiguous (Rust resolves inherent-then-trait,
   and among traits it is an error), so the universal `PropName::Label`
   setter stays P5's `ButtonExt::label`, which has identical semantics.
3. **P4's eighteen inherent `on_*` setters are untouched** — E8 names them as
   the inherent namespace.

---

### P6-D39 — `Cmd::OpenPopup`'s payload is wired; `PopoverC`'s own payload is still a stub

**Carried out by:** P6 (`ui/src/view/app.rs`, `ui/tests/widget_pixels.rs`).
**Added:** 2026-09-01, in the P6 whole-part fix wave. **Closes P5-D34 in
part.**

`Runtime` now keeps a `PopupSurface` per open popup — its own root, styles,
animations, container map, layout tree and instances — `open_popup`
reconciles `Cmd::OpenPopup`'s `view` payload into it, and `Cmd::ClosePopup`
drops that popup and every popup opened after it. Offscreen the popup is
composited over the window at its anchor rectangle's bottom-left, which is
what makes P5-D34's required pixel test possible:
`widget_pixels.rs::a_popup_paints_the_view_its_open_command_carried`.

Two limits remain, both outside P6's file list:

1. **`PopoverC::open` still passes `|| View::new(Kind::Popover)`** — an empty
   payload — so `Popover` and the four P5 kinds embedding it still open a
   blank popup. `ui/src/widgets/popover.rs` is a P5 per-widget file, which §9
   forbids P6 from editing. **P7 owns this**; the mechanism it needs now
   exists.
2. **The windowed loop builds the payload into `Window::popup_root(key)` and
   marks it dirty, but `App::run` cannot paint a popup surface**: it drives
   the main surface through `Window::paint_with`, and `Window` exposes no
   popup equivalent (`render_popups` is private and runs only from
   `Window::render`, which `App::run` must not call — it would repaint the
   main surface from the window's own style map). A
   `Window::paint_popup_with(key, f)` seam in `ui/src/window/mod.rs` — a
   P6-forbidden file — closes it. **P7 owns this too.**

---

### P7-D40 — `Builtin::path` returns `skia_rs_safe::path::Path`, not `core::Path`

**Carried out by:** P7 (`ui/src/icons/builtin.rs`). **Added:** 2026-09-01, in
the P7 whole-part fix wave (recorded late; the code landed with the part).

§6 prints `pub fn path(self, size: f32) -> skia_rs_safe::core::Path`. There is
no `Path` in `skia_rs_safe::core`: the type lives in `skia_rs_safe::path`
alongside `PathBuilder` and `FillType`, which is what `ui/src/paint/geometry.rs`
already imports. **Ruling:** a spelling correction — same type, correct module
path. No caller is affected.

### P7-D41 — `icons::Handle` and `icons::IconSize` are defined in `icons/mod.rs`

**Carried out by:** P7 (`ui/src/icons/mod.rs`). **Added:** 2026-09-01.

§5.1's `ImageC { resolved: Option<icons::Handle>, .. }` and the `Image`
builder's `.icon_size(IconSize)` name two types §6 never declares.
**Ruling:** `pub type Handle = Rc<skia_rs_safe::codec::Image>;` — exactly what
`IconTheme::render` returns — and `pub enum IconSize { Inherit, Normal, Large }`
with `pixels(self, inherited: u32) -> u32` (16 / 32, GTK4's `GtkIconSize`).
Additive; P5-D26 already types `ImageC.resolved` as the same `Rc<Image>`.

### P7-D42 — `IconTheme` carries the output scale, and the window writes it

**Carried out by:** P7 (`ui/src/icons/theme.rs`, `ui/src/window/mod.rs`,
`ui/src/view/app.rs`). **Added:** 2026-09-01.

§6's `paint_icon` doc says "HiDPI comes from `-gtk-scaled()` and the output
scale on `cx`", but the same section freezes `PaintCx`'s only new field as
`icons`, and neither `PaintCx` nor `ResolveEnv` has a scale. **Ruling:**
`IconTheme` gains `scale() -> u32` and `set_scale(&mut self, scale: u32)`
(clamped to `1..=4`, default `1`, cache-clearing on a real change);
`paint_icon` reads `cx.icons.scale()`. `PaintCx` still gains exactly one field.

**Both halves are wired.** `Window::open` seeds the scale from the
`wl_surface.preferred_buffer_scale` the first configure round already
delivered, and `Window::pump` calls `set_scale` on every later
`InputEvent::ScaleChanged` (and marks the window dirty, so the next frame
re-rasterises). `App::run` does the same for the theme it owns. `WindowState.scale`
loses its `#[allow(dead_code)]` and its "Task 11 updates this" comment.

### P7-D43 — hermetic themed roots are split from the flat pixmap fallback

**Carried out by:** P7 (`ui/src/icons/theme.rs`). **Added:** 2026-09-01.

§6 documents `with_name_and_roots(name, roots)` as "no env reads at all" and
also lists `/usr/share/pixmaps` among the roots — but pixmaps is not a themed
root (no `index.theme`, no subdirs) and a test built with
`with_name_and_roots` must not read `/usr`. **Ruling:** `IconTheme` stores
`roots: Vec<PathBuf>` (themed) and `pixmaps: Vec<PathBuf>` (flat) separately;
`with_name_and_roots` fills `roots` and leaves `pixmaps` empty; `from_env`
fills both; `with_name_roots_and_pixmaps(name, roots, pixmaps)` is added so the
flat fallback is testable hermetically. `with_name_and_roots`'s frozen
signature and semantics are unchanged.

### P7-D44 — `IconEnv` exists so `from_env` is testable

**Carried out by:** P7 (`ui/src/icons/theme.rs`). **Added:** 2026-09-01.

`from_env()` reads the process environment, which a test cannot vary safely
(`std::env::set_var` is `unsafe` in edition 2024 and racy across the harness's
threads). **Ruling:** `pub struct IconEnv { xdg_data_home, xdg_data_dirs, home,
xdg_config_home }` with `IconEnv::from_env()`, plus
`IconTheme::from_icon_env(&IconEnv)`; `IconTheme::from_env()` is exactly
`Self::from_icon_env(&IconEnv::from_env())`. The same shape `app::ThemeEnv`
already uses for the same reason. Additive.

### P7-D45 — XPM is looked up but never decoded

**Carried out by:** P7 (`ui/src/icons/{lookup,render}.rs`). **Added:** 2026-09-01.

§6's `IconFormat` has an `Xpm` variant and the spec's extension order is
`png, svg, xpm`, but `skia-rs-codec` has no XPM decoder. **Ruling:** `lookup`
returns `IconFormat::Xpm` when that is what the spec's search finds — the
lookup stays spec-exact — and `render` returns `None` for it after a one-time
log, so `IconTheme::render` falls through to `image-missing` exactly as it does
for a corrupt PNG. Documented on both.

### P7-D46 — `recolour` also neutralises what would undo it

**Carried out by:** P7 (`ui/src/icons/symbolic.rs`). **Added:** 2026-09-01.

`render_svg_in_container` re-applies `dom.stylesheet`, and `apply_stylesheet`
applies each node's inline `style=` attribute *after* the sheet. A recolour
that only ran a synthetic sheet over a clone would be undone at render time by
any symbolic SVG carrying a `<style>` block or an inline `fill`. **Ruling:**
`recolour` (a) applies the synthetic sheet to the clone, (b) appends the same
rules to the clone's `stylesheet` so the re-application inside
`render_svg_in_container` ends on ours, and (c) removes the `fill` declaration
from the inline `style=` of every node it recoloured. All three operate on the
clone: the cached parse is never mutated and the SVG source is never
string-patched, which is what §6 forbids.

### P7-D47 — `paint::icon::paint_builtin` is added beside `paint_icon`

**Carried out by:** P7 (`ui/src/paint/icon.rs`). **Added:** 2026-09-01.

`-gtk-icon-source: builtin` computes to `Value::Keyword(Keyword::Builtin)`,
not to an `IconRef`, so `paint_icon(.., icon: &IconRef, ..)` cannot express it;
and the keyword alone does not say *which* shape. **Ruling:**
`pub fn paint_builtin(canvas, builtin: Builtin, rect: layout::Rect,
style: &ComputedStyle, cx: &mut PaintCx<'_>)`, honouring the same
`-gtk-icon-transform`/`-gtk-icon-filter`/`-gtk-icon-shadow` stack `paint_icon`
does. Additive.

### P7-D48 — `ImageCache::resolve_path` is added

**Carried out by:** P7 (`ui/src/paint/mod.rs`). **Added:** 2026-09-01.

`-gtk-recolor(url(...))` needs the same stylesheet-relative URL resolution
`background-image: url()` gets, and `ImageCache::resolve` is private.
**Ruling:** a two-line `pub fn resolve_path(&self, url: &str) -> PathBuf`
wrapper over the existing private `resolve`. No behaviour changes and no
existing signature moves.

### P7-D49 — `Palette::for_color`, with the *vendored* sheet's status colours

**Carried out by:** P7 (`ui/src/icons/theme.rs`). **Added:** 2026-09-01.

`Palette::from_style` needs a `ComputedStyle`, but three call sites
(`Builtin::draw`, the recolour unit tests, and `paint_icon`'s `-gtk-recolor()`
palette override) only have a foreground `Rgba`. **Ruling:**
`pub fn for_color(foreground: Rgba) -> Palette` returns the foreground with the
theme's default success/warning/error, and `from_style` layers
`-gtk-icon-palette` and the sheet's `ColorTable` over it.

**Correction to the P7 plan's own text, recorded rather than silently kept.**
The plan's deviation 10 names those defaults as `#26a269`, `#cd9309`,
`#e01b24`. Those are the GNOME 42 palette. This tree's vendored sheet declares
`@warning_color #f57900`, `@error_color #cc0000`, `@success_color #33d17a`
(`ui/themes/adwaita-light.css:1918-1920`), and the shipped constants are those
— which is the correct choice, because the whole point of the defaults is that
"an icon rendered without a sheet in hand matches one rendered with it". The
plan's three hexes are wrong for this tree and are superseded here.
`render.rs`'s and `symbolic.rs`'s `test_palette()` deliberately keep the GNOME
42 hexes, precisely *because* they differ from the defaults: a test that reads
a slot back then proves the slot was used rather than that a default matched.
Both now say so in a doc comment.

### P7-D50 — `paint::icon::paint_icon_image` is added for background layers

**Carried out by:** P7 (`ui/src/paint/{icon,background}.rs`). **Added:** 2026-09-01.

`background-image: -gtk-icontheme(…)` is painted by `paint::background::
paint_layer`, whose M2-frozen signature carries the node's `currentColor` but
not its `ComputedStyle`, so it cannot call `paint_icon(.., style, ..)`.
**Ruling:** a style-less sibling,
`pub fn paint_icon_image(canvas, icon: &IconRef, rect: layout::Rect,
palette: &Palette, cx: &mut PaintCx<'_>)`, takes the palette the caller built
from `currentColor` and uses the layer's own tile rectangle as the icon box.
The alternative — threading a `ComputedStyle` into `paint_layer` — would change
an M2 paint signature §8 freezes.

### P7-D51 — the additive items §6 does not print

**Carried out by:** P7. **Added:** 2026-09-01.

§6 freezes an interface, and a part "may add private items freely". These are
the **public** additions P7 made that §6 does not print, listed so no later
part mistakes one for frozen text:

| item | file | why |
|---|---|---|
| `paint::icon::resolve_icon` | `paint/icon.rs` | the resolve half of `paint_icon`, so `ImageC` can keep a `Handle` between frames (§5.1's `resolved` field) |
| `paint::icon::icon_size_px` | `paint/icon.rs` | `-gtk-icon-size`, or the largest square that fits; read by `paint_icon`, `paint_builtin` and `paint_icon_source` |
| `paint::icon::paint_icon_source` | `paint/icon.rs` | the production reader of `Prop::GtkIconSource`; see P7-D52 |
| `icons::Builtin::for_node` | `icons/builtin.rs` | which builtin shape a CSS node means; see P7-D52 |
| `IconTheme::render_path` (`pub(crate)`) | `icons/theme.rs` | `-gtk-recolor(url(…))` names a file, not an icon name, and must share the caches |
| `IconTheme::dom_for` (private) | `icons/theme.rs` | the `(path, mtime)` parse cache §6's "Cache keys" paragraph requires |
| `Palette::key` (`pub(crate)`) | `icons/theme.rs` | the `[u32; 4]` hashable form of a palette, for `RenderKey` |
| `icons::SubDir`, `icons::ThemeIndex` | `icons/theme.rs` | the parsed `index.theme`; §6 names `Directories`/`ScaledDirectories` semantics but no type |
| `icons::MAX_ICON_PX` | `icons/mod.rs` | the untrusted-input ceiling the Global Constraints require |
| `icons::Handle`, `icons::IconSize` | `icons/mod.rs` | see P7-D41 |

### P7-D52 — P7 edits `ui/src/widgets/**`; the plan's Global Constraint yields to §9

**Carried out by:** P7 (`ui/src/widgets/{image,check_button,spin_button,popover,drop_down,menu_button,popover_menu_bar}.rs`,
`ui/src/paint/mod.rs`). **Added:** 2026-09-01, in the P7 whole-part fix wave.

The P7 plan's Global Constraints say "P7 must not touch widget behaviour
(`ui/src/widgets/**`)". §9's P5 boundary says a P5 pixel test that depends on
real builtin geometry "is deferred to **P7's fix wave**", and §9's P8 boundary
says P8 "must not touch any widget implementation". The two cannot both hold:
the deferred work is *in* the widgets and no part after P7 may do it.
**Ruling: §9 wins.** P7's fix wave makes the minimum widget edits that let
§6's "single entry point every icon-shaped paint goes through" actually be one:

1. **`paint_node_with_children` reads `node`** and calls the new
   `paint::icon::paint_icon_source(canvas, node, alloc.content_box, style, cx)`
   once per node, between the outline and the text. That is the third of §6's
   three doors and the *only* production reader of `Prop::GtkIconSource`; it
   covers `check`, `radio`, `arrow` and `expander` subnodes uniformly, from
   each subnode's own computed style, which is what makes an expander's
   `-gtk-icon-transform: rotate(90deg)` apply. `-gtk-icon-source: builtin`
   picks its shape through `Builtin::for_node` (element name, plus
   `:indeterminate` and an `arrow`'s direction class). It also closes §8.1's
   "`paint_node`'s unused `node` param" row: the
   `#[allow(unused_variables)]` on `paint_node_with_children` is **removed**.
2. **`ImageC::resolve` really resolves** — a build-time prefetch through
   `cx.icons.render`/`render_path` with the default palette — and
   **`ImageC::paint` draws through `paint::icon::{resolve_icon, paint_icon}`**,
   re-resolving against the node's real palette so a symbolic icon follows
   `color`. Closes the "`IconTheme::render` is P7's" stub.
3. **`CheckButtonC::paint` goes through `paint_builtin`**, not `Builtin::draw`,
   so the indicator honours the `-gtk-icon-*` stack. It also gains a
   `measure` returning `INDICATOR_PX` (14, Adwaita's
   `check { -gtk-icon-size: 14px }`): `-gtk-icon-size` is not a layout
   property and `layout.rs` never sees the `check` subnode's style, so without
   an intrinsic size the indicator laid out 0x0 and no glyph could be drawn at
   all.
4. **`SpinButtonC::paint` draws its two steppers** through `paint_builtin`,
   into the same rectangles `stepper_rects` gives the pointer. Replaces a dead
   `let _ = (Builtin::SpinPlus, Builtin::SpinMinus);`.
5. The popover payload edits, recorded separately as P7-D54.

No widget's *event* behaviour changes, and no assertion in `node_trees.rs`
moves.

### P7-D53 — E14 is superseded: `Button::render` keeps its M2 signature

**Carried out by:** P7 (`ui/src/widget/button.rs`, `ui/src/window/layer.rs`).
**Added:** 2026-09-01, in the P7 whole-part fix wave.

E14 authorised `ui/src/widget/button.rs`'s own `PaintCx { .. }` literals to
gain an `icons` field, and stated that "the §8.2 gate is unaffected —
`ui/tests/themed_button_offscreen.rs` … constructs no `PaintCx` and stays
byte-identical". As first landed, P7 had instead given `Button::render` a
fifth parameter, which forced an edit to the gate file. That is not what E14
allows.

**Ruling.** `Button` **owns** its icon theme: a private
`icons: crate::icons::IconTheme` field, initialised
`with_name_and_roots("hicolor", vec![])`, read as `&mut self.icons` inside
`render`. `Button::render`'s M2 signature is restored byte for byte, and
`ui/tests/themed_button_offscreen.rs` is reverted to its pre-P7 bytes. E14's
last paragraph now holds as written. The M1 gate's button draws no icon, so a
hermetic, root-less theme is the honest theme for it.

Consequence: `window/layer.rs`'s `AppState` no longer needs an `icons` field at
all, and it is removed — which also disposes of the "zero roots, so every
lookup misses" half of the T13 sign-off note there.

### P7-D54 — P6-D39 is closed: real popover payloads, and `Window::paint_popup_with`

**Carried out by:** P7 (`ui/src/widgets/popover.rs` and its three callers,
`ui/src/window/mod.rs`, `ui/src/view/app.rs`). **Added:** 2026-09-01, in the
P7 whole-part fix wave. **Closes P6-D39 (both limits) and P5-D34 in full.**

**Limit 1 — the stub payload.** `PopoverC::open` no longer hard-codes
`|| View::new(Kind::Popover)`. It takes the payload explicitly:

```rust
pub fn open<Msg: Clone + 'static>(&mut self, anchor: PopupAnchorPoint,
                                  size: (u32, u32),
                                  content: Option<Rc<dyn Fn() -> View<Msg>>>,
                                  cx: &mut EventCx<'_, Msg>);
```

`Some(view)` opens a real `xdg_popup` whose surface is reconciled from that
view. `None` means **this popover's body is already retained in the parent
window's own tree**, and then no compositor surface is asked for at all.

That `None` is the honest answer for every popover this crate embeds, and it
is why the blank popup existed in the first place: `MenuButtonC`'s contents are
its own application children, `DropDownC` appends
`popover > contents > listview > row…` under its own node, and
`PopoverMenuBarC`'s menus are its own subnodes — all of them already laid out,
hit-tested and painted in the parent tree (which is what
`widgets::drop_down::tests::opening_a_drop_down_and_clicking_a_row_selects_that_item`
drives end to end). A second surface carrying a *copy* of that content would
double-draw it; a second surface carrying a stub drew a blank rectangle over
it. Opening nothing is both correct and cheaper. The three call sites pass
`None` with that reason stated inline, and the DropDown unit test's final
assertion is inverted from "opening and closing both go through `cx.cmds`" to
"an embedded popover opens no compositor surface", with its own mutation check.

An application whose popover body is a `View` — not in the parent tree — passes
`Some` and gets the full P6 machinery: `Cmd::OpenPopup`'s payload built into
its own `PopupSurface`, composited offscreen and given a real buffer in a
windowed run.

**Limit 2 — the missing paint seam.** `ui/src/window/mod.rs` gains

```rust
pub fn paint_popup_with(&mut self, key: PopupKey,
                        f: impl FnOnce(&mut skia_rs_safe::canvas::Surface))
    -> Result<bool, SurfaceError>;
pub fn popup_size(&self, key: PopupKey) -> Option<(u32, u32)>;
```

the popup equivalent of `Window::paint_with`: it folds any pending
`xdg_popup.configure`, refuses to attach before the popup's first
`xdg_surface.configure` (`Ok(false)`, never a protocol kill), grows the
popup's own pool and raster surface, runs `f`, uploads and commits.
`App::run` now restyles, relayouts and paints every open `PopupSurface`
through it after each main-surface frame, at the popup's own `(0.0, 0.0)`
origin. `Window::render`'s private `render_popups` is unchanged and still
serves the non-reactive path.

### P7-D55 — `Window` selects a real icon theme, and hands it to `App::run`

**Carried out by:** P7 (`ui/src/window/mod.rs`, `ui/src/view/app.rs`).
**Added:** 2026-09-01, in the P7 whole-part fix wave.

As first landed, `Window`'s `icons` field was
`with_name_and_roots("hicolor", vec![])` — zero roots, so every lookup missed,
`image-missing` included — with no setter and no named later task. Any client
driving a `Window` directly (the shell and settings clients this rebuild
exists for) would have painted no icons, ever.

**Ruling.** `Window::open` builds `IconTheme::from_env()` and seeds its scale
from the first `preferred_buffer_scale` (P7-D42). Three accessors make the
choice a client's:

- `Window::icons(&mut self) -> &mut IconTheme`,
- `Window::set_icon_theme(&mut self, IconTheme)` — keeps the announced scale,
  marks the window dirty,
- `Window::take_icon_theme(&mut self) -> IconTheme` — takes it, leaving the
  hermetic placeholder.

`App::run` uses `take_icon_theme` when the caller supplied none through
`App::with_icons`, so the window and the reactive loop can never disagree
about which theme is in use; what is left behind is only ever read by
`Window::render`, which the reactive loop never calls.

### P7-D56 — `RenderKey` includes `symbolic`

**Carried out by:** P7 (`ui/src/icons/theme.rs`). **Added:** 2026-09-01, in the
P7 whole-part fix wave.

§6's "Cache keys" paragraph gives `IconTheme::render`'s memo key as
`(IconFile.path, mtime, size, scale, palette)`. That is not sufficient:
`render_path` takes `symbolic` as a parameter and `paint/icon.rs` calls it both
ways for the same path — `true` for `-gtk-recolor(url(x.svg))`, `false` for a
`url()` arm of `-gtk-scaled()`. A sheet using both at one size and palette got
whichever rendered first, so the second was silently recoloured when it should
have been verbatim, or verbatim when it should have been recoloured.
**Ruling:** `symbolic: bool` joins the key. Pinned by
`a_recoloured_render_and_a_verbatim_one_are_not_the_same_cache_entry`
(mutation check stated in the test).

### P7-D57 — P5-D31's two `#[ignore]`d pixel tests are un-ignored, hermetically

**Carried out by:** P7 (`ui/tests/widget_pixels.rs`). **Added:** 2026-09-01, in
the P7 whole-part fix wave. **Closes P5-D31.**

`an_image_paints_its_resolved_icon` ("P7 fills in `IconTheme::render`") and
`a_check_button_paints_the_builtin_check_glyph` ("P7 fills in `Builtin::path`")
both name work P7 completed, and §9's P5 boundary makes P7's fix wave the last
part permitted to close them. Both are un-ignored and both carry a mutation
check.

They are made **hermetic** in the process, which is a change to how the whole
file runs: `widget_pixels.rs`'s shared `run` helper now passes
`App::with_icons(IconTheme::with_name_and_roots("hicolor", Vec::new()))`
instead of letting `run_offscreen` fall back to `IconTheme::from_env()`. Before
P7 that fallback was inert (nothing drew icons); after P7 it would have made
every pixel assertion in the file depend on the machine's installed icon set,
which contradicts §6's "the installed Adwaita theme is exercised only when
present". `an_image_paints_its_resolved_icon` builds its own `App` against the
checked-in `tests/fixtures/mini-icon-theme` instead of asking for
`image-missing` from the machine.

### P8-D58 — `ui/src/gallery.rs` is added as a library module

**Carried out by:** P8 (`ui/src/gallery.rs`, new; `ui/src/bin/gallery.rs`,
new). **Added:** 2026-09-02, in the Part 8 whole-part fix wave.

**§0's module map gives P8 only `ui/src/bin/gallery.rs`.**

**As shipped:** the sample table and every exhaustive `match kind { … }` live
in `ui/src/gallery.rs`, a library module in the same crate as `Kind`;
`ui/src/bin/gallery.rs` is CLI plumbing only (argv → `Options` → `gallery::*`).

**Ruling.** Forced by `Kind` being `#[non_exhaustive]` (§4.2) — that attribute
bites *across crates*, so a binary target (a separate crate from the library)
could not exhaustively match `Kind` without a wildcard arm, and the contract's
own "adding a `Kind` without a gallery entry is a compile-time hole" guarantee
would evaporate. Keeping the match in-crate is the only way to keep that
guarantee.

### P8-D59 — `App::probe` and `Probe<Msg>` are added to `ui/src/view/app.rs`

**Carried out by:** P8 (`ui/src/view/app.rs`). **Added:** 2026-09-02, in the
Part 8 whole-part fix wave.

**§7 says** "This is how the gate learns coordinates — it never hard-codes",
extending the M2 `--print-allocation` precedent (headless), but names no
accessor for it and §4.7 does not list one.

**As shipped:** `App::probe` runs the reconcile → restyle → layout pipeline
once, headlessly, and hands back the retained tree plus its `LayoutTree`,
through a new `Probe<Msg>` type.

**Ruling.** Additive to a P4-owned file, nothing renamed or re-typed.
`--probe-points` and `--print-allocation` must produce geometry without a
compositor, and the whole pipeline that produces it is otherwise private to
`App`; reimplementing it inside the gallery would duplicate P4's reconcile/
restyle/layout sequence rather than reuse it. `App::run_offscreen` already
existed but returns pixels (`Frames`), not allocations, so it does not serve
this need.

### P8-D60 — probe points are derived by the gallery, not exposed per widget

**Carried out by:** P8 (`ui/src/gallery.rs::probe_points_of`). **Added:**
2026-09-02, in the Part 8 whole-part fix wave.

**§7 says** "Every widget exposes its probe points as `(label, node)` pairs",
but §9 also says P8 "**Must not touch:** any widget implementation." Both
cannot hold — exposing probe points per widget means editing every widget.

**As shipped:** `gallery::probe_points_of(instance, tree)` derives them from
the retained `Node` subtree instead: `"root"` for the widget's own node, then
each descendant labelled by its CSS node name, with `<name><index>` for every
repeated name in that subtree (`tab0`, `tab1`, `row0`).

**Ruling.** This reproduces §7's own worked examples (`"check"`, `"slider"`,
`"trough"`, `"text"`, `"arrow"`, `"tab0"`, `"row0"`) while touching no widget
file, and stays derived rather than hard-coded, which is the property §7's
sentence about "the gate never hard-codes" actually cares about.

### P8-D61 — `support::probe_points`/`spawn_gallery` signatures widen

**Carried out by:** P8 (`ui/tests/support/mod.rs`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**§7 lists** `probe_points(theme: &str)` and
`spawn_gallery(socket, theme) -> Reaper`.

**As shipped:**
`probe_points(theme: Theme, widget: Option<&str>)` and
`spawn_gallery(socket, theme, scroll) -> GalleryProc`, where `GalleryProc` is a
`Reaper` plus the spawned child's captured stdout.

**Ruling.** The interaction gate needs the isolated `--widget` layout's
coordinates, which the two-argument form cannot ask for. `GalleryProc` exists
because §7's "screencopy **or model** assertions" require reading the
messages the app folded, which only cross the process boundary on the child's
stdout. `Reaper` itself is unchanged and still used unmodified by the M2
tests.

### P8-D62 — the gallery maps a `zwlr_layer_shell_v1` overlay, not an xdg-toplevel

**Carried out by:** P8 (`ui/src/bin/gallery.rs`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**§7 does not name a surface role** for the gallery binary.

**As shipped:** the gallery maps a layer-shell overlay (anchor top|left,
margin 0), keyboard-interactive `Exclusive`.

**Ruling.** Probe coordinates are output coordinates, and only the layer role
has a compositor-independent origin — the same reasoning behind M2's
`themed-button` precedent. `Exclusive` keyboard interactivity is required so
the typing/Tab interactions in the interaction gate get a focused keyboard.

### P8-D63 — `every_probe_point_differs_between_light_and_dark` is asserted per widget

**Carried out by:** P8 (`ui/tests/gallery_gate.rs`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**§7 names a test** `every_probe_point_differs_between_light_and_dark`, read
literally asserting every probe point on every widget differs between themes.

**As shipped:** the test keeps its contract name but asserts **at least one
probe point per widget** differs by more than `SCREENCOPY_TOLERANCE`, naming
the failing widget if none does.

**Ruling.** The literal reading is false on its face: a fully transparent
subnode, or one Adwaita styles identically in both sheets, legitimately
matches across themes without indicating a broken theme. Requiring only one
differing probe point per widget still proves the widget is theme-reactive at
all, which is what the test exists to catch.

### P8-D64 — `--probe-points`/`--print-allocation` line formats are pinned

**Carried out by:** P8 (`ui/src/gallery.rs`). **Added:** 2026-09-02, in the
Part 8 whole-part fix wave.

**§7 pins** `--probe-points` as `<widget> <label> <x> <y>` but leaves
`--print-allocation` as "one allocation per line", with no format.

**As shipped:** `--print-allocation` prints
`<widget> <x> <y> <width> <height>` — the entry's border box, in page
coordinates.

**Ruling.** This is the exact shape the rest-state gate's page-slice
arithmetic needs (it must know each entry's height to compute slice overlap —
see P8-D70); §7 left the format unspecified, so pinning it is filling a gap,
not overriding a stated format.

### P8-D65 — one test is added to `gallery_gate.rs` beyond §7's six

**Carried out by:** P8 (`ui/tests/gallery_gate.rs`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**§7 names six `gallery_gate.rs` tests.**

**As shipped:** a seventh, `the_readme_widget_table_lists_every_kind`, is
added alongside them.

**Ruling.** All six of §7's named tests remain and are implemented verbatim;
the seventh is additive and keeps `ui/README.md`'s widget table (P8's own
documentation deliverable) from rotting the moment a `Kind` is added, by
asserting the table against `Kind::all()`.

### P8-D66 — `harness/src/lib.rs` grows `VirtualPointerClient::{axis, axis_discrete}`

**Carried out by:** P8 (`harness/src/lib.rs`). **Added:** 2026-09-02, in the
Part 8 whole-part fix wave.

**§9's P2 boundary** does not anticipate P8 adding to the harness crate.

**As shipped:** two additive methods, `VirtualPointerClient::axis` and
`::axis_discrete`. No existing harness method is renamed, re-typed, or
removed, and no existing harness test (§8.2) is touched.

**Ruling.** The contract's own interaction-gate list requires
`scrolling_a_list_view_recycles_rows_without_losing_selection`, and the
harness injector had motion/button/frame methods but no scroll-axis event at
all before this change. Additive-only, so it costs the M2/P2 harness tests
nothing.

### P8-D67 — ASSUMED types the contract names but never defines

**Carried out by:** P3/P5/P6 (definitions); recorded by P8. **Added:**
2026-09-02, in the Part 8 whole-part fix wave.

**The contract names, but never defines:** `SurfaceSpec` (§3.1
`Window::open`'s first parameter), `ListItem` (§4.3 `Prop::Items`), the
list/grid/column `factory` parameter, and the widget enums `MessageType`,
`Orientation`, `Position`, `Side`, `SelectionMode`, `StackTransition`,
`ContentFit`, `Ellipsize`, `Policy`, `LevelBarMode`, `ArrowDirection`, `Rgba`,
`MenuFlags`, `DisplayHint`, `MatchMode`, `SortOrder`, `Sorter`, `IconSize`,
`WrapMode`.

**As shipped:** P3/P5/P6 defined all of them before P8 started; P8 constructs
values of each type by reading the shipped definition at the call site
(marked `// ASSUMED` where the plan does so) rather than inventing a
replacement.

**Ruling.** Recorded for completeness per the plan's own instruction ("record
it in the contract's §10"); no gap was found — every named type exists in
`ui/src/window/mod.rs`, `ui/src/view/mod.rs`, or `ui/src/widgets/*.rs` by the
time P8 runs, so no gate logic depends on an undefined type.

### P8-D68 — `App::run` is called with the `Window`, not a `Surface`

**Carried out by:** P8 (`ui/src/gallery.rs::run`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**§4.7 spells it** `App::run(self, surface: Surface)`.

**As shipped:** the gallery calls `App::run(window)`.

**Ruling.** Not a P8 deviation from what P4 shipped — P4-D21 already
established that `App::run` needs the `CompiledSheet`, `FontDatabase`, clock
and `AnimationState` that only `Window` owns (§3.1), none of which `Surface`
exposes. P8 records the call-site consequence: the gallery constructs a
`Window` either way, so passing it to `App::run` costs nothing beyond what
P4-D21 already paid for.

### P8-D69 — `every_widget_renders_at_rest` excludes fifteen widgets from its paint assertion

**Carried out by:** P8 (`ui/tests/gallery_gate.rs`, `KNOWN_BLANK_AT_REST`).
**Added:** 2026-09-02, in the Part 8 whole-part fix wave.

**§7's rest-state gate implies every widget in the gallery paints
non-background pixels at rest.**

**As shipped:** fifteen widgets are excluded from that one assertion via a
measured (not inferred) `KNOWN_BLANK_AT_REST` list — every entry's
non-background pixel count was recorded, not asserted on, in one real-harness
pass over the whole page, and every entry that came back at zero is listed:

- Thirteen collapse to a zero-area allocation (`progress_bar`, `scrollbar`,
  `window_controls`, `color_dialog`, `font_dialog`, `stack_switcher`,
  `stack_sidebar`, `list_view`, `grid_view`, `popover_menu`,
  `popover_menu_bar`, `about_dialog`, `alert_dialog`) — a layout defect, not a
  rendering one; root-caused for `progress_bar`
  (`ui/src/widgets/progress_bar.rs`): `measure()` delegates entirely to an
  unshaped label when `show_text(true)` instead of reporting the trough's own
  intrinsic size.
- Two have a real allocation and still draw nothing: `link_button`
  (`ui/src/widgets/link_button.rs` appends a `label` subnode but never gives
  it text) and `check_button` (`ui/src/widgets/check_button.rs`'s `paint`
  returns `false` when neither active nor inconsistent, and the sample starts
  unchecked).

**Ruling.** `separator` and `calendar` are deliberately **not** excluded even
though both look zero-ish (1x1 and 2x2) — both paint, once
`paints_something` was widened to scan the whole border box rather than an
inset 5x5 grid (the same change also fixed a false negative on `action_bar`).
The gate still requires all fifteen to appear whole in some slice — only the
non-background-pixel assertion is skipped for them, so a widget silently
vanishing from the page still fails. Closing each of these fifteen
controllers' bugs is left to a follow-up task; this list must be empty, or
carry a fresh equally-justified amendment, before the whole-milestone gate
this task runs.

### P8-D70 — the rest-state gate derives its scroll step instead of the plan's literal `SLICE_STEP = 700`

**Carried out by:** P8 (`ui/tests/gallery_gate.rs`). **Added:** 2026-09-02, in
the Part 8 whole-part fix wave.

**The part plan specified** a literal `SLICE_STEP = 700`, sized for an
800px-tall output.

**As shipped:** `slice_step(capturable, tallest) = capturable - tallest - 4`,
computed from the harness's real output height and the tallest gallery entry.

**Ruling.** The harness output is 720px tall (`WLR_HEADLESS_OUTPUTS` is a
count, not a geometry knob), leaving only 20px of overlap under the literal
step — not enough to guarantee `scrolled_window` (229px tall) and `list_box`
(72px) are ever captured whole, so they intermittently failed the gate's
`missing` assertion on a page that in fact rendered correctly. The derived
formula makes the "every entry captured whole at least once" guarantee hold
for any output height, closing a flake rather than a contract signature.

### P8-D71 — `interaction_gate.rs` ships with six of the contract's sixteen tests, not sixteen; Task 10's four remain uncommitted

**Carried out by:** Task 10 (found, left uncommitted); Task 14 (attempted to
land, found a second regression, reverted). **Added:** 2026-09-02, in the
Part 8 whole-part fix wave. **Amended:** 2026-09-02, same fix wave — the
heading previously said "ten", a miscount inherited from
`.superpowers/sdd/m3-part8/task-12-report.md` ("stays at 10 committed
tests"), which counted Task 10's four *uncommitted* tests as committed. Six
is the number the body, the History paragraph and the file itself all carry.
Task 12's own four missing tests are recorded separately as P8-D72.

**§9's P8 gate says** "all sixteen `interaction_gate.rs` tests"; Task 14's own
Step 2 says "if anything fails, stop … this task does not start until the
tree is green."

**As shipped:** `interaction_gate.rs` carries six tests, not sixteen. Ten
are absent. This amendment covers six of them: Task 10's four
(`typing_into_an_entry_shows_the_glyphs_and_moves_the_caret`,
`typing_into_a_search_entry_fires_one_search_after_the_delay`,
`peeking_a_password_entry_reveals_the_text`,
`stepping_a_spin_button_repeats_while_the_button_is_held`) and Task 11's two
(the drop-down popover and list-view interactions). The remaining four —
Task 12's expander, stack-page, menu-button-popover and Tab-order tests —
are recorded, with their diagnoses, in **P8-D72** below.

**History.** Task 10 wrote its four tests plus a chrome-eviction fix
(`Controller::child_index`/`reserved_total` overrides) across
`EntryC`/`SearchEntryC`/`PasswordEntryC`/`SpinButtonC`, found three of the
four fail on real pre-existing behavioural defects (`EntryC`'s caret-click
`local_rect` path; `SpinButtonC`'s repeat timer overshoot), and — per §9's
"Must not touch: any widget implementation" and the CONTROLLER NOTE requiring
all gates green — left everything uncommitted in the working tree rather
than either weaken an assertion or check in a red test file
(`.superpowers/sdd/m3-part8/task-10-report.md`). That state survived,
unrecorded, through Tasks 11–13's own commits. Task 14 found it, verified
Task 10's own diagnosis (the same three tests fail, identically), and
initially committed it (`1f64d71`) intending to record the three as a
known-red list the way P8-D69 records `KNOWN_BLANK_AT_REST`. Running the
rest of the whole-milestone gate immediately after found a **second, larger
problem** Task 10's report did not check for: `cargo test -p icedtea-ui
--test widget_pixels` regressed from 39/39 to 37/39 — the same
`child_index`/`reserved_total` fix that lets a probe point exist at all also
changes each of the four widgets' measured intrinsic size (subnodes that were
previously evicted before layout now count toward it), which shifts the
hard-coded click coordinates two pre-existing, already-committed
`widget_pixels.rs` tests depend on
(`typing_into_an_entry_shows_the_glyphs_and_moves_the_caret` and
`peeking_a_password_entry_reveals_the_text` — same names, a different file,
pixel-exact rather than harness-driven), so both clicks silently miss and
both tests fail. Task 10's report claim ("does not regress any existing
test") checked only `cargo test -p icedtea-ui --lib` (1053/1053), not this
integration file.

**Ruling.** Reverted (`183e8ad`) rather than shipped with a second,
undiagnosed regression: fixing the chrome-eviction bug is squarely
`EntryC`/`SearchEntryC`/`PasswordEntryC`/`SpinButtonC` widget-implementation
work, which both Task 10 and Task 14 are contractually barred from doing
(§9's P8 "must not touch"), and the coordinate fix `widget_pixels.rs` would
now need is not obviously safe to make without re-deriving four widgets'
new intrinsic sizes by hand under the same restriction. No further P5 fix
wave is scheduled after this, M3's final part. The tree is left exactly as
Task 10 left it — six `interaction_gate.rs` tests, `widget_pixels.rs` at
39/39, both green — and this amendment is the record a fix-wave task needs:
the chrome-eviction bug is real and confirmed (Task 10's diagnosis), landing
its fix requires also re-deriving `widget_pixels.rs`'s four affected pixel
coordinates in the same change, and neither Task 10 nor Task 14 is the
task allowed to do either. Every other named count in Task 14 Step 2 (the M1
gate's 4, the M2 gate's 9/10 + 4, the transition test's 1, the M3 gates'
8 + 6-of-16, `compositor/tests/popups.rs` green three times running,
`widget_pixels.rs` at 39/39) is verified green.

**Amended/Closed (entry half): 2026-09-02, M3 close-out.** Task 10's four
tests are landed and green; `interaction_gate.rs` now carries **ten** tests,
not six, and `widget_pixels.rs` **40**, not 39. Task 11's two drop-down and
list-view tests remain the only ones this amendment still leaves open (they
are the other half of this entry, closed separately).

**The chrome-eviction fix was *not* landed, and must not be.** Recovering
`1f64d71` and re-deriving `widget_pixels.rs`'s coordinates — the path this
amendment's Ruling anticipated — was tried and rejected on evidence. The
`child_index`/`reserved_total` overrides do not merely *shift* those two
widgets' intrinsic sizes; they **disable the controllers' own `measure`
entirely**. An entry-family widget takes no `View` children, so
`view::render::container_for` classifies it `Container::Leaf`, which
`LayoutTree::set_style` writes into taffy as `Display::Flex`; taffy calls a
node's measure closure only for a node with no taffy children
(`compute_layout_with_measure` dispatches on `has_children`), and
`LayoutTree::sync_node` gives every CSS child a taffy node. Attaching the
chrome therefore silences `EntryC::measure`, `PasswordEntryC::measure` and
`SpinButtonC::measure`, and each control collapses to whatever its chrome
happens to size to: measured on this branch, a `PasswordEntry` showing
"hunter2" went from a 78x34 box to a 43x34 one, and the peek band it reserves
moved out from under `widget_pixels.rs`'s click. The 39/39 → 37/39 regression
was that sizing collapse showing through, not coordinate drift — re-deriving
the two coordinates would have shipped four mis-sized widgets and called it
a coordinate fix. Sizing these four from their own subnodes (each subnode
needs its own `Measure`) is real work, out of scope for a close-out, and
tracked by this amendment rather than half-done by it.

**What landed instead.** The four tests take their click points from the one
box that is always correct — the widget's own border box, from
`gallery --print-allocation`, via the new `support::allocation_sized` and
`Driver::allocation` — plus the hit geometry the controller itself publishes
(`password_entry::PEEK_WIDTH_PX` and `spin_button::STEPPER_SIZE`, both made
`pub` for this). Nothing is hard-coded either way, and no production sizing
changes.

**Two of Task 10's three diagnoses were confirmed; the third was wrong.**

1. **`EntryC`'s caret-click `local_rect` path — confirmed, fixed.** The block
   hit-tested `local_rect(cx.tree, cx.node, &self.edit.text_node)`, which is
   `None` for `text` in every frame, so the whole block was dead and an
   `Entry`'s caret could not be placed by clicking *at all*: it stayed at
   `TextEditState::build`'s initial `text.len()`. Now `content_rect_local`,
   the fix `SearchEntryC` and `PasswordEntryC` already carried. Driven RED
   first: with the old spelling, clicking before the first glyph of "Entry"
   and typing "hi" gives `changed entry Entryhi` (the caret never moved);
   with the fix, `changed entry hiEntry`. Note that Task 10's test clicked
   *past* the text, where the initial caret already is — an assertion no
   caret placement, working or broken, can fail.
2. **`SpinButtonC`'s repeat overshoot — confirmed, fixed in
   `RepeatTimer::fire`.** `fire` counted every interval that had elapsed
   since the deadline it missed and `tick` stepped once per count, so one
   late tick — routine, since a gallery repaint takes seconds — took a held
   stepper straight from its first step to the adjustment's clamped bound
   (the observed `3 -> 4 -> 10`), and re-applied `climb` once per missed
   interval on the way. `fire` now returns `bool`, fires at most once per
   call and re-anchors the next deadline on `now`, which is what a repeat
   means (GTK's stepper is a `g_timeout` callback: once per invocation, never
   replayed). Covered by the new
   `a_late_repeat_fires_once_and_re_anchors_on_the_clock_it_was_given`.
3. **The password-peek failure was *not* a production defect.** Task 10 read
   it as the same degenerate-geometry problem. It was the probe point: the
   `text` centre it sampled is background in both the masked and the revealed
   rendering, so a single pixel there cannot see the toggle at all. The test
   now samples the widget's whole interior band and clicks the peek icon
   three times, asserting reveal, re-mask, and that the third click renders
   exactly what the first did — which no focus-ring or caret change can
   satisfy, and which the "peek without re-shaping" mutation fails.

**One further defect was found and fixed on the way**, latent rather than
observed: `EntryC` placed the caret with `layout.byte_at`, a *display* offset,
where both siblings use `buffer_offset_at`. With `GtkEntry:visibility` off the
layout is over 3-byte bullets while the buffer is not, so the caret lands at
the wrong character, and on a longer or non-ASCII buffer past its end or
inside a codepoint, which `String::replace_range` panics on. Covered by
`widget_pixels.rs`'s new
`clicking_into_a_masked_entry_places_the_caret_in_the_buffer_not_the_mask`.

**Commit:** see `.superpowers/sdd/m3-close/closure-entries.md` for the full
RED/GREEN trail.

**Amended/Closed (drop-down half): 2026-09-02, M3 close-out.** Task 11's
drop-down interaction is landed and green:
`interaction_gate.rs::opening_a_drop_down_and_picking_an_item_updates_the_button`
opens the popover with a real click, picks row 1, and requires the list to be
absent before the first click and absent again after the pick. The file now
carries **eleven** tests, not ten.

**The defect was bigger than Task 11 reported.** `PopoverC::open`'s
`content: None` branch did not merely fail to open a surface: it did nothing
at all, and nothing else ever flipped the popover node's visibility, so a
`DropDown`'s list was **always laid out and painted**. Measured before the
fix, a *closed* `dropdown`'s border box was 102x40 — its 36x34 button plus
three live `row`s beside it — and a click at those rows' coordinates selected
an item. `open` was a bool no pixel could see.

**Fixed by revealing the retained body, not by opening a surface.** `PopoverC`
keeps its own `root` node and gains `hide_when_closed()`/`reveal(bool)`;
`open` reveals, `close` and `PopupDone` hide, and `DropDownC` opts in and
guards its row hit-test with `self.open`. The payload stays `None`: P7-D54
rules that a `DropDown`'s list is retained in the parent window's tree and a
second surface would double-draw it, and `App::run` routes input only over
`rt.instances`, never `rt.popups` — a real `xdg_popup` here would be a
picture of a list that could not be clicked. Only `DropDown` opts in;
`PopoverMenuBar` sizes its `item`s from their menu child and its own
hover-to-switch test reads those boxes, so hiding a menu bar's menus is its
own change.

**`GtkWidget:visible` is now modelled, because detaching broke a vendored
fixture.** Hiding the popover by detaching its node failed `node_trees.rs`
("fixture requires a node at `dropdown/popover`"): GTK's node trees list
hidden nodes, and §5.2's vendored fixtures with them. `LayoutTree::
set_displayed`/`is_displayed` now write taffy `Display::None` — applied
*after* `container_style`, which otherwise overwrites `display` — and
`view::render`'s paint walk skips a hidden node and its subtree, so a hidden
node keeps its CSS-tree place while taking no space, receiving no events and
painting nothing. `widgets::set_displayed` is the controller-side entry, on
the same side table `set_gap`/`set_container` use.

**`gallery --open` is a geometry affordance, not a new interaction.** A closed
drop-down's rows have no geometry, so no closed-state probe can say where row
1 lands once a click opens it. `--open` builds every popover-bearing sample
with its popover shown (`DropDownC` honours `PropName::Expanded`, the name
every other disclosure widget already carries), and
`support::probe_points_open`/`Driver::point_open` read it. The test still
drives the real open by clicking.

**One coordinate was re-derived.** `widget_pixels.rs::opening_a_drop_down_and_picking_an_item_updates_the_button`
clicked (67, 100) — a point derived from the old 102x40 box. A closed
drop-down is now 36x34 and centred in that test's 200x200 window, so the click
moved to (100, 100); the captured row shows the button's own border at x 82
and x 117. This is a sizing *correction*, not the sizing *collapse* the entry
half of this amendment refuses: no controller's `measure` is silenced, and
`widget_pixels.rs` is 40/40.

**Amended (list-view half): 2026-09-02, M3 close-out. Three production
defects fixed; the interaction test is still not landed.** Task 11's diagnosis
("no `measure()` override") named the wrong cause — a `ListView`'s container
is `Container::Box`, so taffy never calls a measure closure for it at all.
What was actually wrong:

1. **`WidthRequest`/`HeightRequest` were read by three widgets in the whole
   crate** (`label`, `entry`, `drawing_area`). GTK's size request is
   universal, and fourteen gallery entries laid out at the wrong size because
   of it, twelve of them collapsed or near it (`list_view` and `grid_view` at
   0x0, `stack_sidebar` at 1x0, …). `Universal::apply` now honours both
   through `widgets::set_size_request`, folded into taffy by `flush_layout`
   as `LayoutTree::set_size_floor` — a floor, as GTK's pair are, so CSS
   `min-width`/`min-height` still win where they are larger.
2. **Reconcile evicted the pool.** A `ListView` takes no view children, so
   every node child of a `listview` is past `reserved_total`'s default bound
   and the trim step detached every pooled row on the first reconcile — no
   row ever had an allocation. `ListViewC::child_index`/`reserved_total` now
   reserve the pool and the rubberband.
3. **`set_metrics` had no production caller**, which Task 11 got right.
   `ListViewC::tick` now calls `adopt_metrics`: the viewport from the node's
   own allocation, the row height from the *sheet* rather than from the
   allocation (reading it back off a stretched pool makes the pool size and
   the row height chase each other frame after frame).

**Still open: `scrolling_a_list_view_recycles_rows_without_losing_selection`.**
A pooled row renders and measures nothing — its bound text lives in the
`ROW_BINDING` side table (`widgets::set_text`/`text_of`) and nothing paints or
measures it — so a row's whole height is Adwaita's `listview > row
{ padding: 2px }`, 4px. The gallery's ten rows occupy 40px of a 120px
viewport: `max_offset` is 0, nothing scrolls, nothing is recycled, and a test
named for recycling would assert nothing; driven anyway, a click at every
offset into the sample's content box produces `messages: []`. Giving
`ListView` rows a `label` subnode with a real text `Measure` and a paint pass
— which is what makes ten rows overflow the viewport and a selection visible
enough to assert on — is P6 widget work with its own design, shared with
`GridViewC`/`ColumnViewC`, not a close-out edit. Recorded here rather than
half-done.

**Commit:** `642f612`; see
`.superpowers/sdd/m3-close/closure-popup-lists.md` for the full RED/GREEN
trail.

### P8-D72 — Task 12's four interactions are absent too, each blocked by a distinct pre-existing production defect

**Carried out by:** Task 12 (written, driven RED, root-caused, left
uncommitted). **Added:** 2026-09-02, in the Part 8 whole-part fix wave —
P8-D71 named only six of the ten missing tests, so a reader of the contract
alone would have concluded these four defects were unknown.

**§9's P8 gate says** "all sixteen `interaction_gate.rs` tests"; §9's P8
"must not touch: any widget implementation" and the standing "all gates stay
green" rule together forbid both fixing these defects and committing a red
test.

**As shipped:** none of Task 12's four tests is in the file:

1. `expanding_an_expander_animates_and_reveals_the_child` — **P6,
   `ui/src/widgets/expander.rs`.** The click and message path work
   (`expanded true` arrives), but the `content` pixel never changes.
   `ExpanderC::progress` is written by the tick loop and read only by its own
   test hook `content_scale`; nothing in `ui/src` outside `expander.rs`
   reads it — no CSS write, no layout write, no paint clip. `content` never
   receives `PseudoStates::CHECKED` (only the root and `arrow` do), and
   Adwaita has no rule styling a `content` node by an ancestor `:checked`.
   Child disclosure was never wired to layout or paint; `progress` drives
   the arrow only. (The task's own `child = title + 30px` probe offset also
   lands 2px below the widget's border box; `driver.point("expander",
   "content")` is the correct sample point, and still fails.) A fix wants a
   `content` height or clip driven by `progress`, on the model of
   `RevealerC` in `ui/src/widgets/mod.rs`, which already carries the
   identical `progress`/`started`/`tick` shape.
2. `switching_a_stack_page_runs_the_transition` — **P6,
   `ui/src/widgets/stack_switcher.rs`.** `--print-allocation --widget
   stack_switcher` returns a 0x0 box and `--probe-points` emits only `root`:
   no `button0`/`button1` exists. `rebuild` (lines ~97-115) builds each page
   button as a bare `Node::new("button")` and never appends a `label` child,
   unlike every other button-shaped node in the crate, so the row collapses
   to zero intrinsic size. `on_event`'s per-button hit-test uses that same
   allocation, so **a live `StackSwitcher` cannot be clicked at all** —
   page switching is unreachable in the app, not merely untested.
3. `opening_a_menu_button_popover_takes_the_grab_and_dismisses_outside` —
   **P5, `MenuButtonC`/`PopoverC`.** Two independent defects. (a)
   `MenuButtonC::build` (`ui/src/widgets/menu_button.rs` ~114) uses
   `PopoverC::for_test`, whose root `Node` is a local dropped at the end of
   the call — the `arrow`/`contents` subtree is never appended to
   `MenuButtonC`'s own node, so no popover probe point exists in any state.
   `DropDownC::build` (`ui/src/widgets/drop_down.rs` ~182-186) shows the
   correct shape: append a real `popover_node`, then call `<PopoverC as
   Controller<Msg>>::build` on it. (b) Even so, `PopoverC::open`
   (`ui/src/widgets/popover.rs` ~167-200) returns early whenever `content`
   is `None`, *before* consulting `self.autohide`, so it never pushes
   `Cmd::OpenPopup` — the same gap Task 11 already routed for `DropDownC`,
   now confirmed on a second embedder. Any fix should sweep every embedder
   (`about_dialog.rs`, `alert_dialog.rs`, `popover_menu.rs`,
   `shortcuts_window.rs`) rather than special-casing these two.
4. `tabbing_through_a_form_moves_the_focus_ring_in_geometric_order` —
   **P3, focus-ring paint.** Not an `ActionBar` defect: its geometry is
   correct (`button0` at 609,401, `button1` at 671,401). A throwaway
   differential test on a bare `button` gallery reproduced it exactly —
   `(248, 247, 247) -> (248, 247, 247)`, no ring — so the first-ever `Tab`
   from a fresh window paints no focus ring on the simplest focusable
   widget in the crate. `Universal::new` adds `FOCUSABLE_CLASS` and
   `window::focus::navigate`/`collect`/`is_focusable` look correct by
   inspection; the existing
   `a_pointer_click_focuses_without_showing_the_focus_ring` asserts on
   messages, never on ring *pixels*, so nothing in this crate had ever
   proven the ring renders.

**Ruling.** Left uncommitted, matching Task 10's and Task 11's precedent and
this part's own bar on widget-implementation work: none of the four is
reachable green without production fixes spanning three other parts (P3, P5,
P6), and committing them red would break the standing all-gates-green rule.
Recorded here so the contract, read alone, names all ten missing
interactions and all four of these defects. Finding 4 is the structurally
significant one: with no pixel-level proof of the focus ring anywhere in the
crate, §10's "geometric focus order" interaction is currently unverifiable
end-to-end against *any* widget.

**Amended/Closed: 2026-09-02, M3 close-out.** All four of Task 12's tests are
landed and green; `interaction_gate.rs` now carries **fifteen** tests, not
eleven. Four production defects were driven RED first and fixed. Two of Task
12's four diagnoses were exact, one was backwards and one named the wrong
part.

1. **`ExpanderC` — confirmed, but stated backwards; fixed.** Nothing outside
   the module read `expanded` or `progress`, as recorded — but what that
   cost was the *collapsed* state, not the expanded one: a collapsed gallery
   expander was already painting its child, and the RED run says so
   (`the band from (611, 368) to (668, 368) stayed [(188,188,188),
   (48,54,56), …]`, glyph pixels at rest). `ExpanderC::sync_disclosure` now
   writes `GtkWidget:visible` onto `content` through `widgets::set_displayed`
   from `build`, `set_expanded` and every `tick`, tracking `expanded ||
   progress > 0.0` so the content stays up for the whole of a collapse.
   Hidden and not detached, for P8-D71's drop-down half's reason. **Not a
   partial-height reveal**: nothing in the paint walker reads a per-node
   transform or clip (`widgets::Revealer`'s own doc records the same limit),
   and GTK4's own `GtkExpander` has no size transition either —
   `gtk_expander_set_expanded` moves the child's visibility. `progress` stays
   the arrow's rotation, and the animation half of the test's name is
   `ExpanderC`'s own
   `expanding_animates_progress_to_one_and_then_stops_asking_for_frames`.
2. **`StackSwitcherC` — half right; both halves fixed.** The missing `label`
   child is real, but it is not what made the box 0x0. A `StackSwitcher`
   takes no view children (its pages arrive as `PropName::Pages`), so every
   one of its node children is past reconcile's default trim bound of
   `view_count` and the trim step detached all of them on the first
   reconcile — `ListViewC`'s defect, in the same shape.
   `child_index`/`reserved_total` now reserve the buttons (and `rebuild`
   detaches only its own), and each button gains a `label` child and the
   `.text-button` class GTK gives it, which is what Adwaita's
   `stackswitcher > button.text-button { min-width: 100px }` is written
   against. The switcher goes from `0 0` to `268 34`, `button0` at (573,
   360) and `button1` at (707, 360) — clickable in the live app for the first
   time. The label paints no text, like `MenuButton`'s and `Expander`'s: a
   subnode with no controller has no `Measure` and no `paint`.
3. **`MenuButtonC` — half (a) exact and fixed; half (b) was already closed.**
   The orphaned `PopoverC::for_test` root was the whole of it: a real
   `popover.background.menu` node is now appended to the menu button's own
   node, `<PopoverC as Controller<Msg>>::build` runs on it and
   `hide_when_closed()` follows — `DropDownC::build`'s shape exactly.
   `PopoverC::open`'s `content: None` early return, half (b), had already
   been fixed by this close-out's drop-down commit (`642f612`), which reveals
   the retained body before consulting `content`; nothing further was needed.
   `PopoverC::for_test` now has no production caller. A new
   `MenuButtonC::set_expanded` is the single place the pointer path and the
   property path meet, and `PropName::Expanded` reaches it, so `gallery
   --open` builds the tree a click produces.

   **One vendored §5.2 fixture was amended**, the only contract-signature
   change in this entry: `menu_button.txt` gains
   `╰── [popover.background.menu]`. GTK's own doc block for `GtkMenuButton`
   stops at the button, but it is eliding, not contradicting —
   `gtkmenubutton.c` calls `gtk_widget_set_parent (popover, GTK_WIDGET
   (menu_button))`, so the popover's CSS node *is* a child of `menubutton`,
   which is how GTK's `GtkDropDown` block spells the same relationship and
   how `drop_down.txt` already carries it. Optional, because a `MenuButton`
   with no menu has none. The reasoning is in `menu_button.rs`'s module doc.
4. **The focus ring was routed to the wrong part, and the defect is much
   larger than "the ring does not paint".** `grep` for `navigate` and
   `note_key` across `ui/src` finds `src/bin/window-probe.rs` and nothing
   else. P3 shipped `window::focus::navigate`, `focus_sort`,
   `window_binding` and `FocusRing::note_key`, all unit-tested, and
   **`App::run` never called any of them**: `route`'s `InputEvent::Key` arm
   delivered every key to the focused controller, no controller in the crate
   reads Tab, so Tab did nothing at all in the live app — no focus moved and
   the ring had nothing to paint. This was not a dirty-marking bug and not a
   P3 paint bug; it was missing wiring in P7's `App`. `route` now feeds
   `rt.focus.note_key(key)` for every key both ways before acting on it
   (GTK's `_gtk_window_update_focus_visible` rule) and takes
   `window_binding`'s two Tab directions at the window, navigating with the
   wrap `window-probe` uses. **Only the Tab directions**: the arrows, Space,
   Return and Escape `window_binding` also names belong to widgets that
   already read them for themselves (a `ListView`'s row cursor, a
   `SpinButton`'s step), and stealing them at the window would break those.

**Reconciliations in the landed tests.** The expander samples `gallery
--open`'s coordinates rather than the task's `title + 30px`, which lands
below a collapsed expander's own border box. The menu-button test's
"dismisses outside" is not reachable: P7-D54 retains an embedded popover's
body in the parent window's tree and `App::run` routes input only over
`rt.instances`, so there is no grab to take and a click outside the widget
never reaches `MenuButtonC`; the toggle is the dismissal this crate has, and
the test asserts that round trip over the strip of the open box that lies
outside the closed one (a band containing the button could not come back
exactly, since the button keeps the `:hover` the click left on it). The
Tab test bands whole buttons rather than each centre, because Adwaita's ring
is `outline-offset: -2px`.

**Test support.** `support::allocation_open`/`Driver::allocation_open` is
`--print-allocation` with `--open`, the counterpart to `probe_points_open`;
`gallery --open` now also seeds `model.expanded`, so a disclosure widget and
not only a popover is built disclosed.

**One adjacent defect was found and deliberately not fixed.**
`StackSidebar` has `StackSwitcher`'s eviction defect too and still has it:
`gallery --print-allocation --widget stack_sidebar` gives `580 320 121 80`
— its `width_request`/`height_request` alone — and `--probe-points` emits
only `root`, so not one of its rows is in the layout tree. It builds its rows
from `PropName::Pages` the same way, takes no view children, and carries no
`child_index`/`reserved_total`. No §10 interaction covers it and it is not
one of this entry's four, so it is recorded rather than folded into a
defect-closure commit; the fix is `StackSwitcherC`'s, verbatim.

**Still open, and the only interaction of §10's sixteen that is:**
`scrolling_a_list_view_recycles_rows_without_losing_selection`, for the
reason P8-D71's list-view half gives (a pooled row measures and paints
nothing, so ten rows cannot overflow a viewport and nothing recycles). All
four of this entry's own interactions are closed; what it still carries is
the `StackSidebar` note above.

**Amended 2026-09-02** (M3 close-out, fix wave 1). The pooled-row half is
fixed — rows measure and paint their bound text (`widgets::measure_row`/
`paint_row`), a row's height comes from the sheet *and* the font
(`ListViewC::css_row_height`), and the viewport is what the view was asked to
be rather than what its own rows made it (`ListViewC::adopt_metrics`), so ten
rows do overflow a 120px viewport and a scroll does recycle. The test is
written and ships, `#[ignore]`d: no `wl_pointer.axis` reaches any client
under this compositor, which is **P8-D74**. The behaviour itself is proven
offscreen; see that entry.

**Commit:** see `.superpowers/sdd/m3-close/closure-chrome-focus.md` for the
full RED/GREEN trail.

### P8-D73 — `CheckButtonC` and `SwitchC` did receive the chrome-eviction fix P8-D71 reverts elsewhere

**Carried out by:** Task 5 (`e41bdbf`, `SwitchC`) and Task 9 (`ff43ffd`,
`CheckButtonC`). **Added:** 2026-09-02, in the Part 8 whole-part fix wave,
after a review found the same defect class adjudicated two ways inside one
part.

**§9's P8 says** "must not touch: any widget implementation" — the ground on
which P8-D71 reverts the identical `Controller::child_index`/`reserved_total`
override for `EntryC`/`SearchEntryC`/`PasswordEntryC`/`SpinButtonC`.

**As shipped:** `ui/src/widgets/switch.rs:114` and
`ui/src/widgets/check_button.rs:166` each carry that override.
`SwitchC` reserves its three chrome children (`on_image`/`off_image`/
`slider`), `CheckButtonC` its `check` plus optional `label`. Without them,
`reconcile`'s trim step (`reconcile_reserved`'s `trim_from`) detaches all
chrome on the first reconcile, which a headless probe walking the real node
tree caught — the subnodes simply were not there. Two shipped interaction
tests depend on them: `checking_a_check_button_paints_the_builtin_check`
(probe point `check_button`/`check`) and
`flipping_a_switch_animates_the_slider_to_the_other_end` (`switch`/`slider`).

**Ruling.** Kept, not reverted, and recorded here rather than left silent.
The distinguishing fact is empirical, not principled: **`widget_pixels.rs`
is 39/39 with both edits in the tree** (re-verified in this fix wave), so
unlike the four entry-family widgets, these two shift no already-committed
hard-coded coordinate and land no undiagnosed second regression. Reverting
them would delete two green, shipped gate tests and restore a confirmed
chrome-eviction bug in two widgets, to gain only formal symmetry with
P8-D71 — whose reversion was driven by the 39/39 → 37/39 regression, not by
§9 alone. §9's "must not touch" is hereby read, for the chrome-eviction
class specifically, as barring changes that regress an existing gate; a
two-line reservation count that regresses nothing and is proven by a gate
test is permitted, provided it is recorded here. No other widget
implementation was modified by P8.

### P8-D74 — the "scroll a list" interaction cannot be driven: `wlr` 0.20.28 forwards no `wl_pointer.axis`

**Carried out by:** the M3 close-out, fix wave 1. **Added:** 2026-09-02.

**§7's gate says** `interaction_gate.rs` ships sixteen tests, one of them
`scrolling_a_list_view_recycles_rows_without_losing_selection`. P8-D72
recorded it as the last one still open, blocked on pooled rows measuring and
painting nothing.

**As shipped:** the rows defect is fixed (`widgets::measure_row`/`paint_row`,
`ListViewC::css_row_height`, `ListViewC::adopt_metrics`' request-derived
viewport) and the interaction still cannot be driven through the harness
compositor, for a reason below this crate entirely.

**The blocker, traced end to end.** With a print at the top of
`view::app::dispatch_input`, a `--widget list_view` gallery run under the
harness receives `Configure`, `KeyboardEnter`, `PointerEnter` and a stream of
`Frame`s — and never one `Scroll`, however many
`zwlr_virtual_pointer_v1.axis` + `frame` pairs `Driver::scroll` injects. The
gap is in the compositor stack: `wlr` 0.20.28 (`compositor/Cargo.toml`) never
subscribes to a pointer's `events.axis` and never calls
`wlr_seat_pointer_notify_axis` — `grep -rn axis` over its whole `src/`
returns only comments about layout axes. No Wayland client under this
compositor has ever received a `wl_pointer.axis`; `Driver::scroll` has no
caller anywhere in the tree older than this test, so nothing had noticed.
Closing it means a `wlr` release, which this branch cannot make.

**Ruling.** The test ships, named exactly as §7 names it, written exactly as
it would run, and `#[ignore]`d with that reason in the attribute and the full
trace in its doc comment. `interaction_gate.rs` therefore carries all sixteen
of §7's names; fifteen run, one is ignored, and the ignore is a transport
gap, not a widget defect.

**The interaction is covered, not dropped.** `widgets::list_view::tests::
pixels::scrolling_recycles_the_pooled_rows_and_keeps_the_selection` drives the
same ten rows in the same 120px viewport through a real `App`, a real layout
pass and a real paint on the offscreen surface, where the scroll *can* be
delivered: it clicks the second row, scrolls to the end, scrolls home, and
asserts on the frames — the selection paints, the pool rebinds to other rows,
and the recycled rows come home carrying the same selection. Both of its
mutation checks were verified to fail.

**Un-ignore it** the day `wlr` forwards an axis event; nothing else in the
test needs to change.

**Closed (2026-09-03).** `wlr` 0.20.29 links every pointer's `events.axis`
(physical and virtual alike), emits it to the new defaulted
`SeatHandler::pointer_axis`, and forwards it with
`wlr_seat_pointer_notify_axis` + `wlr_seat_pointer_notify_frame` —
additive, so `compositor/src/state.rs`'s `impl SeatHandler` needed no
change. The pins moved 0.20.28 → 0.20.29, the `#[ignore]` came off, and
`interaction_gate.rs` runs all sixteen of §7's tests. Exactly as predicted,
nothing else in the test changed.


### P8-D75 — §4.3's universal props are applied centrally, not per controller

**Carried out by:** the M3 close-out, fix wave 1. **Added:** 2026-09-02.

**§4.3 says** fifteen `GtkWidget` props are universal. §5's widget catalogue
assumes every kind honours them.

**As shipped (before this wave):** `widgets::Universal` was opt-in per
controller, and 30 of the widget modules mentioned it nowhere — their
`set_prop` fell through to `_ => return` for every universal name. The
visible half was the size request: `gallery`'s `progress_bar` sample asked
for `width_request(160)` and `--print-allocation` reported
`progress_bar 640 140 0 19`, a zero-width widget; the `scrollbar` sample did
the same. `Classes`, `Id`, `Sensitive` and `Focusable` were dropped just as
silently on those same kinds. `642f612` had called the size request
"universal" while fixing only the controllers that already owned a
`Universal`.

**Ruling.** `widgets::apply_universal(node, kind, name, value)` is called
from both prop seams — `widgets::build_controller`'s initial loop and
`view::reconcile`'s diff loop — *before* the controller's own `set_prop`, for
every kind alike, keyed on a node-keyed side table like every other piece of
per-node widget bookkeeping. It is idempotent against a controller that also
owns a `Universal`.

It applies six of the fifteen, not all of them:
`Classes`/`Focusable`/`Id`/`Sensitive`/`WidthRequest`/`HeightRequest`. Those
six have exactly one meaning on every `GtkWidget`.
`Checked`/`Indeterminate`/`Selected` are deliberately excluded: a widget is
free to give those names its own meaning — `ListView`'s `.selected(index)` is
a model index, not a `:selected` flag on the view's own node — so writing a
pseudo-state for them centrally would be wrong. The remaining §4.3 names
have no universal writer and stay where they are.


---

## 11. Execution notes — cross-part consistency check (E1–E16)

Added 2026-08-27 by the consistency checker after all eight part plans were
written, from a full cross-read of P0–P8 for: matched Produces/Consumes,
one-part-per-contract-item, migration/gate agreement, disjoint file ownership,
forward-reference-free execution order, and complete P5/P6 widget coverage.

**Numbering.** These `E<n>` entries are the *cross-part* rulings and are
binding on every part. They are distinct from the per-part deviation records
each plan appends to §10 when it lands (those are numbered from `D1` inside
their own plan and should be recorded in §10 as `§10 P<part>-D<n>`, not as a
bare `E<n>`, so the two sets never collide).

**What was verified clean.** The P5/P6 split covers all 64 `Kind` variants
exactly once with no orphan and no duplicate (P5 = §5.1–§5.3's 32, P6 =
§5.4–§5.7's 32); all twelve `compositor/tests/popups.rs` names in §2.4, all six
`gallery_gate.rs` names and all sixteen `interaction_gate.rs` names in §7 appear
verbatim in P2 and P8; P1 consumes nothing and P2's consumed §1 block matches
§1 signature-for-signature; execution order 1→8 needs no later symbol earlier
(every forward reference is discharged by a declared deviation: P4 D8/D9 for
`Align`/`IconTheme`, P5 D2/D10 for `Builtin`/`ListViewC`, P8 D10 for the
assumed widget enums).

---

### E1 — `build_controller` has one definition, in `widgets/mod.rs`, with a `GenericC` catch-all

P4 (Task 9) produces `pub fn build_controller<Msg>(kind, node, props, cx) ->
Box<dyn Controller<Msg>>` in `ui/src/view/controller.rs`, falling back to
`GenericC`. P5 (D6, Task 7) produces a function of the same name and signature
in `ui/src/widgets/mod.rs`, falling back to `Box::new(Unimplemented(kind))`.
Two definitions; and P5's fallback would regress every kind P5/P6 has not yet
written from a working `GenericC` to an inert stub, breaking P4's counter-app
and P8's gallery.

**Ruling.** One function, one fallback.
1. P4 ships `view::controller::build_controller` as written — it is what
   `reconcile`'s `Insert` arm calls, and it is a real implementation over
   `GenericC`.
2. When P5 lands, P4's body becomes exactly
   `crate::widgets::build_controller(kind, node, props, cx)`; the name and
   signature at the `reconcile` call site do not move.
3. `widgets::build_controller`'s catch-all is
   `Box::new(<GenericC as Controller<Msg>>::build(node, props, cx))` — **never**
   an `Unimplemented` stub. P5 deletes `Unimplemented` from its plan.
4. P6 adds its 32 kinds' arms to `widgets::build_controller` (P6's file table
   and Task 3 already name it after the in-place rename below).

Carried out by: P4 (step 2's forward note), P5 (Task 7), P6 (Task 3).

### E2 — `NodeAddr`, `node_addr` and `StyleMap` are P3's; P4 re-exports them

P3 D4 ships `pub use selectors::OpaqueElement as NodeAddr`, `pub fn
node_addr(&Node) -> NodeAddr` and `pub type StyleMap = HashMap<NodeAddr,
Rc<ComputedStyle>>` in `ui/src/window/mod.rs`. P4 D6 / Task 7 produces all
three again in `ui/src/view/render.rs`. They are the same aliases over the same
type, so this is not a type error — it is two public spellings of one identity,
and `hit_test`/`hit_chain` (§3.4) take P3's.

**Ruling.** P3's are canonical. P4's Task 7 replaces its three `Produces`
lines with
`pub use crate::window::{NodeAddr, StyleMap, node_addr};`
and defines only `Animations` and `restyle_tree`. P3's `window::restyle` and
P4's `view::render::restyle_tree` are different functions and both stay.

Carried out by: P4 (Task 7).

### E3 — every `InputEvent::{PointerEnter, KeyboardEnter}` literal must name `target`

P3 D6 adds `target: SurfaceTarget` to both variants. Nineteen struct literals
in P4 (3) and P5 (16) construct them with the contract's original field list
and would not compile.

**Ruling.** P3 additionally ships two constructors on `InputEvent`:

```rust
impl InputEvent {
    /// `PointerEnter` on the main window surface — the only target an
    /// offscreen or single-surface script can mean.
    #[must_use] pub fn pointer_enter(x: f64, y: f64, serial: u32) -> InputEvent;
    #[must_use] pub fn keyboard_enter(serial: u32) -> InputEvent;
}
```

both filling `target: SurfaceTarget::Window`. Every P4/P5/P6/P8 script literal
`InputEvent::PointerEnter { x, y, serial }` becomes
`InputEvent::pointer_enter(x, y, serial)`, and likewise for `KeyboardEnter`.
Code that genuinely targets a popup keeps the struct literal.

Carried out by: P3 (adds the constructors, in the task that defines
`InputEvent`), P4 and P5 (use them).

### E4 — `App::sheet` is withdrawn; P4's `App::with_sheet` is the setter — FIXED IN PLACE

P5 D8 added `App::sheet(self, CompiledSheet) -> Self` as "the one edit to
`ui/src/view/app.rs` P5 makes"; P4 D11 already ships
`App::{with_sheet, with_fonts, with_icons}` for the identical reason. P5's
plan has been edited: D8 is marked WITHDRAWN, its file-structure row now reads
"Untouched", its one call site is `.with_sheet(…)`, and its §10 record E8 is
rewritten. P5 adds **no stylesheet setter** to `view/app.rs`, which is what
§9's P5 boundary wanted.

**AMENDED 2026-08-31 (§10 P5-D32, P5-D33).** The sentence this entry
originally ended on — "P5 now makes **no** edit under `view/` except D5's
`pub use` lines in `builders.rs`" — is withdrawn as a false statement about
what P5 shipped. E4's actual, still-binding ruling is the narrow one above:
`App::sheet` is withdrawn and `App::with_sheet` is the setter. P5 does edit
`view/controller.rs`, `view/reconcile.rs` and `view/app.rs`; every such edit
is recorded in §10 under **P5-D32** (the `Any` supertrait, the
`build_controller`/`generic_controller` split, `reconcile`'s `child_slot`
routing, four rewritten tests) and **P5-D33** (`route`'s `aim`). P6 and P7
must read those two entries, not this sentence, for the state of `view/**`,
and must record any further `view/**` change in §10 the same way.

### E5 — one `ListItem`, in `view/mod.rs`, owned by P4

P4 D7 / Task 1 defines `pub struct ListItem { id: u64, text: Rc<str>,
subtitle: Option<Rc<str>>, icon: Option<IconRef> }` in `ui/src/view/mod.rs`,
because §4.3's `Prop::Items(Rc<[ListItem]>)` needs it. P5 D4 lists `ListItem`
among the eighteen types it declares in `ui/src/widgets/mod.rs`, as
`{ id: u64, label: Rc<str>, icon: Option<IconRef>, sensitive: bool }` with
`ListItem::new(id, label)`. Two incompatible definitions of one contract type.

**Ruling.** `view::ListItem` is the single definition, and it carries the
union of the two field sets:

```rust
pub struct ListItem {
    pub id: u64,
    pub text: Rc<str>,
    pub subtitle: Option<Rc<str>>,
    pub icon: Option<IconRef>,
    pub sensitive: bool,
}
impl ListItem { pub fn new(id: u64, text: &str) -> Self; }  // subtitle None, icon None, sensitive true
```

The field is `text`, not `label` (`Prop::Items` is a model, and §4.3 already
spends `PropName::Label` on the widget property). P5 drops `ListItem` from
D4's list — seventeen types, not eighteen — and reads `item.text`. P6's
`Sorter`/`ItemFactory` take `&view::ListItem`.

Carried out by: P4 (Task 1), P5 (Task 7), P6 (Task 2).

### E6 — the node-tree renderer and matcher keep P5's names and signatures

P5 (D7, Task 7) ships `node_tree_of(kind, props) -> String`,
`render_node_tree(root: &Node) -> String` and
`fixture_matches(fixture: &str, rendered: &str) -> Result<(), String>` in
`ui/src/widgets/mod.rs`, and its 32 conformance tests call them. P6 (D7, D8,
Task 3) creates `ui/src/widgets/node_tree.rs` with `node_tree_of`,
`render_tree(root)` and `matches_fixture(root: &Node, fixture: &str) ->
Result<(), Mismatch>` — a second implementation under three different names.

**Ruling.** P5's three names and signatures are normative and do not move.
P6's Task 3 **relocates** P5's bodies into `ui/src/widgets/node_tree.rs`
unchanged, re-exports all three from `widgets`, and adds
`matches_fixture(root: &Node, fixture: &str) -> Result<(), Mismatch>` as a
thin node-first wrapper that calls `fixture_matches(fixture,
&render_node_tree(root))` and boxes the `String` reason into `Mismatch`. There
is exactly one matcher implementation. P6's D7/D8 have been edited in place to
say so.

Carried out by: P6 (Task 3).

**AMENDED 2026-09-01 (§10 P6-D37).** P6 shipped a second matcher instead;
the P6 whole-part fix wave carried E6 out as written and deleted it. The two
pieces of fixture slack P6's blocks need were added to the surviving matcher
rather than kept in a second one — see P6-D37, which also names this matcher
normative for P8's gallery gate.

### E7 — `PaintCx` is re-exported from `view::controller` — FIXED IN PLACE

`Controller::paint`'s `cx` is M2's `crate::paint::PaintCx` (`paint/mod.rs:44`),
which P4 imports by that path; P5's Task 7 consumes list and eight of its
widget modules import `crate::view::controller::PaintCx`. P4's Task 8
`Produces` block now carries `pub use crate::paint::PaintCx;` in
`view/controller.rs`, which makes both spellings resolve to the one type. No
signature moves.

### E8 — the builder-setter convention is P5's extension traits, not a single inherent namespace

P6's deviation 13 rules that "every setter takes `impl Into<Prop>`" because
"`View<Msg>` has one inherent-method namespace across the whole catalogue" —
GTK gives `position` an `int` on `GtkPaned` and a `GtkPositionType` on
`GtkPopover`, and Rust has no overloading. P5 already solved that a different
way: every widget's setters live in a per-widget extension trait
(`pub trait ButtonExt<Msg>: Sized { fn label(self, &str) -> Self; … }` with
`impl<Msg: Clone + 'static> ButtonExt<Msg> for View<Msg>`), taking concrete
types. P5's 32 widgets are written that way and §9 forbids P6 from touching
P5's widget files.

**Ruling.** P5's convention wins for the whole catalogue. P6's deviation 13 is
withdrawn: P6 ships `BoxExt`, `PanedExt`, `PopoverMenuExt`, … with
concrete-typed setters. Different traits give `PanedExt::position(i32)` and
`PopoverExt::position(Position)` without any `Into<Prop>` erasure, which is
what §4.4's naming rule ("named after the GTK property, in `snake_case`, with
no `set_` prefix") reads most naturally as. Two consequences P6 must honour:
a trait method is **shadowed** by an inherent method of the same name, so no
`*Ext` trait may reuse a name P4 put on `View<Msg>` inherently (`class`, `id`,
`visible`, `sensitive`, `focusable`, `tooltip`, `halign`, `valign`, `hexpand`,
`vexpand`, `margin`, `width_request`, `height_request`, `cursor`, `key`,
`child`, `children`, `prop`, `on`, and the eighteen `on_*` setters); and
where a widget's own setter must differ from P4's inherent `on_*`, §10 P6-D14's
`.on_item_activated` renaming is the pattern to follow.

Carried out by: P6 (Task 4 and every widget task).

**AMENDED 2026-09-01 (§10 P6-D38).** P6 shipped ten inherent
`impl<Msg> View<Msg>` blocks alongside its `*Ext` traits, and the predicted
shadowing landed on `View::position` (over `PopoverExt::position`) and
`View::label` (over `ButtonExt::label`). The P6 whole-part fix wave converted
all ten; P6-D38 records the three consequences, notably that setter arguments
stay `impl Into<Prop>` except `PanedExt::position(i32)`, and that there is no
`FrameExt::label` because two traits cannot share a method name without making
every call site ambiguous.

### E9 — `on_date_selected` carries `Handler::Text` (`YYYY-MM-DD`)

P4 D13 rules `on_date_selected` is `Handler::Text` carrying `YYYY-MM-DD`, and
P4 Task 5 ships `View::on_date_selected(self, f: impl Fn(&str) -> Msg)` as an
**inherent** method. P5 Task 17 ships `CalendarExt::on_date_selected(self, f:
impl Fn(usize) -> Msg)` at `Handler::Index`, day-of-month. The inherent method
wins at every call site, so P5's would be silently unreachable and
`CalendarC`'s `fire_index(EventKind::DateSelected, …)` would never be observed.

**Ruling.** P4's `Handler::Text` / `YYYY-MM-DD` stands — it is the only one of
the two that can carry §5.1's `(i32, u32, u32)` losslessly. P5 removes
`on_date_selected` from `CalendarExt` entirely and `CalendarC::on_event` fires
`cx.handlers.fire_text(EventKind::DateSelected, "2026-03-14")`, zero-padded,
always four-digit year. P5's Task 17 test asserts on the string.

Carried out by: P5 (Task 17).

### E10 — `on_scrolled` and `on_reordered` take P6's two-argument handlers

Same shadowing hazard as E9. P4 Task 5 ships inherent
`on_scrolled(impl Fn(f64) -> Msg)` and `on_reordered(impl Fn(usize) -> Msg)`;
P6 deviation 4 adds `Handler::Pair`/`Handler::Indices` and requires
`.on_scrolled(|(f64, f64)| Msg)` and `.on_reordered(|(usize, usize)| Msg)`,
which is what §5.4 and §5.5 actually write.

**Ruling.** §5.4/§5.5 win. P6's Task 2, in the same commit that adds
`Handler::{Pair, Indices}` and `Handlers::{fire_pair, fire_indices}`,
**re-types P4's two inherent setters in place** to

```rust
pub fn on_scrolled(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;   // Handler::Pair
pub fn on_reordered(self, f: impl Fn(usize, usize) -> Msg + 'static) -> Self; // Handler::Indices
```

This is the one exception to "no P4 or P5 code touched" in P6 deviation 3;
P4's only call sites are its own `every_event_kind_has_exactly_one_on_setter`
test, which P6 updates in the same commit. The `EventKind`s
(`Scrolled`, `Reordered`) and the count of eighteen setters are unchanged.
P4's D13 half about `on_reordered` is superseded; its half about
`on_date_selected` stands (E9).

Carried out by: P6 (Task 2), with P4's test updated in that commit.

### E11 — P5 creates no `ListViewC` and no `list_view.rs`

P6 deviation 9 and its file table both assume "whatever minimal `ListViewC` P5
created" in `ui/src/widgets/list_view.rs`, and that P5's `DropDownC`/
`FontDialogC` embed `list: ListViewC`. P5 deviation 10 rules the opposite and
is the authority on what P5 ships: both fields are a plain `Node`, and
`ui/src/widgets/list_view.rs` does not appear in P5's file structure at all.

**Ruling.** P5 D10 stands. P6's Task 19 **creates** `list_view.rs` from
nothing (its file-table row's "replace P5's minimal one" is void) and must not
assume a P5 call site to keep compiling. Migrating `DropDownC.list` and
`FontDialogC.list` from `Node` to `ListViewC` is **out of M3 scope**: it would
require editing P5's widget files, which §9 forbids. Note it for M4.
`Selection`/`SelectionMode` are likewise P6-only; P5 needs neither.

Carried out by: P6 (Task 19).

### E12 — §8.4 R1's "~59" and §9's "27 kinds" are both wrong: 64 kinds, 32 per part

§4.2's own enumeration contains **64** `Kind` variants (P5's §5.1–§5.3 = 16 + 11
+ 5 = 32; P6's §5.4–§5.7 = 17 + 8 + 3 + 4 = 32). §8.4 R1 estimates "~59"; §9's
P6 boundary says "the 27 kinds in §5.4–§5.7". Both are miscounts — 27 is P6's
*file* count, not its kind count (six kinds share a file with their parent:
`NotebookTab`, `StackPage`, `ListBoxRow`, `FlowBoxChild`, `ColumnViewColumn`,
`PopoverMenuItem`).

**Ruling.** The enumeration in §4.2 is normative. Read §8.4 R1 as "**64**
`Kind` variants" and §9's P6 boundary as "the **32** kinds in §5.4–§5.7, in
**26** widget files". P4 D14's pinned counts — `Kind` 64, `PropName` 97,
`EventKind` 18 — are correct and a P4 test pins all three. A verified
coverage sweep confirms every one of the 64 is implemented by exactly one part
and every part task maps to a §4.2 variant: no orphans, no duplicates.

### E13 — `icons::Handle` is `Rc<skia_rs_safe::codec::Image>`, defined by P5 — PARTLY FIXED IN PLACE

P5 D3 rejected §5.1's `icons::Handle` (undefined in §6) and typed
`ImageC.resolved` as `Option<Rc<skia_rs_safe::core::Image>>`; P7 D2 defines
`pub type Handle = Rc<skia_rs_safe::codec::Image>`. `Image` lives in
`skia-rs-codec` (`crates/skia-rs-codec/src/image.rs:177`), not in
`skia-rs-core`, so P5's path was wrong; the seven occurrences in P5's plan have
been corrected to `skia_rs_safe::codec::Image` in place.

**Ruling.** P5 additionally adds the one-line
`pub type Handle = Rc<skia_rs_safe::codec::Image>;` to `ui/src/icons/mod.rs`
in the same commit that creates `icons/builtin.rs` (P5 D2 already gives P5 that
file), and types `ImageC.resolved` as `Option<icons::Handle>` per §5.1. P7's
Task 1 is already idempotent and keeps the alias.

Carried out by: P5 (Task 7 / Task 13), P7 (Task 1).

### E14 — `ui/src/widget/button.rs` gains the `icons` field; §8.3's "unchanged" row is amended

§8.3 lists `ui/src/widget/button.rs`'s four tests as "**not rewritten** …
unchanged", but §6/§8.1 add `icons` to `PaintCx` and that file constructs
`PaintCx { .. }` literals in its own tests. P7's file table names the edit and
promises "no behaviour, no pixel constant, no test assertion changes".

**Ruling.** §8.3's row is amended to read: *unchanged except for the
mechanical `icons: &mut IconTheme::with_name_and_roots("hicolor", vec![])`
field added to its `PaintCx` literals, exactly as `paint/mod.rs`'s row already
allows.* The §8.2 gate is unaffected —
`ui/tests/themed_button_offscreen.rs` (the M1 pixel gate) constructs no
`PaintCx` and stays byte-identical.

Carried out by: P7 (Task 13).

**Superseded in part by §10 P7-D53 (2026-09-01).** P7's fix wave found that
adding `icons` to the literals is not enough — `Button::render`'s *signature*
had grown a fifth parameter, which forced an edit to the §8.2 gate file. It
now owns its own `IconTheme` field instead, the M2 signature is restored, and
`ui/tests/themed_button_offscreen.rs` is byte-identical again. §8.3's
`ui/src/widget/button.rs` row reverts to plain "**not rewritten** …
unchanged": the file gains one private field and nothing in its four tests
moves.

### E15 — `has_frame` needs `PropName::HasFrame`, appended by P5

P5's `ButtonExt::has_frame` and `MenuButtonExt::has_frame` both store their
value under `PropName::ShowArrow`, which §4.3 spends on `MenuButton`'s
`always-show-arrow` and `DropDown`'s `show-arrow`. On `MenuButton` the two
setters would collide outright. P6 deviation 3 appends a `PropName::HasFrame`
variant, but P6 runs after P5.

**Ruling.** P5 appends `HasFrame` to `PropName` itself, in the commit that
lands `ButtonC`. §4.3's `PropName` is `#[non_exhaustive]` and P6 deviation 3
already establishes that appending variants is additive and permitted; §9's
"P5 must not touch `view/**`" is read, as it already is for D5's `builders.rs`
re-exports, as *must not change existing items*. P6 then does not append
`HasFrame` a second time, and P4's `PropName` count of 97 (D14) becomes 98 with
P5's variant and is re-pinned in P5's close-out.

Carried out by: P5 (Task 19), P6 (Task 2).

### E16 — `ui/README.md` is appended to by P3–P5 and P7, and finalised by P8

§9 gives `ui/README.md` to P8 exclusively, but P3 (Task 20), P4 (Task 18) and
P5 (Task 27) each add their layer's section as part of landing, and P7's
boundary explicitly says it must *not* touch it.

**Ruling.** No conflict, made explicit: P3, P4 and P5 **append** a section for
the layer they landed; P7 appends nothing; P8 owns the file as a whole and does
the M3 pass — the widget table, the module map and the status line — reconciling
whatever the earlier parts wrote. P8's added test
`the_readme_widget_table_lists_every_kind` (P8 D8) is what keeps it honest.

### E17 — `ConstraintAdjustment` is hand-rolled, not a `bitflags` type

§1.1 spells `ConstraintAdjustment` with `bitflags::bitflags!`. The `wlr` crate
has no `bitflags` dependency and declines one twice in its own source
(`buffer.rs:386` for `DataPtrAccess`, and `BufferCaps` before it), pinning the
bit values with a test instead. **Ruling:** P1 hand-rolls it —
`pub struct ConstraintAdjustment(u32)` with six associated consts, `BitOr`,
`BitOrAssign`, `contains` and `bits`. Every call site spells identically
(`ConstraintAdjustment::FLIP_X | ConstraintAdjustment::SLIDE_Y`,
`.contains(...)`), so no other part is affected. **Carried out by:** P1.

### E18 — `PopupId` gains the two dangling constructors

§1.1 lists only `PopupId`'s derives. `ToplevelId` and `LayerSurfaceId` both
carry `dangling_for_test` (and `ToplevelId` also `dangling_nth_for_test`), and
without them the "an unknown id misses rather than dereferencing" promise cannot
be tested by a consumer — `crates/wlr/tests/popups.rs` and P2's `State` unit
tests both need it. **Ruling:** additive; P1 adds
`PopupId::{dangling_for_test, dangling_nth_for_test}` mirroring `ToplevelId`'s
exactly, including the 2^32 band. **Carried out by:** P1. **Consumed by:** P2
(`PopupKey::for_test` can wrap `PopupId::dangling_nth_for_test`).

### E19 — an unknown positioner enum value is dropped silently, not logged

§1.1 and the spec's §7 both say an unknown `xdg_positioner_anchor`/`_gravity`
value "maps to `None` and is logged once, never panics". The `wlr` crate binds
**no** Rust-side logging symbol and says so in its own source
(`runtime.rs:8741-8744`): wlroots' `wlr_log` is a `static inline` macro over
an unbound `_wlr_log`, and the crate deliberately has no `log`/`tracing`
dependency. **Ruling:** the never-panic half stands in full and is directly
tested (`a_positioner_with_nonsense_enum_values_never_panics`); the logging half
is dropped in `wlr` and **moves to P2**, which has logging and is where a
malformed positioner first becomes an observable compositor decision.
**Carried out by:** P1 (the silent mapping), P2 (the log).

### E20 — `Popup::unconstrain` DOES send, and is guarded on `initialized`

§1.2 says `unconstrain` "does not send anything". **That is factually wrong**,
and the wrong sentence pointed a consumer at a process abort. Disassembling
this distribution's shipped `libwlroots-0.20.so`:
`wlr_xdg_popup_unconstrain_from_box` calls
`wlr_xdg_positioner_rules_unconstrain_box` **and then**
`wlr_xdg_surface_schedule_configure` unconditionally (call sites `0x8f20e`,
`0x8f255`, `0x8f25e`). `wlr_xdg_surface_schedule_configure` contains
`assert(surface->initialized)`, and this distribution ships wlroots **without
`NDEBUG`**. §1.4's `ToplevelHandler::new_popup` is exactly where a consumer is
told to place a popup, and `initialized` is still false there — no commit has
happened — so `popup.unconstrain(&box)` from inside `new_popup` killed the
whole compositor process.

**Ruling:** §1.2's "does not send anything" is struck. `Popup::unconstrain`
carries the same `base.is_null() || !(*base).initialized` early return
`Popup::send_configure` already had, and **does nothing at all** on a
not-yet-initialized popup rather than aborting; its signature is unchanged and
the guard is invisible to any caller holding a live, committed popup.
`Runtime::configure_popup` keeps its own `initialized` check, which is what
lets it report `false` instead of silently succeeding. Pinned by
`unconstraining_an_uninitialized_popup_is_a_no_op_rather_than_an_abort`
(mutation-verified: deleting the guard kills the test binary).
**Carried out by:** P1. **Consumed by:** P2 (may call `unconstrain` from a
reactive-reposition pass without pre-checking initialization).

### E21 — `wlr_xdg_popup_unconstrain_from_box` is incremental; `Popup::unconstrain` reseeds first
wlroots 0.20's unconstrain uses `scheduled.geometry` as an in/out box and returns early when the
already-adjusted box fits the new constraint, so compositor-driven re-placement of a reactive popup
re-sent stale geometry. `Popup::unconstrain` now reseeds `scheduled.geometry` from the positioner
rules (`wlr_xdg_positioner_rules_get_geometry`) before unconstraining — wlroots' own reposition
path — so repeat calls with a different constraint box yield the correct configure. Fixed in
wlr 0.20.28 (commit 02b1c90 on feature/wlr-xdg-popup). P2's
`a_reactive_popup_is_reconfigured_when_its_parent_moves` is the end-to-end proof.

### M5-D1 — `wait_bounded` polls N fds, not one (superseded §3.1)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §3.1's single-`PollFd`
`wait_bounded` is replaced by a set whose element 0 is the Wayland queue and
whose rest are `Window::watch_fd` registrations, reported as
`InputEvent::FdReady(WatchId)`. Full text:
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §1 M5-D1.

### M5-D4 — `App::new` takes closures, not `fn` pointers (amends §4.7)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §4.7's
`new(model, update: fn(..), view: fn(..))` (this file, l. 1516-1521) becomes
`impl FnMut`/`impl Fn`, a strict widening: every `fn`-item call site compiles
unchanged. Full text: M5 contract §1 M5-D4.

### M5-D2 — `App` grows an external-message hook (§4.7 had none)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §4.7's loop had no way for a
worker thread to reach `update`; `App::with_inbox` and `App::on_fd` add one, on
a normative per-frame drain order — the inbox's wake pipe, then its channel in
send order, then `on_fd` messages, then the frame's own input batch, folded
once. `run` registers the pipe with `Window::watch_fd` and unwatches it on
exit; `run_offscreen` drains at the same point so an ingress test needs no
compositor. Full text: M5 contract §1 M5-D2.

### M5-D3 — `Cmd` grows a side-effect leaf (§4.7 had none)

**Carried out by:** M5 P0. **Added:** 2026-09-03. `Cmd::Task(Rc<dyn Fn()>)`
runs a non-blocking side effect on the loop thread, outside `update`, after the
fold that produced it — a `flatten` leaf, printed opaquely as `Task(..)`,
executed identically by `drain` whether reached through `run` or
`run_offscreen`. It has no return value: an answer comes back through the
inbox (M5-D2) as a message. Full text: M5 contract §1 M5-D3.

### M5-D5 — three pointer `EventKind`s and a button-carrying `Handler` (extends §4.4)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §4.4's `EventKind` gains
`PointerDown`/`PointerMotion`/`PointerUp` (appended to `ALL`, now 21 entries)
so raw pointer phases reach any widget kind, not only the ones with a typed
gesture already. `Handler` gains `PairButton(Rc<dyn Fn(f64, f64, u32) ->
Msg>)`; `Handlers::fire_pair_button` fires either a `PairButton` or a `Pair`
binding on the same kind (button dropped for the latter), with no `Unit`
fallthrough. Six builders (`on_pointer_down`/`_motion`/`_up`, each with a
`_with_button` variant) and `BTN_RIGHT`/`BTN_MIDDLE` beside a re-exported
`BTN_LEFT` in `window::pointer` (P0-D3). They are fired centrally, by
`view::app::fire_pointer_handlers` from `view::app::deliver` (P0-D1) rather
than from `GenericC::on_event`, at `Phase::Target` and again on the ancestor
chain at `Phase::Bubble` (P0-D8); the gallery's `drawing_area` sample binds all
three. Full text: M5 contract §6 P0-D1, P0-D2, P0-D3, P0-D8.

### M5-D6 — `Prop::Draw` carries the paint context (widens §4.3)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §4.3's
`Prop::Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect)>)` widens to `Fn(&mut Canvas<'_>,
Rect, &mut crate::paint::PaintCx<'_>)` so a `drawing_area` callback can shape
text, resolve an icon or read the theme's colours — the same paint context
every controller's `paint` receives — instead of being limited to fills and
strokes. `Props::diff` still compares by `Rc` pointer, so no diff behaviour
changes. Full text: M5 contract §1 M5-D6.

### M5-D7 — `Keymap` exposes a level-0 lookup and `KeyEvent` carries it (extends §3.3)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §3.3's `Keymap` gains
`base_keysym(keycode) -> xkb::Keysym`, the sym at group 0, level 0 —
GDK's `translate_key(keycode, 0, 0)` — never panicking on an unmapped
keycode (`Keysym::NoSymbol` instead). Every `KeyEvent` gains a `base` field,
stamped by `Keymap::translate` with `base_keysym(keycode)`, so a view-layer
`on_key` closure can normalise an accelerator capture without reaching into
the window. Settings' keybinding capture needs exactly this: `icedtea_config`'s
`key_name_to_keysym` always encodes the unshifted keysym, so a capture that
stored the modified sym would produce a binding the compositor's
`match_action` can never fire. Full text: M5 contract §1 M5-D7.

### M5-D9 — probing works on a live `Window`, not only offscreen (extends P8-D59/P8-D60)

**Carried out by:** M5 P0. **Added:** 2026-09-03. P8-D59/P8-D60 put probing on
`App::probe`, which is offscreen-only — a harness test against a *running*
process has no other way to ask where a widget ended up, and the gate rules
forbid hard-coded coordinates. `window::probe_points_of`/`allocation_of` label
and centre a laid-out tree with the gallery's own rule (id-or-node-name with a
repeat index, floored centres), and `Window::probe_points`/`Window::allocation`
expose them. `window-probe --emit-probe` writes the report lines
`ui/tests/support` already parses (`probe <label> <x> <y>`,
`alloc <id> <x> <y> <w> <h>`). Full text: M5 contract §1 M5-D9.

### M5-D8 — four widgets paint at rest; `KNOWN_BLANK_AT_REST` drops to six (supersedes P8-D69)

**Carried out by:** M5 P0. **Added:** 2026-09-03. P8-D69's fifteen-entry
`KNOWN_BLANK_AT_REST` list is superseded: `ColorDialogButtonC`,
`ColorDialogC`, `CheckButtonC` and `ScrollbarC` now paint at rest —
respectively the swatch on a real intrinsic floor (P0-D10), the palette grid
`measure` and `paint` both read from one `grid()`, the unchecked indicator box
from `style.color()`/`style.border_colors()`, and the trough plus slider — so
`ui/tests/gallery_gate.rs`'s list is exactly six entries
(`window_controls`, `font_dialog`, `popover_menu`, `popover_menu_bar`,
`alert_dialog` — all zero-area allocations — and `link_button`), pinned by `the_readme_names_every_known_blank_widget` against
`ui/README.md`. P8-D69's own text is retained above as the record of what
fifteen used to mean; the live number is this one. The gate's separate
`THEME_BLIND_BY_DESIGN` carve-out grows to four with `color_dialog`, recorded
as M5 contract §6 P0-D9. Full text: M5 contract §1 M5-D8, §6 P0-D9, P0-D10.

### M5-P0-fix — `Cmd::Unwatch`, and pointer handlers that bubble

**Carried out by:** M5 P0 (whole-part fix wave). **Added:** 2026-09-04.
`Cmd` gains `Unwatch(WatchId)`, handled in `App::run`'s window-bound command
match: without it an app that has handed its window to `App::run` cannot retire
a watch at all, and a level-triggered `POLLHUP`/`POLLERR` on a foreign fd spins
the loop with no exit (M5 contract §6 P0-D7). `fire_pointer_handlers` also runs
at `Phase::Bubble`, so a container with a pointer handler sees a gesture that
landed on a child instance, subject to P4-D19's `cx.handled` rule (§6 P0-D8).
Full text: M5 contract §6 P0-D7, P0-D8.
