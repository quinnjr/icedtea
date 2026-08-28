# Pure-Rust GTK-themed UI — M3 Part 1: wlr 0.20.28 — xdg-popup on toplevels and layer surfaces, grab semantics — Implementation Plan

> **Note for agentic workers:** this plan is written to be executed by an agent
> with no memory of the conversation that produced it. Every task is
> self-contained: it names the exact files, the exact code, the exact commands
> and the exact commit message. Execute the tasks **in order**. Do not skip the
> "run it and watch it fail" step — that step is what proves the test is
> testing something. Do not batch commits. If a step's expected output does not
> match what you observe, **stop and report**; do not improvise a fix that the
> plan did not describe.

---

## Contract deviations

The binding contract is
`/home/joseph/Projects/icedtea/docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.
Every name and signature in this plan is taken from it verbatim except the
three items below. Each is recorded here rather than applied silently, and each
must be appended to the contract's §10 as an amendment (Task 15).

- **D1 — `ConstraintAdjustment` is hand-rolled, not `bitflags::bitflags!`.**
  The contract's §1.1 spells it with the `bitflags` macro. The `wlr` crate has
  **no `bitflags` dependency** and refuses one twice in its own source
  (`crates/wlr/src/buffer.rs:386` for `DataPtrAccess`, and
  `render`'s `BufferCaps` before it), with the constants pinned by a test
  instead. Taking a new runtime dependency into a published crate for six bits
  contradicts that standing decision. The public spelling is unchanged —
  `ConstraintAdjustment::SLIDE_X`, `|`, `contains` all read identically at every
  call site — only the definition differs. `#[derive(Debug, Clone, Copy,
  PartialEq, Eq, Hash)] pub struct ConstraintAdjustment(u32)` with six
  associated consts, `BitOr`, `BitOrAssign` and `contains`.
- **D2 — `PopupId::dangling_for_test` / `dangling_nth_for_test` are added.**
  Not in the contract's §1.1, which lists only the derives. They mirror
  `ToplevelId` (`toplevel.rs:36`, `:83`) and `LayerSurfaceId` (`layer.rs:127`)
  exactly, and without them P1's "an unknown id misses rather than
  dereferencing" promise — which `crates/wlr/tests/popups.rs` must assert, as
  `tests/layers.rs:45` does for layer surfaces — has no way to be written by a
  consumer. Purely additive.
- **D3 — an unknown positioner anchor/gravity is mapped to `None` silently, not
  "logged once".** The contract's §1.1 and the spec's §7 both say "logged once".
  This crate binds **no Rust-side logging symbol at all** and says so in its own
  source: `runtime.rs:8386-8389` — *"this crate binds no Rust-side logging
  symbol (wlroots' own `wlr_log` is a `static inline` macro over an unbound
  `_wlr_log`, and the crate deliberately has no `log`/`tracing` dependency)"*.
  The never-panic half of the rule is honoured in full and is directly tested
  (Task 2). The log half is not implementable in P1 without a new dependency;
  the compositor (P2) is where a malformed positioner becomes observable, and it
  has logging.

---

## Goal

Give the `wlr` crate xdg-popup support, released as **0.20.28**, so that the
compositor (P2) and then the toolkit (P3–P8) can put menus, tooltips, dropdowns
and popovers on screen.

Concretely, when this part is done a consumer of `wlr` can:

- be told about a popup created on a **toplevel**, on a **layer surface**, or on
  another **popup**, and learn which of the three it hangs off
  (`ToplevelHandler::new_popup`, `Popup::parent`);
- read the client's positioner (`Popup::positioner_rules`,
  `PositionerRules::{geometry, unconstrain_box}`) without borrowing anything
  that dies when wlroots re-enters;
- place the popup inside a constraint box and answer its configure
  (`Runtime::configure_popup`), including the mandatory answer to its **initial
  commit**, without ever tripping the `assert(surface->initialized)` abort;
- follow the popup through map/unmap/reposition/destroy
  (`popup_mapped`, `popup_unmapped`, `popup_reposition`, `popup_destroyed`);
- walk and dismiss a chain (`Runtime::{popups_of, popup_chain, dismiss_popup,
  dismiss_popups_of}`);
- ask whether a popup asked for a grab (`Runtime::popup_is_grabbing`) and
  whether *some* explicit seat grab is in force
  (`Runtime::seat_has_explicit_grab`) — which is the single fact P2's
  `sync_seat_focus` needs to stay out of wlroots' way.

And crucially, what this part **does not** do: it does not create, inspect, end
or imitate a `wlr_xdg_popup_grab`. wlroots owns that object's entire lifetime.
P1 observes it and gets out of the way. The existing implicit-pointer-grab code
(`grab_still_applies`, `pointer_motion_to_focus`, `on_pointer_button`) and its
test module `mod implicit_grab_tests` (`backend.rs:8517`) are **not touched**,
and `an_explicit_grab_drops_the_implicit_one` (`backend.rs:8574`) staying green
unmodified is the regression signal that popups integrate with the 0.20.27 grab
rather than fight it.

Publishing 0.20.28 to crates.io is a **consent stop** (Task 16).

---

## Architecture

```
                        wlr_xdg_shell.events.new_popup   ← DELIBERATELY UNUSED
                                                            (a layer popup's
                                                             parent is NULL here)

  toplevel.base->events.new_popup ─┐
  layer_surface.events.new_popup ──┼──►  on_new_popup<S>   (backend.rs)
  popup.base->events.new_popup ────┘        │
                                            │  1. PopupId = ensure_id_raw(popup.base->surface->addons)
                                            │  2. parent = the Bound's id slot  (NEVER re-derived)
                                            │  3. tree = wlr_scene_xdg_surface_create(parent_tree, base)
                                            │  4. link 6 listeners, all carrying PopupId in Bound
                                            │  5. Runtime::record_popup(id, raw, tree, parent)
                                            │  6. emit Event::NewPopup(id)
                                            ▼
      base->surface->events.commit  ─► on_popup_commit    ─► PopupInitialCommit + send_configure
      base->surface->events.map     ─► on_popup_map       ─► PopupMapped
      base->surface->events.unmap   ─► on_popup_unmap     ─► PopupUnmapped
      popup->events.reposition      ─► on_popup_reposition─► PopupReposition
      popup->events.destroy         ─► on_popup_destroy   ─► forget_popup, drop listeners,
                                                             PopupDestroyed

   dispatch::Event ──► deliver_all ──► with_popup ──► ToplevelHandler::popup_*
                   └─► deliver (run path) ──► unreachable arm (compile-required)
```

Three structural decisions carry the whole design:

1. **Parent-scoped listeners, not the shell-level one.** A layer-shell popup is
   created by `xdg_surface.get_popup` with `parent == NULL` and only *then*
   reparented by `zwlr_layer_surface_v1.get_popup`
   (`wlr-layer-shell-unstable-v1.xml:287-298`). At `wlr_xdg_shell.events.new_popup`
   time it therefore cannot be classified at all. The parent's own `new_popup`
   signal is the only place the parent is knowable, for all three parent kinds.
2. **Identity travels in `Bound`, never in `data`.** wlroots emits
   `wlr_surface.events.map`/`.unmap` and several role-object destroys with a
   **null** `data` (`backend.rs:5091-5106` documents this for toplevels), and
   whether `wlr_xdg_popup.events.{destroy,reposition}` do was not verifiable
   from the installed artefacts. Carrying the `PopupId` in the `Bound` at link
   time makes the question moot — and is the crate's standing rule anyway.
3. **The scene subtree is the parent's.**
   `wlr_scene_xdg_surface_create(parent_tree, popup->base)` makes a popup stack
   with its parent for free, makes `Runtime::leaf_surface_at` resolve clicks on
   it for free, and inherits the session-lock isolation gate
   (`runtime.rs:8185-8197`) for free. P1 therefore adds **no** second lock gate
   and **destroys no popup scene tree** — wlroots frees a tree's children
   recursively with the tree (`forget_toplevel`, `runtime.rs:6842`), so a manual
   destroy of a child is a double free.

---

## Tech Stack

- **Repo:** `/home/joseph/Projects/wlroots-sys`, crate `crates/wlr` (`wlr`),
  branch `develop` @ `d072c96` (= wlr 0.20.27). **P1's executor creates the
  branch**; no other session may create a branch in this repo.
- **Language:** Rust, edition 2024, `rust-version` 1.94 (both from
  `[workspace.package]`).
- **FFI:** `wlr-sys` 0.20 (path dependency; `links = "wlroots"`), bindgen-generated,
  **no per-symbol allowlist** — every `wlr_xdg_popup_*` and
  `wlr_xdg_positioner_*` symbol is already bound and callable as `sys::…` today.
- **wlroots:** 0.20, headers at `/usr/include/wlroots-0.20/wlr/`, shipped
  **without `NDEBUG`** — a failed `assert` is a hard `abort()` of the whole
  process, which is why `Popup::send_configure` gates on `initialized`.
- **Dependencies added:** **none.** (See deviation D1.)
- **Test harness:** the crate's own `#[cfg(test)] mod tests` blocks for anything
  needing private access, and `crates/wlr/tests/*.rs` integration binaries for
  the public surface, headless (`WLR_BACKENDS=headless`,
  `WLR_HEADLESS_OUTPUTS=1`, set through a per-binary `std::sync::Once` —
  `tests/layers.rs:17-32` is the template).
- **Coverage ledger:** `crates/wlr/coverage/{wrapped,waived}.toml`, audited by
  `cargo test -p wlr --test coverage_audit`, burned down by
  `cargo xtask coverage`. The audit scans the crate's source for literal
  `sys::wlr_*` strings (`coverage.rs:431-500`), so **a wrapped row whose symbol
  never appears textually as `sys::<symbol>` is a hard failure** — this shapes
  several helper signatures below on purpose.

---

## Spec

- **Design spec:**
  `/home/joseph/Projects/icedtea/docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
  — §1 (P1's row), §2 ("Popups: `wlr` crate + compositor"), §7 (per-part gates).
- **Binding contract:**
  `/home/joseph/Projects/icedtea/docs/superpowers/plans/2026-08-27-m3-part0-contract.md`
  — §0 (module map), **§1 in full** (§1.1 through §1.10), §9's "P1" boundary,
  §9's cross-cutting rules, §10 (amendments).
- **Research notes:**
  `/home/joseph/Projects/icedtea/.superpowers/m3-plan-notes/wlr-popups.md` — the
  C API surface, what is *not* available, the crate conventions, the grab
  analysis and the ledger disposition table.

---

## Global Constraints

**Crate pins (unchanged by this part).** `wlr-sys` 0.20 (path),
`xkbcommon-sys` 1.4, `pkg-config` 0.3 (build), `trybuild` 1.0 /
`static_assertions` 1.1 / `rustix` 1 (dev). On the icedtea side, unchanged and
not touched here: `wayland-client` 0.31, `wayland-protocols` 0.32,
`wayland-protocols-wlr` 0.3, `skia-rs-safe` 0.4.0, `taffy` 0.14, `cssparser`
0.37, `selectors` 0.40, `fontconfig` 0.11 (optional). **No `smithay`, no `gtk`,
no `gtk4-rs`, ever.** **No new dependency is added by P1** (see D1).

**Edition/toolchain.** edition 2024, `rust-version` 1.94, both inherited from
`[workspace.package]`. Do not edit either.

**Gates.** The M3 milestone-wide gates are
`cargo test -p icedtea-ui`, `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
(and again with `--no-default-features`), and `cargo fmt --all --check`. **P1
runs in the other repo and its gate is the `wlr` set instead**, run from
`/home/joseph/Projects/wlroots-sys`:

```bash
cargo test -p wlr
cargo test -p wlr --test coverage_audit          # with WLR_COVERAGE_BINDINGS pinned
cargo xtask coverage                             # burn-down, must not regress
cargo clippy -p wlr --all-targets -- -D warnings
cargo clippy -p wlr --all-targets --no-default-features -- -D warnings
cargo doc -p wlr --no-deps                       # RUSTDOCFLAGS="-D warnings"
cargo fmt --all --check
cargo publish -p wlr --dry-run
```

`mod implicit_grab_tests` (`crates/wlr/src/backend.rs:8517`) must stay green
**and byte-identical**. For P2 the gate becomes
`cargo test -p icedtea-compositor --test popups` run three times, all twelve
tests passing every time.

**Commit trailer.** Every commit in this part ends with, on its own line after a
blank line:

```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

**M1/M2 gates stay green.** Nothing in this part touches `ui/`, so
`cargo test -p icedtea-ui` is unaffected; if it is not green after this part,
something was touched that should not have been.

**Ordering and consumption.** The eight M3 parts execute **in order 1 → 8**, so
any part may consume what an earlier part produced. P1 consumes nothing. P1 is
executed in a **wloots-sys worktree**, and icedtea consumes the **published**
`wlr = "0.20.28"` through its `Cargo.toml` pins. During P2–P8 development a root
`[patch.crates-io]` pointing at the worktree is allowed — in **its own commit**,
and **dropped before merge** — exactly as the implicit-grab fix did.

**Untrusted input never panics.** Positioner anchors, gravities and constraint
adjustments come from a client. An unknown value maps to the initial value and
is dropped (D3). Every parser of such input gets a never-panic test (Task 2).

**Every load-bearing test states its mutation check** — the exact edit to make to
the implementation, the exact test that must then fail, and the restore. Perform
it; do not merely read it.

---

## File Structure

All paths are relative to `/home/joseph/Projects/wlroots-sys`.

| File | Status | What P1 does to it |
|---|---|---|
| `crates/wlr/src/popup.rs` | **new** | `PopupId`, `PopupParent`, `PositionerAnchor`, `PositionerGravity`, `ConstraintAdjustment`, `PositionerRules`, `Popup<'h>` and its own `mod tests` |
| `crates/wlr/src/lib.rs` | edit | `mod popup;` + the seven-name `pub use popup::{…}` block |
| `crates/wlr/src/runtime.rs` | edit | `PopupEntry`, `RuntimeInner.popups`, `record_popup`/`forget_popup`/`popup_entry`/`clear_popups`, the nine public `Runtime` popup methods, `seat_has_explicit_grab`, and new `mod tests` cases |
| `crates/wlr/src/backend.rs` | edit | `Bound.popup`, `Registration::link_popup`, `Session.popups`, `PopupListeners`, `on_new_popup`/`_commit`/`_map`/`_unmap`/`_reposition`/`_destroy`, three `new_popup` link sites, `toplevel_id_of_surface`'s role check, `with_popup`, six `deliver_all` arms, six names in `deliver`'s unreachable arm, `clear_popups` from `run_inner` |
| `crates/wlr/src/dispatch.rs` | edit | six `Event` variants |
| `crates/wlr/src/handler.rs` | edit | six defaulted `ToplevelHandler` methods |
| `crates/wlr/tests/popups.rs` | **new** | headless integration tests for the public popup surface |
| `crates/wlr/coverage/wrapped.toml` | edit | 13 rows moved in |
| `crates/wlr/coverage/waived.toml` | edit | 13 rows removed, 1 re-reasoned |
| `crates/wlr/README.md` | edit | `## 0.20.28 — xdg-popup` release-notes section |
| `crates/wlr/Cargo.toml` | edit | `version = "0.20.28"` |
| `crates/wlr/src/backend.rs` § `mod implicit_grab_tests` | **untouched** | must stay byte-identical |

---

## Tasks

### Task 0 — Worktree and branch

**Files:** none (git only).

**Interfaces:** Consumes nothing. Produces the branch every later task commits
onto.

**Steps.**

1. Confirm the starting point and that the tree is clean:

```bash
cd /home/joseph/Projects/wlroots-sys
git status --porcelain
git log --oneline -1
```

Expected: no output from the first command, and `d072c96 Merge pull request #10 from quinnjr/fix/wlr-implicit-pointer-grab`
from the second. If either differs, **stop and report** — another session may be
using this repo.

2. Create the worktree and branch (this is the only place in this plan that
   creates a branch here):

```bash
cd /home/joseph/Projects/wlroots-sys
git worktree add -b feat/wlr-xdg-popup /home/joseph/Projects/wlroots-sys-m3p1 develop
cd /home/joseph/Projects/wlroots-sys-m3p1
git log --oneline -1
```

Expected: the same `d072c96` line.

3. Record the bindings path the coverage audit must use, so every later
   invocation pins the same file rather than "the newest under `target/`":

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo build -p wlr --all-features
find target -name 'bindings.rs' -newermt '-10 minutes' | head
```

Take the single path printed and export it in every shell that runs the audit:

```bash
export WLR_COVERAGE_BINDINGS=<that path>
export WLR_COVERAGE_ALL_FEATURES=1
```

4. Prove the baseline is green before adding anything:

```bash
cargo test -p wlr 2>&1 | tail -20
cargo test -p wlr --test coverage_audit 2>&1 | tail -5
```

Expected: all suites pass, coverage audit passes.

**From here on, every path in this plan that says
`/home/joseph/Projects/wlroots-sys` means the worktree
`/home/joseph/Projects/wlroots-sys-m3p1`.**

No commit for this task.

---

### Task 1 — `popup.rs`: ids, parents, and the positioner enums

**Files:** `crates/wlr/src/popup.rs` (new), `crates/wlr/src/lib.rs`.

**Interfaces.**

*Consumes:* `crate::id::next_id` (private), `crate::{LayerSurfaceId, ToplevelId}`.

*Produces:*

```rust
pub struct PopupId(pub(crate) u64);
impl PopupId {
    pub fn dangling_for_test() -> PopupId;
    pub fn dangling_nth_for_test(n: u64) -> PopupId;
}

pub enum PopupParent { Toplevel(ToplevelId), Layer(LayerSurfaceId), Popup(PopupId) }
impl PopupParent {
    pub fn is_popup(self) -> bool;
    // `root` lands in Task 4, once `Runtime::popup_parent` exists.
}

#[non_exhaustive] pub enum PositionerAnchor  { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }
#[non_exhaustive] pub enum PositionerGravity { None, Top, Bottom, Left, Right, TopLeft, BottomLeft, TopRight, BottomRight }

pub struct ConstraintAdjustment(u32);
impl ConstraintAdjustment {
    pub const NONE: ConstraintAdjustment;
    pub const SLIDE_X: ConstraintAdjustment;
    pub const SLIDE_Y: ConstraintAdjustment;
    pub const FLIP_X: ConstraintAdjustment;
    pub const FLIP_Y: ConstraintAdjustment;
    pub const RESIZE_X: ConstraintAdjustment;
    pub const RESIZE_Y: ConstraintAdjustment;
    pub fn contains(self, other: ConstraintAdjustment) -> bool;
    pub fn bits(self) -> u32;
}
impl std::ops::BitOr for ConstraintAdjustment { type Output = ConstraintAdjustment; }
impl std::ops::BitOrAssign for ConstraintAdjustment {}
```

**Step 1 — write the failing test.**

Create `crates/wlr/src/popup.rs` containing **only** the module doc and the test
module (the types come in step 3, so this must fail to compile — which is the
failure this step is looking for):

```rust
//! Borrow-scoped xdg-popup handles, their stable ids, and the client's
//! positioner.
//!
//! Same shape as [`Toplevel`](crate::Toplevel), for the same reason: a
//! `wlr_xdg_popup` is freed whenever its client says so, so a handle that
//! escapes the handler it was passed to is a use-after-free. The lifetime and
//! the private constructor make that a compile error.
//!
//! The id is attached with `wlr_addon` to the popup's **`wlr_surface`**
//! (`popup->base->surface`), not to the popup itself: `wlr_xdg_popup` has no
//! addon set, `wlr_surface` does, and the two die together — the identical
//! argument [`crate::ToplevelId`]'s module doc makes.
//!
//! # What this module deliberately does not do
//!
//! It does not create, inspect, end or imitate a `wlr_xdg_popup_grab`. wlroots
//! builds one itself when a client sends `xdg_popup.grab`, installs three seat
//! grabs (pointer, keyboard, touch), routes delivery to the popup chain,
//! dismisses the chain on a press outside it, and restores the pre-grab
//! keyboard focus when the grab ends. There is no C API to touch any of that —
//! the export table has no `wlr_xdg_popup_grab_*` symbol at all — and a second
//! `wlr_seat_pointer_start_grab` would displace wlroots' own and break chain
//! dismissal. What this crate offers instead is observation:
//! [`Popup::grab_requested`] (`popup->seat != NULL`) and
//! [`Runtime::seat_has_explicit_grab`](crate::Runtime::seat_has_explicit_grab).

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids come from the same process-wide counter every other id type in
    /// this crate uses, so a popup id can never collide with a toplevel or
    /// layer-surface id — but it *can* be mislabelled, which is what
    /// `toplevel_id_of_surface`'s role check exists to stop. This pins the
    /// non-collision half.
    #[test]
    fn the_dangling_ids_are_distinct_and_out_of_the_issued_range() {
        assert_eq!(PopupId::dangling_nth_for_test(0), PopupId::dangling_for_test());
        assert_ne!(PopupId::dangling_nth_for_test(1), PopupId::dangling_for_test());
        assert_ne!(
            PopupId::dangling_nth_for_test(1),
            PopupId::dangling_nth_for_test(2)
        );
        // The band is 2^32 wide immediately below u64::MAX, so even an absurd
        // `n` stays in the reserved range rather than wrapping into the
        // counter's own.
        assert!(PopupId::dangling_nth_for_test(u64::MAX).0 > u64::MAX - (1u64 << 32) - 1);
    }

    #[test]
    fn a_popup_parent_knows_whether_it_is_itself_a_popup() {
        assert!(PopupParent::Popup(PopupId::dangling_for_test()).is_popup());
        assert!(!PopupParent::Toplevel(ToplevelId::dangling_for_test()).is_popup());
        assert!(!PopupParent::Layer(LayerSurfaceId::dangling_for_test()).is_popup());
    }

    /// The six bits are `1, 2, 4, 8, 16, 32`, exactly as
    /// `enum xdg_positioner_constraint_adjustment` declares them. This is what
    /// makes hand-rolling the bitmask (rather than taking a `bitflags`
    /// dependency — see this crate's `DataPtrAccess` for the standing decision)
    /// a *checked* decision: the constants are re-exported from `wlr-sys`, so a
    /// protocol renumbering would change them silently and only this test would
    /// notice.
    #[test]
    fn the_constraint_bits_are_the_ones_the_protocol_declares() {
        assert_eq!(ConstraintAdjustment::NONE.bits(), 0);
        assert_eq!(ConstraintAdjustment::SLIDE_X.bits(), 1);
        assert_eq!(ConstraintAdjustment::SLIDE_Y.bits(), 2);
        assert_eq!(ConstraintAdjustment::FLIP_X.bits(), 4);
        assert_eq!(ConstraintAdjustment::FLIP_Y.bits(), 8);
        assert_eq!(ConstraintAdjustment::RESIZE_X.bits(), 16);
        assert_eq!(ConstraintAdjustment::RESIZE_Y.bits(), 32);

        assert_eq!(
            ConstraintAdjustment::SLIDE_X.bits(),
            sys::xdg_positioner_constraint_adjustment::XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_X
                .0
        );
        assert_eq!(
            ConstraintAdjustment::FLIP_Y.bits(),
            sys::xdg_positioner_constraint_adjustment::XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_FLIP_Y.0
        );
        assert_eq!(
            ConstraintAdjustment::RESIZE_Y.bits(),
            sys::xdg_positioner_constraint_adjustment::XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_RESIZE_Y
                .0
        );
    }

    /// `contains` is "every bit of the argument is set here", not "any" — the
    /// distinction that decides whether a compositor may flip a popup it may
    /// only slide. Same semantics `DataPtrAccess::contains` pins.
    #[test]
    fn contains_is_every_bit_not_any_bit() {
        let both = ConstraintAdjustment::FLIP_X | ConstraintAdjustment::FLIP_Y;
        assert_eq!(both.bits(), 0b1100);
        assert!(both.contains(ConstraintAdjustment::FLIP_X));
        assert!(both.contains(ConstraintAdjustment::FLIP_Y));
        assert!(both.contains(both));
        assert!(!both.contains(ConstraintAdjustment::SLIDE_X));
        assert!(!both.contains(ConstraintAdjustment::FLIP_X | ConstraintAdjustment::SLIDE_X));
        // The empty set is contained in everything, including itself.
        assert!(both.contains(ConstraintAdjustment::NONE));
        assert!(ConstraintAdjustment::NONE.contains(ConstraintAdjustment::NONE));
        assert!(!ConstraintAdjustment::NONE.contains(ConstraintAdjustment::FLIP_X));
    }

    #[test]
    fn bit_or_assign_accumulates() {
        let mut c = ConstraintAdjustment::NONE;
        c |= ConstraintAdjustment::SLIDE_X;
        c |= ConstraintAdjustment::SLIDE_Y;
        assert_eq!(c.bits(), 0b11);
    }
}
```

Register the module in `crates/wlr/src/lib.rs`. Insert `mod popup;` between
`mod output;` and `mod region;`, keeping the list alphabetical:

```rust
mod output;
mod popup;
mod region;
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -20
```

Expected failure: a compile error, `error[E0433]: failed to resolve: use of
undeclared type `PopupId`` (and the same for `PopupParent`,
`ConstraintAdjustment`, `sys`). That is the correct failure — the tests name
types that do not exist yet.

**Step 3 — implement.**

Insert the following into `crates/wlr/src/popup.rs`, **above** the `#[cfg(test)]
mod tests` block and below the module doc:

```rust
use crate::id::next_id;
use crate::{LayerSurfaceId, ToplevelId, sys};

/// Identifies one live `wlr_xdg_popup` for as long as the consumer chooses to
/// remember it.
///
/// Storable, comparable and hashable — unlike a handle. Minted on the popup's
/// `wlr_surface` addon set from the same process-wide counter as
/// [`ToplevelId`], [`LayerSurfaceId`] and [`NodeId`](crate::NodeId), so ids
/// never collide across kinds.
///
/// Deliberately no `PartialOrd`/`Ord`: an opaque id's ordering would promise
/// creation-order semantics nobody asked for, and this API is frozen within the
/// wlroots minor, so a derive added here could not be withdrawn. See
/// [`ToplevelId`], whose doc this mirrors, for the full argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupId(pub(crate) u64);

impl PopupId {
    /// An id no live popup can have, for testing the "unknown id" path.
    ///
    /// Public for the reason [`ToplevelId::dangling_for_test`] is: "every
    /// by-id operation reports a miss rather than dereferencing" is a promise
    /// to consumers, and a promise nobody can write a test for is not one.
    /// Ids come from a counter that starts at 1 and only increments, so
    /// `u64::MAX` cannot be handed to a real popup.
    pub fn dangling_for_test() -> PopupId {
        PopupId(u64::MAX)
    }

    /// A distinct id no live popup can have, for testing.
    ///
    /// `n` is folded into a fixed 2^32-wide band immediately below `u64::MAX`
    /// rather than subtracted unclamped, so even `n = u64::MAX` lands on a
    /// value the shared counter cannot reach — the identical argument
    /// [`ToplevelId::dangling_nth_for_test`] makes. `n = 0` aliases
    /// [`dangling_for_test`](Self::dangling_for_test).
    pub fn dangling_nth_for_test(n: u64) -> PopupId {
        PopupId(u64::MAX - (n % (1u64 << 32)))
    }

