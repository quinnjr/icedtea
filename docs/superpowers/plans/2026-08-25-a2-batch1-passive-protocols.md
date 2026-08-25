# A2 Batch 1 — Passive / Scene-Internal Protocols — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the six passive/scene-internal Wayland compatibility globals — viewporter, fractional-scale, single-pixel-buffer, content-type, presentation-time, and xdg-output — so apps get crisp HiDPI, correct presentation feedback, and logical output geometry, by publishing one additive `wlr` release and wiring the globals at compositor boot.

**Architecture:** Each global follows the crate's proven `Runtime::create_<x>_manager(&Display) -> Result<()>` pattern (double-create guard → `wlr_<x>_create` → store `NonNull` in a `RuntimeInner` cell). wlroots' `wlr_scene` does the per-surface transform/scale/crop work internally, so five of the six need only global creation; presentation-time additionally needs one `wlr_scene_set_presentation` wiring call, and xdg-output needs the output layout. The compositor creates each as a non-fatal boot line beside the existing `create_*` calls. No `wlr-sys` change — every C symbol is already bound.

**Tech Stack:** Rust 2024; `wlr` safe wrapper over wlroots 0.20 (repo `wloots-sys`); wlroots scene graph; `zbus`-free (pure Wayland); harness clients bind from `wayland-protocols` (already a dep).

**Spec:** `docs/superpowers/specs/2026-08-20-icedtea-compat-protocols-design.md` (milestone **M-A2.1**), roadmap `docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md` (Track A, item A2). This plan implements Batch 1 only; Batch 2 (cursor-shape, xdg-activation, gamma-control) and Batch 3 (foreign-toplevel) are separate future plans, each its own additive publish.

## Global Constraints

- **Version allocated at publish time — do NOT hard-code.** The spec's "wlr 0.20.23" for Batch 1 is stale (0.20.23 shipped XWayland). Use the next free `0.20.x`; **0.20.24** is the expected number, but confirm it is unclaimed on crates.io and coordinate with any parallel wlroots-sys sessions (`SendMessage`) immediately before publishing.
- **Every `cargo publish` and every branch merge is a hard-stop for owner consent** (project rule). Present ready → wait for explicit go. Never auto-publish, auto-merge, or auto-push.
- **Additive-only within `0.20.x`** — no existing `wlr` item's name/signature changes (the semver rule the port has held since M1).
- **crates.io auth** is via Cargo's libsecret provider — run `cargo publish` directly; never pass `--token`.
- **Two repos, two branches:** wlroots-sys work on `feature/wlr-a2.1` off `develop`; icedtea work on `feature/a2-batch1` off `develop`. During compositor development, a dev `[patch.crates-io] wlr = { path = … }` may point at the local wlr worktree; it MUST be dropped (and the version pin bumped to the published crate) before the compositor branch merges — exactly as the XWayland M5 did.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **wlr gates (wloots-sys, on the freeze commit):** `cargo test -p wlr` + `cargo clippy --all-targets -- -D warnings` + `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps` + `cargo fmt --all --check` + the coverage audit (`cargo test -p wlr --test coverage_audit`).
- **icedtea gates:** `cargo test -p icedtea-compositor` + `cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets`.

## Spike results (M-A2.0 — already run, recorded here)

- Compositor boot list (`compositor/src/lib.rs:72–138`) creates none of the six; `init_graphics`' only globals are `wlr_compositor_create`, `wlr_subcompositor_create`, `wlr_data_device_manager_create`.
- The `wlr` crate wraps none of the six yet.
- `wlr-sys` **already binds** every needed C symbol (`wlr_viewporter_create`, `wlr_fractional_scale_manager_v1_create`, `wlr_fractional_scale_v1_notify_scale`, `wlr_single_pixel_buffer_manager_v1_create`, `wlr_content_type_manager_v1_create`, `wlr_presentation_create`, `wlr_scene_set_presentation`, `wlr_xdg_output_manager_v1_create`) → **no wlr-sys change**.
- **xdg-output and presentation-time are absent** → both are in scope for Batch 1.
- **fractional-scale auto-send** (whether `wlr_scene` calls `notify_scale` on output-enter) is unconfirmed from source; Task 2 ships the notify wrapper as an additive fallback and Task 8's load-bearing test settles it empirically.

## File Structure

