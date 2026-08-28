# Pure-Rust GTK-themed UI — M3 Part 2: compositor popup placement/stacking/focus + harness popup client + e2e — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## Contract deviations

Five. Everything else in Part 0 §2 is used verbatim — every type name, field
name, method name and signature in §2.1, §2.2, §2.3 and §2.4 appears below
unchanged.

1. **D1 — `wlr::PopupId::dangling_nth_for_test(n) -> PopupId` is required by
   §2.1 but not listed in §1.1.** Contract §2.1 mandates
   `PopupKey::for_test(n: u64) -> Self`, "a dangling id that never resolves to
   a live popup; for `State` unit tests", and §2.4 requires `state.rs`'s own
   `mod tests` to cover `popup_constraint_box`, `popup_at_point` and
   `popup_chain` "over `PopupKey::for_test` ids". `PopupId`'s inner field is
   `pub(crate)` to the `wlr` crate (§1.1), so `icedtea-compositor` cannot mint
   one. §1.1 lists no constructor. P1 must therefore ship
   `PopupId::dangling_for_test()` and `PopupId::dangling_nth_for_test(n)`,
   mirroring `ToplevelId`'s pair (`toplevel.rs:53`/`:83`) and
   `LayerSurfaceId::dangling_for_test` (`layer.rs:133`) verbatim, including the
   "far outside the real id space" property those two document and test.
   **This is a §10 amendment P1 carries out**; Task 1 below verifies it in the
   published `0.20.28` before any other P2 work starts, and stops the part if
   it is missing.

2. **D2 — P2 implements `popup_initial_commit` as well.** §2.2's "Handler
   impl" paragraph names `new_popup`, `popup_reposition`, `popup_mapped`,
   `popup_unmapped` and `popup_destroyed`, and omits `popup_initial_commit`
   (which §1.3 defines). It is load-bearing: §1.2 says `Popup::send_configure`
   returns `0` and *skips the call* while the surface is not `initialized`, so
   the `unconstrain` P2 runs from `new_popup` has no configure to ride on, and
   §1.7 says the crate answers the initial commit with an unconditional
   `send_configure()`. Unconstraining from `popup_initial_commit` — before the
   crate's own `send_configure` — is the only point at which the compositor's
   constraint box reaches the client's *first* popup configure. Without it the
   first configure carries the raw positioner geometry and
   `a_popup_that_would_leave_the_output_is_flipped_by_the_constraint_adjustment`
   cannot pass. `new_popup`'s `configure_popup` call is kept as §2.2 specifies;
   it is a no-op before initialization, exactly as §1.2 documents.

3. **D3 — focus rule 3 applies only to a chain that took focus.** §2.2's focus
   rule 2 says a non-grabbing popup moves keyboard focus "not at all"; rule 3
   says the last popup of *a* chain being destroyed points focus at the chain's
   root. Read together, a non-grabbing popup that never touched focus would
   *move* focus on destruction — the exact thing rule 2 forbids, and observable
   as a failure of §2.4's `a_non_grabbing_popup_never_moves_keyboard_focus`.
   Ruling: `focus_before_popup` is recorded only when a chain's **first** popup
   is `grabbing`, which is what the field's own doc already implies ("whom
   keyboard focus returns to when a **grabbing** chain ends"), and
   `restore_focus_after_popups` is a no-op when it is `None`. Rules 1–5 are
   otherwise implemented verbatim.

4. **D4 — the `wlr`-typed popup calls live behind `Wayland`, not in
   `state.rs`.** §2.1 gives `wayland.rs` only `PopupKey`, which would leave
   `state.rs` calling `Runtime::configure_popup` with a `wlr::Box2D` and
   `Runtime::popup(..).toplevel_coords(..)` directly. `wayland.rs`'s module doc
   (`wayland.rs:1-18`) makes it "the one seam between the window model and the
   Wayland compositor library", and every other outbound push in the file obeys
   it. P2 therefore adds six `pub(crate)` seam methods to `Wayland`
   (`configure_popup`, `popup_geometry`, `popup_is_reactive`,
   `popup_is_grabbing`, `dismiss_popup`, `has_explicit_grab`), each a silent
   no-op with no runtime attached like every one of its neighbours. This is an
   *addition*, which the contract's preamble permits ("a part may add private
   items freely"); it is listed here only because it moves where §2.2's
   `Runtime::configure_popup` call is written. No signature in §2.2 changes:
   `State::popup_constraint_box` still returns `Rectangle`, and
   `State::runtime_has_explicit_grab` (named in focus rule 1) is the `State`
   method that reads the seam.

5. **D5 — `PopupEntry.geometry` is refreshed on map and on reconfigure, not on
   configure.** §2.2 calls the field "last configured geometry, in root-surface
   coordinates". §1.2 exposes only `Popup::geometry() -> Box2D` = `current`
   (what the client has actually committed) and `Popup::toplevel_coords`, which
   also reads committed state; the *scheduled* geometry a `configure_popup`
   just wrote is not readable through the contract's API at all. The field is
   therefore filled from `Popup::toplevel_coords(0, 0)` + `Popup::geometry()`
   at `popup_mapped`, after every `popup_reposition`, and after every
   `reconstrain_popups` pass — i.e. it always names where the popup **is**, not
   where it has been told to go. That is the correct reading for its only
   consumer, `popup_at_point`: a popup keeps taking clicks at its old place
   until it acks and commits the new one.

**Assumption P2 executes against:** P1 has landed in a `wlroots-sys` worktree
and `wlr` `0.20.28` either is published or is reachable through a root
`[patch.crates-io]` (Task 1). `icedtea`'s branch is `rebuild/pure-rust-gtk-m3`
off `develop` @ `8df998e`, `cargo test --workspace` is green, and no P3–P8 file
exists yet. P2 touches no file under `ui/`.

---

**Goal:** Give the compositor real xdg-popup support end to end — placement
against the parent's output usable area, stacking and hit-testing under the
parent, the focus rules a grabbing menu chain needs, and a harness popup client
strong enough that `compositor/tests/popups.rs` proves all of it against a real
client over a real socket — so that P3's `window::Surface::Popup` and P6's
menus have something that actually maps.

**Architecture:** `wlr` 0.20.28 announces popups through six defaulted
`ToplevelHandler` methods and places them in the scene as a subtree of their
parent, so z-order is free. The compositor keeps a **model** record only:
`State::popups` maps `PopupKey` → `PopupEntry { host, root, output, sequence,
grabbing, mapped, geometry }`, and `State::popup_stack` is the creation-ordered
list whose tail is the topmost popup. On `new_popup` the host is resolved
(window / layer surface / parent popup), the chain root and its output are
recorded, and the popup is unconstrained against
`popup_constraint_box` — the root's output `usable` rect translated into the
root surface's own coordinate system — then configured. `popup_initial_commit`
repeats that so the client's first configure already carries it;
`popup_reposition` and `reconstrain_popups` repeat it on demand. Keyboard focus
is wlroots' job while a popup grab is up (`sync_seat_focus` gains an
explicit-grab early return); when a grabbing chain empties, the compositor
points focus back at the chain's root rather than at whatever the pointer
happens to be over. The harness grows a real popup client: `PopupSpec` carries
a whole `xdg_positioner` in one value, `TestClient` owns a *chain* of
`PopupHandles` (each with its own surface and shm buffer) that it drives all
the way to mapped, and `LayerPanelClient` can open one through
`zwlr_layer_surface_v1.get_popup`.

**Tech Stack:** Rust (edition 2024, `rust-version` 1.94), `wlr` 0.20.28
(`PopupId`, `PopupParent`, `Popup<'h>`, `PositionerRules`, `Box2D`,
`ToplevelHandler`, `Runtime`), `wayland-client` 0.31, `wayland-protocols` 0.32
(`xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_wm_base}`),
`wayland-protocols-wlr` 0.3 (`zwlr_layer_surface_v1`), `icedtea-contract`
(`Rectangle`, `WindowId`, `Event`), `rustix` (memfd for popup shm),
`crossbeam-channel`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
(§Section 2 "Popups: `wlr` crate + compositor (P1/P2)" — the **Compositor** and
**Harness** paragraphs; §Section 7 per-part gate "P2:
`compositor/tests/popups.rs` ×3 deterministic"; §Section 7 cross-cutting rules).
Frozen interface contract:
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§0 module map, §1
`wlr` 0.20.28 — consumed, §2.1 `PopupKey`, §2.2 the `State` registry, placement,
z-order and focus rules, §2.3 the harness API, §2.4 the twelve test names, §8.1
/ §8.3 migration, §9 "P2 — compositor popups + harness popup client",
§9 cross-cutting rules, §8.4 rulings R5 and R7).

**Research notes:** `.superpowers/m3-plan-notes/compositor-harness.md` (§1
`ToplevelKey`/`Wayland::window_for`, §2 `LayerEntry`/`compute_layer_placement`/
`usable_before`, §3 `OutputSurface::usable`, §4 `sync_seat_focus`'s three
existing gates, §5 `window_at_point`, §6 "popups — NOT AVAILABLE at any layer",
§7a `open_grabbing_popup`/`PopupHandles`, §8 the one existing popup test, §9
`LayerPanelClient`/`VirtualPointerClient`/`VirtualKeyboardClient`, §10
`Snapshot` is toplevel-only);
`.superpowers/m3-plan-notes/wlr-popups.md` (§1.3 `wlr_xdg_popup_unconstrain_from_box`
is **root-toplevel-surface space**, §2 the scene subtree and the session-lock
isolation that already covers popups, §4 wlroots owns the popup grab, §6
"downstream hooks (context for P2)").

## Global Constraints

- Repo: `icedtea`, branch `rebuild/pure-rust-gtk-m3`. P2 owns **only**
  `compositor/src/state.rs` (popup paths), `compositor/src/wayland.rs`,
  `compositor/tests/popups.rs` (new), the popup half of `harness/src/lib.rs`,
  the one rewritten test in `compositor/tests/client_protocol.rs`,
  `compositor/Cargo.toml` and `harness/Cargo.toml` (`wlr = "0.20.28"`), and the
  root `Cargo.toml`'s temporary `[patch.crates-io]`.
- **Must not touch:** anything under `ui/`; any non-popup compositor path;
  `Snapshot`/`WindowInfo` in `contract/` (they stay toplevel-only — every popup
  assertion goes through the client-side harness accessors, never D-Bus).