    /// Mint a fresh id from the crate-wide counter, for the tables' own tests.
    ///
    /// Not public: a real popup's id comes from its surface's addon set, which
    /// is what makes wlroots release it at exactly the right moment.
    #[cfg(test)]
    pub(crate) fn next_for_test() -> PopupId {
        PopupId(next_id())
    }
}

/// What a popup hangs off.
///
/// A layer-shell popup is created with a **NULL** xdg parent
/// (`xdg_surface.get_popup` with `parent = null`) and only then reparented by
/// `zwlr_layer_surface_v1.get_popup`, so its parent is knowable *only* from the
/// parent-scoped `new_popup` signal — never from `popup->parent`, which is null
/// at creation. This crate therefore records the parent once, at announcement
/// time, and [`Popup::parent`] returns that recording rather than re-deriving
/// anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupParent {
    /// An `xdg_toplevel`.
    Toplevel(ToplevelId),
    /// A `zwlr_layer_surface_v1`.
    Layer(LayerSurfaceId),
    /// Another popup — a nested chain (a submenu off a menu).
    Popup(PopupId),
}

impl PopupParent {
    /// Whether this parent is itself a popup, i.e. whether the popup naming it
    /// is part of a nested chain rather than hanging directly off a window.
    #[must_use]
    pub fn is_popup(self) -> bool {
        matches!(self, PopupParent::Popup(_))
    }
}

/// `xdg_positioner.set_anchor` — which point of the anchor rectangle the popup
/// is positioned against.
///
/// `#[non_exhaustive]`: the protocol may add anchors, and a value this crate
/// does not know maps to [`PositionerAnchor::None`] rather than panicking. The
/// value comes from an untrusted client, so that is a hard rule, not a
/// courtesy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PositionerAnchor {
    /// The centre of the anchor rectangle.
    None,
    /// The middle of its top edge.
    Top,
    /// The middle of its bottom edge.
    Bottom,
    /// The middle of its left edge.
    Left,
    /// The middle of its right edge.
    Right,
    /// Its top-left corner.
    TopLeft,
    /// Its bottom-left corner.
    BottomLeft,
    /// Its top-right corner.
    TopRight,
    /// Its bottom-right corner.
    BottomRight,
}

impl PositionerAnchor {
    /// Convert from `enum xdg_positioner_anchor`.
    ///
    /// An unrecognised value yields [`PositionerAnchor::None`], which is the
    /// protocol's own initial value, and never panics — the value crosses the
    /// wire from a client this compositor does not control. Nothing is logged:
    /// this crate binds no Rust-side logging symbol (wlroots' `wlr_log` is a
    /// `static inline` macro over an unbound `_wlr_log`, and there is
    /// deliberately no `log`/`tracing` dependency), the same reason
    /// `Runtime::apply_cursor` gives for its own silent fallback.
    ///
    /// Matched on the raw `u32` rather than on the newtype's associated
    /// constants, and the mapping is pinned by
    /// `the_anchor_values_are_the_ones_the_protocol_declares` — the same
    /// discipline `DataPtrAccess`'s own test applies.
    pub(crate) fn from_raw(raw: u32) -> PositionerAnchor {
        match raw {
            1 => PositionerAnchor::Top,
            2 => PositionerAnchor::Bottom,
            3 => PositionerAnchor::Left,
            4 => PositionerAnchor::Right,
            5 => PositionerAnchor::TopLeft,
            6 => PositionerAnchor::BottomLeft,
            7 => PositionerAnchor::TopRight,
            8 => PositionerAnchor::BottomRight,
            // 0 is `NONE`; anything else is a client sending a value this
            // protocol version does not define.
            _ => PositionerAnchor::None,
        }
    }
}

/// `xdg_positioner.set_gravity` — which direction the popup extends from its
/// anchor point.
///
/// `#[non_exhaustive]` for the reason [`PositionerAnchor`] is, and with the same
/// unknown-value rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PositionerGravity {
    /// Centred on the anchor point.
    None,
    /// Extends upward.
    Top,
    /// Extends downward.
    Bottom,
    /// Extends leftward.
    Left,
    /// Extends rightward.
    Right,
    /// Extends up and left.
    TopLeft,
    /// Extends down and left.
    BottomLeft,
    /// Extends up and right.
    TopRight,
    /// Extends down and right.
    BottomRight,
}

impl PositionerGravity {
    /// Convert from `enum xdg_positioner_gravity`; unknown values yield
    /// [`PositionerGravity::None`]. See [`PositionerAnchor::from_raw`] for the
    /// full argument, which applies here verbatim.
    pub(crate) fn from_raw(raw: u32) -> PositionerGravity {
        match raw {
            1 => PositionerGravity::Top,
            2 => PositionerGravity::Bottom,
            3 => PositionerGravity::Left,
            4 => PositionerGravity::Right,
            5 => PositionerGravity::TopLeft,
            6 => PositionerGravity::BottomLeft,
            7 => PositionerGravity::TopRight,
            8 => PositionerGravity::BottomRight,
            _ => PositionerGravity::None,
        }
    }
}

/// `xdg_positioner.set_constraint_adjustment` — which rearrangements the client
/// permits when the popup would fall outside the constraint box.
///
/// A bitmask of `enum xdg_positioner_constraint_adjustment`. Hand-rolled rather
/// than a `bitflags` dependency, following
/// [`DataPtrAccess`](crate::DataPtrAccess) and `BufferCaps` before it: the six
/// bits are the whole domain and their values are pinned by this module's own
/// tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConstraintAdjustment(u32);

impl ConstraintAdjustment {
    /// No rearrangement is permitted; the popup is placed as asked even if it
    /// falls outside. The protocol's initial value, and [`Default`].
    pub const NONE: ConstraintAdjustment = ConstraintAdjustment(0);
    /// The popup may be slid along the x axis to fit.
    pub const SLIDE_X: ConstraintAdjustment = ConstraintAdjustment(1);
    /// The popup may be slid along the y axis to fit.
    pub const SLIDE_Y: ConstraintAdjustment = ConstraintAdjustment(2);
    /// The popup may be flipped to the opposite side of its anchor on x.
    pub const FLIP_X: ConstraintAdjustment = ConstraintAdjustment(4);
    /// The popup may be flipped to the opposite side of its anchor on y.
    pub const FLIP_Y: ConstraintAdjustment = ConstraintAdjustment(8);
    /// The popup may be narrowed to fit.
    pub const RESIZE_X: ConstraintAdjustment = ConstraintAdjustment(16);
    /// The popup may be shortened to fit.
    pub const RESIZE_Y: ConstraintAdjustment = ConstraintAdjustment(32);

    /// Whether **every** bit of `other` is set here — not "any", which is the
    /// distinction that decides whether a compositor may flip a popup the
    /// client only permitted it to slide.
    #[must_use]
    pub fn contains(self, other: ConstraintAdjustment) -> bool {
        self.0 & other.0 == other.0
    }

    /// The raw mask, as the protocol numbers it.
    #[must_use]
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Build from a raw `enum xdg_positioner_constraint_adjustment` value.
    ///
    /// Bits this crate does not know are **kept**, not dropped: the mask is
    /// handed straight back to wlroots by
    /// [`PositionerRules::unconstrain_box`], which is the code that interprets
    /// it, and silently clearing a bit here would change the client's request
    /// rather than merely failing to describe it. Nothing can panic — it is one
    /// integer.
    pub(crate) fn from_raw(raw: u32) -> ConstraintAdjustment {
        ConstraintAdjustment(raw)
    }
}

impl std::ops::BitOr for ConstraintAdjustment {
    type Output = ConstraintAdjustment;

    fn bitor(self, rhs: ConstraintAdjustment) -> ConstraintAdjustment {
        ConstraintAdjustment(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for ConstraintAdjustment {
    fn bitor_assign(&mut self, rhs: ConstraintAdjustment) {
        self.0 |= rhs.0;
    }
}
```

Add to the test module, alongside the existing cases, the anchor/gravity value
pin (it belongs with the constraint-bit pin and is referenced by its name in
`from_raw`'s doc):

```rust
    /// The nine anchors and nine gravities are numbered `0..=8` in
    /// `xdg-shell.xml`, and `from_raw`'s match arms hard-code those numbers.
    /// Pinning them against the generated constants is what makes hard-coding
    /// them safe: a protocol renumbering changes the constants and this test
    /// fails, rather than every popup in the session being anchored wrongly.
    #[test]
    fn the_anchor_values_are_the_ones_the_protocol_declares() {
        use sys::xdg_positioner_anchor as A;
        use sys::xdg_positioner_gravity as G;

        assert_eq!(
            PositionerAnchor::from_raw(A::XDG_POSITIONER_ANCHOR_NONE.0),
            PositionerAnchor::None
        );
        assert_eq!(
            PositionerAnchor::from_raw(A::XDG_POSITIONER_ANCHOR_TOP.0),
            PositionerAnchor::Top
        );
        assert_eq!(
            PositionerAnchor::from_raw(A::XDG_POSITIONER_ANCHOR_BOTTOM_RIGHT.0),
            PositionerAnchor::BottomRight
        );
        assert_eq!(
            PositionerGravity::from_raw(G::XDG_POSITIONER_GRAVITY_NONE.0),
            PositionerGravity::None
        );
        assert_eq!(
            PositionerGravity::from_raw(G::XDG_POSITIONER_GRAVITY_BOTTOM_LEFT.0),
            PositionerGravity::BottomLeft
        );
        assert_eq!(
            PositionerGravity::from_raw(G::XDG_POSITIONER_GRAVITY_TOP_RIGHT.0),
            PositionerGravity::TopRight
        );
    }
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: `test result: ok. 6 passed`, clippy clean, fmt clean.

**Mutation check (perform it).** In `ConstraintAdjustment::contains`, change
`self.0 & other.0 == other.0` to `self.0 & other.0 != 0`. Re-run
`cargo test -p wlr --lib popup::`; `contains_is_every_bit_not_any_bit` must
fail on the `!both.contains(FLIP_X | SLIDE_X)` assertion. Restore the line and
re-run to green. Then change `PositionerAnchor::from_raw`'s `5 =>` arm to
`PositionerAnchor::TopRight`; `the_anchor_values_are_the_ones_the_protocol_declares`
must still pass (it does not probe 5) — so **also** add nothing and instead
change the `8 =>` arm to `PositionerAnchor::TopLeft`, which the test does probe,
and confirm it fails. Restore both.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/popup.rs crates/wlr/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(wlr): popup ids, parents and the positioner enums

`PopupId` mirrors `ToplevelId` exactly, minted from the same process-wide
counter, with the same two dangling constructors so a consumer can test the
"unknown id misses rather than dereferences" promise.

`PopupParent` is recorded at announcement time rather than re-derived: a
layer-shell popup is created with a NULL xdg parent and only then reparented by
`zwlr_layer_surface_v1.get_popup`, so `popup->parent` is null exactly when the
answer matters.

`PositionerAnchor`/`PositionerGravity` are `#[non_exhaustive]` and map an
unknown client value to the protocol's initial value instead of panicking.
`ConstraintAdjustment` is hand-rolled rather than taking a `bitflags`
dependency, following `DataPtrAccess`'s standing decision, with the six bits
pinned against the generated constants by a test.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2 — `PositionerRules`: the copied-out positioner, and its never-panic tests

**Files:** `crates/wlr/src/popup.rs`.

**Interfaces.**

*Consumes:* Task 1's `PositionerAnchor::from_raw`, `PositionerGravity::from_raw`,
`ConstraintAdjustment::from_raw`; `crate::Box2D` (`geom.rs:31`, a `#[repr(C)]`
twin of `wlr_box` with `as_c()`).

*Produces:*

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub(crate) unsafe fn from_c(rules: &sys::wlr_xdg_positioner_rules) -> PositionerRules;
    pub(crate) fn to_c(&self) -> sys::wlr_xdg_positioner_rules;
}
```

`to_c` exists because `geometry`/`unconstrain_box` operate on a **copy** — the
contract's §1.1 says the rules are "copied out (never borrowed) so it survives
re-entry into wlroots", which means the two C calls need a C-shaped value to
point at, rebuilt from the Rust one. It also means the round trip
`from_c(to_c(r)) == r` is a real invariant worth testing, and it is the cheapest
way to make the literal string `sys::wlr_xdg_positioner_rules` appear in a
`sys::` use, which the coverage audit requires for that row (Task 12).

**Step 1 — write the failing tests.** Append to `popup.rs`'s `mod tests`:

```rust
    /// Build a `wlr_xdg_positioner_rules` by hand. The struct is plain data —
    /// four `wlr_box`/size/offset groups, three enums and two bools, no
    /// pointers and no `wl_listener` — so `Default`-zeroing it and filling
    /// fields is sound, unlike the toplevel/decoration structs `runtime.rs`'s
    /// tests must `alloc_zeroed` behind a raw pointer.
    fn raw_rules(
        anchor_rect: (i32, i32, i32, i32),
        size: (i32, i32),
        anchor: u32,
        gravity: u32,
        adjustment: u32,
    ) -> sys::wlr_xdg_positioner_rules {
        let mut r: sys::wlr_xdg_positioner_rules = unsafe { std::mem::zeroed() };
        r.anchor_rect = sys::wlr_box {
            x: anchor_rect.0,
            y: anchor_rect.1,
            width: anchor_rect.2,
            height: anchor_rect.3,
        };
        r.size.width = size.0;
        r.size.height = size.1;
        r.anchor = sys::xdg_positioner_anchor(anchor);
        r.gravity = sys::xdg_positioner_gravity(gravity);
        r.constraint_adjustment = sys::xdg_positioner_constraint_adjustment(adjustment);
        r
    }

    /// The plain case: anchor the popup's top-left at the anchor rect's
    /// bottom-left and let it extend down-right. wlroots' own
    /// `wlr_xdg_positioner_rules_get_geometry` is what computes this — the
    /// crate deliberately does not reimplement the placement algebra, for the
    /// reason `geom.rs`'s module doc gives about `wlr_box` predicates: a
    /// reimplementation is free to drift, silently, in a patch release.
    #[test]
    fn the_geometry_anchors_and_gravitates_the_way_wlroots_does() {
        // anchor rect (10, 20, 100, 40); anchor = BOTTOM_LEFT (6);
        // gravity = BOTTOM_RIGHT (8); size 30x20.
        let rules = unsafe { PositionerRules::from_c(&raw_rules((10, 20, 100, 40), (30, 20), 6, 8, 0)) };
        let g = rules.geometry();
        assert_eq!((g.width, g.height), (30, 20), "the size is the client's");
        assert_eq!(
            (g.x, g.y),
            (10, 60),
            "bottom-left of (10,20,100,40) is (10,60), and BOTTOM_RIGHT gravity \
             puts the popup's top-left there"
        );
    }

    /// `unconstrain_box` is a pure function of the rules and a box: it touches
    /// no live popup, which is what makes it usable from a compositor deciding
    /// placement before anything is configured.
    #[test]
    fn unconstraining_slides_a_popup_back_inside_when_sliding_is_permitted() {
        // The popup would start at x = 190 and be 100 wide, running to 290 in a
        // 200-wide constraint. SLIDE_X (1) is permitted.
        let rules =
            unsafe { PositionerRules::from_c(&raw_rules((190, 0, 1, 1), (100, 20), 6, 8, 1)) };
        let free = rules.geometry();
        assert_eq!(free.x, 190, "unconstrained, it starts where it was asked to");

        let fitted = rules.unconstrain_box(&Box2D::new(0, 0, 200, 200));
        assert!(
            fitted.x + fitted.width <= 200,
            "with SLIDE_X permitted the popup must end up inside the constraint; \
             got x={} width={}",
            fitted.x,
            fitted.width
        );
        assert_eq!(fitted.width, 100, "sliding must not resize");
    }

    /// Without a permitted adjustment there is nothing wlroots may do, and the
    /// answer is the unconstrained geometry — *not* a clamp this crate invents.
    /// A compositor that wants a clamp asks for one through the adjustment
    /// bits, which is the protocol's own design.
    #[test]
    fn unconstraining_with_no_adjustment_permitted_changes_nothing() {
        let rules =
            unsafe { PositionerRules::from_c(&raw_rules((190, 0, 1, 1), (100, 20), 6, 8, 0)) };
        assert_eq!(
            rules.unconstrain_box(&Box2D::new(0, 0, 200, 200)),
            rules.geometry()
        );
    }

    /// Untrusted input never panics (spec §7). Every anchor/gravity value a
    /// 32-bit client could possibly send — including the whole u32 range at the
    /// boundaries — is converted, round-tripped through C and asked for its
    /// geometry, and none of it may abort, panic or produce a NaN-shaped box.
    #[test]
    fn a_positioner_with_nonsense_enum_values_never_panics() {
        for raw in [0u32, 8, 9, 10, 255, 1000, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
            let rules =
                unsafe { PositionerRules::from_c(&raw_rules((0, 0, 1, 1), (10, 10), raw, raw, raw)) };
            // Unknown anchors and gravities land on the protocol's initial
            // value; unknown *constraint* bits are kept verbatim, because that
            // mask goes straight back to wlroots (see `from_raw`'s doc).
            if raw > 8 {
                assert_eq!(rules.anchor, PositionerAnchor::None);
                assert_eq!(rules.gravity, PositionerGravity::None);
            }
            assert_eq!(rules.constraint_adjustment.bits(), raw);
            let _ = rules.geometry();
            let _ = rules.unconstrain_box(&Box2D::new(0, 0, 100, 100));
        }
    }

    /// Degenerate geometry from a client — zero and negative sizes, an empty
    /// anchor rect, an empty constraint — must come back as a value, never as
    /// an abort. `Box2D`'s own contract already calls a non-positive extent
    /// "empty", so an empty answer is a legitimate answer here.
    #[test]
    fn a_positioner_with_degenerate_sizes_never_panics() {
        for size in [(0, 0), (-1, -1), (i32::MIN, i32::MIN), (i32::MAX, i32::MAX)] {
            for rect in [(0, 0, 0, 0), (0, 0, -5, -5), (i32::MIN, i32::MIN, 1, 1)] {
                let rules = unsafe { PositionerRules::from_c(&raw_rules(rect, size, 6, 8, 63)) };
                let _ = rules.geometry();
                let _ = rules.unconstrain_box(&Box2D::default());
                let _ = rules.unconstrain_box(&Box2D::new(0, 0, 100, 100));
            }
        }
    }

    /// The copy is a *copy*: `to_c` then `from_c` must land back on the same
    /// value, or a rules snapshot handed to wlroots would not describe what the
    /// consumer read. This is also what keeps the `wlr_xdg_positioner_rules`
    /// coverage row honest.
    #[test]
    fn the_rules_round_trip_through_the_c_representation() {
        let mut raw = raw_rules((3, 4, 5, 6), (7, 8), 5, 3, 0b101010);
        raw.offset.x = -2;
        raw.offset.y = 9;
        raw.reactive = true;
        raw.has_parent_configure_serial = true;
        raw.parent_configure_serial = 4242;
        raw.parent_size.width = 800;
        raw.parent_size.height = 600;

        let rules = unsafe { PositionerRules::from_c(&raw) };
        assert_eq!(rules.anchor_rect, Box2D::new(3, 4, 5, 6));
        assert_eq!(rules.size, (7, 8));
        assert_eq!(rules.anchor, PositionerAnchor::TopLeft);
        assert_eq!(rules.gravity, PositionerGravity::Left);
        assert_eq!(rules.constraint_adjustment.bits(), 0b101010);
        assert_eq!(rules.offset, (-2, 9));
        assert!(rules.reactive);
        assert_eq!(rules.parent_configure_serial, Some(4242));
        assert_eq!(rules.parent_size, Some((800, 600)));

        let back = unsafe { PositionerRules::from_c(&rules.to_c()) };
        assert_eq!(back, rules);
    }

    /// `parent_size` and `parent_configure_serial` are `Option` on purpose: a
    /// client that never sent `set_parent_size`/`set_parent_configure` leaves
    /// zeroes there, and reporting `Some((0, 0))` would be indistinguishable
    /// from a client that really did send a zero size. wlroots flags the serial
    /// with `has_parent_configure_serial`; for the size, the zero *is* the
    /// sentinel wlroots itself uses.
    #[test]
    fn an_unsent_parent_size_and_serial_read_as_none() {
        let rules = unsafe { PositionerRules::from_c(&raw_rules((0, 0, 1, 1), (10, 10), 0, 0, 0)) };
        assert_eq!(rules.parent_size, None);
        assert_eq!(rules.parent_configure_serial, None);
    }
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -20
```

Expected failure: `error[E0433]: failed to resolve: use of undeclared type
`PositionerRules``, repeated for each new test.

**Step 3 — implement.** Append to `popup.rs`, above `mod tests`:

```rust
/// A snapshot of the client's `xdg_positioner`, copied out of
/// `wlr_xdg_positioner_rules`.
///
/// **Copied, never borrowed.** Every accessor that hands one of these back
/// releases wlroots' memory before returning, because the caller will re-enter
/// wlroots — configuring, dismissing, positioning — which can emit a signal,
/// which can destroy the very popup the rules were read from. A borrowed view
/// would be a use-after-free the borrow checker could not see, since the
/// lifetime would be tied to the handle rather than to wlroots' own decisions.
///
/// The two methods are FFI calls into wlroots rather than the placement algebra
/// they look like, for the reason [`geom`](crate::Box2D)'s module doc gives
/// about `wlr_box` predicates: xdg-shell's anchor/gravity/adjustment rules have
/// edge cases whose answers are not the obvious ones, and a reimplementation is
/// free to drift from wlroots' answer, silently, in a patch release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionerRules {
    /// `xdg_positioner.set_anchor_rect`, in the parent's window-geometry space.
    pub anchor_rect: Box2D,
    /// `xdg_positioner.set_anchor`.
    pub anchor: PositionerAnchor,
    /// `xdg_positioner.set_gravity`.
    pub gravity: PositionerGravity,
    /// `xdg_positioner.set_constraint_adjustment`.
    pub constraint_adjustment: ConstraintAdjustment,
    /// `xdg_positioner.set_size`, as `(width, height)`.
    pub size: (i32, i32),
    /// `xdg_positioner.set_parent_size` (protocol v3+). `Some` only when the
    /// client actually sent one — see this type's own round-trip test for why
    /// a zero cannot be reported as `Some((0, 0))`.
    pub parent_size: Option<(i32, i32)>,
    /// `xdg_positioner.set_offset`.
    pub offset: (i32, i32),
    /// `xdg_positioner.set_reactive`: the client wants the popup
    /// re-unconstrained whenever its parent moves.
    pub reactive: bool,
    /// `xdg_positioner.set_parent_configure`. `Some` only when the client sent
    /// one, which wlroots flags with `has_parent_configure_serial`.
    pub parent_configure_serial: Option<u32>,
}

impl PositionerRules {
    /// `wlr_xdg_positioner_rules_get_geometry` — the **unconstrained** geometry
    /// these rules describe, in the parent surface's coordinate system.
    #[must_use]
    pub fn geometry(&self) -> Box2D {
        let rules = self.to_c();
        let mut out = Box2D::default();
        // SAFETY: `rules` is a live, exclusively-owned local of exactly the C
        // type; `out` is a live local whose layout is pinned to `wlr_box` by
        // `geom.rs`'s compile-time asserts. wlroots only reads the first and
        // only writes the second.
        unsafe {
            sys::wlr_xdg_positioner_rules_get_geometry(
                &raw const rules,
                (&raw mut out).cast::<sys::wlr_box>(),
            );
        }
        out
    }

    /// `wlr_xdg_positioner_rules_unconstrain_box` — these rules applied against
    /// a constraint box, **without touching any live popup**.
    ///
    /// The answer is whatever wlroots' own algorithm produces given the
    /// adjustment bits the client permitted: with none permitted the result is
    /// [`geometry`](Self::geometry) unchanged, however far outside the
    /// constraint that falls. This crate invents no clamp of its own.
    #[must_use]
    pub fn unconstrain_box(&self, constraint: &Box2D) -> Box2D {
        let rules = self.to_c();
        let mut out = self.geometry();
        // SAFETY: as for `geometry`, plus `constraint.as_c()` which points at
        // the caller's live `Box2D` and is only read.
        unsafe {
            sys::wlr_xdg_positioner_rules_unconstrain_box(
                &raw const rules,
                constraint.as_c(),
                (&raw mut out).cast::<sys::wlr_box>(),
            );
        }
        out
    }

    /// Copy the rules out of wlroots' own struct.
    ///
    /// # Safety
    ///
    /// `rules` must point at a live, initialised `wlr_xdg_positioner_rules`.
    /// Only reads; nothing is retained.
    pub(crate) unsafe fn from_c(rules: &sys::wlr_xdg_positioner_rules) -> PositionerRules {
        PositionerRules {
            anchor_rect: Box2D::new(
                rules.anchor_rect.x,
                rules.anchor_rect.y,
                rules.anchor_rect.width,
                rules.anchor_rect.height,
            ),
            anchor: PositionerAnchor::from_raw(rules.anchor.0),
            gravity: PositionerGravity::from_raw(rules.gravity.0),
            constraint_adjustment: ConstraintAdjustment::from_raw(rules.constraint_adjustment.0),
            size: (rules.size.width, rules.size.height),
            // A parent size the client never sent is left zeroed by wlroots,
            // and "the client asked for 0x0" is not a thing xdg-shell permits,
            // so the zero is a usable sentinel. The serial has a real flag and
            // uses it.
            parent_size: if rules.parent_size.width == 0 && rules.parent_size.height == 0 {
                None
            } else {
                Some((rules.parent_size.width, rules.parent_size.height))
            },
            offset: (rules.offset.x, rules.offset.y),
            reactive: rules.reactive,
            parent_configure_serial: if rules.has_parent_configure_serial {
                Some(rules.parent_configure_serial)
            } else {
                None
            },
        }
    }