**wloots-sys (`feature/wlr-a2.1` off `develop`):**
- Modify `crates/wlr/src/runtime.rs` — six `create_*` methods + `notify_fractional_scale` + `set_scene_presentation`; matching `RuntimeInner` cells.
- Modify `crates/wlr/src/lib.rs` (or wherever `RuntimeInner` is defined) — the new `RefCell<Option<NonNull<…>>>` fields, defaulted to `None`.
- Modify `crates/wlr/coverage/{waived,wrapped}.toml` — move the now-used sys symbols waived→wrapped.
- Modify `crates/wlr/README.md` — changelog section; `crates/wlr/Cargo.toml` — version bump.

**icedtea (`feature/a2-batch1` off `develop`):**
- Modify `compositor/src/lib.rs` — six boot lines + one `set_scene_presentation` wiring call.
- Modify `compositor/Cargo.toml`, `harness/Cargo.toml` — wlr version pin.
- Modify `harness/src/lib.rs` — bind each new global; expose advertised assertions.
- Create `compositor/tests/compat_protocols.rs` — advertised-globals + fractional-scale/viewporter + presentation-feedback tests (mirrors `compositor/tests/*` screencopy/pointer-constraint client pattern).

---

### Task 1: wlr — the four pure create-global wrappers

**Files:**
- Modify: `crates/wlr/src/runtime.rs` (add four methods near the other `create_*_manager`, ~line 5305)
- Modify: `crates/wlr/src/lib.rs` (RuntimeInner: add four `RefCell<Option<NonNull<_>>>` cells, all `None` in the constructor)
- Test: `crates/wlr/src/runtime.rs` unit tests module (double-create returns `Error::Operation`)

**Interfaces:**
- Produces: `Runtime::create_viewporter(&Display) -> Result<()>`, `Runtime::create_single_pixel_buffer_manager(&Display) -> Result<()>`, `Runtime::create_content_type_manager(&Display) -> Result<()>`, `Runtime::create_xdg_output_manager(&Display) -> Result<()>`.
- Consumes: the existing `RuntimeInner` cell pattern and `init_graphics`' output layout (for xdg-output).

- [ ] **Step 1: Add the four RuntimeInner cells.** In the `RuntimeInner` struct add, defaulted to `RefCell::new(None)` in its constructor:

```rust
viewporter: RefCell<Option<NonNull<sys::wlr_viewporter>>>,
single_pixel_buffer_manager: RefCell<Option<NonNull<sys::wlr_single_pixel_buffer_manager_v1>>>,
content_type_manager: RefCell<Option<NonNull<sys::wlr_content_type_manager_v1>>>,
xdg_output_manager: RefCell<Option<NonNull<sys::wlr_xdg_output_manager_v1>>>,
```

- [ ] **Step 2: Write the three simple wrappers** (mirror `create_relative_pointer_manager`, runtime.rs:5292). `version` args match wlroots' current header (viewporter/single-pixel take only `display`; content-type takes `display, version` — use the manager's max supported version constant `1`):

```rust
/// Create the `wp_viewporter` global. Clients crop/scale their buffers via a
/// viewport; wlroots' scene applies it at render time, so no handler is needed.
/// Errors if called twice.
pub fn create_viewporter(&self, display: &Display) -> Result<()> {
    if self.inner.viewporter.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_viewporter called twice"));
    }
    // SAFETY: display live for the call; the viewporter is display-owned and
    // freed with it, so this crate never frees it.
    let raw = unsafe { sys::wlr_viewporter_create(display.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_viewporter_create"))?;
    *self.inner.viewporter.borrow_mut() = Some(raw);
    Ok(())
}

/// Create the `wp_single_pixel_buffer_manager_v1` global (cheap solid-color
/// buffers). The renderer consumes the buffer type automatically. Errors twice.
pub fn create_single_pixel_buffer_manager(&self, display: &Display) -> Result<()> {
    if self.inner.single_pixel_buffer_manager.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_single_pixel_buffer_manager called twice"));
    }
    let raw = unsafe { sys::wlr_single_pixel_buffer_manager_v1_create(display.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_single_pixel_buffer_manager_v1_create"))?;
    *self.inner.single_pixel_buffer_manager.borrow_mut() = Some(raw);
    Ok(())
}

/// Create the `wp_content_type_manager_v1` global. Content-type is surface
/// metadata wlroots attaches; no handler required now. Errors twice.
pub fn create_content_type_manager(&self, display: &Display) -> Result<()> {
    if self.inner.content_type_manager.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_content_type_manager called twice"));
    }
    let raw = unsafe { sys::wlr_content_type_manager_v1_create(display.as_ptr(), 1) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_content_type_manager_v1_create"))?;
    *self.inner.content_type_manager.borrow_mut() = Some(raw);
    Ok(())
}
```

