# M8 IME consumer (icedtea) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consume the wlr 0.20.32-dev IME depth API (snapshots, reposition event, size accessor) to fix place-once staleness, render a preedit overlay, and show an IME indicator — proven by harness e2e before the wlr publish.

**Architecture:** `State` re-reads caret+size in the new reposition arm (same translated-anchor code path as creation); overlay is a scene text node driven by IME commit events reusing `text.rs` shaping; indicator flows through the compositor snapshot to the shell panel. Develops against the wlr worktree via workspace `[patch]`, dropped at the end.

**Tech Stack:** Rust (edition 2024, rust 1.94), wlr 0.20.32-dev via `[patch]`, Wayland protocols (harness doubles), cosmic-text shaping via `compositor/src/text.rs`.

**Spec:** `docs/superpowers/specs/2026-09-11-m8-ime-depth-design.md` (§6–§7). Proves the wlr plan (`wlroots-sys` `docs/superpowers/plans/2026-09-11-m8-ime-wlr-depth.md`); its e2e green is the R1 publish gate. B6–B9 (A6.2) is the precedent for every pattern below.

## Global Constraints

- Edition 2024, `rust-version = "1.94"` in the workspace manifest.
- Develop against `[patch.crates-io] wlr = { path = "<wlr-m8-ime worktree>/crates/wlr" }` at the workspace root `Cargo.toml` (links-invariant; mirrors B6). Drop it at the end and pin published `0.20.32`.
- Surface-local rects are translated through the focused content origin for placement; the `text_input_rectangle` echo stays surface-local (A6.2 review lesson).
- Test-only oracles mirror `CursorPosition`: `DbCommand` variant → `state.rs` arm → harness accessor; never exposed via `CompositorInterface`.
- Gates per task: `cargo test --workspace -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- TDD with bite-sized commits; scoped review per task; whole-branch review before merge (A6.2 precedent).

---

## File structure

- `compositor/src/state.rs` — owns: `popup_repositioned` arm (re-read caret+size, `set_node_position`, re-echo surface-local rect); overlay show/hide/update calls; `input_popup_nodes` maintenance (unchanged shape).
- `compositor/src/ime_overlay.rs` (new) — owns: pure overlay geometry + text layout decisions (position for caret+preedit text, cursor span split, hide conditions); unit tests in-module (mirrors `input_method.rs` discipline).
- `compositor/src/text.rs` — owns (existing): `rasterize_title`, `rasterize_glyph` reused for overlay glyphs; no changes expected, extend only if shaping needs it.
- `compositor/src/dbus.rs` + `harness/src/lib.rs` — owns: any new test-only oracle triplets needed by e2e (follow the exact `InputPopupNode` precedent if the tests need node/overlay visibility).
- `contract/src/types.rs` — owns: `Snapshot` indicator fields (additive-optional or version bump — decided in Task M8-7).
- `shell/src/panel.rs` — owns: indicator rendering from the snapshot.
- `harness/src/lib.rs` — owns: double recording for preedit/commit/delete payloads + serials (extend `InputMethodClient`/`TextInputClient` event vectors + accessors in the existing record-everything style).
- `compositor/tests/client_protocol.rs` — owns: e2e tests M8-8..M8-11 below.

---

### Task M8-5: Reposition wiring + size (fixes place-once)

**Files:**
- Modify: `compositor/src/state.rs` (new `popup_repositioned` arm in the existing `impl wlr::SeatHandler for State`, next to `new_popup_surface`)
- Test: `compositor/tests/client_protocol.rs` (new e2e)

**Interfaces:**
- Consumes: `SeatHandler::popup_repositioned` (defaulted, wlr plan Task M8-3); `focused_text_input_cursor_rectangle`, `input_popup_size`, `set_node_position`, `send_input_popup_rectangle` (existing); `place_below_clamped` (existing); `input_popup_nodes` record (existing)
- Produces: live re-placement on caret commit (consumed by M8-6 overlay positioning)

- [ ] **Step 1: Failing e2e — reposition moves the node**

```rust
#[test]
fn ime_popup_repositions_when_the_caret_moves() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(ti.wait_until(|c| c.entered() >= 1), "text-input never focused");
    ti.enable();
    ti.commit_with("q", 1, 1, (100, 200, 2, 16));
    assert!(im.wait_until(|s| s.activates() >= 1), "IME never activated");
    im.create_popup();
    let before = comp
        .input_popup_position()
        .expect("compositor never placed the popup");
    // Move the caret with a second commit carrying a new rectangle.
    ti.commit_with("qw", 2, 2, (300, 400, 2, 16));
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut after = None;
    while std::time::Instant::now() < deadline {
        im.pump();
        if let Some(pos) = comp.input_popup_position() {
            if pos != before {
                after = Some(pos);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let after = after.expect("popup never repositioned after the caret commit");
    assert_eq!(
        after,
        (300, 444),
        "repositioned translated below the new caret: content origin (0, 28) + (300, 400+16)"
    );
}
```

- [ ] **Step 2: Run it, watch it fail**

Run: `cargo test -p icedtea-compositor --test client_protocol ime_popup_repositions_when_the_caret_moves`
Expected: FAIL — no `popup_repositioned` arm exists, position stays `before`, deadline expires. (If it fails earlier — e.g. `before` is `None` — the wlr worktree API drifted; stop and reconcile with the wlr plan's Produces before writing compositor code.)

- [ ] **Step 3: Implement the reposition arm**

In the existing `impl wlr::SeatHandler for State`, beside `new_popup_surface`, add:

```rust
fn popup_repositioned(&mut self, popup: wlr::InputPopupSurfaceId) {
    let Some(rt) = self.wayland.runtime() else {
        return;
    };
    // Same translated-anchor computation as creation: factor the
    // anchor+output computation both arms share rather than duplicating
    // it (a private `fn popup_anchor(&self) -> (Rectangle, Option<Rectangle>)`
    // on State, called from both arms).
    let (anchor, output) = self.popup_anchor();
    let size = rt.input_popup_size(popup).unwrap_or((0, 0));
    let popup_rect = icedtea_contract::Rectangle {
        x: 0, y: 0, width: size.0, height: size.1,
    };
    let (x, y) = crate::input_method::place_below_clamped(anchor, popup_rect, output);
    let Some(node) = self.input_popup_nodes.get(&popup).copied() else {
        return;
    };
    if rt.set_node_position(node, x, y).is_none() {
        return;
    }
    let echo = /* surface-local caret rect, same as the creation arm */;
    rt.send_input_popup_rectangle(
        popup,
        wlr::Box2D::new(echo.x, echo.y, echo.width, echo.height),
    );
}
```

Rules: echo surface-local (never the translated anchor); record untouched (node already tracked); silent `None` paths mirror the creation arm.

- [ ] **Step 4: Run the e2e**

Run: `cargo test -p icedtea-compositor --test client_protocol ime_popup_repositions_when_the_caret_moves`
Expected: PASS. Then: `cargo test -p icedtea-compositor --test client_protocol` (full binary, currently 66 tests) — Expected: all PASS (creation path untouched).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets -- -D warnings`; `cargo fmt --all --check`. Expected: clean.

```bash
git add compositor/src/state.rs compositor/tests/client_protocol.rs
git commit -m "feat(compositor): re-place IME popups on caret commit (M8)"
```

### Task M8-6: Preedit overlay

**Files:**
- Create: `compositor/src/ime_overlay.rs` (pure geometry + layout: caret anchor in, overlay rect + glyph runs out; unit tests in-module)
- Modify: `compositor/src/lib.rs` (`pub mod ime_overlay;` beside `pub mod input_method;`); `compositor/src/state.rs` (IME-commit hook: show/update/hide a scene text node via `text.rs` shaping)
- Test: `compositor/tests/client_protocol.rs` (overlay e2e via a harness-visible oracle — add an `InputPopupOverlay`-style triplet ONLY if the test cannot observe the overlay otherwise; prefer reusing `scene_node_position`-style oracles)

**Interfaces:**
- Consumes: `CommittedImeState` (preedit text + cursor span; wlr plan Task M8-1); translated caret (Task M8-5's `popup_anchor`); `rasterize_glyph`/`rasterize_title` (`text.rs`, existing)
- Produces: overlay scene node lifecycle (consumed by nothing further; observed by e2e)

- [ ] **Step 1: Failing unit tests — overlay layout**

In `compositor/src/ime_overlay.rs`, write `pub fn layout_overlay(caret: Rectangle, text: &str, cursor: (u32, u32), output: Option<Rectangle>) -> OverlayLayout` (new struct: `rect: Rectangle`, `cursor_x: i32`) with tests: empty preedit → hidden flag; short text below caret; spill flips above (reuse `place_below_clamped` inside — do NOT reimplement clamping); cursor span within text bounds.

- [ ] **Step 2: Run, watch fail** — `cargo test -p icedtea-compositor --lib ime_overlay` → FAIL, no module.
- [ ] **Step 3: Minimal implementation** — pure function + struct; clamp via `place_below_clamped` with the measured text extent as the popup size.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Failing e2e — overlay shows preedit then clears**

```rust
#[test]
fn preedit_overlay_shows_composing_text_then_clears_on_commit() {
    // ... spawn, enable, commit_with caret, activate (test 8 preamble) ...
    // Drive the IME double to emit a preedit_string "nihon" commit; assert
    // the overlay oracle reports visible text "nihon"; drive commit_string;
    // assert hidden.
}
```

(Exact double-driving calls depend on the harness preedit recording added in this task's Step 6 — write the double support first if it does not exist: `InputMethodClient::commit_preedit(text)` sending the v2 preedit request, recorded vectors + accessors in the existing style.)

- [ ] **Step 6: Implement** — state.rs IME-commit hook: on preedit payload → create/update scene text node at `layout_overlay` position; on commit-string/deactivate → destroy node. Node tracked in a dedicated `Option<NodeId>` field (NOT `input_popup_nodes` — different lifecycle), destroyed via `destroy_node`.
- [ ] **Step 7: Gates + commit** — full `client_protocol` binary green; clippy; fmt; commit `feat(compositor): preedit overlay for composing text (M8)`.

### Task M8-7: IME indicator (snapshot + shell)

**Files:**
- Modify: `contract/src/types.rs` (`Snapshot`: additive `pub ime_active: bool`, `pub ime_name: Option<String>` — additive-optional so `COMPOSITOR_CONTRACT_VERSION` stays 2; if the shell needs more, bump with shell lockstep instead — decided here, documented in the commit message); `compositor/src/state.rs` (fill the fields on snapshot build); `shell/src/panel.rs` (render: active IME name or hidden when inactive)
- Test: `compositor/tests/client_protocol.rs` (snapshot asserts); shell panel probe test beside the existing panel tests if the repo has a snapshot-driven panel test harness (mirror it; do not invent a new harness)

**Interfaces:**
- Consumes: IME active state (existing activation tracking); overlay-independent
- Produces: user-visible indicator (terminal deliverable of this plan)

- [ ] **Step 1: Failing e2e — snapshot carries IME state**

```rust
#[test]
fn snapshot_reports_ime_activation() {
    // ... test 8 preamble through activate ...
    let snap = comp.snapshot();
    assert!(snap.ime_active, "snapshot must report the active IME");
    // ... destroy the text-input (8b preamble) → deactivate ...
    let snap = comp.snapshot();
    assert!(!snap.ime_active, "snapshot must clear on deactivate");
}
```

- [ ] **Step 2: Run, watch fail** — `Snapshot` has no such fields.
- [ ] **Step 3: Minimal implementation** — contract fields + state fill + panel render (hidden when inactive; name when active).
- [ ] **Step 4: Run** — Expected: PASS; full workspace suite for the contract change (`cargo test --workspace -- --test-threads=1` — snapshot flows to shell/settings tests).
- [ ] **Step 5: Gates + commit** — clippy, fmt; commit `feat(shell): IME indicator in panel (M8)`.

### Task M8-8: Serial pairing e2e + B-FINAL(M8)

**Files:**
- Modify: `compositor/tests/client_protocol.rs` (serial test); workspace `Cargo.toml` (drop `[patch]`); `compositor/Cargo.toml`, `harness/Cargo.toml` (pin `0.20.32`); `Cargo.lock` (regenerated)
- Test: the serial e2e below is the R1 gate for the wlr publish

**Interfaces:**
- Consumes: `CommitSerial` pairing (wlr plan Task M8-2); registry wlr 0.20.32 (published after this task goes green)

- [ ] **Step 1: Failing e2e — serial pairing**

```rust
#[test]
fn ime_commit_serial_reaches_text_input_done() {
    // ... activate ...
    // Drive an IME commit carrying serial N (harness records the serial
    // from the v2 commit event); read the text-input done serial the app
    // double observes; assert equal. Requires the harness doubles to record
    // both serials — extend the event vectors first if absent.
}
```

- [ ] **Step 2: Run, watch fail** — serials unrecorded or unpaired.
- [ ] **Step 3: Implement** — harness recording only (compositor forwards already via tokens); assert pairing.
- [ ] **Step 4: Full gate** — `cargo test --workspace -- --test-threads=1` (0 failures), clippy, fmt. This green run is the R1 publish gate for wlr 0.20.32 — report it, do not publish (consent stop).
- [ ] **Step 5: B-FINAL(M8)** — after published 0.20.32: drop `[patch]`, pin `wlr = "0.20.32"` in both manifests, `cargo update -p wlr`, verify `Cargo.lock` carries registry source + checksum, re-run the full gate, commit `build(m6.1): consume published wlr 0.20.32, drop dev [patch] (M8)`.