- Pinned crate versions: `wlr` 0.20.28 (the only bump), `wayland-client` 0.31,
  `wayland-protocols` 0.32 (features `client`, `unstable`, `staging` — already
  set in `harness/Cargo.toml`), `wayland-protocols-wlr` 0.3,
  `wayland-protocols-misc` 0.3.12, `rustix` 1, `xkbcommon` 0.8 (the
  compositor's own; P3 adds 0.9 to `ui/`, not here). No new dependency is added
  by this part.
- No `smithay`, no `gtk4`/`gtk4-rs`, no GObject.
- Edition 2024, `rust-version` 1.94 — inherited from the workspace; do not
  change them.
- Gates, all green at every commit:
  - `cargo test -p icedtea-compositor` (unit + every integration test)
  - `cargo test -p icedtea-harness`
  - `cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings`
  - `cargo fmt --all --check` (a hard gate since M2's PR #20/#21 made the tree
    rustfmt-clean)
  - from Task 8 on: `cargo test -p icedtea-compositor --test popups`, run
    **three times** and green all three (contract §9 P2 gate).
  - `cargo test -p icedtea-ui`, `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
    `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
    stay green untouched — P2 edits no `ui/` file, so a failure there is a
    merge accident, not a P2 change.
- **THE GATE (contract §8.2) stays byte-identical.** P2 edits none of
  `ui/tests/themed_button_offscreen.rs`, `ui/tests/adwaita_coverage.rs`,
  `ui/tests/gtk4_property_reference.rs`, `ui/tests/transition_screencopy.rs`,
  `ui/src/css/**`, `ui/src/anim/**`, `ui/src/shm.rs`. In the `wlr` crate,
  `backend.rs`'s `mod implicit_grab_tests` must stay green **unmodified** —
  P2 changes no `wlr` source at all, so this is a consumption check only.
- **Exactly two rewrites are permitted (contract §8.3), both listed there:**
  `compositor/tests/client_protocol.rs`'s
  `a_popup_grab_still_dismisses_on_a_click_outside_it` (its press/serial/
  click-outside sequence and its `popup_done()` assertion kept verbatim, its
  negative configure assertion **inverted**), and the harness/`state.rs` doc
  paragraphs §8.3's last two rows name. Every other existing compositor and
  harness test keeps every assertion.
- `harness::TestClient::popup_dismissed()` → `popup_done()` is the **only**
  rename in the harness; its one call site is `client_protocol.rs:2765`.
- Every load-bearing test records a mutation check in its own doc comment:
  break the code the test claims to cover, confirm the test fails, restore.
- Timing assertions are generous complexity bounds, never wall-clock pins;
  every wait goes through `wait_until`/`wait_until_timeout`, never a bare sleep
  plus assert.
- Untrusted input never panics. A client-supplied positioner (anchor, gravity,
  constraint adjustment, sizes, offsets, parent serials) is *entirely*
  attacker-controlled: every popup path is `Option`-returning and no-ops on a
  miss, exactly as `layer_surface_commit`'s `let … else` posture does, and
  Task 4 carries a dedicated never-panic test over hostile positioner geometry.
- Parts execute in order 1 → 8 on the one branch, so P1's `Produces` are
  available; P3 consumes this part's `Produces` exactly as written below.
- Every commit message ends with the trailer
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- **P1's publish and the final merge are consent stops.** P2 neither publishes
  nor merges; Task 11 ends by presenting the part as ready.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `Cargo.toml` (workspace root) | Modify (Task 1), Revert (Task 11) | The temporary `[patch.crates-io] wlr = { path = … }` pointing at P1's worktree, in its own commit, dropped once `0.20.28` is on crates.io. |
| `compositor/Cargo.toml` | Modify | `wlr = "0.20.28"` in `[dependencies]` (line 36) and `[dev-dependencies]` (line 69). |
| `harness/Cargo.toml` | Modify | `wlr = "0.20.28"` (line 18). |
| `compositor/src/wayland.rs` | Modify | `PopupKey` (§2.1) and the six `pub(crate)` popup seam methods on `Wayland` (D4). The only compositor file that names `wlr` popup types besides `state.rs`'s handler signatures. |
| `compositor/src/state.rs` | Modify | `PopupHost`, `PopupRoot`, `PopupEntry`; the `popups`/`popup_stack`/`next_popup_sequence`/`focus_before_popup` fields; `popup_constraint_box`, `record_popup`, `forget_popup`, `popup_root`, `popup_chain`, `popup_at_point`, `reconstrain_popups`, `restore_focus_after_popups`, `runtime_has_explicit_grab`; the six `ToplevelHandler` popup methods; `sync_seat_focus`'s fourth early return; the `reconstrain_popups` hooks in `sync_window_to_scene`/`arrange_layers`; the popup pruning in `forget_toplevel`/`layer_surface_destroyed`; the two rewritten doc paragraphs (`state.rs:912`, `:6497`). New unit tests in its own `mod tests`. |
| `harness/src/lib.rs` | Modify | `PopupSpec` + the `PopupAnchor`/`PopupGravity`/`PopupConstraint` re-exports; `PopupRole(usize)`; the popup fields on `ClientState`; `PopupHandles` grown to carry a buffer; `TestClient`'s popup chain API; `LayerPanelClient::open_popup`; the `popup_dismissed` → `popup_done` rename; the two rewritten doc paragraphs. |
| `compositor/tests/popups.rs` | Create | The twelve tests contract §2.4 names, in that order. |
| `compositor/tests/client_protocol.rs` | Modify | `a_popup_grab_still_dismisses_on_a_click_outside_it`, rewritten per §8.3. |

Compositor unit tests live in `compositor/src/state.rs`'s existing
`#[cfg(test)] mod tests`; everything that needs a real client lives in
`compositor/tests/popups.rs`.

---

## Task 1: Consume `wlr` 0.20.28 and pin the positioner arithmetic

**Files:**
- Create: `compositor/tests/popups.rs`
- Modify: `Cargo.toml` (workspace root — add `[patch.crates-io]`)
- Modify: `compositor/Cargo.toml:36`, `compositor/Cargo.toml:69`
- Modify: `harness/Cargo.toml:18`

**Interfaces:**
- Consumes (contract §1, from P1):
  ```rust
  pub struct PopupId(pub(crate) u64);
  impl PopupId {
      pub fn dangling_for_test() -> PopupId;          // deviation D1
      pub fn dangling_nth_for_test(n: u64) -> PopupId; // deviation D1
  }
  pub enum PopupParent { Toplevel(ToplevelId), Layer(LayerSurfaceId), Popup(PopupId) }
  impl PopupParent { pub fn root(self, rt: &Runtime) -> Option<PopupParent>; pub fn is_popup(self) -> bool; }
  pub struct PositionerRules {
      pub anchor_rect: Box2D,
      pub anchor: PositionerAnchor,
      pub gravity: PositionerGravity,
      pub constraint_adjustment: ConstraintAdjustment,
      pub size: (i32, i32),
      pub parent_size: Option<(i32, i32)>,
      pub offset: (i32, i32),
      pub reactive: bool,
      pub parent_configure_serial: Option<u32>,
  }
  impl PositionerRules {
      #[must_use] pub fn geometry(&self) -> Box2D;
      #[must_use] pub fn unconstrain_box(&self, constraint: &Box2D) -> Box2D;
  }
  pub enum PositionerAnchor { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }
  pub enum PositionerGravity { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }
  pub struct ConstraintAdjustment: u32;  // SLIDE_X SLIDE_Y FLIP_X FLIP_Y RESIZE_X RESIZE_Y
  ```
- Produces: nothing yet — the file `compositor/tests/popups.rs` exists with one
  test that pins the positioner arithmetic every later test in this part
  depends on.

- [ ] **Step 1: Write the failing test**

Create `compositor/tests/popups.rs`:

```rust
//! xdg-popup, end to end: a real `wayland-client` opening real popups on a
//! real headless compositor over a real socket.
//!
//! Everything asserted here is asserted from the **client** side. The D-Bus
//! `Snapshot` is toplevel-only by design (contract §9, "must not touch:
//! `Snapshot`/`WindowInfo`"), so a popup's placement is read back the way a
//! real client reads it: from `xdg_popup.configure`. Model-side state
//! (`popup_constraint_box`, `popup_at_point`, `popup_chain`) is unit-tested in
//! `compositor/src/state.rs`'s own `mod tests` instead.
//!
//! Contract: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` §2.4
//! fixes these twelve names and this order.

use std::time::Duration;

use icedtea_harness::{
    Compositor, PopupAnchor, PopupConstraint, PopupGravity, PopupSpec, SessionLockClient,
    TestClient, VirtualKeyboardClient, VirtualPointerClient,
};

/// `BTN_LEFT`, the only button the compositor's decoration path acts on.
const BTN_LEFT: u32 = 0x110;

/// The `wlr` crate really does expose the xdg-popup API this part is written
/// against, and `wlr_xdg_positioner_rules` really does compute the geometry
/// every placement test below predicts.
///
/// This is the part's dependency check *and* its arithmetic oracle: the
/// anchor/gravity pair used throughout `popups.rs`
/// (`BottomLeft` + `BottomRight` on a `(10, 10, 20, 20)` anchor rect) is
/// pinned here against wlroots' own C implementation, so a placement test
/// that fails later is a compositor bug rather than a mis-derived expectation.
///
/// Mutation check: change the expected geometry's `y` from `30` to `10` and
/// this test fails — the call really reaches wlroots rather than returning a
/// default.
#[test]
fn the_wlr_crate_exposes_the_xdg_popup_api_part_2_is_written_against() {
    // Deviation D1: `PopupKey::for_test` cannot exist without these.
    let a = wlr::PopupId::dangling_nth_for_test(1);
    let b = wlr::PopupId::dangling_nth_for_test(2);
    assert_eq!(a, wlr::PopupId::dangling_nth_for_test(1));
    assert_ne!(a, b, "distinct n must produce distinct dangling popup ids");

    assert!(wlr::PopupParent::Popup(a).is_popup());
    assert!(!wlr::PopupParent::Toplevel(wlr::ToplevelId::dangling_nth_for_test(1)).is_popup());

    let rules = wlr::PositionerRules {
        anchor_rect: wlr::Box2D::new(10, 10, 20, 20),
        anchor: wlr::PositionerAnchor::BottomLeft,
        gravity: wlr::PositionerGravity::BottomRight,
        constraint_adjustment: wlr::ConstraintAdjustment::empty(),
        size: (64, 48),
        parent_size: None,
        offset: (0, 0),
        reactive: false,
        parent_configure_serial: None,
    };
    // Anchor BottomLeft of (10, 10, 20, 20) is (10, 30); gravity BottomRight
    // puts the surface's top-left corner on the anchor point.
    assert_eq!(
        rules.geometry(),
        wlr::Box2D::new(10, 30, 64, 48),
        "the anchor/gravity arithmetic every placement test in this file \
         predicts"
    );
    // Nothing to adjust: a constraint that already contains the geometry
    // leaves it exactly where the rules put it.
    assert_eq!(
        rules.unconstrain_box(&wlr::Box2D::new(-100, -100, 400, 400)),
        wlr::Box2D::new(10, 30, 64, 48)
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: FAIL to compile — `error[E0433]: failed to resolve: could not find
`PopupId` in `wlr`` (and the same for `PopupParent`, `PositionerRules`,
`PositionerAnchor`, `PositionerGravity`, `ConstraintAdjustment`), plus
`error[E0432]: unresolved import` on `icedtea_harness::{PopupAnchor,
PopupConstraint, PopupGravity, PopupSpec}` — the pinned `wlr` is still
`0.20.27` and the harness has no popup API yet.

- [ ] **Step 3: Write the implementation**

First find P1's worktree and confirm the deviation-D1 constructors exist. Do
not guess the path — read it:

```bash
git -C /home/joseph/Projects/wlroots-sys worktree list
```

Use the worktree whose branch is P1's popup branch; call its path `$P1`. Then:

```bash
grep -n 'dangling_nth_for_test\|dangling_for_test' "$P1"/crates/wlr/src/popup.rs
grep -n '^version' "$P1"/crates/wlr/Cargo.toml
```

Expected: both constructors present on `PopupId`, and `version = "0.20.28"`.
**If either is missing, stop and report it as the §10 amendment D1 describes —
do not work around it in `icedtea`.** `PopupId`'s field is `pub(crate)`; there
is no other way to mint one, and contract §2.4 requires the `State` unit tests
that need it.

`Cargo.toml` (workspace root) — append, substituting the real `$P1` path:

```toml
# TEMPORARY, dropped in P2's last commit: P2–P8 develop against P1's wlr
# worktree before 0.20.28 is published. Exactly the shape the implicit-grab
# fix used (develop @ 90813cc reverted it the same way). `cargo publish` and
# the merge both require this gone.
[patch.crates-io]
wlr = { path = "/home/joseph/Projects/wlroots-sys-p1-popups/crates/wlr" }
```

`compositor/Cargo.toml` — both occurrences:

```toml
wlr = "0.20.28"
```

`harness/Cargo.toml:18`:

```toml
wlr = "0.20.28"
```

Then create `harness/src/lib.rs`'s popup re-exports so the test's imports
resolve. Near the other `pub use` items at the top of the file, add:

```rust
/// The `xdg_positioner` enums, re-exported so a test can name an anchor or a
/// gravity without depending on `wayland-protocols` itself.
pub use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor as PopupAnchor, ConstraintAdjustment as PopupConstraint, Gravity as PopupGravity,
};
```

and, immediately above `PopupHandles` (`lib.rs:1836`), the spec value:

```rust
/// Everything an `xdg_positioner` needs, in one value, so a test that opens a
/// popup reads as one statement rather than eight setter calls.
///
/// The fields are the protocol's own: `anchor_rect` is `(x, y, width, height)`
/// in the **parent's window-geometry** coordinates, `size` is the popup's
/// requested size, and `constraint_adjustment` is the bitmask the compositor
/// is allowed to use when the unadjusted position would fall outside the
/// constraint box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupSpec {
    pub anchor_rect: (i32, i32, i32, i32),
    pub size: (i32, i32),
    pub anchor: PopupAnchor,
    pub gravity: PopupGravity,
    pub constraint_adjustment: PopupConstraint,
    pub offset: (i32, i32),
    /// `Some(serial)` sends `xdg_popup.grab(seat, serial)` **before** the
    /// first commit, as xdg-shell requires. The serial must come from
    /// [`TestClient::last_pointer_serial`] after a button press.
    pub grab: Option<u32>,
    pub reactive: bool,
}

impl PopupSpec {
    /// A `w` x `h` popup anchored at `(0, 0, 1, 1)` with anchor and gravity
    /// both `BottomLeft`, no constraint adjustment, no grab and not reactive
    /// -- the shape most tests want, with every interesting knob left to the
    /// builders below.
    pub fn new(w: i32, h: i32) -> Self {
        PopupSpec {
            anchor_rect: (0, 0, 1, 1),
            size: (w, h),
            anchor: PopupAnchor::BottomLeft,
            gravity: PopupGravity::BottomLeft,
            constraint_adjustment: PopupConstraint::empty(),
            offset: (0, 0),
            grab: None,
            reactive: false,
        }
    }

    pub fn anchor_rect(self, x: i32, y: i32, w: i32, h: i32) -> Self {
        PopupSpec {
            anchor_rect: (x, y, w, h),
            ..self
        }
    }

    pub fn anchor(self, a: PopupAnchor) -> Self {
        PopupSpec { anchor: a, ..self }
    }

    pub fn gravity(self, g: PopupGravity) -> Self {
        PopupSpec { gravity: g, ..self }
    }

    pub fn constraint(self, c: PopupConstraint) -> Self {
        PopupSpec {
            constraint_adjustment: c,
            ..self
        }
    }

    pub fn grab(self, serial: u32) -> Self {
        PopupSpec {
            grab: Some(serial),
            ..self
        }
    }

    pub fn reactive(self, on: bool) -> Self {
        PopupSpec { reactive: on, ..self }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 1 test. (`SessionLockClient`, `TestClient`,
`VirtualKeyboardClient`, `VirtualPointerClient`, `Compositor`, `BTN_LEFT`,
`Duration` are imported but unused until Task 8; silence that for this one
commit with `#![allow(unused_imports)]` at the top of `popups.rs` **and delete
that attribute in Task 8's commit** — it is the only allow this part adds, and
Task 8 Step 3 removes it explicitly.)

Run: `cargo test --workspace`
Expected: PASS — nothing else changed behaviour; the `wlr` bump is additive
(every new handler method is defaulted, contract §1.3).

- [ ] **Step 5: Commit**

Two commits: the `[patch]` alone, then the bump.

```bash
cargo fmt --all
cargo fmt --all --check
git add Cargo.toml
git commit -m "$(cat <<'EOF'
build: dev-patch wlr to P1's xdg-popup worktree

Temporary, and dropped in this part's last commit: P2-P8 develop against
P1's worktree until wlr 0.20.28 is published to crates.io. Same shape the
implicit-grab fix used.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"

cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings
git add compositor/Cargo.toml harness/Cargo.toml harness/src/lib.rs compositor/tests/popups.rs
git commit -m "$(cat <<'EOF'
feat(harness): PopupSpec, and consume wlr 0.20.28's xdg-popup API

Pins compositor and harness to wlr 0.20.28 and adds the harness value that
carries a whole xdg_positioner: PopupSpec plus the re-exported protocol
anchor/gravity/constraint enums.

compositor/tests/popups.rs opens with the arithmetic oracle every later
placement test predicts against: wlr_xdg_positioner_rules_get_geometry on a
BottomLeft/BottomRight pair, checked against wlroots' own C implementation.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `PopupKey` and the `Wayland` popup seam

**Files:**
- Modify: `compositor/src/wayland.rs` (add `PopupKey` next to `ToplevelKey` at
  `:32-50`; add the six seam methods next to `keyboard_focus` at `:497`)

**Interfaces:**
- Consumes: Task 1's `wlr::{PopupId, Box2D, Runtime}`; `icedtea_contract::Rectangle`.
- Produces:
  ```rust
  // compositor/src/wayland.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub struct PopupKey(pub(crate) wlr::PopupId);
  impl PopupKey {
      pub(crate) fn new(id: wlr::PopupId) -> Self;
      pub fn for_test(n: u64) -> Self;
  }
  impl Wayland {
      pub(crate) fn configure_popup(&self, popup: PopupKey, constraint: Rectangle) -> bool;
      pub(crate) fn popup_geometry(&self, popup: PopupKey) -> Option<Rectangle>;
      pub(crate) fn popup_is_reactive(&self, popup: PopupKey) -> bool;
      pub(crate) fn popup_is_grabbing(&self, popup: PopupKey) -> bool;
      pub(crate) fn dismiss_popup(&self, popup: PopupKey) -> usize;
      pub(crate) fn has_explicit_grab(&self) -> bool;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `compositor/src/wayland.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    /// `PopupKey::for_test` mints distinct, dangling keys, and every seam
    /// method is a silent no-op against a `Wayland` with no runtime attached
    /// -- the property every unit test in `state.rs` relies on and the one
    /// this file's module doc states for all of its outbound methods.
    ///
    /// Mutation check: make `configure_popup` `unwrap()` the runtime instead
    /// of `?`-ing it and this test panics.
    #[test]
    fn popup_keys_are_distinct_and_every_popup_seam_method_no_ops_without_a_runtime() {
        let a = PopupKey::for_test(1);
        let b = PopupKey::for_test(2);
        assert_eq!(a, PopupKey::for_test(1), "the same n gives the same key");
        assert_ne!(a, b, "distinct n gives distinct keys");

        let wayland = Wayland::new();
        assert!(
            !wayland.configure_popup(
                a,
                Rectangle {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100
                }
            ),
            "no runtime: configuring a popup reports failure rather than panicking"
        );
        assert_eq!(wayland.popup_geometry(a), None);
        assert!(!wayland.popup_is_reactive(a));
        assert!(!wayland.popup_is_grabbing(a));
        assert_eq!(wayland.dismiss_popup(a), 0);
        assert!(!wayland.has_explicit_grab());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --lib wayland::tests::popup_keys`
Expected: FAIL to compile — `error[E0433]: failed to resolve: use of undeclared
type `PopupKey``, and `error[E0599]: no method named `configure_popup` found
for struct `Wayland``.

- [ ] **Step 3: Write the implementation**

`compositor/src/wayland.rs`, immediately after `impl ToplevelKey` (`:50`):

```rust
/// Identifies one live client popup.
///
/// The popup twin of [`ToplevelKey`], and wrapped for the same reason: this
/// file stays the only one that mentions the compositor library's own id
/// types, so a key held past the popup's departure resolves to nothing rather
/// than to freed memory or to a different popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) wlr::PopupId);

impl PopupKey {
    /// Wrap a library id. Only the handler impls call this.
    pub(crate) fn new(id: wlr::PopupId) -> Self {
        PopupKey(id)
    }

    /// A key that no live client can have, for tests that drive `State`'s
    /// popup entry points without a client.
    ///
    /// Same contract as [`ToplevelKey::for_test`]: `n` only distinguishes one
    /// test key from another, and a key built this way never resolves to a
    /// live popup, so every outbound push through it is a no-op.
    pub fn for_test(n: u64) -> Self {
        PopupKey(wlr::PopupId::dangling_nth_for_test(n))
    }
}
```

and, after `keyboard_focus` (`:497`), the seam. Every method follows this
file's own rule: no runtime, no binding, or an id gone stale since the
`run_all` that announced it returned ⇒ a silent no-op, never a panic.

```rust
    /// Unconstrain `popup` against `constraint` and answer its
    /// `xdg_surface` with a configure.
    ///
    /// `constraint` is in the **root toplevel/layer surface's own coordinate
    /// system**, not layout or output space -- `wlr_xdg_popup_unconstrain_from_box`'s
    /// own header says so, and contract ruling R7 restates it. The caller
    /// ([`crate::state::State::popup_constraint_box`]) does the translation;
    /// this method only converts the model's `Rectangle` to the library's box.
    ///
    /// `false` on a miss (no runtime, popup gone, or the popup's surface not
    /// `initialized` yet, in which case the library skips the configure rather
    /// than tripping wlroots' own assert -- see contract §1.2's
    /// `Popup::send_configure`).
    pub(crate) fn configure_popup(&self, popup: PopupKey, constraint: Rectangle) -> bool {
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        runtime.configure_popup(
            popup.0,
            &wlr::Box2D::new(
                constraint.x,
                constraint.y,
                constraint.width,
                constraint.height,
            ),
        )
    }

    /// Where `popup` currently *is*, in its chain root's surface coordinates:
    /// `wlr_xdg_popup_get_toplevel_coords` for the origin,
    /// `wlr_xdg_popup_state.geometry` for the size.
    ///
    /// Committed state, not scheduled: between a configure and the client's
    /// ack this still names the old position, which is exactly right for
    /// hit-testing -- a popup keeps taking clicks where it is drawn.
    pub(crate) fn popup_geometry(&self, popup: PopupKey) -> Option<Rectangle> {
        let runtime = self.runtime.as_ref()?;
        let handle = runtime.popup(popup.0)?;
        let (x, y) = handle.toplevel_coords(0, 0);
        let geometry = handle.geometry();
        Some(Rectangle {
            x,
            y,
            width: geometry.width,
            height: geometry.height,
        })
    }

    /// Whether the client asked for its popup to be re-unconstrained whenever
    /// the parent moves (`xdg_positioner.set_reactive`).
    pub(crate) fn popup_is_reactive(&self, popup: PopupKey) -> bool {
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.popup(popup.0))
            .is_some_and(|handle| handle.is_reactive())
    }

    /// Whether the client sent `xdg_popup.grab` for this popup.
    pub(crate) fn popup_is_grabbing(&self, popup: PopupKey) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.popup_is_grabbing(popup.0))
    }

    /// Send `xdg_popup.popup_done` to `popup` and, deepest-first, to every
    /// popup under it. Returns how many were dismissed (`0` on any miss).
    pub(crate) fn dismiss_popup(&self, popup: PopupKey) -> usize {
        self.runtime
            .as_ref()
            .map_or(0, |runtime| runtime.dismiss_popup(popup.0))
    }

    /// Whether *some* explicit seat grab is in force right now -- an
    /// xdg-popup grab or a drag-and-drop grab.
    ///
    /// The compositor never installs one of these itself: wlroots owns the
    /// popup grab's whole lifetime (contract §1.6). This is the read that
    /// tells `sync_seat_focus` to keep its hands off the seat's keyboard
    /// while one is up.
    pub(crate) fn has_explicit_grab(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(wlr::Runtime::seat_has_explicit_grab)
    }
```

If `Rectangle` is not already imported in `wayland.rs`, it is: `use
icedtea_contract::{Rectangle, WindowId};` at `:22`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --lib wayland::tests`
Expected: PASS — every existing `wayland.rs` test plus the new one.

Run: `cargo test -p icedtea-compositor`
Expected: PASS — no behaviour changed; nothing calls the new methods yet.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/src/wayland.rs
git commit -m "$(cat <<'EOF'
feat(compositor): PopupKey and the popup half of the wayland seam

PopupKey is the popup twin of ToplevelKey, so state.rs keeps naming model
keys rather than library ids. Six pub(crate) methods on Wayland carry every
outbound popup push -- configure, geometry read-back, reactive/grabbing
flags, dismissal, and the explicit-grab probe -- each a silent no-op with no
runtime attached, like every other method in this file.

The constraint box those calls take is root-surface space, not layout
space; the translation is state.rs's, per contract ruling R7.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: The `State` popup registry

**Files:**
- Modify: `compositor/src/state.rs` (add `PopupHost`, `PopupRoot`, `PopupEntry`
  next to `LayerEntry` at `:96`; add the four fields next to `layers`/
  `layer_focus`/`next_layer_sequence` at `:865-878`; initialise them in
  `State::new`; add the registry methods next to `compute_layer_placement`;
  extend `mod tests`)

**Interfaces:**
- Consumes: Task 2's `crate::wayland::PopupKey`; `wlr::LayerSurfaceId`;
  `icedtea_contract::{Rectangle, WindowId}`.
- Produces:
  ```rust
  // compositor/src/state.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum PopupHost { Window(WindowId), Layer(wlr::LayerSurfaceId), Popup(PopupKey) }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum PopupRoot { Window(WindowId), Layer(wlr::LayerSurfaceId) }
  pub struct PopupEntry {
      pub host: PopupHost,
      pub root: PopupRoot,
      pub output: u32,
      pub sequence: u64,
      pub grabbing: bool,
      pub mapped: bool,
      pub geometry: Rectangle,
  }
  impl State {
      pub(crate) fn record_popup(&mut self, popup: PopupKey, host: PopupHost, grabbing: bool);
      pub(crate) fn forget_popup(&mut self, popup: PopupKey);
      pub fn popup_root(&self, popup: PopupKey) -> Option<PopupRoot>;
      pub fn popup_chain(&self, root: PopupRoot) -> Vec<PopupKey>;
      pub fn popup_count(&self) -> usize;
  }
  ```
  Private companions: `State::popup_root_of_host`, `State::popup_output_of_root`,
  `State::popup_roots`.

- [ ] **Step 1: Write the failing test**

Append to `compositor/src/state.rs`'s `mod tests`:

```rust
    /// Recording a chain builds it root-first and `popup_chain` returns it
    /// deepest-last, whatever order the map iterates in.
    ///
    /// The chain order is load-bearing twice over: `reconstrain_popups`
    /// reconfigures parents before children (a child's own placement is
    /// expressed against its parent), and `dismiss` order is the protocol's
    /// reverse-creation requirement.
    ///
    /// Mutation check: drop the `sort_unstable_by_key` from `popup_chain` and
    /// this fails -- `HashMap` iteration order is not creation order.
    #[test]
    fn a_recorded_popup_chain_reports_its_root_and_orders_itself_deepest_last() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            },
        );

        let outer = crate::wayland::PopupKey::for_test(1);
        let inner = crate::wayland::PopupKey::for_test(2);
        let deepest = crate::wayland::PopupKey::for_test(3);
        state.record_popup(outer, PopupHost::Window(window), false);
        state.record_popup(inner, PopupHost::Popup(outer), false);
        state.record_popup(deepest, PopupHost::Popup(inner), false);

        let root = PopupRoot::Window(window);
        assert_eq!(state.popup_root(outer), Some(root));
        assert_eq!(
            state.popup_root(deepest),
            Some(root),
            "a nested popup's root is the chain's bottom, not its parent"
        );
        assert_eq!(state.popup_chain(root), vec![outer, inner, deepest]);
        assert_eq!(state.popup_count(), 3);

        // A key nobody recorded is a miss, never a panic.
        assert_eq!(state.popup_root(crate::wayland::PopupKey::for_test(99)), None);
        assert!(
            state
                .popup_chain(PopupRoot::Layer(wlr::LayerSurfaceId::dangling_for_test()))
                .is_empty(),
            "a root with no popups has an empty chain"
        );
    }

    /// Forgetting a popup drops it from both the map and the stack, and
    /// forgetting one nobody recorded is a no-op rather than a panic
    /// (contract §1.3: a destroy may name an id this handler was never told
    /// about).
    ///
    /// Mutation check: drop the `popup_stack.retain` from `forget_popup` and
    /// the stack-length assertion fails -- a stale key would keep answering
    /// `popup_at_point` after its popup was gone.
    #[test]
    fn forgetting_a_popup_clears_both_the_map_and_the_stack() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            },
        );
        let a = crate::wayland::PopupKey::for_test(1);
        let b = crate::wayland::PopupKey::for_test(2);
        state.record_popup(a, PopupHost::Window(window), false);
        state.record_popup(b, PopupHost::Popup(a), false);

        state.forget_popup(b);
        assert_eq!(state.popup_chain(PopupRoot::Window(window)), vec![a]);
        assert_eq!(state.popup_stack, vec![a]);

        state.forget_popup(crate::wayland::PopupKey::for_test(99));
        assert_eq!(state.popup_count(), 1, "an unknown key forgets nothing");

        state.forget_popup(a);
        assert_eq!(state.popup_count(), 0);
        assert!(state.popup_stack.is_empty());
    }

    /// A popup whose host this compositor does not model is dropped rather
    /// than recorded under a fabricated root -- the untrusted-client rule
    /// (contract §9: a malformed value is dropped and logged, never a panic).
    ///
    /// Mutation check: make `popup_root_of_host` return
    /// `Some(PopupRoot::Window(WindowId(0)))` on a miss and the count
    /// assertion fails.
    #[test]
    fn a_popup_on_an_unmodelled_host_is_not_recorded() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        // A parent popup that was never recorded: the client raced its own
        // destroy against the child's creation.
        state.record_popup(
            crate::wayland::PopupKey::for_test(1),
            PopupHost::Popup(crate::wayland::PopupKey::for_test(50)),
            true,
        );
        assert_eq!(state.popup_count(), 0);
        assert!(state.popup_stack.is_empty());
        assert_eq!(
            state.focus_before_popup, None,
            "a popup that was never recorded must not have parked a focus \
             restore target"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --lib state::tests::a_recorded_popup_chain state::tests::forgetting_a_popup state::tests::a_popup_on_an_unmodelled_host`
Expected: FAIL to compile — `error[E0433]: failed to resolve: use of undeclared
type `PopupHost`` (and `PopupRoot`), `error[E0599]: no method named
`record_popup` found for struct `State`` (and `forget_popup`, `popup_root`,
`popup_chain`, `popup_count`), `error[E0609]: no field `popup_stack` on type
`State`` (and `focus_before_popup`).

- [ ] **Step 3: Write the implementation**

`compositor/src/state.rs`, after `LayerEntry`'s `impl` block:

```rust
/// What a popup hangs off, in model terms.
///
/// The library's own `wlr::PopupParent` says the same thing in library terms;
/// this is its model translation, made once in `new_popup` so nothing past
/// the handler boundary has to resolve a `ToplevelId` again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupHost {
    Window(WindowId),
    Layer(wlr::LayerSurfaceId),
    Popup(crate::wayland::PopupKey),
}