- [ ] **Step 3: Write the xdg-output wrapper.** It needs the output layout, which `init_graphics` owns in `self.inner.graphics`. Read the layout pointer from there (mirror however other post-`init_graphics` methods reach the layout — e.g. `create_output_manager`, runtime.rs:4997, which also needs the layout); error if graphics is not yet initialized:

```rust
/// Create the `zxdg_output_manager_v1` global (read-only per-output logical
/// geometry many panels and toolkits read). Distinct from the wlr output-
/// *management* protocol. Needs the scene's output layout, so call after
/// `init_graphics`. Errors if called twice or before graphics init.
pub fn create_xdg_output_manager(&self, display: &Display) -> Result<()> {
    if self.inner.xdg_output_manager.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_xdg_output_manager called twice"));
    }
    let layout = self.output_layout_ptr()            // existing accessor used by create_output_manager
        .ok_or(Error::Operation("create_xdg_output_manager before init_graphics"))?;
    let raw = unsafe { sys::wlr_xdg_output_manager_v1_create(display.as_ptr(), layout.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_xdg_output_manager_v1_create"))?;
    *self.inner.xdg_output_manager.borrow_mut() = Some(raw);
    Ok(())
}
```

(If `create_output_manager` reaches the layout by a different private helper name, reuse that exact name here rather than inventing `output_layout_ptr`.)

- [ ] **Step 4: Write the failing double-create tests** — one per wrapper, following the existing `create_*` test convention (build a headless `Runtime`+`Display`, call twice, assert the second returns `Err(Error::Operation(_))`; for xdg-output also assert the pre-init error). Run: `cargo test -p wlr create_viewporter create_single_pixel create_content_type create_xdg_output`. Expected: FAIL (methods absent) then PASS after Steps 2–3.

- [ ] **Step 5: Run gates.** `cargo test -p wlr` + `cargo clippy -p wlr --all-targets -- -D warnings`. Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add crates/wlr/src/runtime.rs crates/wlr/src/lib.rs
git commit -m "feat(wlr): A2 batch-1 passive globals — viewporter, single-pixel, content-type, xdg-output"
```

---

### Task 2: wlr — fractional-scale manager + notify fallback

**Files:**
- Modify: `crates/wlr/src/runtime.rs`, `crates/wlr/src/lib.rs`
- Test: runtime.rs unit tests

**Interfaces:**
- Produces: `Runtime::create_fractional_scale_manager(&Display) -> Result<()>`; `Runtime::notify_fractional_scale(surface: &Surface, scale: f64)` (additive fallback, called by the compositor only if the scene does not auto-send).

- [ ] **Step 1: Add the cell** `fractional_scale_manager: RefCell<Option<NonNull<sys::wlr_fractional_scale_manager_v1>>>` (None default).

- [ ] **Step 2: Write the create wrapper** (version arg `1`):

```rust
/// Create the `wp_fractional_scale_manager_v1` global. wlroots' scene sends
/// each surface its preferred fractional scale on output-enter; this makes a
/// client render a sharp buffer for a fractional output scale (e.g. 1.5).
/// Errors if called twice.
pub fn create_fractional_scale_manager(&self, display: &Display) -> Result<()> {
    if self.inner.fractional_scale_manager.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_fractional_scale_manager called twice"));
    }
    let raw = unsafe { sys::wlr_fractional_scale_manager_v1_create(display.as_ptr(), 1) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_fractional_scale_manager_v1_create"))?;
    *self.inner.fractional_scale_manager.borrow_mut() = Some(raw);
    Ok(())
}
```

- [ ] **Step 3: Write the notify fallback** — a thin wrapper over `wlr_fractional_scale_v1_notify_scale(surface, scale)`, taking whatever `Surface` handle the crate already exposes (mirror an existing per-surface method's surface-pointer access). Document it as "only needed if the scene does not auto-send; harmless if called redundantly."

- [ ] **Step 4: Failing double-create test**, then PASS. Run: `cargo test -p wlr create_fractional_scale`.

- [ ] **Step 5: Gates** (`cargo test -p wlr`, clippy). PASS.

- [ ] **Step 6: Commit** `feat(wlr): A2 batch-1 fractional-scale manager + notify fallback`.

---

### Task 3: wlr — presentation-time + scene wiring

**Files:**
- Modify: `crates/wlr/src/runtime.rs`, `crates/wlr/src/lib.rs`
- Test: runtime.rs unit tests

**Interfaces:**
- Produces: `Runtime::create_presentation(&Display) -> Result<()>`; `Runtime::set_scene_presentation(&self) -> Result<()>` (wires the scene to the presentation global so per-surface feedback rides each output commit).

- [ ] **Step 1: Add the cell** `presentation: RefCell<Option<NonNull<sys::wlr_presentation>>>` (None default).

- [ ] **Step 2: Write create + wiring.** `wlr_presentation_create` takes `(display, backend)`; the backend is available at `init_graphics` time — expose the create to take `&Backend` (mirror `init_graphics`' backend access) or read the stored backend pointer:

```rust
/// Create the `wp_presentation` global (real presentation feedback). Pair with
/// `set_scene_presentation` to have the scene emit per-surface feedback on each
/// output commit. Errors if called twice.
pub fn create_presentation(&self, display: &Display, backend: &Backend<'_>) -> Result<()> {
    if self.inner.presentation.borrow().is_some() {
        return Err(Error::Operation("Runtime::create_presentation called twice"));
    }
    let raw = unsafe { sys::wlr_presentation_create(display.as_ptr(), backend.as_ptr()) };
    let raw = NonNull::new(raw).ok_or(Error::Create("wlr_presentation_create"))?;
    *self.inner.presentation.borrow_mut() = Some(raw);
    Ok(())
}