    /// Rebuild wlroots' own struct from this copy, so the two `rules_*`
    /// functions have something to point at.
    ///
    /// Zeroed first rather than field-by-field constructed: the struct is plain
    /// data (boxes, sizes, offsets, three enums, two bools — no pointers, no
    /// `wl_listener`), so materialising a zero value is sound, unlike the
    /// role-object structs `runtime.rs`'s tests must only ever touch through a
    /// raw pointer.
    pub(crate) fn to_c(&self) -> sys::wlr_xdg_positioner_rules {
        // SAFETY: every field of `wlr_xdg_positioner_rules` is an integer, a
        // bool, or a `#[repr(C)]` struct of those; none has a validity
        // requirement an all-zero pattern violates, and every field is
        // overwritten below except the padding.
        let mut rules: sys::wlr_xdg_positioner_rules = unsafe { std::mem::zeroed() };
        rules.anchor_rect = sys::wlr_box {
            x: self.anchor_rect.x,
            y: self.anchor_rect.y,
            width: self.anchor_rect.width,
            height: self.anchor_rect.height,
        };
        rules.anchor = sys::xdg_positioner_anchor(anchor_to_raw(self.anchor));
        rules.gravity = sys::xdg_positioner_gravity(gravity_to_raw(self.gravity));
        rules.constraint_adjustment =
            sys::xdg_positioner_constraint_adjustment(self.constraint_adjustment.bits());
        rules.size.width = self.size.0;
        rules.size.height = self.size.1;
        let (pw, ph) = self.parent_size.unwrap_or((0, 0));
        rules.parent_size.width = pw;
        rules.parent_size.height = ph;
        rules.offset.x = self.offset.0;
        rules.offset.y = self.offset.1;
        rules.reactive = self.reactive;
        rules.has_parent_configure_serial = self.parent_configure_serial.is_some();
        rules.parent_configure_serial = self.parent_configure_serial.unwrap_or(0);
        rules
    }
}

/// The inverse of [`PositionerAnchor::from_raw`]. Total, and pinned by the same
/// test: a variant added to the enum without a number here would not compile.
fn anchor_to_raw(anchor: PositionerAnchor) -> u32 {
    match anchor {
        PositionerAnchor::None => 0,
        PositionerAnchor::Top => 1,
        PositionerAnchor::Bottom => 2,
        PositionerAnchor::Left => 3,
        PositionerAnchor::Right => 4,
        PositionerAnchor::TopLeft => 5,
        PositionerAnchor::BottomLeft => 6,
        PositionerAnchor::TopRight => 7,
        PositionerAnchor::BottomRight => 8,
    }
}

/// The inverse of [`PositionerGravity::from_raw`]; see [`anchor_to_raw`].
fn gravity_to_raw(gravity: PositionerGravity) -> u32 {
    match gravity {
        PositionerGravity::None => 0,
        PositionerGravity::Top => 1,
        PositionerGravity::Bottom => 2,
        PositionerGravity::Left => 3,
        PositionerGravity::Right => 4,
        PositionerGravity::TopLeft => 5,
        PositionerGravity::BottomLeft => 6,
        PositionerGravity::TopRight => 7,
        PositionerGravity::BottomRight => 8,
    }
}
```

Extend the `use` line at the top of `popup.rs`:

```rust
use crate::{Box2D, LayerSurfaceId, ToplevelId, sys};
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: `test result: ok. 13 passed`.

**Mutation checks (perform both).**

1. In `from_c`, change the `parent_size` arm to
   `Some((rules.parent_size.width, rules.parent_size.height))` unconditionally.
   `an_unsent_parent_size_and_serial_read_as_none` must fail. Restore.
2. In `unconstrain_box`, replace the body with `self.geometry()` (drop the C
   call). `unconstraining_slides_a_popup_back_inside_when_sliding_is_permitted`
   must fail on the `fitted.x + fitted.width <= 200` assertion. Restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/popup.rs
git commit -m "$(cat <<'EOF'
feat(wlr): PositionerRules, copied out of wlroots and never borrowed

Every accessor that hands a consumer the client's positioner releases wlroots'
memory before returning, because the caller re-enters wlroots — configuring,
dismissing, positioning — which can emit a signal that destroys the popup the
rules came from. `geometry` and `unconstrain_box` rebuild the C struct from the
copy rather than reimplementing xdg-shell's placement algebra, for the reason
`geom.rs` gives about `wlr_box` predicates: a reimplementation is free to drift
from wlroots' answer in a patch release.

An unsent `set_parent_size`/`set_parent_configure` reads as `None`, not as a
zero indistinguishable from a real one. Nonsense enum values and degenerate
sizes from an untrusted client are converted and computed without panicking,
which is directly tested across the u32 boundaries.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3 — `Popup<'h>`: the borrow-scoped handle

**Files:** `crates/wlr/src/popup.rs`.

**Interfaces.**

*Consumes:* Task 1's `PopupId`/`PopupParent`, Task 2's `PositionerRules`,
`crate::Box2D`.

*Produces:*

```rust
pub struct Popup<'h> { /* raw: NonNull<sys::wlr_xdg_popup>, id, parent, _scope */ }

impl<'h> Popup<'h> {
    pub(crate) unsafe fn from_raw_with_id(
        raw: *mut sys::wlr_xdg_popup,
        id: PopupId,
        parent: PopupParent,
    ) -> Popup<'h>;
}

impl Popup<'_> {
    #[must_use] pub fn id(&self) -> PopupId;
    #[must_use] pub fn parent(&self) -> PopupParent;
    #[must_use] pub fn anchor_rect(&self) -> Box2D;
    #[must_use] pub fn positioner_rules(&self) -> PositionerRules;
    #[must_use] pub fn geometry(&self) -> Box2D;
    #[must_use] pub fn is_reactive(&self) -> bool;
    #[must_use] pub fn grab_requested(&self) -> bool;
    pub fn unconstrain(&self, constraint: &Box2D);
    #[must_use] pub fn position(&self) -> (f64, f64);
    #[must_use] pub fn toplevel_coords(&self, popup_sx: i32, popup_sy: i32) -> (i32, i32);
    pub fn send_configure(&self) -> u32;
    #[must_use] pub fn reposition_token(&self) -> Option<u32>;
    pub fn destroy(&self);
}
```

> **Note on `from_raw_with_id`'s third parameter.** The contract's §1.2 writes
> the constructor as `pub(crate) unsafe fn from_raw_with_id` without listing its
> parameters, and §1.2's `parent()` doc requires the parent to be "the parent
> recorded when this popup was created… never re-derived from `(*popup).parent`
> at call time". The recording lives in `PopupEntry` (Task 4), so the parent has
> to reach the handle through its constructor. This is not a deviation: the
> contract froze the public method list, and `from_raw_with_id` is `pub(crate)`.

**Step 1 — write the failing tests.** Append to `popup.rs`'s `mod tests`:

```rust
    /// Allocate a zeroed `wlr_xdg_popup` with a zeroed `wlr_xdg_surface` behind
    /// its `base`, and leak both.
    ///
    /// `alloc_zeroed` behind a raw pointer rather than `Box::new(zeroed())`,
    /// for the reason `runtime.rs`'s `record_toplevel_with_surface` documents:
    /// both structs embed `wl_listener`s whose bare function pointers are UB to
    /// *materialise* as a zero value, so the bytes are only ever touched
    /// through a pointer. Deliberately leaked — these tests never enter
    /// wlroots' own lifecycle, so nothing frees them, and a reclaimed
    /// allocation would leave a dangling `base` for any later assertion.
    fn scratch_popup(initialized: bool) -> *mut sys::wlr_xdg_popup {
        use std::alloc::{Layout, alloc_zeroed};

        // SAFETY: both layouts are non-zero-sized, so `alloc_zeroed` returns
        // either null (checked) or a suitably aligned zeroed allocation.
        let base = unsafe { alloc_zeroed(Layout::new::<sys::wlr_xdg_surface>()) }
            .cast::<sys::wlr_xdg_surface>();
        assert!(!base.is_null(), "allocation failed");
        let popup =
            unsafe { alloc_zeroed(Layout::new::<sys::wlr_xdg_popup>()) }.cast::<sys::wlr_xdg_popup>();
        assert!(!popup.is_null(), "allocation failed");
        // SAFETY: both allocations are freshly zeroed and exclusively owned;
        // every field written below is in bounds.
        unsafe {
            (*base).initialized = initialized;
            (*popup).base = base;
        }
        popup
    }

    /// A handle over a scratch popup with the given parent.
    ///
    /// # Safety
    ///
    /// The handle must not outlive `raw`; these tests leak `raw`, so it does
    /// not.
    unsafe fn handle(raw: *mut sys::wlr_xdg_popup, parent: PopupParent) -> Popup<'static> {
        unsafe { Popup::from_raw_with_id(raw, PopupId::dangling_for_test(), parent) }
    }

    #[test]
    fn a_handle_reports_the_parent_it_was_built_with_not_the_null_in_the_struct() {
        let raw = scratch_popup(false);
        let parent = PopupParent::Layer(LayerSurfaceId::dangling_for_test());
        // SAFETY: `raw` is leaked, so it outlives the handle.
        let popup = unsafe { handle(raw, parent) };
        assert_eq!(popup.parent(), parent);
        assert!(!popup.parent().is_popup());
        // `(*raw).parent` is the zeroed null a layer popup really does carry at
        // creation. If `parent()` ever started reading it, this test is the one
        // that notices.
        // SAFETY: `raw` is a live, exclusively-owned allocation.
        assert!(unsafe { (*raw).parent.is_null() });
    }

    /// `send_configure` must **skip the call** — not merely return zero — when
    /// the surface is not yet `initialized`. `wlr_xdg_surface_schedule_configure`
    /// contains `assert(surface->initialized)`, and this distribution ships
    /// wlroots without `NDEBUG`, so an early call is a hard `abort()` of the
    /// whole compositor process (the hazard `layer.rs`'s module doc documents
    /// in full for `wlr_layer_surface_v1_configure`). This test would *abort*
    /// rather than fail if the guard were removed, which is exactly the point:
    /// an aborting test binary is a louder failure than a red assertion.
    #[test]
    fn send_configure_is_skipped_entirely_before_the_surface_is_initialized() {
        let raw = scratch_popup(false);
        // SAFETY: `raw` is leaked, so it outlives the handle.
        let popup = unsafe { handle(raw, PopupParent::Toplevel(ToplevelId::dangling_for_test())) };
        assert_eq!(popup.send_configure(), 0);
    }

    /// `grab_requested` is `(*popup).seat != NULL` — wlroots fills that field
    /// only when the client sends `xdg_popup.grab`. It is the only observable
    /// this crate has for a popup grab: the export table has no
    /// `wlr_xdg_popup_grab_*` symbol at all.
    #[test]
    fn grab_requested_reads_the_seat_pointer_wlroots_fills_on_the_grab_request() {
        let raw = scratch_popup(false);
        // SAFETY: `raw` is leaked and exclusively owned.
        let popup = unsafe { handle(raw, PopupParent::Toplevel(ToplevelId::dangling_for_test())) };
        assert!(!popup.grab_requested(), "a zeroed seat is no grab");

        // SAFETY: writing a non-null sentinel into a field this crate only ever
        // null-checks. The pointer is never dereferenced by `grab_requested`.
        unsafe { (*raw).seat = std::ptr::dangling_mut() };
        assert!(popup.grab_requested());
    }

    /// The reposition token is present only when wlroots has set the
    /// `WLR_XDG_POPUP_CONFIGURE_REPOSITION_TOKEN` bit in `scheduled.fields`.
    /// Reading the token without checking the mask would report a stale token
    /// from a previous reposition on every ordinary configure.
    #[test]
    fn the_reposition_token_is_none_until_wlroots_sets_its_field_bit() {
        let raw = scratch_popup(false);
        // SAFETY: `raw` is leaked and exclusively owned.
        let popup = unsafe { handle(raw, PopupParent::Toplevel(ToplevelId::dangling_for_test())) };
        assert_eq!(popup.reposition_token(), None);

        // SAFETY: as above; both fields are plain integers in bounds.
        unsafe { (*raw).scheduled.reposition_token = 77 };
        assert_eq!(
            popup.reposition_token(),
            None,
            "a token with the field bit clear is stale, not current"
        );

        // SAFETY: as above.
        unsafe {
            (*raw).scheduled.fields = sys::wlr_xdg_popup_configure_field
                ::WLR_XDG_POPUP_CONFIGURE_REPOSITION_TOKEN
                .0;
        }
        assert_eq!(popup.reposition_token(), Some(77));
    }

    /// `geometry` reads `current`, `anchor_rect`/`positioner_rules` read
    /// `scheduled.rules` — the distinction between "where it actually is" and
    /// "what the client last asked for", which a compositor deciding a
    /// reposition needs both halves of.
    #[test]
    fn geometry_reads_current_while_the_rules_read_what_was_scheduled() {
        let raw = scratch_popup(false);
        // SAFETY: `raw` is leaked and exclusively owned; every field written is
        // plain data in bounds.
        unsafe {
            (*raw).current.geometry = sys::wlr_box { x: 1, y: 2, width: 3, height: 4 };
            (*raw).current.reactive = true;
            (*raw).scheduled.rules.anchor_rect =
                sys::wlr_box { x: 10, y: 20, width: 30, height: 40 };
            (*raw).scheduled.rules.size.width = 64;
            (*raw).scheduled.rules.size.height = 48;
        }
        let popup = unsafe { handle(raw, PopupParent::Toplevel(ToplevelId::dangling_for_test())) };

        assert_eq!(popup.geometry(), Box2D::new(1, 2, 3, 4));
        assert!(popup.is_reactive());
        assert_eq!(popup.anchor_rect(), Box2D::new(10, 20, 30, 40));
        assert_eq!(popup.positioner_rules().size, (64, 48));
        assert_eq!(popup.positioner_rules().anchor_rect, Box2D::new(10, 20, 30, 40));
    }

    /// The `Debug` impl is hand-written and prints named fields only. A derive
    /// would print the raw pointer, which is neither useful nor stable across
    /// runs — the reason `Toplevel`'s own `Debug` is hand-written.
    #[test]
    fn the_debug_impl_prints_named_fields_and_no_pointer() {
        let raw = scratch_popup(false);
        // SAFETY: `raw` is leaked and exclusively owned.
        let popup = unsafe { handle(raw, PopupParent::Toplevel(ToplevelId::dangling_for_test())) };
        let text = format!("{popup:?}");
        assert!(text.starts_with("Popup {"), "got {text}");
        assert!(text.contains("id: PopupId("), "got {text}");
        assert!(text.contains("parent: Toplevel("), "got {text}");
        assert!(!text.contains("0x"), "a raw pointer leaked into Debug: {text}");
    }
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -20
```

Expected failure: `error[E0433]: failed to resolve: use of undeclared type
`Popup``.

**Step 3 — implement.** Append to `popup.rs`, above `mod tests`:

```rust
/// An xdg popup, borrowed for the duration of a handler call.
///
/// Same shape as [`Toplevel`](crate::Toplevel) and for the same reason: a
/// `wlr_xdg_popup` is freed whenever its client says so, so a handle that
/// escapes the handler it was passed to is a use-after-free. The lifetime and
/// the private constructor make that a compile error rather than a documented
/// rule. What you store instead is [`PopupId`].
pub struct Popup<'h> {
    raw: NonNull<sys::wlr_xdg_popup>,
    id: PopupId,
    parent: PopupParent,
    _scope: PhantomData<&'h ()>,
}

/// Hand-written rather than derived, for the same reason [`Toplevel`]'s is: the
/// `PhantomData` scope marker has no value to print, and a raw pointer printed
/// by a derive is neither useful nor stable across runs. Named fields only:
/// `id`, `parent`, `geometry`.
impl std::fmt::Debug for Popup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Popup")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("geometry", &self.geometry())
            .finish()
    }
}

impl<'h> Popup<'h> {
    /// # Safety
    ///
    /// `raw` must be a live `wlr_xdg_popup` whose `base->surface` carries the id
    /// addon that produced `id`, `parent` must be the parent recorded when the
    /// popup was announced, and the returned handle must not outlive the
    /// callback it was created for.
    pub(crate) unsafe fn from_raw_with_id(
        raw: *mut sys::wlr_xdg_popup,
        id: PopupId,
        parent: PopupParent,
    ) -> Popup<'h> {
        Popup {
            raw: NonNull::new(raw).expect("wlroots handed us a null popup"),
            id,
            parent,
            _scope: PhantomData,
        }
    }
}

impl Popup<'_> {
    /// This popup's stable identity, safe to store beyond the handler.
    #[must_use]
    pub fn id(&self) -> PopupId {
        self.id
    }

    /// The parent recorded when this popup was created.
    ///
    /// **Never re-derived from `(*popup).parent` at call time.** A layer-shell
    /// popup's xdg parent is NULL at creation — the client sets it with
    /// `zwlr_layer_surface_v1.get_popup` afterwards — so reading the struct
    /// would report "no parent" for exactly the case this enum exists to
    /// distinguish.
    #[must_use]
    pub fn parent(&self) -> PopupParent {
        self.parent
    }

    /// `scheduled.rules.anchor_rect` — the rectangle the client anchored
    /// against, in the parent's window-geometry space.
    #[must_use]
    pub fn anchor_rect(&self) -> Box2D {
        self.positioner_rules().anchor_rect
    }

    /// The client's full positioner, copied out.
    ///
    /// Reads `scheduled.rules` — what the client last asked for — not `current`.
    /// See [`PositionerRules`] for why the copy is not negotiable.
    #[must_use]
    pub fn positioner_rules(&self) -> PositionerRules {
        // SAFETY: the handle's lifetime guarantees the popup is live;
        // `scheduled` is a plain embedded struct, not a pointer.
        unsafe { PositionerRules::from_c(&(*self.raw.as_ptr()).scheduled.rules) }
    }

    /// `current.geometry` — where the popup actually is, once configured, in the
    /// parent's window-geometry coordinates.
    ///
    /// Zero-sized before the first configure has been acked, which is what
    /// wlroots' own zeroed `wlr_xdg_popup_state` starts at.
    #[must_use]
    pub fn geometry(&self) -> Box2D {
        // SAFETY: the handle's lifetime guarantees the popup is live; `current`
        // is a plain embedded `wlr_xdg_popup_state`.
        let state: &sys::wlr_xdg_popup_state = unsafe { &(*self.raw.as_ptr()).current };
        Box2D::new(
            state.geometry.x,
            state.geometry.y,
            state.geometry.width,
            state.geometry.height,
        )
    }

    /// `current.reactive`: the client wants this popup re-unconstrained
    /// whenever its parent moves.
    ///
    /// A compositor honouring this re-runs
    /// [`Runtime::configure_popup`](crate::Runtime::configure_popup) from
    /// wherever it moves a window or re-arranges its layers.
    #[must_use]
    pub fn is_reactive(&self) -> bool {
        // SAFETY: as for `geometry`.
        let state: &sys::wlr_xdg_popup_state = unsafe { &(*self.raw.as_ptr()).current };
        state.reactive
    }

    /// `(*popup).seat != NULL` — the client sent `xdg_popup.grab`.
    ///
    /// **wlroots owns the grab's lifetime**; this crate only observes it. See
    /// this module's own doc for what that rules out. A `true` here means
    /// wlroots has installed pointer, keyboard and touch grabs for this popup's
    /// chain and will dismiss the chain itself on a press outside it.
    #[must_use]
    pub fn grab_requested(&self) -> bool {
        // SAFETY: the handle's lifetime guarantees the popup is live; the field
        // is only compared against null, never dereferenced.
        unsafe { !(*self.raw.as_ptr()).seat.is_null() }
    }

    /// `wlr_xdg_popup_unconstrain_from_box`: rewrite `scheduled.geometry` from
    /// the rules, fitted into `constraint`.
    ///
    /// **`constraint` is in the ROOT TOPLEVEL PARENT surface's coordinate
    /// system**, not layout or output space — wlroots' own header says so, and
    /// the compositor is what translates. Sends nothing; pair it with
    /// [`send_configure`](Self::send_configure), which is what
    /// [`Runtime::configure_popup`](crate::Runtime::configure_popup) does.
    pub fn unconstrain(&self, constraint: &Box2D) {
        // SAFETY: the handle's lifetime guarantees the popup is live;
        // `constraint.as_c()` points at the caller's live `Box2D`, whose layout
        // is pinned to `wlr_box`, and wlroots only reads it.
        unsafe { sys::wlr_xdg_popup_unconstrain_from_box(self.raw.as_ptr(), constraint.as_c()) };
    }

    /// `wlr_xdg_popup_get_position` — this popup's position in the **parent
    /// surface's** coordinates.
    #[must_use]
    pub fn position(&self) -> (f64, f64) {
        let mut sx = 0.0;
        let mut sy = 0.0;
        // SAFETY: the handle's lifetime guarantees the popup is live; both
        // out-parameters point at live locals that outlive the call.
        unsafe { sys::wlr_xdg_popup_get_position(self.raw.as_ptr(), &raw mut sx, &raw mut sy) };
        (sx, sy)
    }

    /// `wlr_xdg_popup_get_toplevel_coords` — a surface-local point mapped into
    /// the **root toplevel's** surface coordinates, walking the whole popup
    /// chain.
    ///
    /// This is what turns a hit inside a nested submenu into the coordinate
    /// space [`unconstrain`](Self::unconstrain)'s constraint box is expressed
    /// in.
    #[must_use]
    pub fn toplevel_coords(&self, popup_sx: i32, popup_sy: i32) -> (i32, i32) {
        let mut tx = 0;
        let mut ty = 0;
        // SAFETY: the handle's lifetime guarantees the popup is live; both
        // out-parameters point at live `c_int` locals. The C signature takes
        // `int`, not `int32_t`, which is the same type on every target this
        // crate builds for.
        unsafe {
            sys::wlr_xdg_popup_get_toplevel_coords(
                self.raw.as_ptr(),
                popup_sx,
                popup_sy,
                &raw mut tx,
                &raw mut ty,
            );
        }
        (tx, ty)
    }

    /// `wlr_xdg_surface_schedule_configure(base)`. Returns the configure serial,
    /// or `0` when the surface is not `initialized` yet — **in which case the
    /// call is skipped entirely**.
    ///
    /// That skip is not a convenience. `wlr_xdg_surface_schedule_configure`
    /// asserts `surface->initialized`, and this distribution ships wlroots
    /// **without `NDEBUG`**, so calling it before the popup's first commit is a
    /// hard `abort()` of the whole compositor process — the identical,
    /// confirmed hazard `layer.rs`'s module doc documents at length for
    /// `wlr_layer_surface_v1_configure`. The guard must never be simplified
    /// away to an unconditional send.
    pub fn send_configure(&self) -> u32 {
        // SAFETY: the handle's lifetime guarantees the popup is live, so `base`
        // is a live `wlr_xdg_surface`; `initialized` is a plain bool.
        unsafe {
            let base = (*self.raw.as_ptr()).base;
            if base.is_null() || !(*base).initialized {
                return 0;
            }
            sys::wlr_xdg_surface_schedule_configure(base)
        }
    }

    /// `scheduled.reposition_token`, but only when
    /// `scheduled.fields & WLR_XDG_POPUP_CONFIGURE_REPOSITION_TOKEN` is set.
    ///
    /// `None` on an ordinary configure. wlroots sends the
    /// `xdg_popup.repositioned` event itself off that field; **never forge
    /// one** — this accessor exists so a compositor can *observe* the token,
    /// for logging or for correlating its own state, not so it can echo it.
    #[must_use]
    pub fn reposition_token(&self) -> Option<u32> {
        // SAFETY: the handle's lifetime guarantees the popup is live;
        // `scheduled` is a plain embedded `wlr_xdg_popup_configure`.
        let scheduled: &sys::wlr_xdg_popup_configure = unsafe { &(*self.raw.as_ptr()).scheduled };
        let bit =
            sys::wlr_xdg_popup_configure_field::WLR_XDG_POPUP_CONFIGURE_REPOSITION_TOKEN.0;
        if scheduled.fields & bit == 0 {
            None
        } else {
            Some(scheduled.reposition_token)
        }
    }

    /// `wlr_xdg_popup_destroy` — sends `xdg_popup.popup_done`, destroys the role
    /// object and makes the resource inert.
    ///
    /// Does **not** touch the scene tree: the popup's subtree is a child of its
    /// parent's, and wlroots frees a tree's children recursively with the tree.
    /// Destroying it here would be the double free `Runtime::forget_toplevel`
    /// documents. Prefer
    /// [`Runtime::dismiss_popup`](crate::Runtime::dismiss_popup), which
    /// destroys a whole chain deepest-first — the order xdg-shell requires.
    pub fn destroy(&self) {
        // SAFETY: the handle's lifetime guarantees the popup is live. wlroots
        // emits `events.destroy` from inside this call, which runs
        // `on_popup_destroy`, which removes this popup's entry and listeners
        // before wlroots frees anything — the same ordering `on_toplevel_destroy`
        // relies on.
        unsafe { sys::wlr_xdg_popup_destroy(self.raw.as_ptr()) };
    }
}
```

Extend `popup.rs`'s imports to:

```rust
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::id::next_id;
use crate::{Box2D, LayerSurfaceId, ToplevelId, sys};
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup:: 2>&1 | tail -25
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: `test result: ok. 19 passed`.

**Mutation checks (perform all three).**

1. In `send_configure`, delete the `|| !(*base).initialized` clause. Re-run
   `cargo test -p wlr --lib popup::`. The binary **aborts** with
   `Assertion 'surface->initialized' failed` rather than failing an assertion —
   that abort is the pass condition for this mutation, and it is exactly the
   compositor-killing failure the guard exists to prevent. Restore.
2. In `reposition_token`, drop the mask check and always return
   `Some(scheduled.reposition_token)`.
   `the_reposition_token_is_none_until_wlroots_sets_its_field_bit` must fail on
   its first assertion. Restore.
3. In `parent`, replace the body with a read of `(*self.raw.as_ptr()).parent`
   turned into a `PopupParent` (it will not type-check — instead, to make the
   mutation compilable, change the field `parent` to be overwritten with
   `PopupParent::Toplevel(ToplevelId::dangling_for_test())` inside
   `from_raw_with_id`).
   `a_handle_reports_the_parent_it_was_built_with_not_the_null_in_the_struct`
   must fail. Restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/popup.rs
git commit -m "$(cat <<'EOF'
feat(wlr): the borrow-scoped Popup handle

Mirrors `Toplevel` exactly — `NonNull` raw plus id plus a `PhantomData` scope,
a private constructor, and a hand-written `Debug` that prints named fields and
no pointer.

`parent()` returns the parent recorded at announcement time and never reads
`(*popup).parent`, which is NULL for exactly the case the enum exists to
distinguish: a layer-shell popup is created with a null xdg parent and
reparented afterwards.

`send_configure` skips the call outright when the surface is not yet
initialized. `wlr_xdg_surface_schedule_configure` asserts on that flag and this
distribution ships wlroots without NDEBUG, so an early call aborts the whole
compositor — the same confirmed hazard `layer.rs` documents. The test for it
aborts rather than fails when the guard is removed, which is the loudest
possible signal.

`reposition_token` checks wlroots' field bit before reading the token, so an
ordinary configure reports `None` instead of a stale token from a previous
reposition.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4 — `Runtime`'s popup table, the chain walkers, and the public re-exports

**Files:** `crates/wlr/src/runtime.rs`, `crates/wlr/src/lib.rs`,
`crates/wlr/src/popup.rs`.

**Interfaces.**

*Consumes:* Task 1–3's `PopupId`, `PopupParent`, `Popup<'h>`.

*Produces:*

```rust
// runtime.rs
pub(crate) struct PopupEntry {
    pub(crate) raw: NonNull<sys::wlr_xdg_popup>,
    pub(crate) tree: NonNull<sys::wlr_scene_tree>,
    pub(crate) parent: PopupParent,
    pub(crate) configured: Cell<bool>,
}

impl Runtime {
    pub(crate) fn record_popup(&self, id: PopupId, raw: NonNull<sys::wlr_xdg_popup>,
                               tree: NonNull<sys::wlr_scene_tree>, parent: PopupParent);
    pub(crate) fn forget_popup(&self, id: PopupId);
    pub(crate) fn popup_raw(&self, id: PopupId) -> Option<NonNull<sys::wlr_xdg_popup>>;
    pub(crate) fn popup_tree(&self, id: PopupId) -> Option<NonNull<sys::wlr_scene_tree>>;
    pub(crate) fn clear_popups(&self);

    pub fn popup(&self, id: PopupId) -> Option<Popup<'_>>;
    pub fn popup_parent(&self, id: PopupId) -> Option<PopupParent>;
    pub fn popups_of(&self, parent: PopupParent) -> Vec<PopupId>;
    pub fn popup_chain(&self, parent: PopupParent) -> Vec<PopupId>;
}

// popup.rs
impl PopupParent { pub fn root(self, rt: &Runtime) -> Option<PopupParent>; }
```

> **On the contract's `drop_all_popups`.** §1.5 names the run-granularity purge
> `drop_all_popups`, and its own sentence says it "mirror[s] the toplevel
> quartet". The toplevel quartet's fourth member is actually named
> `clear_toplevels` (`runtime.rs:6927`), and `run_inner` calls that name. This
> plan uses **`clear_popups`** so the pair reads together at the call site. It is
> `pub(crate)`, so no contract signature changes; recorded here for the reader,
> not as a §10 amendment.

**Step 1 — write the failing tests.** Append to `runtime.rs`'s existing
`#[cfg(test)] mod tests`:

```rust
    /// Record a popup with a heap-allocated `wlr_xdg_popup`/`wlr_xdg_surface`
    /// pair, and return the id.
    ///
    /// `alloc_zeroed` behind a raw pointer and deliberately leaked, for the
    /// reasons `record_toplevel_with_surface` above already documents in full:
    /// the structs embed `wl_listener`s whose function pointers are UB to
    /// materialise as zero, and these tests never enter wlroots' lifecycle so
    /// nothing frees them.
    fn record_scratch_popup(rt: &Runtime, parent: PopupParent, initialized: bool) -> PopupId {
        use std::alloc::{Layout, alloc_zeroed};

        let id = PopupId(next_id());
        // SAFETY: both layouts are non-zero-sized, so `alloc_zeroed` returns
        // either null (checked) or a suitably aligned zeroed allocation.
        let base = unsafe { alloc_zeroed(Layout::new::<sys::wlr_xdg_surface>()) }
            .cast::<sys::wlr_xdg_surface>();
        assert!(!base.is_null(), "allocation failed");
        let popup = unsafe { alloc_zeroed(Layout::new::<sys::wlr_xdg_popup>()) }
            .cast::<sys::wlr_xdg_popup>();
        assert!(!popup.is_null(), "allocation failed");
        // SAFETY: both allocations are freshly zeroed and exclusively owned.
        unsafe {
            (*base).initialized = initialized;
            (*popup).base = base;
        }
        rt.record_popup(
            id,
            NonNull::new(popup).expect("allocation succeeded"),
            NonNull::<sys::wlr_scene_tree>::dangling(),
            parent,
        );
        id
    }

    #[test]
    fn an_unknown_popup_id_misses_rather_than_dereferencing() {
        let rt = Runtime::new().expect("runtime");
        let dead = PopupId::dangling_for_test();
        assert!(rt.popup(dead).is_none());
        assert_eq!(rt.popup_parent(dead), None);
        assert!(rt.popup_chain(PopupParent::Popup(dead)).is_empty());
        assert!(rt.popups_of(PopupParent::Popup(dead)).is_empty());
    }

    /// Direct children only, in creation order — which is also the z-order
    /// tiebreak the compositor's own stack relies on.
    #[test]
    fn popups_of_lists_direct_children_in_creation_order() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        let b = record_scratch_popup(&rt, window, false);
        let nested = record_scratch_popup(&rt, PopupParent::Popup(a), false);

        assert_eq!(rt.popups_of(window), vec![a, b]);
        assert_eq!(rt.popups_of(PopupParent::Popup(a)), vec![nested]);
        assert!(rt.popups_of(PopupParent::Popup(b)).is_empty());
    }

    /// The whole subtree, deepest last — the order a caller iterates to paint,
    /// and the *reverse* of the order it must destroy in.
    #[test]
    fn popup_chain_walks_the_whole_subtree_deepest_last() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let menu = record_scratch_popup(&rt, window, false);
        let submenu = record_scratch_popup(&rt, PopupParent::Popup(menu), false);
        let subsub = record_scratch_popup(&rt, PopupParent::Popup(submenu), false);
        let sibling = record_scratch_popup(&rt, window, false);

        let chain = rt.popup_chain(window);
        assert_eq!(chain.len(), 4, "every popup under the window: {chain:?}");
        assert_eq!(chain[0], menu, "a direct child comes before its own children");
        assert!(
            chain.iter().position(|p| *p == submenu) < chain.iter().position(|p| *p == subsub),
            "a parent must precede its child: {chain:?}"
        );
        assert!(chain.contains(&sibling));

        assert_eq!(rt.popup_chain(PopupParent::Popup(menu)), vec![submenu, subsub]);
        assert!(rt.popup_chain(PopupParent::Popup(subsub)).is_empty());
    }

    /// `root` walks `Popup(_)` links down to the window or layer at the bottom.
    #[test]
    fn a_popup_parent_resolves_to_the_window_or_layer_at_the_root() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let menu = record_scratch_popup(&rt, window, false);
        let submenu = record_scratch_popup(&rt, PopupParent::Popup(menu), false);

        assert_eq!(window.root(&rt), Some(window));
        assert_eq!(PopupParent::Popup(menu).root(&rt), Some(window));
        assert_eq!(PopupParent::Popup(submenu).root(&rt), Some(window));

        let layer = PopupParent::Layer(LayerSurfaceId::dangling_for_test());
        let panel_menu = record_scratch_popup(&rt, layer, false);
        assert_eq!(PopupParent::Popup(panel_menu).root(&rt), Some(layer));
    }

    /// A link already dead resolves to nothing rather than to a wrong window —
    /// the by-id contract every other accessor in this crate carries.
    #[test]
    fn a_root_walk_through_a_dead_link_is_none() {
        let rt = Runtime::new().expect("runtime");
        assert_eq!(
            PopupParent::Popup(PopupId::dangling_for_test()).root(&rt),
            None
        );
    }

    /// A cycle cannot arise from wlroots — a popup's parent is fixed at
    /// creation and a client cannot re-parent one — but `root` must terminate
    /// on a corrupted table anyway, because a hang in a compositor's input path
    /// is indistinguishable from a freeze to the user. The depth cap is what
    /// guarantees it.
    #[test]
    fn a_root_walk_terminates_even_on_a_cyclic_table() {
        let rt = Runtime::new().expect("runtime");
        let a = record_scratch_popup(&rt, PopupParent::Toplevel(ToplevelId::dangling_for_test()), false);
        let b = record_scratch_popup(&rt, PopupParent::Popup(a), false);
        // Forge the cycle a -> b -> a directly in the table.
        rt.inner
            .popups
            .borrow_mut()
            .get_mut(&a)
            .expect("recorded")
            .parent = PopupParent::Popup(b);

        assert_eq!(PopupParent::Popup(a).root(&rt), None, "cap, not hang");
    }

    /// A chain walk over the same cyclic table must terminate too, and must not
    /// return the same id twice — a caller destroying what it returns would
    /// double-destroy.
    #[test]
    fn a_chain_walk_terminates_and_never_repeats_an_id_on_a_cyclic_table() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        let b = record_scratch_popup(&rt, PopupParent::Popup(a), false);
        rt.inner
            .popups
            .borrow_mut()
            .get_mut(&a)
            .expect("recorded")
            .parent = PopupParent::Popup(b);

        let chain = rt.popup_chain(PopupParent::Popup(a));
        let mut seen = chain.clone();
        seen.sort_unstable_by_key(|p| p.0);
        seen.dedup();
        assert_eq!(seen.len(), chain.len(), "no id twice: {chain:?}");
    }

    /// Forgetting one popup leaves its siblings and its parent alone, and does
    /// **not** touch the scene tree: a popup's tree is a child of its parent's,
    /// and wlroots frees a tree's children recursively — the double free
    /// `forget_toplevel`'s own comment spells out.
    #[test]
    fn forgetting_a_popup_removes_only_that_row() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        let b = record_scratch_popup(&rt, window, false);

        rt.forget_popup(a);
        assert!(rt.popup(a).is_none());
        assert_eq!(rt.popups_of(window), vec![b]);
        assert_eq!(rt.popup_parent(b), Some(window));
    }

    /// `clear_popups` is the run-granularity purge `run_inner` calls when
    /// `run_all` returns, mirroring `clear_toplevels`: popup ids are only
    /// meaningful for the call that announced them, because the per-popup
    /// destroy listener that would otherwise remove a stale row is torn down
    /// with that call's `Session`.
    #[test]
    fn clear_popups_empties_the_table() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        record_scratch_popup(&rt, PopupParent::Popup(a), false);

        rt.clear_popups();
        assert!(rt.popups_of(window).is_empty());
        assert!(rt.popup(a).is_none());
    }
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib runtime::tests::popup 2>&1 | tail -20
```

Expected failure: compile errors — `no method named `record_popup` found for
struct `Runtime``, `no field `popups``, `failed to resolve: use of undeclared
type `PopupParent``.

**Step 3 — implement.**

3a. In `runtime.rs`'s import block, add `PopupId`, `PopupParent` and `Popup` to
the `use crate::{…}` list (alphabetically, beside `NodeId`/`OutputId`).

3b. Add the entry type beside `LayerSurfaceEntry` (`runtime.rs:741`):

```rust
/// One live popup as this crate tracks it.
///
/// Not `Copy`, unlike [`ToplevelEntry`]: `configured` is a `Cell`, and `Cell` is
/// never `Copy` regardless of what it holds. Every accessor that needs `raw` or
/// `tree` outside a held borrow copies just that field out —
/// [`Runtime::popup_raw`]/[`Runtime::popup_tree`] — the same narrowing
/// `LayerSurfaceEntry`'s accessors do for the identical reason.
///
/// `tree` is the subtree `wlr_scene_xdg_surface_create` built **under the
/// parent's tree**, which is what makes a popup stack with its parent for free
/// and what makes `leaf_surface_at` resolve clicks on it — including the
/// session-lock isolation gate — without a line of new code. It is stored and
/// dropped, **never destroyed**: wlroots frees a tree's children recursively
/// with the tree, so destroying a child of a dying parent is a double free (see
/// [`Runtime::forget_toplevel`]'s own comment).
pub(crate) struct PopupEntry {
    pub(crate) raw: NonNull<sys::wlr_xdg_popup>,
    pub(crate) tree: NonNull<sys::wlr_scene_tree>,
    /// The parent recorded at announcement time. A layer popup's
    /// `(*popup).parent` is NULL then, so this is the only place the answer
    /// exists — see [`PopupParent`]'s own doc.
    pub(crate) parent: PopupParent,
    /// Whether [`Runtime::configure_popup`] has ever sent a configure for this
    /// popup. Read by nothing in P1's own logic; it is the flag a compositor's
    /// reactive-reposition pass needs to tell "never placed" from "placed and
    /// due a re-place", and it is cheaper to record here, at the one site that
    /// knows, than to reconstruct downstream.
    pub(crate) configured: std::cell::Cell<bool>,
}
```

3c. Add the table to `RuntimeInner`, immediately after `toplevels`
(`runtime.rs:481`):

```rust
    /// Every live popup: the role object, its scene subtree, and the parent it
    /// was announced under.
    ///
    /// Purged per-popup by [`Runtime::forget_popup`] (from
    /// `on_popup_destroy`, before wlroots frees the popup) and wholesale by
    /// [`Runtime::clear_popups`] when the `run_all` call that announced them
    /// returns — the identical two-level discipline `toplevels` has, and for
    /// the identical reason.
    pub(crate) popups: RefCell<HashMap<PopupId, PopupEntry>>,
```

and its initialiser beside `toplevels: RefCell::new(HashMap::new()),`
(`runtime.rs:1175`):

```rust
                popups: RefCell::new(HashMap::new()),
```

3d. Add the accessors, immediately after `clear_toplevels`:

```rust
    /// The maximum popup nesting this crate will walk.
    ///
    /// wlroots cannot produce a cycle — a popup's parent is fixed when the role
    /// object is created and a client cannot re-parent one — and no real menu
    /// is anywhere near this deep. The cap is not about correctness of the
    /// happy path; it is about the failure mode. Every walk here runs on the
    /// compositor's input path, and an unbounded walk over a corrupted table
    /// would hang the session, which a user cannot distinguish from a freeze. A
    /// bounded walk returns a wrong-but-finite answer instead, and the tests
    /// that forge a cycle pin that it does.
    pub(crate) const MAX_POPUP_DEPTH: usize = 64;

    /// Record a newly-announced popup under `id`.
    ///
    /// Called from `backend.rs`'s `on_new_popup`, before the handler is told,
    /// mirroring [`record_toplevel`](Runtime::record_toplevel).
    pub(crate) fn record_popup(
        &self,
        id: PopupId,
        raw: NonNull<sys::wlr_xdg_popup>,
        tree: NonNull<sys::wlr_scene_tree>,
        parent: PopupParent,
    ) {
        self.inner.popups.borrow_mut().insert(
            id,
            PopupEntry {
                raw,
                tree,
                parent,
                configured: std::cell::Cell::new(false),
            },
        );
    }

    /// Remove `id`'s entry. Called from `on_popup_destroy` before the popup is
    /// freed, mirroring [`forget_toplevel`](Runtime::forget_toplevel).
    ///
    /// **Does not destroy the scene tree**, and must never grow that: a popup's
    /// tree is a child of its parent's, wlroots frees a tree's children
    /// recursively, and this runs while the parent may already be dying. See
    /// [`PopupEntry`]'s own doc.
    ///
    /// Children of this popup are **not** removed here. wlroots destroys a
    /// popup's own children first and emits a `destroy` for each, so each child
    /// removes itself through this same path; sweeping them here would race
    /// that and drop rows a still-pending emission is about to use.
    pub(crate) fn forget_popup(&self, id: PopupId) {
        self.inner.popups.borrow_mut().remove(&id);
    }

    /// This id's recorded raw popup, with the borrow released before returning
    /// — see [`toplevel_entry`](Runtime::toplevel_entry)'s own doc for why that
    /// is not optional.
    pub(crate) fn popup_raw(&self, id: PopupId) -> Option<NonNull<sys::wlr_xdg_popup>> {
        self.inner.popups.borrow().get(&id).map(|e| e.raw)
    }

    /// This id's recorded scene subtree, as [`popup_raw`](Runtime::popup_raw).
    pub(crate) fn popup_tree(&self, id: PopupId) -> Option<NonNull<sys::wlr_scene_tree>> {
        self.inner.popups.borrow().get(&id).map(|e| e.tree)
    }

    /// Mark this popup as having been configured at least once.
    pub(crate) fn mark_popup_configured(&self, id: PopupId) {
        if let Some(entry) = self.inner.popups.borrow().get(&id) {
            entry.configured.set(true);
        }
    }

    /// Drop every popup this runtime knows of, without touching wlroots.
    ///
    /// Called once by `backend.rs`'s `run_inner` when the `run_all` call that
    /// populated the table returns, on every exit path — mirroring
    /// [`clear_toplevels`](Runtime::clear_toplevels) exactly, and for the
    /// identical reason: a popup id is only meaningful for the call that
    /// announced it, because the per-popup destroy listener that would remove a
    /// stale row is itself torn down with that call's `Session`. Without this, a
    /// consumer who kept a `Runtime` clone could resolve a stale id and hand
    /// wlroots memory it had already freed.
    pub(crate) fn clear_popups(&self) {
        self.inner.popups.borrow_mut().clear();
    }

    /// Borrow the popup `id` names, for as long as the borrow lasts.
    ///
    /// `None` once the popup is gone — the by-id miss every id type in this
    /// crate promises.
    pub fn popup(&self, id: PopupId) -> Option<Popup<'_>> {
        let (raw, parent) = {
            let popups = self.inner.popups.borrow();
            let entry = popups.get(&id)?;
            (entry.raw, entry.parent)
        };
        // SAFETY: an entry is removed by `on_popup_destroy`, which wlroots runs
        // before it frees the popup, so a present entry names a live one. The
        // borrow above is released before the handle is built, because the
        // caller will re-enter wlroots, which can emit a signal, which can take
        // the same `RefCell` mutably.
        Some(unsafe { Popup::from_raw_with_id(raw.as_ptr(), id, parent) })
    }

    /// What this popup hangs off, or `None` if it is gone.
    pub fn popup_parent(&self, id: PopupId) -> Option<PopupParent> {
        self.inner.popups.borrow().get(&id).map(|e| e.parent)
    }

    /// The **direct** children of `parent`, in creation order.
    ///
    /// Creation order is the z-order tiebreak among siblings, so this is the
    /// order a compositor's own popup stack should record them in.
    pub fn popups_of(&self, parent: PopupParent) -> Vec<PopupId> {
        let popups = self.inner.popups.borrow();
        let mut out: Vec<PopupId> = popups
            .iter()
            .filter(|(_, entry)| entry.parent == parent)
            .map(|(id, _)| *id)
            .collect();
        // The table is a `HashMap`, so iteration order is arbitrary; ids come
        // from a monotonic counter, so sorting by the id *is* sorting by
        // creation order. This is the one place in the crate that orders ids,
        // and it does so through the raw `u64` rather than an `Ord` impl on
        // `PopupId` precisely so the public type keeps promising nothing about
        // ordering (see `ToplevelId`'s own doc).
        out.sort_unstable_by_key(|id| id.0);
        out
    }

    /// Every popup in the subtree under `parent`, parents before their own
    /// children, deepest last.
    ///
    /// Breadth-first over [`popups_of`](Runtime::popups_of), capped at
    /// [`MAX_POPUP_DEPTH`](Runtime::MAX_POPUP_DEPTH) levels and de-duplicated,
    /// so a corrupted table yields a finite list with no id twice rather than a
    /// hang or a double destroy. Reverse this for the order xdg-shell requires
    /// popups to be destroyed in.
    pub fn popup_chain(&self, parent: PopupParent) -> Vec<PopupId> {
        let mut out: Vec<PopupId> = Vec::new();
        let mut frontier = vec![parent];
        for _ in 0..Self::MAX_POPUP_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for p in frontier.drain(..) {
                for child in self.popups_of(p) {
                    if out.contains(&child) {
                        continue;
                    }
                    out.push(child);
                    next.push(PopupParent::Popup(child));
                }
            }
            frontier = next;
        }
        out
    }
```

3e. Add `PopupParent::root` to `popup.rs`, inside the existing `impl
PopupParent` block:

```rust
    /// The chain root: walks `Popup(_)` links down to the [`Toplevel`] or
    /// [`Layer`] at the bottom.
    ///
    /// `Some(self)` when `self` is already a root. `None` when a link is
    /// already dead — the by-id miss this crate promises everywhere — and also
    /// `None` if the walk exceeds
    /// [`Runtime::MAX_POPUP_DEPTH`](crate::Runtime), which cannot happen with a
    /// table wlroots produced but is what keeps a corrupted one from hanging
    /// the compositor's input path.
    ///
    /// [`Toplevel`]: PopupParent::Toplevel
    /// [`Layer`]: PopupParent::Layer
    #[must_use]
    pub fn root(self, rt: &crate::Runtime) -> Option<PopupParent> {
        let mut cursor = self;
        for _ in 0..crate::Runtime::MAX_POPUP_DEPTH {
            match cursor {
                PopupParent::Popup(id) => cursor = rt.popup_parent(id)?,
                root => return Some(root),
            }
        }
        None
    }
```

`MAX_POPUP_DEPTH` is `pub(crate)`, so `popup.rs` can name it while consumers
cannot; the doc link above resolves to `Runtime` itself, which is what a
consumer reads.

3f. Add the re-exports to `lib.rs`, immediately before
`pub use region::{Region, RegionRef};`:

```rust
pub use popup::{
    ConstraintAdjustment, Popup, PopupId, PopupParent, PositionerAnchor, PositionerGravity,
    PositionerRules,
};
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: all lib tests pass, including the nine new `runtime::tests` cases.

**Mutation checks (perform all three).**

1. In `popups_of`, delete the `out.sort_unstable_by_key(|id| id.0);` line.
   `popups_of_lists_direct_children_in_creation_order` must fail — re-run it a
   few times if the first run happens to hash in order
   (`cargo test -p wlr --lib popups_of_lists -- --exact` in a loop of 20).
   Restore.
2. In `popup_chain`, delete the `if out.contains(&child) { continue; }` guard.
   `a_chain_walk_terminates_and_never_repeats_an_id_on_a_cyclic_table` must fail
   (it will also grow to `MAX_POPUP_DEPTH` entries). Restore.
3. In `PopupParent::root`, replace the `for _ in 0..MAX_POPUP_DEPTH` with
   `loop`. `a_root_walk_terminates_even_on_a_cyclic_table` must **hang** — kill
   it with the test harness timeout or Ctrl-C after ten seconds; a hang is the
   pass condition for this mutation, and it is the failure mode the cap exists
   to prevent. Restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/runtime.rs crates/wlr/src/popup.rs crates/wlr/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(wlr): the runtime popup table and the chain walkers

`PopupEntry` mirrors `LayerSurfaceEntry` — not `Copy` because of its `Cell`, so
every accessor copies out just the field it needs and releases the borrow before
returning. That is not tidiness: the caller re-enters wlroots, which can emit a
signal, which can take the same `RefCell` mutably.

The tree is stored and dropped, never destroyed. A popup's subtree is a child of
its parent's and wlroots frees a tree's children recursively, so destroying one
here is the double free `forget_toplevel` documents.

`popups_of` sorts by the raw id, which is creation order because the counter is
monotonic — done through the `u64` rather than an `Ord` impl so `PopupId` keeps
promising nothing about ordering. `popup_chain` and `PopupParent::root` are
depth-capped and de-duplicated: wlroots cannot make a cycle, but both run on the
compositor's input path, where an unbounded walk over a corrupted table is a
session freeze the user cannot tell from a crash.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5 — `Runtime`: configure, dismiss, and the two grab observables

**Files:** `crates/wlr/src/runtime.rs`.

**Interfaces.**

*Consumes:* Task 3's `Popup::{unconstrain, send_configure, position, destroy,
grab_requested}`, Task 4's table and walkers.

*Produces:*

```rust
impl Runtime {
    pub fn configure_popup(&self, id: PopupId, constraint: &Box2D) -> bool;
    pub fn popup_position(&self, id: PopupId) -> Option<(f64, f64)>;
    pub fn dismiss_popup(&self, id: PopupId) -> usize;
    pub fn dismiss_popups_of(&self, parent: PopupParent) -> usize;
    pub fn popup_is_grabbing(&self, id: PopupId) -> bool;
    pub fn seat_has_explicit_grab(&self) -> bool;
}
```

**Step 1 — write the failing tests.** Append to `runtime.rs`'s `mod tests`
(reusing `record_scratch_popup` from Task 4):

```rust
    /// A popup whose surface is not yet `initialized` cannot be configured:
    /// `Popup::send_configure` skips the call (see its own doc — this
    /// distribution's wlroots asserts on that flag and aborts), so
    /// `configure_popup` must report `false` rather than claiming success. The
    /// unconstrain half still runs, which is harmless and is what leaves
    /// `scheduled.geometry` correct for the configure the initial commit will
    /// trigger moments later.
    #[test]
    fn configuring_an_uninitialized_popup_reports_false() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let id = record_scratch_popup(&rt, window, false);
        assert!(!rt.configure_popup(id, &Box2D::new(0, 0, 800, 600)));
    }

    #[test]
    fn configuring_an_unknown_popup_reports_false_rather_than_dereferencing() {
        let rt = Runtime::new().expect("runtime");
        assert!(!rt.configure_popup(PopupId::dangling_for_test(), &Box2D::new(0, 0, 800, 600)));
        assert_eq!(rt.popup_position(PopupId::dangling_for_test()), None);
        assert!(!rt.popup_is_grabbing(PopupId::dangling_for_test()));
    }

    /// `dismiss_popup` returns how many popups it destroyed, and destroys the
    /// whole subtree under `id` as well as `id` itself. These scratch popups
    /// are not wired to wlroots, so nothing is actually freed and no destroy
    /// signal fires — what is under test here is the *count and the order*, not
    /// the FFI, which the harness-driven `compositor/tests/popups.rs` (P2)
    /// covers end to end.
    #[test]
    fn dismissing_a_popup_counts_itself_and_its_whole_subtree() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let menu = record_scratch_popup(&rt, window, false);
        let submenu = record_scratch_popup(&rt, PopupParent::Popup(menu), false);
        record_scratch_popup(&rt, PopupParent::Popup(submenu), false);
        let sibling = record_scratch_popup(&rt, window, false);

        assert_eq!(rt.dismiss_popup(menu), 3, "the menu and its two descendants");
        assert_eq!(
            rt.popups_of(window),
            vec![sibling],
            "a sibling chain is untouched"
        );
    }

    #[test]
    fn dismissing_an_unknown_popup_destroys_nothing() {
        let rt = Runtime::new().expect("runtime");
        assert_eq!(rt.dismiss_popup(PopupId::dangling_for_test()), 0);
        assert_eq!(
            rt.dismiss_popups_of(PopupParent::Popup(PopupId::dangling_for_test())),
            0
        );
    }

    #[test]
    fn dismissing_a_parents_popups_covers_every_chain_under_it() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        record_scratch_popup(&rt, PopupParent::Popup(a), false);
        record_scratch_popup(&rt, window, false);

        assert_eq!(rt.dismiss_popups_of(window), 3);
        assert!(rt.popups_of(window).is_empty());
    }

    /// The destruction order is deepest-first, which is what xdg-shell requires
    /// (`xdg_popup.destroy` on a popup with live children is a protocol error).
    /// The order is observable here because `dismiss_popup` forgets each row as
    /// it goes: a shallow-first implementation would find the deeper rows
    /// already unreachable and under-count.
    #[test]
    fn dismissal_is_deepest_first() {
        let rt = Runtime::new().expect("runtime");
        let window = PopupParent::Toplevel(ToplevelId::dangling_for_test());
        let a = record_scratch_popup(&rt, window, false);
        let b = record_scratch_popup(&rt, PopupParent::Popup(a), false);
        let c = record_scratch_popup(&rt, PopupParent::Popup(b), false);

        let order = rt.popup_chain(PopupParent::Popup(a));
        assert_eq!(order, vec![b, c], "chain order is shallow-first…");
        assert_eq!(
            rt.dismiss_popup(a),
            3,
            "…and dismissal reverses it, so every row is still present when its \
             own destroy runs"
        );
    }

    /// With no seat created there is no grab to observe, and asking must be a
    /// plain `false` rather than a null dereference — a compositor calls this
    /// from `sync_seat_focus`, which runs before a seat exists during startup.
    #[test]
    fn a_runtime_without_a_seat_has_no_explicit_grab() {
        let rt = Runtime::new().expect("runtime");
        assert!(!rt.seat_has_explicit_grab());
    }
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib runtime::tests 2>&1 | tail -20
```

Expected failure: `no method named `configure_popup` found for struct
`Runtime``, and the same for the other five.

**Step 3 — implement.** Add to `runtime.rs`, immediately after `popup_chain`:

```rust
    /// Place `id` inside `constraint` and answer its configure.
    ///
    /// Two wlroots calls in sequence — `wlr_xdg_popup_unconstrain_from_box`,
    /// which rewrites `scheduled.geometry` from the client's rules, then
    /// `wlr_xdg_surface_schedule_configure`, which is the only way to actually
    /// send it. There is no `wlr_xdg_popup_set_size`/`_configure`; this pair is
    /// the whole placement API.
    ///
    /// **`constraint` is in the root toplevel/layer parent surface's coordinate
    /// system**, not layout or output space. wlroots' own header says so, and
    /// getting it wrong places every popup at an offset rather than failing
    /// loudly. The compositor is what translates its output's usable area into
    /// that space.
    ///
    /// `false` when the popup is gone, or when its surface is not yet
    /// `initialized` — see [`Popup::send_configure`](crate::Popup::send_configure)
    /// for why an early call would abort the process rather than fail. The
    /// unconstrain still runs in the uninitialized case, deliberately: it costs
    /// nothing, touches no wire, and leaves `scheduled.geometry` correct for the
    /// configure the initial commit is about to trigger.
    pub fn configure_popup(&self, id: PopupId, constraint: &Box2D) -> bool {
        let Some(popup) = self.popup(id) else {
            return false;
        };
        popup.unconstrain(constraint);
        if popup.send_configure() == 0 {
            return false;
        }
        drop(popup);
        self.mark_popup_configured(id);
        true
    }

    /// This popup's position in its **parent surface's** coordinates, or `None`
    /// if it is gone.
    pub fn popup_position(&self, id: PopupId) -> Option<(f64, f64)> {
        Some(self.popup(id)?.position())
    }

    /// Destroy `id` and every popup under it, **deepest first**, and report how
    /// many were destroyed.
    ///
    /// Deepest-first is not a preference: `xdg_popup.destroy` on a popup that
    /// still has live children is a protocol error, and wlroots enforces it.
    ///
    /// Each destroy sends `xdg_popup.popup_done` and makes the resource inert;
    /// wlroots emits `events.destroy` from inside the call, which runs
    /// `on_popup_destroy`, which is what actually removes the row. Nothing here
    /// touches a scene tree.
    pub fn dismiss_popup(&self, id: PopupId) -> usize {
        let mut order = self.popup_chain(PopupParent::Popup(id));
        order.push(id);
        let mut destroyed = 0;
        // Reversed: `popup_chain` is shallow-first, and destroying a parent
        // before its children is the protocol error above.
        for victim in order.into_iter().rev() {
            let Some(popup) = self.popup(victim) else {
                // Already gone — wlroots destroys a popup's children with it,
                // so a deeper row may have been swept by an earlier iteration's
                // own destroy emission. A miss here is expected, not an error.
                continue;
            };
            popup.destroy();
            destroyed += 1;
        }
        destroyed
    }

    /// Destroy every popup hanging off `parent`, chains and all, deepest first.
    /// Returns how many were destroyed.
    ///
    /// This is what a compositor calls when the window or layer surface a menu
    /// belongs to goes away, and what P2's "a click outside dismisses the whole
    /// chain" path falls back to for **non-grabbing** popups (a grabbing chain
    /// is wlroots' own to dismiss — see [`Popup::grab_requested`]).
    pub fn dismiss_popups_of(&self, parent: PopupParent) -> usize {
        let mut destroyed = 0;
        for child in self.popups_of(parent) {
            destroyed += self.dismiss_popup(child);
        }
        destroyed
    }

    /// `(*popup).seat != NULL` — whether this popup's client sent
    /// `xdg_popup.grab`.
    ///
    /// `false` for an unknown id. See [`Popup::grab_requested`], and this
    /// crate's `popup` module doc, for what a `true` means the compositor must
    /// **not** do.
    pub fn popup_is_grabbing(&self, id: PopupId) -> bool {
        self.popup(id).is_some_and(|p| p.grab_requested())
    }

    /// Whether *some* explicit seat grab is in force right now —
    /// `wlr_seat_pointer_has_grab(seat) || wlr_seat_keyboard_has_grab(seat)`.
    ///
    /// An xdg-popup grab is one; a drag-and-drop grab is another. This is the
    /// single fact a compositor's focus synchronisation needs: while it is
    /// `true`, wlroots is routing pointer, keyboard and touch itself, will
    /// dismiss the popup chain on a press outside it, and will restore the
    /// pre-grab keyboard focus when the grab ends — so a compositor that also
    /// moves focus is fighting it. P2's `sync_seat_focus` returns early on this.
    ///
    /// `false` with no seat: this is called from focus paths that run before
    /// [`create_seat`](Runtime::create_seat) has, and answering "no grab" there
    /// is both true and safe.
    pub fn seat_has_explicit_grab(&self) -> bool {
        let seat = *self.inner.seat.borrow();
        let Some(seat) = seat else {
            return false;
        };
        // SAFETY: `seat` is this runtime's own `wlr_seat`, created by
        // `create_seat` and live for as long as the runtime; both predicates
        // only read `seat->{pointer,keyboard}_state.grab`.
        unsafe {
            sys::wlr_seat_pointer_has_grab(seat.as_ptr())
                || sys::wlr_seat_keyboard_has_grab(seat.as_ptr())
        }
    }
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: all lib tests pass, seven new cases among them.