/// The bottom of a popup chain -- never a popup.
///
/// Every placement, focus and dismissal decision is taken against this rather
/// than against the immediate host: a menu three levels deep is still
/// constrained to *its window's* output, and focus still returns to *its
/// window* when the chain ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupRoot {
    Window(WindowId),
    Layer(wlr::LayerSurfaceId),
}

/// The model's own record of one live xdg-popup.
///
/// Same reasoning as [`LayerEntry`]: the library's `wlr::Popup` handle only
/// lives for the duration of one handler call, so everything a later,
/// unrelated call needs (a window move re-running placement, a click looking
/// for the topmost popup, a destroy deciding whether the chain has emptied)
/// is copied out while the handle is live.
pub struct PopupEntry {
    /// The immediate parent -- a window, a layer surface, or another popup.
    pub host: PopupHost,
    /// The chain's bottom, resolved once at record time. Never re-derived:
    /// the host may die before this popup does.
    pub root: PopupRoot,
    /// The model output index the root sits on, or [`NO_OUTPUT`] when the
    /// root had no output at record time -- exactly [`LayerEntry::output`]'s
    /// convention, and every reader treats a miss the same way.
    pub output: u32,
    /// This popup's position in a total, stable creation order across every
    /// popup this compositor has ever announced. The z-order tiebreak among
    /// siblings, and the sort key `popup_chain` orders by.
    pub sequence: u64,
    /// The client sent `xdg_popup.grab`. Observed, never driven: wlroots owns
    /// the grab's whole lifetime (contract §1.6).
    pub grabbing: bool,
    /// Whether the popup currently has a buffer on screen.
    pub mapped: bool,
    /// Where the popup is, in its `root`'s surface coordinates. Committed
    /// state, refreshed on map and after every reconfigure -- see
    /// `Wayland::popup_geometry`'s doc for why that is the right reading for
    /// `popup_at_point`.
    pub geometry: Rectangle,
}
```

The four `State` fields, immediately after `next_layer_sequence` (`:878`):

```rust
    /// Every live popup's model-side bookkeeping, keyed by the library's own
    /// id. Populated at `new_popup`, kept current on map/unmap/reposition,
    /// removed at `popup_destroyed` -- and pruned wholesale when a chain's
    /// root dies, since a root's death takes its popups with it and the
    /// per-popup destroys may never be delivered.
    popups: HashMap<crate::wayland::PopupKey, PopupEntry>,
    /// Creation-ordered; the tail is the topmost popup.
    ///
    /// Z-order itself is the scene's (contract §2.2: a popup's scene subtree
    /// hangs off its parent's, so it stacks with its parent for free and the
    /// compositor never calls `wlr_scene_node_place_*` for a popup). This
    /// exists for the two things the scene cannot answer from the model side:
    /// hit-testing (`popup_at_point`) and dismissal order.
    popup_stack: Vec<crate::wayland::PopupKey>,
    /// Source of [`PopupEntry::sequence`]. Monotonic and never reused, the
    /// same shape as `next_layer_sequence`.
    next_popup_sequence: u64,
    /// Whom keyboard focus returns to when a **grabbing** chain ends.
    ///
    /// Set only when a chain's first popup grabs (contract deviation D3: a
    /// non-grabbing popup never moves focus, so restoring one would be the
    /// move rule 2 forbids). Taken -- not merely read -- by
    /// `restore_focus_after_popups`, so a second chain cannot inherit the
    /// first one's target.
    focus_before_popup: Option<PopupRoot>,
```

and their initialisers in `State::new`, beside `layers: HashMap::new()`:

```rust
            popups: HashMap::new(),
            popup_stack: Vec::new(),
            next_popup_sequence: 0,
            focus_before_popup: None,