/// Hand the presentation global to the scene so it emits `presented`/`discarded`
/// feedback per surface on each output commit. Call once, after
/// `create_presentation` and `init_graphics`. Errors if either is missing.
pub fn set_scene_presentation(&self) -> Result<()> {
    let presentation = self.inner.presentation.borrow()
        .ok_or(Error::Operation("set_scene_presentation before create_presentation"))?;
    let scene = self.scene_ptr()   // existing private scene accessor from init_graphics
        .ok_or(Error::Operation("set_scene_presentation before init_graphics"))?;
    // SAFETY: both pointers are live (scene owned by Graphics for the runtime's
    // life; presentation display-owned); the call only stores the association.
    unsafe { sys::wlr_scene_set_presentation(scene.as_ptr(), presentation.as_ptr()) };
    Ok(())
}
```

(Reuse the crate's actual private scene accessor name in place of `scene_ptr()`.)

- [ ] **Step 3: Failing test** — double-create, plus `set_scene_presentation` errors before `create_presentation`. Then PASS. Run: `cargo test -p wlr create_presentation set_scene_presentation`.

- [ ] **Step 4: Gates.** PASS.

- [ ] **Step 5: Commit** `feat(wlr): A2 batch-1 presentation-time + scene wiring`.

---

### Task 4: wlr — coverage ledger, README, version bump, full gates

**Files:**
- Modify: `crates/wlr/coverage/waived.toml`, `crates/wlr/coverage/wrapped.toml`
- Modify: `crates/wlr/README.md`, `crates/wlr/Cargo.toml`

- [ ] **Step 1: Run the coverage audit** to get the exact list of newly-used sys symbols it now demands be wrapped: `cargo test -p wlr --test coverage_audit`. Expected: FAIL naming each `wlr_*` symbol Tasks 1–3 now use that is still `waived` (the eight `create`/`notify`/`set_scene_presentation` symbols, plus any struct types they reference).

- [ ] **Step 2: Move each named symbol** from `waived.toml` to `wrapped.toml`, `module = "runtime"`, `item = "Runtime::<the method>"` (e.g. `wlr_viewporter_create` → `Runtime::create_viewporter`; `wlr_scene_set_presentation` → `Runtime::set_scene_presentation`; `wlr_fractional_scale_v1_notify_scale` → `Runtime::notify_fractional_scale`). Re-run `cargo test -p wlr --test coverage_audit`. Expected: PASS.

- [ ] **Step 3: Add the README changelog section** for the next version (see Global Constraints — do not hard-code; use the number Task 5 confirms), one bullet group per global, "all additive," in the style of the `0.20.23` section.

- [ ] **Step 4: Bump `crates/wlr/Cargo.toml` version** to the confirmed next `0.20.x`.

- [ ] **Step 5: Run the full wlr gate suite:** `cargo test -p wlr` + `cargo clippy --all-targets -- -D warnings` + `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps` + `cargo fmt --all --check`. Expected: all PASS.

- [ ] **Step 6: Commit** `release(wlr): <version> — A2 batch-1 passive compat protocols`.

---

### Task 5: Publish wlr + merge to develop — HARD-STOP

**Files:** none (release action)

- [ ] **Step 1: Confirm the version is free** on crates.io and coordinate with any parallel wlroots-sys sessions (`SendMessage`) so the number is claimed in order.
- [ ] **Step 2: `cargo publish -p wlr --dry-run`** — confirm it packages and verifies (builds against published `wlr-sys`).
- [ ] **Step 3: STOP — present ready for owner consent.** Report: version, dry-run result, gate results. Wait for explicit go.
- [ ] **Step 4 (on go): `cargo publish -p wlr`.** Irreversible.
- [ ] **Step 5: Fast-forward `feature/wlr-a2.1` → `develop`** in the main wloots-sys repo; `git push origin develop` (owner consent already given for the merge as part of the go).

---

### Task 6: compositor — consume the crate and create the six globals

**Files:**
- Modify: `compositor/Cargo.toml`, `harness/Cargo.toml` (wlr pin → published version)
- Modify: `compositor/src/lib.rs` (six boot lines + one wiring call)

**Interfaces:**
- Consumes: the eight new `Runtime` methods from Tasks 1–3.

- [ ] **Step 1: Bump the wlr pin** in `compositor/Cargo.toml` (both occurrences) and `harness/Cargo.toml` to the published version; drop any dev `[patch.crates-io] wlr` from the root `Cargo.toml`; `cargo build -p icedtea-compositor -p icedtea-harness`. Expected: resolves the published crate, builds.

- [ ] **Step 2: Add the boot lines** in `compositor/src/lib.rs`, non-fatal, beside the existing `create_*` block (~line 136), each in the house `if let Err(err) = … { tracing::warn!(...) }` style:

```rust
if let Err(err) = runtime.create_viewporter(&display) {
    tracing::warn!(?err, "viewporter unavailable; clients fall back to unscaled buffers");
}
if let Err(err) = runtime.create_fractional_scale_manager(&display) {
    tracing::warn!(?err, "fractional-scale unavailable; HiDPI clients render at integer scale");
}
if let Err(err) = runtime.create_single_pixel_buffer_manager(&display) {
    tracing::warn!(?err, "single-pixel-buffer unavailable");
}
if let Err(err) = runtime.create_content_type_manager(&display) {
    tracing::warn!(?err, "content-type manager unavailable");
}
if let Err(err) = runtime.create_xdg_output_manager(&display) {
    tracing::warn!(?err, "xdg-output unavailable; some panels/tools lose logical geometry");
}
if let Err(err) = runtime.create_presentation(&display, &backend) {
    tracing::warn!(?err, "presentation-time unavailable; clients get no presentation feedback");
} else if let Err(err) = runtime.set_scene_presentation() {
    tracing::warn!(?err, "presentation created but scene wiring failed");
}
```

(Place `create_presentation`/`set_scene_presentation` after `init_graphics` so the scene exists; `backend` is already in scope at that point — verify against lib.rs's actual variable name.)

- [ ] **Step 3: Build + existing suite** `cargo test -p icedtea-compositor` + clippy. Expected: PASS (no behavior change yet, globals just exist).

- [ ] **Step 4: Commit** `feat(compositor): create A2 batch-1 passive globals at boot`.

---

### Task 7: harness — bind the globals and assert advertisement

**Files:**
- Modify: `harness/src/lib.rs` (bind each new global in the harness registry; extend the `advertised_globals` surface the screencopy tests use)
- Test: `compositor/tests/compat_protocols.rs` (new)

**Interfaces:**
- Consumes: the harness's existing `advertised_globals` registry hook (screencopy spec, criterion 1).
- Produces: `advertised_globals()` includes each of `wp_viewporter`, `wp_fractional_scale_manager_v1`, `wp_single_pixel_buffer_manager_v1`, `wp_content_type_manager_v1`, `wp_presentation`, `zxdg_output_manager_v1`.

- [ ] **Step 1: Write failing advertised-globals test** in the new `compat_protocols.rs` (mirror the screencopy advertised-globals assertion): boot a harness compositor, assert each of the six interface names appears in the advertised registry. Run: `cargo test -p icedtea-compositor --test compat_protocols advertised`. Expected: FAIL (globals not yet bound in the harness registry, or interface-name list incomplete).

- [ ] **Step 2: Extend the harness registry** to bind each new global from `wayland-protocols` (viewporter, fractional-scale, single-pixel-buffer, content-type, presentation, xdg-output), matching how it already binds screencopy/pointer-constraints. Re-run. Expected: PASS.

- [ ] **Step 3: Gates** (`cargo clippy -p icedtea-harness --all-targets`). PASS.

- [ ] **Step 4: Commit** `test(compositor): assert A2 batch-1 globals are advertised`.

---

### Task 8: fractional-scale + viewporter load-bearing test

**Files:**
- Modify: `compositor/tests/compat_protocols.rs`
- Modify: `harness/src/lib.rs` (a viewporter/fractional-scale client helper, if not already covered)

**Interfaces:**
- Consumes: the existing screencopy client (to capture rendered output) and the harness's output-scale setter (`set_output_scale_for_test`).

- [ ] **Step 1: Write the fractional-scale test.** Set the output to a fractional scale (`1.5`, legal per `guarded_scale`). A harness client binding `wp_fractional_scale_v1` for its surface must receive a `preferred_scale` event of `1.5 × 120 = 180` (the protocol encodes scale × 120). Run: `cargo test -p icedtea-compositor --test compat_protocols fractional_scale`. Expected: FAIL first if the scene does **not** auto-send — that is the spike's open question; if it fails, add a `runtime.notify_fractional_scale(...)` call on the surface's output-enter/map path in `state.rs` (the Task 2 fallback) and re-run. Expected: PASS. Record in the commit message which branch (auto-send vs. manual) was needed.

- [ ] **Step 2: Write the viewporter crop test.** A client attaches a buffer and sets a `wp_viewport` source-crop + dest-size; capture the output via the existing screencopy client and assert the cropped-and-scaled region renders (a distinct solid region at the expected size/offset), proving the scene honors the viewport — not merely that the global exists. Run: `cargo test -p icedtea-compositor --test compat_protocols viewporter`. Expected: PASS.

- [ ] **Step 3: Gates.** `cargo test -p icedtea-compositor --test compat_protocols` + clippy. PASS.

- [ ] **Step 4: Commit** `test(compositor): fractional-scale preferred-scale + viewporter crop (load-bearing)`.

---

### Task 9: presentation-feedback test

**Files:**
- Modify: `compositor/tests/compat_protocols.rs`

- [ ] **Step 1: Write the test.** A harness client commits a frame with a `wp_presentation` feedback request and asserts it receives a `presented` (or `discarded`) event after an output commit. Run: `cargo test -p icedtea-compositor --test compat_protocols presentation`. Expected: FAIL then PASS once `set_scene_presentation` is wired (Task 6). If the headless backend never reports a presentation clock, accept `discarded` as a valid terminal feedback and assert on "one of presented|discarded arrives" — document that in the test.

- [ ] **Step 2: Gates.** PASS.

- [ ] **Step 3: Commit** `test(compositor): presentation-time feedback arrives on commit`.

---

### Task 10: final review + merge window — HARD-STOP

- [ ] **Step 1: Full gates both repos** on the freeze commits: icedtea `cargo test -p icedtea-compositor` + `cargo clippy -p icedtea-compositor -p icedtea-harness --all-targets`; confirm wlr already merged (Task 5).
- [ ] **Step 2: Whole-branch review** of `feature/a2-batch1` (dispatch a reviewer / run `/code-review`).
- [ ] **Step 3: STOP — present the merge window** for `feature/a2-batch1 → develop`: gates green, review clean, wlr published + merged. Wait for the owner's go before pushing/PR/merge. Never auto-merge.

## Self-Review

- **Spec coverage:** viewporter ✓(T1/T6), fractional-scale ✓(T2/T6/T8), single-pixel-buffer ✓(T1/T6), content-type ✓(T1/T6), presentation-time ✓(T3/T6/T9), xdg-output ✓(T1/T6, added because the spike found it absent). Batch 2/3 protocols are explicitly out of scope (separate plans). Verification spike ✓ (M-A2.0, recorded above). Publish discipline + hard-stops ✓(T5/T10).
- **Type consistency:** every wrapper returns `Result<()>` and follows `create_relative_pointer_manager`'s exact shape; the two accessor names flagged as "reuse the crate's actual private name" (`output_layout_ptr`, `scene_ptr`) must be reconciled against runtime.rs at execution — the plan calls this out rather than inventing final names.
- **Placeholders:** none — every code step carries real code or a named existing pattern to mirror.