**Mutation checks (perform both).**

1. In `dismiss_popup`, drop the `.rev()`. `dismissal_is_deepest_first` must
   fail: destroying `a` first leaves `b` and `c` unreachable through
   `self.popup(...)` only if wlroots swept them — which it does not for these
   scratch rows, so instead the *order* assertion is what catches it. If it does
   not fail, the implementation is not forgetting rows as it goes and the test
   is not load-bearing — **stop and report** rather than proceeding.
2. In `seat_has_explicit_grab`, change `||` to `&&`. No existing test fails
   (there is no seat in the unit tests) — that is expected and is why P2's
   `a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it` and
   `keyboard_focus_returns_to_the_parent_when_a_grabbing_chain_ends` are the
   real coverage for this method. Record that in the commit body, restore the
   `||`, and do not add a fake seat here.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/runtime.rs
git commit -m "$(cat <<'EOF'
feat(wlr): configure, dismiss and observe popups from the Runtime

`configure_popup` is the only placement API xdg-shell offers: unconstrain from a
box, then schedule a configure. There is no `wlr_xdg_popup_set_size`. The box is
in the ROOT TOPLEVEL PARENT surface's coordinates, which is stated on the method
because getting it wrong offsets every popup instead of failing loudly.

`dismiss_popup` destroys deepest-first — destroying a popup with live children is
a protocol error — and tolerates a row wlroots already swept during an earlier
destroy emission.

