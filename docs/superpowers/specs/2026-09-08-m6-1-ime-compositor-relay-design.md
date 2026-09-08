# Pure-Rust GTK UI rebuild — Milestone 6.1 (IME), Spec 1: the compositor-side text-input relay

**Status:** design approved in brainstorm on 2026-09-08; awaiting written-spec review, then `writing-plans`.
**Parent:** `2026-08-20-pure-rust-gtk-ui-design.md` (program spec; M6 is item 6 of its decomposition — polish). M6 was decomposed into sub-milestones; M6.0 (deferred follow-ups) and M6-FUP1 (probe settle) are done and merged. M6.1 = input methods.
**Predecessor:** M6.0/M6-FUP1, both merged to develop @ `2955997`.
**Adopts:** `2026-08-20-icedtea-input-methods-design.md` (roadmap **A6**) as the authoritative compositor-side design, refreshed here (see §Refresh). A6 is implementation-ready — 9 decisions of record, exact `wlr`-crate additions, chokepoint hooks, and an automated harness test plan. This spec does **not** restate A6; it frames A6 as M6.1 Spec 1, records the deltas since A6 was written, and pins the success criteria that tie it to Spec 2.

## Why (the regression this closes)

M5's P6-D8: both migrated apps lost input methods when they left GTK. Settings' wallpaper-path, workspace-name and keybinding fields and the shell clipboard popover's search field accept direct key events only — no CJK/compose/dead-key composition, no on-screen-keyboard. Restoring IME is end-to-end and cross-repo: a **client** (the toolkit) speaks `zwp_text_input_v3` to the **compositor**, which **relays** to an **input-method** client (`zwp_input_method_v2` — an external IME such as fcitx5/ibus, or an OSK such as squeekboard). icedtea ships the relay and the client; it ships **no** IME/OSK of its own.

## M6.1 decomposition (two specs)

1. **Spec 1 — the compositor-side relay (this document).** The `wlr` crate implements the `text-input-v3` ↔ `input-method-v2` relay; the compositor creates the two globals and places IME candidate popups. Shipped as one additive `wlr` release + a compositor bump, exactly the M4.5 shape. **This is A6.**
2. **Spec 2 — the toolkit `text-input-v3` client (separate spec, after Spec 1 merges).** icedtea-ui's `Entry`/text-editing widgets speak the client side (enable-on-keyboard-focus, surrounding-text/content-type/cursor-rect, apply `preedit_string`/`commit_string`/`delete_surrounding_text`); then settings' three fields and the shell search field are wired, closing P6-D8. Built and tested against Spec 1's relay through the harness.

Spec 1 first: the relay must exist for the client to talk to and for the harness to test end-to-end.

## What Spec 1 is (adopted from A6, in brief)

The exhaustive design — every `RuntimeInner` field, listener, chokepoint hook, and per-signal handler — is A6 §"Architecture" and §"Decisions of record"; implementation follows it. In summary:

