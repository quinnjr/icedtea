# M6.1 Spec 1 — Compositor-side IME relay (text-input-v3 ↔ input-method-v2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make icedtea's compositor speak `zwp_text_input_manager_v3` and `zwp_input_method_manager_v2`, relaying an app's text-input to an external IME/OSK (fcitx5/ibus/squeekboard) — activate/surrounding-text/commit in both directions, candidate-popup placement, and keyboard-grab pass-through — so M6.1 Spec 2 (the toolkit client) has a working relay to target.

**Architecture:** The relay is genuine cross-object routing (wlroots 0.20 ships **no** `wlr_input_method_relay` helper). It lives in the `wlr` crate (wlroots-sys), hung off the crate's existing seat, keyboard-focus, and `on_key`/`on_modifiers` chokepoints — the M4.5 pointer-constraints precedent. The compositor's only jobs are: create the two globals at boot (non-fatal, like every other protocol), place IME candidate popup surfaces in the scene graph (the one piece not in the crate, because `wlr_input_popup_surface_v2` has no `xdg_positioner`), and expose a test oracle. Shipped as two additive `wlr` releases (A6.1 = 0.20.30 core relay; A6.2 = 0.20.31 popups + keyboard-grab), each consumed by a matching compositor bump — the M4.5 split-phase publish→consume shape.

**Tech Stack:** Rust 2024; `wlr`/`wlr-sys` (smithay→wlr FFI port of wlroots 0.20); raw `sys::wlr_*` FFI with RAII `Registration` listeners keyed by destroy-listener address; icedtea compositor (`icedtea-compositor`) + `harness` test-double clients (`wayland-client`); `crossbeam-channel` `DbCommand` oracle. Tests are automated-only (no real IME); harness/compositor integration tests run with `--test-threads=1`.

**Spec:** `docs/superpowers/specs/2026-09-08-m6-1-ime-compositor-relay-design.md` (which adopts `docs/superpowers/specs/2026-08-20-icedtea-input-methods-design.md`, roadmap **A6** — the authoritative design; its 9 decisions of record are cited throughout as "decision #N").

## Global Constraints

Copied verbatim from the spec (§Refresh, §Workflow) and A6 (§Decisions, §Out of scope). Every task's requirements implicitly include this section.