```

The registry itself, after `compute_layer_placement`:

```rust
    /// Announce a popup to the model.
    ///
    /// Drops the popup outright when its host is not something this
    /// compositor models -- an unbound toplevel, a layer surface already
    /// forgotten, or a parent popup that died between the two `new_popup`
    /// signals. That is the untrusted-client posture the rest of this file
    /// takes (contract §9): a client can drive any of the three, and none of
    /// them is a reason to fabricate a root.
    pub(crate) fn record_popup(
        &mut self,
        popup: crate::wayland::PopupKey,
        host: PopupHost,
        grabbing: bool,
    ) {
        let Some(root) = self.popup_root_of_host(host) else {
            tracing::debug!(
                ?popup,
                ?host,
                "ignoring a popup whose host this compositor does not model"
            );
            return;
        };
        let output = self.popup_output_of_root(root);
        let sequence = self.next_popup_sequence;
        self.next_popup_sequence += 1;
        // Read before the insert: "was this chain empty" must not count the
        // popup being recorded.
        let chain_was_empty = !self.popups.values().any(|entry| entry.root == root);
        self.popups.insert(
            popup,
            PopupEntry {
                host,
                root,
                output,
                sequence,
                grabbing,
                // False until `popup_mapped`, exactly as `LayerEntry::mapped`
                // is: a popup with no buffer yet is not on screen and must not
                // answer `popup_at_point`.
                mapped: false,
                geometry: Rectangle {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            },
        );
        self.popup_stack.push(popup);
        // Deviation D3: only a grabbing chain parks a restore target, and only
        // its first popup does -- a submenu opening over a menu must not
        // overwrite the window the whole chain came from.
        if grabbing && chain_was_empty {
            self.focus_before_popup = Some(root);
        }
    }

    /// Drop `popup` from the model. A no-op, panic-free, on a key this
    /// compositor was never told about -- contract §1.3's caveat, which
    /// `layer_surface_destroyed` already takes the same posture towards.
    pub(crate) fn forget_popup(&mut self, popup: crate::wayland::PopupKey) {
        self.popups.remove(&popup);
        self.popup_stack.retain(|key| *key != popup);
    }

    /// The chain root `popup` belongs to, or `None` if it is not recorded.
    pub fn popup_root(&self, popup: crate::wayland::PopupKey) -> Option<PopupRoot> {
        self.popups.get(&popup).map(|entry| entry.root)
    }

    /// Every popup under `root`, deepest last.
    ///
    /// Sorted by `sequence` rather than by walking `host` links: a chain is
    /// created bottom-up, so creation order *is* depth order, and a sort
    /// cannot loop on a cycle a malicious client might otherwise build out of
    /// `PopupHost::Popup` links.
    pub fn popup_chain(&self, root: PopupRoot) -> Vec<crate::wayland::PopupKey> {
        let mut chain: Vec<(u64, crate::wayland::PopupKey)> = self
            .popups
            .iter()
            .filter(|(_, entry)| entry.root == root)
            .map(|(key, entry)| (entry.sequence, *key))
            .collect();
        chain.sort_unstable_by_key(|(sequence, _)| *sequence);
        chain.into_iter().map(|(_, key)| key).collect()
    }

    /// How many popups the model currently holds. Introspection for tests --
    /// mirrors `ssd_rect_count()`.
    pub fn popup_count(&self) -> usize {
        self.popups.len()
    }

    /// Resolve a host to its chain root, or `None` when the host is not
    /// modelled. A popup host resolves through the parent's own recorded
    /// root, so the walk is one step deep however deep the chain is.
    fn popup_root_of_host(&self, host: PopupHost) -> Option<PopupRoot> {
        match host {
            PopupHost::Window(id) => self
                .window_manager
                .get(id)
                .map(|window| PopupRoot::Window(window.id)),
            PopupHost::Layer(id) => self
                .layers
                .contains_key(&id)
                .then_some(PopupRoot::Layer(id)),
            PopupHost::Popup(parent) => self.popups.get(&parent).map(|entry| entry.root),
        }
    }

    /// The model output index `root` sits on, or [`NO_OUTPUT`].
    fn popup_output_of_root(&self, root: PopupRoot) -> u32 {
        match root {
            PopupRoot::Window(id) => self
                .window_manager
                .get(id)
                .and_then(|window| self.output_for_window(window.geometry))
                .unwrap_or(NO_OUTPUT),
            PopupRoot::Layer(id) => self.layers.get(&id).map_or(NO_OUTPUT, |entry| entry.output),
        }
    }

    /// Every distinct chain root with at least one live popup, in a stable
    /// order (by the lowest `sequence` in each chain) so a sweep over them is
    /// deterministic.
    fn popup_roots(&self) -> Vec<PopupRoot> {
        let mut seen: Vec<(u64, PopupRoot)> = Vec::new();
        for entry in self.popups.values() {
            match seen.iter_mut().find(|(_, root)| *root == entry.root) {
                Some((sequence, _)) => *sequence = (*sequence).min(entry.sequence),
                None => seen.push((entry.sequence, entry.root)),
            }
        }
        seen.sort_unstable_by_key(|(sequence, _)| *sequence);
        seen.into_iter().map(|(_, root)| root).collect()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --lib state::tests`
Expected: PASS — every existing `state.rs` unit test plus the three new ones.
`popup_roots` is not called yet; `-D warnings` will flag it as dead code, so
add it in **Task 5** instead and keep Task 3's version out. (Concretely: write
`popup_roots` in Task 5 Step 3, not here — this plan lists it under Task 3's
`Produces` only because it is part of the same registry API surface.)

Run: `cargo test -p icedtea-compositor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/src/state.rs
git commit -m "$(cat <<'EOF'
feat(compositor): the model-side popup registry

PopupHost/PopupRoot/PopupEntry plus State's popups map, creation-ordered
popup_stack, sequence source and focus_before_popup slot, with
record/forget/popup_root/popup_chain over them.

A chain's root is resolved once, at record time, and never re-derived: the
host can die before its popup does. A popup whose host this compositor does
not model is dropped rather than recorded under a fabricated root, and only
a grabbing chain's first popup parks a focus restore target.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Placement — the constraint box, and hit-testing

**Files:**
- Modify: `compositor/src/state.rs` (add `root_surface_origin`,
  `popup_constraint_box`, `popup_at_point` after Task 3's registry; extend
  `mod tests`)

**Interfaces:**
- Consumes: Task 3's `PopupRoot`/`PopupEntry`/`popups`/`popup_stack`;
  `OutputSurface::usable` (`state.rs:54`); `State::compute_layer_placement`
  (`:1568`); `crate::decoration::{has_ssd, content_rect}`;
  `icedtea_contract::Rectangle::contains`.
- Produces:
  ```rust
  impl State {
      pub fn popup_constraint_box(&self, popup: PopupKey) -> Option<Rectangle>;
      pub fn popup_at_point(&self, point: (i32, i32)) -> Option<PopupKey>;
  }
  ```
  Private companion: `State::root_surface_origin(&self, root: PopupRoot) -> Option<(i32, i32)>`.

- [ ] **Step 1: Write the failing test**

Append to `compositor/src/state.rs`'s `mod tests`:

```rust
    /// The constraint box is the root's output `usable` rect expressed in the
    /// **root surface's own** coordinates -- contract ruling R7, and
    /// `wlr_xdg_popup_unconstrain_from_box`'s own header. For a
    /// server-decorated window that origin is the *content* rect, not the
    /// frame: the client's surface starts one title bar below the frame's top
    /// edge, so a box translated by the frame origin would let a popup ride
    /// `TITLE_BAR_HEIGHT` past the bottom of the screen.
    ///
    /// Mutation check: translate by `w.geometry` instead of
    /// `content_rect(w.geometry, ssd)` and the `y` assertion fails by exactly
    /// `TITLE_BAR_HEIGHT`.
    #[test]
    fn a_popups_constraint_box_is_the_usable_area_in_root_surface_coordinates() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(
            0,
            Rectangle {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        );
        // A 30px top exclusive zone, so `usable` is genuinely smaller than
        // `geometry` and the test cannot pass by reading the wrong one.
        if let Some(output) = state.outputs.get_mut(&0) {
            output.usable = Rectangle {
                x: 0,
                y: 30,
                width: 800,
                height: 570,
            };
        }
        // "app" is not GTK-style, so `has_ssd` gives it a title bar.
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: 100,
                y: 50,
                width: 300,
                height: 200,
            },
        );
        let ssd = crate::decoration::has_ssd("app", None, false);
        assert!(ssd, "this test needs a server-decorated window");
        let content = crate::decoration::content_rect(
            Rectangle {
                x: 100,
                y: 50,
                width: 300,
                height: 200,
            },
            ssd,
        );

        let popup = crate::wayland::PopupKey::for_test(1);
        state.record_popup(popup, PopupHost::Window(window), false);
        assert_eq!(
            state.popup_constraint_box(popup),
            Some(Rectangle {
                x: 0 - content.x,
                y: 30 - content.y,
                width: 800,
                height: 570,
            })
        );

        // A nested popup is constrained against the same root, not against
        // its immediate parent.
        let nested = crate::wayland::PopupKey::for_test(2);
        state.record_popup(nested, PopupHost::Popup(popup), false);
        assert_eq!(
            state.popup_constraint_box(nested),
            state.popup_constraint_box(popup)
        );

        // An unrecorded key, and a popup whose output vanished, are misses.
        assert_eq!(
            state.popup_constraint_box(crate::wayland::PopupKey::for_test(99)),
            None
        );
        state.outputs.remove(&0);
        assert_eq!(state.popup_constraint_box(popup), None);
    }

    /// `popup_at_point` searches the stack back to front, so the newest popup
    /// over a point wins, and answers nothing at all while the session is
    /// locked.
    ///
    /// Mutation check: drop the `.rev()` and the "topmost wins" assertion
    /// fails; drop the `session_locked` gate and the locked assertion fails.
    #[test]
    fn popup_at_point_finds_the_topmost_mapped_popup_and_nothing_while_locked() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(
            0,
            Rectangle {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        );
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: 100,
                y: 50,
                width: 300,
                height: 200,
            },
        );
        let ssd = crate::decoration::has_ssd("app", None, false);
        let content = crate::decoration::content_rect(
            Rectangle {
                x: 100,
                y: 50,
                width: 300,
                height: 200,
            },
            ssd,
        );

        let under = crate::wayland::PopupKey::for_test(1);
        let over = crate::wayland::PopupKey::for_test(2);
        let unmapped = crate::wayland::PopupKey::for_test(3);
        for key in [under, over, unmapped] {
            state.record_popup(key, PopupHost::Window(window), false);
        }
        // All three cover the same root-surface rect; only the first two are
        // mapped.
        let rect = Rectangle {
            x: 10,
            y: 10,
            width: 40,
            height: 40,
        };
        for key in [under, over] {
            if let Some(entry) = state.popups.get_mut(&key) {
                entry.mapped = true;
                entry.geometry = rect;
            }
        }
        if let Some(entry) = state.popups.get_mut(&unmapped) {
            entry.geometry = rect;
        }

        // Frame space: the root surface's origin plus the popup's own offset.
        let inside = (content.x + 20, content.y + 20);
        assert_eq!(
            state.popup_at_point(inside),
            Some(over),
            "the newest popup over the point wins"
        );
        assert_eq!(
            state.popup_at_point((content.x + 200, content.y + 200)),
            None,
            "a point outside every popup hits nothing"
        );

        // Unmapping the top one hands the point to the one below it, not to
        // the unmapped third.
        if let Some(entry) = state.popups.get_mut(&over) {
            entry.mapped = false;
        }
        assert_eq!(state.popup_at_point(inside), Some(under));

        state.session_locked = true;
        assert_eq!(
            state.popup_at_point(inside),
            None,
            "no popup answers input while the session is locked"
        );
    }

    /// Hostile positioner geometry never panics.
    ///
    /// Every number in a `PopupEntry::geometry` originates in an
    /// `xdg_positioner` the client wrote, so `i32::MIN`/`i32::MAX` extents and
    /// origins are reachable from a malicious client, and so is a root whose
    /// own geometry is degenerate. Contract §9: a malformed value is dropped,
    /// never a panic -- in particular the coordinate translation
    /// `point - origin` must not overflow.
    ///
    /// Mutation check: replace the `saturating_sub` in `popup_at_point` with
    /// `-` and this test panics with "attempt to subtract with overflow" in a
    /// debug build.
    #[test]
    fn hostile_popup_geometry_never_panics() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(
            0,
            Rectangle {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        );
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: i32::MIN,
                y: i32::MIN,
                width: 1,
                height: 1,
            },
        );
        let popup = crate::wayland::PopupKey::for_test(1);
        state.record_popup(popup, PopupHost::Window(window), false);
        if let Some(entry) = state.popups.get_mut(&popup) {
            entry.mapped = true;
            entry.geometry = Rectangle {
                x: i32::MIN,
                y: i32::MAX,
                width: i32::MIN,
                height: i32::MAX,
            };
        }

        for point in [
            (0, 0),
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MAX),
            (i32::MIN, i32::MAX),
        ] {
            let _ = state.popup_at_point(point);
        }
        let _ = state.popup_constraint_box(popup);
        let _ = state.popup_chain(PopupRoot::Window(window));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --lib state::tests::a_popups_constraint_box state::tests::popup_at_point state::tests::hostile_popup_geometry`
Expected: FAIL to compile — `error[E0599]: no method named
`popup_constraint_box` found for struct `State`` and the same for
`popup_at_point`.

- [ ] **Step 3: Write the implementation**

`compositor/src/state.rs`, after Task 3's `popup_roots`:

```rust
    /// The origin of `root`'s own `wl_surface`, in frame (output-logical)
    /// space.
    ///
    /// For a window that is the **content** rect, not the frame: a
    /// server-decorated window's client surface starts one `TITLE_BAR_HEIGHT`
    /// below the frame's top edge (`decoration::content_rect`), and the scene
    /// node the popup's subtree hangs off is placed there too. For a layer
    /// surface it is the placement `compute_layer_placement` computes.
    ///
    /// `None`, panic-free, when the root or its output has vanished.
    fn root_surface_origin(&self, root: PopupRoot) -> Option<(i32, i32)> {
        match root {
            PopupRoot::Window(id) => {
                let window = self.window_manager.get(id)?;
                let ssd = crate::decoration::has_ssd(
                    &window.app_id,
                    window.client_decorations_requested,
                    window.fullscreen,
                );
                let content = crate::decoration::content_rect(window.geometry, ssd);
                Some((content.x, content.y))
            }
            PopupRoot::Layer(id) => {
                let (_, _, x, y) = self.compute_layer_placement(id)?;
                Some((x, y))
            }
        }
    }

    /// The constraint box handed to the library's `configure_popup`, **in the
    /// root toplevel/layer surface's coordinate system** (contract ruling R7).
    ///
    /// The box itself is the root's output `usable` rect
    /// ([`OutputSurface::usable`]) -- the same working area tiling and
    /// maximize read, so a panel's menu is clamped exactly where a window
    /// would be -- translated by minus the root's surface origin.
    ///
    /// `None` if the popup is unknown, or its root or that root's output is
    /// gone (including the [`NO_OUTPUT`] sentinel, which never resolves).
    pub fn popup_constraint_box(&self, popup: crate::wayland::PopupKey) -> Option<Rectangle> {
        let entry = self.popups.get(&popup)?;
        let (origin_x, origin_y) = self.root_surface_origin(entry.root)?;
        let usable = self.outputs.get(&entry.output)?.usable;
        Some(Rectangle {
            x: usable.x.saturating_sub(origin_x),
            y: usable.y.saturating_sub(origin_y),
            width: usable.width,
            height: usable.height,
        })
    }

    /// The topmost mapped popup whose last-known geometry contains `point`
    /// (frame space), or `None`.
    ///
    /// Searched back to front over `popup_stack`, so the most recently
    /// created popup over a point wins -- which is also its scene order, since
    /// a popup's subtree is created above its parent's existing children.
    ///
    /// `None` while the session is locked, matching every other model path
    /// that answers input: the library's own hit test is already rooted at the
    /// lock band while locked (contract §1.5), and this is the model-side
    /// half of the same rule.
    pub fn popup_at_point(&self, point: (i32, i32)) -> Option<crate::wayland::PopupKey> {
        if self.session_locked {
            return None;
        }
        self.popup_stack.iter().rev().copied().find(|key| {
            let Some(entry) = self.popups.get(key) else {
                return false;
            };
            if !entry.mapped {
                return false;
            }
            let Some((origin_x, origin_y)) = self.root_surface_origin(entry.root) else {
                return false;
            };
            // Saturating: both the point and the origin are ultimately
            // client-controlled, and `Rectangle::contains` on a saturated
            // coordinate is merely wrong, not a crash.
            entry.geometry.contains(
                point.0.saturating_sub(origin_x),
                point.1.saturating_sub(origin_y),
            )
        })
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --lib state::tests`
Expected: PASS — every existing test plus the three new ones (six popup unit
tests total so far).

Run: `cargo test -p icedtea-compositor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/src/state.rs
git commit -m "$(cat <<'EOF'
feat(compositor): popup placement box and model-side hit-testing

popup_constraint_box is the root's output usable rect expressed in the root
surface's own coordinates, which for a server-decorated window means the
content rect rather than the frame -- ruling R7, and the difference is
exactly one title bar of popup riding off the bottom of the screen.

popup_at_point searches the creation-ordered stack back to front and answers
nothing while the session is locked. Both saturate their coordinate
arithmetic: every number involved came from a client-written positioner.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Handlers, focus rules and reconstrain hooks

**Files:**
- Modify: `compositor/src/state.rs` (add `configure_popup_now`,
  `refresh_popup_geometry`, `reconstrain_popups`, `restore_focus_after_popups`,
  `runtime_has_explicit_grab`, `popup_roots`, `forget_popups_of_root` after
  Task 4's placement code; add the fourth early return to `sync_seat_focus`
  (`:2288`); add the reconstrain hook at the tail of `sync_window_to_scene`
  (`:2124`) and of `arrange_layers` (`:1534`); prune popups in
  `forget_toplevel` (`:4734`) and `layer_surface_destroyed` (`:5996`); add the
  six `ToplevelHandler` popup methods next to the layer ones (`:6010`);
  rewrite the two doc paragraphs at `:912` and `:6497`; extend `mod tests`)

**Interfaces:**
- Consumes: Task 2's `Wayland::{configure_popup, popup_geometry,
  popup_is_reactive, has_explicit_grab}`; Task 3's registry; Task 4's
  `popup_constraint_box`; contract §1.3's six defaulted handler methods.
- Produces:
  ```rust
  impl State {
      pub fn reconstrain_popups(&mut self, root: PopupRoot);
      fn restore_focus_after_popups(&mut self);
      fn runtime_has_explicit_grab(&self) -> bool;
      fn configure_popup_now(&mut self, popup: PopupKey);
      fn refresh_popup_geometry(&mut self, popup: PopupKey);
      fn forget_popups_of_root(&mut self, root: PopupRoot);
      fn popup_roots(&self) -> Vec<PopupRoot>;
      fn popup_host_for(&self, parent: wlr::PopupParent) -> Option<PopupHost>;
  }
  impl wlr::ToplevelHandler for State {
      fn new_popup(&mut self, popup: &wlr::Popup<'_>);
      fn popup_initial_commit(&mut self, popup: &wlr::Popup<'_>);
      fn popup_mapped(&mut self, id: wlr::PopupId);
      fn popup_unmapped(&mut self, id: wlr::PopupId);
      fn popup_reposition(&mut self, popup: &wlr::Popup<'_>);
      fn popup_destroyed(&mut self, id: wlr::PopupId);
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `compositor/src/state.rs`'s `mod tests`:

```rust
    /// A grabbing chain parks its root and hands focus back to it when the
    /// last popup dies -- to the **parent**, not to whatever the pointer
    /// wandered onto while the menu was up (spec §2: "on destroy, focus
    /// returns to the parent, not the pointer position").
    ///
    /// Mutation check: delete the `restore_focus_after_popups()` call from
    /// `popup_destroyed` and the final `focused_id()` assertion reports `b`.
    #[test]
    fn a_grabbing_chains_end_returns_focus_to_its_root_not_to_the_pointer() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let a = state.window_manager.add_window(
            "a.app",
            "a",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 200,
                height: 200,
            },
        );
        let b = state.window_manager.add_window(
            "b.app",
            "b",
            2,
            Rectangle {
                x: 300,
                y: 0,
                width: 200,
                height: 200,
            },
        );
        state.window_manager.focus(a);

        let outer = crate::wayland::PopupKey::for_test(1);
        let inner = crate::wayland::PopupKey::for_test(2);
        state.record_popup(outer, PopupHost::Window(a), true);
        state.record_popup(inner, PopupHost::Popup(outer), false);
        assert_eq!(
            state.focus_before_popup,
            Some(PopupRoot::Window(a)),
            "the chain's first, grabbing popup parks its root"
        );

        // While the chain is up, something else takes model focus -- the
        // click that dismisses it lands on another window.
        state.window_manager.focus(b);
        assert_eq!(state.focused_id(), Some(b));

        // The chain unwinds deepest-first, as the protocol requires.
        state.popup_destroyed_for_test(inner);
        assert_eq!(
            state.focused_id(),
            Some(b),
            "focus is restored only when the chain has fully emptied"
        );
        state.popup_destroyed_for_test(outer);
        assert_eq!(state.focused_id(), Some(a));
        assert_eq!(
            state.focus_before_popup, None,
            "the restore target is taken, so a later chain cannot inherit it"
        );
    }

    /// A non-grabbing chain never moves keyboard focus -- not when it opens,
    /// and not when it ends (contract deviation D3).
    ///
    /// Mutation check: park `focus_before_popup` unconditionally in
    /// `record_popup` (drop the `grabbing &&`) and the final assertion
    /// reports `a`.
    #[test]
    fn a_non_grabbing_chain_never_moves_focus_at_either_end() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let a = state.window_manager.add_window(
            "a.app",
            "a",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 200,
                height: 200,
            },
        );
        let b = state.window_manager.add_window(
            "b.app",
            "b",
            2,
            Rectangle {
                x: 300,
                y: 0,
                width: 200,
                height: 200,
            },
        );
        state.window_manager.focus(b);

        let popup = crate::wayland::PopupKey::for_test(1);
        state.record_popup(popup, PopupHost::Window(a), false);
        assert_eq!(state.focus_before_popup, None);
        assert_eq!(state.focused_id(), Some(b), "opening moved nothing");

        state.popup_destroyed_for_test(popup);
        assert_eq!(state.focused_id(), Some(b), "closing moved nothing");
    }

    /// A root's death takes its whole popup chain with it, however many
    /// per-popup destroys the library gets round to delivering.
    ///
    /// Mutation check: delete the `forget_popups_of_root` call from
    /// `forget_toplevel` and `popup_count()` stays at 2.
    #[test]
    fn a_dying_root_prunes_every_popup_under_it() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let window = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 200,
                height: 200,
            },
        );
        let toplevel = crate::wayland::ToplevelKey::for_test(1);
        state.wayland.bind(window, toplevel);
        let outer = crate::wayland::PopupKey::for_test(1);
        let inner = crate::wayland::PopupKey::for_test(2);
        state.record_popup(outer, PopupHost::Window(window), true);
        state.record_popup(inner, PopupHost::Popup(outer), false);
        assert_eq!(state.popup_count(), 2);

        state.forget_toplevel(toplevel);
        assert_eq!(state.popup_count(), 0);
        assert!(state.popup_stack.is_empty());
        assert_eq!(
            state.focus_before_popup, None,
            "a restore target whose root is gone is dropped, not left to \
             re-focus a dead window"
        );

        // And a layer root behaves the same way.
        let layer = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            layer,
            LayerEntry {
                output: NO_OUTPUT,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::LayerAnchor::empty(),
                exclusive: 0,
                size: (0, 0),
                interactive: false,
                mapped: false,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        let on_panel = crate::wayland::PopupKey::for_test(3);
        state.record_popup(on_panel, PopupHost::Layer(layer), false);
        assert_eq!(state.popup_count(), 1);
        wlr::ToplevelHandler::layer_surface_destroyed(&mut state, layer);
        assert_eq!(state.popup_count(), 0);
    }
```

`LayerEntry`'s literal above must match the struct as it stands; if a field has
been added since, copy the shape from `new_layer_surface`'s own insert
(`state.rs:5836`) rather than inventing values. `state.popup_destroyed_for_test`
is a thin test-only shim, added in Step 3, that calls the trait method — the
trait method itself takes a `wlr::PopupId`, which a `PopupKey` wraps.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --lib state::tests::a_grabbing_chains_end state::tests::a_non_grabbing_chain state::tests::a_dying_root`
Expected: FAIL to compile — `error[E0599]: no method named
`popup_destroyed_for_test` found for struct `State``, and (once that is added)
FAIL at runtime on `assert_eq!(state.focused_id(), Some(a))` — `left: Some(b)`,
`right: Some(a)` — because nothing restores focus yet.

- [ ] **Step 3: Write the implementation**

**(a) Placement/focus helpers**, after Task 4's `popup_at_point`:

```rust
    /// Unconstrain `popup` against its current constraint box and answer it
    /// with a configure. A no-op on any miss -- no runtime, no entry, no
    /// output, or a surface that is not `initialized` yet.
    fn configure_popup_now(&mut self, popup: crate::wayland::PopupKey) {
        let Some(constraint) = self.popup_constraint_box(popup) else {
            return;
        };
        if !self.wayland.configure_popup(popup, constraint) {
            tracing::trace!(
                ?popup,
                "popup not configured yet -- not initialized, or already gone"
            );
        }
    }

    /// Re-read where `popup` actually is and store it. See
    /// `Wayland::popup_geometry` for why this is committed rather than
    /// scheduled state.
    fn refresh_popup_geometry(&mut self, popup: crate::wayland::PopupKey) {
        let Some(geometry) = self.wayland.popup_geometry(popup) else {
            return;
        };
        if let Some(entry) = self.popups.get_mut(&popup) {
            entry.geometry = geometry;
        }
    }

    /// Re-run unconstrain + configure for every **reactive** popup under
    /// `root`, parents before children.
    ///
    /// Called from the two places a root's surface can move under a live
    /// popup: `sync_window_to_scene` (every geometry push, including each
    /// frame of a drag) and `arrange_layers` (a panel appearing, resizing or
    /// leaving changes every output's usable area, which is the constraint
    /// box itself). Non-reactive popups are left alone: the client did not
    /// ask to be re-placed, and re-placing one would move a menu out from
    /// under the pointer.
    pub fn reconstrain_popups(&mut self, root: PopupRoot) {
        for popup in self.popup_chain(root) {
            if !self.wayland.popup_is_reactive(popup) {
                continue;
            }
            self.configure_popup_now(popup);
            self.refresh_popup_geometry(popup);
        }
    }

    /// Hand keyboard focus back to the chain's root once a grabbing chain has
    /// fully emptied.
    ///
    /// Only a grabbing chain ever parks a target (deviation D3), so this is a
    /// no-op for tooltips and non-modal popovers. Points focus at the
    /// **parent**, not at the pointer position: the click that dismissed a
    /// menu commonly lands on some other window, and inheriting that as the
    /// new focus is exactly the behaviour spec §2 rules out.
    ///
    /// A root that died while its chain was up (the window closed under the
    /// menu) drops the target and re-derives the seat from the model instead.
    fn restore_focus_after_popups(&mut self) {
        let Some(root) = self.focus_before_popup.take() else {
            return;
        };
        match root {
            PopupRoot::Window(id) if self.window_manager.get(id).is_some() => {
                let previous = self.focused_id();
                // An explicit toplevel-focus assertion, so `layer_focus` is
                // released first -- see `release_layer_focus`'s own doc.
                self.release_layer_focus();
                if self.window_manager.focus(id).is_some() {
                    self.sync_focus_change(previous);
                }
                self.emit_pending();
            }
            PopupRoot::Layer(id) if self.layers.contains_key(&id) => {
                if let Some(runtime) = self.wayland.runtime()
                    && runtime.focus_layer_keyboard(id).is_some()
                {
                    self.layer_focus = Some(id);
                } else {
                    self.sync_seat_focus();
                }
            }
            _ => self.sync_seat_focus(),
        }
    }

    /// Whether wlroots currently has an explicit seat grab up -- an xdg-popup
    /// grab or a drag-and-drop grab.
    fn runtime_has_explicit_grab(&self) -> bool {
        self.wayland.has_explicit_grab()
    }

    /// Drop every popup under `root`, and any focus-restore target pointing
    /// at it.
    ///
    /// wlroots frees a dying parent's popups with it, and the per-popup
    /// `destroy` signals may or may not reach a handler first; the model has
    /// to be correct either way. Called from `forget_toplevel` and
    /// `layer_surface_destroyed`.
    fn forget_popups_of_root(&mut self, root: PopupRoot) {
        for popup in self.popup_chain(root) {
            self.forget_popup(popup);
        }
        if self.focus_before_popup == Some(root) {
            self.focus_before_popup = None;
        }
    }
```

plus Task 3's `popup_roots` (written here, where it acquires its first caller),
and:

```rust
    /// Translate the library's `PopupParent` into the model's `PopupHost`.
    /// `None` when the parent is not something this compositor models.
    fn popup_host_for(&self, parent: wlr::PopupParent) -> Option<PopupHost> {
        match parent {
            wlr::PopupParent::Toplevel(id) => self
                .wayland
                .window_for(crate::wayland::ToplevelKey::new(id))
                .map(PopupHost::Window),
            wlr::PopupParent::Layer(id) => self
                .layers
                .contains_key(&id)
                .then_some(PopupHost::Layer(id)),
            wlr::PopupParent::Popup(id) => {
                let key = crate::wayland::PopupKey::new(id);
                self.popups.contains_key(&key).then_some(PopupHost::Popup(key))
            }
        }
    }

    /// Test-only shim: drive `ToplevelHandler::popup_destroyed` from a
    /// `PopupKey` without the caller needing to unwrap the library id.
    #[cfg(test)]
    fn popup_destroyed_for_test(&mut self, popup: crate::wayland::PopupKey) {
        wlr::ToplevelHandler::popup_destroyed(self, popup.0);
    }
```

**(b) `sync_seat_focus`'s fourth early return** — inserted immediately after
the `session_locked` gate (`state.rs:2291`) and **before** the
`layer_holds_keyboard_focus` gate (`:2316`), per contract §2.2 focus rule 1:

```rust
        // Focus rule 1 (contract §2.2): while an explicit seat grab is up --
        // an xdg-popup grab, a drag-and-drop grab -- the seat's keyboard is
        // wlroots' to route, not the model's. wlroots gives the popup chain
        // the keyboard itself and restores the pre-grab focus when the grab
        // ends, and every `wlr_seat_keyboard_notify_enter` this method would
        // make is routed *through* that grab anyway. Reasserting the model's
        // toplevel focus here would be the same churn the `layer_focus` guard
        // below already exists to prevent, one grab further out.
        if self.runtime_has_explicit_grab() {
            return;
        }
```

**(c) The two reconstrain hooks.** At the very end of `sync_window_to_scene`
(after `self.sync_seat_focus();`, `:2124`):

```rust
        // A window that moved or resized moved its popups' constraint box
        // with it. Reactive popups asked to be re-placed when that happens;
        // the rest are left where the client put them.
        self.reconstrain_popups(PopupRoot::Window(id));
```

At the very end of `arrange_layers` (after `self.emit_pending();`, `:1534`):

```rust
        // Every output's `usable` rect was just recomputed, and that rect *is*
        // the popup constraint box -- a panel appearing or leaving re-places
        // every reactive popup on that output, whatever it hangs off.
        for root in self.popup_roots() {
            self.reconstrain_popups(root);
        }
```

**(d) The two prunes.** In `forget_toplevel`, after `let Some(id) =
self.wayland.window_for(toplevel) else { return; };` and **before**
`self.wayland.forget(id)`:

```rust
        // The window's popups die with it; the model must not outlive them.
        self.forget_popups_of_root(PopupRoot::Window(id));
```

In `layer_surface_destroyed`, as its first statement:

```rust
        self.forget_popups_of_root(PopupRoot::Layer(id));
```

**(e) The six handler methods**, in `impl wlr::ToplevelHandler for State`,
immediately after `layer_surface_destroyed` (`:6010`):

```rust
    /// A client created a popup. Resolve its host, record it, and place it.
    ///
    /// The configure attempted here is normally a no-op: the popup's surface
    /// is not `initialized` until its first commit, and the library skips the
    /// call rather than tripping wlroots' own assert (contract §1.2). It is
    /// made anyway so that a popup which *is* already initialized -- a
    /// reposition racing a re-announce -- is placed at once rather than a
    /// round trip later. `popup_initial_commit` is what actually lands the
    /// constraint box on the client's first configure.
    fn new_popup(&mut self, popup: &wlr::Popup<'_>) {
        let key = crate::wayland::PopupKey::new(popup.id());
        let Some(host) = self.popup_host_for(popup.parent()) else {
            tracing::debug!(?key, "popup on a parent this compositor does not model");
            return;
        };
        self.record_popup(key, host, popup.grab_requested());
        self.configure_popup_now(key);
    }

    /// The popup's first commit: unconstrain before the library answers it.
    ///
    /// Deviation D2. xdg-shell requires the compositor to answer a popup's
    /// first commit or it never maps, and the library does that
    /// unconditionally right after this returns (contract §1.7) -- so this is
    /// the one moment at which the compositor's constraint box can reach the
    /// client's *first* configure rather than its second.
    fn popup_initial_commit(&mut self, popup: &wlr::Popup<'_>) {
        self.configure_popup_now(crate::wayland::PopupKey::new(popup.id()));
    }

    /// The popup now has a buffer on screen: it starts answering
    /// `popup_at_point`, and its committed geometry is finally readable.
    fn popup_mapped(&mut self, id: wlr::PopupId) {
        let key = crate::wayland::PopupKey::new(id);
        if let Some(entry) = self.popups.get_mut(&key) {
            entry.mapped = true;
        }
        self.refresh_popup_geometry(key);
    }

    /// The popup is no longer displayed. Not a destroy -- the entry survives,
    /// mirroring `layer_surface_unmapped`.
    fn popup_unmapped(&mut self, id: wlr::PopupId) {
        if let Some(entry) = self.popups.get_mut(&crate::wayland::PopupKey::new(id)) {
            entry.mapped = false;
        }
    }

    /// The client sent `xdg_popup.reposition` with a new positioner: re-run
    /// placement against the current constraint box. The library sends
    /// `xdg_popup.repositioned` with the client's token off the configure
    /// this triggers -- the compositor forges nothing.
    fn popup_reposition(&mut self, popup: &wlr::Popup<'_>) {
        let key = crate::wayland::PopupKey::new(popup.id());
        if !self.popups.contains_key(&key) {
            return;
        }
        self.configure_popup_now(key);
        self.refresh_popup_geometry(key);
    }

    /// The popup is gone for good. Panic-free on an id this handler was never
    /// told about -- contract §1.3's caveat, the same posture
    /// `layer_surface_destroyed` takes.
    ///
    /// When this empties the chain, focus goes back to the chain's root
    /// (focus rule 3, deviation D3).
    fn popup_destroyed(&mut self, id: wlr::PopupId) {
        let key = crate::wayland::PopupKey::new(id);
        let Some(root) = self.popup_root(key) else {
            return;
        };
        self.forget_popup(key);
        if self.popup_chain(root).is_empty() {
            self.restore_focus_after_popups();
        }
    }
```

**(f) The two doc rewrites** (contract §8.3's last row). `state.rs:912`'s
paragraph currently describes a popup as a hypothetical; rewrite the sentence
that begins "a menu/tooltip/combo popup positioned at its own coordinates" so
it points at the registry — e.g. append: *"Native xdg-popups are modelled
separately, by `State::popups`/`PopupEntry`: they hang off a `PopupHost`, are
placed against `popup_constraint_box`, and never enter `WindowManager` either."*
`state.rs:6497`'s "popups ... are real `wl_surface`s, take pointer focus
normally" note gains: *"See `State::popup_at_point` and the popup handlers
below for the model side of that."* Both are additive clarifications; do not
delete the existing prose.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --lib state::tests`
Expected: PASS — every existing unit test plus nine popup ones.

Run: `cargo test -p icedtea-compositor --test client_protocol a_popup_grab_still_dismisses_on_a_click_outside_it`
Expected: **FAIL**, and this is the contract §8.3 event the existing test was
written to produce: `a popup was configured: the `wlr` crate has grown
xdg-popup support, so this test should now map the popup properly rather than
relying on the grab alone`. The compositor now answers the popup's first
commit, so its `!popup_configured()` assertion is false.

Record that failure, then invert the assertion **in this commit**, keeping the
whole press/serial/click-outside sequence and the `popup_done` assertion
verbatim (§8.3). At this point `popup_configured()` still returns `bool`, so
the inversion is:

```rust
    a.open_grabbing_popup(serial, 64, 48);
    assert!(
        a.wait_until(|c| c.popup_configured()),
        "the popup was never configured: the compositor must answer a popup's \
         first commit or it can never legally attach a buffer and map"
    );
```

Task 7 finishes the rewrite when `popup_configured()` becomes
`Option<(i32, i32, i32, i32)>` and `popup_dismissed()` becomes `popup_done()`.
The suite must not be red across a commit boundary, and §8.3 already fixes
exactly what this rewrite may change.

Run: `cargo test -p icedtea-compositor`
Expected: PASS — every integration test, including the rewritten one.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/src/state.rs compositor/tests/client_protocol.rs
git commit -m "$(cat <<'EOF'
feat(compositor): xdg-popup handlers, focus rules and reconstrain hooks

The six defaulted ToplevelHandler popup methods, wired to the registry:
new_popup resolves the host and places the popup, popup_initial_commit
unconstrains before the library answers the first commit (without it the
client's first configure carries the raw positioner geometry), map/unmap
track on-screen state, reposition re-places, destroy prunes and -- when the
chain empties -- hands focus back to the chain's root rather than to
whatever the dismissing click landed on.

sync_seat_focus gains a fourth early return: while an explicit seat grab is
up, the keyboard is wlroots' to route. Reactive popups are re-placed from
sync_window_to_scene and arrange_layers, the two places a root's surface or
its output's usable area can move underneath one.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: The harness popup chain

**Files:**
- Modify: `harness/src/lib.rs` (the popup fields on `ClientState` at `:843-848`;
  `PopupRole` at `:1125`; the two `Dispatch` impls at `:1103`/`:1144`;
  `PopupHandles` at `:1836`; `TestClient::popup` → `popups` at `:1901`;
  `TestClient::map` at `:2365`; `open_grabbing_popup`/`popup_configured`/
  `popup_dismissed` at `:2620-2692`; `detach` at `:2623`)

**Interfaces:**
- Consumes: Task 1's `PopupSpec`, `PopupAnchor`, `PopupGravity`,
  `PopupConstraint`; `create_shm_buffer` (`lib.rs:2125`); `FALLBACK_SIZE`;
  `TIMEOUT`.
- Produces:
  ```rust
  impl TestClient {
      pub fn open_popup(&mut self, spec: PopupSpec);
      pub fn open_popup_from_popup(&mut self, spec: PopupSpec);
      pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)>;
      pub fn popup_configured_at(&self, depth: usize) -> Option<(i32, i32, i32, i32)>;
      pub fn popup_done(&self) -> bool;
      pub fn popup_depth(&self) -> usize;
      pub fn destroy_popup(&mut self);
      pub fn open_grabbing_popup(&mut self, serial: u32, w: i32, h: i32);
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `compositor/tests/popups.rs` (this is contract §2.4's first test; it
also proves the harness chain works, so the harness gets its gate here rather
than in a harness-local unit test — there is no compositor to talk to from
`harness`'s own `mod tests`):

```rust
/// A popup under a toplevel is configured at exactly the geometry its
/// positioner asks for, when nothing constrains it.
///
/// `(10, 10, 20, 20)` anchored `BottomLeft` with gravity `BottomRight` puts
/// the popup's top-left corner at `(10, 30)` in the parent's window-geometry
/// coordinates -- the arithmetic
/// `the_wlr_crate_exposes_the_xdg_popup_api_part_2_is_written_against` pins
/// against wlroots itself. The window is nowhere near an output edge, so no
/// constraint adjustment can apply and the configure must be the raw geometry.
///
/// Mutation check: make `State::configure_popup_now` return early and the
/// popup is never configured, so `open_popup` times out waiting to map.
#[test]
fn a_popup_under_a_toplevel_is_configured_at_the_positioner_geometry() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(
        a.wait_until(|c| c.last_configure().is_some()),
        "the parent never configured"
    );

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );

    assert_eq!(
        a.popup_configured(),
        Some((10, 30, 64, 48)),
        "the popup must be configured at the unconstrained positioner geometry"
    );
    assert_eq!(a.popup_depth(), 1);
    assert!(!a.popup_done(), "nothing dismissed this popup");

    a.destroy_popup();
    assert_eq!(a.popup_depth(), 0);
    a.detach();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --test popups a_popup_under_a_toplevel`
Expected: FAIL to compile — `error[E0599]: no method named `open_popup` found
for struct `TestClient`` (and `popup_depth`, `destroy_popup`), and
`error[E0308]: mismatched types` on `a.popup_configured()` — it still returns
`bool`.

- [ ] **Step 3: Write the implementation**

**(a) `ClientState`'s popup fields** — replace the `popup_configured: bool`
field (`lib.rs:848`) and keep `popup_done` (`:845`):

```rust
    /// The geometry each live popup's last `xdg_popup.configure` carried,
    /// indexed by depth -- `[0]` is the outermost popup of the chain.
    ///
    /// A `Vec` rather than a scalar because a nested menu chain has one
    /// configure per level and every level is separately assertable
    /// (`popup_configured_at`).
    popup_geometries: Vec<Option<(i32, i32, i32, i32)>>,
    /// Whether each depth's popup `xdg_surface.configure` has arrived, which
    /// is what says that popup may legally attach a buffer and map.
    popup_acked: Vec<bool>,
    /// The token echoed by the most recent `xdg_popup.repositioned`.
    popup_repositioned: Option<u32>,
```

Initialise all three in the same place `popup_done` is initialised (the
`ClientState` literal inside `connect_and_bind`): `popup_geometries:
Vec::new(), popup_acked: Vec::new(), popup_repositioned: None,`.

Add a small helper next to them, used by both `Dispatch` impls:

```rust
impl ClientState {
    /// Grow `popup_geometries`/`popup_acked` so `depth` is addressable.
    fn ensure_popup_depth(&mut self, depth: usize) {
        if self.popup_geometries.len() <= depth {
            self.popup_geometries.resize(depth + 1, None);
        }
        if self.popup_acked.len() <= depth {
            self.popup_acked.resize(depth + 1, false);
        }
    }
}
```

**(b) `PopupRole` carries the depth** (`lib.rs:1125`):

```rust
/// Marker user-data for a popup's `xdg_surface` **and** its `xdg_popup`,
/// carrying the popup's depth in its client's chain (`0` = outermost).
///
/// Without the marker both roles would share `Dispatch<XdgSurface, ()>` and a
/// popup configure would advance [`TestClient::configure_count`], quietly
/// breaking every test that waits on that counter to observe a toplevel
/// change. The depth is what lets a nested chain's levels be asserted
/// separately.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PopupRole(pub(crate) usize);
```

and the two `Dispatch` impls:

```rust
impl Dispatch<xdg_surface::XdgSurface, PopupRole> for ClientState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        role: &PopupRole,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Acked here for the same reason the toplevel's is -- see that impl.
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.ensure_popup_depth(role.0);
            state.popup_acked[role.0] = true;
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, PopupRole> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        role: &PopupRole,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                state.ensure_popup_depth(role.0);
                state.popup_geometries[role.0] = Some((x, y, width, height));
            }
            // Latched, never cleared: a dismissed popup is gone for good, and
            // a chain is dismissed whole.
            xdg_popup::Event::PopupDone => state.popup_done = true,
            xdg_popup::Event::Repositioned { token } => {
                state.popup_repositioned = Some(token)
            }
            _ => {}
        }
    }
}
```

**(c) `PopupHandles` grows a buffer** (`lib.rs:1836`). Keep the leak-caveat
paragraph (`:1839-1844`) **verbatim** — contract §2.3 requires it — and replace
only the last paragraph ("No buffer or pool: the popup is never configured...")
with a description of the real thing:

```rust
/// The objects backing one `xdg_popup`, held so the popup outlives the call
/// that made it.
///
/// Dropping these fields does *not* destroy the popup: `wayland-client`
/// 0.31 proxies have no `Drop` impl that sends a protocol destroy request,
/// so simply letting a `PopupHandles` go out of scope leaks the objects
/// server-side until the connection itself closes. To retire a popup
/// deliberately, call [`PopupHandles::destroy`] (or go through
/// [`TestClient::detach`], which does so for you). `popup_done` -- what
/// [`TestClient::popup_done`] watches for -- is sent by the compositor
/// on its own initiative; nothing this struct does triggers it.
///
/// The popup is driven all the way to mapped, so it owns a real shm buffer
/// at the size the compositor configured it to.
pub(crate) struct PopupHandles {
    popup: xdg_popup::XdgPopup,
    xdg_surface: xdg_surface::XdgSurface,
    surface: wl_surface::WlSurface,
    positioner: xdg_positioner::XdgPositioner,
    buffer: wl_buffer::WlBuffer,
    pool: wl_shm_pool::WlShmPool,
    /// The shm file stays open for as long as the pool refers to it.
    _shm_file: std::fs::File,
}

impl PopupHandles {
    /// Destroy every object backing this popup, child-to-parent as
    /// xdg-shell requires: the buffer and its pool, then the `xdg_popup`
    /// role object, then its `xdg_surface`, then the `wl_surface` it was
    /// assigned to, then the `xdg_positioner` that placed it.
    pub(crate) fn destroy(self) {
        self.buffer.destroy();
        self.pool.destroy();
        self.popup.destroy();
        self.xdg_surface.destroy();
        self.surface.destroy();
        self.positioner.destroy();
    }
}
```

**(d) `TestClient::popup` becomes a chain.** The field (`lib.rs:1901`):

```rust
    /// Every live popup this client has opened, outermost first -- the chain
    /// `open_popup`/`open_popup_from_popup` build and `destroy_popup` unwinds.
    /// `TestClient::detach` destroys them topmost-first, as xdg-shell requires.
    popups: Vec<PopupHandles>,
```

`TestClient::map`'s literal (`lib.rs:2380`): `popup: None,` → `popups:
Vec::new(),`.

`detach` (`lib.rs:2623`): replace the `if let Some(popup) = this.popup.take()`
block with

```rust
        // Topmost-first: xdg-shell requires a popup's children destroyed
        // before it.
        while let Some(popup) = this.popups.pop() {
            popup.destroy();
        }
```

**(e) The popup API**, replacing `open_grabbing_popup`, `popup_configured` and
`popup_dismissed` (`lib.rs:2620-2692`):

```rust
    /// Build the `xdg_positioner` `spec` describes.
    fn positioner_for(&self, spec: PopupSpec) -> xdg_positioner::XdgPositioner {
        let wm_base = self
            .state
            .wm_base
            .clone()
            .expect("compositor did not advertise xdg_wm_base");
        let positioner = wm_base.create_positioner(&self.qh, ());
        let (w, h) = spec.size;
        positioner.set_size(w, h);
        let (ax, ay, aw, ah) = spec.anchor_rect;
        positioner.set_anchor_rect(ax, ay, aw, ah);
        positioner.set_anchor(spec.anchor);
        positioner.set_gravity(spec.gravity);
        positioner.set_constraint_adjustment(spec.constraint_adjustment);
        positioner.set_offset(spec.offset.0, spec.offset.1);
        if spec.reactive {
            positioner.set_reactive();
        }
        positioner
    }

    /// Open an `xdg_popup` on this client's toplevel and drive it to mapped.
    ///
    /// Sequence, exactly as xdg-shell requires: positioner → `get_popup` →
    /// optional `grab` (before the first commit) → an empty commit → wait for
    /// `xdg_popup.configure` + `xdg_surface.configure` → ack (inside the
    /// `Dispatch` impl) → attach a real shm buffer at the configured size →
    /// commit.
    ///
    /// Panics if a popup from this client is already live (use
    /// [`TestClient::open_popup_from_popup`] for a nested one), on a
    /// non-positive size, or if the compositor never configures within
    /// [`TIMEOUT`].
    pub fn open_popup(&mut self, spec: PopupSpec) {
        assert!(
            self.popups.is_empty(),
            "open_popup called with a previous popup still live -- destroy it \
             first (TestClient::destroy_popup) or use open_popup_from_popup"
        );
        let parent = self.xdg_surface.clone();
        self.push_popup(&parent, spec);
    }

    /// Open a child popup of this client's current topmost popup, so a test
    /// can build a nested menu chain.
    ///
    /// Panics if there is no popup to hang it off.
    pub fn open_popup_from_popup(&mut self, spec: PopupSpec) {
        let parent = self
            .popups
            .last()
            .expect("open_popup_from_popup with no popup open")
            .xdg_surface
            .clone();
        self.push_popup(&parent, spec);
    }

    /// The shared body of both `open_popup` entry points.
    fn push_popup(&mut self, parent: &xdg_surface::XdgSurface, spec: PopupSpec) {
        let (w, h) = spec.size;
        assert!(
            w > 0 && h > 0,
            "popup size ({w}, {h}) must be positive -- xdg-shell rejects a \
             zero-sized positioner outright"
        );
        let depth = self.popups.len();
        let compositor = self
            .state
            .compositor
            .clone()
            .expect("compositor did not advertise wl_compositor");
        let shm = self
            .state
            .shm
            .clone()
            .expect("compositor did not advertise wl_shm");
        let wm_base = self
            .state
            .wm_base
            .clone()
            .expect("compositor did not advertise xdg_wm_base");

        let positioner = self.positioner_for(spec);
        let surface = compositor.create_surface(&self.qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &self.qh, PopupRole(depth));
        let popup = xdg_surface.get_popup(Some(parent), &positioner, &self.qh, PopupRole(depth));
        if let Some(serial) = spec.grab {
            let seat = self.state.seat.clone().expect("no wl_seat");
            // Before the first commit, per xdg-shell.
            popup.grab(&seat, serial);
        }
        self.state.ensure_popup_depth(depth);
        self.state.popup_geometries[depth] = None;
        self.state.popup_acked[depth] = false;
        surface.commit();
        self.conn.flush().expect("flush popup create");

        // `roundtrip`, not `blocking_dispatch`, for the reason `map` gives:
        // a roundtrip always returns, so the deadline stays honest against a
        // compositor that has nothing to say.
        let deadline = Instant::now() + TIMEOUT;
        while !(self.state.popup_acked[depth] && self.state.popup_geometries[depth].is_some()) {
            assert!(
                Instant::now() < deadline,
                "no xdg_popup.configure + xdg_surface.configure within {TIMEOUT:?}"
            );
            self.queue
                .roundtrip(&mut self.state)
                .expect("popup configure roundtrip");
        }

        let (_, _, cw, ch) = self.state.popup_geometries[depth].expect("just checked above");
        // A configure of 0x0 would mean the compositor chose nothing; the
        // positioner's own size is the honest fallback.
        let (bw, bh) = if cw > 0 && ch > 0 { (cw, ch) } else { (w, h) };
        let (shm_file, pool, buffer) = create_shm_buffer(&shm, &self.qh, bw, bh);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, bw, bh);
        surface.commit();
        self.conn.flush().expect("flush popup map");
        self.queue
            .roundtrip(&mut self.state)
            .expect("popup map roundtrip");

        self.popups.push(PopupHandles {
            popup,
            xdg_surface,
            surface,
            positioner,
            buffer,
            pool,
            _shm_file: shm_file,
        });
    }

    /// `(x, y, width, height)` from the outermost popup's last
    /// `xdg_popup.configure`, in the parent's window-geometry coordinates.
    /// `None` until one has arrived.
    pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)> {
        self.popup_configured_at(0)
    }

    /// Per-depth: `popup_configured_at(0)` is the outermost popup.
    pub fn popup_configured_at(&self, depth: usize) -> Option<(i32, i32, i32, i32)> {
        self.state.popup_geometries.get(depth).copied().flatten()
    }

    /// Whether *any* popup of this client has received
    /// `xdg_popup.popup_done`. Latched: a chain is dismissed whole.
    pub fn popup_done(&self) -> bool {
        self.state.popup_done
    }

    /// How many popups this client currently has open.
    pub fn popup_depth(&self) -> usize {
        self.popups.len()
    }

    /// Destroy the topmost popup -- the reverse-creation order xdg-shell
    /// requires. A no-op when there is none.
    pub fn destroy_popup(&mut self) {
        if let Some(popup) = self.popups.pop() {
            popup.destroy();
            self.conn.flush().expect("flush popup destroy");
            let _ = self.queue.roundtrip(&mut self.state);
        }
    }

    /// Open a grabbing popup at the default positioner.
    ///
    /// Kept for `client_protocol.rs`'s implicit-vs-explicit grab regression
    /// test, which predates [`PopupSpec`]; it is exactly
    /// `open_popup(PopupSpec::new(w, h).grab(serial))`.
    ///
    /// `serial` must be one the seat issued this client's `wl_pointer` --
    /// [`TestClient::last_pointer_serial`] after a button press.
    pub fn open_grabbing_popup(&mut self, serial: u32, w: i32, h: i32) {
        self.open_popup(PopupSpec::new(w, h).grab(serial));
    }