- **`wlr` crate (one additive release).** wlroots 0.20 ships **no** relay helper (`wlr_input_method_relay` was removed; sway/river each carry their own), so this is genuine cross-object routing — the first such since M4. The relay hangs off the crate's existing seat and keyboard-focus chokepoints (A6 decision #2, the M4.5 precedent). It adds: the two managers + `create_*_manager` fns; `text_inputs`/`input_method`/`input_method_popups` tracking (destroy keyed on the destroy-listener address — the M4.4 lesson); enter/leave driven from the **keyboard**-focus chokepoint (not pointer — decision #4); state-diffed enable→activate→surrounding-text and IME-commit→app forwarding off wlroots' post-diff `current` state (decision #5); at-most-one input-method per seat, second gets `unavailable` (decision #3); keyboard-grab forwarding as a branch in the existing `on_key`/`on_modifiers` chokepoint (decision #6); and the raw `wlr_input_popup_surface_v2` object + its rectangle send (decision #7).
- **compositor.** Create the two globals at boot, non-fatal, like every other protocol in `lib.rs`. Place the IME candidate **popup surfaces** in the scene graph — the one piece not in the crate, because `wlr_input_popup_surface_v2` has no `xdg_positioner`/auto-placement: anchor below the focused text-input's `cursor_rectangle`, clamp to the output, the way `render.rs` already places compositor-managed scene nodes (decision #7). Expose a `DbCommand::InputMethodActive` (and a popup-position accessor) as the harness oracle.

**Scope boundary:** `text-input-v3` relay + `input-method-v2` relay + candidate **popup surfaces** + **keyboard-grab** forwarding. `tablet-v2` is **out** (unrelated stylus protocol, a later self-contained follow-on — A6 decision #1). No IME/OSK application is shipped (A6 §Out of scope).

## Sub-parts (A6's own decomposition)

- **A6.1 — core relay:** globals, enter/leave, enable→activate→surrounding-text, IME-commit→app, disable/leave→deactivate, second-IME-refused. Harness tests 1–7.
- **A6.2 — popups + keyboard-grab (after A6.1):** popup placement + `text_input_rectangle`, keyboard-grab intercept. Harness tests 8–9+.

Whether A6.1 and A6.2 are one PR or two is a `writing-plans` call; both are within Spec 1.

## Testing (automated-only)

Per A6 §Testing, extend `harness` + `compositor/tests/` with two test-double clients — `TextInputClient` (binds `zwp_text_input_manager_v3`, maps a surface for keyboard focus, enable/commit/disable, records enter/leave/preedit/commit/delete/done) and `InputMethodClient` (binds `zwp_input_method_manager_v2`, records activate/deactivate/surrounding/content/done/unavailable, sends preedit/commit/delete + commit, can grab the keyboard and create a popup) — following the `PointerConstraintsClient`/`VirtualPointerClient` pattern (M4.5/M4.2). The nine A6 tests assert on **both sides** (client event capture **and** a `DbCommand::InputMethodActive`/popup-position oracle), the "assert on both sides" discipline from M4.4's `SessionLocked`. Since no real IME is shipped, these test-doubles are the only way to drive the relay end-to-end — and they double as the fixtures Spec 2's client tests reuse.

## Success criteria

1. `zwp_text_input_manager_v3` and `zwp_input_method_manager_v2` are advertised globals; the compositor boots non-fatally without an IME bound.
2. The A6.1 + A6.2 harness tests pass (enter/leave follows keyboard focus; enable→activate→surrounding relay; IME commit→app relay; disable/leave→deactivate; second-IME refused; popup positioned + told its rectangle; keyboard-grab intercepts).
3. **Unblocks Spec 2:** a client that binds `text-input-v3` (the toolkit) can enable on focus and receive `commit_string` from a bound input-method — i.e. Spec 2's client has a working relay to target. This is the milestone-level acceptance: M6.1 is "done" only when Spec 2 also lands and settings/shell fields compose text end-to-end against a real IME.

## Refresh (deltas since A6 was written, 2026-08-20)

A6's design is version-independent; the following are the only updates for M6.1:

1. **`wlr` release version.** A6 targets `0.20.23`; the current published crate is `0.20.29`. Spec 1 ships the next available additive release (**`0.20.30`** unless a parallel bump takes it). The compositor's pin (`compositor/Cargo.toml`, `wlr = "0.20.29"`) bumps at the end; `[patch.crates-io]` to the wloots-sys worktree during development.
2. **Repo naming.** A6 says "icedtea-wm"; the repo is now **icedtea** (`org.icedtea.Compositor`). Paths are `compositor/src/…`.
3. **Line numbers are as-of-A6 and MUST be re-verified.** A6 cites exact `runtime.rs`/`backend.rs` line numbers in the `wlr` crate. That source (wloots-sys — the smithay→wlr port) is **not** checked out in this worktree; the compositor consumes the *published* crate. `writing-plans` and implementation re-locate the current chokepoints by role — `Runtime::focus_toplevel_keyboard`/`clear_keyboard_focus` (the keyboard-focus chokepoint), the `on_key`/`on_modifiers` chokepoint, and the pointer-constraints-manager field/`create_*_manager` neighbours — against the live wloots-sys `develop`, not by A6's stale line numbers.
4. **Compositor is confirmed to have none of this today** (`grep text_input|input_method compositor/src` is empty), so Spec 1 is purely additive.

## Workflow (standing, unchanged)

git-flow; `wlr` work in the wloots-sys repo as its own additive release with an **API-review verdict before `cargo publish`** (a consent stop) and a `[patch]` in this repo until then; the compositor bump + globals + popup placement in this repo on `feature/pure-rust-gtk-m6-1-compositor`; SDD with per-task Opus review, a whole-branch review, and a **merge window** before any `git flow … finish` (which uses `--keepremote` → push develop → delete the remote branch so the PR reads Merged). The wloots-sys repo is shared with parallel sessions — coordinate via SendMessage before pushing to its `develop` (the M4.5 precedent).