`seat_has_explicit_grab` is the one fact a compositor needs about popup grabs:
while it is true, wlroots routes input, dismisses the chain on an outside press
and restores the pre-grab focus itself, so a compositor that also moves focus is
fighting it. It has no unit coverage here (there is no seat in these tests); the
real coverage is P2's grab tests against the harness, and mutating the `||` to
`&&` correctly fails nothing in this crate.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6 — `ToplevelHandler`: the six defaulted popup methods

**Files:** `crates/wlr/src/handler.rs`.

**Interfaces.**

*Consumes:* `crate::{Popup, PopupId}`.

*Produces:* six methods on the **existing** `ToplevelHandler` trait
(`handler.rs:378`) — **not** a new trait, and **not** a supertrait on the sealed
`Handlers` (`handler.rs:961`), which would be a breaking change:

```rust
fn new_popup(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_initial_commit(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_mapped(&mut self, id: PopupId) { let _ = id; }
fn popup_unmapped(&mut self, id: PopupId) { let _ = id; }
fn popup_reposition(&mut self, popup: &Popup<'_>) { let _ = popup; }
fn popup_destroyed(&mut self, id: PopupId) { let _ = id; }
```

**Step 1 — write the failing test.** Create
`crates/wlr/tests/popups.rs` with just the additivity case (the rest of the file
arrives in Task 10):

```rust
//! xdg-popup, against a real headless compositor with no client.
//!
//! Same shape as `layers.rs`: what is provable without a client library is that
//! the additive handler methods really are additive, that every by-id operation
//! misses on an id no popup was ever given, and that the public types behave.
//! Anything that needs a live client — placement, flipping, chains, grab
//! dismissal, focus restore — is P2's `compositor/tests/popups.rs` against the
//! harness, which drives a real `xdg_popup` end to end.

/// A `ToplevelHandler` written against 0.20.27, with an empty body, must still
/// compile and still be usable in 0.20.28. That is the whole additivity claim
/// of this release, and it is a compile-time claim, so the test that asserts it
/// is a type that exists.
struct LegacyHandler;

impl wlr::ToplevelHandler for LegacyHandler {}

/// A handler that overrides every new method, proving the signatures are what
/// the contract froze and that a `Popup<'_>` is usable from inside one.
#[derive(Default)]
struct PopupHandler {
    seen: Vec<String>,
}

impl wlr::ToplevelHandler for PopupHandler {
    fn new_popup(&mut self, popup: &wlr::Popup<'_>) {
        self.seen.push(format!("new {:?}", popup.id()));
    }
    fn popup_initial_commit(&mut self, popup: &wlr::Popup<'_>) {
        self.seen.push(format!("commit {:?}", popup.parent()));
    }
    fn popup_mapped(&mut self, id: wlr::PopupId) {
        self.seen.push(format!("mapped {id:?}"));
    }
    fn popup_unmapped(&mut self, id: wlr::PopupId) {
        self.seen.push(format!("unmapped {id:?}"));
    }
    fn popup_reposition(&mut self, popup: &wlr::Popup<'_>) {
        self.seen.push(format!("reposition {:?}", popup.reposition_token()));
    }
    fn popup_destroyed(&mut self, id: wlr::PopupId) {
        self.seen.push(format!("destroyed {id:?}"));
    }
}

#[test]
fn the_popup_handler_methods_are_additive_and_overridable() {
    let _legacy = LegacyHandler;
    let handler = PopupHandler::default();
    assert!(handler.seen.is_empty());
}
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --test popups 2>&1 | tail -20
```

Expected failure: `error[E0407]: method `new_popup` is not a member of trait
`ToplevelHandler``, five more like it.

**Step 3 — implement.** Add to `handler.rs`, at the end of the
`ToplevelHandler` trait body (after the layer-shell methods, keeping the
protocol grouping the trait already has), and extend the trait's `use` line with
`Popup`/`PopupId`:

```rust
    /// A client created an `xdg_popup` — a menu, tooltip, dropdown or popover —
    /// on one of your toplevels, on one of your layer surfaces, or on another
    /// popup.
    ///
    /// [`Popup::parent`](crate::Popup::parent) tells you which, and it is the
    /// **only** way to find out: a layer-shell popup is created with a NULL xdg
    /// parent and reparented by `zwlr_layer_surface_v1.get_popup` afterwards, so
    /// the popup's own `parent` field is null at exactly the moment the question
    /// is asked. This crate records the answer at announcement time.
    ///
    /// The popup is **not** mapped yet and has no buffer. The crate has already
    /// inserted it into the scene graph, as a subtree of its parent's, so it
    /// stacks with its parent and hit-tests correctly without anything further.
    ///
    /// Place it here — compute a constraint box in the **root toplevel parent
    /// surface's** coordinate system and call
    /// [`Runtime::configure_popup`](crate::Runtime::configure_popup). If you do
    /// nothing, the dispatch layer still answers the popup's initial commit
    /// (xdg-shell requires an answer or the popup never maps), and the client
    /// gets the geometry its own positioner asked for, unconstrained.
    ///
    /// Record [`Popup::id`](crate::Popup::id); the handle is valid only for
    /// this call.
    fn new_popup(&mut self, popup: &Popup<'_>) {
        let _ = popup;
    }

    /// The popup committed for the first time, which is where xdg-shell
    /// requires the compositor to answer with a configure.
    ///
    /// The crate schedules that configure unconditionally right after this
    /// method returns, so a handler that does nothing still produces a popup
    /// that maps. Re-place it here if the placement depends on something only
    /// the first commit revealed.
    fn popup_initial_commit(&mut self, popup: &Popup<'_>) {
        let _ = popup;
    }

    /// The popup has a buffer and should be displayed.
    fn popup_mapped(&mut self, id: PopupId) {
        let _ = id;
    }

    /// The popup should not be displayed any more — a null buffer, or the role
    /// object going away.
    ///
    /// **Not** the same as destruction: a popup can be unmapped and mapped
    /// again, keeping its id.
    fn popup_unmapped(&mut self, id: PopupId) {
        let _ = id;
    }

    /// The client sent `xdg_popup.reposition` with a new positioner.
    ///
    /// wlroots has already written the new rules into the popup's scheduled
    /// state; re-run your constraint computation and call
    /// [`Runtime::configure_popup`](crate::Runtime::configure_popup) again.
    /// wlroots sends the `xdg_popup.repositioned` event itself off its own token
    /// field — [`Popup::reposition_token`](crate::Popup::reposition_token) lets
    /// you observe it, but **never forge one**.
    fn popup_reposition(&mut self, popup: &Popup<'_>) {
        let _ = popup;
    }

    /// The popup is gone. Only the id is passed, because there is no longer an
    /// object to borrow.
    ///
    /// **`id` may be one you were never told about**, for the same reason
    /// [`OutputHandler::destroyed`] documents: an announcement that arrived
    /// while another handler was running is queued, and a popup created and
    /// destroyed inside that window produces a destroy with no preceding
    /// `new_popup`. Write this so an unknown id is harmless — `remove` on a map,
    /// never indexing.
    ///
    /// wlroots destroys a popup's own children before the popup itself and
    /// emits one of these for each, so a chain arrives here deepest-first.
    fn popup_destroyed(&mut self, id: PopupId) {
        let _ = id;
    }
```

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --test popups 2>&1 | tail -10
cargo test -p wlr 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: `test the_popup_handler_methods_are_additive_and_overridable ... ok`,
and every pre-existing suite still green — in particular
`crates/wlr/tests/compile_fail.rs`, which would notice if `Handlers` had gained
a supertrait.

**Mutation check.** Remove the `{ let _ = popup; }` default body from
`new_popup`, making it a required method. `cargo test -p wlr --test popups`
must fail to compile with `error[E0046]: not all trait items implemented` on
`impl wlr::ToplevelHandler for LegacyHandler {}`. That failure *is* the
additivity claim. Restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/handler.rs crates/wlr/tests/popups.rs
git commit -m "$(cat <<'EOF'
feat(wlr): six defaulted popup methods on ToplevelHandler

Added to the existing trait rather than to a new one, and emphatically not as a
supertrait on the sealed `Handlers`: every method is defaulted, so
`impl ToplevelHandler for S {}` written against 0.20.27 still compiles. A
`LegacyHandler` with an empty impl block in `tests/popups.rs` is what asserts
that, and removing any default body correctly turns it into a compile error.

`popup_destroyed` carries the "`id` may be one you were never told about" caveat
word for word from `toplevel_destroyed` and `layer_surface_destroyed`, and adds
the popup-specific half: wlroots destroys a chain deepest-first and emits one
destroy per level.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7 — `dispatch::Event`: six variants, two match sites

**Files:** `crates/wlr/src/dispatch.rs`, `crates/wlr/src/backend.rs`.

**Interfaces.**

*Consumes:* Task 4's `Runtime::popup`, Task 6's six handler methods.

*Produces:*

```rust
// dispatch.rs — inside the existing pub(crate) enum Event
NewPopup(PopupId),
PopupInitialCommit(PopupId),
PopupMapped(PopupId),
PopupUnmapped(PopupId),
PopupReposition(PopupId),
PopupDestroyed(PopupId),

// backend.rs
fn with_popup<S>(session: &Session<'_, S>, id: PopupId, f: impl FnOnce(&Popup<'_>));
```

Both match sites are **compile-required**, not optional: `deliver_all`
(`backend.rs:2260`) must route the six, and `deliver` (`backend.rs:7259`) has a
`|`-chained unreachable arm listing every event the plain `run` path cannot
produce — omitting the new names there makes the match non-exhaustive and the
crate stops compiling.

**Step 1 — write the failing test.** Append to `crates/wlr/tests/popups.rs`:

```rust
/// Ensures `WLR_BACKENDS`/`WLR_HEADLESS_OUTPUTS` are set exactly once, before
/// any test in this binary calls `Backend::autocreate`. See `toplevels.rs`'s
/// identical copy for the full argument — this is a separate integration-test
/// binary with its own environment and its own possible parallel `#[test]`
/// threads, so it needs its own `Once`.
fn headless_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: `Once::call_once` runs this closure at most once and blocks
        // every other caller on this `Once` until it returns, so no concurrent
        // `getenv` can observe a torn write.
        unsafe {
            std::env::set_var("WLR_BACKENDS", "headless");
            std::env::set_var("WLR_HEADLESS_OUTPUTS", "1");
        }
    });
}

/// A `run_all` over a headless backend with a popup-aware handler installed
/// must start, dispatch and stop cleanly. No client connects, so no popup is
/// ever announced — what this proves is that the six new events are wired
/// through `deliver_all` and that installing the handler changes nothing about
/// the ordinary lifecycle. The *delivery* of a real popup event is P2's
/// harness-driven coverage.
#[test]
fn a_run_with_a_popup_handler_starts_and_stops_cleanly() {
    headless_env();

    struct App {
        ticks: u32,
    }

    impl wlr::OutputHandler for App {
        fn frame(&mut self, output: &wlr::Output<'_>) {
            self.ticks += 1;
            let _ = output.commit();
        }
    }
    impl wlr::ToplevelHandler for App {
        fn new_popup(&mut self, popup: &wlr::Popup<'_>) {
            // Never reached without a client; here so the method is live code
            // rather than a default, which is what makes `deliver_all`'s arm
            // reachable at all.
            let _ = popup.id();
        }
    }
    impl wlr::SeatHandler for App {}
    impl wlr::LoopHandler for App {}
    impl wlr::FdHandler for App {}

    let mut app = App { ticks: 0 };
    let backend = wlr::Backend::autocreate().expect("headless backend");
    backend
        .run_all(&mut app, wlr::Until::Frames(2))
        .expect("run_all");
    assert!(app.ticks >= 1, "the headless output produced no frame");
}
```

> If this crate's `Until`/`run_all`/handler-set spelling differs from the above,
> copy the exact shape from `crates/wlr/tests/toplevels.rs`'s own `run_all`
> test and keep the assertions.

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --test popups 2>&1 | tail -20
```

Expected failure: the test compiles and passes *already* — the handler default
makes it green before the events exist. **That is the wrong failure**, so make
the real one first: add the six variants to `Event` (step 3a only) and re-run
`cargo build -p wlr`, which must fail with
`error[E0004]: non-exhaustive patterns: `Event::NewPopup(_)` … not covered`
pointing at both `deliver_all` and `deliver`. That compile failure is this
task's red state.

**Step 3 — implement.**

3a. In `dispatch.rs`, extend the `use crate::{…}` list with `PopupId`, and add
the six variants immediately after the `LayerSurfaceDestroyed` line:

```rust
    /// A client created an `xdg_popup`. The parent is not carried: it is
    /// recorded in the runtime's own table at announcement time and read back
    /// through [`Runtime::popup`](crate::Runtime::popup), for the reason this
    /// enum carries ids and never handles — a deferred event may name an object
    /// that no longer exists by the time it is delivered.
    NewPopup(PopupId),
    /// The popup's first commit, where xdg-shell requires an answer.
    PopupInitialCommit(PopupId),
    PopupMapped(PopupId),
    PopupUnmapped(PopupId),
    /// The client sent `xdg_popup.reposition`.
    PopupReposition(PopupId),
    PopupDestroyed(PopupId),
```

3b. In `backend.rs`, add `with_popup` immediately after `with_layer_surface`:

```rust
/// Borrow the popup `id` names, if this runtime still knows of one.
/// Mirrors [`with_toplevel`] exactly, including its obligations on `f`: the
/// table borrow is released before `f` runs (a handler can re-enter wlroots,
/// which can emit a signal, which can take the borrow mutably), and `f` must not
/// reach anything that frees the popup mid-call.
///
/// [`Runtime::popup`] is what releases the borrow — it copies `raw` and `parent`
/// out and drops the guard before building the handle, so this function is a
/// thin wrapper rather than a second implementation of that discipline.
fn with_popup<S>(session: &Session<'_, S>, id: PopupId, f: impl FnOnce(&Popup<'_>)) {
    let Some(popup) = session.runtime.popup(id) else {
        return;
    };
    f(&popup);
}
```

3c. In `deliver_all`, add six arms immediately after
`Event::LayerSurfaceDestroyed(id) => state.layer_surface_destroyed(id),`:

```rust
        Event::NewPopup(id) => with_popup(session, id, |p| state.new_popup(p)),
        Event::PopupInitialCommit(id) => {
            with_popup(session, id, |p| state.popup_initial_commit(p));
            // xdg-shell requires an answer to a popup's first commit or it
            // never maps, and the trait default has no `Runtime` to send one
            // with — so this is the dispatch layer discharging that guarantee,
            // exactly as `RequestMaximize`'s unconditional `configure_toplevel`
            // does for its own. Unconditional, not "only if the handler staged
            // nothing": wlroots coalesces a second scheduled configure into
            // whatever `Runtime::configure_popup` already sent, so scheduling
            // again is harmless, and harmless-every-time is simpler and no less
            // correct than probing.
            //
            // `send_configure` is what skips the call if the surface somehow is
            // not `initialized` — see its own doc for why that guard is a
            // process-abort question and not a tidiness one.
            if let Some(popup) = session.runtime.popup(id) {
                popup.send_configure();
            }
        }
        Event::PopupMapped(id) => state.popup_mapped(id),
        Event::PopupUnmapped(id) => state.popup_unmapped(id),
        Event::PopupReposition(id) => with_popup(session, id, |p| state.popup_reposition(p)),
        Event::PopupDestroyed(id) => state.popup_destroyed(id),
```

3d. In `deliver`, add the six names to the `|`-chained unreachable arm, right
after `| Event::LayerSurfaceDestroyed(..)`:

```rust
        // Unreachable for the same reason the layer-surface events above are:
        // `run` never registers an xdg shell, so no popup can be announced on
        // this path. Dropped rather than `unreachable!()` because this is on
        // the path from an `extern "C"` frame, where a panic aborts.
        | Event::NewPopup(..)
        | Event::PopupInitialCommit(..)
        | Event::PopupMapped(..)
        | Event::PopupUnmapped(..)
        | Event::PopupReposition(..)
        | Event::PopupDestroyed(..)
```

3e. Extend `backend.rs`'s `use crate::{…}` list with `Popup` and `PopupId`.

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo build -p wlr 2>&1 | tail -5
cargo test -p wlr 2>&1 | tail -20
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: the build succeeds, every suite green.

**Mutation check.** Delete the `| Event::NewPopup(..)` line from `deliver`'s
unreachable arm. `cargo build -p wlr` must fail with
`error[E0004]: non-exhaustive patterns`. Restore. Then delete the
`Event::PopupInitialCommit` arm's `popup.send_configure();` block; nothing in
this crate fails (no client), so record in the commit body that
`a_popup_under_a_toplevel_is_configured_at_the_positioner_geometry` in P2 is the
test that covers it, and restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/dispatch.rs crates/wlr/src/backend.rs crates/wlr/tests/popups.rs
git commit -m "$(cat <<'EOF'
feat(wlr): six popup events, routed through deliver_all

The events carry ids and never handles, like every other variant: a deferred
event may name an object that no longer exists by the time it is delivered, and
`with_popup` resolves through the runtime so a dead id simply misses.

`PopupInitialCommit`'s arm schedules a configure unconditionally after the
handler runs. xdg-shell requires an answer to a popup's first commit or the
popup never maps, and a defaulted handler method has no `Runtime` to send one
with — the same guarantee `RequestMaximize` discharges in the dispatch layer for
the same reason. wlroots coalesces the second schedule into whatever the handler
already sent.

The `deliver` unreachable arm is compile-required, not documentation: `run`
registers no xdg shell, and leaving a name out of that arm makes the match
non-exhaustive.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8 — `backend.rs`: listeners, ids, the scene subtree, and the six callbacks

**Files:** `crates/wlr/src/backend.rs`.

This is the largest task in the part and it is one commit, because the pieces do
not compile apart: `on_new_popup` links the five per-popup callbacks, so they
must exist; the callbacks read `Bound::popup`, so the slot must exist; and
`link_popup` is dead code — a hard error under `-D warnings` — until
`on_new_popup` calls it.

**Interfaces.**

*Consumes:* Task 4's `record_popup`/`popup_raw`, Task 7's six `Event` variants,
Task 3's `Popup`.

*Produces (all private to `backend.rs`):*

```rust
struct Bound { /* … */ popup: Option<PopupId> }

impl Registration {
    unsafe fn link_popup(signal: *mut sys::wl_signal, notify: sys::wl_notify_func_t,
                         session: *const (), popup: PopupId) -> Self;
}

struct PopupListeners {
    _commit: Registration, _map: Registration, _unmap: Registration,
    _destroy: Registration, _reposition: Registration, _new_popup: Registration,
}

struct Session<'r, S> { /* … */ popups: RefCell<HashMap<PopupId, PopupListeners>> }

unsafe extern "C" fn on_new_popup<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
unsafe extern "C" fn on_popup_commit<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
unsafe extern "C" fn on_popup_map<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
unsafe extern "C" fn on_popup_unmap<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
unsafe extern "C" fn on_popup_reposition<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
unsafe extern "C" fn on_popup_destroy<S: Handlers>(l: *mut sys::wl_listener, data: *mut c_void);
```

**The parent-resolution rule, stated once.** `on_new_popup` is linked into three
different signals and recovers the parent from **whichever id slot its own
`Bound` carries**:

| Linked into | `Bound` slot set | Parent recorded |
|---|---|---|
| `(*toplevel).base->events.new_popup` (in `on_new_toplevel`) | `toplevel` | `PopupParent::Toplevel(id)` |
| `(*layer_surface).events.new_popup` (in `on_new_layer_surface`) | `layer` | `PopupParent::Layer(id)` |
| `(*popup).base->events.new_popup` (in `on_new_popup` itself) | `popup` | `PopupParent::Popup(id)` |