```

`wl_buffer` and `wl_shm_pool` are already imported in `lib.rs`; `Instant` and
`TIMEOUT` likewise.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 2 tests.

Run: `cargo test -p icedtea-compositor --test client_protocol`
Expected: FAIL on `a_popup_grab_still_dismisses_on_a_click_outside_it` —
`error[E0308]: mismatched types` (`popup_configured()` is now
`Option<(i32,i32,i32,i32)>`) and `error[E0599]: no method named
`popup_dismissed``. Finish the §8.3 rewrite in this commit: change the
inverted assertion to `c.popup_configured().is_some()` and the final one to
`c.popup_done()`, leaving every other line of that test untouched.

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings
git add harness/src/lib.rs compositor/tests/popups.rs compositor/tests/client_protocol.rs
git commit -m "$(cat <<'EOF'
feat(harness): a real xdg-popup client, chained

TestClient owns a Vec<PopupHandles> instead of one never-mapped popup: each
level gets its own surface, positioner, shm buffer and PopupRole(depth)
user-data, and open_popup drives the whole positioner -> get_popup -> [grab]
-> commit -> configure -> ack -> attach -> commit handshake. Nested chains
hang off the previous level's xdg_surface.

popup_configured now reports the geometry the compositor chose rather than a
bool that was only ever false, and popup_dismissed is renamed popup_done.
client_protocol.rs's grab regression test keeps its whole sequence with the
negative configure assertion inverted, per contract 8.3.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Reposition, layer-panel popups, and the harness doc rewrites

**Files:**
- Modify: `harness/src/lib.rs` (add `reposition_popup`/`popup_repositioned` to
  `TestClient`; add `qh` and `popup` to `LayerPanelClient` at `:3152` and its
  `spawn` at `:3178`; add `LayerPanelClient::{open_popup, popup_configured,
  popup_done}`; rewrite the module-level doc paragraph that claims `wlr` has no
  xdg-popup support)

**Interfaces:**
- Consumes: Task 6's `PopupHandles`, `PopupRole`, `ClientState`'s popup fields,
  `TestClient::positioner_for`.
- Produces:
  ```rust
  impl TestClient {
      pub fn reposition_popup(&mut self, spec: PopupSpec, token: u32);
      pub fn popup_repositioned(&self) -> Option<u32>;
  }
  impl LayerPanelClient {
      pub fn open_popup(&mut self, spec: PopupSpec);
      pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)>;
      pub fn popup_done(&self) -> bool;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `compositor/tests/popups.rs` — contract §2.4's fourth and tenth