- **External IME only.** No IME/OSK application is shipped in this repo. The compositor speaks the two protocols correctly; fcitx5/ibus/squeekboard are third-party clients. The harness test-doubles are the only drivers.
- **The relay lives in the `wlr` crate, not the compositor** (A6 decision #2). The compositor creates the two globals, places popup surfaces, and exposes a test oracle — nothing else.
- **`tablet-v2` is out** (A6 decision #1) — unrelated stylus protocol, a later self-contained follow-on. Not in this milestone.
- **`wp_text_input_unstable_v1`/`v2` are out** — v3 only, matching wlroots 0.20 (only `wlr_text_input_v3.h` ships).
- **Single headless seat (`seat0`).** No multi-seat input-method semantics.
- **The compositor never interprets `content_type`** — it is relayed faithfully to the IME, which acts on it.
- **Virtual-keyboard is untouched** — A6 is additive on top of `create_virtual_keyboard_manager`; complementary, not redundant.
- **`wlr` releases are additive and next after 0.20.29:** 0.20.30 (A6.1), 0.20.31 (A6.2). Semver-additive (defaulted trait methods; new fns), so an existing `impl` written against 0.20.29 still compiles.
- **`[patch.crates-io]` during dev, published pin at the end.** During development the compositor consumes the wlroots-sys worktree via `[patch.crates-io]` in the icedtea root `Cargo.toml`; the published `wlr = "0.20.30"`/`"0.20.31"` pin bump + patch drop is an explicit late task in each part.
- **`cargo publish` is a CONSENT STOP.** An API-review verdict is held before `cargo publish -p wlr`; do not publish without the owner's explicit approval.
- **wlroots-sys is a shared repo.** Coordinate with any parallel wlroots-sys session via `SendMessage` **before** pushing to its `develop`. `[patch]` resolves to whatever branch is checked out there.
- **git-flow, `--keepremote` finish, merge window.** Feature branches off `develop` in both repos; SDD with per-task Opus review + a whole-branch Opus review; a **merge window** is presented for the owner's go/no-go before any `git flow … finish` — never auto-merged. wlroots-sys integrates via a PR into `develop`.
- **Gates.** wlroots-sys: `cargo test -p wlr` + `cargo test -p wlr-sys` + `cargo clippy --all-targets -- -D warnings` + `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps` + `cargo fmt --all --check` + `cargo test -p wlr --test coverage_audit`, green on the freeze commit before each publish. icedtea: `cargo test --workspace` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --all --check`.

---

## Reconciliations from A6 (ratified by the controller before execution)

A6's design is version-independent and correct in shape, but it cites the pre-port flat `src/` layout and stale line numbers. The current wlroots-sys `wlr` crate (develop @ `879d262`, 0.20.29) and icedtea compositor (develop @ `2955997`) were mapped for this plan. Deltas the controller must ratify:

1. **Version.** A6 says 0.20.23 (A6.1) / 0.20.24 (A6.2); current published is 0.20.29. This plan ships **0.20.30 (A6.1)** and **0.20.31 (A6.2)**.
2. **Layout.** Not flat `src/`. wlroots-sys is a workspace: the safe crate is `crates/wlr/src/` (`runtime.rs` 11 140 lines, `backend.rs` 9 529, `handler.rs`, `dispatch.rs`, `seat.rs`); the FFI crate is `crates/wlr-sys/` (bindings snapshot `crates/wlr-sys/prebuilt/bindings-docsrs.rs`). icedtea paths are `compositor/src/…` and `harness/src/…`.
3. **Chokepoint line numbers refreshed** (all A6 numbers are stale):
   - `Runtime::focus_toplevel_keyboard` → `crates/wlr/src/runtime.rs:8262` (A6 said 6739).
   - `Runtime::clear_keyboard_focus` → `crates/wlr/src/runtime.rs:8320` (A6 said 6797).
   - `on_key`/`on_modifiers` → `crates/wlr/src/backend.rs:7104` / `:7175`; the seat-notify calls to branch around are `wlr_seat_keyboard_notify_key` at `:7165` and `wlr_seat_keyboard_notify_modifiers` at `:7192` (A6 said 5332-5420).
   - `create_pointer_constraints_manager` (the manager-creation precedent) → `runtime.rs:5152`; its ptr accessor `pointer_constraints_manager_ptr` → `:5171`; `RuntimeInner` constraint fields → `:286-329` (A6 said 4798 / 200-205).
   - `on_new_pointer_constraint` (the `on_new_*` listener precedent) → `backend.rs:4339`; the boot-time manager-listener registration block → `backend.rs:2056`; the destroy-keyed table `Session::pointer_constraints: RefCell<HashMap<usize, PointerConstraintListeners>>` → `backend.rs:867`, keyed by `destroy.listener_addr()` (a `usize`), with removal at `:4686`.
4. **There is no single keyboard-focus chokepoint pair.** A6 assumed `focus_toplevel_keyboard`/`clear_keyboard_focus` are the only two. In fact **four** `Runtime` methods drive `wlr_seat_keyboard_notify_enter`/`notify_clear_focus`: `focus_toplevel_keyboard` (`:8262`), `focus_layer_keyboard` (~`:8041`), `focus_xwayland_surface_keyboard` (~`:6716`, behind `#[cfg(wlr_has_xwayland)]`), and `clear_keyboard_focus` (`:8320`). A fifth, `focus_lock_surface_keyboard` (pub(crate), ~`:6700`), focuses the lock screen and must be **excluded** (no IME while locked). The relay therefore introduces one private helper `RuntimeInner::relay_keyboard_focus(new_surface)` invoked from those four sites, not two. (wlroots' `wlr_seat` keyboard_state has **no** `focus_change` signal — only `pointer_state` does — so a signal-driven approach like cursor-shape's is not available; the mutator sites must be hooked directly.)
5. **Tracking tables live on `RuntimeInner`, not `Session`.** A6 says `RuntimeInner` (correct) but models them on the pointer-constraints `HashMap`, which actually lives on `Session` in `backend.rs`. That works for constraints because their focus-reconciliation runs in a **backend** function (`enter_surface_under_cursor`, which holds `&Session`) and uses the wlroots per-surface helper `wlr_pointer_constraints_v1_constraint_for_surface`. Text-input enter/leave runs in **`Runtime` methods** (item 4) that have **no** `Session`, and there is no per-surface wlroots lookup for text-inputs. So the relay tables must be readable from `Runtime` → they live on `RuntimeInner`. `Session` holds `runtime: &Runtime` (`backend.rs:904`) and reaches `runtime.inner.*` directly (precedent: `runtime.inner.active_constraint.set(None)` at `backend.rs:4690`), so the backend `on_*` listener callbacks mutate the same `RuntimeInner` tables. Destroy listeners remove their own entry (keyed by `destroy.listener_addr()`), whose drop unlinks the bundled `Registration`s — the M4.4/M4.5 destroy-keying lesson, just homed on `RuntimeInner`. One `Runtime` is created per compositor process (`Runtime::new`, `compositor/src/lib.rs:59`) and the headless harness spawns a fresh process per test (`Compositor::spawn`), so RuntimeInner-homed listeners unlink at process end with no cross-run leak.
6. **`render.rs` is pure geometry — it does not create scene nodes.** A6 decision #7 says the compositor places the popup "the way `render.rs` places nodes." `compositor/src/render.rs` is in fact `scene_order`/`decoration_strip_geometry`/`wallpaper_color` — pure functions, no wlroots calls. Scene-node creation is **crate-owned**: `Runtime::add_rect_in_band` / `add_buffer_in_band` / `set_node_position` (`runtime.rs:3221`) / `reparent_node` (`:3349`), over the `Band` enum (`runtime.rs:840`: `Background`/`Bottom`/`Toplevel`/`Top`/`Overlay`/`Lock`). So A6.2 adds a **new** crate method to build a scene node from the popup's client `wlr_surface` (wrapping `sys::wlr_scene_subsurface_tree_create`, which the FFI already exposes), returning a `NodeId`; the compositor computes the anchor from the text-input's `cursor_rectangle`, clamps to the output, calls `set_node_position`, and calls the new `send_input_popup_rectangle`. Popups sit in `Band::Top` (above toplevels, below `Overlay`).
7. **Handler additions ride the `Event` enum + `dispatch.rs` + a defaulted trait method** — the `SeatHandler::session_lock_changed` precedent (`handler.rs:1027`, dispatched via `Event::SessionLockChanged(bool)` at `dispatch.rs:203` and `backend.rs:2574`). A6.2 adds an `InputMethodHandler` with defaulted `new_popup_surface`/`popup_surface_destroyed`/`popup_repositioned` methods, additively.
8. **Coverage audit.** The ~30 `wlr_text_input_*` / `wlr_input_method_*` / `wlr_input_popup_*` symbols are currently rows in `crates/wlr/coverage/waived.toml` with `reason = "not-yet"`, `milestone = "M8"` (e.g. `waived.toml:1306`, `:3928`). `cargo test -p wlr --test coverage_audit` requires that every wrapped symbol appears in a `sys::` use under `crates/wlr/src` **and** as a `[[wrapped]]` row (`module` + `item`) in `wrapped.toml`, and forbids a `wrapped` row for a symbol not referenced. So each symbol this plan wraps **moves** from `waived.toml` to `wrapped.toml`; symbols genuinely left unused (e.g. `wlr_input_method_v2_send_text_change_cause` if not forwarded) stay waived with the milestone note re-pointed from "M8" to this milestone. This is an explicit step in the A8/A13 release tasks.

### Controller rulings (ratified 2026-09-08)

- **Q1 (A6.2 popup scene API name) → RATIFIED as written.** The new crate method is `Runtime::add_input_popup_in_band(popup: InputPopupSurfaceId, band: Band) -> Option<NodeId>`, wrapping `wlr_scene_subsurface_tree_create` over the popup's `surface`. Rationale: matches the existing `add_rect_in_band`/`add_buffer_in_band` naming precedent, and keeps all FFI/unsafe crate-side (the "relay lives in the crate" constraint, A6 decision #2) — the compositor stays pure geometry (compute anchor, clamp, `set_node_position`). The raw-surface-plus-`reparent_node` variant is rejected as leaking scene FFI into the compositor.
- **Q2 (test 10 scope) → RATIFIED: keep.** Test 10 ("icedtea keybindings still fire during a grab") stays in Part B/A6.2 (Task B9, Step 3). Cheap, and it guards the hottest path against a grab-branch refactor short-circuiting `dispatcher.emit` (decision #6).
- **Q3 (one PR or two) → RATIFIED: two.** A6.1 and A6.2 are **separate release+PR pairs** (0.20.30 then 0.20.31, each its own wlroots-sys PR + icedtea PR), per A6 decision #8 and the M4.5 precedent — so A6.2's new scene code and hot-path `on_key` branch review in isolation on a stable, already-consumed A6.1.

---

# PART A — the `wlr` crate relay (wlroots-sys), release 0.20.30 (A6.1: core relay)

Branch `feature/wlr-a6.1` off `develop` in **`/home/joseph/Projects/wlroots-sys`** (coordinate via SendMessage before any push to its `develop`). All paths below are under that repo. Model tier per task noted; the relay's cross-object routing is **Opus**, boilerplate is **Sonnet**.

The sys bindings for every symbol this part needs already exist in `crates/wlr-sys/prebuilt/bindings-docsrs.rs` (verified: the two managers + their `_create`, `wlr_text_input_v3` with `pending`/`current`/`current_enabled`/`current_serial`/`focused_surface` and `events.{enable,commit,disable,destroy}`, `wlr_input_method_v2` with `current`/`active`/`popup_surfaces`/`keyboard_grab` and `events.{commit,new_popup_surface,grab_keyboard,destroy}`, and all `wlr_text_input_v3_send_*` / `wlr_input_method_v2_send_*` functions). **No bindings regeneration is needed** — this part is additive Rust over an already-bound API.

---

### Task A1: RuntimeInner relay state + entry types + the two `create_*_manager` fns

**Files:**
- Modify: `crates/wlr/src/runtime.rs` — `RuntimeInner` struct near `:286-329` (add fields), the `RuntimeInner { … }` initialiser near `:1185` (default them), and add the two `create_*_manager` fns + ptr accessors near the pointer-constraints ones (`:5152`).
- Test: `crates/wlr/tests/input_method.rs` (new).

**Interfaces:**
- Produces:
  - `struct TextInputEntry { raw: NonNull<sys::wlr_text_input_v3>, client: *mut sys::wl_client, _listeners: [Registration; 4] }` (enable/commit/disable/destroy).
  - `struct InputMethodEntry { raw: NonNull<sys::wlr_input_method_v2>, focused_text_input: Option<usize>, keyboard_grab: Option<NonNull<sys::wlr_input_method_keyboard_grab_v2>>, _listeners: Vec<Registration> }` (`focused_text_input` is the `text_inputs` map key of the text-input currently driving activation).
  - `RuntimeInner` fields (all `pub(crate)`):
    - `text_input_manager: RefCell<Option<NonNull<sys::wlr_text_input_manager_v3>>>`
    - `input_method_manager: RefCell<Option<NonNull<sys::wlr_input_method_manager_v2>>>`
    - `text_inputs: RefCell<HashMap<usize, TextInputEntry>>` (keyed by the destroy-listener address)
    - `input_method: RefCell<Option<InputMethodEntry>>`
  - `pub fn create_text_input_manager(&self, display: &Display) -> Result<()>`
  - `pub fn create_input_method_manager(&self, display: &Display) -> Result<()>`
  - `pub(crate) fn text_input_manager_ptr(&self) -> Option<NonNull<sys::wlr_text_input_manager_v3>>`
  - `pub(crate) fn input_method_manager_ptr(&self) -> Option<NonNull<sys::wlr_input_method_manager_v2>>`
- Consumes: the `create_pointer_constraints_manager` shape at `runtime.rs:5152` (copy its double-create guard + `NonNull::new(...).ok_or(Error::Create(...))` idiom verbatim).

- [ ] **Step 1 (Sonnet): Write the failing test**

`crates/wlr/tests/input_method.rs`:
```rust
//! A6.1 core relay: manager creation + double-create guard, driven headlessly.
use wlr::Runtime;

#[test]
fn create_text_input_and_input_method_managers_once() {
    let (rt, display) = wlr::test_support::headless_runtime(); // existing test helper
    rt.create_text_input_manager(&display).expect("first text-input create");
    rt.create_input_method_manager(&display).expect("first input-method create");
    assert!(rt.create_text_input_manager(&display).is_err(), "double-create must error");
    assert!(rt.create_input_method_manager(&display).is_err(), "double-create must error");
}
```
(If no `test_support::headless_runtime` exists, mirror the setup at the top of an existing `crates/wlr/tests/*.rs` that already builds a headless `Runtime` + `Display` — check `tests/headless.rs` for the exact constructor and reuse it verbatim.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wlr --test input_method create_text_input_and_input_method_managers_once`
Expected: FAIL — `no method named create_text_input_manager`.

- [ ] **Step 3 (Opus): Add the fields, entry types, and create fns**

In `RuntimeInner` (after the pointer-constraints fields, ~`:290`):
```rust
pub(crate) text_input_manager: RefCell<Option<NonNull<sys::wlr_text_input_manager_v3>>>,
pub(crate) input_method_manager: RefCell<Option<NonNull<sys::wlr_input_method_manager_v2>>>,
pub(crate) text_inputs: RefCell<HashMap<usize, TextInputEntry>>,
pub(crate) input_method: RefCell<Option<InputMethodEntry>>,
```
In the initialiser (~`:1185`, beside `pointer_constraints_manager: RefCell::new(None)`):
```rust
text_input_manager: RefCell::new(None),
input_method_manager: RefCell::new(None),
text_inputs: RefCell::new(HashMap::new()),
input_method: RefCell::new(None),
```
The two create fns, mirroring `create_pointer_constraints_manager` (`:5152`):
```rust
/// Create the `zwp_text_input_manager_v3` global. Apps bind it to declare an
/// editable field; the crate relays their state to the bound input-method.
pub fn create_text_input_manager(&self, display: &Display) -> Result<()> {
    if self.inner.text_input_manager.borrow().is_some() {
        return Err(Error::Create("Runtime::create_text_input_manager called twice"));
    }
    // SAFETY: `display` is a live wlr_display owned by the caller.
    let raw = unsafe { sys::wlr_text_input_manager_v3_create(display.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_text_input_manager_v3_create"))?;
    *self.inner.text_input_manager.borrow_mut() = Some(raw);
    Ok(())
}
/// Create the `zwp_input_method_manager_v2` global. At most one client per seat
/// is tracked (decision #3); a second is sent `unavailable`.
pub fn create_input_method_manager(&self, display: &Display) -> Result<()> {
    if self.inner.input_method_manager.borrow().is_some() {
        return Err(Error::Create("Runtime::create_input_method_manager called twice"));
    }
    let raw = unsafe { sys::wlr_input_method_manager_v2_create(display.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_input_method_manager_v2_create"))?;
    *self.inner.input_method_manager.borrow_mut() = Some(raw);
    Ok(())
}
pub(crate) fn text_input_manager_ptr(&self) -> Option<NonNull<sys::wlr_text_input_manager_v3>> {
    *self.inner.text_input_manager.borrow()
}
pub(crate) fn input_method_manager_ptr(&self) -> Option<NonNull<sys::wlr_input_method_manager_v2>> {
    *self.inner.input_method_manager.borrow()
}
```
Define `TextInputEntry` / `InputMethodEntry` (per Interfaces) near the top of `runtime.rs` beside other `pub(crate)` structs. Use `Registration` (from `crate::backend`, already `pub(crate)`) for the listener handles.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p wlr --test input_method create_text_input_and_input_method_managers_once`
Expected: PASS.

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/runtime.rs crates/wlr/tests/input_method.rs
git commit -m "feat(wlr): text-input/input-method manager creation + relay state (A6.1)"
```

---

### Task A2: `on_new_text_input` / `on_new_input_method` listeners + boot registration + second-IME-refused

**Files:**
- Modify: `crates/wlr/src/backend.rs` — add the two `on_new_*` `extern "C"` fns beside `on_new_pointer_constraint` (`:4339`); register both managers' listeners in the boot regs block beside the pointer-constraints registration (`:2056`).
- Test: `crates/wlr/tests/input_method.rs`.

**Interfaces:**
- Consumes: `Runtime::text_input_manager_ptr` / `input_method_manager_ptr` (A1); `Registration::link_bare` + `.listener_addr()` (backend, `:344`/`:414`); the `bound_of(l)` + `(*bound).session.cast::<Session<'_, S>>()` idiom (`on_new_pointer_constraint`, `:4348`).
- Produces: `on_new_text_input::<S>` inserts a `TextInputEntry` into `runtime.inner.text_inputs` keyed by its destroy-listener addr, with `enable`/`commit`/`disable`/`destroy` listeners linked on `(*ti).events.*`; `on_new_input_method::<S>` refuses a second bind with `wlr_input_method_v2_send_unavailable` and stores the first as `InputMethodEntry` with `commit`/`destroy` listeners (popup/grab listeners are added in A6.2). Backing destroy handlers `on_text_input_destroy` / `on_input_method_destroy` remove the entry (and clear `focused_text_input` if it named this one).

- [ ] **Step 1 (Sonnet): Write the failing test**

Extend `input_method.rs` — this needs the icedtea harness to bind real clients, so the *behavioural* assertion for second-refused lands in Part B (test 6). Here assert only that registration is wired without panic on a headless run that binds nothing:
```rust
#[test]
fn managers_register_listeners_without_a_client() {
    let (rt, display) = wlr::test_support::headless_runtime();
    rt.create_text_input_manager(&display).unwrap();
    rt.create_input_method_manager(&display).unwrap();
    // Driving one dispatch iteration with no client must not touch the empty tables.
    wlr::test_support::dispatch_once(&rt); // existing helper; else roundtrip an empty Display
    assert!(rt_debug_text_input_count(&rt) == 0);
}
```
Add a `#[cfg(test)]`/`pub(crate)` debug accessor `rt_debug_text_input_count` returning `self.inner.text_inputs.borrow().len()` if no equivalent exists.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wlr --test input_method managers_register_listeners_without_a_client`
Expected: FAIL — helper/accessor missing, or (once compiling) confirms the empty table.

- [ ] **Step 3 (Opus): Implement the listeners + boot registration**

`on_new_text_input` (model on `on_new_pointer_constraint`, `:4339`):
```rust
unsafe extern "C" fn on_new_text_input<S: Handlers>(l: *mut sys::wl_listener, data: *mut std::ffi::c_void) {
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(ti) = NonNull::new(data.cast::<sys::wlr_text_input_v3>()) else { return };
        let client = sys::wl_resource_get_client((*ti.as_ptr()).resource);
        let enable  = Registration::link_bare(&raw mut (*ti.as_ptr()).events.enable,  on_text_input_enable::<S>,  (*bound).session, std::ptr::null());
        let commit  = Registration::link_bare(&raw mut (*ti.as_ptr()).events.commit,  on_text_input_commit::<S>,  (*bound).session, std::ptr::null());
        let disable = Registration::link_bare(&raw mut (*ti.as_ptr()).events.disable, on_text_input_disable::<S>, (*bound).session, std::ptr::null());
        let destroy = Registration::link_bare(&raw mut (*ti.as_ptr()).events.destroy, on_text_input_destroy::<S>, (*bound).session, std::ptr::null());
        let key = destroy.listener_addr();
        (*session).runtime.inner.text_inputs.borrow_mut().insert(
            key, TextInputEntry { raw: ti, client, _listeners: [enable, commit, disable, destroy] });
    }
}
```
`on_new_input_method` — second-refused (decision #3):
```rust
unsafe extern "C" fn on_new_input_method<S: Handlers>(l: *mut sys::wl_listener, data: *mut std::ffi::c_void) {
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(im) = NonNull::new(data.cast::<sys::wlr_input_method_v2>()) else { return };
        if (*session).runtime.inner.input_method.borrow().is_some() {
            sys::wlr_input_method_v2_send_unavailable(im.as_ptr()); // do NOT track it
            return;
        }
        let commit  = Registration::link_bare(&raw mut (*im.as_ptr()).events.commit,  on_input_method_commit::<S>,  (*bound).session, std::ptr::null());
        let destroy = Registration::link_bare(&raw mut (*im.as_ptr()).events.destroy, on_input_method_destroy::<S>, (*bound).session, std::ptr::null());
        *(*session).runtime.inner.input_method.borrow_mut() = Some(InputMethodEntry {
            raw: im, focused_text_input: None, keyboard_grab: None, _listeners: vec![commit, destroy] });
    }
}
```
Destroy handlers recover their key from `l` (`bound_of(l)` → the `Registration`'s own addr is `l as usize`); remove the entry, and in `on_text_input_destroy` clear the IME's `focused_text_input` if it equals this key. (The address a destroy handler is handed *is* its own listener addr, since the key was `destroy.listener_addr()`; assert this matches `l as usize` the way `on_pointer_constraint_destroy` does at `:4686`.)

Boot registration — in the regs block beside `:2056`:
```rust
if let Some(manager) = runtime.text_input_manager_ptr() {
    regs.push(unsafe { Registration::link_bare(
        &raw mut (*manager.as_ptr()).events.new_text_input, on_new_text_input::<S>,
        (session as *const Session<'_, S>).cast::<()>(), std::ptr::null()) });
}
if let Some(manager) = runtime.input_method_manager_ptr() {
    regs.push(unsafe { Registration::link_bare(
        &raw mut (*manager.as_ptr()).events.new_input_method, on_new_input_method::<S>,
        (session as *const Session<'_, S>).cast::<()>(), std::ptr::null()) });
}
```
Add empty-body stubs for `on_text_input_enable/commit/disable` and `on_input_method_commit` (filled in A4-A7) so this compiles.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wlr --test input_method`
Expected: PASS.

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/backend.rs crates/wlr/src/runtime.rs crates/wlr/tests/input_method.rs
git commit -m "feat(wlr): new-text-input/new-input-method listeners + second-IME refused (A6.1)"
```

---

### Task A3: keyboard-focus relay helper — enter on incoming, leave+deactivate on outgoing

**Files:**
- Modify: `crates/wlr/src/runtime.rs` — add `RuntimeInner::relay_keyboard_focus`; call it from `focus_toplevel_keyboard` (`:8262`), `focus_layer_keyboard` (~`:8041`), `focus_xwayland_surface_keyboard` (~`:6716`, under `#[cfg(wlr_has_xwayland)]`), and `clear_keyboard_focus` (`:8320`). Do **not** hook `focus_lock_surface_keyboard`.
- Test: Part B test 2 (the non-vacuous enter/leave gate) is the real gate; here add a crate-level compile/smoke assertion only.

**Interfaces:**
- Produces: `pub(crate) fn relay_keyboard_focus(&self, new_surface: *mut sys::wlr_surface)` on `Runtime` (reads `self.inner.text_inputs` + `self.inner.input_method`). Called with the incoming focused surface (or `null` for clear). It computes the outgoing surface from the seat's current `keyboard_state.focused_surface` **before** the `notify_enter`/`notify_clear` call at each site.
- Consumes: `sys::wlr_text_input_v3_send_{enter,leave}`, `sys::wlr_input_method_v2_send_{deactivate,done}`, `sys::wl_resource_get_client`, the seat ptr (`self.inner.seat`).

- [ ] **Step 1 (Opus): Implement the helper**

```rust
/// Drive text-input enter/leave off a keyboard-focus change (decision #4).
/// `new_surface` is the surface about to gain keyboard focus, or null on clear.
/// Called from each keyboard-focus mutator, *before* it notifies the seat, so
/// the seat's current `focused_surface` is still the outgoing one.
pub(crate) fn relay_keyboard_focus(&self, new_surface: *mut sys::wlr_surface) {
    let Some(seat) = *self.inner.seat.borrow() else { return };
    // SAFETY: seat is live for the runtime's lifetime.
    let old_surface = unsafe { (*seat.as_ptr()).keyboard_state.focused_surface };
    if old_surface == new_surface { return; }
    let tis = self.inner.text_inputs.borrow();
    // Outgoing: leave every text-input whose client owns the outgoing surface,
    // and deactivate the IME if it was activated for one of them.
    if !old_surface.is_null() {
        let old_client = unsafe { sys::wl_surface_get_client_or(old_surface) }; // helper: wl_resource_get_client((*surface).resource)
        for (key, ti) in tis.iter() {
            if ti.client == old_client {
                unsafe { sys::wlr_text_input_v3_send_leave(ti.raw.as_ptr()); }
                let mut im = self.inner.input_method.borrow_mut();
                if let Some(entry) = im.as_mut() {
                    if entry.focused_text_input == Some(*key) {
                        unsafe {
                            sys::wlr_input_method_v2_send_deactivate(entry.raw.as_ptr());
                            sys::wlr_input_method_v2_send_done(entry.raw.as_ptr());
                        }
                        entry.focused_text_input = None;
                    }
                }
            }
        }
    }
    // Incoming: enter every text-input whose client owns the new surface.
    // Activation waits for that text-input's own `enable` (decision #5), so no
    // send_activate here.
    if !new_surface.is_null() {
        let new_client = unsafe { sys::wl_surface_get_client_or(new_surface) };
        for ti in tis.values() {
            if ti.client == new_client {
                unsafe { sys::wlr_text_input_v3_send_enter(ti.raw.as_ptr(), new_surface); }
            }
        }
    }
}
```
Add a small `unsafe fn wl_surface_get_client_or(s) -> *mut wl_client { sys::wl_resource_get_client((*s).resource) }` if the crate has no existing surface→client helper; check `backend.rs` for one first (the seat keyboard-enter path already does this lookup — reuse it).

At each of the four sites, insert `self.relay_keyboard_focus(surface)` for the focus paths and `self.relay_keyboard_focus(std::ptr::null_mut())` in `clear_keyboard_focus`, positioned **immediately before** the existing `wlr_seat_keyboard_notify_enter`/`notify_clear_focus` call. In `focus_toplevel_keyboard` the incoming `surface` is already computed at `:8283`; pass it. Guard the xwayland site with `#[cfg(wlr_has_xwayland)]` (that whole method already is).

- [ ] **Step 2 (Opus): Smoke test compiles + no-client no-op**

Extend `input_method.rs`:
```rust
#[test]
fn relay_focus_is_a_noop_with_no_text_inputs() {
    let (rt, _d) = wlr::test_support::headless_runtime();
    rt.clear_keyboard_focus(); // must not panic with empty tables
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p wlr --test input_method` — Expected: PASS.

- [ ] **Step 4 (Sonnet): Commit**

```bash
git add crates/wlr/src/runtime.rs crates/wlr/tests/input_method.rs
git commit -m "feat(wlr): enter/leave text-input on keyboard-focus change (A6.1, decision #4)"
```

---

### Task A4: `on_text_input_enable` → activate + surrounding-text + done

**Files:** Modify `crates/wlr/src/backend.rs` (replace the A2 stub). Behavioural gate is Part B test 3.

**Interfaces:**
- Produces: `on_text_input_enable::<S>` — if this text-input's `focused_surface` currently holds keyboard focus and an IME is tracked: set `InputMethodEntry.focused_text_input` to this entry's key, `wlr_input_method_v2_send_activate`, then forward `current` state and `send_done`.
- Consumes: `sys::wlr_input_method_v2_send_{activate,surrounding_text,content_type,done}`; the text-input `current` state fields (`(*ti).current.surrounding.{text,cursor,anchor}`, `(*ti).current.content_type.{hint,purpose}`, `(*ti).current.features`); `(*ti).focused_surface` vs `(*seat).keyboard_state.focused_surface`.

- [ ] **Step 1 (Opus): Implement**

```rust
unsafe extern "C" fn on_text_input_enable<S: Handlers>(l: *mut sys::wl_listener, _data: *mut std::ffi::c_void) {
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let rt = (*session).runtime;
        // Find this text-input's entry by matching the `enable` listener's session
        // key. Simplest: iterate text_inputs and match on the raw ptr recovered
        // from container_of on `l`, OR store the key on the bound. Match sway:
        // resolve the wlr_text_input_v3 from the listener via container_of.
        let ti = wlr_text_input_from_enable_listener(l); // container_of!(l, wlr_text_input_v3, events.enable)
        let Some(seat) = rt.seat_ptr() else { return };
        if (*ti).focused_surface != (*seat.as_ptr()).keyboard_state.focused_surface { return; }
        let mut im = rt.inner.input_method.borrow_mut();
        let Some(entry) = im.as_mut() else { return };
        // Record which text-input is driving activation (its text_inputs key).
        entry.focused_text_input = rt.text_input_key_for(ti);
        sys::wlr_input_method_v2_send_activate(entry.raw.as_ptr());
        forward_text_input_state_to_ime(&*ti, entry.raw.as_ptr()); // surrounding + content_type per features
        sys::wlr_input_method_v2_send_done(entry.raw.as_ptr());
    }
}
```
Add helpers: `container_of!` is already in `wlr-sys` (see `RELEASING.md` "hand-written surface" list) — use it to recover the `wlr_text_input_v3` from the `events.enable`/`commit`/`disable` listener. `Runtime::text_input_key_for(ti) -> Option<usize>` scans `text_inputs` for the entry whose `raw.as_ptr() == ti`. `forward_text_input_state_to_ime(ti, im)` sends `surrounding_text` (when `features & WLR_TEXT_INPUT_V3_FEATURE_SURROUNDING_TEXT`), `content_type` (when `…_FEATURE_CONTENT_TYPE`), reading `ti.current.*`.

- [ ] **Step 2-4:** No new crate-level unit test (needs two real clients — Part B test 3). Run `cargo test -p wlr` + `cargo clippy -p wlr --all-targets -- -D warnings` to prove it compiles and the existing suite is green.

Run: `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings`
Expected: PASS / no warnings.

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/backend.rs
git commit -m "feat(wlr): text-input enable -> IME activate + surrounding-text (A6.1, decision #5)"
```

---

### Task A5: `on_text_input_commit` → re-forward diffed surrounding/content/cause

**Files:** Modify `crates/wlr/src/backend.rs`.

**Interfaces:**
- Produces: `on_text_input_commit::<S>` — if this text-input is the IME's `focused_text_input`, forward `current` state (`send_surrounding_text`/`send_content_type`/`send_text_change_cause` as populated by `current.features`/`current.text_change_cause`) then `send_done`.
- Consumes: same sends as A4 plus `sys::wlr_input_method_v2_send_text_change_cause`.

- [ ] **Step 1 (Opus): Implement**

```rust
unsafe extern "C" fn on_text_input_commit<S: Handlers>(l: *mut sys::wl_listener, _d: *mut std::ffi::c_void) {
    unsafe {
        let session = (*bound_of(l)).session.cast::<Session<'_, S>>();
        let rt = (*session).runtime;
        let ti = wlr_text_input_from_commit_listener(l); // container_of!(l, wlr_text_input_v3, events.commit)
        let mut im = rt.inner.input_method.borrow_mut();
        let Some(entry) = im.as_mut() else { return };
        if entry.focused_text_input != rt.text_input_key_for(ti) { return; }
        forward_text_input_state_to_ime(&*ti, entry.raw.as_ptr());
        if (*ti).current.text_change_cause != 0 {
            sys::wlr_input_method_v2_send_text_change_cause(entry.raw.as_ptr(), (*ti).current.text_change_cause);
        }
        sys::wlr_input_method_v2_send_done(entry.raw.as_ptr());
    }
}
```

- [ ] **Step 2-4:** Run `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings` — Expected: PASS. (Behavioural gate: Part B test 3 asserts surrounding-text after a commit.)

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/backend.rs
git commit -m "feat(wlr): text-input commit re-forwards surrounding/content to IME (A6.1)"
```

---

### Task A6: `on_text_input_disable` → deactivate the IME

**Files:** Modify `crates/wlr/src/backend.rs`.

**Interfaces:**
- Produces: `on_text_input_disable::<S>` — if this text-input is the IME's `focused_text_input`, `wlr_input_method_v2_send_deactivate` + `send_done`, clear `focused_text_input`.

- [ ] **Step 1 (Opus): Implement**

```rust
unsafe extern "C" fn on_text_input_disable<S: Handlers>(l: *mut sys::wl_listener, _d: *mut std::ffi::c_void) {
    unsafe {
        let session = (*bound_of(l)).session.cast::<Session<'_, S>>();
        let rt = (*session).runtime;
        let ti = wlr_text_input_from_disable_listener(l);
        let mut im = rt.inner.input_method.borrow_mut();
        let Some(entry) = im.as_mut() else { return };
        if entry.focused_text_input != rt.text_input_key_for(ti) { return; }
        sys::wlr_input_method_v2_send_deactivate(entry.raw.as_ptr());
        sys::wlr_input_method_v2_send_done(entry.raw.as_ptr());
        entry.focused_text_input = None;
    }
}
```

- [ ] **Step 2-4:** Run `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings` — Expected: PASS. (Behavioural gate: Part B test 5.)

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/backend.rs
git commit -m "feat(wlr): text-input disable deactivates the IME (A6.1)"
```

---

### Task A7: `on_input_method_commit` → relay preedit/commit/delete back to the app

**Files:** Modify `crates/wlr/src/backend.rs`.

**Interfaces:**
- Produces: `on_input_method_commit::<S>` — relay the IME's `current` state to the text-input named by `focused_text_input` (no-op if `None`): `wlr_text_input_v3_send_preedit_string`, `_send_commit_string`, `_send_delete_surrounding_text` as populated, then `_send_done`.
- Consumes: `sys::wlr_text_input_v3_send_{preedit_string,commit_string,delete_surrounding_text,done}`; the IME `current` state (`(*im).current.preedit.{text,cursor_begin,cursor_end}`, `(*im).current.commit_text`, `(*im).current.delete.{before_length,after_length}`).

- [ ] **Step 1 (Opus): Implement**

```rust
unsafe extern "C" fn on_input_method_commit<S: Handlers>(l: *mut sys::wl_listener, _d: *mut std::ffi::c_void) {
    unsafe {
        let session = (*bound_of(l)).session.cast::<Session<'_, S>>();
        let rt = (*session).runtime;
        let im = rt.inner.input_method.borrow();
        let Some(entry) = im.as_ref() else { return };
        let Some(key) = entry.focused_text_input else { return }; // IME committed with nothing focused
        let tis = rt.inner.text_inputs.borrow();
        let Some(ti) = tis.get(&key) else { return };
        let cur = &(*entry.raw.as_ptr()).current;
        if !cur.preedit.text.is_null() {
            sys::wlr_text_input_v3_send_preedit_string(ti.raw.as_ptr(), cur.preedit.text, cur.preedit.cursor_begin, cur.preedit.cursor_end);
        }
        if !cur.commit_text.is_null() {
            sys::wlr_text_input_v3_send_commit_string(ti.raw.as_ptr(), cur.commit_text);
        }
        if cur.delete.before_length != 0 || cur.delete.after_length != 0 {
            sys::wlr_text_input_v3_send_delete_surrounding_text(ti.raw.as_ptr(), cur.delete.before_length, cur.delete.after_length);
        }
        sys::wlr_text_input_v3_send_done(ti.raw.as_ptr(), (*ti.raw.as_ptr()).current_serial);
    }
}
```
(Confirm the `send_done` arity — `wlr_text_input_v3_send_done(text_input, serial)` — against the sys signature before wiring; the serial is the text-input's `current_serial`.)

- [ ] **Step 2-4:** Run `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings` — Expected: PASS. (Behavioural gate: Part B test 4.)

- [ ] **Step 5 (Sonnet): Commit**

```bash
git add crates/wlr/src/backend.rs
git commit -m "feat(wlr): IME commit relays preedit/commit/delete to the app (A6.1)"
```

---

### Task A8: A6.1 release — coverage moves, version bump 0.20.30, CHANGELOG, gates, **CONSENT STOP publish**

**Files:**
- Modify: `crates/wlr/coverage/waived.toml` (move wrapped symbols out), `crates/wlr/coverage/wrapped.toml` (add rows), `crates/wlr/README.md` (CHANGELOG section — the CHANGELOG lives in the crate README), `crates/wlr-sys/build.rs` / `crates/wlr-sys/Cargo.toml` / `crates/wlr/Cargo.toml` (version per `docs/RELEASING.md` checklist — note the minor stays 0.20, only the patch moves to `.30`).

**Interfaces:** none (release mechanics).

- [ ] **Step 1 (Opus): Move coverage rows.** For every symbol this part now wraps — `wlr_text_input_manager_v3`, `wlr_text_input_manager_v3_create`, `wlr_text_input_v3`, `wlr_text_input_v3_features`, `wlr_text_input_v3_send_{enter,leave,preedit_string,commit_string,delete_surrounding_text,done}`, `wlr_input_method_manager_v2`, `wlr_input_method_manager_v2_create`, `wlr_input_method_v2`, `wlr_input_method_v2_state`, `wlr_input_method_v2_preedit_string`, `wlr_input_method_v2_delete_surrounding_text`, `wlr_input_method_v2_send_{activate,deactivate,surrounding_text,content_type,text_change_cause,done,unavailable}` — delete its `[[waived]]` block in `waived.toml` and add a `[[wrapped]]` row in `wrapped.toml` (`symbol` + `module = "runtime"` or `"backend"` + `item` = the public/`pub(crate)` item that reaches it). Leave A6.2-only symbols (`wlr_input_popup_surface_v2*`, `wlr_input_method_keyboard_grab_v2*`, `new_popup_surface`/`grab_keyboard` paths) waived, but re-point their `milestone = "M8"` note to this milestone's A6.2.

- [ ] **Step 2: Run the coverage audit.** Run: `cargo test -p wlr --test coverage_audit` — Expected: PASS (every wrapped symbol referenced under `crates/wlr/src`; no orphan wrapped rows).

- [ ] **Step 3 (Sonnet): Bump the version** to `0.20.30` following `docs/RELEASING.md` (patch bump only; `WLROOTS_MINOR`/`WLROOTS_NEXT_MINOR` unchanged since wlroots minor is unchanged; `tests/link.rs` self-enforces). Write the `0.20.30` CHANGELOG section in `crates/wlr/README.md` in the tone of the `0.20.29` entry (`git show 4a63011`): what it adds (`Runtime::create_text_input_manager`/`create_input_method_manager`, the relay), why it is additive (no trait change in A6.1 — the relay is crate-internal), and the coverage moves.

- [ ] **Step 4: All wlroots-sys gates green on the freeze commit.**

Run:
```bash
cargo test -p wlr && cargo test -p wlr-sys \
  && cargo clippy --all-targets -- -D warnings \
  && RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps \
  && cargo fmt --all --check \
  && cargo test -p wlr --test coverage_audit
```
Expected: all PASS.

- [ ] **Step 5 (Sonnet): Commit the release.**

```bash
git add -A
git commit -m "release(wlr): 0.20.30 — input-method/text-input core relay (A6.1)"
```

- [ ] **Step 6: SendMessage-coordinate, then open the wlroots-sys PR.** Before any push to `develop`, `SendMessage` any parallel wlroots-sys session (shared repo). Then `git flow feature finish` is **deferred** — push `feature/wlr-a6.1` and open a PR into `develop`; CI (test/miri/msrv/fmt) must be green.

- [ ] **Step 7: 🛑 CONSENT STOP — do not `cargo publish` without owner approval.** Present the API-review verdict + green gates and ask for go/no-go. On approval: `cargo package -p wlr-sys && cargo publish -p wlr-sys` (if the sys crate changed — it did not here, so this is a no-op/skip), then `cargo package -p wlr` (watch for `Compiling wlr-sys 0.20.x` **without** a path) and `cargo publish -p wlr`. (Per personal-profile rule + spec: publish is a consent stop; the merge-window rule also still applies to the eventual `git flow finish`.)

---

# PART B — icedtea compositor globals + oracle + harness doubles + tests (A6.1, consumes wlr 0.20.30)

Branch `feature/pure-rust-gtk-m6-1-compositor` (already checked out in the worktree). All paths under `/home/joseph/Projects/icedtea/.worktrees/m6-1-compositor`. Harness/compositor tests run `--test-threads=1`.

---

### Task B0: `[patch.crates-io]` the wlr crate to the wlroots-sys worktree (dev-time)

**Files:** Modify root `Cargo.toml`.

- [ ] **Step 1 (Sonnet): Add the patch.** Append to `Cargo.toml`:
```toml
[patch.crates-io]
wlr = { path = "/home/joseph/Projects/wlroots-sys/crates/wlr" }
```
(Resolves to whatever branch is checked out in the shared wlroots-sys repo — `feature/wlr-a6.1` during A6.1 dev. This is dev-only; it is dropped in Task B-FINAL after the published pin bump.)

- [ ] **Step 2: Prove it resolves.** Run: `cargo build -p icedtea-compositor` — Expected: builds against the worktree `wlr` (the new `create_*_manager` fns are visible).

- [ ] **Step 3 (Sonnet): Commit.**
```bash
git add Cargo.toml Cargo.lock
git commit -m "build(m6.1): [patch] wlr to the wlroots-sys A6.1 worktree (dev-only)"
```

---

### Task B1: create the two globals at boot (both boot paths) — test 1 (globals advertised)

**Files:**
- Modify: `compositor/src/lib.rs` (globals block; insert after `create_relative_pointer_manager`, ~`:128`), `harness/src/lib.rs` (the duplicated boot path, insert beside `create_relative_pointer_manager` at `:342`).
- Test: `compositor/tests/client_protocol.rs`.

**Interfaces:**
- Consumes: `Runtime::create_text_input_manager` / `create_input_method_manager` (Part A).
- Produces: `zwp_text_input_manager_v3` + `zwp_input_method_manager_v2` in `advertised_globals`.

- [ ] **Step 1 (Sonnet): Write the failing test** in `client_protocol.rs`, mirroring the pointer-constraints advertised-globals test at `:196-210`:
```rust
#[test]
fn text_input_and_input_method_globals_are_advertised() {
    let comp = Compositor::spawn();
    let globals = comp.advertised_globals();
    assert!(globals.iter().any(|g| g == "zwp_text_input_manager_v3"), "text-input global missing");
    assert!(globals.iter().any(|g| g == "zwp_input_method_manager_v2"), "input-method global missing");
}
```

- [ ] **Step 2: Run to verify it fails.** Run: `cargo test -p icedtea-compositor --test client_protocol text_input_and_input_method_globals_are_advertised -- --test-threads=1` — Expected: FAIL (globals absent).

- [ ] **Step 3 (Sonnet): Create the globals** in `compositor/src/lib.rs` (same non-fatal tone as neighbours):
```rust
// Lets external IMEs (fcitx5/ibus) and on-screen keyboards (squeekboard) relay
// composition to apps' text fields. Non-fatal: without them, IME/OSK clients
// simply cannot attach and apps fall back to raw key input.
if let Err(err) = runtime.create_text_input_manager(&display) {
    tracing::error!(%err, "text-input (IME app side) is unavailable");
}
if let Err(err) = runtime.create_input_method_manager(&display) {
    tracing::error!(%err, "input-method (IME/OSK side) is unavailable");
}
```
Add the identical two calls to the harness boot path (`harness/src/lib.rs`, beside `:342`) with `.expect(...)` matching the harness's surrounding style (the harness `expect`s its creates, e.g. `:339`).

- [ ] **Step 4: Run to verify it passes.** Run: `cargo test -p icedtea-compositor --test client_protocol text_input_and_input_method_globals_are_advertised -- --test-threads=1` — Expected: PASS.

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add compositor/src/lib.rs harness/src/lib.rs
git commit -m "feat(compositor): advertise text-input-v3 + input-method-v2 globals (M6.1 test 1)"
```

---

### Task B2: `DbCommand::InputMethodActive` oracle + Runtime accessor + harness method — test 7

**Files:**
- Modify: `crates/wlr/src/runtime.rs` (Part A worktree) — add `pub fn input_method_active(&self) -> bool`; move its symbol coverage row. Then `compositor/src/dbus.rs` (`DbCommand` enum, beside `SessionLocked` at `:148`), `compositor/src/state.rs` (handler, beside the `SessionLocked` arm at `:4508`), `harness/src/lib.rs` (a `input_method_active()` accessor beside `session_locked()` at `:623`).
- Test: exercised by tests 3/5/6 (Task B5); a direct assertion here.

**Interfaces:**
- Produces: `Runtime::input_method_active() -> bool` = `self.inner.input_method.borrow().as_ref().map_or(false, |e| e.focused_text_input.is_some())`; `DbCommand::InputMethodActive { reply: Sender<bool> }`; `Compositor::input_method_active(&self) -> bool` in the harness.
- Consumes: `State::wayland.runtime()` (`state.rs:4510`).

- [ ] **Step 1 (Sonnet): Add the crate accessor** in the wlroots-sys worktree `runtime.rs`:
```rust
/// Whether the tracked input-method is currently activated for a focused,
/// enabled text-input. The test oracle for the relay (decision-#5 activation);
/// mirrors `is_session_locked`'s read-only shape.
pub fn input_method_active(&self) -> bool {
    self.inner.input_method.borrow().as_ref().is_some_and(|e| e.focused_text_input.is_some())
}
```
Move `wlr_input_method_v2` remains wrapped (already moved in A8); no new coverage row needed if the accessor reuses an already-wrapped symbol. Commit in the wlroots-sys repo as a fixup to `feature/wlr-a6.1` **before** the A8 freeze (or squash into A1) — this accessor must ship in 0.20.30. (If A8 already froze, this is a 0.20.31-or-patch concern; schedule the accessor into A1 so it lands in 0.20.30.)

- [ ] **Step 2 (Sonnet): Write the failing harness plumbing + test.** In `compositor/src/dbus.rs` (beside `SessionLocked`):
```rust
/// Test-only: read `wlr::Runtime::input_method_active` via the runtime handle.
/// Same reasoning as `SessionLocked` — only the harness sends this.
InputMethodActive { reply: Sender<bool> },
```
In `compositor/src/state.rs` (beside the `SessionLocked` arm):
```rust
DbCommand::InputMethodActive { reply } => {
    let active = self.wayland.runtime().map(|rt| rt.input_method_active()).unwrap_or(false);
    let _ = reply.send(active);
    return Some(());
}
```
In `harness/src/lib.rs` (beside `session_locked`):
```rust
/// Whether an IME is currently activated for a focused+enabled text-input,
/// via `wlr::Runtime::input_method_active`. Blocks on the reply.
pub fn input_method_active(&self) -> bool {
    let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
    self.send(DbCommand::InputMethodActive { reply: reply_tx });
    reply_rx.recv_timeout(TIMEOUT).expect("compositor never answered InputMethodActive")
}
```

- [ ] **Step 3: Compile.** Run: `cargo build -p icedtea-compositor -p icedtea-harness` — Expected: builds. (The behavioural true/false transitions are asserted in Task B5 tests 3/5/6.)

- [ ] **Step 4 (Sonnet): Commit.**
```bash
git add compositor/src/dbus.rs compositor/src/state.rs harness/src/lib.rs
git commit -m "feat(harness): DbCommand::InputMethodActive oracle (M6.1 test 7)"
```

---

### Task B3: `TextInputClient` harness double

**Files:** Modify `harness/src/lib.rs` (add the struct + impl beside `PointerConstraintsClient` at `:4688`; also add `zwp_text_input_manager_v3` binding to the client `ClientState` global registry).

**Interfaces:**
- Produces: `pub struct TextInputClient` wrapping a `TestClient::map_toplevel` (so it can gain keyboard focus via the existing auto-focus-on-map path), a `zwp_text_input_v3`, with `enable()`, `commit_with(surrounding: &str, cursor: u32, anchor: u32, cursor_rect: (i32,i32,i32,i32))`, `disable()`, `pump()`, and recorded-event accessors `entered()`, `left()`, `preedits()`, `commits()`, `deletes()`, `dones()`.
- Consumes: the `TestClient::map_toplevel` (`:2502`) + `wait_until`/`has_input_serial` focus-injection path used at `client_protocol.rs:1641`.

- [ ] **Step 1 (Sonnet): Add the `zwp_text_input_manager_v3` global** to `ClientState` (mirror how `pointer_constraints` is bound in `connect_and_bind`/the registry `Dispatch`). Bring in `wayland_protocols_misc::zwp_text_input_v3::client::{zwp_text_input_manager_v3, zwp_text_input_v3}` (confirm the crate providing text-input-v3 client bindings is on the dep list — `wayland-protocols-misc`; add it to `harness/Cargo.toml` if absent).

- [ ] **Step 2 (Sonnet): Write the double.**
```rust
/// A `zwp_text_input_v3` app double: maps a focusable toplevel (via `TestClient`),
/// creates a text-input on the seat, and records relay events (enter/leave/
/// preedit/commit/delete/done). M6.1 tests 2-5's app side.
pub struct TextInputClient {
    client: TestClient,
    text_input: zwp_text_input_v3::ZwpTextInputV3,
    // event counters/records live in `client.state` (extend ClientState), read via accessors.
}
impl TextInputClient {
    pub fn spawn(socket: &str) -> TextInputClient { /* map_toplevel + get_text_input(seat) + settle */ }
    pub fn enable(&mut self) { self.text_input.enable(); self.text_input.commit(); self.flush(); }
    pub fn commit_with(&mut self, text: &str, cursor: u32, anchor: u32, rect: (i32,i32,i32,i32)) {
        self.text_input.set_surrounding_text(text.into(), cursor as i32, anchor as i32);
        self.text_input.set_cursor_rectangle(rect.0, rect.1, rect.2, rect.3);
        self.text_input.commit(); self.flush();
    }
    pub fn disable(&mut self) { self.text_input.disable(); self.text_input.commit(); self.flush(); }
    pub fn entered(&self) -> u32 { self.client.state.text_input_enters }
    pub fn left(&self) -> u32 { self.client.state.text_input_leaves }
    pub fn commits(&self) -> &[String] { &self.client.state.text_input_commit_strings }
    pub fn preedits(&self) -> &[String] { &self.client.state.text_input_preedit_strings }
    pub fn dones(&self) -> u32 { self.client.state.text_input_dones }
    pub fn pump(&mut self) { /* dispatch_pending on the client's queue */ }
}
```
Record `enter`/`leave`/`preedit_string`/`commit_string`/`delete_surrounding_text`/`done` in the `Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()>` impl on `ClientState` (fields added there). Follow the exact `Dispatch` + counter idiom the existing `wl_keyboard`/`wl_data_device` doubles use.

- [ ] **Step 3: Compile + a smoke test** (map + get enter on focus):
```rust
#[test]
fn text_input_client_enters_on_keyboard_focus() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket); // seat needs a keyboard
    let mut ti = TextInputClient::spawn(&comp.socket);    // maps + auto-focused
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1), "text-input never got enter on focus");
}
```

- [ ] **Step 4: Run.** Run: `cargo test -p icedtea-compositor --test client_protocol text_input_client_enters_on_keyboard_focus -- --test-threads=1` — Expected: PASS.

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add harness/src/lib.rs harness/Cargo.toml
git commit -m "test(harness): TextInputClient double (M6.1)"
```

---

### Task B4: `InputMethodClient` harness double

**Files:** Modify `harness/src/lib.rs` (add beside `TextInputClient`); add `zwp_input_method_manager_v2` to `ClientState`.

**Interfaces:**
- Produces: `pub struct InputMethodClient` — surfaceless (a bare bind, no toplevel), holding a `zwp_input_method_v2`, with `send_commit(preedit: Option<&str>, commit: Option<&str>, delete: Option<(u32,u32)>)`, and recorded-event accessors `activates()`, `deactivates()`, `surroundings()`, `content_types()`, `dones()`, `unavailable()`. (Popup + grab methods are added in Part B/A6.2, Task B7.)
- Consumes: `connect_and_bind` (surfaceless — mirror `DataControlClient` at `:4135`, "never holds focus, a bare `wl_surface` suffices"), the `zwp_input_method_manager_v2` client binding.

- [ ] **Step 1 (Sonnet): Bind the manager + write the double.**
```rust
/// A `zwp_input_method_v2` IME/OSK double: binds the manager (surfaceless, like
/// a real IME daemon), creates an input-method, records activate/deactivate/
/// surrounding_text/content_type/done/unavailable, and can send preedit/commit/
/// delete + commit. M6.1 tests 3-6's IME side.
pub struct InputMethodClient { /* conn, queue, state, input_method */ }
impl InputMethodClient {
    pub fn spawn(socket: &str) -> InputMethodClient { /* bind manager + get_input_method(seat) + settle */ }
    pub fn send_commit(&mut self, preedit: Option<&str>, commit: Option<&str>, delete: Option<(u32,u32)>) {
        if let Some(p) = preedit { self.input_method.set_preedit_string(p.into(), 0, p.len() as i32); }
        if let Some(c) = commit { self.input_method.commit_string(c.into()); }
        if let Some((b,a)) = delete { self.input_method.delete_surrounding_text(b, a); }
        self.input_method.commit(self.serial); self.flush();
    }
    pub fn activates(&self) -> u32 { self.state.im_activates }
    pub fn deactivates(&self) -> u32 { self.state.im_deactivates }
    pub fn surroundings(&self) -> &[String] { &self.state.im_surroundings }
    pub fn dones(&self) -> u32 { self.state.im_dones }
    pub fn is_unavailable(&self) -> bool { self.state.im_unavailable }
    pub fn pump(&mut self) { /* dispatch_pending */ }
}
```
Record `activate`/`deactivate`/`surrounding_text`/`content_type`/`done`/`unavailable` in the `Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()>` impl. `commit`'s `serial` arg is the count of `done` events received (the protocol's serial). (Client bindings: `wayland_protocols_misc::zwp_input_method_v2::client` — confirm/add the dep.)

- [ ] **Step 2: Compile + smoke test** (bind succeeds, no activation yet):
```rust
#[test]
fn input_method_client_binds_without_activation() {
    let comp = Compositor::spawn();
    let im = InputMethodClient::spawn(&comp.socket);
    assert!(!comp.input_method_active(), "no text-input enabled yet");
    assert_eq!(im.activates(), 0);
}
```

- [ ] **Step 3: Run.** Run: `cargo test -p icedtea-compositor --test client_protocol input_method_client_binds_without_activation -- --test-threads=1` — Expected: PASS.

- [ ] **Step 4 (Sonnet): Commit.**
```bash
git add harness/src/lib.rs harness/Cargo.toml
git commit -m "test(harness): InputMethodClient double (M6.1)"
```

---

### Task B5: A6.1 behavioural tests 2-6 (relay both directions, both sides asserted)

**Files:** Modify `compositor/tests/client_protocol.rs`.

**Interfaces:** Consumes `TextInputClient` (B3), `InputMethodClient` (B4), `VirtualKeyboardClient`, `VirtualPointerClient`, `Compositor::input_method_active` (B2). Each test asserts on **both** the client-side event capture **and** the `InputMethodActive` oracle (the M4.4 "assert on both sides" discipline).

- [ ] **Step 1 (Opus): Test 2 — enter/leave follows keyboard focus, not pointer focus (non-vacuous).**
```rust
#[test]
fn text_input_enter_follows_keyboard_focus_not_pointer() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    // A second surface takes pointer focus; the text-input surface does not.
    let mut other = TestClient::map_toplevel(&comp.socket, "other", "other");
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket); // maps last -> keyboard-focused
    // Non-vacuous half: move pointer over `other`, assert enter did NOT fire from pointer-only presence.
    // ... (pointer over other) ...
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1), "enter must follow keyboard focus");
    // Move keyboard focus away (map+focus `other` again); assert leave.
    let mut third = TestClient::map_toplevel(&comp.socket, "third", "third");
    assert!(ti.client_wait_until(|c| c.text_input_leaves >= 1), "leave must fire on keyboard-focus loss");
}
```
Run + assert PASS. Commit.

- [ ] **Step 2 (Opus): Test 3 — enable → activate → surrounding-text relay.**
```rust
#[test]
fn enable_activates_ime_and_relays_surrounding_text() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket); // IME bound first
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1));
    ti.enable();
    ti.commit_with("hello", 5, 5, (10, 20, 2, 16));
    assert!(im.wait_until(|s| s.im_activates >= 1), "IME never activated");
    assert!(im.wait_until(|s| s.im_surroundings.last().map(String::as_str) == Some("hello")));
    assert!(comp.input_method_active(), "oracle: IME must be active"); // both sides
}
```
Run + PASS. Commit.

- [ ] **Step 3 (Opus): Test 4 — IME commit → app relay.**
```rust
#[test]
fn ime_commit_relays_preedit_and_commit_to_app() {
    // ... spawn vk, im, ti; enter; enable; commit_with ...
    im.send_commit(Some("に"), Some("日本"), None);
    assert!(ti.client_wait_until(|c| c.text_input_preedit_strings.last().map(String::as_str) == Some("に")));
    assert!(ti.client_wait_until(|c| c.text_input_commit_strings.last().map(String::as_str) == Some("日本")));
}
```
Run + PASS. Commit.

- [ ] **Step 4 (Opus): Test 5 — disable deactivates; focus-away leaves.**
```rust
#[test]
fn disable_deactivates_and_focus_away_leaves() {
    // ... enter + enable so active ...
    assert!(comp.input_method_active());
    ti.disable();
    assert!(im.wait_until(|s| s.im_deactivates >= 1), "IME never deactivated on disable");
    assert!(!comp.input_method_active(), "oracle: no longer active");
    let _third = TestClient::map_toplevel(&comp.socket, "third", "third"); // move keyboard focus away
    assert!(ti.client_wait_until(|c| c.text_input_leaves >= 1), "text-input never left on focus loss");
}
```
Run + PASS. Commit.

- [ ] **Step 5 (Opus): Test 6 — second input-method refused.**
```rust
#[test]
fn second_input_method_is_refused() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut first = InputMethodClient::spawn(&comp.socket);
    let second = InputMethodClient::spawn(&comp.socket);
    assert!(second.wait_until(|s| s.im_unavailable), "second IME must get `unavailable`");
    // Even when a text-input enables, only the first ever activates.
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1));
    ti.enable(); ti.commit_with("x", 1, 1, (0,0,1,1));
    assert!(first.wait_until(|s| s.im_activates >= 1), "the first IME activates");
    assert_eq!(second.activates(), 0, "the refused IME never activates");
}
```
Run + PASS. Commit each test separately:
```bash
git add compositor/tests/client_protocol.rs
git commit -m "test(compositor): M6.1 A6.1 relay tests 2-6 (both-sides asserted)"
```

- [ ] **Step 6: Full A6.1 icedtea gate.** Run: `cargo test --workspace -- --test-threads=1 && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check` — Expected: all PASS.

---

### Task B-FINAL (A6.1): bump published pin, drop the `[patch]`

**Files:** Modify root `Cargo.toml`, `compositor/Cargo.toml` (`:36`, `:69`), `harness/Cargo.toml` (`:18`).

- [ ] **Step 1:** Only after wlr 0.20.30 is **published** (Task A8 consent stop cleared). Bump every `wlr = "0.20.29"` to `wlr = "0.20.30"` and **delete** the `[patch.crates-io]` block from root `Cargo.toml`.

- [ ] **Step 2:** Run: `cargo update -p wlr && cargo test --workspace -- --test-threads=1 && cargo clippy --all-targets -- -D warnings` — Expected: resolves 0.20.30 from crates.io (no path), all green.

- [ ] **Step 3 (Sonnet): Commit.**
```bash
git add Cargo.toml Cargo.lock compositor/Cargo.toml harness/Cargo.toml
git commit -m "build(m6.1): consume published wlr 0.20.30, drop dev [patch] (A6.1)"
```

- [ ] **Step 4: Whole-branch Opus review, then merge window.** Request a whole-branch `/code-review` (Opus). Then **present the branch as ready and STOP for the merge window** — do not run `git flow feature finish`. On the owner's go: `git flow feature finish --keepremote pure-rust-gtk-m6-1-compositor` (per personal-profile git-flow rule), push `develop`, and let the PR read Merged. (A6.1 and A6.2 may share the one icedtea feature branch, finishing once after A6.2 — see Part B/A6.2 close-out; if A6.2 is deferred, finish here.)

---

# PART A — the `wlr` crate relay (wlroots-sys), release 0.20.31 (A6.2: popups + keyboard-grab)

Branch `feature/wlr-a6.2` off `develop` (after 0.20.30 is published and merged). Layered on A6.1. Opus for the grab branch + popup exposure; Sonnet for the handler-trait boilerplate.

---

### Task A9: expose popup + keyboard-grab objects, listeners, and `InputMethodEntry` fields

**Files:** Modify `crates/wlr/src/runtime.rs` (add `input_method_popups: RefCell<HashMap<usize, PopupEntry>>` to `RuntimeInner`; add `popup`/`grab` fields already present on `InputMethodEntry` from A1), `crates/wlr/src/backend.rs` (register `new_popup_surface`/`grab_keyboard` listeners in `on_new_input_method` from A2; add `on_input_method_new_popup_surface`, `on_input_method_popup_destroy`, `on_input_method_grab_keyboard`, `on_input_method_grab_destroy`).

**Interfaces:**
- Produces:
  - `struct PopupEntry { raw: NonNull<sys::wlr_input_popup_surface_v2>, node: Option<NodeId>, _destroy: Registration }`.
  - `pub struct InputPopupSurfaceId(usize)` — the crate handle the compositor uses (the `input_method_popups` key).
  - `on_input_method_new_popup_surface::<S>` — inserts a `PopupEntry` keyed by its destroy-listener addr, then dispatches `Event::InputMethodPopupCreated(InputPopupSurfaceId)` to the handler (A12).
  - `on_input_method_grab_keyboard::<S>` — stores the grab `NonNull` on `InputMethodEntry.keyboard_grab`, calls `wlr_input_method_keyboard_grab_v2_set_keyboard` with the seat's active keyboard (mirror the `wlr_seat_get_keyboard` at `backend.rs:7122`), registers its destroy listener (clears the field).
- Consumes: A2's `on_new_input_method` (extend its `_listeners` vec with the two new listeners).

- [ ] **Step 1 (Sonnet): Write the failing test** (grab-keyboard set + popup created dispatch, driven from Part B; here a crate compile smoke): add fields, extend `on_new_input_method` to link `new_popup_surface`/`grab_keyboard`.

- [ ] **Step 2-4 (Opus): Implement** the four handlers + registration; run `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings`. Expected PASS. (Behavioural gates: Part B tests 8-9.)

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add crates/wlr/src/runtime.rs crates/wlr/src/backend.rs
git commit -m "feat(wlr): input-method popup + keyboard-grab objects & listeners (A6.2)"
```

---

### Task A10: popup scene-node creator + `send_input_popup_rectangle` + rectangle accessor

**Files:** Modify `crates/wlr/src/runtime.rs`.

**Interfaces:**
- Produces:
  - `pub fn add_input_popup_in_band(&self, popup: InputPopupSurfaceId, band: Band) -> Option<NodeId>` — wraps `sys::wlr_scene_subsurface_tree_create` over the popup's `surface`, attaches a `NodeId` (reuse the `attach_node`/`NodeEntry` machinery at `runtime.rs:2821`), stores it on the `PopupEntry`. (See Open Question Q1.)
  - `pub fn send_input_popup_rectangle(&self, popup: InputPopupSurfaceId, rect: Box2D)` — wraps `sys::wlr_input_popup_surface_v2_send_text_input_rectangle`.
  - `pub fn input_popup_surface(&self, popup: InputPopupSurfaceId) -> Option<*mut sys::wlr_surface>` — for the compositor's placement math.
- Consumes: `Band` (`:840`), `set_node_position` (`:3221`), `attach_node`/`node_entry` (`:2821`/`:2876`).

- [ ] **Step 1 (Opus): Implement** the three methods; move `wlr_input_popup_surface_v2`, `_send_text_input_rectangle`, `_try_from_wlr_surface`, `wlr_scene_subsurface_tree_create` (if newly referenced) from `waived.toml` to `wrapped.toml`.

- [ ] **Step 2-4:** Run `cargo test -p wlr --test coverage_audit && cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings` — Expected PASS.

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add crates/wlr/src/runtime.rs crates/wlr/coverage/*.toml
git commit -m "feat(wlr): input-popup scene node + text_input_rectangle send (A6.2, decision #7)"
```

---

### Task A11: `on_key`/`on_modifiers` keyboard-grab branch (the hot path)

**Files:** Modify `crates/wlr/src/backend.rs` — `on_key` (`:7104`, branch before the `wlr_seat_keyboard_notify_key` at `:7165`) and `on_modifiers` (`:7175`, before `:7192`).

**Interfaces:**
- Produces: when `runtime.inner.input_method`'s `keyboard_grab` is `Some`, forward via `wlr_input_method_keyboard_grab_v2_send_key`/`send_modifiers` on the grab **instead of** the seat calls, and skip the seat forward. The `dispatcher.emit(Event::Key…)` at `:7146` runs **unconditionally first** (decision #6 — icedtea keybindings are unaffected); only the trailing forward target changes.
- Consumes: A9's `InputMethodEntry.keyboard_grab`.

- [ ] **Step 1 (Opus): Implement** the branch in `on_key`:
```rust
// (after dispatcher.emit at :7146, replacing the `if !last_key_consumed` seat-forward block)
if !(*session).last_key_consumed.get() {
    let grab = (*session).runtime.inner.input_method.borrow().as_ref().and_then(|e| e.keyboard_grab);
    if let Some(grab) = grab {
        sys::wlr_input_method_keyboard_grab_v2_send_key(grab.as_ptr(), (*ev).time_msec, (*ev).keycode, (*ev).state.0);
    } else {
        sys::wlr_seat_set_keyboard(seat.as_ptr(), kb);
        sys::wlr_seat_keyboard_notify_key(seat.as_ptr(), (*ev).time_msec, (*ev).keycode, (*ev).state.0);
    }
}
```
Mirror in `on_modifiers` (grab → `wlr_input_method_keyboard_grab_v2_send_modifiers(grab, &raw mut (*kb).modifiers)` else the existing `wlr_seat_keyboard_notify_modifiers`). Move the two grab-send symbols + `set_keyboard` waived→wrapped.

- [ ] **Step 2-4:** Run `cargo test -p wlr --test coverage_audit && cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings` — Expected PASS. (Behavioural gates: Part B tests 9 + 10, both non-vacuous.)

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add crates/wlr/src/backend.rs crates/wlr/coverage/*.toml
git commit -m "feat(wlr): keyboard-grab intercepts key/modifiers, dispatcher runs first (A6.2, decision #6)"
```

---

### Task A12: `InputMethodHandler` trait + `Event` variants + dispatch

**Files:** Modify `crates/wlr/src/handler.rs` (new trait, defaulted methods — the `SeatHandler::session_lock_changed` additive precedent at `:1027`), `crates/wlr/src/dispatch.rs` (new `Event` variants beside `SessionLockChanged` at `:203`), `crates/wlr/src/backend.rs` (dispatch arm beside `:2574`).

**Interfaces:**
- Produces:
  - `pub trait InputMethodHandler { fn new_popup_surface(&mut self, popup: InputPopupSurfaceId) {…} fn popup_surface_destroyed(&mut self, popup: InputPopupSurfaceId) {…} }` (all defaulted — additive).
  - `Event::InputMethodPopupCreated(InputPopupSurfaceId)` / `Event::InputMethodPopupDestroyed(InputPopupSurfaceId)`.
  - Requires `S: InputMethodHandler` where the other handler bounds live (confirm the `Handlers` supertrait bundle in `handler.rs`/`dispatch.rs` and add it additively — the bundle is the one thing that is not defaulted, so adding a supertrait bound is the semver-sensitive step; verify an empty `impl` still compiles the way `tests/axis.rs` asserts for `pointer_axis`).
- Consumes: A9's dispatch call.

- [ ] **Step 1 (Sonnet): Write the additive-compat test** (mirror `tests/axis.rs`): an `impl InputMethodHandler for S {}` empty block compiles.

- [ ] **Step 2-4 (Opus): Implement** trait + variants + dispatch arms; run `cargo test -p wlr && cargo clippy -p wlr --all-targets -- -D warnings`. Expected PASS.

- [ ] **Step 5 (Sonnet): Commit.**
```bash
git add crates/wlr/src/handler.rs crates/wlr/src/dispatch.rs crates/wlr/src/backend.rs crates/wlr/tests/
git commit -m "feat(wlr): InputMethodHandler popup callbacks (A6.2, additive)"
```

---

### Task A13: A6.2 release — coverage finalise, version 0.20.31, CHANGELOG, gates, **CONSENT STOP publish**

Identical mechanics to A8, for 0.20.31.

- [ ] **Step 1 (Opus):** Finish moving all remaining A6.2 symbols (`wlr_input_popup_surface_v2*`, `wlr_input_method_keyboard_grab_v2*`) waived→wrapped; run `cargo test -p wlr --test coverage_audit`.
- [ ] **Step 2 (Sonnet):** Bump to `0.20.31` per `docs/RELEASING.md`; write the `0.20.31` CHANGELOG section in `crates/wlr/README.md` — new `InputMethodHandler`, the popup scene node + rectangle, the `on_key` grab branch; note the trait addition is additive (empty-impl test).
- [ ] **Step 3:** All wlroots-sys gates green (as A8 Step 4).
- [ ] **Step 4 (Sonnet):** `git commit -m "release(wlr): 0.20.31 — input-method popups + keyboard grab (A6.2)"`.
- [ ] **Step 5:** SendMessage-coordinate; push `feature/wlr-a6.2`; PR into `develop`; CI green.
- [ ] **Step 6: 🛑 CONSENT STOP — do not `cargo publish` without owner approval.** Same procedure as A8 Step 7 for 0.20.31.

---

# PART B — icedtea popup placement + tests (A6.2, consumes wlr 0.20.31)

Same icedtea feature branch. Opus for popup placement; Opus for the grab tests.

---

### Task B6: re-point `[patch]` to the A6.2 worktree

- [ ] **Step 1 (Sonnet):** With `feature/wlr-a6.2` checked out in wlroots-sys, re-add the `[patch.crates-io] wlr = { path = … }` block (dropped in B-FINAL A6.1) to root `Cargo.toml`. Run `cargo build -p icedtea-compositor` — Expected: builds against the A6.2 worktree (the `InputMethodHandler` + popup methods are visible).
- [ ] **Step 2 (Sonnet):** `git commit -m "build(m6.1): re-[patch] wlr to the A6.2 worktree (dev-only)"`.

---

### Task B7: popup placement — implement `InputMethodHandler` on `State`

**Files:** Create `compositor/src/input_method.rs` (one-module-per-concern, beside `input.rs`); wire it in `compositor/src/lib.rs` (`mod input_method;`) and implement `wlr::InputMethodHandler for State` (State's handler impls live in `state.rs` — add the impl there or in the new module, matching where `SeatHandler for State` lives). Add popup + grab methods to `InputMethodClient` in `harness/src/lib.rs`.

**Interfaces:**
- Consumes: `Runtime::add_input_popup_in_band`, `set_node_position`, `send_input_popup_rectangle`, `input_popup_surface` (A10); the focused text-input's last `cursor_rectangle` — tracked in `State` from the text-input relay (the compositor learns the rect from `DbCommand`/an accessor, OR the crate exposes `Runtime::focused_text_input_cursor_rectangle() -> Option<Box2D>`; **add that accessor in A10** if the compositor cannot otherwise read it — flagged: the compositor needs the anchor rect, which lives in the crate's `text_inputs` `current.cursor_rectangle`).
- Produces: on `new_popup_surface`: create the scene node in `Band::Top`, compute anchor = below the cursor rectangle in output coords, clamp to the primary output's geometry (`State::outputs` min-index, as `OutputSize` does at `state.rs:4547`), `set_node_position`, then `send_input_popup_rectangle`. On `popup_surface_destroyed`: nothing extra (the crate drops the node on destroy) — or clear any compositor-side record.

- [ ] **Step 1 (Opus): Add the crate anchor accessor** (if needed) in the A10 worktree: `pub fn focused_text_input_cursor_rectangle(&self) -> Option<Box2D>` reading the IME's `focused_text_input` → `text_inputs[key].raw.current.cursor_rectangle`. Move its symbol coverage if new. (Land in A10 so it ships in 0.20.31.)

- [ ] **Step 2 (Opus): Implement placement** in `compositor/src/input_method.rs`:
```rust
impl wlr::InputMethodHandler for State {
    fn new_popup_surface(&mut self, popup: wlr::InputPopupSurfaceId) {
        let Some(rt) = self.wayland.runtime() else { return };
        let node = match rt.add_input_popup_in_band(popup, wlr::Band::Top) { Some(n) => n, None => return };
        let anchor = rt.focused_text_input_cursor_rectangle().unwrap_or_default();
        let output = self.outputs.keys().min().and_then(|i| self.outputs.get(i)).map(|o| o.geometry);
        let (x, y) = place_below_clamped(anchor, output); // pure fn in this module, unit-tested
        rt.set_node_position(node, x, y);
        rt.send_input_popup_rectangle(popup, anchor);
    }
}
```
Write `place_below_clamped(anchor: Box2D, output: Option<Rectangle>) -> (i32, i32)` as a **pure** function (anchor below `cursor_rectangle`, clamp so the popup stays on-output) with its own unit tests in the module — the M4.5/`render.rs` "pure geometry, unit-tested" discipline.

- [ ] **Step 3 (Opus): Re-position on commit.** When a subsequent text-input commit changes the cursor rectangle (the crate re-dispatches or the compositor polls on a popup-repositioned event), recompute and `set_node_position`. If the crate does not surface a reposition event, add `InputMethodHandler::popup_repositioned` in A12 — decide during A12 and reconcile. (Minimum viable: place once on creation; A6 says "re-position on every subsequent commit that changes the cursor rectangle" — include a `popup_repositioned` callback.)

- [ ] **Step 4 (Sonnet): Popup + grab methods on `InputMethodClient`** (harness): `create_popup() -> creates zwp_input_popup_surface_v2 with a mapped wl_surface`; `grab_keyboard()`; popup event accessors `popup_text_input_rectangle() -> Option<(i32,i32,i32,i32)>`; grab key/modifier counters.

- [ ] **Step 5: Compile.** Run: `cargo build -p icedtea-compositor -p icedtea-harness && cargo test -p icedtea-compositor input_method::place_below_clamped -- --test-threads=1` — Expected: pure-fn tests PASS.

- [ ] **Step 6 (Sonnet): Commit.**
```bash
git add compositor/src/input_method.rs compositor/src/lib.rs compositor/src/state.rs harness/src/lib.rs
git commit -m "feat(compositor): place IME candidate popups near the cursor rectangle (M6.1 A6.2)"
```

---

### Task B8: DbCommand popup-position oracle

**Files:** Modify `compositor/src/dbus.rs` (variant), `compositor/src/state.rs` (handler), `harness/src/lib.rs` (accessor). Mirror `CursorPosition` exactly.

**Interfaces:** Produces `DbCommand::InputPopupPosition { reply: Sender<Option<(i32,i32)>> }` reading the placed node's scene position (via a `Runtime::node_position(NodeId)`-style accessor or the crate `input_popup` scene position); `Compositor::input_popup_position() -> Option<(i32,i32)>`.

- [ ] **Step 1-3 (Sonnet):** Add the variant + handler + harness accessor (copy `CursorPosition` at `dbus.rs:154` / `state.rs:4517` / `harness:633`). Run `cargo build` — Expected: builds.
- [ ] **Step 4 (Sonnet): Commit.** `git commit -m "feat(harness): DbCommand::InputPopupPosition oracle (M6.1 test 8)"`.

---

### Task B9: A6.2 behavioural tests 8, 9, 10

**Files:** Modify `compositor/tests/client_protocol.rs`.

- [ ] **Step 1 (Opus): Test 8 — popup positioned + told its rectangle.**
```rust
#[test]
fn ime_popup_is_positioned_and_told_its_rectangle() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1));
    ti.enable();
    ti.commit_with("q", 1, 1, (100, 200, 2, 16)); // known cursor rectangle
    im.create_popup();
    assert!(im.wait_until(|s| s.popup_text_input_rectangle == Some((100,200,2,16))), "popup never told its anchor rect");
    let pos = comp.input_popup_position().expect("popup was placed");
    assert!(pos.1 >= 200 + 16, "popup must sit below the cursor rectangle"); // clamped-to-output verified in the pure-fn tests
}
```
Run + PASS. Commit.

- [ ] **Step 2 (Opus): Test 9 — keyboard grab intercepts (non-vacuous both ways).**
```rust
#[test]
fn keyboard_grab_intercepts_physical_keys() {
    let comp = Compositor::spawn();
    let mut vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket); // has a wl_keyboard too
    assert!(ti.client_wait_until(|c| c.text_input_enters >= 1));
    // Non-vacuous baseline: BEFORE grab, a key reaches the app's wl_keyboard.
    vk.key_press(30);
    assert!(ti.client_wait_until(|c| c.wl_keyboard_key_events >= 1), "pre-grab key must reach the surface");
    let pre = ti.wl_keyboard_key_events();
    im.grab_keyboard();
    vk.key_press(31);
    assert!(im.wait_until(|s| s.grab_key_events >= 1), "grab never received the key");
    // The app's normal wl_keyboard must receive NOTHING more.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while std::time::Instant::now() < deadline { vk.pump(); ti.pump(); }
    assert_eq!(ti.wl_keyboard_key_events(), pre, "grabbed key must not reach the surface");
}
```
Run + PASS. Commit.

- [ ] **Step 3 (Opus): Test 10 — icedtea keybindings still fire during a grab** (guards `dispatcher.emit` runs first — decision #6).
```rust
#[test]
fn compositor_keybindings_fire_during_a_grab() {
    // ... spawn vk, im (grab_keyboard), a couple of toplevels ...
    // Inject the compositor's alt-tab binding; assert the focused window changed
    // even though the grab is active (dispatcher ran before the forward branch).
}
```
(Use whatever alt-tab / window-cycle observable the existing keybinding tests use — find it in `client_protocol.rs`/`end_to_end.rs`.) Run + PASS. Commit:
```bash
git add compositor/tests/client_protocol.rs
git commit -m "test(compositor): M6.1 A6.2 tests 8-10 (popup, grab intercept, keybindings survive)"
```

- [ ] **Step 4: Full A6.2 gate.** Run: `cargo test --workspace -- --test-threads=1 && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check` — Expected: all PASS.

---

### Task B-FINAL (A6.2): published pin, drop `[patch]`, whole-branch review, merge window

- [ ] **Step 1:** After wlr 0.20.31 is **published**: bump every `wlr = "0.20.30"` → `"0.20.31"` and delete the `[patch.crates-io]` block. Run `cargo update -p wlr && cargo test --workspace -- --test-threads=1 && cargo clippy --all-targets -- -D warnings`. Expected: resolves 0.20.31 from crates.io, all green.
- [ ] **Step 2 (Sonnet): Commit.** `git commit -m "build(m6.1): consume published wlr 0.20.31, drop dev [patch] (A6.2)"`.
- [ ] **Step 3: Whole-branch Opus `/code-review`** across all of A6.1 + A6.2.
- [ ] **Step 4: 🛑 Merge window.** Present the branch (both repos' PRs merged/mergeable, all gates green, review clean-or-resolved) and STOP for the owner's go/no-go. On go: `git flow feature finish --keepremote pure-rust-gtk-m6-1-compositor` with `GIT_MERGE_AUTOEDIT=no`, then `git push origin main develop --follow-tags` as the workflow prescribes. Never auto-merge.

---

## Self-Review

**Spec coverage** (against the M6.1 Spec 1 success criteria + A6's A6.1/A6.2 tests):
- Criterion 1 (globals advertised, boots non-fatal): Task B1 + test 1. ✓
- Criterion 2 A6.1 (enter/leave follows keyboard focus; enable→activate→surrounding; IME commit→app; disable/leave→deactivate; second refused): Tasks A3-A7 + B5 tests 2-6, oracle test 7 (B2). ✓
- Criterion 2 A6.2 (popup positioned + told rectangle; grab intercepts): Tasks A9-A11 + B7 + B9 tests 8-9, plus test 10. ✓
- Criterion 3 (unblocks Spec 2 — a text-input client enables on focus and receives commit_string from a bound IME): proven end-to-end by test 4. ✓
- A6 decisions: #1 tablet-v2 out (Global Constraints). #2 relay in crate (Part A). #3 second refused (A2 + test 6). #4 keyboard-focus enter/leave (A3, reconciliation #4). #5 state-diffed enable/commit (A4/A5). #6 grab branch, dispatcher-first (A11 + test 10). #7 popup placement compositor-side (A10 + B7, reconciliation #6). #8 split-phase publish, [patch] (B0/B-FINAL, A8/A13 consent stops). #9 SDD + coordinate + merge window (per-task tiers + A8/A13 Step 6 + B-FINAL Step 4). ✓

**Placeholder scan:** every code step carries real code grounded in verified current file:line; no "TODO"/"add error handling"/"similar to Task N". The one deliberately deferred detail is the popup-reposition-on-commit callback (Task B7 Step 3 / A12) — flagged as a decision to make during A12, not a silent gap. ✓

**Type consistency:** `TextInputEntry`/`InputMethodEntry`/`PopupEntry`, `create_text_input_manager`/`create_input_method_manager`, `input_method_active`, `add_input_popup_in_band`, `send_input_popup_rectangle`, `InputPopupSurfaceId`, `InputMethodHandler`, `DbCommand::InputMethodActive`/`InputPopupPosition`, `relay_keyboard_focus` — each defined once and referenced consistently across Parts A and B. ✓