`wlr_xdg_shell.events.new_popup` is deliberately **not** used: at shell level a
layer-shell popup still has `parent == NULL` and cannot be classified at all.

**Step 1 — write the failing test.** Append to `backend.rs`, as a **new**
`#[cfg(test)] mod popup_bound_tests` placed **after** `mod implicit_grab_tests`
so that module stays byte-identical and its line numbers move as a block:

```rust
#[cfg(test)]
mod popup_bound_tests {
    use super::*;

    /// The id slot a `new_popup` listener carries is what decides the parent
    /// kind, because the signal's own `data` cannot: a layer-shell popup's
    /// `popup->parent` is NULL at announcement time. This pins the mapping
    /// `on_new_popup` performs, without needing a live signal to fire.
    fn parent_from_slots(
        toplevel: Option<ToplevelId>,
        layer: Option<LayerSurfaceId>,
        popup: Option<PopupId>,
    ) -> Option<PopupParent> {
        popup_parent_from_slots(toplevel, layer, popup)
    }

    #[test]
    fn a_toplevel_slot_names_a_toplevel_parent() {
        let t = ToplevelId::dangling_for_test();
        assert_eq!(
            parent_from_slots(Some(t), None, None),
            Some(PopupParent::Toplevel(t))
        );
    }

    #[test]
    fn a_layer_slot_names_a_layer_parent() {
        let l = LayerSurfaceId::dangling_for_test();
        assert_eq!(
            parent_from_slots(None, Some(l), None),
            Some(PopupParent::Layer(l))
        );
    }

    #[test]
    fn a_popup_slot_names_a_nested_parent() {
        let p = PopupId::dangling_for_test();
        assert_eq!(
            parent_from_slots(None, None, Some(p)),
            Some(PopupParent::Popup(p))
        );
    }

    /// A `Bound` with no id slot set cannot have come from any of the three
    /// sites `on_new_popup` is linked at. Returning `None` (and so dropping the
    /// announcement) rather than guessing is the only safe answer: guessing
    /// would attach a popup to the wrong window, and `unreachable!()` is
    /// forbidden here because this runs under an `extern "C"` frame where a
    /// panic aborts the process.
    #[test]
    fn no_slot_at_all_resolves_to_nothing_rather_than_guessing() {
        assert_eq!(parent_from_slots(None, None, None), None);
    }

    /// Precedence is popup, then layer, then toplevel. No real `Bound` ever has
    /// two set — each of the three link sites fills exactly one — but the
    /// function is total, and a deterministic answer beats an arbitrary one if
    /// the invariant is ever broken by a future link site.
    #[test]
    fn the_deepest_slot_wins_if_two_are_somehow_set() {
        let t = ToplevelId::dangling_for_test();
        let l = LayerSurfaceId::dangling_for_test();
        let p = PopupId::dangling_for_test();
        assert_eq!(
            parent_from_slots(Some(t), Some(l), Some(p)),
            Some(PopupParent::Popup(p))
        );
        assert_eq!(
            parent_from_slots(Some(t), Some(l), None),
            Some(PopupParent::Layer(l))
        );
    }
}
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup_bound_tests 2>&1 | tail -20
```

Expected failure: `error[E0425]: cannot find function `popup_parent_from_slots`
in this scope`.

**Step 3 — implement, in seven pieces.**

3a. **`Bound` gains a slot.** Add after the `node` field, before the
cfg-gated `xwayland` one:

```rust
    /// The popup this listener belongs to, for the six per-popup listeners
    /// `on_new_popup` links (commit/map/unmap on the base `wlr_surface`, the
    /// popup's own destroy/reposition, and the nested `new_popup`); `None` for
    /// every other listener in this file.
    ///
    /// A sixth id field alongside `id`/`toplevel`/`layer`/`node` rather than a
    /// shared enum, for the identical reason `toplevel`'s own doc gives:
    /// `Bound` is private to this module, so widening it costs nothing outside
    /// it, and every other call site wants its own exact type.
    ///
    /// Load-bearing in the same way `toplevel` is, and one way further.
    /// wlroots emits `wlr_surface.events.map`/`.unmap` with a **null** `data`,
    /// so map/unmap could not read an id out of the signal even in principle;
    /// and whether `wlr_xdg_popup.events.{destroy,reposition}` carry a non-null
    /// `data` was not verifiable from the shipped artefacts (the emitting
    /// functions are `static`, so they are not in the export table, and the C
    /// sources are not installed). Carrying the id here makes that question
    /// stop mattering, which is the crate's standing rule anyway.
    popup: Option<PopupId>,
```

Add `popup: None,` to **every** existing `Bound` construction — in
`Registration::link` and in `Registration::link_xwayland`. The compiler names
each missing one, so let it.

3b. **`link_popup`.** Add immediately after `link_node`:

```rust
    /// Link a per-popup listener, carrying the [`PopupId`] its callback reads
    /// back from [`Bound::popup`]. Every other id slot is `None`.
    ///
    /// A dedicated constructor rather than a parameter on
    /// [`Registration::link`], following `link_xwayland`'s precedent and for
    /// the reason that function's own doc gives: `link`'s slot list "is frozen
    /// at the five ids the non-Xwayland call sites use", and threading a sixth
    /// `Option` through it would put an extra `None` on every one of its call
    /// sites for a value all but these leave empty. This builds the boxed
    /// [`Bound`] directly, exactly as `link` does.
    ///
    /// `alive` is null, which is the **stronger** claim (see
    /// [`Registration::drop`]): every one of these is dropped from inside the
    /// popup's own destroy emission, while the popup is still alive, or while
    /// the run — and so the whole session table — still stands.
    ///
    /// # Safety
    ///
    /// As for [`Registration::link`]: `signal` must point at an initialised
    /// `wl_signal` whose owner outlives the returned `Registration`, and
    /// `session` must be a `*const Session<S>` for the `S` `notify` casts it
    /// back to, valid for as long as the registration lives.
    unsafe fn link_popup(
        signal: *mut sys::wl_signal,
        notify: sys::wl_notify_func_t,
        session: *const (),
        popup: PopupId,
    ) -> Self {
        let mut bound = Box::new(Bound {
            listener: sys::wl_listener {
                link: sys::wl_list {
                    prev: std::ptr::null_mut(),
                    next: std::ptr::null_mut(),
                },
                notify,
            },
            session,
            alive: std::ptr::null(),
            flag: std::ptr::null(),
            id: None,
            toplevel: None,
            layer: None,
            node: None,
            popup: Some(popup),
            #[cfg(wlr_has_xwayland)]
            xwayland: None,
        });

        // SAFETY: as for `link` — `signal` is an initialised `wl_signal` per the
        // caller's contract, and the listener is a freshly boxed one whose
        // address stays put until this `Registration` drops.
        unsafe { sys::wl_signal_add(signal, &raw mut bound.listener) };

        Registration { bound }
    }
```

3c. **`PopupListeners` and the session table.** Add the struct beside
`LayerSurfaceListeners`:

```rust
/// One live popup's listeners: the base surface's commit/map/unmap, the popup's
/// own destroy and reposition, and the `new_popup` that catches a nested child.
/// Field order is not load-bearing, as for [`ToplevelListeners`], but all six
/// must drop — and so unlink — as part of removing the entry, which happens from
/// inside the popup's own destroy emission, while it is still alive.
struct PopupListeners {
    _commit: Registration,
    _map: Registration,
    _unmap: Registration,
    _destroy: Registration,
    _reposition: Registration,
    _new_popup: Registration,
}
```

and the field on `Session`, after `layers`:

```rust
    /// This run's listeners on every live popup. Removed, and so unlinked, from
    /// `on_popup_destroy` — before the popup is freed, mirroring `toplevels`
    /// and `layers` above.
    popups: RefCell<HashMap<PopupId, PopupListeners>>,
```

Initialise it wherever `layers: RefCell::new(HashMap::new()),` is initialised,
with the same expression.

3d. **The slot-to-parent helper**, at module scope near `toplevel_id_of_surface`:

```rust
/// Which parent a `new_popup` listener's `Bound` names.
///
/// `on_new_popup` is linked into three different signals — a toplevel's base, a
/// layer surface's own, and a popup's base — and each site fills exactly one id
/// slot. The slot is the *only* source of this answer: the signal's `data` is
/// the new popup, and `(*popup).parent` is NULL for a layer-shell popup, which
/// is precisely the case that has to be distinguished.
///
/// `None` when no slot is set, which cannot happen from the three sites above.
/// The announcement is dropped in that case rather than guessed at: attaching a
/// popup to the wrong window is worse than not attaching it, and `unreachable!()`
/// is not available on a path reached from an `extern "C"` frame, where a panic
/// aborts the process.
fn popup_parent_from_slots(
    toplevel: Option<ToplevelId>,
    layer: Option<LayerSurfaceId>,
    popup: Option<PopupId>,
) -> Option<PopupParent> {
    if let Some(p) = popup {
        return Some(PopupParent::Popup(p));
    }
    if let Some(l) = layer {
        return Some(PopupParent::Layer(l));
    }
    toplevel.map(PopupParent::Toplevel)
}
```

3e. **`on_new_popup`.** Add after `on_layer_surface_destroy`:

```rust
/// A client created an `xdg_popup` on a toplevel, on a layer surface, or on
/// another popup. Give it an id and a scene subtree before anyone is told about
/// it — mirrors `on_new_toplevel` exactly, with the parent's own tree in place
/// of the toplevel band.
///
/// Linked into three signals, never into `wlr_xdg_shell.events.new_popup`: at
/// shell level a layer-shell popup still has `parent == NULL` (the client sets
/// it with `zwlr_layer_surface_v1.get_popup` after `xdg_surface.get_popup`), so
/// the parent is knowable only from the parent-scoped signal. Which of the three
/// this emission came from is read out of the `Bound`'s id slots — see
/// [`popup_parent_from_slots`].
unsafe extern "C" fn on_new_popup<S: Handlers>(
    l: *mut sys::wl_listener,
    data: *mut std::ffi::c_void,
) {
    // SAFETY: wlroots invokes this only for listeners this file linked into a
    // parent's `events.new_popup`, whose `session` is the
    // `*const Session<'_, S>` paired with this instantiation. The signal carries
    // a `*mut wlr_xdg_popup`, live and fully initialised — its `base` and
    // `base->surface` included — at the point wlroots announces it.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let popup = data.cast::<sys::wlr_xdg_popup>();
        if popup.is_null() {
            return;
        }
        let base = (*popup).base;
        if base.is_null() {
            return;
        }
        let surface = (*base).surface;
        if surface.is_null() {
            return;
        }

        // Which parent this listener belongs to. Never `(*popup).parent`: it is
        // NULL for a layer-shell popup at exactly this moment.
        let Some(parent) = popup_parent_from_slots(
            (*bound).toplevel,
            (*bound).layer,
            (*bound).popup,
        ) else {
            return;
        };

        // The id lives on the surface's addon set, which is the only one that
        // exists here and the one that dies with the popup — the identical
        // choice `on_new_toplevel` and `on_new_layer_surface` make, and for the
        // identical reason: the role object carries no addon set of its own.
        let id = PopupId(ensure_id_raw(&raw mut (*surface).addons));

        // The parent's scene tree is this popup's parent tree, which is what
        // makes a popup stack with its parent for free and what makes
        // `Runtime::leaf_surface_at` resolve clicks on it — including the
        // session-lock isolation gate — without a line of new code here. No
        // parent tree means the parent is gone or graphics were never
        // initialised; drop the announcement rather than dereference a null,
        // the same choice `on_new_toplevel` makes for the same situation.
        let parent_tree = match parent {
            PopupParent::Toplevel(t) => (*session).runtime.toplevel_entry(t).map(|e| e.tree),
            PopupParent::Layer(ls) => (*session).runtime.layer_surface_scene_ptr(ls),
            PopupParent::Popup(p) => (*session).runtime.popup_tree(p),
        };
        let Some(parent_tree) = parent_tree else {
            return;
        };

        let tree = sys::wlr_scene_xdg_surface_create(parent_tree.as_ptr(), base);
        let Some(tree) = NonNull::new(tree) else {
            return;
        };
        let Some(raw) = NonNull::new(popup) else {
            return;
        };

        // Six listeners, all with a null liveness flag: each is dropped from
        // inside the popup's own destroy emission, while the object is still
        // alive, which is a stronger guarantee than any flag (see
        // `Registration::drop`).
        //
        // Every one of them carries `id` in its own `Bound::popup` rather than
        // recovering it from `data` at callback time — see `Bound::popup`'s own
        // doc for the argument, which is the one `Bound::toplevel` already
        // makes plus one unverifiable case more.
        let commit = Registration::link_popup(
            &raw mut (*surface).events.commit,
            on_popup_commit::<S>,
            (*bound).session,
            id,
        );
        let map = Registration::link_popup(
            &raw mut (*surface).events.map,
            on_popup_map::<S>,
            (*bound).session,
            id,
        );
        let unmap = Registration::link_popup(
            &raw mut (*surface).events.unmap,
            on_popup_unmap::<S>,
            (*bound).session,
            id,
        );
        let destroy = Registration::link_popup(
            &raw mut (*popup).events.destroy,
            on_popup_destroy::<S>,
            (*bound).session,
            id,
        );
        let reposition = Registration::link_popup(
            &raw mut (*popup).events.reposition,
            on_popup_reposition::<S>,
            (*bound).session,
            id,
        );
        // Nested chains: a submenu is a popup whose parent is this popup, and
        // this is the signal that announces it with *this* popup's id in the
        // `Bound`.
        let new_popup = Registration::link_popup(
            &raw mut (*base).events.new_popup,
            on_new_popup::<S>,
            (*bound).session,
            id,
        );

        let displaced = (*session).popups.borrow_mut().insert(
            id,
            PopupListeners {
                _commit: commit,
                _map: map,
                _unmap: unmap,
                _destroy: destroy,
                _reposition: reposition,
                _new_popup: new_popup,
            },
        );
        drop(displaced);

        (*session).runtime.record_popup(id, raw, tree, parent);

        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::NewPopup(id), deliver);
    }
}
```

3f. **The five per-popup callbacks.** Add immediately after `on_new_popup`:

```rust
/// A popup's surface committed. Answer its **first** commit, which xdg-shell
/// requires or the popup never maps.
///
/// A near-copy of `on_surface_commit`'s skeleton, minus the decoration
/// synthesis (a popup has no decoration): gate on `initial_commit`, emit, then
/// schedule a configure unconditionally. The schedule goes through
/// `Popup::send_configure`, which is what refuses to call
/// `wlr_xdg_surface_schedule_configure` before `initialized` — this
/// distribution ships wlroots without `NDEBUG`, so an early call aborts the
/// process (see `layer.rs`'s module doc for the confirmed hazard).
unsafe extern "C" fn on_popup_commit<S: Handlers>(
    l: *mut sys::wl_listener,
    _data: *mut std::ffi::c_void,
) {
    // SAFETY: linked by `on_new_popup` into this surface's `events.commit`, and
    // unlinked (from `on_popup_destroy`) before the surface is freed. `_data` is
    // deliberately unused: the id comes from `Bound::popup`, the same
    // resolve-by-the-id-carried-at-link-time discipline
    // `on_layer_surface_commit` follows.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(id) = (*bound).popup else { return };
        let Some(raw) = (*session).runtime.popup_raw(id) else {
            return;
        };
        let base = (*raw.as_ptr()).base;
        if base.is_null() || !(*base).initial_commit {
            return;
        }

        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::PopupInitialCommit(id), deliver);

        // The unconditional answer. `deliver_all`'s own arm also schedules one
        // after the handler runs; wlroots coalesces the two into a single
        // configure, so this is the belt to that arm's braces — and it is what
        // answers the commit at all when the event was *deferred* (queued behind
        // an outer handler), where the arm has not run yet.
        if let Some(popup) = (*session).runtime.popup(id) {
            popup.send_configure();
        }
    }
}

/// The popup has a buffer.
unsafe extern "C" fn on_popup_map<S: Handlers>(
    l: *mut sys::wl_listener,
    _data: *mut std::ffi::c_void,
) {
    // SAFETY: linked by `on_new_popup` into this surface's `events.map`.
    // `_data` is deliberately unused: wlroots emits `wlr_surface.events.map`
    // with a **null** `data`, so the id must come from `Bound::popup` — the
    // identical argument `on_toplevel_map` makes.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(id) = (*bound).popup else { return };
        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::PopupMapped(id), deliver);
    }
}

/// The popup's buffer went away.
unsafe extern "C" fn on_popup_unmap<S: Handlers>(
    l: *mut sys::wl_listener,
    _data: *mut std::ffi::c_void,
) {
    // SAFETY: as for `on_popup_map` — `wlr_surface.events.unmap` is the other
    // signal wlroots emits with a null `data`.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(id) = (*bound).popup else { return };
        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::PopupUnmapped(id), deliver);
    }
}

/// The client sent `xdg_popup.reposition`.
///
/// wlroots has already written the new rules into `scheduled.rules` and set its
/// own reposition-token field; all this crate does is tell the handler, so it
/// can re-run its constraint computation. wlroots sends the
/// `xdg_popup.repositioned` event itself — nothing here forges one.
unsafe extern "C" fn on_popup_reposition<S: Handlers>(
    l: *mut sys::wl_listener,
    _data: *mut std::ffi::c_void,
) {
    // SAFETY: linked by `on_new_popup` into `wlr_xdg_popup.events.reposition`;
    // the popup is alive for the duration of the emission. `_data` is
    // deliberately unused: whether this signal carries a non-null `data` was not
    // verifiable from the shipped artefacts, and `Bound::popup` makes the
    // question moot — see that field's own doc.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(id) = (*bound).popup else { return };
        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::PopupReposition(id), deliver);
    }
}

/// A popup is about to be freed. Forget it *now*, whatever the handler does.
unsafe extern "C" fn on_popup_destroy<S: Handlers>(
    l: *mut sys::wl_listener,
    _data: *mut std::ffi::c_void,
) {
    // SAFETY: linked by `on_new_popup` into `wlr_xdg_popup.events.destroy`; the
    // popup is still alive for the duration of the emission. `_data` unused for
    // the reason `on_popup_reposition` gives.
    unsafe {
        let bound = bound_of(l);
        let session = (*bound).session.cast::<Session<'_, S>>();
        let Some(id) = (*bound).popup else { return };

        // Both tables are cleared before the event is emitted, and that ordering
        // is the whole soundness argument for deferral: a destroy queued behind
        // a running handler is delivered long after wlroots freed the object, so
        // a lookup at delivery time would resolve the id to freed memory.
        // Clearing here means it simply misses — the identical argument
        // `on_toplevel_destroy` makes.
        //
        // Removing the entry drops every registration it holds, one of which
        // owns this very `Bound`. `wl_signal_emit_mutable` advances past the
        // firing listener before calling us, so that is exactly what it exists
        // to tolerate; `bound` is dangling from here on and is not touched
        // again.
        //
        // The scene subtree is **not** destroyed. It is a child of the parent's
        // tree, and wlroots frees a tree's children recursively — calling
        // `wlr_scene_node_destroy` here would be the double free
        // `Runtime::forget_toplevel`'s own comment describes. `forget_popup`
        // drops the pointer and nothing more.
        (*session).runtime.forget_popup(id);
        let listeners = (*session).popups.borrow_mut().remove(&id);
        drop(listeners);

        let deliver = (*session).deliver;
        (*session)
            .dispatcher
            .emit(&*session, Event::PopupDestroyed(id), deliver);
    }
}
```

3g. **The two outer link sites.**

In `on_new_toplevel`, after the `request_resize` registration and before the
`(*session).toplevels.borrow_mut().insert(...)` call, add a **tenth**
registration and a field on `ToplevelListeners`:

```rust
        // The tenth listener: popups created on this toplevel. Linked on the
        // toplevel's `base` rather than on `wlr_xdg_shell` so the parent is
        // knowable — see `on_new_popup`'s own doc.
        let new_popup = Registration::link_toplevel(
            &raw mut (*base).events.new_popup,
            on_new_popup::<S>,
            (*bound).session,
            std::ptr::null(),
            id,
        );
```

```rust
struct ToplevelListeners {
    // … the existing nine …
    _new_popup: Registration,
}
```

with `_new_popup: new_popup,` added to the struct literal, and the struct's own
doc updated from "nine" to "ten" and from "its four client-request signals" to
"…, and the `new_popup` that announces a popup created on it".

In `on_new_layer_surface`, after the `destroy` registration, add a **fifth**:

```rust
        // The fifth listener: popups created on this layer surface — a panel's
        // menu. This is the *only* place a layer popup's parent is knowable:
        // the client creates it with `xdg_surface.get_popup(parent = NULL)` and
        // reparents it with `zwlr_layer_surface_v1.get_popup` afterwards, so at
        // shell level it has no parent at all.
        let new_popup = Registration::link_layer(
            &raw mut (*ls).events.new_popup,
            on_new_popup::<S>,
            (*bound).session,
            std::ptr::null(),
            id,
        );
```

with `_new_popup: Registration` added to `LayerSurfaceListeners` (doc updated
from "four" to "five") and `_new_popup: new_popup,` in its literal.

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup_bound_tests 2>&1 | tail -10
cargo test -p wlr 2>&1 | tail -25
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
git diff --stat HEAD -- crates/wlr/src/backend.rs
```

Expected: five new tests pass; **every** existing suite green, including
`tests/layers.rs`, `tests/toplevels.rs` and `mod implicit_grab_tests`.

Verify the untouchable module really is untouched:

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git diff HEAD -- crates/wlr/src/backend.rs | grep -c 'implicit_grab' 
```

Expected: `0`. If it is anything else, **stop and report** — the contract's P1
boundary forbids touching that code or its tests.

**Mutation checks (perform all three).**

1. In `popup_parent_from_slots`, move the `toplevel` arm to the top.
   `the_deepest_slot_wins_if_two_are_somehow_set` must fail. Restore.
2. In `on_new_popup`, change the `PopupParent::Layer(ls)` arm of the
   `parent_tree` match to `(*session).runtime.toplevel_band_ptr()`. Nothing in
   this crate fails (no client), so record in the commit body that
   `a_popup_under_a_layer_panel_is_constrained_to_the_same_output` in P2 is what
   covers it, and restore.
3. In `on_popup_destroy`, move the `emit` **above** the `forget_popup` +
   `remove` pair. Nothing in this crate fails; P2's
   `destroying_a_parent_destroys_its_popup_chain_without_a_double_free` is what
   covers it. Record and restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/backend.rs
git commit -m "$(cat <<'EOF'
feat(wlr): announce, track and tear down xdg-popups

Listeners go on the parent-scoped `new_popup` signals — a toplevel's base, a
layer surface's own, and each popup's base — and never on
`wlr_xdg_shell.events.new_popup`. At shell level a layer-shell popup still has
`parent == NULL`, because the client creates it with a null xdg parent and
reparents it with `zwlr_layer_surface_v1.get_popup` afterwards, so the shell
signal cannot classify the one case that needs classifying. Which of the three
sites an emission came from is read out of the `Bound`'s id slot.

Identity travels in `Bound::popup`, never in the signal's `data`: wlroots emits
`wlr_surface.events.map`/`.unmap` with a null `data`, and whether the popup's own
destroy and reposition signals do was not verifiable from the shipped artefacts.
Carrying the id makes the question moot.

The scene subtree is created under the parent's tree, so popups stack with their
parent and `leaf_surface_at` resolves clicks on them — session-lock isolation
included — with no new code. `on_popup_destroy` drops the pointer and never
destroys the tree: wlroots frees a tree's children recursively, and the parent
may already be dying.

`on_popup_commit` answers the initial commit unconditionally through
`Popup::send_configure`, which refuses to call
`wlr_xdg_surface_schedule_configure` before `initialized` — that assert is a
process abort on this distribution's wlroots.

The implicit-pointer-grab code and `mod implicit_grab_tests` are untouched.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9 — the mislabelling fix, and the run-granularity purge

**Files:** `crates/wlr/src/backend.rs`.

Task 8 made popup surfaces carry an id addon. `toplevel_id_of_surface`
(`backend.rs:5197`) wraps **whatever** `find_id` returns in a `ToplevelId` with
no role check, so it now hands back a `ToplevelId` for a popup surface. A
`PopupId` and a `ToplevelId` can never *collide* — one counter, never reused —
but they can be *mislabelled*, and a mislabelled id is a live bug the moment any
caller stops bailing on the table miss. This task closes it and wires the purge
`run_inner` owes the popup table.

**Interfaces.**

*Consumes:* Task 4's `clear_popups`.

*Produces:*

```rust
unsafe fn toplevel_id_of_surface(surface: *mut sys::wlr_surface) -> Option<ToplevelId>;  // now role-checked
```

**Step 1 — write the failing test.** Append to `mod popup_bound_tests`:

```rust
    /// `toplevel_id_of_surface` must not hand back a `ToplevelId` for a surface
    /// that is actually a popup's. Both id kinds come from one counter, so they
    /// never collide — but before the role check they were freely
    /// *mislabelled*, and every caller's safety rested on the table miss that
    /// followed rather than on the id being right.
    ///
    /// A real `wlr_surface` cannot be built here (it needs a compositor), so
    /// this exercises the decision function the callback delegates to, with the
    /// role probe's answer supplied directly.
    #[test]
    fn a_surface_that_is_a_popup_yields_no_toplevel_id() {
        let id = ToplevelId::dangling_for_test();
        assert_eq!(toplevel_id_if_not_a_popup(Some(id), false), Some(id));
        assert_eq!(toplevel_id_if_not_a_popup(Some(id), true), None);
        assert_eq!(toplevel_id_if_not_a_popup(None, false), None);
        assert_eq!(toplevel_id_if_not_a_popup(None, true), None);
    }
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --lib popup_bound_tests 2>&1 | tail -10
```

Expected failure: `error[E0425]: cannot find function
`toplevel_id_if_not_a_popup` in this scope`.

**Step 3 — implement.**

3a. Replace `toplevel_id_of_surface` with the role-checked pair:

```rust
/// The decision `toplevel_id_of_surface` makes, separated from the FFI so it can
/// be tested without a live `wlr_surface`.
///
/// An id addon on a popup's surface is a real id — it is just not a
/// **toplevel's**. Returning `None` is the honest answer; returning the id
/// anyway would be a `ToplevelId` naming a popup, and the only thing standing
/// between that and a wrong-window bug would be the table miss that happens to
/// follow at every current call site.
fn toplevel_id_if_not_a_popup(id: Option<ToplevelId>, is_popup: bool) -> Option<ToplevelId> {
    if is_popup { None } else { id }
}