tests, written here because they are what proves this task's two additions:

```rust
/// A popup opened by a layer-shell panel is clamped to the same working area
/// windows get: the panel's own output `usable` rect, which the panel's
/// exclusive zone has already carved.
///
/// The panel is top-anchored with a 28px exclusive zone at the output origin,
/// so its surface origin is `(0, 0)` and the constraint box in its own
/// coordinates starts at `y = 28`. A popup anchored at `(0, 0, 1, 1)` would
/// land at `y = 1`, inside the panel's own strip; with `SLIDE_Y` it must be
/// pushed down to the usable area and stay entirely on the output.
///
/// Mutation check: make `State::popup_constraint_box` read
/// `outputs[..].geometry` instead of `.usable` and the `>= 28` assertion
/// fails -- the popup is allowed to sit under the panel.
#[test]
fn a_popup_under_a_layer_panel_is_constrained_to_the_same_output() {
    let comp = Compositor::spawn();
    let (_ow, oh) = comp.output_size();
    let mut panel = icedtea_harness::map_layer_panel(&comp.socket, 28);
    assert!(
        panel.wait_until(|c| c.layer_configure().is_some()),
        "the panel never configured"
    );

    panel.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(0, 0, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY),
    );

    let (px, py, pw, ph) = panel
        .popup_configured()
        .expect("the panel's popup was never configured");
    assert_eq!((pw, ph), (64, 48), "the popup was slid, not resized");
    assert!(
        py >= 28,
        "the popup must be pushed below the panel's own exclusive zone, got \
         y = {py} (panel-surface coordinates, usable starts at 28)"
    );
    assert!(
        py + ph <= oh,
        "the popup must stay on the output: y = {py}, h = {ph}, output \
         height = {oh}"
    );
    assert_eq!(px, 0, "nothing constrains x, so it stays at the anchor");
}

/// `xdg_popup.reposition` re-runs placement and echoes the client's token
/// back through `xdg_popup.repositioned`.
///
/// The token is opaque and the compositor forges nothing: wlroots emits
/// `repositioned` off the configure the compositor's own re-placement
/// triggers, so an echoed token *is* the proof that placement re-ran.
///
/// Mutation check: make `State::popup_reposition` return before
/// `configure_popup_now` and neither the new geometry nor the token arrives.
#[test]
fn a_reposition_request_reconfigures_and_echoes_the_token() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_configured(), Some((10, 30, 64, 48)));
    assert_eq!(a.popup_repositioned(), None, "nothing has repositioned yet");

    const TOKEN: u32 = 0x1234_5678;
    a.reposition_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(50, 50, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
        TOKEN,
    );

    assert!(
        a.wait_until(|c| c.popup_repositioned() == Some(TOKEN)),
        "the reposition token was never echoed back"
    );
    assert_eq!(
        a.popup_configured(),
        Some((50, 70, 64, 48)),
        "the popup must be reconfigured at the new positioner's geometry"
    );

    a.detach();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --test popups a_popup_under_a_layer_panel a_reposition_request`
Expected: FAIL to compile — `error[E0599]: no method named `open_popup` found
for struct `LayerPanelClient`` (and `popup_configured` on it), and no method
named `reposition_popup` / `popup_repositioned` on `TestClient`.

- [ ] **Step 3: Write the implementation**

**(a) `TestClient` reposition**, after `destroy_popup`:

```rust
    /// Send `xdg_popup.reposition(positioner, token)` for the topmost popup.
    ///
    /// The old positioner is destroyed and replaced: xdg-shell discards every
    /// parameter the previous one set, so keeping it alive would only leak an
    /// object. Panics if there is no popup open.
    ///
    /// This only *sends* the request; the answer arrives asynchronously as
    /// `xdg_popup.repositioned` + `xdg_popup.configure`. Wait for it with
    /// `wait_until(|c| c.popup_repositioned() == Some(token))`.
    pub fn reposition_popup(&mut self, spec: PopupSpec, token: u32) {
        assert!(
            !self.popups.is_empty(),
            "reposition_popup with no popup open"
        );
        let positioner = self.positioner_for(spec);
        let handles = self.popups.last_mut().expect("just checked above");
        handles.popup.reposition(&positioner, token);
        let previous = std::mem::replace(&mut handles.positioner, positioner);
        previous.destroy();
        self.conn.flush().expect("flush reposition");
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// The token echoed by the most recent `xdg_popup.repositioned`, or
    /// `None` if no reposition has completed.
    pub fn popup_repositioned(&self) -> Option<u32> {
        self.state.popup_repositioned
    }
```

**(b) `LayerPanelClient` gains a queue handle and a popup.** Add two fields to
the struct (`lib.rs:3152`):

```rust
    /// Kept so popups can be created after `spawn` returns -- every proxy
    /// this client makes later needs it.
    qh: QueueHandle<ClientState>,
    /// The popup opened by [`LayerPanelClient::open_popup`], if any. Same
    /// caveat as [`PopupHandles`]: dropping it does not destroy it.
    popup: Option<PopupHandles>,
```

and to `spawn`'s returned literal: `qh, popup: None,`.

**(c) `LayerPanelClient::open_popup`**, after `layer_configure_count`
(`lib.rs:3326`):

```rust
    /// Open an `xdg_popup` parented to this panel and drive it to mapped.
    ///
    /// The layer-shell dance, per `zwlr_layer_shell_v1`'s own protocol xml:
    /// the popup is created with `xdg_surface.get_popup(None, …)` -- a NULL
    /// xdg parent -- and then reparented with
    /// `zwlr_layer_surface_v1.get_popup`, before the popup's initial commit.
    ///
    /// Panics if a popup is already live, on a non-positive size, or if the
    /// compositor never configures within [`TIMEOUT`].
    pub fn open_popup(&mut self, spec: PopupSpec) {
        assert!(
            self.popup.is_none(),
            "open_popup called with a previous popup still live"
        );
        let (w, h) = spec.size;
        assert!(w > 0 && h > 0, "popup size ({w}, {h}) must be positive");

        let compositor = self.state.compositor.clone().expect("no wl_compositor");
        let shm = self.state.shm.clone().expect("no wl_shm");
        let wm_base = self
            .state
            .wm_base
            .clone()
            .expect("compositor did not advertise xdg_wm_base");

        let positioner = wm_base.create_positioner(&self.qh, ());
        positioner.set_size(w, h);
        let (ax, ay, aw, ah) = spec.anchor_rect;
        positioner.set_anchor_rect(ax, ay, aw, ah);
        positioner.set_anchor(spec.anchor);
        positioner.set_gravity(spec.gravity);
        positioner.set_constraint_adjustment(spec.constraint_adjustment);
        positioner.set_offset(spec.offset.0, spec.offset.1);
        if spec.reactive {
            positioner.set_reactive();
        }

        let surface = compositor.create_surface(&self.qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &self.qh, PopupRole(0));
        // NULL xdg parent, then reparented onto the layer surface.
        let popup = xdg_surface.get_popup(None, &positioner, &self.qh, PopupRole(0));
        self.layer_surface.get_popup(&popup);
        if let Some(serial) = spec.grab {
            let seat = self.state.seat.clone().expect("no wl_seat");
            popup.grab(&seat, serial);
        }
        self.state.ensure_popup_depth(0);
        self.state.popup_geometries[0] = None;
        self.state.popup_acked[0] = false;
        surface.commit();
        self.conn.flush().expect("flush panel popup create");

        let deadline = Instant::now() + TIMEOUT;
        while !(self.state.popup_acked[0] && self.state.popup_geometries[0].is_some()) {
            assert!(
                Instant::now() < deadline,
                "no popup configure for a layer-shell popup within {TIMEOUT:?}"
            );
            self.queue
                .roundtrip(&mut self.state)
                .expect("panel popup configure roundtrip");
        }

        let (_, _, cw, ch) = self.state.popup_geometries[0].expect("just checked above");
        let (bw, bh) = if cw > 0 && ch > 0 { (cw, ch) } else { (w, h) };
        let (shm_file, pool, buffer) = create_shm_buffer(&shm, &self.qh, bw, bh);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, bw, bh);
        surface.commit();
        self.conn.flush().expect("flush panel popup map");
        self.queue
            .roundtrip(&mut self.state)
            .expect("panel popup map roundtrip");

        self.popup = Some(PopupHandles {
            popup,
            xdg_surface,
            surface,
            positioner,
            buffer,
            pool,
            _shm_file: shm_file,
        });
    }

    /// `(x, y, width, height)` from this panel's popup's last
    /// `xdg_popup.configure`, in the **panel surface's** coordinates.
    pub fn popup_configured(&self) -> Option<(i32, i32, i32, i32)> {
        self.state.popup_geometries.first().copied().flatten()
    }

    /// Whether this panel's popup has received `xdg_popup.popup_done`.
    pub fn popup_done(&self) -> bool {
        self.state.popup_done
    }
```

**(d) The two harness doc rewrites** (contract §8.3). The module-level
paragraph at `lib.rs:2668-2676` (now attached to nothing, since the old
`popup_configured` doc it lived in is gone) and any other prose asserting "the
`wlr` crate has no xdg-popup support yet" must be replaced with a description
of the working API. Find them with:

```bash
grep -n 'no xdg-popup support\|executable record of the gap\|never mapped' harness/src/lib.rs
```

and rewrite each hit to describe what is now true — the compositor answers a
popup's initial commit, so a popup configures, attaches and maps like any other
surface, and a *grab* remains a seat-level wlroots construct independent of
that. Do **not** delete the `PopupHandles` leak caveat (`:1839-1844`); contract
§2.3 requires it verbatim and Task 6 already preserved it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 4 tests.

Run: `cargo test --workspace`
Expected: PASS.

Run: `grep -rn 'no xdg-popup support\|executable record of the gap' harness/src/lib.rs compositor/src/state.rs`
Expected: no output — every claim §8.3 marks false is gone.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings
git add harness/src/lib.rs compositor/tests/popups.rs
git commit -m "$(cat <<'EOF'
feat(harness): popup reposition, and layer-shell popups

reposition_popup swaps the topmost popup's positioner and sends the client's
token; popup_repositioned reads back what the compositor echoed.

LayerPanelClient can open a popup the way the layer-shell protocol requires:
xdg_surface.get_popup with a NULL parent, then
zwlr_layer_surface_v1.get_popup, before the initial commit -- which is also
the only path on which the compositor can classify a layer parent at all.

The docs that called xdg-popup support missing, and popup_configured "the
executable record of the gap", are rewritten: they describe a working API
now. The PopupHandles leak caveat is preserved verbatim.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Constraint adjustment and nested chains

Contract §2.4 fixes the twelve names; keep them in the file in that listed
order, inserting each new test at its contract position rather than appending.
Tasks 6 and 7 already landed names 1, 4 and 10.

**Files:**
- Modify: `compositor/tests/popups.rs` (add contract §2.4's names 2, 3 and 5;
  remove the `#![allow(unused_imports)]` Task 1 added)

**Interfaces:**
- Consumes: Task 6's `TestClient::{open_popup, open_popup_from_popup,
  popup_configured, popup_configured_at, popup_depth}`;
  `icedtea_compositor::decoration::{has_ssd, content_rect}`;
  `Compositor::{snapshot, output_size}`.
- Produces: nothing — tests only.

- [ ] **Step 1: Write the failing test**

Add a shared helper at the top of `compositor/tests/popups.rs`, under the
`BTN_LEFT` constant:

```rust
/// The parent's **content** rect -- the origin of the `wl_surface` a popup's
/// coordinates are expressed against, which for a server-decorated window is
/// one title bar below the frame's top edge.
///
/// Read from the model's own snapshot rather than assumed, because the
/// compositor chooses the placement: a test that hardcoded a position would
/// silently stop testing constraint adjustment the day the layout changed.
fn content_rect_of(comp: &Compositor, app_id: &str) -> Rectangle {
    let geometry = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == app_id)
        .unwrap_or_else(|| panic!("{app_id} is not in the model"))
        .geometry;
    let ssd = icedtea_compositor::decoration::has_ssd(app_id, None, false);
    icedtea_compositor::decoration::content_rect(geometry, ssd)
}
```

and `use icedtea_contract::Rectangle;` to the imports. Then the three tests:

```rust
/// A popup that would hang off the bottom of the output is flipped to the
/// other side of its anchor when the client allowed `FLIP_Y`.
///
/// The anchor rect sits 100px above the bottom of the usable area and the
/// popup is 200 tall, so the unadjusted placement overflows by 101. Flipping
/// swaps anchor `Bottom*` for `Top*` and gravity `Bottom*` for `Top*`, which
/// puts the popup's *bottom* on the anchor point: `y = anchor_y - height`.
///
/// Mutation check: pass `PopupConstraint::empty()` instead and the popup is
/// configured at `ay + 1` -- proving the assertion below really measures the
/// adjustment rather than the raw geometry.
#[test]
fn a_popup_that_would_leave_the_output_is_flipped_by_the_constraint_adjustment() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let content = content_rect_of(&comp, "popup.app");
    let (_ow, oh) = comp.output_size();

    const H: i32 = 200;
    // 100px of room left below the anchor point: the popup overflows by 101.
    let ay = oh - content.y - 100;
    assert!(
        ay > H,
        "this output ({oh}px tall, content at y = {}) is too short to \
         distinguish a flip from a clamp",
        content.y
    );

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::FlipY),
    );

    assert_eq!(
        a.popup_configured(),
        Some((0, ay - H, 64, H)),
        "a flipped popup puts its bottom edge on the anchor point"
    );
    a.detach();
}

/// A popup that cannot flip is slid back inside the usable area instead.
///
/// Same geometry as the flip test, with only `SLIDE_Y` allowed: the popup
/// keeps its size and its side of the anchor, and moves up just far enough to
/// fit. The assertions are the property `SLIDE_Y` promises -- "no longer
/// constrained, same size" -- rather than a hardcoded offset, so the test
/// measures the behaviour rather than one implementation's arithmetic.
///
/// Mutation check: pass `PopupConstraint::empty()` and the "fits" assertion
/// fails by exactly the 101px overflow.
#[test]
fn a_popup_that_cannot_flip_is_slid_back_inside_the_usable_area() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let content = content_rect_of(&comp, "popup.app");
    let (_ow, oh) = comp.output_size();

    const H: i32 = 200;
    let ay = oh - content.y - 100;
    assert!(ay > H, "output too short for this test");

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY),
    );

    let (px, py, pw, ph) = a.popup_configured().expect("the popup never configured");
    assert_eq!((pw, ph), (64, H), "a slid popup keeps its size");
    assert_eq!(px, 0, "nothing constrains x");
    assert!(
        py < ay + 1,
        "the popup must have moved up from the unadjusted {} to fit",
        ay + 1
    );
    // The constraint box's bottom edge, in the parent surface's coordinates.
    assert!(
        py + ph <= oh - content.y,
        "the slid popup must end up inside the usable area: y = {py}, \
         h = {ph}, bottom = {}",
        oh - content.y
    );
    a.detach();
}

/// Every level of a nested chain is configured against **its own parent**, not
/// against the chain's root.
///
/// A submenu's positioner is written in its parent popup's coordinates; if the
/// compositor answered in root coordinates instead, the second level here
/// would come back offset by the first level's `(10, 30)`.
///
/// Mutation check: resolve `PopupHost::Popup` to the chain root in
/// `new_popup` (drop the parent-scoped `new_popup` listener's answer and
/// re-parent everything to the root) and the inner assertion reports
/// `(15, 45, 32, 24)`.
#[test]
fn a_nested_popup_chain_configures_every_level_relative_to_its_parent() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );

    assert_eq!(a.popup_depth(), 2);
    assert_eq!(a.popup_configured_at(0), Some((10, 30, 64, 48)));
    assert_eq!(
        a.popup_configured_at(1),
        Some((5, 15, 32, 24)),
        "the submenu is placed in its parent popup's coordinates"
    );
    assert!(!a.popup_done(), "nothing dismissed this chain");

    // Unwinds in the order xdg-shell requires, without a protocol error.
    a.destroy_popup();
    assert_eq!(a.popup_depth(), 1);
    a.destroy_popup();
    assert_eq!(a.popup_depth(), 0);
    a.detach();
}
```

- [ ] **Step 2: Run test to verify it fails**

Before writing any implementation, confirm these fail for the *right* reason.
Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS for all six. These three tests need **no new production code** —
they exercise paths Tasks 3–7 already built.

If any of them fails, that is a real defect in Tasks 3–7, not a missing
feature. Diagnose it there:
- a configure that never arrives ⇒ `popup_initial_commit` (Task 5) is not
  reached, or `popup_constraint_box` returned `None` (check
  `PopupEntry::output` against `NO_OUTPUT`);
- a raw, unadjusted geometry ⇒ the constraint box is wrong; log it and compare
  against `PositionerRules::unconstrain_box` computed in-test;
- an inner popup offset by the outer's origin ⇒ `popup_host_for` mapped
  `PopupParent::Popup` to the root instead of the parent.

- [ ] **Step 3: Write the implementation**

None beyond deleting Task 1's temporary attribute: remove
`#![allow(unused_imports)]` from the top of `compositor/tests/popups.rs`. Every
import it silenced is now used except `SessionLockClient`,
`VirtualKeyboardClient`, `VirtualPointerClient`, `Duration` and `BTN_LEFT`,
which Tasks 9 and 10 use — so do the deletion in **Task 10 Step 3** instead and
leave the attribute in place here. (Stated explicitly so no one deletes it
early and turns Task 9 red on `-D warnings`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 6 tests.

Run three times in a row; all three green (contract §9's determinism gate,
applied early so a flake is caught while the file is small):

```bash
for i in 1 2 3; do cargo test -p icedtea-compositor --test popups || break; done
```

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/tests/popups.rs
git commit -m "$(cat <<'EOF'
test(compositor): popup constraint adjustment and nested chains

Flip and slide are measured against the parent's real content rect and the
real output size, read back from the model rather than assumed, so the tests
keep measuring the adjustment if the layout ever changes. The nested test
pins the thing the parent-scoped new_popup listener exists for: a submenu is
placed in its parent popup's coordinates, not the chain root's.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Grab dismissal and the focus rules

**Files:**
- Modify: `compositor/tests/popups.rs` (add contract §2.4's names 6, 7 and 8,
  plus four shared helpers)

**Interfaces:**
- Consumes: Task 6's popup API; `VirtualPointerClient::{motion_absolute,
  button, frame}`; `TestClient::{pointer_enters, pointer_buttons,
  last_pointer_serial}`; `Compositor::{snapshot, output_size}`;
  `icedtea_contract::Rectangle::contains`.
- Produces: nothing — tests and test helpers only.

- [ ] **Step 1: Write the failing test**

Add the helpers under `content_rect_of`:

```rust
/// Move the pointer to `point`, press and release `BTN_LEFT`.
fn click_at(comp: &Compositor, vp: &mut VirtualPointerClient, point: (i32, i32)) {
    let (ow, oh) = comp.output_size();
    vp.motion_absolute(point.0 as f64, point.1 as f64, ow as u32, oh as u32);
    vp.frame();
    vp.button(BTN_LEFT, true);
    vp.frame();
    vp.button(BTN_LEFT, false);
    vp.frame();
}

/// Press and release inside `client`'s surface at `point`, and return the
/// serial the press minted -- the serial an `xdg_popup.grab` must cite.
///
/// Waits for the client's own `enter` and `button` rather than sleeping: the
/// serial does not exist until the press has actually been delivered.
fn mint_pointer_serial(
    comp: &Compositor,
    vp: &mut VirtualPointerClient,
    client: &mut TestClient,
    point: (i32, i32),
) -> u32 {
    let (ow, oh) = comp.output_size();
    let enters = client.pointer_enters();
    vp.motion_absolute(point.0 as f64, point.1 as f64, ow as u32, oh as u32);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_enters() > enters),
        "the client never got wl_pointer.enter at {point:?}"
    );
    let buttons = client.pointer_buttons().len();
    vp.button(BTN_LEFT, true);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, true))),
        "the client never got the press that mints the grab serial"
    );
    let serial = client
        .last_pointer_serial()
        .expect("the press just delivered a serial");
    let buttons = client.pointer_buttons().len();
    vp.button(BTN_LEFT, false);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, false))),
        "the client never got the release"
    );
    serial
}

/// The `app_id` of whatever the model currently says is focused.
fn focused_app_id(comp: &Compositor) -> Option<String> {
    comp.snapshot()
        .windows
        .iter()
        .find(|w| w.focused)
        .map(|w| w.app_id.clone())
}

/// Poll the model's focus for up to `window`, returning whether it ever became
/// `app_id`. A generous bound, not a wall-clock pin: focus changes here are
/// driven by deferred handler events, so the only honest assertion is
/// "eventually".
fn wait_for_focus(comp: &Compositor, app_id: &str, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    loop {
        if focused_app_id(comp).as_deref() == Some(app_id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
```

and `use std::time::Duration;` (already imported by Task 1). Then the tests:

```rust
/// One click outside a grabbing chain dismisses the **whole** chain, not just
/// its topmost level.
///
/// wlroots owns this: the compositor installs no grab of its own (contract
/// §1.6), so what is under test is that the compositor still does the two
/// things the popup grab needs from it -- clearing pointer focus on an
/// out-of-client enter, and notifying the button -- with its own implicit
/// pointer grab in the way.
///
/// Mutation check: skip `notify_button` under an explicit grab and
/// `popup_done` never arrives. (This is the same mutation
/// `client_protocol.rs`'s grab regression test records; re-run it here after
/// the popup actually maps, since a mapped popup takes a different pointer
/// focus path than the never-mapped one that test used to open.)
#[test]
fn a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    let inside = (
        content.x + content.width / 2,
        content.y + content.height / 2,
    );
    let serial = mint_pointer_serial(&comp, &mut vp, &mut a, inside);

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .grab(serial),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_depth(), 2, "both levels mapped");
    assert!(!a.popup_done(), "nothing has dismissed the chain yet");

    // Bare desktop: outside every surface this client owns.
    let (ow, oh) = comp.output_size();
    let outside = (ow - 2, oh - 2);
    let frame = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "popup.app")
        .expect("the parent must be in the model")
        .geometry;
    assert!(
        !frame.contains(outside.0, outside.1),
        "{outside:?} is inside the parent {frame:?}, so the click would not \
         be outside the popup's client"
    );
    click_at(&comp, &mut vp, outside);

    assert!(
        a.wait_until(|c| c.popup_done()),
        "the chain was never dismissed by a click outside it"
    );

    // The client still owns both proxies; destroying them in reverse order
    // after a popup_done must not be a protocol error.
    a.destroy_popup();
    a.destroy_popup();
    a.detach();
}

/// When a grabbing chain ends, keyboard focus goes back to the chain's
/// **parent**, not to whatever the dismissing click landed on.
///
/// Spec §2: "on destroy, focus returns to the parent, not the pointer
/// position." The click that dismisses a menu here lands on a *second*
/// window, so the compositor's own click-to-focus moves model focus to `b`
/// first; `restore_focus_after_popups` is what puts it back on `a`.
///
/// Mutation check: delete the `restore_focus_after_popups()` call from
/// `State::popup_destroyed` and the final assertion reports `b.app`.
#[test]
fn keyboard_focus_returns_to_the_parent_when_a_grabbing_chain_ends() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "a.app", "a");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let mut b = TestClient::map_toplevel(&comp.socket, "b.app", "b");
    assert!(b.wait_until(|c| c.last_configure().is_some()));

    let a_frame = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "a.app")
        .expect("a must be in the model")
        .geometry;
    let b_content = content_rect_of(&comp, "b.app");
    // A point on `b` that `a`'s frame does not cover. The compositor cascades
    // new windows by a fixed step, so this strip exists; assert it rather than
    // assume it, and say why if it ever stops existing.
    let on_b = (
        a_frame.x + a_frame.width + 4,
        b_content.y + b_content.height / 2,
    );
    assert!(
        b_content.contains(on_b.0, on_b.1) && !a_frame.contains(on_b.0, on_b.1),
        "{on_b:?} must be inside b's content {b_content:?} and outside a's \
         frame {a_frame:?} -- the window cascade no longer leaves a strip of \
         b uncovered, so this test needs a different outside point"
    );

    // Click into `a` to focus it and mint the grab serial.
    let a_content = content_rect_of(&comp, "a.app");
    let serial = mint_pointer_serial(
        &comp,
        &mut vp,
        &mut a,
        (
            a_content.x + a_content.width / 2,
            a_content.y + a_content.height / 2,
        ),
    );
    assert!(
        wait_for_focus(&comp, "a.app", Duration::from_secs(5)),
        "clicking into a must focus it"
    );

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .grab(serial),
    );

    click_at(&comp, &mut vp, on_b);
    assert!(
        a.wait_until(|c| c.popup_done()),
        "the click on b never dismissed the chain"
    );

    assert!(
        wait_for_focus(&comp, "a.app", Duration::from_secs(5)),
        "focus must return to the popup's parent, not follow the pointer to \
         b -- currently focused: {:?}",
        focused_app_id(&comp)
    );

    a.destroy_popup();
    a.detach();
    b.detach();
}

/// A non-grabbing popup -- a tooltip, a non-modal popover -- never moves
/// keyboard focus, at either end of its life.
///
/// Focus rule 2, and contract deviation D3's reason for existing: rule 3's
/// restore must not fire for a chain that never took focus, or opening a
/// tooltip on an unfocused window would steal the keyboard when the tooltip
/// closed.
///
/// Mutation check: drop the `grabbing &&` guard in `State::record_popup` and
/// the closing assertion reports `a.app`.
#[test]
fn a_non_grabbing_popup_never_moves_keyboard_focus() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "a.app", "a");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let mut b = TestClient::map_toplevel(&comp.socket, "b.app", "b");
    assert!(b.wait_until(|c| c.last_configure().is_some()));
    assert!(
        wait_for_focus(&comp, "b.app", Duration::from_secs(5)),
        "the most recently mapped window holds focus"
    );

    // `a` -- which is *not* focused -- opens a popup with no grab.
    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert!(a.popup_configured().is_some(), "the popup never configured");
    assert!(
        !wait_for_focus(&comp, "a.app", Duration::from_millis(300)),
        "opening a non-grabbing popup must not move keyboard focus"
    );
    assert_eq!(focused_app_id(&comp).as_deref(), Some("b.app"));

    a.destroy_popup();
    assert!(
        !wait_for_focus(&comp, "a.app", Duration::from_millis(300)),
        "closing a non-grabbing popup must not move keyboard focus either"
    );
    assert_eq!(focused_app_id(&comp).as_deref(), Some("b.app"));

    a.detach();
    b.detach();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --test popups keyboard_focus_returns_to_the_parent`
Expected: PASS if Task 5's `restore_focus_after_popups` is wired correctly.
Before trusting that, run the mutation the doc comment names — comment out the
`self.restore_focus_after_popups();` call in `State::popup_destroyed`, re-run,
and confirm:
`focus must return to the popup's parent, not follow the pointer to b --
currently focused: Some("b.app")`. Restore the call.

Run: `cargo test -p icedtea-compositor --test popups a_non_grabbing_popup`
Expected: PASS. Run its mutation too — drop `grabbing &&` from
`State::record_popup` — and confirm the closing assertion fails with
`Some("a.app")`. Restore.

Run: `cargo test -p icedtea-compositor --test popups a_grabbing_popup_chain`
Expected: PASS.

- [ ] **Step 3: Write the implementation**

None — these three tests exercise Task 5's focus rules and wlroots' own grab.
Any failure is a Task 5 defect:
- the chain is never dismissed ⇒ the popup's `grab` request was sent after its
  first commit (check `push_popup`'s ordering) or the serial was stale;
- focus does not return ⇒ `focus_before_popup` was never parked (the chain's
  first popup did not report `grab_requested()`), or `popup_chain(root)` was
  non-empty at destroy time because `forget_popup` ran after the emptiness
  check;
- focus moves for a non-grabbing popup ⇒ deviation D3's guard is missing.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 9 tests.

```bash
for i in 1 2 3; do cargo test -p icedtea-compositor --test popups || break; done
```
Expected: green three times.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/tests/popups.rs
git commit -m "$(cat <<'EOF'
test(compositor): popup grab dismissal and the two focus rules

A click outside a grabbing chain dismisses the whole chain, with the
implicit pointer grab in the way -- the same regression client_protocol.rs
guards, re-run now that the popup actually maps and so takes a different
pointer-focus path.

The focus tests are written so they can fail: the dismissing click lands on
a second window, so the compositor's own click-to-focus moves focus there
first and only restore_focus_after_popups puts it back. The non-grabbing
twin proves that restore does not fire for a chain that never took focus.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Reactive re-placement, session lock, and teardown

**Files:**
- Modify: `compositor/tests/popups.rs` (add contract §2.4's names 9, 11 and 12;
  delete Task 1's `#![allow(unused_imports)]`)

**Interfaces:**
- Consumes: Task 6/7's popup API; `TestClient::request_move`;
  `SessionLockClient::{spawn, lock, wait_locked, unlock}`;
  `Compositor::{session_locked, settle, snapshot}`.
- Produces: nothing — tests only.

- [ ] **Step 1: Write the failing test**

Add one more helper under `wait_for_focus`:

```rust
/// Poll the model until it holds exactly `count` windows, or `window`
/// elapses. Returns whether it ever did.
fn wait_for_window_count(comp: &Compositor, count: usize, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    loop {
        if comp.snapshot().windows.len() == count {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
```

then the three tests:

```rust
/// A **reactive** popup is re-placed when its parent moves.
///
/// The popup is anchored so that it overflows the bottom of the usable area
/// and has to slide: how far it slides is a pure function of where the parent
/// surface sits, so dragging the parent upward must change the configured `y`.
/// A non-reactive popup would keep the placement it was given, which is why
/// `reconstrain_popups` filters on the flag.
///
/// Mutation check: delete the `reconstrain_popups(PopupRoot::Window(id))` call
/// from `State::sync_window_to_scene` and the `wait_until` below times out --
/// the popup keeps its original `y` for the whole drag.
#[test]
fn a_reactive_popup_is_reconfigured_when_its_parent_moves() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    let (ow, oh) = comp.output_size();

    const H: i32 = 200;
    const DRAG: i32 = 120;
    let ay = oh - content.y - 100;
    assert!(
        ay > H && content.y > DRAG,
        "this output ({ow}x{oh}, content at y = {}) leaves no room to drag \
         the parent upward and still overflow",
        content.y
    );

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY)
            .reactive(true),
    );
    let (_, before, _, _) = a
        .popup_configured()
        .expect("the reactive popup never configured");

    // Press and hold inside the parent, then drive an interactive move: the
    // button must still be down when `xdg_toplevel.move` arrives.
    let start = (
        content.x + content.width / 2,
        content.y + content.height / 2,
    );
    let enters = a.pointer_enters();
    vp.motion_absolute(start.0 as f64, start.1 as f64, ow as u32, oh as u32);
    vp.frame();
    assert!(a.wait_until(|c| c.pointer_enters() > enters));
    let buttons = a.pointer_buttons().len();
    vp.button(BTN_LEFT, true);
    vp.frame();
    assert!(a.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, true))));
    let serial = a.last_pointer_serial().expect("the press minted a serial");

    a.request_move(serial);
    comp.settle();
    vp.motion_absolute(
        start.0 as f64,
        (start.1 - DRAG) as f64,
        ow as u32,
        oh as u32,
    );
    vp.frame();

    assert!(
        a.wait_until(|c| c.popup_configured().map(|g| g.1) != Some(before)),
        "a reactive popup must be re-placed when its parent moves; it kept \
         y = {before}"
    );

    vp.button(BTN_LEFT, false);
    vp.frame();
    comp.settle();

    let moved = content_rect_of(&comp, "popup.app");
    assert!(
        moved.y < content.y,
        "the drag must actually have moved the parent up: {} -> {}",
        content.y,
        moved.y
    );
    let (_, after, _, ph) = a.popup_configured().expect("still configured");
    assert!(
        after + ph <= oh - moved.y,
        "the re-placed popup must still fit inside the usable area: \
         y = {after}, h = {ph}, bottom = {}",
        oh - moved.y
    );

    a.destroy_popup();
    a.detach();
}

/// A popup on a window the lock screen is covering receives no input at all
/// while the session is locked.
///
/// The isolation is the library's -- its hit test is rooted at the lock band
/// while locked, so a popup on a hidden window is unreachable by construction
/// (contract §1.5) -- and the compositor adds no second gate. This test is the
/// executable proof of that claim, with its own positive controls on both
/// sides of the lock so it cannot pass by the popup simply never having taken
/// input.
///
/// Mutation check: the guard lives in `wlr`'s `Runtime::leaf_surface_at`.
/// Deleting its lock branch makes the middle assertion fail. The two controls
/// are what make that mutation the *only* way this test can pass.
#[test]
fn a_popup_on_a_hidden_window_receives_no_input_while_the_session_is_locked() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    let (px, py, pw, ph) = a.popup_configured().expect("the popup never configured");
    // The popup's centre, in output coordinates.
    let on_popup = (content.x + px + pw / 2, content.y + py + ph / 2);

    // Control 1: unlocked, the popup takes clicks.
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        a.wait_until(|c| c.pointer_buttons().len() > buttons),
        "the popup never took a click even before the lock -- the rest of \
         this test would be vacuous"
    );

    let mut locker = SessionLockClient::spawn(&comp.socket);
    locker.lock();
    assert!(locker.wait_locked(), "session never reported locked");
    assert!(comp.session_locked(), "compositor is_session_locked() is false");

    // The load-bearing assertion: nothing reaches the popup while locked.
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        !a.wait_until_timeout(Duration::from_millis(300), |c| c
            .pointer_buttons()
            .len()
            > buttons),
        "a popup behind the lock screen received a click"
    );

    // Control 2: unlocking restores it, so the silence above was the lock and
    // not the popup having quietly gone away.
    locker.unlock();
    assert!(!comp.session_locked(), "still locked after unlock");
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        a.wait_until(|c| c.pointer_buttons().len() > buttons),
        "the popup did not take clicks again after unlocking"
    );

    a.destroy_popup();
    a.detach();
}

/// A client that dies with a live popup chain takes the whole chain down with
/// it, and the compositor survives.
///
/// The client is dropped without destroying anything -- `wayland-client` 0.31
/// proxies send no destroy on drop, so this is a bare socket close, which is
/// the shape a crashed application has. wlroots then frees the parent's scene
/// tree, and every popup subtree hanging off it, recursively; a compositor
/// that also destroyed a popup's own tree would double-free here (contract
/// §1.5's rule, which `forget_toplevel`'s own doc in the `wlr` crate
/// explains).
///
/// Mutation checks, one per repo. `wlr`: reintroduce a
/// `wlr_scene_node_destroy` on the popup's tree in `forget_popup` and the
/// compositor aborts, so every assertion below times out. `icedtea`: delete
/// the `forget_popups_of_root` call from `State::forget_toplevel` and
/// `a_dying_root_prunes_every_popup_under_it` (state.rs's own unit test)
/// fails -- this e2e cannot see the model's popup map, which is exactly why
/// that unit test exists alongside it.
#[test]
fn destroying_a_parent_destroys_its_popup_chain_without_a_double_free() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_depth(), 2);
    assert!(wait_for_window_count(&comp, 1, Duration::from_secs(5)));

    // No `detach`, no `destroy_popup`: the socket just closes.
    drop(a);

    assert!(
        wait_for_window_count(&comp, 0, Duration::from_secs(5)),
        "the compositor never dropped the dead client's window -- it is \
         wedged or gone"
    );

    // Still alive and still serving: a fresh client maps, opens its own popup,
    // and is configured.
    let mut b = TestClient::map_toplevel(&comp.socket, "after.app", "after");
    assert!(
        b.wait_until(|c| c.last_configure().is_some()),
        "the compositor no longer configures toplevels after the teardown"
    );
    b.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(b.popup_configured(), Some((10, 30, 64, 48)));
    b.detach();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-compositor --test popups a_reactive_popup`