/// Recover the [`ToplevelId`] a surface's id addon carries, if any — and if the
/// surface is not in fact a popup's.
///
/// The role check is not cosmetic. Since 0.20.28 a popup's `wlr_surface`
/// carries an id addon of its own (that is where `PopupId` lives, for the same
/// reason `ToplevelId` does), and `find_id` cannot tell the two apart: they come
/// from one process-wide counter, so they never collide, but without this check
/// they are freely mislabelled.
///
/// # Safety
///
/// `surface` must be a live `wlr_surface` with an initialised addon set.
unsafe fn toplevel_id_of_surface(surface: *mut sys::wlr_surface) -> Option<ToplevelId> {
    // SAFETY: the caller guarantees the surface is live.
    unsafe {
        let id = find_id(&raw const (*surface).addons).map(ToplevelId);
        let is_popup = !sys::wlr_xdg_popup_try_from_wlr_surface(surface).is_null();
        toplevel_id_if_not_a_popup(id, is_popup)
    }
}
```

3b. Wire `clear_popups` into `run_inner`, on the same statement group as
`clear_toplevels`. Find that call:

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
grep -n 'clear_toplevels' crates/wlr/src/backend.rs
```

and add, immediately after it:

```rust
        // Popup ids are only meaningful for the call that announced them, for
        // the reason `clear_toplevels` documents at length: the per-popup
        // destroy listener that would remove a stale row is torn down with this
        // call's `Session`. Purged on every exit path, including an early `?`
        // and a panic — this sits with `clear_toplevels` precisely so the two
        // cannot drift apart.
        runtime.clear_popups();
```

Match the surrounding code's receiver name (`runtime` vs `self.runtime`) exactly
as `clear_toplevels`' own line spells it.

**Step 4 — run it and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr 2>&1 | tail -25
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: everything green, including the pre-existing toplevel suites — the
role check must not have broken ordinary toplevel commits, which is what
`tests/toplevels.rs` and `tests/decoration.rs` cover.

**Mutation checks (perform both).**

1. In `toplevel_id_if_not_a_popup`, invert the condition to
   `if is_popup { id } else { None }`.
   `a_surface_that_is_a_popup_yields_no_toplevel_id` must fail, **and** so must
   several pre-existing toplevel tests (every ordinary commit now resolves to
   `None`), which is a useful second signal. Restore.
2. Delete the `runtime.clear_popups();` line. Nothing fails in this crate — the
   hazard needs a consumer holding a `Runtime` clone across `run_all`'s return.
   Record that in the commit body and restore. (The equivalent toplevel hazard
   has the same shape and the same absence of a unit test; see
   `clear_toplevels`' own doc.)

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/src/backend.rs
git commit -m "$(cat <<'EOF'
fix(wlr): do not hand a popup surface back as a ToplevelId

A popup's `wlr_surface` now carries an id addon of its own, and `find_id` cannot
tell a `PopupId` from a `ToplevelId` — one counter issues both, so they never
collide, but without a role check they are freely mislabelled.
`wlr_xdg_popup_try_from_wlr_surface` is that check. Every current caller happened
to survive the mislabelling because the wrong id missed in the toplevel table;
resting on that would have made the next caller a wrong-window bug.

`clear_popups` joins `clear_toplevels` in `run_inner` so popup ids stop
resolving when the run that announced them returns, for the reason that method
documents: the destroy listener that would otherwise purge a stale row is torn
down with the run's `Session`. No unit test covers it — the hazard needs a
consumer holding a `Runtime` clone past `run_all` — which is exactly the
situation `clear_toplevels` is in.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10 — `crates/wlr/tests/popups.rs`: the public-surface integration suite

**Files:** `crates/wlr/tests/popups.rs`.

**Interfaces.**

*Consumes:* everything P1 has produced, through the crate's **public** API only
(an integration-test binary sees nothing else).

*Produces:* the test binary that is part of P1's gate.

**Step 1 — write the failing tests.** Append to the file created in Tasks 6–7:

```rust
#[test]
fn every_by_id_popup_operation_misses_on_an_id_no_popup_was_given() {
    headless_env();
    let runtime = wlr::Runtime::new().expect("runtime");
    let dead = wlr::PopupId::dangling_for_test();

    assert!(runtime.popup(dead).is_none());
    assert_eq!(runtime.popup_parent(dead), None);
    assert_eq!(runtime.popup_position(dead), None);
    assert!(!runtime.popup_is_grabbing(dead));
    assert!(!runtime.configure_popup(dead, &wlr::Box2D::new(0, 0, 800, 600)));
    assert_eq!(runtime.dismiss_popup(dead), 0);
    assert!(runtime.popups_of(wlr::PopupParent::Popup(dead)).is_empty());
    assert!(runtime.popup_chain(wlr::PopupParent::Popup(dead)).is_empty());
    assert_eq!(wlr::PopupParent::Popup(dead).root(&runtime), None);
}

/// Every distinct dangling id must miss, not just the canonical one — a
/// compositor's own tests drive several popups at once and need more than one
/// id that resolves to nothing.
#[test]
fn several_dangling_popup_ids_all_miss_and_stay_distinct() {
    headless_env();
    let runtime = wlr::Runtime::new().expect("runtime");
    let ids: Vec<_> = (1..=4).map(wlr::PopupId::dangling_nth_for_test).collect();
    for (i, a) in ids.iter().enumerate() {
        assert!(runtime.popup(*a).is_none());
        for b in &ids[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

/// A parent that is a live window with no popups reports an empty chain rather
/// than an error — the shape a compositor's "dismiss everything under this
/// window" path calls on every unmap.
#[test]
fn a_parent_with_no_popups_has_an_empty_chain_and_dismisses_nothing() {
    headless_env();
    let runtime = wlr::Runtime::new().expect("runtime");
    let window = wlr::PopupParent::Toplevel(wlr::ToplevelId::dangling_for_test());
    assert!(runtime.popups_of(window).is_empty());
    assert!(runtime.popup_chain(window).is_empty());
    assert_eq!(runtime.dismiss_popups_of(window), 0);
    assert_eq!(window.root(&runtime), Some(window));
}

/// `seat_has_explicit_grab` is called from focus paths that run before a seat
/// exists. It must answer, not dereference.
#[test]
fn asking_about_an_explicit_grab_before_there_is_a_seat_is_false() {
    headless_env();
    let runtime = wlr::Runtime::new().expect("runtime");
    assert!(!runtime.seat_has_explicit_grab());
}

/// The positioner types are usable from outside the crate: a consumer building
/// its own placement policy needs to construct a `PositionerRules`-shaped
/// question and read the answer. This also pins that the public field set is
/// what the contract froze — a renamed or removed field fails to compile here.
#[test]
fn the_positioner_types_are_usable_from_outside_the_crate() {
    let c = wlr::ConstraintAdjustment::FLIP_X | wlr::ConstraintAdjustment::SLIDE_Y;
    assert!(c.contains(wlr::ConstraintAdjustment::FLIP_X));
    assert!(!c.contains(wlr::ConstraintAdjustment::FLIP_Y));

    // Every public field, named. If the contract's shape changes, this stops
    // compiling, which is the point.
    fn describe(r: &wlr::PositionerRules) -> (i32, i32, bool) {
        let _ = (
            r.anchor,
            r.gravity,
            r.constraint_adjustment,
            r.offset,
            r.parent_size,
            r.parent_configure_serial,
            r.anchor_rect,
        );
        (r.size.0, r.size.1, r.reactive)
    }
    let _ = describe as fn(&wlr::PositionerRules) -> (i32, i32, bool);

    assert_eq!(
        wlr::PositionerAnchor::BottomLeft,
        wlr::PositionerAnchor::BottomLeft
    );
    assert_ne!(
        wlr::PositionerGravity::Top,
        wlr::PositionerGravity::Bottom
    );
}

/// `PopupParent` is `Hash` + `Eq` because a compositor keys its own popup
/// registry by it. Pinning that here keeps a derive from being dropped.
#[test]
fn popup_parent_is_usable_as_a_map_key() {
    use std::collections::HashMap;
    let mut m: HashMap<wlr::PopupParent, u32> = HashMap::new();
    let w = wlr::PopupParent::Toplevel(wlr::ToplevelId::dangling_for_test());
    let l = wlr::PopupParent::Layer(wlr::LayerSurfaceId::dangling_for_test());
    let p = wlr::PopupParent::Popup(wlr::PopupId::dangling_for_test());
    m.insert(w, 1);
    m.insert(l, 2);
    m.insert(p, 3);
    assert_eq!(m.len(), 3);
    assert_eq!(m.get(&w), Some(&1));
    assert!(p.is_popup());
    assert!(!w.is_popup() && !l.is_popup());
}
```

**Step 2 — run it and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --test popups 2>&1 | tail -20
```

Expected: **pass**, because Tasks 1–9 already built everything these tests call.
That is the wrong colour for a TDD step, so establish the red state by
temporarily commenting out the `pub use popup::{…}` block in `lib.rs` and
re-running: the binary must fail to compile with `error[E0433]: failed to
resolve: could not find `PopupId` in `wlr``. Restore the block and re-run to
green. This test file's job is to pin the **public** surface, so "the export
block is what makes it compile" is precisely the property to demonstrate.

**Step 3 — implement.** Nothing to implement; the code exists. If any test fails
for a real reason, fix it in the task that owns the code, not here.

**Step 4 — run the whole gate.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr 2>&1 | tail -30
cargo clippy -p wlr --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy -p wlr --all-targets --no-default-features -- -D warnings 2>&1 | tail -5
cargo fmt --all --check
```

Expected: every suite green under both feature sets.

**Mutation check.** Remove `PopupParent` from `lib.rs`'s `pub use popup::{…}`
block. `popup_parent_is_usable_as_a_map_key` and three others must fail to
compile. Restore.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/tests/popups.rs
git commit -m "$(cat <<'EOF'
test(wlr): pin the public popup surface

An integration-test binary sees only what `lib.rs` exports, which makes this the
file that notices a missing re-export, a dropped derive or a renamed public
field — none of which the in-crate tests can see.

The behavioural half is the by-id miss: every popup operation on an id no popup
was ever given reports nothing rather than dereferencing, which is the promise
every id type in this crate carries and the reason `dangling_for_test` is public
at all.

Anything needing a live client — placement, flipping, nested chains, grab
dismissal, focus restore — is P2's `compositor/tests/popups.rs` against the
harness, and is deliberately not faked here.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11 — the coverage ledger

**Files:** `crates/wlr/coverage/wrapped.toml`, `crates/wlr/coverage/waived.toml`.

The audit has three hard rules (`coverage/README.md`): a symbol in **both** files
fails; a `wrapped` row whose symbol has **no** `sys::` use under
`crates/wlr/src` fails; and a `not-yet` waiver whose symbol **does** have a
`sys::` use fails. Tasks 1–9 wrote `sys::wlr_xdg_popup_*`,
`sys::wlr_xdg_positioner_rules*` and `sys::wlr_seat_keyboard_has_grab`, so the
third rule is already being tripped — this task is what stops it, and the
contract requires the ledger to move **in the same commit as the wrapper**. It is
one commit late by construction (the wrappers landed across nine commits); the
release commit in Task 13 is what the audit gates, and Task 11 runs before it.

**Interfaces.** Consumes nothing in code. Produces a green
`cargo test -p wlr --test coverage_audit`.

**Step 1 — run the audit and watch it fail.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
export WLR_COVERAGE_BINDINGS=<the path recorded in Task 0>
export WLR_COVERAGE_ALL_FEATURES=1
cargo test -p wlr --test coverage_audit 2>&1 | tail -40
```

Expected failure: a list headed roughly `not-yet waiver(s) whose symbol is used
in crates/wlr/src`, naming the twelve `wlr_xdg_*` symbols plus
`wlr_seat_keyboard_has_grab`.

**Step 2 — move the rows.**

2a. **Delete** these thirteen blocks from `waived.toml`. Each is a four-line
`[[waived]]` block; delete the block and the blank line after it. Their
line numbers before this edit (they shift as you go — work **bottom-up**):

`4541` `wlr_xdg_positioner_rules_unconstrain_box`,
`4535` `wlr_xdg_positioner_rules_get_geometry`,
`4529` `wlr_xdg_positioner_rules`,
`4505` `wlr_xdg_popup_unconstrain_from_box`,
`4499` `wlr_xdg_popup_try_from_wlr_surface`,
`4493` `wlr_xdg_popup_state`,
`4481` `wlr_xdg_popup_get_toplevel_coords`,
`4475` `wlr_xdg_popup_get_position`,
`4463` `wlr_xdg_popup_destroy`,
`4457` `wlr_xdg_popup_configure_field`,
`4451` `wlr_xdg_popup_configure`,
`4445` `wlr_xdg_popup`,
`2737` `wlr_seat_keyboard_has_grab`.

2b. **Re-reason** the one that stays, at (post-edit) `wlr_xdg_popup_grab`:

```toml
[[waived]]
symbol = "wlr_xdg_popup_grab"
reason = "internal"
note   = "wlroots constructs and destroys this itself on xdg_popup.grab and installs three seat grabs from it; the export table has no wlr_xdg_popup_grab_* function at all, so a consumer can never hold one. The observables this crate offers instead are Popup::grab_requested (popup->seat) and Runtime::seat_has_explicit_grab (wlr_seat_pointer_has_grab / wlr_seat_keyboard_has_grab)."
```

No `milestone` key: `internal` does not take one.

2c. **Leave alone**, unchanged: `wlr_xdg_popup_from_resource`,
`wlr_xdg_positioner`, `wlr_xdg_positioner_from_resource`,
`wlr_xdg_positioner_is_complete` (all resource-only, unreachable from the safe
API — matching `wlr_xdg_surface_from_resource`'s own standing waiver);
`wlr_xdg_surface_for_each_popup_surface`, `wlr_xdg_surface_popup_surface_at`,
`wlr_layer_surface_v1_for_each_popup_surface`,
`wlr_layer_surface_v1_popup_surface_at` (the scene graph covers all four —
`Runtime::leaf_surface_at` resolves a click on a popup today).

2d. **Add** these thirteen blocks to `wrapped.toml`, each separated by a blank
line, inserted in the file's existing ordering:

```toml
[[wrapped]]
symbol = "wlr_xdg_popup"
module = "popup"
item   = "Popup"

[[wrapped]]
symbol = "wlr_xdg_popup_configure"
module = "popup"
item   = "Popup::reposition_token"

[[wrapped]]
symbol = "wlr_xdg_popup_configure_field"
module = "popup"
item   = "Popup::reposition_token"

[[wrapped]]
symbol = "wlr_xdg_popup_destroy"
module = "popup"
item   = "Popup::destroy"

[[wrapped]]
symbol = "wlr_xdg_popup_get_position"
module = "popup"
item   = "Popup::position"

[[wrapped]]
symbol = "wlr_xdg_popup_get_toplevel_coords"
module = "popup"
item   = "Popup::toplevel_coords"

[[wrapped]]
symbol = "wlr_xdg_popup_state"
module = "popup"
item   = "Popup::geometry"

[[wrapped]]
symbol = "wlr_xdg_popup_try_from_wlr_surface"
module = "backend"
item   = "toplevel_id_of_surface role check"

[[wrapped]]
symbol = "wlr_xdg_popup_unconstrain_from_box"
module = "popup"
item   = "Popup::unconstrain"

[[wrapped]]
symbol = "wlr_xdg_positioner_rules"
module = "popup"
item   = "PositionerRules"

[[wrapped]]
symbol = "wlr_xdg_positioner_rules_get_geometry"
module = "popup"
item   = "PositionerRules::geometry"

[[wrapped]]
symbol = "wlr_xdg_positioner_rules_unconstrain_box"
module = "popup"
item   = "PositionerRules::unconstrain_box"

[[wrapped]]
symbol = "wlr_seat_keyboard_has_grab"
module = "runtime"
item   = "Runtime::seat_has_explicit_grab"
```

**Step 3 — run the audit and watch it pass.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo test -p wlr --test coverage_audit 2>&1 | tail -20
cargo xtask coverage 2>&1 | tail -20
```

Expected: the audit passes, and `cargo xtask coverage` shows the wrapped count up
by thirteen and the `not-yet` backlog down by thirteen. Record both numbers; the
release commit body quotes them.

**Mutation checks (perform both).**

1. Delete the `wlr_xdg_popup_state` block from `wrapped.toml` and re-add it to
   `waived.toml` as `not-yet`/`M9`. The audit must fail with "a `not-yet` row
   whose symbol appears in a `sys::` use". Restore.
2. Add a `wlr_xdg_popup_destroy` row to `waived.toml` **without** removing the
   wrapped one. The audit must fail with the both-files rule. Restore.

**Step 4 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/coverage/wrapped.toml crates/wlr/coverage/waived.toml
git commit -m "$(cat <<'EOF'
chore(wlr): move thirteen popup symbols from waived to wrapped

The audit fails a `not-yet` waiver whose symbol appears in a `sys::` use, which
is exactly what writing the popup wrappers did.

`wlr_xdg_popup_grab` stays waived but is re-reasoned from `not-yet`/M9 to
`internal`: the milestone promised something that can never be delivered, because
the export table has no `wlr_xdg_popup_grab_*` function at all. wlroots builds
the struct on `xdg_popup.grab`, installs three seat grabs from it and tears it
down itself; the note now points at the two observables this crate does offer.

The four resource-only positioner/popup symbols and the four popup-iteration
helpers stay waived: the first group needs a `wl_resource` the safe API never
hands out, and the scene graph already covers the second.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12 — README: the xdg-popup section

**Files:** `crates/wlr/README.md`.

**Interfaces.** Consumes nothing. Produces the consumer-facing document P2 reads
before writing a single line of compositor code.

**Step 1 — check the current shape.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
grep -n '^## 0\.20\.' crates/wlr/README.md | head
```

The newest section is `## 0.20.27 — implicit pointer grab`, and new sections go
**immediately above** it (that is where 0.20.27 itself was inserted — see
`git show 704a374 -- crates/wlr/README.md`).

**Step 2 — write the section.** Insert directly above the `## 0.20.27` line:

```markdown
## 0.20.28 — xdg-popup

Menus, tooltips, dropdowns and popovers. A popup can hang off a toplevel, off a
wlr-layer-shell surface (a panel's menu), or off another popup (a submenu), and
[`Popup::parent`] tells you which.

### Listeners are parent-scoped, and that is not a detail

This crate listens on `wlr_xdg_surface.events.new_popup` (on each toplevel's
`base`, and on each popup's own `base`) and on
`wlr_layer_surface_v1.events.new_popup`. It deliberately does **not** listen on
`wlr_xdg_shell.events.new_popup`.

A layer-shell popup is created by the client as `xdg_surface.get_popup` with
**parent = NULL** and only then reparented by `zwlr_layer_surface_v1.get_popup`,
which the layer-shell protocol requires to happen before the popup's initial
commit. So at shell level a layer popup has no parent at all, and the one case
that most needs classifying is the one case that cannot be classified. The
parent-scoped signals know the answer by construction.

The consequence for you: `ToplevelHandler::new_popup` is the only place the
parent is knowable, `Popup::parent` returns what was recorded there, and neither
this crate nor your compositor should ever read `(*popup).parent`.

### wlroots owns the grab — do not fight it

When a client sends `xdg_popup.grab`, wlroots builds a `wlr_xdg_popup_grab`,
installs pointer, keyboard **and** touch grabs on the seat, routes all three to
the popup chain, dismisses the chain (`xdg_popup.popup_done`) on a press outside
it, and restores the pre-grab keyboard focus when the grab ends. There is no API
to create, inspect or end one — the export table has no `wlr_xdg_popup_grab_*`
symbol.

So this crate observes rather than drives, and your compositor should too:

- `Popup::grab_requested()` / `Runtime::popup_is_grabbing(id)` — this popup's
  client asked for a grab.
- `Runtime::seat_has_explicit_grab()` — *some* explicit grab (a popup's, a
  drag's) is in force right now. **Return early from your focus synchronisation
  while this is true.** Moving keyboard focus yourself during a popup grab
  fights wlroots' own routing and breaks chain dismissal.
- Do **not** call `wlr_seat_pointer_start_grab` / `_keyboard_start_grab` /
  `_end_grab` for a popup. A second `start_grab` displaces wlroots' own.
- Do **not** add a "focus the popup" call. `wlr_seat_keyboard_notify_enter`
  routes *through* the active keyboard grab, so `Runtime::focus_toplevel_keyboard`
  already does the right thing by construction.

An explicit popup grab supersedes the implicit pointer grab 0.20.27 added. That
is not new code: the implicit grab drops itself the moment
`wlr_seat_pointer_has_grab` becomes true, which is precisely what a popup grab
makes true.

Non-grabbing popups — tooltips, non-modal popovers — get no grab and no focus
restore. Their focus is your compositor's decision, explicitly.

### Placement is two calls, and the box is in an unusual space

There is no `wlr_xdg_popup_set_size` or `_configure`. Placing a popup is
`wlr_xdg_popup_unconstrain_from_box` (which rewrites the scheduled geometry from
the client's positioner rules) followed by a scheduled configure, and
`Runtime::configure_popup(id, &constraint)` is that pair.

**`constraint` is in the ROOT TOPLEVEL PARENT surface's coordinate system** — not
layout space, not output space. wlroots' own header says so. Translating your
output's usable area into that space is your compositor's job; getting it wrong
offsets every popup rather than failing loudly.

`Popup::positioner_rules()` hands you the client's whole positioner, copied out,
and `PositionerRules::{geometry, unconstrain_box}` answer placement questions
without touching a live popup at all — which is what a compositor deciding where
a menu *would* go needs.

### The scene graph does the stacking and the hit-testing for you

Each popup gets its subtree from `wlr_scene_xdg_surface_create(parent_tree,
popup->base)`, where `parent_tree` is its parent's. So a popup stacks with its
parent automatically, `Runtime::leaf_surface_at` resolves clicks on it, and the
session-lock isolation gate covers it — all without a line of compositor code.

This crate never destroys a popup's scene tree. wlroots frees a tree's children
recursively with the tree, and a popup's tree is a child of its parent's.

### Chains

`Runtime::popups_of(parent)` lists direct children in creation order (which is
the sibling z-order tiebreak); `Runtime::popup_chain(parent)` is the whole
subtree, parents before children. `Runtime::dismiss_popup(id)` destroys a popup
and everything under it **deepest-first**, which is the order xdg-shell requires,
and returns how many it destroyed. `PopupParent::root(&runtime)` walks a nested
chain down to the window or layer surface at the bottom.

All of these are depth-capped and de-duplicated. wlroots cannot produce a cycle,
but every one of them runs on your input path, where an unbounded walk over a
corrupted table is a session freeze.

### Handler methods (`ToplevelHandler`, all defaulted)

`new_popup`, `popup_initial_commit`, `popup_mapped`, `popup_unmapped`,
`popup_reposition`, `popup_destroyed`. All defaulted, so
`impl ToplevelHandler for S {}` written against 0.20.27 still compiles.

If you implement none of them, popups still map: the dispatch layer answers a
popup's initial commit unconditionally, because xdg-shell requires an answer and
a defaulted method has no `Runtime` to send one with. They map where the client's
own positioner asked, unconstrained.

`popup_destroyed`'s id may be one you were never told about — a popup created and
destroyed while another handler was running produces a destroy with no preceding
`new_popup`. `remove` from a map; never index.

[`Popup::parent`]: https://docs.rs/wlr/latest/wlr/struct.Popup.html#method.parent
```

**Step 3 — check the docs build.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps 2>&1 | tail -20
```

Expected: no warnings. `#![warn(missing_docs)]` is on, so every public item added
in Tasks 1–5 needs its doc comment; a missing one surfaces here, and under
`-D warnings` it is an error. Fix any it names in the owning file.

**Step 4 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/README.md
git commit -m "$(cat <<'EOF'
docs(wlr): the xdg-popup section

Four things a consumer cannot infer from the API and will otherwise get wrong:
the listeners are parent-scoped because a layer popup has no parent at shell
level; wlroots owns the popup grab entirely and a compositor that also moves
focus during one breaks chain dismissal; the constraint box is in the root
toplevel parent surface's coordinates rather than layout or output space; and the
scene subtree already gives stacking, hit-testing and lock isolation, so nothing
downstream should re-implement or destroy it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 13 — the release commit and the full gate

**Files:** `crates/wlr/Cargo.toml`.

**Interfaces.** Consumes everything. Produces the 0.20.28 version bump and a
green, complete gate — the state the consent stop in Task 14 asks about.

**Step 1 — bump the version.** In `crates/wlr/Cargo.toml`:

```toml
version = "0.20.28"
```

Change nothing else in that file. Do **not** add a dependency (D1), do not touch
`exclude`, `edition.workspace`, `rust-version.workspace` or the feature table.

**Step 2 — run the whole gate, in this order.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
export WLR_COVERAGE_BINDINGS=<the path recorded in Task 0>
export WLR_COVERAGE_ALL_FEATURES=1

cargo fmt --all --check                                                  # 1
cargo build -p wlr --all-features                                        # 2
cargo test -p wlr                                                        # 3
cargo test -p wlr --test coverage_audit                                  # 4
cargo xtask coverage                                                     # 5
cargo clippy -p wlr --all-targets -- -D warnings                         # 6
cargo clippy -p wlr --all-targets --no-default-features -- -D warnings   # 7
RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps                     # 8
cargo publish -p wlr --dry-run                                           # 9
```

Every one must succeed. Notes on the ones that can surprise:

- **(3)** includes `tests/compile_fail.rs`, which pins that `Handlers` is still
  sealed by a blanket impl with no supertrait. If that file fails, Task 6 added
  the methods to the wrong trait.
- **(3)** also includes `mod implicit_grab_tests`. It must pass **unmodified**;
  confirm with `git diff develop -- crates/wlr/src/backend.rs | grep -c
  implicit_grab`, which must print `0`.
- **(9)** packages the crate. `exclude` keeps `coverage/**`, `src/coverage.rs`
  and `tests/coverage_audit.rs` out of the tarball; `crates/wlr/tests/popups.rs`
  **is** included and must compile from a clean extraction, which the dry-run
  verifies. It will also warn about the `wlr-sys` path dependency resolving to
  the published `0.20` — that warning is expected and is how every previous
  release of this crate has gone out.

**Step 3 — verify the diff is only what this part owns.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git diff --stat develop
```

Expected files, and only these: `crates/wlr/Cargo.toml`,
`crates/wlr/README.md`, `crates/wlr/coverage/waived.toml`,
`crates/wlr/coverage/wrapped.toml`, `crates/wlr/src/backend.rs`,
`crates/wlr/src/dispatch.rs`, `crates/wlr/src/handler.rs`,
`crates/wlr/src/lib.rs`, `crates/wlr/src/popup.rs`,
`crates/wlr/src/runtime.rs`, `crates/wlr/tests/popups.rs`.

Anything else — especially anything under `crates/wlr-sys/` or `xtask/` — is
out of P1's boundary. **Stop and report.**

**Step 4 — add the release notes' version line.** The README section from Task
12 is already headed `## 0.20.28 — xdg-popup`; nothing more is needed there.

**Step 5 — commit.**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git add crates/wlr/Cargo.toml
git commit -m "$(cat <<'EOF'
release(wlr): 0.20.28 — xdg-popup

Popups on toplevels, on layer surfaces and on other popups; positioner geometry
and unconstraining; reposition; a scene subtree parented under the popup's own
parent; chain walking and deepest-first dismissal; and two grab observables.

Additive: every new handler method is defaulted, so `impl ToplevelHandler for S
{}` written against 0.20.27 still compiles, and `tests/popups.rs` asserts that
with an empty impl block.

The implicit pointer grab 0.20.27 added is untouched, tests included. An
xdg-popup grab is exactly the "explicit grab" that code already defers to.

Coverage: thirteen symbols moved waived to wrapped; `wlr_xdg_popup_grab`
re-reasoned `not-yet`/M9 to `internal`, since no `wlr_xdg_popup_grab_*` function
exists to wrap.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 14 — contract amendments

**Files:**
`/home/joseph/Projects/icedtea/docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

The contract's §10 is the record of every deviation a part discovers. This
plan's three are listed at the top of this document; they now go into the
contract itself, in its own `### E<n>` shape, so P2–P8 read one authority rather
than two.

**Step 1 — append to §10** (which is empty at freeze, so these are E1–E3):

```markdown
### E1 — `ConstraintAdjustment` is hand-rolled, not a `bitflags` type

§1.1 spells `ConstraintAdjustment` with `bitflags::bitflags!`. The `wlr` crate
has no `bitflags` dependency and declines one twice in its own source
(`buffer.rs:386` for `DataPtrAccess`, and `BufferCaps` before it), pinning the
bit values with a test instead. **Ruling:** P1 hand-rolls it —
`pub struct ConstraintAdjustment(u32)` with six associated consts, `BitOr`,
`BitOrAssign`, `contains` and `bits`. Every call site spells identically
(`ConstraintAdjustment::FLIP_X | ConstraintAdjustment::SLIDE_Y`,
`.contains(...)`), so no other part is affected. **Carried out by:** P1.

### E2 — `PopupId` gains the two dangling constructors

§1.1 lists only `PopupId`'s derives. `ToplevelId` and `LayerSurfaceId` both
carry `dangling_for_test` (and `ToplevelId` also `dangling_nth_for_test`), and
without them the "an unknown id misses rather than dereferencing" promise cannot
be tested by a consumer — `crates/wlr/tests/popups.rs` and P2's `State` unit
tests both need it. **Ruling:** additive; P1 adds
`PopupId::{dangling_for_test, dangling_nth_for_test}` mirroring `ToplevelId`'s
exactly, including the 2^32 band. **Carried out by:** P1. **Consumed by:** P2
(`PopupKey::for_test` can wrap `PopupId::dangling_nth_for_test`).

### E3 — an unknown positioner enum value is dropped silently, not logged

§1.1 and the spec's §7 both say an unknown `xdg_positioner_anchor`/`_gravity`
value "maps to `None` and is logged once, never panics". The `wlr` crate binds
**no** Rust-side logging symbol and says so in its own source
(`runtime.rs:8386-8389`): wlroots' `wlr_log` is a `static inline` macro over an
unbound `_wlr_log`, and the crate deliberately has no `log`/`tracing`
dependency. **Ruling:** the never-panic half stands in full and is directly
tested (`a_positioner_with_nonsense_enum_values_never_panics`); the logging half
is dropped in `wlr` and **moves to P2**, which has logging and is where a
malformed positioner first becomes an observable compositor decision.
**Carried out by:** P1 (the silent mapping), P2 (the log).
```

**Step 2 — commit, in the icedtea repo.**

```bash
cd /home/joseph/Projects/icedtea
git add docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
docs(m3): record P1's three contract amendments

E1 hand-rolls `ConstraintAdjustment` rather than taking a `bitflags` dependency
into a published crate that declines one twice in its own source; the public
spelling is unchanged.

E2 adds `PopupId`'s two dangling constructors, without which the by-id-miss
promise has no consumer-writable test.

E3 drops the "logged once" half of the unknown-positioner-value rule in `wlr`,
which binds no Rust-side logging symbol at all, and moves it to P2. The
never-panic half stands and is directly tested.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 15 — publish 0.20.28 (CONSENT STOP)

**Files:** none.

**This task does not run without the user's explicit go-ahead.** Publishing to
crates.io is irreversible: a version cannot be replaced, only yanked, and every
later M3 part pins `wlr = "0.20.28"`.

**Step 1 — present the state and ask.** Report, in one message:

- the branch and its commit range (`git log --oneline develop..HEAD`);
- the gate results from Task 13, all nine, quoted;
- the coverage numbers before and after (from Task 11's `cargo xtask coverage`);
- the confirmation that `mod implicit_grab_tests` is byte-identical;
- the three contract amendments;

then ask: **publish `wlr` 0.20.28 now, or hold while you review?**

Per the user's standing rule, do not merge or publish automatically, and offer
the review window explicitly (`/simplify`, `/code-review`, `/optimize`).

**Step 2 — only on an explicit yes:**

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
cargo publish -p wlr
```

No `--token`, no secrets-manager lookup: the crates.io token is already wired
through Cargo's libsecret provider (`cargo:libsecret` in `~/.cargo/config.toml`)
and `cargo publish` authenticates on its own.

**Step 3 — confirm it landed.**

```bash
sleep 30
cargo search wlr --limit 1
```

Expected: `wlr = "0.20.28"`.

**Step 4 — the PR.** Push the branch and open the PR against `wloots-sys`'s
`develop`, following the repo's own history (PRs #3–#10 are the pattern):

```bash
cd /home/joseph/Projects/wlroots-sys-m3p1
git push -u origin feat/wlr-xdg-popup
gh pr create --base develop --title "wlr 0.20.28 — xdg-popup" --body "$(cat <<'EOF'
## Summary

xdg-popup for the `wlr` crate: popups on toplevels, on layer surfaces and on
other popups; positioner geometry and unconstraining; reposition; a scene
subtree parented under the popup's own parent; chain walking and deepest-first
dismissal; and the two grab observables a compositor needs.

Additive — every new `ToplevelHandler` method is defaulted, and
`tests/popups.rs` asserts that with an empty impl block written as a 0.20.27
consumer would write it.

## The three decisions worth reviewing

- **Parent-scoped listeners.** `wlr_xdg_shell.events.new_popup` is deliberately
  unused: a layer-shell popup has `parent == NULL` there, so the one case that
  needs classifying cannot be.
- **wlroots owns the grab.** No `start_grab`/`end_grab` call is added, no "focus
  the popup" call, and the implicit-pointer-grab code from 0.20.27 is untouched —
  `mod implicit_grab_tests` is byte-identical and green.
- **The scene subtree is not destroyed.** It is a child of the parent's tree and
  wlroots frees children recursively.

## Gate

`cargo test -p wlr`, the coverage audit, `cargo xtask coverage`, clippy
`-D warnings` under default and `--no-default-features`, `cargo doc -D warnings`,
`cargo fmt --all --check`, `cargo publish --dry-run` — all green.

Coverage: thirteen symbols waived → wrapped; `wlr_xdg_popup_grab` re-reasoned
`not-yet`/M9 → `internal` (no such function exists to wrap).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

**Step 5 — merging the PR is a second consent stop.** Present it as ready and
ask; do not merge.

---

## Self-Review

### Spec coverage

Every normative item in the contract's §1, and every P1 sentence in the spec's
§1/§2/§7, mapped to the task that implements it and the test that proves it.

| Source | Requirement | Task | Test |
|---|---|---|---|
| §1.1 | `PopupId`, derives, no `PartialOrd`/`Ord`, minted from the shared counter | 1 | `the_dangling_ids_are_distinct_and_out_of_the_issued_range` |
| §1.1 | `PopupParent::{Toplevel, Layer, Popup}` | 1 | `a_popup_parent_knows_whether_it_is_itself_a_popup` |
| §1.1 | `PopupParent::root` | 4 | `a_popup_parent_resolves_to_the_window_or_layer_at_the_root`, `a_root_walk_through_a_dead_link_is_none`, `a_root_walk_terminates_even_on_a_cyclic_table` |
| §1.1 | `PopupParent::is_popup` | 1 | `a_popup_parent_knows_whether_it_is_itself_a_popup` |
| §1.1 | `PositionerRules` — all nine fields, copied out | 2 | `the_rules_round_trip_through_the_c_representation`, `an_unsent_parent_size_and_serial_read_as_none` |
| §1.1 | `PositionerRules::geometry` | 2 | `the_geometry_anchors_and_gravitates_the_way_wlroots_does` |
| §1.1 | `PositionerRules::unconstrain_box` | 2 | `unconstraining_slides_a_popup_back_inside_when_sliding_is_permitted`, `unconstraining_with_no_adjustment_permitted_changes_nothing` |
| §1.1 | `PositionerAnchor`/`PositionerGravity` `#[non_exhaustive]`, unknown → `None`, never panics | 1, 2 | `the_anchor_values_are_the_ones_the_protocol_declares`, `a_positioner_with_nonsense_enum_values_never_panics` |
| §1.1 | `ConstraintAdjustment` six bits (**D1**: hand-rolled) | 1 | `the_constraint_bits_are_the_ones_the_protocol_declares`, `contains_is_every_bit_not_any_bit`, `bit_or_assign_accumulates` |
| §1.2 | `Popup<'h>` mirrors `Toplevel<'h>`; hand-written `Debug`, no raw pointer | 3 | `the_debug_impl_prints_named_fields_and_no_pointer` |
| §1.2 | `Popup::{id, parent}`; parent never re-derived | 3 | `a_handle_reports_the_parent_it_was_built_with_not_the_null_in_the_struct` |
| §1.2 | `Popup::{anchor_rect, positioner_rules, geometry, is_reactive}` | 3 | `geometry_reads_current_while_the_rules_read_what_was_scheduled` |
| §1.2 | `Popup::grab_requested` = `seat != NULL` | 3 | `grab_requested_reads_the_seat_pointer_wlroots_fills_on_the_grab_request` |
| §1.2 | `Popup::unconstrain`, root-toplevel space, sends nothing | 3 | covered via `Runtime::configure_popup` (5) and P2's placement tests |
| §1.2 | `Popup::{position, toplevel_coords}` | 3 | `configuring_an_unknown_popup_reports_false_rather_than_dereferencing` (5), P2 |
| §1.2 | `Popup::send_configure` returns 0 **and skips the call** when uninitialized | 3 | `send_configure_is_skipped_entirely_before_the_surface_is_initialized` (mutation: aborts) |
| §1.2 | `Popup::reposition_token` gated on the field bit | 3 | `the_reposition_token_is_none_until_wlroots_sets_its_field_bit` |
| §1.2 | `Popup::destroy` does not touch the scene tree | 3 | doc + Task 8's `on_popup_destroy`; P2's `destroying_a_parent_destroys_its_popup_chain_without_a_double_free` |
| §1.3 | Six defaulted `ToplevelHandler` methods, not a new trait, not a `Handlers` supertrait | 6 | `the_popup_handler_methods_are_additive_and_overridable`; `tests/compile_fail.rs` unchanged |
| §1.3 | `popup_destroyed`'s "id you were never told about" caveat, verbatim | 6 | doc review |
| §1.4 | Three parent-scoped `new_popup` link sites; shell-level unused | 8 | `a_toplevel_slot_names_a_toplevel_parent`, `a_layer_slot_names_a_layer_parent`, `a_popup_slot_names_a_nested_parent` |
| §1.4 | `Bound.popup`; `link` not widened; `link_popup` built directly | 8 | compiles; `no_slot_at_all_resolves_to_nothing_rather_than_guessing` |
| §1.4 | id minted on `base->surface->addons` | 8 | code + Task 9's role check |
| §1.4 | `toplevel_id_of_surface` gains a role check | 9 | `a_surface_that_is_a_popup_yields_no_toplevel_id` |
| §1.4 | `Session.popups` + `PopupListeners`, unlinked from `on_popup_destroy` | 8 | code; P2's chain-destroy test |
| §1.4 | id carried in `Bound`, never read from `data` | 8 | every callback takes `_data` |
| §1.5 | `PopupEntry {raw, tree, parent, configured}` | 4 | `forgetting_a_popup_removes_only_that_row` |
| §1.5 | subtree = `wlr_scene_xdg_surface_create(parent_tree, base)` | 8 | code; P2's stacking/hit tests |
| §1.5 | **no** second lock gate, **no** popup tree destroyed | 4, 8 | docs on `PopupEntry`/`forget_popup`/`on_popup_destroy`; diff review |
| §1.5 | `Runtime::{popup, popup_parent, popups_of, popup_chain}` | 4 | four `runtime::tests` cases |
| §1.5 | `Runtime::{configure_popup, popup_position, dismiss_popup, dismiss_popups_of, popup_is_grabbing, seat_has_explicit_grab}` | 5 | seven `runtime::tests` cases |
| §1.5 | every accessor releases the borrow before returning | 4, 5 | code + docs; `Runtime::popup`'s explicit scope block |
| §1.5 | `record/forget/entry/clear` quartet; `clear_popups` from `run_inner` | 4, 9 | `clear_popups_empties_the_table` |
| §1.6 | no `start_grab`/`end_grab`, no focus-the-popup call | all | `git diff` shows no seat-grab call added |
| §1.6 | implicit-grab code and tests untouched | 8 | `git diff … | grep -c implicit_grab` = 0 |
| §1.7 | six `Event` variants, ids only | 7 | compiles (`Copy + Eq` enum) |
| §1.7 | `deliver_all` arms via `with_popup` | 7 | `a_run_with_a_popup_handler_starts_and_stops_cleanly` |
| §1.7 | `deliver`'s unreachable arm — compile-required | 7 | mutation: removing a name breaks the build |
| §1.7 | `on_popup_commit` gates on `initial_commit`, emits, then configures | 8 | code; P2's mapping tests |
| §1.8 | no new layer id; `PopupParent::Layer(LayerSurfaceId)` | 1 | compiles |
| §1.9 | 13 rows waived → wrapped; `wlr_xdg_popup_grab` re-reasoned `internal`; 8 stay waived | 11 | `cargo test -p wlr --test coverage_audit` |
| §1.10 | README section; `lib.rs` seven-name re-export; `Cargo.toml` 0.20.28 | 12, 4, 13 | `cargo doc -D warnings`; `tests/popups.rs` compiles |
| §1.10 | publish is a consent stop; `--dry-run` first | 13, 15 | procedure |
| Spec §7 | untrusted input never panics | 2 | `a_positioner_with_nonsense_enum_values_never_panics`, `a_positioner_with_degenerate_sizes_never_panics` |
| Spec §7 | every load-bearing test records a mutation check | 1–11 | each task's "Mutation checks" step |
| §9 P1 gate | crate tests, audit, xtask, clippy ×2, doc, dry-run, fmt, implicit-grab green | 13 | Task 13's nine commands |

**Never-panic coverage of untrusted parsers.** P1 decodes exactly one class of
untrusted input: the client's positioner (anchor, gravity, constraint
adjustment, anchor rect, size, offset, parent size, parent configure serial).
Both never-panic tests are in Task 2 and between them cover the whole `u32`
range for the three enums and the `i32` extremes for the four geometry fields.
There is no keymap, `index.theme`, SVG or PNG decoding in this part — those are
P3 and P7.

**Gaps deliberately left to P2, and named here so nothing is silently uncovered.**
Placement against a real output; flipping and sliding at an output edge; a popup
under a layer panel; nested chains configured relative to their parents; grab
dismissal by a click outside; keyboard focus returning to the parent;
non-grabbing popups not moving focus; reactive reconfiguration on a parent move;
reposition echoing the token; input denial while the session is locked; and
destroying a parent taking its chain down without a double free. All twelve are
the exact test names the contract's §2.4 froze for
`compositor/tests/popups.rs`, and each is named in the mutation-check step of
the P1 task whose code it covers.

### Placeholder scan

Searched for `TBD`, `TODO`, `FIXME`, `XXX`, `similar to Task`, `as above (see
Task`, `…`-as-code, `unimplemented!`, `todo!`, and prose-only code steps.

- **Zero** `TBD`/`TODO`/`FIXME`/`XXX` in this document.
- **Zero** "similar to Task N" or "same as above" substitutions for code. Every
  code block is complete and pasteable. Where two functions are genuinely
  parallel (`PositionerAnchor::from_raw` / `PositionerGravity::from_raw`,
  `on_popup_map` / `on_popup_unmap`), both are written out in full.
- **Two** `<…>` placeholders, both deliberate and both resolved by an earlier
  step in the same document: `WLR_COVERAGE_BINDINGS=<the path recorded in Task
  0>` (Task 0 step 3 prints it) and the `git worktree` target path (fixed:
  `/home/joseph/Projects/wlroots-sys-m3p1`).
- **One** conditional instruction: Task 7's note that if `Until`/`run_all`'s
  spelling differs, copy the shape from `tests/toplevels.rs`. That is a
  fallback against a detail this plan could not verify without building, not a
  gap — the assertions are fully specified either way.
- **Every** type named in a signature is defined in this document or already
  exists in the crate at a cited line. No undefined types.
- **Every** commit step gives the exact `git add` paths and the exact message,
  trailer included.
- **Every** task has a real failing-test step with the **expected error text**,
  not merely "run the test".

### Type consistency vs the contract

| Contract §1 signature | This plan | Same? |
|---|---|---|
| `pub struct PopupId(pub(crate) u64)` | identical | yes |
| `pub enum PopupParent { Toplevel(ToplevelId), Layer(LayerSurfaceId), Popup(PopupId) }` | identical | yes |
| `PopupParent::root(self, rt: &Runtime) -> Option<PopupParent>` | identical | yes |
| `PopupParent::is_popup(self) -> bool` | identical | yes |
| `PositionerRules` — nine fields, exact names and types | identical, field order included | yes |
| `PositionerRules::geometry(&self) -> Box2D` | identical | yes |
| `PositionerRules::unconstrain_box(&self, constraint: &Box2D) -> Box2D` | identical | yes |
| `PositionerAnchor` / `PositionerGravity` — nine variants, `#[non_exhaustive]` | identical | yes |
| `ConstraintAdjustment` — six named bits, values 1/2/4/8/16/32 | same names and values, hand-rolled | **D1** |
| `Popup::id/parent/anchor_rect/positioner_rules/geometry/is_reactive/grab_requested` | identical | yes |
| `Popup::unconstrain(&self, constraint: &Box2D)` | identical | yes |
| `Popup::position(&self) -> (f64, f64)` | identical | yes |
| `Popup::toplevel_coords(&self, popup_sx: i32, popup_sy: i32) -> (i32, i32)` | identical | yes |
| `Popup::send_configure(&self) -> u32` | identical | yes |
| `Popup::reposition_token(&self) -> Option<u32>` | identical | yes |
| `Popup::destroy(&self)` | identical | yes |
| `ToplevelHandler::new_popup(&mut self, popup: &Popup<'_>)` | identical | yes |
| `ToplevelHandler::popup_initial_commit(&mut self, popup: &Popup<'_>)` | identical | yes |
| `ToplevelHandler::popup_mapped(&mut self, id: PopupId)` | identical | yes |
| `ToplevelHandler::popup_unmapped(&mut self, id: PopupId)` | identical | yes |
| `ToplevelHandler::popup_reposition(&mut self, popup: &Popup<'_>)` | identical | yes |
| `ToplevelHandler::popup_destroyed(&mut self, id: PopupId)` | identical | yes |
| `Bound { … popup: Option<PopupId> }` | identical | yes |
| `link_popup(signal, notify, session, popup) -> Registration` | identical (returns `Self`, which is `Registration`) | yes |
| `PopupListeners { _commit, _map, _unmap, _destroy, _reposition, _new_popup }` | identical | yes |
| `Session.popups: RefCell<HashMap<PopupId, PopupListeners>>` | identical | yes |
| `PopupEntry { raw, tree, parent, configured: Cell<bool> }` | identical | yes |
| `Runtime::popup(&self, id) -> Option<Popup<'_>>` | identical | yes |
| `Runtime::popup_parent(&self, id) -> Option<PopupParent>` | identical | yes |
| `Runtime::popups_of(&self, parent) -> Vec<PopupId>` | identical | yes |
| `Runtime::popup_chain(&self, parent) -> Vec<PopupId>` | identical | yes |
| `Runtime::configure_popup(&self, id, constraint: &Box2D) -> bool` | identical | yes |
| `Runtime::popup_position(&self, id) -> Option<(f64, f64)>` | identical | yes |
| `Runtime::dismiss_popup(&self, id) -> usize` | identical | yes |
| `Runtime::dismiss_popups_of(&self, parent) -> usize` | identical | yes |
| `Runtime::popup_is_grabbing(&self, id) -> bool` | identical | yes |
| `Runtime::seat_has_explicit_grab(&self) -> bool` | identical | yes |
| `Event::{NewPopup, PopupInitialCommit, PopupMapped, PopupUnmapped, PopupReposition, PopupDestroyed}(PopupId)` | identical | yes |
| `drop_all_popups` | named `clear_popups` (`pub(crate)`, mirrors `clear_toplevels`) | naming only, noted in Task 4 |
| `lib.rs` re-export of seven names | identical | yes |

**Additions beyond the contract** (all additive, none changing a frozen
signature): `PopupId::{dangling_for_test, dangling_nth_for_test}` (**D2**);
`ConstraintAdjustment::{NONE, bits, from_raw}`;
`PositionerRules::{from_c, to_c}` (`pub(crate)`);
`Runtime::{popup_raw, popup_tree, mark_popup_configured, MAX_POPUP_DEPTH}`
(`pub(crate)`); `Popup::from_raw_with_id`'s `parent` parameter (`pub(crate)`);
`popup_parent_from_slots` and `toplevel_id_if_not_a_popup` (private to
`backend.rs`).

**Downstream types P2 must build on, confirmed present:** `PopupId` (wrapped by
`PopupKey`), `PopupParent` (mapped to `PopupHost`/`PopupRoot`),
`Runtime::configure_popup` (called from `popup_constraint_box`'s result),
`Runtime::seat_has_explicit_grab` (the fourth early return in `sync_seat_focus`),
`Runtime::dismiss_popups_of` (non-grabbing chain teardown),
`Popup::grab_requested` (`PopupEntry.grabbing`), `Popup::is_reactive`
(`reconstrain_popups`), `Popup::reposition_token` (the token echo assertion),
and the six handler methods P2's `impl ToplevelHandler for State` overrides.

### Line-count and split check

Sixteen tasks (0–15). Measured, not estimated — each task's span runs from its
own `### Task` heading to the line before the next heading (Task 15 ends at the
line before `## Summary`), and "fenced" counts the lines inside ``` blocks
including the fence lines themselves:

| Task | Lines | Span | Fenced | Over ~400? |
|---|---|---|---|---|
| 0 — worktree and branch | 66 | 275–340 | 24 | no |
| 1 — `popup.rs`: ids, parents, positioner enums | 544 | 341–884 | 483 | **yes** |
| 2 — `PositionerRules` | 470 | 885–1354 | 415 | **yes** |
| 3 — `Popup<'h>` | 544 | 1355–1898 | 480 | **yes** |
| 4 — `Runtime`'s popup table and chain walkers | 574 | 1899–2472 | 486 | **yes** |
| 5 — configure, dismiss, the two grab observables | 318 | 2473–2790 | 268 | no |
| 6 — six defaulted `ToplevelHandler` methods | 223 | 2791–3013 | 174 | no |
| 7 — `dispatch::Event` | 249 | 3014–3262 | 177 | no |
| 8 — `backend.rs` listeners and callbacks | 733 | 3263–3995 | 610 | **yes** |
| 9 — mislabelling fix, run-granularity purge | 171 | 3996–4166 | 103 | no |
| 10 — `tests/popups.rs` | 187 | 4167–4353 | 145 | no |
| 11 — coverage ledger | 188 | 4354–4541 | 106 | no |
| 12 — README | 168 | 4542–4709 | 139 | no |
| 13 — release commit and full gate | 99 | 4710–4808 | 47 | no |
| 14 — contract amendments | 75 | 4809–4883 | 57 | no |
| 15 — publish (CONSENT STOP) | 48 | 4884–4931 | 12 | no |

Reproduce with:

```bash
awk '/^### Task /{if(n)print n": "NR-s" lines"; n=$0; s=NR}
     /^## Summary/{print n": "NR-s" lines"; exit}' \
  docs/superpowers/plans/2026-08-27-m3-part1-wlr-xdg-popup.md
```

So **five** tasks exceed the ~400-line sizing guideline: 1, 2, 3, 4 and 8, with
Task 8 the largest at 733. An earlier draft of this section claimed Task 8 was
"~380 lines… under the ~400-line split threshold" and Tasks 1 and 3 were "~340
and ~360"; those numbers were estimates that were never measured and all three
were wrong by roughly 2×. The measured table above replaces them. What follows
is why each over-guideline task is nevertheless left whole, argued per task
rather than waved at collectively.

Two things are true of every one of the five. First, 83–89% of each is fenced
code, not prose — a consequence of the placeholder rule above (every code block
complete and pasteable, no "same as Task N" substitutions), so plan length here
tracks the size of the code being written rather than the amount of reading an
executor must do to decide anything. Second, each is a single TDD cycle —
failing test, watch it fail, implement, watch it pass, one commit — and cutting
one in half produces either a commit whose tests do not compile or a commit that
trips `-D warnings`, both of which are worse for an executor than a long task.

- **Task 1 (544).** One new file's worth of types and their tests. Splitting the
  ids from the positioner enums leaves `PositionerAnchor::from_raw`,
  `PositionerGravity::from_raw` and `ConstraintAdjustment::from_raw` — all
  `pub(crate)`, none reachable from the id types — as dead code in the first
  half, which `-D warnings` rejects.
- **Task 2 (470).** One struct, four methods, and the never-panic suite the
  Global Constraints require for client-supplied enum values. The
  `from_c`/`to_c` round trip is what makes the never-panic tests constructible
  at all, so the parser and its tests cannot land in separate commits.
- **Task 3 (544).** Thirteen methods on one handle over one raw pointer. A split
  into "constructor plus readers" and "actions" is the only clean cut available
  and it is not worth taking: both halves need the same leaked-`alloc_zeroed`
  `wlr_xdg_popup` fixture, so the fixture and its safety comment would have to
  be duplicated or moved in a third commit, and the mutation checks
  (`send_configure` skipping the call before initialization, in particular) span
  both halves.
- **Task 4 (574).** `record_popup`/`forget_popup`/`popup_raw`/`popup_tree` are
  `pub(crate)` and have no in-crate caller until Task 8; within Task 4 they are
  kept alive only by the public walkers and the tests that sit in the same
  commit. Landing the table without the walkers is a dead-code commit.
- **Task 8 (733).** As the task's own opening paragraph states: `on_new_popup`
  links the five per-popup callbacks, the callbacks read `Bound::popup`, and
  `link_popup` is dead until `on_new_popup` calls it. The three link sites are
  the only things that make `on_new_popup` reachable. Nothing here compiles
  apart.

The guideline exists to stop a task an executor cannot hold at once, and the
better proxy for that is the number of independent decisions a task carries.
By that measure Task 8 is the one to watch — six callbacks, three link sites,
six `deliver_all` arms — which is why its parent-resolution rule is stated once
in a table up front rather than re-derived at each of the three link sites.