Expected: PASS if Task 5's `sync_window_to_scene` hook landed. Run its named
mutation now — comment out the `reconstrain_popups(PopupRoot::Window(id))` call
— re-run and confirm the failure message is `a reactive popup must be
re-placed when its parent moves; it kept y = …`. Restore.

Run: `cargo test -p icedtea-compositor --test popups a_popup_on_a_hidden_window destroying_a_parent`
Expected: PASS. These two assert properties the `wlr` crate provides; their
mutations are cross-repo and recorded in the doc comments rather than run here
(running them means editing P1's worktree, which P2 must not do). Instead
verify non-vacuity locally with the controls the tests already carry: both
`a_popup_on_a_hidden_window…` controls must be green, and
`destroying_a_parent…`'s "fresh client maps" tail must be green.

- [ ] **Step 3: Write the implementation**

None. Delete `#![allow(unused_imports)]` from the top of
`compositor/tests/popups.rs` — every import is now used.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-compositor --test popups`
Expected: PASS — 12 tests, exactly contract §2.4's twelve names plus the Task 1
oracle (13 in total).

Confirm the names match the contract exactly:

```bash
grep -n '^fn [a-z_]*(' compositor/tests/popups.rs
```

Expected: the twelve §2.4 names present, spelled identically, plus
`the_wlr_crate_exposes_the_xdg_popup_api_part_2_is_written_against` and the
six helpers.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
git add compositor/tests/popups.rs
git commit -m "$(cat <<'EOF'
test(compositor): reactive re-placement, lock isolation, client teardown

A reactive popup anchored so it must slide is re-placed when its parent is
dragged: how far it slides is a pure function of where the parent sits, so
the configured y is a real observable of reconstrain_popups.

The lock test carries a positive control on both sides of the lock, so the
silence in the middle can only be the library's lock-band hit test. The
teardown test drops a client with a two-deep chain on a bare socket close --
the shape a crash has -- and proves the compositor still serves afterwards.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Determinism, drop the patch, and close the part

**Files:**
- Modify: `Cargo.toml` (workspace root — remove the `[patch.crates-io]` block
  Task 1 added)

**Interfaces:**
- Consumes: everything Tasks 1–10 produced; `wlr` 0.20.28 **published to
  crates.io** by P1's consent stop.
- Produces: a part that is ready to present.

- [ ] **Step 1: Write the failing test**

No new test. The gate itself is the check, and it starts red on the one thing
Task 1 deliberately left outstanding. Run:

```bash
grep -n 'patch.crates-io' Cargo.toml
```

Expected: the block Task 1 added is still there — which means the workspace is
**not** building against a published crate and cannot be merged.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo publish -p icedtea-compositor --dry-run 2>&1 | head -20`
Expected: FAIL — `error: failed to prepare local package for uploading … the
`[patch]` section … cannot be published`, or the equivalent "patched
dependency" refusal. That is the executable form of "the patch must go".

(If `icedtea-compositor` is not a published crate at all, use
`cargo metadata --format-version 1 | grep -c '"source":null'` instead and
record the count; the point is to have run *something* that observes the patch
before removing it.)

- [ ] **Step 3: Write the implementation**

First confirm P1's publish actually happened — it is a consent stop and may not
have:

```bash
cargo search wlr --limit 1
```

Expected: `wlr = "0.20.28"`. **If it still says `0.20.27`, stop here and report
that P2 is complete but blocked on P1's publish consent.** Do not merge and do
not remove the patch: the branch is correct as it stands, it simply is not
mergeable yet.

Once `0.20.28` is published, delete the whole `[patch.crates-io]` block from
the workspace root `Cargo.toml`, then:

```bash
cargo update -p wlr
```

and confirm `Cargo.lock` names `wlr 0.20.28` from `registry+https://github.com/rust-lang/crates.io-index`:

```bash
grep -A 2 '^name = "wlr"' Cargo.lock
```

- [ ] **Step 4: Run test to verify it passes**

The full part gate, in this order:

```bash
cargo fmt --all --check
cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo test --workspace
for i in 1 2 3; do
  cargo test -p icedtea-compositor --test popups || { echo "FLAKE on run $i"; break; }
done
```

Expected: every command green, and `popups.rs` green on all three runs
(contract §9's P2 determinism gate). A flake is a defect to fix here, not to
report: the usual cause is an assertion that races a deferred handler event —
replace the bare `assert_eq!` with the matching `wait_until`/`wait_for_*`
bound, never with a sleep.

Then the §8.2 gate — confirm P2 changed nothing it must not have:

```bash
git diff --stat develop... -- ui/ contract/
```
Expected: **empty**. P2 touches neither.

```bash
git diff develop... --stat -- compositor/tests/client_protocol.rs
```
Expected: one file, and the diff limited to
`a_popup_grab_still_dismisses_on_a_click_outside_it` — its press/serial/
click-outside sequence and its `popup_done()` assertion unchanged, the negative
configure assertion inverted, per §8.3. Read the diff and confirm it by eye;
this is the one existing test P2 is allowed to rewrite.

Re-run the §8.3 mutation check that test records, and write the result into its
doc comment:

- comment out `wlr_seat_pointer_notify_button` for the explicit-grab case
  (i.e. skip `notify_button` under an explicit grab) in P1's worktree copy of
  `wlr`,
- confirm `a_popup_grab_still_dismisses_on_a_click_outside_it` fails with "the
  popup was never dismissed",
- restore, and record "re-run 2026-08-27 against a mapped popup: still
  non-vacuous" in the test's doc comment.

If P1's worktree is no longer available (the patch was just dropped), do this
step **before** Step 3's deletion and note that ordering in the commit message.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
git add Cargo.toml Cargo.lock compositor/tests/client_protocol.rs
git commit -m "$(cat <<'EOF'
build: consume the published wlr 0.20.28, drop the dev patch

P1 published 0.20.28, so the workspace builds against crates.io again and
the [patch.crates-io] block that carried P2-P8 through development goes
away -- the same shape develop @ 90813cc used for the implicit-grab fix.

Also records the re-run of client_protocol.rs's popup-grab mutation check
now that the popup actually maps: skipping notify_button under an explicit
grab still makes popup_done never arrive.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

Then **stop**. Present the part as ready — twelve `popups.rs` tests green ×3,
every existing compositor and harness test green, the §8.2 gate untouched, the
one §8.3 rewrite scoped and its mutation check re-recorded — and ask whether to
proceed to P3 or to run `/simplify`, `/code-review`, `/optimize` first. **The
merge is a consent stop; do not merge.**

---

## Self-Review

### 1. Spec coverage

Spec §Section 2's **Compositor** and **Harness** paragraphs and contract §2,
line by line:

| Spec / contract requirement | Task |
|---|---|
| On `new_popup`: constraint box = the parent's output usable area | 4 (`popup_constraint_box`), 5 (`new_popup`) |
| "a layer parent's output for panels" | 4 (`root_surface_origin`'s `PopupRoot::Layer` arm), 8 (`a_popup_under_a_layer_panel…`) |
| `unconstrain` | 2 (`Wayland::configure_popup`), 5 (`configure_popup_now`) |
| Record the popup under its parent for z-order and `window_at_point` | 3 (`popups`/`popup_stack`/`sequence`), 4 (`popup_at_point`) |
| "above the parent, below other layer overlays" — falls out of the scene subtree; the model never calls `place_*` | 3 (documented on `popup_stack`) |
| Grabbing popups take keyboard focus (wlroots'; the compositor stands aside) | 5 (`sync_seat_focus`'s fourth early return), 9 (`a_grabbing_popup_chain…`) |
| On destroy, focus returns to the parent, not the pointer position | 5 (`restore_focus_after_popups`), 9 (`keyboard_focus_returns_to_the_parent…`) |
| Session lock: popups on hidden windows get no input | 4 (`popup_at_point`'s lock gate), 10 (`a_popup_on_a_hidden_window…`) |
| Escape dismissal is client-side; the compositor synthesises nothing | 5 — no synthesis anywhere; asserted by omission and stated in `popup_destroyed`'s doc |
| `PopupKey` (§2.1), `for_test` | 2 |
| `PopupHost`, `PopupRoot`, `PopupEntry` and its seven fields (§2.2) | 3 |
| `popups`, `popup_stack`, `next_popup_sequence`, `focus_before_popup` (§2.2) | 3 |
| `popup_constraint_box`, `record_popup`, `forget_popup`, `popup_root`, `popup_chain`, `popup_at_point`, `reconstrain_popups`, `restore_focus_after_popups` (§2.2) | 3, 4, 5 |
| Focus rules 1–5 (§2.2) | 5 (rule 1 in `sync_seat_focus`, rules 2/3 in `record_popup`/`restore_focus_after_popups` per D3, rule 4 by no new gate, rule 5 by no synthesis) |
| Handler impl: host resolution, `record_popup`, constraint box, `configure_popup`; reposition; map/unmap; destroy + chain-empty rule, tolerating an unknown id | 5 |
| `PopupSpec` + its seven builders (§2.3) | 1 |
| `TestClient::{open_popup, open_popup_from_popup, popup_configured, popup_configured_at, popup_done, popup_depth, destroy_popup, open_grabbing_popup}` (§2.3) | 6 |
| `TestClient::{reposition_popup, popup_repositioned}` (§2.3) | 7 |
| `LayerPanelClient::{open_popup, popup_configured, popup_done}` (§2.3) | 7 |
| `PopupHandles` becomes a `Vec` chain, own surface + `BufferPool`, `detach` destroys topmost-first, the 0.31 leak caveat preserved verbatim (§2.3) | 6 |
| `popup_dismissed` → `popup_done`, one call site (§2.3) | 6 |
| The twelve `compositor/tests/popups.rs` names (§2.4), in order | 6 (1), 7 (4, 10), 8 (2, 3, 5), 9 (6, 7, 8), 10 (9, 11, 12) |
| Each runs deterministically ×3 (§2.4, §9) | 8, 9, 10 (per-task), 11 (final) |
| `state.rs`'s own `mod tests` covers `popup_constraint_box`, `popup_at_point`, `popup_chain` over `PopupKey::for_test` ids (§2.4) | 3, 4 |
| §8.1: `popup_dismissed` → `popup_done`; `popup_configured() -> Option<…>` | 6 |
| §8.3: the one `client_protocol.rs` rewrite, its mutation check re-run and recorded | 5 (inversion), 6 (types), 11 (mutation re-run) |
| §8.3: the harness and `state.rs` doc paragraphs rewritten, the leak caveat preserved | 5 (state.rs), 7 (harness) |
| §9 P2 "must not touch": `ui/`, non-popup compositor paths, `Snapshot`/`WindowInfo` | Global Constraints; verified in 11 (`git diff --stat`) |
| §9 P2 gate: twelve tests ×3, every existing test green, mutation re-run recorded | 11 |
| Rulings R5 (parent-scoped `new_popup`) and R7 (root-toplevel-surface space) | 5 (`popup_host_for`'s `Popup` arm proves R5 end to end in test 5), 4 (`popup_constraint_box`'s translation is R7) |
| §9 cross-cutting: untrusted input never panics | 4 (`hostile_popup_geometry_never_panics`), and every popup path `Option`-returning |
| §9 cross-cutting: every load-bearing test records a mutation check | every test's doc comment |
| §9 cross-cutting: timing assertions are complexity bounds | 9, 10 (`wait_for_focus`/`wait_for_window_count`/`wait_until_timeout`; no bare sleep-then-assert) |
| P1 consumed, never modified (§9 "P2 owns … icedtea") | 1 (verification only), 11 (patch drop) |

No gap. Everything else in the M3 spec — the `wlr` crate itself (§Section 2's
**Crate** paragraph, P1), the window/event layer (§3, P3), the reactive
framework (§4, P4), widgets (§5, P5/P6), icons (§6, P7), the gallery gate
(§7, P8) — is out of P2's scope by contract §9 and is untouched here.

### 2. Placeholder scan

Searched for `TBD`, `TODO`, `FIXME`, `implement later`, `fill in`, `add
appropriate`, `handle edge cases`, `similar to Task`, `write tests for the
above`, `etc.` in a code position: **none present**. Every code step carries
complete, compilable code and every test step carries the test body.

Five places give a *derivation* rather than a fixed value; each names the exact
command to run and what to do with its output, in the style M2 Part 2's plan
used for its `grep`-derived constant:

- **Task 1 Step 3** — P1's worktree path comes from `git -C
  /home/joseph/Projects/wlroots-sys worktree list`; the `[patch]` block's
  `path` is whatever that prints. A hardcoded path would be a guess.
- **Task 5 Step 1** — the `LayerEntry` literal in
  `a_dying_root_prunes_every_popup_under_it` must match the struct as it stands
  at execution time; copy the shape from `new_layer_surface`'s own insert
  (`state.rs:5836`) rather than inventing field values.
- **Task 5 Step 3(f)** and **Task 7 Step 3(d)** — the doc paragraphs to rewrite
  are located by `grep -n 'no xdg-popup support\|executable record of the gap'`
  rather than by line number, because Tasks 1–6 will already have shifted those
  lines. The replacement text is specified; only the location is derived.
- **Task 11 Step 2** — the parenthesised fallback when
  `cargo publish --dry-run` does not apply names the exact alternative command.

Two places give a **conditional stop** rather than a workaround, each naming
the condition and the exact action: Task 1 Step 3 (deviation D1's constructors
missing ⇒ stop, report the §10 amendment, do not work around it in `icedtea`)
and Task 11 Step 3 (P1 has not published ⇒ stop, report blocked-on-consent, do
not merge and do not drop the patch).

One place is a deliberate deferral **with its own removal step named**: Task 1
Step 4 adds `#![allow(unused_imports)]` to `popups.rs`; Task 8 Step 3 says
explicitly not to delete it yet and why; Task 10 Step 3 deletes it.

### 3. Type consistency vs the contract

- `PopupKey(pub(crate) wlr::PopupId)`, `PopupKey::new(id) -> Self` (`pub(crate)`),
  `PopupKey::for_test(n: u64) -> Self` — exactly §2.1, derives included
  (`Debug, Clone, Copy, PartialEq, Eq, Hash`).
- `PopupHost::{Window(WindowId), Layer(wlr::LayerSurfaceId), Popup(PopupKey)}`
  and `PopupRoot::{Window(WindowId), Layer(wlr::LayerSurfaceId)}` — exactly
  §2.2, same derives.
- `PopupEntry { host, root, output: u32, sequence: u64, grabbing: bool,
  mapped: bool, geometry: Rectangle }` — exactly §2.2's seven fields, in that
  order, with `output` following `LayerEntry`'s `NO_OUTPUT` convention as its
  doc requires.
- `State` fields `popups: HashMap<PopupKey, PopupEntry>`,
  `popup_stack: Vec<PopupKey>`, `next_popup_sequence: u64`,
  `focus_before_popup: Option<PopupRoot>` — exactly §2.2, all private, placed
  beside `layers`/`layer_focus` as §2.2 says.
- `popup_constraint_box(&self, popup: PopupKey) -> Option<Rectangle>`,
  `record_popup(&mut self, popup, host, grabbing: bool)` (`pub(crate)`),
  `forget_popup(&mut self, popup)` (`pub(crate)`),
  `popup_root(&self, popup) -> Option<PopupRoot>`,
  `popup_chain(&self, root: PopupRoot) -> Vec<PopupKey>`,
  `popup_at_point(&self, point: (i32, i32)) -> Option<PopupKey>`,
  `reconstrain_popups(&mut self, root: PopupRoot)`,
  `restore_focus_after_popups(&mut self)` (private) — every one exactly §2.2,
  visibility included.
- `runtime_has_explicit_grab()` — the name §2.2's focus rule 1 uses verbatim,
  as a private `State` method reading the seam (deviation D4).
- Additions beyond §2.2, all private or `pub(crate)`, permitted by the
  contract's preamble: `popup_count` (introspection, mirrors `ssd_rect_count`),
  `popup_roots`, `popup_root_of_host`, `popup_output_of_root`,
  `root_surface_origin`, `configure_popup_now`, `refresh_popup_geometry`,
  `forget_popups_of_root`, `popup_host_for`, `popup_destroyed_for_test`
  (`#[cfg(test)]`), and `Wayland`'s six seam methods (D4).
- `PopupSpec` — all eight fields and all seven builders exactly §2.3, including
  `grab: Option<u32>` and its "before the first commit" doc.
- `TestClient::{open_popup, open_popup_from_popup, popup_configured() ->
  Option<(i32,i32,i32,i32)>, popup_configured_at(usize) -> Option<…>,
  popup_done() -> bool, popup_depth() -> usize, reposition_popup(PopupSpec,
  u32), popup_repositioned() -> Option<u32>, destroy_popup(),
  open_grabbing_popup(u32, i32, i32)}` — exactly §2.3.
- `LayerPanelClient::{open_popup(PopupSpec), popup_configured() ->
  Option<…>, popup_done() -> bool}` — exactly §2.3.
- Consumed from §1, unchanged: `wlr::PopupId`, `wlr::PopupParent`,
  `wlr::Popup<'_>`, `wlr::PositionerRules` and its two methods,
  `wlr::{PositionerAnchor, PositionerGravity, ConstraintAdjustment}`,
  `wlr::Box2D`, `Runtime::{popup, configure_popup, popup_is_grabbing,
  dismiss_popup, seat_has_explicit_grab, focus_layer_keyboard}`,
  `ToplevelHandler::{new_popup, popup_initial_commit, popup_mapped,
  popup_unmapped, popup_reposition, popup_destroyed}`. The one addition P2
  requires of P1 is deviation D1's `PopupId::dangling_{,nth_}for_test`.
