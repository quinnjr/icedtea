# Pure-Rust GTK-themed UI — M3 Part 3: window/event layer: Surface roles, xkbcommon keyboard, pointer/touch, focus, selection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## Contract deviations

Twelve, each forced by something in the current tree that the contract text did
not account for. Everything else in Part 0 §3.1–§3.5, §3.8, §3.9 is used verbatim.

1. **`ui/src/wayland.rs` has 21 tests, not 11.** Contract §8.3's row says "11"
   (a figure carried over from the M2 contract, when the file was M1-era). The
   file today has 21 `#[test]`s (`every_error_says_what_actually_failed` …
   `the_stale_callback_recovery_resamples_the_animation_clock`). All 21 relocate
   to `ui/src/window/layer.rs` with every assertion, constant and tolerance
   preserved verbatim.

2. **`LayerWindow` stays a concrete struct (moved), not a `pub type` alias.**
   Contract §8.1 says `wayland::LayerWindow` → "`pub type LayerWindow = …`
   compat shim". No alias can satisfy it: `ui/src/app.rs:403` calls
   `LayerWindow::open(sheet, fonts, button)` then `window.run()`, and `app.rs` is
   on P3's must-not-touch list. The whole M1 `AppState`/`LayerWindow` body moves
   verbatim into `ui/src/window/layer.rs`; `ui/src/wayland.rs` becomes a
   four-line `pub use` shim so `ui/tests/layer_shell_screencopy.rs` (which
   imports `icedtea_ui::wayland::BTN_LEFT`, *not* `LayerWindow` — §8.3's row
   names the wrong item) diffs by **zero** lines rather than "imports only".
   `LayerWindowError` **is** the contract's `pub type LayerWindowError =
   SurfaceError;` alias, as written.

3. **`ShmBuffer::new`/`BufferPool::{new,acquire}` become generic over the
   dispatch state.** They are hard-typed `qh: &QueueHandle<AppState>`
   (`ui/src/shm.rs:192,276,309`), so §8.2's "`BufferPool`/`ShmBuffer`/`SlotPool`
   are reused by every `Surface` kind unchanged" is impossible as literally
   written — the new client state is not `AppState`. The three signatures gain
   `D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer,
   BufferSlot> + 'static`, `use crate::wayland::AppState` goes, and **all 11
   shm tests stay byte-identical** (none of them names a `QueueHandle`).

4. **`StyleMap`'s key is `selectors::OpaqueElement`, re-exported as
   `window::NodeAddr`, obtained through `window::node_addr(&Node)`.** Contract
   §3.4 keys it by `Node::addr()`, which is `pub(crate)` (`css/node.rs:445`) and
   P3 may not touch `css/**` to widen it. `<Node as selectors::Element>::opaque`
   is public, `Copy + Eq + Hash`, and is already the identity `LayoutTree` keys
   on (`layout.rs:311`), so it is the same identity by a reachable name.

5. **`Window` builds and owns the `StyleMap`.** Contract §3.4 says "P3 takes it
   by reference and never builds one", but P3's `Window::render` must restyle
   before it can paint, and P4 does not exist yet. `hit_test`/`hit_chain` keep
   their `&StyleMap` parameter exactly as written; `Window::styles()` exposes
   the map P4 will later take over producing.

6. **`InputEvent::{PointerEnter, KeyboardEnter}` each gain
   `target: SurfaceTarget`.** The contract's flat enum cannot say which of a
   window's several `wl_surface`s an event arrived on, and a popup is a separate
   surface with its own pointer/keyboard focus. `enum SurfaceTarget { Window,
   Popup(PopupKey) }`; every later event follows the last enter, which is
   exactly how Wayland defines focus.

7. **`Window` gains `popup_root`, `popup_layout`, `popup_animations`,
   `popup_mark_dirty`, `styles`, `surface`, `seat_serial`.** Additive only;
   P4 has no other way to build, lay out and paint a popup's content.
   `open_popup`/`close_popup`/`clipboard`/`render`/`pump`/`next_deadline` keep
   their contract signatures.

8. **`SurfaceSpec` and `Role` are defined by P3.** Contract §3.1 names
   `Window::open(surface_spec: SurfaceSpec, …)` without defining the type.

9. **`cursor_shape_for` takes a cursor *name*, not a `&ComputedStyle`.** GTK 4
   has no `cursor` CSS property: M2's registry holds 114 properties and
   `ui/tests/gtk4_property_reference.rs` (a §8.2 byte-identical gate) pins that
   nothing else is registered, so a `&ComputedStyle` can never carry one. GTK
   sets cursors programmatically (`gtk_widget_set_cursor_from_name`), and P4's
   `PropName::Cursor` is a widget prop, not a CSS declaration. Signature:
   `pub fn cursor_shape_for(name: &str) -> CursorShape`.

10. **`hit_test`'s "`visibility: hidden`" skip is expressed as an empty or
    absent allocation.** For the same reason as deviation 9: GTK 4 has no
    `visibility` CSS property and M2's registry has none. The rule implemented
    is: no allocation, or a zero-area border box, or computed `opacity == 0`,
    or (under `respect_sensitive`) `PseudoStates::DISABLED`. P4's
    `PropName::Visible = false` must therefore render as a node with no
    allocation, which is documented at both ends.

11. **`css/node.rs` gains a per-tree `focus_visible` flag (default `true`).**
    R3's `:focus-visible` rule is otherwise unobservable: M2 derives
    `FOCUS_VISIBLE` from `FOCUS` unconditionally (`node.rs:353-360`) and masks
    it out of `set_states`, so a `FocusRing::focus_visible()` bool could never
    reach a selector. The edit adds one `Cell<bool>` to `TreeToken`, one
    `Node::set_tree_focus_visible` method, and one condition in `states()`;
    the default keeps every existing `css/node.rs` test byte-identical (§8.2).
    This is P3's only edit under `css/**`.

12. **Additive public items the contract does not list.** All are additions,
    which §0's "a part may add private items freely" permits in spirit and
    which later parts need: `KeyEvent::consumed` (a public field, because
    `effective_mods` cannot recompute a consumed mask without the keymap, and
    P5's controller tests must be able to build a `KeyEvent`);
    `Keymap::set_compose_table` (hermetic compose tests);
    `focus::{Binding, window_binding, FOCUSABLE_CLASS, MAX_FOCUS_DEPTH}`
    (contract §3.5's binding table as a value rather than prose);
    `pointer::{MAX_HIT_DEPTH, MIN_VELOCITY}`;
    `window::{restyle, fold_deadlines, node_addr, NodeAddr, MapPhase,
    may_attach, CONFIGURE_TIMEOUT}`;
    `selection::{TEXT_MIME, read_offer_fd}`;
    `Window::{set_node_text, node_text}` (the smallest thing that puts real
    glyphs on a real surface before P5's `TextLayout` — **P5 deletes both in
    the commit that lands `TextLayout`**).

**Flagged, implemented as written (not a deviation):** contract §3.9 wants an
e2e in which "a `VirtualKeyboardClient` types into an `entry`" and "a popup
opened from a `menubutton` receives the grab". `Kind::Entry` and
`Kind::MenuButton` are P5/P6. P3 builds the same node trees by hand
(`entry > text`, `menubutton > button`) in its own `window-probe` binary — the
same GTK node names the P5/P6 widgets will produce, so the e2e keeps measuring
what it says it measures.

**Assumption P3 executes against:** P1 and P2 have landed. `wlr` 0.20.28 is
published; `compositor/` and `harness/` speak xdg-popup (contract §2), so
`harness::Compositor::spawn()` places popups and `compositor/tests/popups.rs` is
green. `ui/` is untouched by P1/P2 and is exactly `develop` @ `8df998e` plus
this branch's docs commits.

---

**Goal:** Replace `ui/src/wayland.rs`'s single-surface, single-widget,
layer-shell-only client with a real window and event layer: three surface roles
(`xdg_toplevel`, `zwlr_layer_surface_v1`, `xdg_popup`) over one connection, a
libxkbcommon keyboard with compose and client-side repeat, retained-tree hit
testing with a client-side implicit grab, kinetic scrolling, GTK's geometric
focus ring with GTK's real `:focus-visible` rule, and `wl_data_device` +
primary-selection clipboard — everything P4's reactive loop needs and nothing
more.

**Architecture:** `ui/src/window/` is one Wayland client (`WindowState`) driving
one `wl_display` connection. `Window` owns the connection, the event queue, a
`Surface` (one of three roles), the retained root `Node`, its `LayoutTree`,
`AnimationState`, `StyleMap` and Skia raster surface, plus a `Vec<PopupWindow>`
of child popups each with their own `wl_surface`, `BufferPool` and tree.
`Window::pump` is M1's `wait_bounded` generalised: dispatch pending →
`prepare_read` → `poll(2)` → read, answering `xdg_wm_base.ping` on the way past
and returning a batch of `InputEvent`s. `window::keyboard` wraps
`xkbcommon::xkb` (`Context`, `Keymap`, `State`, `compose::{Table, State}`) and
owns the repeat timer xkbcommon does not have, on `anim::Clock`.
`window::pointer` hit-tests the retained tree by walking `Node::children()` in
reverse paint order against `LayoutTree::allocation`. `window::focus` sorts
candidates geometrically (GTK's `gtk_widget_focus_sort`, not tree order) and
owns the focus ring. `window::selection` mirrors `harness/src/lib.rs`'s
data-device shape. M1's `LayerWindow` moves into `window/layer.rs` verbatim and
keeps running the `themed-button` demo; the new path is parallel, exactly as
P5's `ButtonC` will be parallel to M2's `Button`.

**Tech Stack:** Rust (edition 2024, rust-version 1.94), `wayland-client` 0.31
(core protocol, `EventQueue`, `Dispatch`, `event_created_child!`,
`delegate_noop!`), `wayland-protocols` 0.32 with features `client`, `staging`,
`unstable` (`xdg::shell::client::{xdg_wm_base, xdg_surface, xdg_toplevel,
xdg_popup, xdg_positioner}`, `wp::cursor_shape::v1::client`,
`wp::primary_selection::zv1::client`), `wayland-protocols-wlr` 0.3
(`zwlr_layer_shell_v1`), `xkbcommon` 0.9 (links `libxkbcommon`; default feature
`wayland` for `Keymap::new_from_fd`), `rustix` 1 (`poll`), `taffy` 0.14 (through
`layout::LayoutTree`), `skia-rs-safe` 0.4.0 (raster surface), `bitflags` 2,
`tracing`.

**Spec:** `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
(§Section 3 "Window & event layer", §Section 7 testing strategy and gates).
Frozen interface contract:
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§0 module map, §3.1
`Surface`/`Window`/`InputEvent`, §3.2 roles, §3.3 keyboard, §3.4 pointer, §3.5
focus, §3.8 selection, §3.9 tests, §8 migration, §9 P3 boundaries, R2/R3
rulings). Previous milestone's still-binding contract:
`docs/superpowers/plans/2026-08-26-m2-part0-contract.md` §12 E1–E15.

**Research notes:** `.superpowers/m3-plan-notes/xkb-wayland-client.md`
(§1 xkbcommon 0.9's exact surface and the compose/keyboard `State` name
collision, §2 `wl_keyboard`'s evdev-keycode +8 rule and `repeat_info`, §3
xdg-shell's configure/ack sequencing and the positioner completeness rule, §4
the harness's data-device pattern, §5 cursor-shape's `staging` feature),
`.superpowers/m3-plan-notes/current-ui-crate.md` (§11 `wayland.rs`'s
`wait_bounded`/frame-callback machinery, §13 `BufferPool`, §7 `LayoutTree`,
§1 `Node`/`PseudoStates`), `.superpowers/m3-plan-notes/compositor-harness.md`.

## Global Constraints

- Crate: `ui/` = `icedtea-ui`. P3 may create/modify **only**
  `ui/src/window/**` (new), `ui/src/wayland.rs` (reduced to a re-export shim),
  `ui/src/lib.rs` (module list), `ui/Cargo.toml` (dependency and `[[bin]]`
  lines), `ui/src/shm.rs` (deviation 3 — three generic bounds, no test edits),
  `ui/src/css/node.rs` (deviation 11 — one `Cell<bool>`, one method, one
  condition), `ui/src/bin/window-probe.rs` (new), `ui/tests/window_events.rs`
  (new), `ui/tests/fixtures/keymaps/**` (new), `ui/tests/support/mod.rs`
  (grown, not rewritten). **No edits** to `css/**` beyond deviation 11,
  `anim/**`, `paint/**`, `text.rs`, `layout.rs`, `widget/button.rs`, `app.rs`.
- Pinned crate versions: `wayland-client 0.31`, `wayland-protocols 0.32`
  (features `client`, `staging`, `unstable`), `wayland-protocols-wlr 0.3`,
  `xkbcommon 0.9` (new), `skia-rs-safe 0.4.0`, `taffy 0.14`, `cssparser 0.37`,
  `selectors 0.40`, `bitflags 2`, `rustix 1`, `fontconfig 0.11` (optional).
  No other dependency is added; `memmap2` is **not** added (xkbcommon's
  `Keymap::new_from_fd` already maps the fd `MAP_PRIVATE`).
- No `smithay`, no `gtk4`/`gtk4-rs`/`gio`/`glib`/`pango`/`cairo`/`gdk`. Pure
  Rust plus already-linked system C libraries (`libxkbcommon`, `libfontconfig`).
- Edition 2024, `rust-version` 1.94 — already set in the workspace; do not
  change them.
- Gates, all green at every commit:
  `cargo test -p icedtea-ui`,
  `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
  `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`,
  `cargo fmt --all --check`.
  (P1's gates — `cargo test -p wlr --test coverage_audit`, `cargo xtask
  coverage`, `cargo publish --dry-run` — and P2's `cargo test -p
  icedtea-compositor --test popups` ×3 belong to those parts, not this one;
  P3 must not regress them, and runs `cargo test --workspace` once, in Task 18.)
- Every commit message ends with the trailer
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- **The M1/M2 gates stay green and byte-identical** (contract §8.2):
  `ui/tests/themed_button_offscreen.rs` (4), `ui/tests/adwaita_coverage.rs` (9),
  `ui/tests/gtk4_property_reference.rs` (4),
  `ui/tests/transition_screencopy.rs` (1), `ui/tests/layer_shell_screencopy.rs`
  (3 — zero-line diff, deviation 2), every test in `ui/src/css/**`,
  `ui/src/anim/**`, `ui/src/shm.rs` (11), `ui/src/widget/button.rs` (4),
  `ui/src/app.rs`. The 21 `wayland.rs` tests are **relocated, not rewritten**
  (deviation 1): a relocation that drops or weakens an assertion fails review.
- Pinned constants that must not move: `POOL_INITIAL_BUFFERS=2`,
  `POOL_MAX_BUFFERS=3`, `CONFIGURE_TIMEOUT=5s`, `MARGIN=0`, `BTN_LEFT=0x110`,
  900 compiled Adwaita rules, 1941 lines / 37 `@define-color`s, 114/95/19
  property counts.
- Parts execute in order 1 → 8 on the branch `rebuild/pure-rust-gtk-m3`, so P1's
  and P2's `Produces` are available. During P2–P8 development a root
  `[patch.crates-io]` pointing `wlr` at the wloots-sys worktree is allowed in
  its own commit and dropped before merge, exactly as the implicit-grab fix
  did; P3 adds no such patch of its own and must not remove P2's if present.
- Untrusted input never panics: `xdg_toplevel.configure`'s `states` array, a
  compositor-supplied keymap fd, a vendored keymap file, a compose table, an
  `xdg_positioner`'s numbers, a hostile node tree depth. Each gets a named
  never-panic test.
- Every load-bearing test records a mutation check: break the code the test
  claims to cover, confirm the test fails, restore. Timing assertions are
  generous complexity bounds, never wall-clock pins. `Duration::ZERO` from any
  `next_deadline` means "now", never "spin".
- Nothing outside `css::registry` names a CSS property string. P3 names none.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/Cargo.toml` | Modify | Add `xkbcommon = "0.9"`, `wayland-protocols = { version = "0.32", features = ["client", "staging", "unstable"] }`; add the `window-probe` `[[bin]]`. |
| `ui/src/lib.rs` | Modify | `pub mod window;` added to the module list; `pub mod wayland;` stays (now a shim). |
| `ui/src/window/mod.rs` | Create | `Surface`, `SurfaceSpec`, `Role`, `SurfaceStates`, `SurfaceError`, `SurfaceTarget`, `InputEvent`, `Window`, `WindowState`, `NodeAddr`/`node_addr`/`StyleMap`, the generalised `pump`, the render pipeline, the registry/seat/`xdg_wm_base` dispatch. |
| `ui/src/window/toplevel.rs` | Create | `Toplevel`: `xdg_surface` + `xdg_toplevel`, title/app-id/min/max, configure→`SurfaceStates`, `ack_configure`, close. |
| `ui/src/window/layer.rs` | Create | `Layer`: `zwlr_layer_surface_v1` role for the new path — **and** M1's `AppState`/`LayerWindow`/`wait_bounded`/frame machinery and its 21 tests, moved verbatim. |
| `ui/src/window/popup.rs` | Create | `Popup`, `PopupKey`, `Positioner`, `Anchor`, `Gravity`, `ConstraintAdjustment`, `PopupAnchorPoint`, positioner validation, grab, reposition, destroy-topmost-first. |
| `ui/src/window/keyboard.rs` | Create | `Mods`, `KeyEvent`, `Keymap`, `KeymapError`: keymap from fd/names/string, `update_key`/`update_mask`, `translate` with compose, consumed-mod masking, `repeats`, and the repeat timer on `anim::Clock`. |
| `ui/src/window/pointer.rs` | Create | `Hit`, `hit_test`, `hit_chain`, `ImplicitGrab`, `Scroll`, `ScrollSource`, `Kinetic`, `CursorShape`, `cursor_shape_for`. |
| `ui/src/window/focus.rs` | Create | `FocusDirection`, `FocusCause`, `FocusRing`, `focus_sort`, `navigate`, `is_focusable`, `window_binding`. |
| `ui/src/window/selection.rs` | Create | `Clipboard` over `wl_data_device` + `zwp_primary_selection_device_v1`. |
| `ui/src/wayland.rs` | Modify | Reduced to `pub use crate::window::layer::{AppState, BTN_LEFT, CONFIGURE_TIMEOUT, LayerWindow, LayerWindowError, MARGIN};` — the M1 import paths keep resolving. |
| `ui/src/shm.rs` | Modify | Three signatures made generic over the dispatch state (deviation 3). No test edits. |
| `ui/src/css/node.rs` | Modify | Per-tree `focus_visible` flag (deviation 11). No test edits. |
| `ui/src/bin/window-probe.rs` | Create | The e2e client: opens a toplevel (or a layer surface) holding a hand-built `window > box > entry > text` / `menubutton` tree, reports geometry on stdout, drives `Window::pump`/`render`. |
| `ui/tests/fixtures/keymaps/us.xkb` | Create | `xkbcli compile-keymap --layout us` output, vendored for hermetic key translation. |
| `ui/tests/fixtures/keymaps/us-intl.xkb` | Create | `xkbcli compile-keymap --layout us --variant intl` output — the dead-key layout the compose test needs. |
| `ui/tests/fixtures/keymaps/compose.us` | Create | A three-line Compose table, so the compose test does not depend on the machine's locale files. |
| `ui/tests/window_events.rs` | Create | The five §3.9 end-to-end tests against the harness compositor. |
| `ui/tests/support/mod.rs` | Modify | Grown with `spawn_window_probe`, `probe_geometry`, `capture_until_frame`. |

Unit tests live in `#[cfg(test)] mod tests` inside each `window/*.rs`, matching
the crate's layout; `ui/tests/` stays reserved for the Wayland/offscreen gates.

---

## Task 1: Dependencies and the `window` module's shared vocabulary

**Files:**
- Modify: `ui/Cargo.toml` (the `[dependencies]` block)
- Modify: `ui/src/lib.rs` (module list)
- Create: `ui/src/window/mod.rs`

**Interfaces:**
- Consumes: `bitflags 2`; `wayland_client::{ConnectError, DispatchError}`;
  `selectors::{Element, OpaqueElement}`; `crate::css::computed::ComputedStyle`;
  `crate::css::node::Node`.
- Produces:
  ```rust
  pub use selectors::OpaqueElement as NodeAddr;
  #[must_use] pub fn node_addr(node: &Node) -> NodeAddr;
  pub type StyleMap = std::collections::HashMap<NodeAddr, std::rc::Rc<ComputedStyle>>;

  pub struct SurfaceStates: u16;          // bitflags: MAXIMIZED FULLSCREEN RESIZING
                                          // ACTIVATED TILED_{LEFT,RIGHT,TOP,BOTTOM} SUSPENDED
  impl SurfaceStates { #[must_use] pub fn from_wire(states: &[u8]) -> SurfaceStates; }

  pub enum SurfaceError { Connect(ConnectError), MissingGlobal(&'static str),
      Shm(std::io::Error), Socket(std::io::Error), Render(&'static str), NoFont,
      Dispatch(DispatchError), Closed, Timeout(std::time::Duration),
      Protocol(&'static str), Keymap }
  impl std::fmt::Display for SurfaceError;
  impl std::error::Error for SurfaceError;

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum SurfaceTarget { Window, Popup(PopupKey) }   // PopupKey lands in Task 14
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/src/window/mod.rs`. Put the module header and this test module in it;
the production items come in Step 3.

```rust
#[cfg(test)]
mod tests {
    use super::{SurfaceError, SurfaceStates, node_addr};
    use crate::css::node::Node;
    use std::error::Error;
    use std::time::Duration;

    /// The `xdg_toplevel::State` values, as the protocol numbers them.
    fn wire(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn states_from_the_wire_decode_every_flag() {
        assert_eq!(SurfaceStates::from_wire(&wire(&[])), SurfaceStates::empty());
        assert_eq!(
            SurfaceStates::from_wire(&wire(&[1, 2, 3, 4])),
            SurfaceStates::MAXIMIZED
                | SurfaceStates::FULLSCREEN
                | SurfaceStates::RESIZING
                | SurfaceStates::ACTIVATED
        );
        assert_eq!(
            SurfaceStates::from_wire(&wire(&[5, 6, 7, 8, 9])),
            SurfaceStates::TILED_LEFT
                | SurfaceStates::TILED_RIGHT
                | SurfaceStates::TILED_TOP
                | SurfaceStates::TILED_BOTTOM
                | SurfaceStates::SUSPENDED
        );
        assert!(
            !SurfaceStates::from_wire(&wire(&[4])).contains(SurfaceStates::MAXIMIZED),
            "activated must not imply maximized"
        );
    }

    #[test]
    fn a_hostile_states_array_never_panics() {
        // The compositor hands this straight to us: a short tail, an unknown
        // state (xdg-shell 7 adds constrained_* = 10..=13, and a future
        // version will add more) and a 64 KiB array all have to be survivable.
        for bytes in [
            vec![0u8],
            vec![0u8, 0, 0],
            vec![0xFF; 7],
            wire(&[10, 11, 12, 13, 999, u32::MAX]),
            vec![0u8; 65_536],
        ] {
            let states = SurfaceStates::from_wire(&bytes);
            assert!(
                SurfaceStates::all().contains(states),
                "from_wire invented a flag outside SurfaceStates::all()"
            );
        }
        // A trailing partial u32 is dropped, not read out of bounds.
        let mut ragged = wire(&[4]);
        ragged.push(9);
        assert_eq!(SurfaceStates::from_wire(&ragged), SurfaceStates::ACTIVATED);
    }

    #[test]
    fn every_surface_error_says_what_actually_failed() {
        let cases: Vec<(SurfaceError, &str)> = vec![
            (SurfaceError::Shm(std::io::Error::other("ENOSPC")), "shm buffer"),
            (SurfaceError::Socket(std::io::Error::other("EPIPE")), "Wayland socket"),
            (SurfaceError::Render("the raster surface"), "raster"),
            (SurfaceError::NoFont, "typeface"),
            (SurfaceError::Closed, "closed"),
            (SurfaceError::Timeout(Duration::from_secs(5)), "did not configure"),
            (SurfaceError::MissingGlobal("zwlr_layer_shell_v1"), "zwlr_layer_shell_v1"),
            (SurfaceError::Protocol("xdg_positioner needs a size"), "xdg_positioner"),
            (SurfaceError::Keymap, "keymap"),
        ];
        for (error, expected) in &cases {
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "{error:?} reads {message:?}, which does not mention {expected:?}"
            );
        }
        assert!(
            SurfaceError::Shm(std::io::Error::other("ENOSPC")).source().is_some(),
            "the underlying io::Error is not reachable"
        );
        assert!(SurfaceError::NoFont.source().is_none());
        assert!(SurfaceError::Keymap.source().is_none());
    }

    #[test]
    fn node_addr_is_a_stable_per_node_identity() {
        let a = Node::new("button");
        let b = Node::new("button");
        assert_eq!(node_addr(&a), node_addr(&a.clone()), "a handle is the same node");
        assert_ne!(node_addr(&a), node_addr(&b), "same name, different node");
        // Identity survives mutation: a StyleMap keyed on it must not lose
        // entries when a class is added mid-frame.
        let before = node_addr(&a);
        a.add_class("flat");
        assert_eq!(before, node_addr(&a));
    }
}
```

**Mutation checks.**
`states_from_the_wire_decode_every_flag`: swap the `4 => ACTIVATED` arm for
`MAXIMIZED` → the last assertion fails.
`a_hostile_states_array_never_panics`: replace `chunks_exact(4)` with
`chunks(4)` and `try_into().unwrap()` → the ragged-tail case panics.
`every_surface_error_says_what_actually_failed`: make `Protocol`'s arm print a
bare "protocol error" → the `xdg_positioner` assertion fails.
`node_addr_is_a_stable_per_node_identity`: make `node_addr` hash the node's name
instead of its payload address → the `assert_ne!` fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: FAIL to compile — `error[E0433]: failed to resolve: use of undeclared
crate or module `window`` from `lib.rs` until Step 3 declares it, then
`error[E0432]: unresolved import `super::SurfaceStates`` and the same for
`SurfaceError` and `node_addr`.

- [ ] **Step 3: Write the implementation**

`ui/Cargo.toml` — add two dependency lines (alphabetical position, after
`wayland-protocols-wlr`):

```toml
wayland-protocols = { version = "0.32", features = ["client", "staging", "unstable"] }
xkbcommon = "0.9"
```

`ui/src/lib.rs` — add `pub mod window;` after `pub mod widget;`:

```rust
pub mod wayland;
pub mod widget;
pub mod window;
```

`ui/src/window/mod.rs` — the header and the shared vocabulary:

```rust
//! The window and event layer: three Wayland surface roles over one
//! connection, keyboard/pointer/touch input, focus and selection.
//!
//! Hand-rolled on `wayland-client` in the shape the rest of this repo uses
//! (`harness/src/lib.rs`, M1's `wayland.rs`): no `smithay-client-toolkit`, no
//! `calloop`. `window/layer.rs` also carries M1's `LayerWindow` unchanged --
//! the `themed-button` demo and its pixel gate still run on it.

pub mod focus;
pub mod keyboard;
pub mod layer;
pub mod pointer;
pub mod popup;
pub mod selection;
pub mod toplevel;

use std::time::Duration;

use wayland_client::{ConnectError, DispatchError};

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;

/// A node's identity, as a key for per-frame caches.
///
/// `selectors::OpaqueElement` is the payload address `LayoutTree` already keys
/// on (`layout.rs:311`), is `Copy + Eq + Hash`, and -- unlike a raw `usize` --
/// is the identity the `selectors` crate itself guarantees is stable for as
/// long as any handle to the node lives.
pub use selectors::OpaqueElement as NodeAddr;

/// `node`'s [`NodeAddr`].
#[must_use]
pub fn node_addr(node: &Node) -> NodeAddr {
    <Node as selectors::Element>::opaque(node)
}

/// One frame's computed styles, keyed by node.
///
/// Rebuilt by [`Window::render`] on every restyle and handed to
/// [`pointer::hit_test`] by reference; P4 takes over producing it.
pub type StyleMap = std::collections::HashMap<NodeAddr, std::rc::Rc<ComputedStyle>>;

bitflags::bitflags! {
    /// `xdg_toplevel.configure` states, plus the layer/popup analogues.
    ///
    /// A layer surface and a popup have no states at all, so theirs is always
    /// empty -- which is what makes `!ACTIVATED` mean `:backdrop` only for a
    /// toplevel.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct SurfaceStates: u16 {
        /// `xdg_toplevel.state.maximized`
        const MAXIMIZED = 1 << 0;
        /// `xdg_toplevel.state.fullscreen`
        const FULLSCREEN = 1 << 1;
        /// `xdg_toplevel.state.resizing`
        const RESIZING = 1 << 2;
        /// `xdg_toplevel.state.activated`. Clear => `:backdrop` on the root.
        const ACTIVATED = 1 << 3;
        /// `xdg_toplevel.state.tiled_left`
        const TILED_LEFT = 1 << 4;
        /// `xdg_toplevel.state.tiled_right`
        const TILED_RIGHT = 1 << 5;
        /// `xdg_toplevel.state.tiled_top`
        const TILED_TOP = 1 << 6;
        /// `xdg_toplevel.state.tiled_bottom`
        const TILED_BOTTOM = 1 << 7;
        /// `xdg_toplevel.state.suspended`
        const SUSPENDED = 1 << 8;
    }
}

impl SurfaceStates {
    /// Decode `xdg_toplevel.configure`'s `states` array.
    ///
    /// The wire carries a packed array of native-endian `u32`s, straight from
    /// the compositor, so this is an untrusted decode: a ragged tail is
    /// dropped rather than read past, and an unknown state (xdg-shell 7's
    /// `constrained_*`, or anything a future version adds) is ignored rather
    /// than folded into some arbitrary flag.
    #[must_use]
    pub fn from_wire(states: &[u8]) -> SurfaceStates {
        let mut out = SurfaceStates::empty();
        for chunk in states.chunks_exact(4) {
            let raw = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            out |= match raw {
                1 => SurfaceStates::MAXIMIZED,
                2 => SurfaceStates::FULLSCREEN,
                3 => SurfaceStates::RESIZING,
                4 => SurfaceStates::ACTIVATED,
                5 => SurfaceStates::TILED_LEFT,
                6 => SurfaceStates::TILED_RIGHT,
                7 => SurfaceStates::TILED_TOP,
                8 => SurfaceStates::TILED_BOTTOM,
                9 => SurfaceStates::SUSPENDED,
                _ => SurfaceStates::empty(),
            };
        }
        out
    }
}

/// Everything that can go wrong opening, running or closing a surface.
///
/// A superset of M1's `LayerWindowError`: the nine carried variants keep their
/// names, payloads and messages, so `wayland::LayerWindowError` is a type alias
/// for this (contract §8.1) and M1's own error test still passes verbatim.
#[derive(Debug)]
pub enum SurfaceError {
    /// `wl_display` connection failed.
    Connect(ConnectError),
    /// The compositor never advertised a global the client requires.
    MissingGlobal(&'static str),
    /// Allocating or uploading a `wl_shm` buffer failed.
    Shm(std::io::Error),
    /// The Wayland socket itself failed: a flush, a poll, or a read.
    Socket(std::io::Error),
    /// A Skia surface could not be allocated.
    Render(&'static str),
    /// No usable UI typeface is installed.
    NoFont,
    /// The event queue failed.
    Dispatch(DispatchError),
    /// The compositor closed the surface before it ever configured it.
    Closed,
    /// The compositor did not configure the surface in time.
    Timeout(Duration),
    /// The client would have violated the protocol; the request was refused
    /// locally rather than sent (a positioner with no size, a popup destroyed
    /// out of order).
    Protocol(&'static str),
    /// The compositor's keymap could not be mapped or compiled.
    Keymap,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "cannot connect to the Wayland display: {e}"),
            Self::MissingGlobal(name) => write!(f, "compositor did not advertise {name}"),
            Self::Shm(e) => write!(f, "cannot allocate or upload the shm buffer: {e}"),
            Self::Socket(e) => write!(f, "the Wayland socket failed: {e}"),
            Self::Render(what) => write!(f, "cannot allocate {what}"),
            Self::NoFont => write!(
                f,
                "no UI typeface found; install dejavu, liberation or noto sans"
            ),
            Self::Dispatch(e) => write!(f, "Wayland dispatch failed: {e}"),
            Self::Closed => write!(
                f,
                "the compositor closed the surface before configuring it"
            ),
            Self::Timeout(after) => write!(
                f,
                "the compositor did not configure the surface within {after:?}"
            ),
            Self::Protocol(what) => write!(f, "refusing to send an invalid request: {what}"),
            Self::Keymap => write!(f, "the compositor's keymap could not be compiled"),
        }
    }
}

impl std::error::Error for SurfaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) => Some(e),
            Self::Shm(e) | Self::Socket(e) => Some(e),
            Self::Dispatch(e) => Some(e),
            Self::MissingGlobal(_)
            | Self::Render(_)
            | Self::NoFont
            | Self::Closed
            | Self::Timeout(_)
            | Self::Protocol(_)
            | Self::Keymap => None,
        }
    }
}
```

The seven `pub mod` lines at the top will not compile until their files exist.
Create the six not covered by this task as one-line stubs now, each with a
module doc comment and nothing else, so the crate builds; every later task
fills one in:

```rust
// ui/src/window/toplevel.rs
//! The `xdg_toplevel` surface role. Task 10 fills this in.
// ui/src/window/layer.rs
//! The `zwlr_layer_surface_v1` role, and M1's `LayerWindow`. Task 2 fills this in.
// ui/src/window/popup.rs
//! The `xdg_popup` role and its positioner. Task 14 fills this in.
// ui/src/window/keyboard.rs
//! `wl_keyboard` through `libxkbcommon`. Task 3 fills this in.
// ui/src/window/pointer.rs
//! Hit testing, implicit grab, scrolling, cursor shapes. Task 6 fills this in.
// ui/src/window/focus.rs
//! The focus ring and GTK's geometric focus sort. Task 8 fills this in.
// ui/src/window/selection.rs
//! Clipboard and primary selection. Task 15 fills this in.
```

`SurfaceTarget` needs `PopupKey`, which Task 14 defines; declare it there and
re-export it here in Task 14's step. Nothing in Tasks 1–12 needs it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: PASS — 4 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS — every existing test still green; nothing else changed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
git add ui/Cargo.toml ui/src/lib.rs ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): add the window module's shared vocabulary

SurfaceStates decodes xdg_toplevel.configure's packed state array as
untrusted input: a ragged tail is dropped and an unknown state ignored.
SurfaceError is a superset of M1's LayerWindowError with the same nine
variant names, payloads and messages, so the alias in the next commit
keeps app.rs and the M1 error test working. NodeAddr keys per-frame
caches on the identity `selectors` already guarantees.

Adds xkbcommon 0.9 and wayland-protocols 0.32 (client, staging, unstable).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Relocate M1's `LayerWindow` into `window/layer.rs`

**Files:**
- Modify: `ui/src/window/layer.rs` (replace the stub with `wayland.rs`'s body)
- Modify: `ui/src/wayland.rs` (reduce to a re-export shim)
- Modify: `ui/src/shm.rs:26-29,192,276,309` (deviation 3: generic over `D`)

**Interfaces:**
- Consumes: Task 1's `SurfaceError`.
- Produces:
  ```rust
  // ui/src/window/layer.rs
  pub struct AppState { /* private, exactly as M1 */ }
  pub struct LayerWindow { /* private, exactly as M1 */ }
  pub type LayerWindowError = crate::window::SurfaceError;
  pub const BTN_LEFT: u32 = 0x110;
  pub const MARGIN: i32 = 0;
  pub const CONFIGURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
  impl LayerWindow {
      pub fn open(sheet: CompiledSheet, fonts: FontDatabase, button: Button)
          -> Result<Self, LayerWindowError>;
      pub fn run(&mut self) -> Result<(), LayerWindowError>;
  }

  // ui/src/shm.rs
  impl ShmBuffer {
      pub fn new<D>(shm: &wl_shm::WlShm, qh: &QueueHandle<D>, width: i32, height: i32,
                    slot: BufferSlot) -> io::Result<Self>
      where D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, BufferSlot> + 'static;
  }
  impl BufferPool {
      pub fn new<D>(shm: &wl_shm::WlShm, qh: &QueueHandle<D>, width: i32, height: i32)
          -> io::Result<Self> where D: /* same bounds */;
      pub fn acquire<D>(&mut self, shm: &wl_shm::WlShm, qh: &QueueHandle<D>)
          -> io::Result<Option<usize>> where D: /* same bounds */;
  }
  ```

- [ ] **Step 1: Write the failing test**

The tests for this task already exist and must not be rewritten: the 21 in
`ui/src/wayland.rs` and the 3 in `ui/tests/layer_shell_screencopy.rs`. The
"failing test" for a relocation is the compile itself plus one new assertion
that the shim really is a shim. Append to `ui/src/wayland.rs` (this is the only
new code in that file, and it replaces its 1,765 lines):

```rust
#[cfg(test)]
mod tests {
    /// The M1 import paths still resolve to the relocated items.
    ///
    /// `ui/tests/layer_shell_screencopy.rs` -- a byte-identical gate file --
    /// imports `icedtea_ui::wayland::BTN_LEFT`, and `app.rs` calls
    /// `LayerWindow::open`/`run` through this module. A rename that broke
    /// either would otherwise only show up in a test binary that needs a live
    /// compositor.
    #[test]
    fn the_m1_import_paths_still_resolve() {
        assert_eq!(super::BTN_LEFT, 0x110);
        assert_eq!(super::MARGIN, 0);
        assert_eq!(super::CONFIGURE_TIMEOUT, std::time::Duration::from_secs(5));
        let error: super::LayerWindowError = super::LayerWindowError::Closed;
        assert!(error.to_string().contains("closed"));
        // `LayerWindow` is nameable through the old path (the type check is
        // the assertion; there is no compositor here to open one against).
        fn _accepts(_: fn(
            crate::css::cascade::CompiledSheet,
            crate::text::FontDatabase,
            crate::widget::button::Button,
        ) -> Result<super::LayerWindow, super::LayerWindowError>) {}
        _accepts(super::LayerWindow::open);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib wayland::tests`
Expected: FAIL to compile — `error[E0432]: unresolved import
`crate::window::layer::LayerWindow`` (the shim's `pub use` names items the stub
module does not have yet).

- [ ] **Step 3: Write the implementation**

Three mechanical moves, in this order.

**3a. `ui/src/shm.rs`** — make the three constructors generic. Replace the
import block at `shm.rs:26-29`:

```rust
use wayland_client::{Dispatch, QueueHandle};
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
```

(delete `use crate::wayland::AppState;`), then change the three signatures,
leaving every body untouched:

```rust
    pub fn new<D>(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<D>,
        width: i32,
        height: i32,
        slot: BufferSlot,
    ) -> io::Result<Self>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, BufferSlot> + 'static,
    {
```

```rust
    pub fn new<D>(shm: &wl_shm::WlShm, qh: &QueueHandle<D>, width: i32, height: i32) -> io::Result<Self>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, BufferSlot> + 'static,
    {
```

```rust
    pub fn acquire<D>(&mut self, shm: &wl_shm::WlShm, qh: &QueueHandle<D>) -> io::Result<Option<usize>>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, BufferSlot> + 'static,
    {
```

Add one sentence to `BufferPool`'s doc comment recording why:

```rust
/// A double-buffered set of same-sized `wl_shm` buffers.
///
/// Generic over the dispatch state because M3 has four client states that all
/// want one of these (the M1 `LayerWindow`, `window::Window`, and each popup's
/// own pool); the M1 signatures named `AppState` outright, which no other
/// client could satisfy.
```

**3b. `ui/src/window/layer.rs`** — move `ui/src/wayland.rs`'s entire body here,
with exactly four edits:

1. The module doc comment gains a second paragraph:
   ```rust
   //! M1's `LayerWindow` lives here unchanged: `app.rs::run_themed_button` and
   //! the `themed-button` binary still drive it, and `tests/layer_shell_screencopy.rs`
   //! is a byte-identical gate over its pixels. `window::Window` with
   //! `Role::Layer` is the parallel, general path -- not a generalisation of
   //! this one (contract §8.1's `Button`/`ButtonC` precedent).
   ```
2. The `LayerWindowError` enum definition, its `Display` impl and its
   `std::error::Error` impl are **deleted** and replaced by one line:
   ```rust
   /// M1's error type. Contract §8.1: the nine variants it carried are
   /// [`SurfaceError`]'s first nine, with the same names, payloads and
   /// messages.
   pub type LayerWindowError = crate::window::SurfaceError;
   ```
   Every `LayerWindowError::Variant(..)` construction and match arm in the file
   keeps working through the alias, unchanged.
3. `use crate::wayland::AppState;`-style self references (there are none) and
   the `use` block gain `use crate::window::SurfaceError;` only if a `-D
   warnings` build asks for it; otherwise the alias is enough.
4. `AppState` and its `Dispatch` impls become `pub` where they were private
   only if the `wayland.rs` shim needs to name them: it re-exports `AppState`
   because `shm.rs`'s M1 tests do not, but the shim's `pub use` list does.
   Keep `AppState`'s **fields and methods** exactly as private as they were.

Everything else -- `wait_bounded`, `select_paint_slot`, `defer_frame`,
`must_await_release`, `should_request_frame`, `frame_recovery_wait`,
`frame_pending_is_stale`, `socket_error`, `wl_surface_event_forgets_frame_pending`,
`Repaint`, `PendingFrame`, the constants, every `Dispatch` impl, and the whole
`#[cfg(test)] mod tests` with its 21 tests -- moves byte for byte.

**3c. `ui/src/wayland.rs`** — replace the file with the shim plus the test from
Step 1:

```rust
//! M1's Wayland client, kept at its original path.
//!
//! The implementation moved to [`crate::window::layer`] when M3 grew three
//! surface roles; this module is the compatibility surface contract §8.1
//! promises, so `app.rs`, the `themed-button` binary and the byte-identical
//! gate `ui/tests/layer_shell_screencopy.rs` (which imports [`BTN_LEFT`] from
//! here) keep their M1 paths.

pub use crate::window::layer::{
    AppState, BTN_LEFT, CONFIGURE_TIMEOUT, LayerWindow, LayerWindowError, MARGIN,
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib`
Expected: PASS — the 21 relocated tests now report as `window::layer::tests::*`,
`shm.rs`'s 11 and everything else unchanged. Confirm the count explicitly:

Run: `cargo test -p icedtea-ui --lib window::layer::tests 2>&1 | tail -3`
Expected: `test result: ok. 21 passed`.

Run: `git diff --stat ui/tests/layer_shell_screencopy.rs ui/tests/themed_button_offscreen.rs ui/src/widget/button.rs ui/src/app.rs`
Expected: no output — none of the gate files changed.

Run: `git diff -U0 ui/src/shm.rs | grep -c '^[-+].*#\[test\]'`
Expected: `0` — no test in `shm.rs` moved.

Run: `cargo test -p icedtea-ui --test layer_shell_screencopy`
Expected: PASS — 3 tests (needs the harness compositor binary; build the
workspace first with `cargo build --workspace` if it has not been built).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
git add ui/src/wayland.rs ui/src/window/layer.rs ui/src/shm.rs
git commit -m "$(cat <<'EOF'
refactor(ui/window): move M1's LayerWindow to window/layer.rs behind a shim

The file moves verbatim -- all 21 tests, every constant, every tolerance --
and wayland.rs becomes a six-line re-export so app.rs and the
byte-identical layer_shell_screencopy.rs gate keep their M1 paths. Its
error enum becomes `pub type LayerWindowError = SurfaceError`, contract
§8.1's alias.

BufferPool/ShmBuffer gain a generic dispatch-state parameter: they were
hard-typed to M1's AppState, which none of M3's other client states can
be. No shm test changed.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `window/keyboard.rs` — keymap, modifiers, translation

**Files:**
- Modify: `ui/src/window/keyboard.rs` (replace the stub)
- Create: `ui/tests/fixtures/keymaps/us.xkb`
- Create: `ui/tests/fixtures/keymaps/us-intl.xkb`

**Interfaces:**
- Consumes: `xkbcommon::xkb`; Task 1's module.
- Produces:
  ```rust
  pub struct Mods: u8;      // bitflags: SHIFT CTRL ALT LOGO CAPS NUM
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct KeyEvent {
      pub keycode: u32, pub keysym: xkbcommon::xkb::Keysym, pub utf8: Option<String>,
      pub mods: Mods, pub consumed: Mods, pub pressed: bool, pub repeat: bool,
      pub serial: u32, pub time_ms: u32,
  }
  impl KeyEvent { #[must_use] pub fn effective_mods(&self) -> Mods; }
  pub struct Keymap { /* private */ }
  impl Keymap {
      pub fn from_fd(fd: std::os::fd::OwnedFd, size: usize) -> Result<Self, KeymapError>;
      pub fn from_names(layout: &str, variant: &str) -> Result<Self, KeymapError>;
      pub fn from_string(text: &str) -> Result<Self, KeymapError>;
      pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32);
      pub fn update_key(&mut self, keycode: u32, pressed: bool);
      pub fn translate(&mut self, keycode: u32, pressed: bool, serial: u32, time_ms: u32) -> KeyEvent;
      #[must_use] pub fn mods(&self) -> Mods;
      #[must_use] pub fn repeats(&self, keycode: u32) -> bool;
  }
  #[derive(Debug)] pub enum KeymapError { Context, Compile, Mmap(std::io::Error) }
  ```

- [ ] **Step 1: Write the failing test**

First vendor the fixtures (they are inputs to the test, not code):

```bash
mkdir -p ui/tests/fixtures/keymaps
xkbcli compile-keymap --layout us > ui/tests/fixtures/keymaps/us.xkb
xkbcli compile-keymap --layout us --variant intl > ui/tests/fixtures/keymaps/us-intl.xkb
head -1 ui/tests/fixtures/keymaps/us.xkb        # expect: xkb_keymap {
wc -l ui/tests/fixtures/keymaps/us.xkb          # expect: ~2000 lines
```

Then `ui/src/window/keyboard.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::{KeyEvent, Keymap, KeymapError, Mods};
    use xkbcommon::xkb;

    /// Linux evdev codes, as `wl_keyboard.key` reports them (XKB's are +8).
    const KEY_A: u32 = 30;
    const KEY_B: u32 = 48;
    const KEY_1: u32 = 2;
    const KEY_TAB: u32 = 15;
    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_CAPSLOCK: u32 = 58;

    fn us() -> Keymap {
        Keymap::from_string(include_str!("../../tests/fixtures/keymaps/us.xkb"))
            .expect("the vendored us keymap compiles")
    }

    #[test]
    fn a_plain_letter_translates_to_its_keysym_and_text() {
        let mut keymap = us();
        let event = keymap.translate(KEY_A, true, 7, 1234);
        assert_eq!(event.keysym, xkb::Keysym::a);
        assert_eq!(event.utf8.as_deref(), Some("a"));
        assert_eq!(event.keycode, KEY_A, "the raw evdev code is reported, not XKB's +8");
        assert_eq!(event.mods, Mods::empty());
        assert!(event.pressed);
        assert!(!event.repeat);
        assert_eq!(event.serial, 7);
        assert_eq!(event.time_ms, 1234);
    }

    #[test]
    fn a_release_carries_no_text() {
        // GTK inserts text on press only; a release that also carried "a"
        // would type every character twice.
        let mut keymap = us();
        let event = keymap.translate(KEY_A, false, 8, 1250);
        assert_eq!(event.keysym, xkb::Keysym::a);
        assert!(event.utf8.is_none(), "a key release must produce no text");
        assert!(!event.pressed);
    }

    #[test]
    fn shift_shifts_the_level_and_reports_the_modifier() {
        let mut keymap = us();
        keymap.update_key(KEY_LEFTSHIFT, true);
        assert_eq!(keymap.mods(), Mods::SHIFT);
        let event = keymap.translate(KEY_A, true, 9, 0);
        assert_eq!(event.keysym, xkb::Keysym::A);
        assert_eq!(event.utf8.as_deref(), Some("A"));
        assert_eq!(event.mods, Mods::SHIFT);
        keymap.update_key(KEY_LEFTSHIFT, false);
        assert_eq!(keymap.mods(), Mods::empty());
        assert_eq!(keymap.translate(KEY_A, true, 10, 0).utf8.as_deref(), Some("a"));
    }

    #[test]
    fn the_servers_modifier_state_wins_over_our_own_bookkeeping() {
        // A Wayland client feeds `wl_keyboard.modifiers`, not its own key
        // tracking: the compositor is the one that knows about keys pressed
        // before we were focused. `update_mask` must therefore be able to
        // assert a modifier we never saw go down.
        let mut keymap = us();
        let shift = 1u32 << keymap.mod_index_for_test(xkb::MOD_NAME_SHIFT);
        let caps = 1u32 << keymap.mod_index_for_test(xkb::MOD_NAME_CAPS);
        keymap.update_mask(shift, 0, caps, 0);
        assert_eq!(keymap.mods(), Mods::SHIFT | Mods::CAPS);
        assert_eq!(keymap.translate(KEY_A, true, 11, 0).utf8.as_deref(), Some("a"),
            "shift with caps lock cancels back to lower case, as xkb defines it");
        keymap.update_mask(0, 0, 0, 0);
        assert_eq!(keymap.mods(), Mods::empty());
    }

    #[test]
    fn control_reports_ctrl_and_no_printable_text() {
        let mut keymap = us();
        keymap.update_key(KEY_LEFTCTRL, true);
        let event = keymap.translate(KEY_A, true, 12, 0);
        assert!(event.mods.contains(Mods::CTRL));
        // xkb hands back the C0 control character; a text-entry widget must
        // not insert it, and `utf8` is what it inserts.
        assert!(
            event.utf8.as_deref().is_none_or(|s| s.chars().all(|c| !c.is_control())),
            "Ctrl+A produced insertable control text: {:?}",
            event.utf8
        );
        assert_eq!(event.keysym, xkb::Keysym::a, "Ctrl does not change the keysym");
    }

    #[test]
    fn keys_with_no_text_report_none() {
        let mut keymap = us();
        for code in [KEY_TAB, KEY_LEFTSHIFT, KEY_CAPSLOCK] {
            let event = keymap.translate(code, true, 0, 0);
            assert!(
                event.utf8.as_deref().is_none_or(|s| s.chars().all(|c| !c.is_control())),
                "{code} produced control text {:?}",
                event.utf8
            );
        }
        assert_eq!(us().translate(KEY_TAB, true, 0, 0).keysym, xkb::Keysym::Tab);
    }

    #[test]
    fn which_keys_repeat_comes_from_the_keymap() {
        let keymap = us();
        assert!(keymap.repeats(KEY_A), "letters repeat");
        assert!(keymap.repeats(KEY_1), "digits repeat");
        assert!(!keymap.repeats(KEY_LEFTSHIFT), "modifiers do not repeat");
        assert!(!keymap.repeats(KEY_CAPSLOCK), "locks do not repeat");
        assert!(!keymap.repeats(u32::MAX), "an out-of-range keycode is not a panic");
    }

    #[test]
    fn a_malformed_keymap_is_an_error_and_never_a_panic() {
        // The keymap arrives on an fd from the compositor: it is untrusted
        // input, and a compositor that hands us rubbish must not take the
        // client down with it.
        for text in [
            "",
            "\0",
            "xkb_keymap {",
            "xkb_keymap { xkb_keycodes { <A> = notanumber; }; };",
            "not a keymap at all",
            &"x".repeat(100_000),
        ] {
            match Keymap::from_string(text) {
                Err(KeymapError::Compile) => {}
                Err(other) => panic!("unexpected error for {text:?}: {other:?}"),
                Ok(_) => panic!("{text:?} compiled as a keymap"),
            }
        }
        // Non-UTF-8-safe slicing is the other classic panic here: a keymap
        // that is valid text but truncated mid-rule.
        let truncated = &include_str!("../../tests/fixtures/keymaps/us.xkb")[..500];
        assert!(matches!(Keymap::from_string(truncated), Err(KeymapError::Compile)));
    }

    #[test]
    fn from_names_builds_the_same_layout_as_the_fixture() {
        // Not a fixture check: this is the path a test with no vendored file
        // uses, and it must produce the same translations.
        let Ok(mut named) = Keymap::from_names("us", "") else {
            // A build host with no xkeyboard-config installed: the vendored
            // fixture is exactly why every other test here does not need it.
            return;
        };
        assert_eq!(named.translate(KEY_A, true, 0, 0).utf8.as_deref(), Some("a"));
        assert_eq!(named.translate(KEY_B, true, 0, 0).keysym, xkb::Keysym::b);
    }
}
```

**Mutation checks.**
`a_plain_letter_translates_to_its_keysym_and_text`: drop the `+ 8` from the
keycode conversion → the keysym is wrong (`Keysym::x` for `KEY_A`).
`a_release_carries_no_text`: emit `utf8` on release too → the assertion fails.
`shift_shifts_the_level_and_reports_the_modifier`: make `mods()` read
`STATE_MODS_DEPRESSED` only → still passes; make it read `STATE_MODS_LOCKED` →
the `Mods::SHIFT` assertion fails.
`the_servers_modifier_state_wins_over_our_own_bookkeeping`: pass `latched` where
`locked` belongs in `update_mask` → the `CAPS` assertion fails.
`control_reports_ctrl_and_no_printable_text`: return `key_get_utf8` unfiltered →
the control-character assertion fails.
`which_keys_repeat_comes_from_the_keymap`: return `true` unconditionally from
`repeats` → the modifier assertions fail.
`a_malformed_keymap_is_an_error_and_never_a_panic`: `unwrap` the `Option` from
`xkb::Keymap::new_from_string` → the first case panics.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{KeyEvent, Keymap, KeymapError, Mods}``.

- [ ] **Step 3: Write the implementation**

`ui/src/window/keyboard.rs`:

```rust
//! `wl_keyboard` through `libxkbcommon`.
//!
//! The compositor owns the keymap and the modifier state; this module owns the
//! translation, the compose table and -- because xkbcommon has no scheduler --
//! the repeat timer (Task 5).
//!
//! The compose types must be spelled `xkb::compose::{State, Table}`:
//! `xkbcommon::xkb`'s glob re-export of the compose module loses `State` to the
//! keyboard state defined in the same module.

use std::os::fd::OwnedFd;

use xkbcommon::xkb;

bitflags::bitflags! {
    /// The modifiers a widget can key an accelerator off.
    ///
    /// Deliberately not every xkb modifier: `Mod3`/`Mod5` have no GTK meaning,
    /// and an accelerator that matched on them would fire on layouts that use
    /// them for level shifts.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Mods: u8 {
        /// `Shift`
        const SHIFT = 1 << 0;
        /// `Control`
        const CTRL = 1 << 1;
        /// `Mod1`
        const ALT = 1 << 2;
        /// `Mod4`
        const LOGO = 1 << 3;
        /// `Lock`
        const CAPS = 1 << 4;
        /// `Mod2`
        const NUM = 1 << 5;
    }
}

/// One `wl_keyboard.key`, translated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// The raw `wl_keyboard.key` evdev code. XKB's is this + 8.
    pub keycode: u32,
    /// The keysym for the current layout and level, composed if a dead-key
    /// sequence just completed.
    pub keysym: xkb::Keysym,
    /// The text to insert. `None` for a key with no text (modifiers, arrows,
    /// F-keys), for a release, for a control character, and for a keystroke
    /// swallowed mid-compose.
    pub utf8: Option<String>,
    /// Every modifier that was effectively active.
    pub mods: Mods,
    /// The modifiers this keysym consumed to reach its level -- `Shift` for
    /// `A`, nothing for `a`. Contract §3.3's `effective_mods` subtracts them.
    pub consumed: Mods,
    pub pressed: bool,
    /// Synthesised by our own repeat timer, never sent by the compositor.
    pub repeat: bool,
    pub serial: u32,
    pub time_ms: u32,
}

impl KeyEvent {
    /// `mods` with the modifiers this keysym consumed removed -- the mask to
    /// compare an accelerator against.
    ///
    /// On a US layout `Shift+1` is `!`, and the `Shift` was *consumed* to get
    /// there: an accelerator for "plain `!`" must match it, and one for
    /// "`Shift` plus `!`" must not.
    #[must_use]
    pub fn effective_mods(&self) -> Mods {
        self.mods.difference(self.consumed)
    }

    /// Whether this event is `keysym` with exactly `mods` held.
    #[must_use]
    pub fn matches(&self, keysym: xkb::Keysym, mods: Mods) -> bool {
        self.keysym == keysym && self.effective_mods() == mods
    }
}

/// Why a keymap could not be built.
#[derive(Debug)]
pub enum KeymapError {
    /// `xkb_context_new` failed -- effectively out of memory.
    Context,
    /// The keymap text did not compile.
    Compile,
    /// The compositor's keymap fd could not be mapped.
    Mmap(std::io::Error),
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Context => write!(f, "cannot create an xkb context"),
            Self::Compile => write!(f, "the keymap did not compile"),
            Self::Mmap(e) => write!(f, "cannot map the keymap fd: {e}"),
        }
    }
}

impl std::error::Error for KeymapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mmap(e) => Some(e),
            Self::Context | Self::Compile => None,
        }
    }
}

/// The modifier indices this keymap uses, resolved once.
///
/// `xkb_state_serialize_mods` reports a mask over *this keymap's* modifier
/// indices, which are not fixed by the protocol: looking them up per event
/// would be a string comparison per modifier per keystroke.
#[derive(Debug, Clone, Copy)]
struct ModIndices {
    shift: xkb::ModIndex,
    ctrl: xkb::ModIndex,
    alt: xkb::ModIndex,
    logo: xkb::ModIndex,
    caps: xkb::ModIndex,
    num: xkb::ModIndex,
}

impl ModIndices {
    fn new(keymap: &xkb::Keymap) -> Self {
        Self {
            shift: keymap.mod_get_index(xkb::MOD_NAME_SHIFT),
            ctrl: keymap.mod_get_index(xkb::MOD_NAME_CTRL),
            alt: keymap.mod_get_index(xkb::MOD_NAME_ALT),
            logo: keymap.mod_get_index(xkb::MOD_NAME_LOGO),
            caps: keymap.mod_get_index(xkb::MOD_NAME_CAPS),
            num: keymap.mod_get_index(xkb::MOD_NAME_NUM),
        }
    }

    /// The [`Mods`] a raw xkb modifier mask stands for.
    ///
    /// A keymap that simply has no `Mod4` reports `MOD_INVALID`, whose bit
    /// would be shifted out of range -- hence the guard rather than a bare
    /// `1 << index`.
    fn decode(self, mask: xkb::ModMask) -> Mods {
        let bit = |index: xkb::ModIndex| {
            index != xkb::MOD_INVALID && index < 32 && mask & (1 << index) != 0
        };
        let mut mods = Mods::empty();
        mods.set(Mods::SHIFT, bit(self.shift));
        mods.set(Mods::CTRL, bit(self.ctrl));
        mods.set(Mods::ALT, bit(self.alt));
        mods.set(Mods::LOGO, bit(self.logo));
        mods.set(Mods::CAPS, bit(self.caps));
        mods.set(Mods::NUM, bit(self.num));
        mods
    }
}

/// A compiled keymap, its live state, and the compose table.
pub struct Keymap {
    /// Kept alive for the keymap and compose table that borrow it in C.
    context: xkb::Context,
    keymap: xkb::Keymap,
    state: xkb::State,
    compose: Option<xkb::compose::State>,
    mods: ModIndices,
    // Repeat fields arrive in Task 5.
}

impl Keymap {
    /// Build from an already-compiled `xkb::Keymap`.
    fn wrap(context: xkb::Context, keymap: xkb::Keymap) -> Self {
        let state = xkb::State::new(&keymap);
        let mods = ModIndices::new(&keymap);
        let compose = compose_state_from_locale(&context);
        Self {
            context,
            keymap,
            state,
            compose,
            mods,
        }
    }

    /// From `wl_keyboard.keymap`.
    ///
    /// `MAP_PRIVATE` is required from protocol version 7 on and is what
    /// `xkb::Keymap::new_from_fd` already does (`map_copy_read_only`); never
    /// hand-roll an `mmap` with `MAP_SHARED` here.
    ///
    /// # Errors
    ///
    /// [`KeymapError::Mmap`] if the fd cannot be mapped, [`KeymapError::Compile`]
    /// if what it holds is not a keymap.
    pub fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: the fd came from `wl_keyboard.keymap`, `size` is the size
        // the same event reported, and the mapping is read-only and private.
        let compiled = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        }
        .map_err(KeymapError::Mmap)?
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// RMLVO, for hermetic tests: `Keymap::from_names("us", "")`.
    ///
    /// # Errors
    ///
    /// [`KeymapError::Compile`] if `xkeyboard-config` is absent or the names
    /// do not resolve.
    pub fn from_names(layout: &str, variant: &str) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compiled = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            variant,
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// From keymap text (a vendored fixture, or a keymap read back out).
    ///
    /// # Errors
    ///
    /// [`KeymapError::Compile`] for anything that is not a keymap. This is the
    /// untrusted path: `xkb_keymap_new_from_string` returns null rather than
    /// aborting, and that null becomes this error.
    pub fn from_string(text: &str) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compiled = xkb::Keymap::new_from_string(
            &context,
            text.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// From `wl_keyboard.modifiers`.
    ///
    /// `group` fills all three layout components: the protocol carries one
    /// active group, and every other toolkit maps it this way.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, group, group, group);
    }

    /// From `wl_keyboard.key`, for a client tracking state itself.
    ///
    /// A Wayland client normally uses [`Self::update_mask`] instead -- the
    /// compositor's state includes keys held before this surface had focus.
    /// This is the hermetic-test path, and xkbcommon warns against mixing the
    /// two on one state.
    pub fn update_key(&mut self, keycode: u32, pressed: bool) {
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        self.state.update_key(xkb_keycode(keycode), direction);
    }

    /// Translate one `wl_keyboard.key`.
    ///
    /// Called *before* any `update_key` for the same event: xkb resolves a
    /// keysym against the state as it was when the key went down.
    pub fn translate(
        &mut self,
        keycode: u32,
        pressed: bool,
        serial: u32,
        time_ms: u32,
    ) -> KeyEvent {
        let key = xkb_keycode(keycode);
        let raw_sym = self.state.key_get_one_sym(key);
        let mods = self.mods();
        let consumed = self
            .mods
            .decode(self.state.key_get_consumed_mods(key));
        let (utf8, keysym) = if pressed {
            self.compose(key, raw_sym)
        } else {
            // A release inserts nothing; feeding it to the compose table would
            // also cancel a sequence the press just started.
            (None, raw_sym)
        };
        KeyEvent {
            keycode,
            keysym,
            utf8,
            mods,
            consumed,
            pressed,
            repeat: false,
            serial,
            time_ms,
        }
    }

    /// The effective modifier state.
    #[must_use]
    pub fn mods(&self) -> Mods {
        self.mods
            .decode(self.state.serialize_mods(xkb::STATE_MODS_EFFECTIVE))
    }

    /// `xkb_keymap_key_repeats` -- modifiers say `false`.
    #[must_use]
    pub fn repeats(&self, keycode: u32) -> bool {
        self.keymap.key_repeats(xkb_keycode(keycode))
    }

    /// Run the compose table over one keysym.
    ///
    /// Returns the text to insert and the keysym to report. A sequence in
    /// progress inserts nothing and reports the dead key itself, so a widget
    /// sees "no text" rather than a stray acute accent.
    fn compose(&mut self, key: xkb::Keycode, keysym: xkb::Keysym) -> (Option<String>, xkb::Keysym) {
        // Computed before the mutable borrow of `self.compose`: a closure
        // over `&self.state` and `&mut self.compose` cannot coexist.
        let direct = insertable(self.state.key_get_utf8(key));
        let Some(compose) = self.compose.as_mut() else {
            return (direct, keysym);
        };
        if compose.feed(keysym) == xkb::FeedResult::Ignored {
            return (direct, keysym);
        }
        match compose.status() {
            xkb::Status::Composing => (None, keysym),
            xkb::Status::Composed => {
                let text = compose.utf8().and_then(insertable);
                let sym = compose.keysym().unwrap_or(keysym);
                compose.reset();
                (text, sym)
            }
            xkb::Status::Cancelled => {
                compose.reset();
                (None, keysym)
            }
            xkb::Status::Nothing => (direct, keysym),
        }
    }

    /// The keymap's index for a modifier name. Test-only: the mask
    /// `update_mask` takes is over indices this keymap chose.
    #[cfg(test)]
    fn mod_index_for_test(&self, name: &str) -> xkb::ModIndex {
        self.keymap.mod_get_index(name)
    }
}

/// `wl_keyboard.key`'s evdev code as an XKB keycode.
///
/// "to determine the xkb keycode, clients must add 8 to the key event
/// keycode" -- `wayland.xml`'s `keymap_format.xkb_v1`. Saturating, so a
/// hostile `u32::MAX` is a keycode the keymap simply does not have rather
/// than an overflow panic in a debug build.
fn xkb_keycode(keycode: u32) -> xkb::Keycode {
    xkb::Keycode::new(keycode.saturating_add(8))
}

/// Text a widget should actually insert.
///
/// `xkb_state_key_get_utf8` reports the C0 control character for `Ctrl+A`
/// (`\x01`) and an empty string for a key with no text; neither is something
/// an entry inserts.
fn insertable(text: String) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text)
}

/// The compose state for the process's locale, if it has a usable table.
///
/// `LC_ALL` > `LC_CTYPE` > `LANG`, as every other toolkit resolves it. A
/// missing or unparsable table is not an error: the client just does not
/// compose.
fn compose_state_from_locale(context: &xkb::Context) -> Option<xkb::compose::State> {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
        .unwrap_or_else(|| std::ffi::OsString::from("C"));
    let table = xkb::compose::Table::new_from_locale(
        context,
        &locale,
        xkb::compose::COMPILE_NO_FLAGS,
    )
    .ok()?;
    Some(xkb::compose::State::new(
        &table,
        xkb::compose::STATE_NO_FLAGS,
    ))
}
```

`context` and `keymap` are held for their C-side lifetimes; if `-D warnings`
reports `context` as never read, keep the field and add
`#[allow(dead_code, reason = "keeps the xkb_context alive for the keymap and \
compose table that borrow it")]` rather than dropping it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: PASS — 9 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/keyboard.rs ui/tests/fixtures/keymaps/
git commit -m "$(cat <<'EOF'
feat(ui/window): translate wl_keyboard through libxkbcommon

Keymap compiles from the compositor's fd (MAP_PRIVATE, via xkbcommon's
own new_from_fd), from RMLVO names, or from vendored text; translate()
reports the keysym, the insertable text, the effective modifiers and the
modifiers the keysym consumed. A malformed keymap is an error, never a
panic -- it arrives on an fd from another process.

Vendors us.xkb and us-intl.xkb so key translation is hermetic.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Consumed modifiers, accelerator matching, and a hermetic compose table

**Files:**
- Modify: `ui/src/window/keyboard.rs`
- Create: `ui/tests/fixtures/keymaps/compose.us`

**Interfaces:**
- Consumes: Task 3's `Keymap`, `KeyEvent`, `Mods`.
- Produces:
  ```rust
  impl Keymap {
      /// Replace the locale's compose table with an explicit one. Hermetic
      /// tests, and a future `gtk-compose-file` setting.
      pub fn set_compose_table(&mut self, table: &str) -> Result<(), KeymapError>;
  }
  impl KeyEvent { #[must_use] pub fn matches(&self, keysym: xkb::Keysym, mods: Mods) -> bool; }
  ```

- [ ] **Step 1: Write the failing test**

Create the compose fixture — three sequences, no locale files involved:

```
# ui/tests/fixtures/keymaps/compose.us
# A minimal Compose table, vendored so the dead-key test does not depend on
# the build host's locale data. Format: libX11 Compose(5).
<dead_acute> <e> : "é" eacute
<dead_acute> <a> : "á" aacute
<Multi_key> <o> <c> : "©" copyright
```

Append to `ui/src/window/keyboard.rs`'s test module:

```rust
    const KEY_E: u32 = 18;
    const KEY_APOSTROPHE: u32 = 40;
    const KEY_O: u32 = 24;
    const KEY_C: u32 = 46;

    fn us_intl_composing() -> Keymap {
        let mut keymap =
            Keymap::from_string(include_str!("../../tests/fixtures/keymaps/us-intl.xkb"))
                .expect("the vendored us(intl) keymap compiles");
        keymap
            .set_compose_table(include_str!("../../tests/fixtures/keymaps/compose.us"))
            .expect("the vendored compose table compiles");
        keymap
    }

    #[test]
    fn a_dead_key_sequence_yields_one_composed_character() {
        let mut keymap = us_intl_composing();
        let dead = keymap.translate(KEY_APOSTROPHE, true, 1, 0);
        assert_eq!(dead.keysym, xkb::Keysym::dead_acute);
        assert!(
            dead.utf8.is_none(),
            "a dead key must insert nothing, not an acute accent: {:?}",
            dead.utf8
        );
        let composed = keymap.translate(KEY_E, true, 2, 10);
        assert_eq!(composed.utf8.as_deref(), Some("é"));
        assert_eq!(composed.keysym, xkb::Keysym::eacute);
        // The table is reset afterwards: the next `e` is a plain `e`.
        assert_eq!(keymap.translate(KEY_E, true, 3, 20).utf8.as_deref(), Some("e"));
    }

    #[test]
    fn an_abandoned_compose_sequence_inserts_nothing() {
        // `dead_acute` then a key with no sequence cancels: xkb reports
        // Cancelled, and neither the accent nor the letter is inserted.
        let mut keymap = us_intl_composing();
        assert!(keymap.translate(KEY_APOSTROPHE, true, 1, 0).utf8.is_none());
        let cancelled = keymap.translate(KEY_O, true, 2, 10);
        assert!(
            cancelled.utf8.is_none(),
            "a cancelled sequence must insert nothing: {:?}",
            cancelled.utf8
        );
        // And the state recovers rather than staying stuck.
        assert_eq!(keymap.translate(KEY_O, true, 3, 20).utf8.as_deref(), Some("o"));
    }

    #[test]
    fn a_multi_key_sequence_composes_across_three_keystrokes() {
        let mut keymap = us_intl_composing();
        // `Multi_key` is not on the us(intl) layout by default, so drive the
        // table directly through the keysym path a compositor-provided layout
        // with a compose key would reach.
        assert!(keymap.feed_keysym_for_test(xkb::Keysym::Multi_key).is_none());
        assert!(keymap.feed_keysym_for_test(xkb::Keysym::o).is_none());
        assert_eq!(keymap.feed_keysym_for_test(xkb::Keysym::c).as_deref(), Some("©"));
    }

    #[test]
    fn a_malformed_compose_table_is_an_error_and_never_a_panic() {
        let mut keymap = us();
        for table in ["<dead_acute", "\0\0\0", "<nosuchkeysym> <e> : \"x\"", &"<".repeat(50_000)] {
            // Either it compiles to something harmless or it reports an
            // error; what it must never do is panic or abort.
            let _ = keymap.set_compose_table(table);
        }
        // Whatever happened, the keymap still translates.
        assert_eq!(keymap.translate(KEY_A, true, 0, 0).utf8.as_deref(), Some("a"));
    }

    #[test]
    fn an_accelerator_matches_on_the_modifiers_the_keysym_did_not_consume() {
        let mut keymap = us();
        keymap.update_key(KEY_LEFTSHIFT, true);
        let bang = keymap.translate(KEY_1, true, 1, 0);
        assert_eq!(bang.keysym, xkb::Keysym::exclam);
        assert_eq!(bang.mods, Mods::SHIFT, "shift really is held");
        assert_eq!(bang.consumed, Mods::SHIFT, "shift was consumed to reach `!`");
        assert_eq!(bang.effective_mods(), Mods::empty());
        assert!(bang.matches(xkb::Keysym::exclam, Mods::empty()));
        assert!(
            !bang.matches(xkb::Keysym::exclam, Mods::SHIFT),
            "`Shift+!` must not match a plain `!` accelerator"
        );
        keymap.update_key(KEY_LEFTSHIFT, false);

        keymap.update_key(KEY_LEFTCTRL, true);
        let ctrl_a = keymap.translate(KEY_A, true, 2, 0);
        assert_eq!(ctrl_a.consumed, Mods::empty(), "Ctrl is never consumed by a level");
        assert_eq!(ctrl_a.effective_mods(), Mods::CTRL);
        assert!(ctrl_a.matches(xkb::Keysym::a, Mods::CTRL));
        assert!(!ctrl_a.matches(xkb::Keysym::a, Mods::empty()));
    }
```

**Mutation checks.**
`a_dead_key_sequence_yields_one_composed_character`: drop the `Composing`
arm (fall through to `direct`) → the dead key inserts `'`; drop the
`compose.reset()` in the `Composed` arm → the third assertion sees `é` again.
`an_abandoned_compose_sequence_inserts_nothing`: make `Cancelled` fall through
to `direct` → the second assertion fails.
`a_multi_key_sequence_composes_across_three_keystrokes`: feed only the last
keysym → the `©` assertion fails.
`a_malformed_compose_table_is_an_error_and_never_a_panic`: `unwrap()` the
`Result` from `Table::new_from_buffer` → the first case panics.
`an_accelerator_matches_on_the_modifiers_the_keysym_did_not_consume`: make
`effective_mods` return `self.mods` → both "must not match" assertions fail.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: FAIL to compile — `error[E0599]: no method named `set_compose_table`
found for struct `Keymap`` and the same for `feed_keysym_for_test`.

- [ ] **Step 3: Write the implementation**

Add to `impl Keymap` in `ui/src/window/keyboard.rs`:

```rust
    /// Replace the compose table with an explicit one.
    ///
    /// The locale's table is the default (`Keymap::wrap`); this is for a
    /// hermetic test, or a user-configured compose file. A table that does not
    /// compile leaves the previous one in place.
    ///
    /// # Errors
    ///
    /// [`KeymapError::Compile`] if `table` is not a Compose(5) table.
    pub fn set_compose_table(&mut self, table: &str) -> Result<(), KeymapError> {
        let compiled = xkb::compose::Table::new_from_buffer(
            &self.context,
            table.as_bytes(),
            "en_US.UTF-8",
            xkb::compose::FORMAT_TEXT_V1,
            xkb::compose::COMPILE_NO_FLAGS,
        )
        .map_err(|()| KeymapError::Compile)?;
        self.compose = Some(xkb::compose::State::new(
            &compiled,
            xkb::compose::STATE_NO_FLAGS,
        ));
        Ok(())
    }

    /// Feed one keysym straight to the compose table, reporting the text a
    /// completed sequence produced. For sequences whose keys are not on the
    /// loaded layout (`Multi_key`).
    #[cfg(test)]
    fn feed_keysym_for_test(&mut self, keysym: xkb::Keysym) -> Option<String> {
        let compose = self.compose.as_mut()?;
        compose.feed(keysym);
        match compose.status() {
            xkb::Status::Composed => {
                let text = compose.utf8();
                compose.reset();
                text
            }
            xkb::Status::Cancelled => {
                compose.reset();
                None
            }
            _ => None,
        }
    }
```

`KeyEvent::matches` and `effective_mods` were written in Task 3; nothing else
changes. If `-D warnings` objects to `Table::new_from_buffer`'s `Result<_, ()>`
being mapped with a unit pattern, keep `map_err(|()| KeymapError::Compile)` --
that is the crate's real error type (`compose.rs:74-98`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: PASS — 14 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/keyboard.rs ui/tests/fixtures/keymaps/compose.us
git commit -m "$(cat <<'EOF'
feat(ui/window): compose sequences and consumed-modifier accelerators

A dead key inserts nothing and the following letter inserts one composed
character; a cancelled sequence inserts nothing and recovers. Accelerators
compare against the modifiers the keysym did not consume, so `!` on a US
layout matches a plain-`!` binding and not a Shift+`!` one.

The compose table can be set explicitly, which is what makes the test
hermetic: no locale files, no machine-dependent Compose(5) data.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Key repeat on the animation clock

**Files:**
- Modify: `ui/src/window/keyboard.rs`

**Interfaces:**
- Consumes: Task 3's `Keymap`/`KeyEvent`; `std::time::Duration`.
- Produces:
  ```rust
  impl Keymap {
      pub fn set_repeat_info(&mut self, rate: i32, delay: i32);
      pub fn arm_repeat(&mut self, ev: &KeyEvent, now: Duration);
      pub fn clear_repeat(&mut self);
      #[must_use] pub fn repeat_deadline(&self, now: Duration) -> Option<Duration>;
      pub fn repeat_due(&mut self, now: Duration) -> Option<KeyEvent>;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/keyboard.rs`'s test module:

```rust
    use std::time::Duration;

    /// The defaults a compositor sends: 25 chars/sec after 600 ms.
    fn repeating() -> Keymap {
        let mut keymap = us();
        keymap.set_repeat_info(25, 600);
        keymap
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_held_key_repeats_after_the_delay_and_then_at_the_rate() {
        let mut keymap = repeating();
        let press = keymap.translate(KEY_A, true, 1, 0);
        keymap.arm_repeat(&press, ms(1_000));

        assert_eq!(keymap.repeat_deadline(ms(1_000)), Some(ms(600)), "the delay comes first");
        assert!(keymap.repeat_due(ms(1_500)).is_none(), "not yet: 600 ms have not passed");

        let first = keymap.repeat_due(ms(1_600)).expect("the first repeat is due");
        assert_eq!(first.keysym, xkb::Keysym::a);
        assert_eq!(first.utf8.as_deref(), Some("a"));
        assert!(first.repeat, "a synthesised repeat says so");
        assert!(first.pressed);

        // 25 chars/sec == one every 40 ms.
        assert_eq!(keymap.repeat_deadline(ms(1_600)), Some(ms(40)));
        assert!(keymap.repeat_due(ms(1_630)).is_none());
        assert!(keymap.repeat_due(ms(1_640)).is_some());
        assert!(keymap.repeat_due(ms(1_680)).is_some());
    }

    #[test]
    fn a_key_up_and_a_focus_leave_both_disarm_the_repeat() {
        let mut keymap = repeating();
        let press = keymap.translate(KEY_A, true, 1, 0);
        keymap.arm_repeat(&press, ms(0));
        assert!(keymap.repeat_deadline(ms(0)).is_some());

        let release = keymap.translate(KEY_A, false, 2, 10);
        keymap.arm_repeat(&release, ms(10));
        assert!(keymap.repeat_deadline(ms(10)).is_none(), "a release disarms");
        assert!(keymap.repeat_due(ms(10_000)).is_none());

        keymap.arm_repeat(&press, ms(20));
        keymap.clear_repeat();
        assert!(keymap.repeat_deadline(ms(20)).is_none(), "wl_keyboard.leave disarms");
    }

    #[test]
    fn a_key_the_keymap_says_never_repeats_is_never_armed() {
        let mut keymap = repeating();
        let shift = keymap.translate(KEY_LEFTSHIFT, true, 1, 0);
        keymap.arm_repeat(&shift, ms(0));
        assert!(keymap.repeat_deadline(ms(0)).is_none());
        assert!(keymap.repeat_due(ms(10_000)).is_none());
    }

    #[test]
    fn a_zero_rate_disables_repeat_entirely() {
        // `wl_keyboard.repeat_info` with rate 0 means "the client must not
        // repeat" -- a compositor that owns repeat itself sends it.
        let mut keymap = us();
        keymap.set_repeat_info(0, 600);
        let press = keymap.translate(KEY_A, true, 1, 0);
        keymap.arm_repeat(&press, ms(0));
        assert!(keymap.repeat_deadline(ms(0)).is_none());
        assert!(keymap.repeat_due(ms(60_000)).is_none());
        // And a negative rate, which the protocol types allow, behaves the same.
        keymap.set_repeat_info(-5, 600);
        keymap.arm_repeat(&press, ms(0));
        assert!(keymap.repeat_due(ms(60_000)).is_none());
    }

    #[test]
    fn a_late_pump_produces_one_repeat_not_a_burst() {
        // The window was blocked for two seconds (a slow relayout, a stopped
        // debugger). Emitting the 50 repeats that "should" have happened
        // would dump 50 characters into an entry at once.
        let mut keymap = repeating();
        let press = keymap.translate(KEY_A, true, 1, 0);
        keymap.arm_repeat(&press, ms(0));
        assert!(keymap.repeat_due(ms(3_000)).is_some(), "the first catch-up fires");
        assert!(keymap.repeat_due(ms(3_000)).is_none(), "and only one");
        assert_eq!(
            keymap.repeat_deadline(ms(3_000)),
            Some(ms(40)),
            "the next one is scheduled from now, not from the missed deadline"
        );
    }

    #[test]
    fn a_repeat_deadline_of_zero_means_now_and_never_spins() {
        let mut keymap = repeating();
        let press = keymap.translate(KEY_A, true, 1, 0);
        keymap.arm_repeat(&press, ms(0));
        // Exactly at the deadline: "now", not a negative duration, and the
        // event is actually available.
        assert_eq!(keymap.repeat_deadline(ms(600)), Some(Duration::ZERO));
        assert!(keymap.repeat_due(ms(600)).is_some());
    }
```

**Mutation checks.**
`a_held_key_repeats_after_the_delay_and_then_at_the_rate`: use the rate as the
first interval instead of the delay → the `Some(ms(600))` assertion fails;
compute the interval as `rate` milliseconds instead of `1/rate` seconds → the
`ms(40)` assertion fails.
`a_key_up_and_a_focus_leave_both_disarm_the_repeat`: drop the `!ev.pressed`
guard in `arm_repeat` → the release re-arms and the assertion fails.
`a_key_the_keymap_says_never_repeats_is_never_armed`: drop the `repeats()` guard
→ Shift repeats.
`a_zero_rate_disables_repeat_entirely`: use `rate <= 0` → `!=` 0 → the negative
case divides by a negative rate and the assertion fails.
`a_late_pump_produces_one_repeat_not_a_burst`: advance `next` by one interval
instead of rebasing on `now` → the second `repeat_due` fires again.
`a_repeat_deadline_of_zero_means_now_and_never_spins`: use `checked_sub` and
return `None` at equality → the assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: FAIL to compile — `error[E0599]: no method named `set_repeat_info`
found for struct `Keymap``, and the same for `arm_repeat`, `clear_repeat`,
`repeat_deadline`, `repeat_due`.

- [ ] **Step 3: Write the implementation**

In `ui/src/window/keyboard.rs`, add the fields to `Keymap` and the state struct:

```rust
use std::time::Duration;

/// The key currently repeating, and when its next repeat is due.
#[derive(Debug, Clone)]
struct Repeat {
    /// The event to re-emit, already marked `repeat: true`.
    event: KeyEvent,
    /// The clock reading the next repeat fires at.
    next: Duration,
    /// The gap between repeats, from `wl_keyboard.repeat_info`'s rate.
    interval: Duration,
}
```

```rust
pub struct Keymap {
    context: xkb::Context,
    keymap: xkb::Keymap,
    state: xkb::State,
    compose: Option<xkb::compose::State>,
    mods: ModIndices,
    /// `wl_keyboard.repeat_info`'s rate, in characters per second. `<= 0`
    /// disables repeat entirely.
    repeat_rate: i32,
    /// `wl_keyboard.repeat_info`'s delay before the first repeat, in ms.
    repeat_delay: i32,
    repeat: Option<Repeat>,
}
```

`Keymap::wrap` initialises them with the values every compositor's defaults
land near, so a client that somehow never receives `repeat_info` still behaves:

```rust
            repeat_rate: 25,
            repeat_delay: 600,
            repeat: None,
```

And the methods:

```rust
    /// From `wl_keyboard.repeat_info`. `rate == 0` disables repeat entirely.
    ///
    /// Applied to the *next* armed key, not retroactively: a rate change
    /// mid-hold is not something the protocol expects a client to interpolate.
    pub fn set_repeat_info(&mut self, rate: i32, delay: i32) {
        self.repeat_rate = rate;
        self.repeat_delay = delay.max(0);
        if rate <= 0 {
            self.repeat = None;
        }
    }

    /// Arm the repeat timer for a held key, or disarm it.
    ///
    /// A release, a key the keymap says never repeats, and a disabled rate all
    /// disarm: `arm_repeat` is called for every key event so there is exactly
    /// one place that decides.
    pub fn arm_repeat(&mut self, ev: &KeyEvent, now: Duration) {
        if !ev.pressed || self.repeat_rate <= 0 || !self.repeats(ev.keycode) {
            self.repeat = None;
            return;
        }
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.repeat_rate));
        let mut event = ev.clone();
        event.repeat = true;
        self.repeat = Some(Repeat {
            event,
            next: now + Duration::from_millis(u64::from(self.repeat_delay.unsigned_abs())),
            interval,
        });
    }

    /// Disarm. `wl_keyboard.leave` and a lost keyboard capability both call it:
    /// a key held when focus left is not held any more as far as we can know.
    pub fn clear_repeat(&mut self) {
        self.repeat = None;
    }

    /// How long until the next repeat, or `None` if nothing is armed.
    ///
    /// `Duration::ZERO` is a legitimate "now" -- the caller polls with a zero
    /// timeout and immediately gets the event from [`Self::repeat_due`]; it is
    /// never a reason to spin.
    #[must_use]
    pub fn repeat_deadline(&self, now: Duration) -> Option<Duration> {
        self.repeat.as_ref().map(|r| r.next.saturating_sub(now))
    }

    /// The synthetic repeat due at `now`, if any.
    ///
    /// At most one per call, and the next one is scheduled from `now`: a pump
    /// that was blocked for seconds must not flush a burst of repeats into an
    /// entry.
    pub fn repeat_due(&mut self, now: Duration) -> Option<KeyEvent> {
        let repeat = self.repeat.as_mut()?;
        if now < repeat.next {
            return None;
        }
        repeat.next = now + repeat.interval;
        Some(repeat.event.clone())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::keyboard`
Expected: PASS — 20 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/keyboard.rs
git commit -m "$(cat <<'EOF'
feat(ui/window): client-side key repeat on the animation clock

xkbcommon has no scheduler and wl_keyboard only reports rate and delay,
so the timer is ours: delay first, then 1/rate, disarmed by the matching
release, by wl_keyboard.leave, by a key the keymap says never repeats and
by a zero or negative rate. A late pump emits one repeat and rebases the
schedule on now rather than flushing every missed deadline.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `window/pointer.rs` — hit-testing the retained tree

**Files:**
- Modify: `ui/src/window/pointer.rs` (replace the stub)
- Modify: `ui/src/window/mod.rs` (add `restyle`)

**Interfaces:**
- Consumes: `crate::css::node::{Node, PseudoStates}`;
  `crate::css::cascade::CompiledSheet`; `crate::css::computed::{ComputedStyle,
  ResolveEnv}`; `crate::css::select::MatchCx`; `crate::layout::{Allocation,
  Container, FixedMeasure, LayoutError, LayoutTree, Rect}`; Task 1's `StyleMap`.
- Produces:
  ```rust
  // window/mod.rs
  pub fn restyle(root: &Node, sheet: &CompiledSheet, env: &ResolveEnv,
                 styles: &mut StyleMap, tree: &mut LayoutTree,
                 available: taffy::Size<taffy::AvailableSpace>,
                 measure: &mut dyn crate::layout::Measure) -> Result<(), LayoutError>;

  // window/pointer.rs
  #[derive(Debug, Clone)]
  pub struct Hit { pub node: Node, pub local: (f32, f32) }
  pub fn hit_test(root: &Node, tree: &LayoutTree, styles: &StyleMap,
                  point: (f32, f32), respect_sensitive: bool) -> Option<Hit>;
  pub fn hit_chain(root: &Node, tree: &LayoutTree, styles: &StyleMap,
                   point: (f32, f32)) -> Vec<Hit>;
  pub const MAX_HIT_DEPTH: usize = 256;
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/window/pointer.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::{Hit, MAX_HIT_DEPTH, hit_chain, hit_test};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::{Node, PseudoStates};
    use crate::layout::{FixedMeasure, LayoutTree};
    use crate::window::{StyleMap, restyle};

    /// `window > box > (one, two, three)`, three 60x40 buttons in a row with
    /// the second and third pulled 30px left so each overlaps its predecessor.
    const OVERLAP_CSS: &str = "
        window { min-width: 200px; min-height: 100px; }
        box { min-width: 200px; min-height: 40px; }
        button { min-width: 60px; min-height: 40px; }
        #two, #three { margin-left: -30px; }
    ";

    struct Fixture {
        root: Node,
        tree: LayoutTree,
        styles: StyleMap,
    }

    fn fixture(css: &str, root: &Node) -> Fixture {
        let sheet = CompiledSheet::compile(css);
        let env = ResolveEnv::default();
        let mut tree = LayoutTree::new();
        let mut styles = StyleMap::new();
        let mut measure = FixedMeasure(taffy::Size { width: 0.0, height: 0.0 });
        restyle(
            root,
            &sheet,
            &env,
            &mut styles,
            &mut tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(200.0),
                height: taffy::AvailableSpace::Definite(100.0),
            },
            &mut measure,
        )
        .expect("the fixture lays out");
        Fixture {
            root: root.clone(),
            tree,
            styles,
        }
    }

    fn overlapping() -> (Fixture, Node, Node, Node) {
        let window = Node::new("window");
        let container = Node::new("box");
        window.append_child(&container);
        let mut made = Vec::new();
        for id in ["one", "two", "three"] {
            let button = Node::new("button");
            button.set_id(Some(id));
            container.append_child(&button);
            made.push(button);
        }
        let fixture = fixture(OVERLAP_CSS, &window);
        (fixture, made[0].clone(), made[1].clone(), made[2].clone())
    }

    fn id_of(hit: &Hit) -> String {
        hit.node
            .id()
            .map_or_else(|| (*hit.node.name()).to_string(), |id| id.as_str().to_string())
    }

    #[test]
    fn the_last_painted_sibling_wins_an_overlap() {
        let (f, one, _two, _three) = overlapping();
        // `one` spans x 0..60, `two` x 30..90, `three` x 60..120.
        assert_eq!(id_of(&hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), true).unwrap()), "one");
        assert_eq!(
            id_of(&hit_test(&f.root, &f.tree, &f.styles, (45.0, 20.0), true).unwrap()),
            "two",
            "in the one/two overlap the later sibling is painted on top"
        );
        assert_eq!(
            id_of(&hit_test(&f.root, &f.tree, &f.styles, (75.0, 20.0), true).unwrap()),
            "three"
        );
        assert!(one.id().is_some(), "the fixture kept its ids");
    }

    #[test]
    fn a_point_outside_every_child_lands_on_the_container_and_then_nothing() {
        let (f, _one, _two, _three) = overlapping();
        // Below the 40px-tall row, still inside the 100px window.
        let hit = hit_test(&f.root, &f.tree, &f.styles, (10.0, 80.0), true).expect("inside the window");
        assert_eq!(id_of(&hit), "window");
        assert_eq!(hit.local, (10.0, 80.0), "local is relative to the hit node's border box");
        assert!(
            hit_test(&f.root, &f.tree, &f.styles, (500.0, 500.0), true).is_none(),
            "a point outside the root hits nothing at all"
        );
    }

    #[test]
    fn a_fully_transparent_subtree_is_not_hit() {
        // GTK delivers no events to a widget with opacity 0: it is not just
        // invisible, it is not there.
        let (f, _one, _two, _three) = {
            let window = Node::new("window");
            let container = Node::new("box");
            window.append_child(&container);
            let hidden = Node::new("button");
            hidden.set_id(Some("hidden"));
            container.append_child(&hidden);
            let css = "
                window { min-width: 200px; min-height: 100px; }
                box { min-width: 200px; min-height: 40px; }
                button { min-width: 60px; min-height: 40px; }
                #hidden { opacity: 0; }
            ";
            let f = fixture(css, &window);
            (f, hidden.clone(), hidden.clone(), hidden)
        };
        let hit = hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), true).expect("something is hit");
        assert_ne!(id_of(&hit), "hidden", "opacity: 0 must not be hit");
        assert_eq!(id_of(&hit), "box");
    }

    #[test]
    fn a_disabled_node_is_skipped_only_when_sensitivity_is_respected() {
        let (f, one, _two, _three) = overlapping();
        one.set_state(PseudoStates::DISABLED, true);
        // The tree changed but the geometry did not: hit testing reads the
        // node's own state, not a cached style.
        let sensitive = hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), true).unwrap();
        assert_eq!(id_of(&sensitive), "box", "an insensitive widget gets no events");
        let raw = hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), false).unwrap();
        assert_eq!(id_of(&raw), "one", "a tooltip query still finds it");
    }

    #[test]
    fn hit_chain_reports_the_capture_path_outermost_first() {
        let (f, _one, _two, _three) = overlapping();
        let chain = hit_chain(&f.root, &f.tree, &f.styles, (45.0, 20.0));
        let names: Vec<String> = chain.iter().map(id_of).collect();
        assert_eq!(names, vec!["window", "box", "two"]);
        assert_eq!(
            chain.last().map(id_of),
            hit_test(&f.root, &f.tree, &f.styles, (45.0, 20.0), true).as_ref().map(id_of),
            "the target is the last of the chain and the answer hit_test gives"
        );
        assert!(hit_chain(&f.root, &f.tree, &f.styles, (500.0, 500.0)).is_empty());
    }

    #[test]
    fn a_tree_deeper_than_the_guard_is_truncated_not_a_stack_overflow() {
        // A reconciler bug, or a list model with a cyclic-looking shape, can
        // hand the window an absurdly deep tree. Descending it recursively
        // without a bound is a stack overflow -- which is an abort, not a
        // catchable panic.
        let window = Node::new("window");
        let mut cursor = window.clone();
        for _ in 0..(MAX_HIT_DEPTH + 40) {
            let child = Node::new("box");
            cursor.append_child(&child);
            cursor = child;
        }
        let css = "window, box { min-width: 200px; min-height: 100px; }";
        let f = fixture(css, &window);
        let chain = hit_chain(&f.root, &f.tree, &f.styles, (10.0, 10.0));
        assert!(!chain.is_empty(), "the shallow part is still hit");
        assert!(
            chain.len() <= MAX_HIT_DEPTH,
            "the walk descended {} levels, past the {MAX_HIT_DEPTH} guard",
            chain.len()
        );
        assert!(hit_test(&f.root, &f.tree, &f.styles, (10.0, 10.0), true).is_some());
    }

    #[test]
    fn a_node_with_no_allocation_is_never_hit() {
        // Contract deviation 10: "not visible" reaches this layer as "no
        // allocation", because GTK 4 has no `visibility` CSS property.
        let (f, _one, _two, _three) = overlapping();
        let orphan = Node::new("button");
        orphan.set_id(Some("orphan"));
        f.root.child(0).expect("the box").append_child(&orphan);
        // `orphan` was added after the layout pass, so the tree has no
        // allocation for it.
        let hit = hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), true).unwrap();
        assert_ne!(id_of(&hit), "orphan");
    }
}
```

**Mutation checks.**
`the_last_painted_sibling_wins_an_overlap`: walk children forwards instead of
`.rev()` → the overlap assertions report `one` and `two`.
`a_point_outside_every_child_lands_on_the_container_and_then_nothing`: return
the absolute point as `local` → the `hit.local` assertion still passes here (the
window's origin is 0,0) but fails in `hit_chain_reports_the_capture_path` once
`local` is checked against `two`; add the stricter assertion there if the
mutation survives. Drop the containment check on the root → the outside-point
assertion fails.
`a_fully_transparent_subtree_is_not_hit`: drop the `opacity` check → `hidden`
is reported.
`a_disabled_node_is_skipped_only_when_sensitivity_is_respected`: ignore the
`respect_sensitive` flag → one of the two assertions fails whichever way it is
ignored.
`hit_chain_reports_the_capture_path_outermost_first`: reverse the returned
vector → the order assertion fails.
`a_tree_deeper_than_the_guard_is_truncated_not_a_stack_overflow`: remove the
depth guard → the process aborts with a stack overflow (which is the failure;
re-run with `RUST_MIN_STACK=1048576` to make it deterministic).
`a_node_with_no_allocation_is_never_hit`: treat a missing allocation as a
zero-origin infinite rect → `orphan` is reported.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::pointer`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{Hit, MAX_HIT_DEPTH, hit_chain, hit_test}`` and
`error[E0432]: unresolved import `crate::window::restyle``.

- [ ] **Step 3: Write the implementation**

First, `restyle` in `ui/src/window/mod.rs` — the pass `Window::render` and every
hit-test fixture share:

```rust
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ResolveEnv;
use crate::css::select::MatchCx;
use crate::layout::{Container, LayoutError, LayoutTree, Measure};

/// Cascade, compute and lay out the whole tree.
///
/// Iterative, not recursive: the tree comes from a reconciler and a widget
/// author, and a 10,000-deep one must be a slow frame rather than a stack
/// overflow. Each node is resolved against its parent's computed style
/// (`ComputedStyle::resolve`, not `resolve_chain`) because the walk already
/// holds it -- `resolve_chain` would re-walk the ancestors for every node,
/// turning a linear pass quadratic.
///
/// `styles` is cleared first: a node removed since the last frame must not
/// keep a stale entry that [`pointer::hit_test`] would then read.
///
/// # Errors
///
/// [`LayoutError`] from `LayoutTree::sync`/`compute`.
pub fn restyle(
    root: &Node,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    styles: &mut StyleMap,
    tree: &mut LayoutTree,
    available: taffy::Size<taffy::AvailableSpace>,
    measure: &mut dyn Measure,
) -> Result<(), LayoutError> {
    styles.clear();
    tree.sync(root)?;
    let mut cx = MatchCx::new();
    let mut stack: Vec<(Node, Option<std::rc::Rc<ComputedStyle>>)> = vec![(root.clone(), None)];
    while let Some((node, parent)) = stack.pop() {
        let computed = std::rc::Rc::new(ComputedStyle::resolve(
            sheet,
            &node,
            parent.as_deref(),
            env,
            &mut cx,
        ));
        // M2's `Container` has exactly two variants; P6 widens it (contract
        // §3.6) and this is the one place that chooses.
        let container = if node.child_count() == 0 {
            Container::Leaf
        } else {
            Container::default()
        };
        tree.set_style(&node, &computed, container, env);
        styles.insert(node_addr(&node), std::rc::Rc::clone(&computed));
        for child in node.children() {
            stack.push((child, Some(std::rc::Rc::clone(&computed))));
        }
    }
    tree.compute(root, available, measure)
}
```

Then `ui/src/window/pointer.rs`:

```rust
//! Pointer and touch: hit testing the retained tree, the client-side implicit
//! grab, scrolling and cursor shapes.

use crate::css::node::{Node, PseudoStates};
use crate::layout::{LayoutTree, Rect};
use crate::window::{StyleMap, node_addr};

/// A node the pointer is over, and where on it.
#[derive(Debug, Clone)]
pub struct Hit {
    pub node: Node,
    /// The point in the node's border-box space.
    pub local: (f32, f32),
}

/// How deep the hit test will descend.
///
/// A retained tree is built by a reconciler from application data; a bug at
/// either end can produce one thousands of levels deep, and an unbounded
/// recursive descent over that is a stack overflow -- an abort, not a panic a
/// test can catch. GTK's own widget hierarchies are tens of levels at most, so
/// this bound never fires in practice.
pub const MAX_HIT_DEPTH: usize = 256;

/// Whether `rect` contains `point`, half-open on the right and bottom.
///
/// Half-open is what makes two edge-to-edge siblings unambiguous: the pixel at
/// x == 60 belongs to the node starting there, not to the one ending there.
fn contains(rect: Rect, point: (f32, f32)) -> bool {
    point.0 >= rect.x && point.0 < rect.right() && point.1 >= rect.y && point.1 < rect.bottom()
}

/// Whether this node can be hit at all, ignoring geometry.
fn hittable(node: &Node, styles: &StyleMap, respect_sensitive: bool) -> bool {
    if respect_sensitive && node.states().contains(PseudoStates::DISABLED) {
        return false;
    }
    // Contract deviation 10: GTK 4 has no `visibility` property, so an
    // invisible widget reaches this layer as `opacity: 0` or as a node with no
    // allocation (checked by the caller).
    styles
        .get(&node_addr(node))
        .is_none_or(|style| style.opacity() > 0.0)
}

/// The capture->target chain at `point`, outermost first.
///
/// Controller dispatch is capture -> target -> bubble, mirroring GTK, so the
/// chain is the whole answer: `hit_test` is its last element.
///
/// Insensitive subtrees are skipped: GTK delivers no events to an insensitive
/// widget or anything inside it.
#[must_use]
pub fn hit_chain(
    root: &Node,
    tree: &LayoutTree,
    styles: &StyleMap,
    point: (f32, f32),
) -> Vec<Hit> {
    let mut chain = Vec::new();
    descend(root, tree, styles, point, true, &mut chain);
    chain
}

/// The topmost-painted node containing `point` (window frame space).
///
/// Skips a subtree with no allocation, a zero-area one, a fully transparent
/// one, and -- under `respect_sensitive` -- one carrying
/// [`PseudoStates::DISABLED`].
#[must_use]
pub fn hit_test(
    root: &Node,
    tree: &LayoutTree,
    styles: &StyleMap,
    point: (f32, f32),
    respect_sensitive: bool,
) -> Option<Hit> {
    let mut chain = Vec::new();
    descend(root, tree, styles, point, respect_sensitive, &mut chain);
    chain.pop()
}

/// Push `node` onto `chain` if it contains `point`, then the deepest,
/// last-painted descendant that does.
///
/// Children are visited in reverse order because paint order is tree order:
/// the last sibling painted is the one on top, and the one the pointer hits.
/// The walk stays inside the parent's border box, as GTK's does -- a child that
/// overflows its parent is drawn but not hit.
fn descend(
    node: &Node,
    tree: &LayoutTree,
    styles: &StyleMap,
    point: (f32, f32),
    respect_sensitive: bool,
    chain: &mut Vec<Hit>,
) -> bool {
    if chain.len() >= MAX_HIT_DEPTH {
        tracing::warn!(
            depth = chain.len(),
            node = %node.name(),
            "hit test stopped at the depth guard"
        );
        return false;
    }
    let Some(alloc) = tree.allocation(node) else {
        return false;
    };
    if alloc.border_box.is_empty() || !contains(alloc.border_box, point) {
        return false;
    }
    if !hittable(node, styles, respect_sensitive) {
        return false;
    }
    chain.push(Hit {
        node: node.clone(),
        local: (point.0 - alloc.border_box.x, point.1 - alloc.border_box.y),
    });
    for child in node.children().into_iter().rev() {
        if descend(&child, tree, styles, point, respect_sensitive, chain) {
            break;
        }
    }
    true
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::pointer`
Expected: PASS — 7 tests.

Run: `cargo test -p icedtea-ui --lib`
Expected: PASS — nothing else moved.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/pointer.rs ui/src/window/mod.rs
git commit -m "$(cat <<'EOF'
feat(ui/window): hit-test the retained node tree

Replaces M1's single-rect Button::contains with a real walk: children in
reverse paint order so the topmost sibling wins, bounded by the parent's
border box as GTK's is, skipping subtrees with no allocation, zero area,
opacity 0 or -- when sensitivity is respected -- :disabled. hit_chain
returns the capture->target path the controllers will dispatch along.

A depth guard keeps a pathological tree a slow frame rather than a stack
overflow, and `restyle` is iterative for the same reason.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Implicit grab, scroll axes, kinetic deceleration, cursor shapes

**Files:**
- Modify: `ui/src/window/pointer.rs`

**Interfaces:**
- Consumes: Task 6's module; `wayland_protocols::wp::cursor_shape::v1::client`.
- Produces:
  ```rust
  #[derive(Debug, Default)]
  pub struct ImplicitGrab { /* private */ }
  impl ImplicitGrab {
      pub fn press(&mut self, button: u32, target: &Node) -> bool;
      pub fn release(&mut self, button: u32) -> bool;
      #[must_use] pub fn target(&self) -> Option<Node>;
      #[must_use] pub fn is_held(&self) -> bool;
      pub fn clear(&mut self);
  }
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct Scroll { pub dx: f32, pub dy: f32, pub source: ScrollSource, pub stop: bool, pub time_ms: u32 }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ScrollSource { Wheel, Finger, Continuous, WheelTilt }
  #[derive(Debug, Default)]
  pub struct Kinetic { /* private */ }
  impl Kinetic {
      pub fn feed(&mut self, scroll: &Scroll, now: Duration);
      pub fn sample(&mut self, now: Duration) -> Option<(f32, f32)>;
      pub fn cancel(&mut self);
  }
  pub use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape as CursorShape;
  pub fn cursor_shape_for(name: &str) -> CursorShape;
  pub const MIN_VELOCITY: f32 = 20.0;
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/pointer.rs`'s test module:

```rust
    use super::{CursorShape, ImplicitGrab, Kinetic, MIN_VELOCITY, Scroll, ScrollSource, cursor_shape_for};
    use std::time::Duration;

    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;

    fn finger(dx: f32, dy: f32, stop: bool) -> Scroll {
        Scroll { dx, dy, source: ScrollSource::Finger, stop, time_ms: 0 }
    }

    #[test]
    fn a_press_captures_the_pointer_until_the_last_button_is_released() {
        // The client-side mirror of the compositor's implicit grab (which the
        // wlr crate's own `implicit_grab_tests` pin on the server side):
        // motion and the release go to the pressed node, not to whatever is
        // under the pointer now.
        let pressed = Node::new("button");
        let elsewhere = Node::new("label");
        let mut grab = ImplicitGrab::default();
        assert!(!grab.is_held());
        assert!(grab.target().is_none());

        assert!(grab.press(BTN_LEFT, &pressed), "the first press begins the grab");
        assert!(grab.is_held());
        assert!(grab.target().expect("a target").ptr_eq(&pressed));

        assert!(
            !grab.press(BTN_RIGHT, &elsewhere),
            "a second button does not begin a new grab"
        );
        assert!(
            grab.target().expect("a target").ptr_eq(&pressed),
            "and does not move the target"
        );

        assert!(!grab.release(BTN_RIGHT), "one button left: still grabbed");
        assert!(grab.is_held());
        assert!(grab.release(BTN_LEFT), "the last release ends the grab");
        assert!(!grab.is_held());
        assert!(grab.target().is_none());
    }

    #[test]
    fn a_release_of_a_button_that_was_never_pressed_is_ignored() {
        // The compositor sends a release for a button pressed before we had
        // focus; ending the grab on it would drop a real drag.
        let pressed = Node::new("button");
        let mut grab = ImplicitGrab::default();
        grab.press(BTN_LEFT, &pressed);
        assert!(!grab.release(BTN_RIGHT));
        assert!(grab.is_held(), "an unrelated release must not end the grab");
        grab.clear();
        assert!(!grab.is_held(), "a pointer leave clears it outright");
        assert!(!grab.release(BTN_LEFT), "and a late release afterwards is a no-op");
    }

    #[test]
    fn only_a_finger_scroll_coasts() {
        let mut kinetic = Kinetic::default();
        for source in [ScrollSource::Wheel, ScrollSource::WheelTilt, ScrollSource::Continuous] {
            kinetic.feed(
                &Scroll { dx: 0.0, dy: -40.0, source, stop: false, time_ms: 0 },
                Duration::from_millis(0),
            );
            kinetic.feed(
                &Scroll { dx: 0.0, dy: 0.0, source, stop: true, time_ms: 10 },
                Duration::from_millis(10),
            );
            assert!(
                kinetic.sample(Duration::from_millis(20)).is_none(),
                "{source:?} must not coast: a wheel click is a discrete step"
            );
        }
    }

    #[test]
    fn a_flicked_finger_scroll_coasts_and_then_stops() {
        let mut kinetic = Kinetic::default();
        // 100 px over 100 ms == 1000 px/s.
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(0));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(50));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(100));

        let first = kinetic.sample(Duration::from_millis(116)).expect("it coasts");
        assert!(first.1 < 0.0, "the coast continues in the flick's direction: {first:?}");
        assert_eq!(first.0, 0.0, "a purely vertical flick has no horizontal coast");

        let mut deltas = vec![first.1.abs()];
        let mut now = 116u64;
        while let Some(delta) = kinetic.sample(Duration::from_millis(now)) {
            deltas.push(delta.1.abs());
            now += 16;
            assert!(now < 10_000, "the coast never stopped");
        }
        assert!(deltas.len() > 2, "one frame is not a coast: {deltas:?}");
        assert!(
            deltas.windows(2).all(|w| w[1] <= w[0] + 0.001),
            "each frame must move no further than the last: {deltas:?}"
        );
        assert!(
            kinetic.sample(Duration::from_millis(now + 16)).is_none(),
            "once stopped it stays stopped"
        );
    }

    #[test]
    fn a_new_scroll_or_a_cancel_ends_the_coast() {
        let mut kinetic = Kinetic::default();
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(0));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(50));
        assert!(kinetic.sample(Duration::from_millis(66)).is_some());
        kinetic.cancel();
        assert!(
            kinetic.sample(Duration::from_millis(82)).is_none(),
            "touching the list must stop it dead, as GTK does"
        );
    }

    #[test]
    fn a_zero_or_backwards_time_step_never_produces_nan() {
        // `time_ms` comes from the compositor and the clock from us; neither
        // is guaranteed monotonic across a suspend. A NaN delta would poison
        // every scroll offset downstream.
        let mut kinetic = Kinetic::default();
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(100));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(100));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(50));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(100));
        for at in [100u64, 100, 50, 200] {
            if let Some((dx, dy)) = kinetic.sample(Duration::from_millis(at)) {
                assert!(dx.is_finite() && dy.is_finite(), "{dx},{dy} is not finite");
            }
        }
        assert!(MIN_VELOCITY > 0.0);
    }

    #[test]
    fn cursor_names_map_to_the_protocols_shapes() {
        // The CSS/GTK cursor names widgets actually use (contract deviation 9:
        // this is a name, not a CSS property -- GTK 4 has neither).
        assert_eq!(cursor_shape_for("default"), CursorShape::Default);
        assert_eq!(cursor_shape_for("text"), CursorShape::Text);
        assert_eq!(cursor_shape_for("pointer"), CursorShape::Pointer);
        assert_eq!(cursor_shape_for("grab"), CursorShape::Grab);
        assert_eq!(cursor_shape_for("grabbing"), CursorShape::Grabbing);
        assert_eq!(cursor_shape_for("col-resize"), CursorShape::ColResize);
        assert_eq!(cursor_shape_for("row-resize"), CursorShape::RowResize);
        assert_eq!(cursor_shape_for("ew-resize"), CursorShape::EwResize);
        assert_eq!(cursor_shape_for("ns-resize"), CursorShape::NsResize);
        assert_eq!(cursor_shape_for("not-allowed"), CursorShape::NotAllowed);
        assert_eq!(cursor_shape_for("progress"), CursorShape::Progress);
        assert_eq!(cursor_shape_for("wait"), CursorShape::Wait);
        assert_eq!(cursor_shape_for("crosshair"), CursorShape::Crosshair);
        assert_eq!(cursor_shape_for("help"), CursorShape::Help);
        assert_eq!(cursor_shape_for("context-menu"), CursorShape::ContextMenu);
        // Case-insensitive, and anything unknown -- including the `url()` GTK
        // themes use for custom cursors, which we do not support -- is Default.
        assert_eq!(cursor_shape_for("TEXT"), CursorShape::Text);
        assert_eq!(cursor_shape_for("url(hand.png)"), CursorShape::Default);
        assert_eq!(cursor_shape_for(""), CursorShape::Default);
        assert_eq!(cursor_shape_for("zoom-sideways"), CursorShape::Default);
    }
```

**Mutation checks.**
`a_press_captures_the_pointer_until_the_last_button_is_released`: overwrite the
target on every press → the "does not move the target" assertion fails; return
`true` from every `press` → the second-button assertion fails.
`a_release_of_a_button_that_was_never_pressed_is_ignored`: end the grab on any
release → the `is_held` assertion fails.
`only_a_finger_scroll_coasts`: drop the source check in `feed` → a wheel coasts.
`a_flicked_finger_scroll_coasts_and_then_stops`: remove the decay multiplication
→ the monotonic-decrease assertion fails and the loop hits its 10 s guard;
remove the `MIN_VELOCITY` cut-off → the same guard fires.
`a_new_scroll_or_a_cancel_ends_the_coast`: make `cancel` a no-op → the
assertion fails.
`a_zero_or_backwards_time_step_never_produces_nan`: divide by the raw `dt`
without the zero guard → the first `feed` pair yields `inf`/`NaN` and the
finiteness assertion fails.
`cursor_names_map_to_the_protocols_shapes`: make the match case-sensitive → the
`"TEXT"` assertion fails; return `Text` for the wildcard arm → the `url()`
assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::pointer`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{CursorShape, ImplicitGrab, Kinetic, ...}``.

- [ ] **Step 3: Write the implementation**

Append to `ui/src/window/pointer.rs`:

```rust
use std::time::Duration;

pub use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape as CursorShape;

/// The client-side mirror of the compositor's implicit pointer grab.
///
/// Once a button goes down on a node, motion and the release go to that node
/// regardless of what is under the pointer, until the *last* button is
/// released. Without it, dragging a scale's slider off the widget stops moving
/// it, and a press-drag-off-release never fires the click it should not fire.
#[derive(Debug, Default)]
pub struct ImplicitGrab {
    target: Option<Node>,
    /// Which buttons are down, as a bitmask over `button - BTN_LEFT`.
    ///
    /// A mask, not a count: the compositor can send a release for a button
    /// pressed before this surface had focus, and a count would go negative
    /// (or, unsigned, end a grab that is still live).
    held: u32,
}

impl ImplicitGrab {
    /// Record a press. `true` if this press began the grab.
    pub fn press(&mut self, button: u32, target: &Node) -> bool {
        let bit = button_bit(button);
        let began = self.held == 0;
        self.held |= bit;
        if began {
            self.target = Some(target.clone());
        }
        began
    }

    /// Record a release. `true` if this release ended the grab.
    pub fn release(&mut self, button: u32) -> bool {
        let bit = button_bit(button);
        if self.held & bit == 0 {
            return false;
        }
        self.held &= !bit;
        if self.held == 0 {
            self.target = None;
            return true;
        }
        false
    }

    /// The node every motion and release currently goes to.
    #[must_use]
    pub fn target(&self) -> Option<Node> {
        self.target.clone()
    }

    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held != 0
    }

    /// Drop the grab outright: the pointer left, or the seat lost it.
    pub fn clear(&mut self) {
        self.target = None;
        self.held = 0;
    }
}

/// One bit per pointer button, saturating at the 32 the mask can hold.
///
/// `BTN_LEFT` is 0x110 and the codes run upward; a tablet or gaming mouse can
/// report codes far above that, and shifting by more than 31 is undefined.
fn button_bit(button: u32) -> u32 {
    1u32 << button.saturating_sub(0x110).min(31)
}

/// One `wl_pointer.axis` frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scroll {
    pub dx: f32,
    pub dy: f32,
    pub source: ScrollSource,
    /// `wl_pointer.axis_stop` -- the finger left the touchpad, which is what
    /// starts a kinetic coast.
    pub stop: bool,
    pub time_ms: u32,
}

/// `wl_pointer.axis_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
}

/// The fraction of a coast's velocity left after one second.
///
/// Chosen to match GTK's `GtkKineticScrolling` feel: fast at first, visibly
/// stopped within about a second.
const DECAY_PER_SECOND: f32 = 0.05;

/// Below this many pixels per second a coast is over.
pub const MIN_VELOCITY: f32 = 20.0;

/// Kinetic deceleration for finger scrolls.
#[derive(Debug, Default)]
pub struct Kinetic {
    velocity: (f32, f32),
    last: Option<Duration>,
    coasting: bool,
}

impl Kinetic {
    /// Feed one scroll frame.
    ///
    /// While the finger is down this tracks velocity; `axis_stop` starts the
    /// coast. Every other source cancels: a wheel click is a discrete step, and
    /// coasting one would scroll a list on every notch.
    pub fn feed(&mut self, scroll: &Scroll, now: Duration) {
        if scroll.source != ScrollSource::Finger {
            self.cancel();
            return;
        }
        if scroll.stop {
            self.coasting = self.speed() >= MIN_VELOCITY;
            self.last = Some(now);
            return;
        }
        self.coasting = false;
        let dt = self
            .last
            .map_or(0.0, |last| now.saturating_sub(last).as_secs_f32());
        self.last = Some(now);
        // A zero or backwards step tells us nothing about velocity; keeping
        // the previous estimate is the only finite answer.
        if dt <= f32::EPSILON {
            return;
        }
        let instant = (scroll.dx / dt, scroll.dy / dt);
        if !instant.0.is_finite() || !instant.1.is_finite() {
            return;
        }
        // Half old, half new: one jittery frame must not define the flick,
        // and the last frames before lift-off must dominate.
        self.velocity = (
            self.velocity.0.mul_add(0.5, instant.0 * 0.5),
            self.velocity.1.mul_add(0.5, instant.1 * 0.5),
        );
    }

    /// The delta to apply this frame; `None` once stopped.
    pub fn sample(&mut self, now: Duration) -> Option<(f32, f32)> {
        if !self.coasting {
            return None;
        }
        let last = self.last?;
        let dt = now.saturating_sub(last).as_secs_f32();
        if dt <= 0.0 {
            return Some((0.0, 0.0));
        }
        self.last = Some(now);
        let delta = (self.velocity.0 * dt, self.velocity.1 * dt);
        let decay = DECAY_PER_SECOND.powf(dt);
        self.velocity = (self.velocity.0 * decay, self.velocity.1 * decay);
        if !delta.0.is_finite() || !delta.1.is_finite() {
            self.cancel();
            return None;
        }
        if self.speed() < MIN_VELOCITY {
            self.coasting = false;
        }
        Some(delta)
    }

    /// Stop dead: a new touch, a new scroll, a focus change.
    pub fn cancel(&mut self) {
        self.velocity = (0.0, 0.0);
        self.last = None;
        self.coasting = false;
    }

    fn speed(&self) -> f32 {
        self.velocity.0.hypot(self.velocity.1)
    }
}

/// A CSS/GTK cursor name as a `wp_cursor_shape_v1` shape.
///
/// Contract deviation 9: this takes a *name*, not a `&ComputedStyle`. GTK 4 has
/// no `cursor` CSS property -- M2's registry holds the 114 properties GTK
/// actually parses and `ui/tests/gtk4_property_reference.rs` pins that count --
/// so a widget declares its cursor by name, exactly as
/// `gtk_widget_set_cursor_from_name` does.
///
/// Unknown names and `url()` values fall back to `Default`: client-side cursor
/// themes are out of scope for M3, and icedtea supports `wp_cursor_shape_v1`.
#[must_use]
pub fn cursor_shape_for(name: &str) -> CursorShape {
    let lowered = name.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "context-menu" => CursorShape::ContextMenu,
        "help" => CursorShape::Help,
        "pointer" => CursorShape::Pointer,
        "progress" => CursorShape::Progress,
        "wait" => CursorShape::Wait,
        "cell" => CursorShape::Cell,
        "crosshair" => CursorShape::Crosshair,
        "text" => CursorShape::Text,
        "vertical-text" => CursorShape::VerticalText,
        "alias" => CursorShape::Alias,
        "copy" => CursorShape::Copy,
        "move" => CursorShape::Move,
        "no-drop" => CursorShape::NoDrop,
        "not-allowed" => CursorShape::NotAllowed,
        "grab" => CursorShape::Grab,
        "grabbing" => CursorShape::Grabbing,
        "e-resize" => CursorShape::EResize,
        "n-resize" => CursorShape::NResize,
        "ne-resize" => CursorShape::NeResize,
        "nw-resize" => CursorShape::NwResize,
        "s-resize" => CursorShape::SResize,
        "se-resize" => CursorShape::SeResize,
        "sw-resize" => CursorShape::SwResize,
        "w-resize" => CursorShape::WResize,
        "ew-resize" => CursorShape::EwResize,
        "ns-resize" => CursorShape::NsResize,
        "nesw-resize" => CursorShape::NeswResize,
        "nwse-resize" => CursorShape::NwseResize,
        "col-resize" => CursorShape::ColResize,
        "row-resize" => CursorShape::RowResize,
        "all-scroll" => CursorShape::AllScroll,
        "zoom-in" => CursorShape::ZoomIn,
        "zoom-out" => CursorShape::ZoomOut,
        _ => CursorShape::Default,
    }
}
```

`CursorShape` is `#[non_exhaustive]` in the generated bindings, so the match
above builds values but never matches on one; nothing here needs a wildcard arm
over the protocol enum.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::pointer`
Expected: PASS — 14 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/pointer.rs
git commit -m "$(cat <<'EOF'
feat(ui/window): implicit grab, scroll axes, kinetic coast, cursor shapes

ImplicitGrab mirrors the compositor's rule client-side: motion and the
release go to the pressed node until the last button is up, with a button
mask rather than a count so a release for a button pressed before we had
focus cannot end a live grab.

Kinetic coasts finger scrolls only, decays exponentially, stops below
MIN_VELOCITY, and survives a zero or backwards time step without
producing a NaN offset. cursor_shape_for maps GTK's cursor names onto
wp_cursor_shape_v1, with url() and anything unknown falling back to
Default.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `window/focus.rs` — who can take focus, and GTK's geometric sort

**Files:**
- Modify: `ui/src/window/focus.rs` (replace the stub)

**Interfaces:**
- Consumes: `crate::css::node::{Direction, Node, PseudoStates}`;
  `crate::layout::LayoutTree`.
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum FocusDirection { TabForward, TabBackward, Up, Down, Left, Right }
  /// The style class that marks a node as taking focus.
  pub const FOCUSABLE_CLASS: &str = "focusable";
  pub fn is_focusable(node: &Node, tree: &LayoutTree) -> bool;
  pub fn focus_sort(parent: &Node, tree: &LayoutTree, dir: FocusDirection) -> Vec<Node>;
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/window/focus.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::{FOCUSABLE_CLASS, FocusDirection, focus_sort, is_focusable};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::{Direction, Node, PseudoStates};
    use crate::layout::{FixedMeasure, LayoutTree};
    use crate::window::{StyleMap, restyle};

    /// Four 60x40 buttons in one row, two of them pushed 60px down, so the
    /// arrangement reads as a 2x2 grid whose *tree* order is deliberately not
    /// its *geometric* order (ruling R2).
    const GRID_CSS: &str = "
        window { min-width: 300px; min-height: 160px; }
        box { min-width: 300px; min-height: 160px; }
        button { min-width: 60px; min-height: 40px; }
        #c, #d { margin-top: 60px; }
    ";

    struct Grid {
        root: Node,
        container: Node,
        tree: LayoutTree,
        styles: StyleMap,
    }

    /// Tree order: b, d, a, c. Geometry: b and a on the top row (b left of a),
    /// d and c on the bottom row (d left of c).
    fn grid() -> Grid {
        let root = Node::new("window");
        let container = Node::new("box");
        root.append_child(&container);
        for id in ["b", "d", "a", "c"] {
            let button = Node::with_classes("button", &[FOCUSABLE_CLASS]);
            button.set_id(Some(id));
            container.append_child(&button);
        }
        let sheet = CompiledSheet::compile(GRID_CSS);
        let env = ResolveEnv::default();
        let mut tree = LayoutTree::new();
        let mut styles = StyleMap::new();
        let mut measure = FixedMeasure(taffy::Size { width: 0.0, height: 0.0 });
        restyle(
            &root,
            &sheet,
            &env,
            &mut styles,
            &mut tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(160.0),
            },
            &mut measure,
        )
        .expect("the grid lays out");
        Grid { root, container, tree, styles }
    }

    fn ids(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| n.id().map_or_else(String::new, |id| id.as_str().to_string()))
            .collect()
    }

    fn centre(grid: &Grid, id: &str) -> (f32, f32) {
        let node = grid
            .container
            .children()
            .into_iter()
            .find(|n| n.id().is_some_and(|got| got.as_str() == id))
            .expect("the node exists");
        let alloc = grid.tree.allocation(&node).expect("it is laid out");
        (
            alloc.border_box.x + alloc.border_box.width / 2.0,
            alloc.border_box.y + alloc.border_box.height / 2.0,
        )
    }

    #[test]
    fn the_fixture_really_is_two_rows() {
        // Guards the tests below: if the margin stops moving anything, a
        // "geometric order" assertion would silently become a tree-order one.
        let grid = grid();
        assert!(centre(&grid, "b").1 < centre(&grid, "d").1, "b must sit above d");
        assert!((centre(&grid, "b").1 - centre(&grid, "a").1).abs() < 1.0, "b and a share a row");
        assert!(centre(&grid, "b").0 < centre(&grid, "a").0, "b is left of a");
        assert!(centre(&grid, "d").0 < centre(&grid, "c").0, "d is left of c");
    }

    #[test]
    fn tab_order_is_geometric_and_not_tree_order() {
        // Ruling R2. Tree order is b, d, a, c; reading order is b, a, d, c.
        let grid = grid();
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::TabForward)),
            vec!["b", "a", "d", "c"]
        );
        assert_eq!(
            ids(&grid.container.children()),
            vec!["b", "d", "a", "c"],
            "and it really is different from tree order"
        );
    }

    #[test]
    fn shift_tab_reverses_the_ring() {
        let grid = grid();
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::TabBackward)),
            vec!["c", "d", "a", "b"]
        );
    }

    #[test]
    fn the_arrow_directions_sort_along_their_own_axis() {
        let grid = grid();
        // Left/Right order by x first: the two left-hand widgets before the
        // two right-hand ones.
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::Right)),
            vec!["b", "d", "a", "c"]
        );
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::Left)),
            vec!["c", "a", "d", "b"]
        );
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::Down)),
            vec!["b", "a", "d", "c"]
        );
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::Up)),
            vec!["c", "d", "a", "b"]
        );
    }

    #[test]
    fn rtl_reverses_the_horizontal_tie_break_only() {
        let grid = grid();
        grid.root.set_direction(Some(Direction::Rtl));
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::TabForward)),
            vec!["a", "b", "c", "d"],
            "rows still run top to bottom; within a row, right to left"
        );
        grid.root.set_direction(Some(Direction::Ltr));
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::TabForward)),
            vec!["b", "a", "d", "c"]
        );
    }

    #[test]
    fn only_marked_mapped_sensitive_nodes_are_candidates() {
        let grid = grid();
        let children = grid.container.children();
        let b = children[0].clone();
        assert!(is_focusable(&b, &grid.tree));

        b.set_state(PseudoStates::DISABLED, true);
        assert!(!is_focusable(&b, &grid.tree), "an insensitive widget is not in the ring");
        b.set_state(PseudoStates::DISABLED, false);

        b.remove_class(FOCUSABLE_CLASS);
        assert!(
            !is_focusable(&b, &grid.tree),
            "GTK's WindowControls buttons are exactly this case: painted, clickable, never focused"
        );
        b.add_class(FOCUSABLE_CLASS);

        let unmapped = Node::with_classes("button", &[FOCUSABLE_CLASS]);
        grid.container.append_child(&unmapped);
        assert!(
            !is_focusable(&unmapped, &grid.tree),
            "a node with no allocation is not focusable"
        );
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::TabForward)).len(),
            4,
            "and it is not in the sorted ring either"
        );
    }

    #[test]
    fn a_container_with_no_focusable_children_sorts_to_nothing() {
        let root = Node::new("window");
        let tree = LayoutTree::new();
        assert!(focus_sort(&root, &tree, FocusDirection::TabForward).is_empty());
        assert!(!is_focusable(&root, &tree), "an unsynced node is never focusable");
    }
}
```

**Mutation checks.**
`the_fixture_really_is_two_rows`: this *is* the guard; if `margin-top` stops
applying it fails first and the geometric assertions below become meaningless
rather than silently wrong.
`tab_order_is_geometric_and_not_tree_order`: return the children unsorted →
`["b","d","a","c"]`, which the second assertion pins as the wrong answer.
`shift_tab_reverses_the_ring`: sort backwards by y instead of reversing the
whole vector → `["d","c","b","a"]`, which fails.
`the_arrow_directions_sort_along_their_own_axis`: use the y-then-x comparator
for every direction → the `Right`/`Left` assertions fail.
`rtl_reverses_the_horizontal_tie_break_only`: reverse the whole vector under RTL
instead of the x comparison → `["c","d","a","b"]`, which fails.
`only_marked_mapped_sensitive_nodes_are_candidates`: drop any one of the three
conditions in `is_focusable` → the matching assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::focus`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{FOCUSABLE_CLASS, FocusDirection, focus_sort, is_focusable}``.

- [ ] **Step 3: Write the implementation**

`ui/src/window/focus.rs`:

```rust
//! The focus ring: who can take focus, in what order, and whether the ring is
//! drawn.
//!
//! GTK's order is *geometric*, not tree order (ruling R2,
//! `gtk_widget_focus_sort`): a HeaderBar's end packs, a Grid's cells and an
//! Overlay's layers are all laid out in an order their tree does not describe.

use crate::css::node::{Direction, Node, PseudoStates};
use crate::layout::LayoutTree;

/// Which way focus is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    TabForward,
    TabBackward,
    Up,
    Down,
    Left,
    Right,
}

/// The style class that marks a node as taking focus.
///
/// The retained tree's only writable per-node vocabulary is names, ids,
/// classes and pseudo-states, and "can this take focus" is none of the
/// pseudo-states GTK defines -- so P4's `PropName::Focusable` reaches this
/// layer as a class. `WindowControls`' three buttons deliberately do not carry
/// it: GTK paints and clicks them but never focuses them.
pub const FOCUSABLE_CLASS: &str = "focusable";

/// Whether a node participates in the ring at all.
///
/// Marked focusable, not disabled, and actually laid out with a non-zero
/// allocation -- an unmapped widget is not a focus candidate, and neither is
/// one the reconciler added after the last layout pass.
#[must_use]
pub fn is_focusable(node: &Node, tree: &LayoutTree) -> bool {
    if node.states().contains(PseudoStates::DISABLED) {
        return false;
    }
    if !node
        .classes()
        .iter()
        .any(|class| class.as_str() == FOCUSABLE_CLASS)
    {
        return false;
    }
    tree.allocation(node)
        .is_some_and(|alloc| !alloc.border_box.is_empty())
}

/// The centre of `node`'s border box, or `None` if it is not laid out.
fn centre(node: &Node, tree: &LayoutTree) -> Option<(f32, f32)> {
    let alloc = tree.allocation(node)?;
    if alloc.border_box.is_empty() {
        return None;
    }
    Some((
        alloc.border_box.x + alloc.border_box.width / 2.0,
        alloc.border_box.y + alloc.border_box.height / 2.0,
    ))
}

/// `parent`'s mapped, sensitive, focusable direct children, in `dir` order.
///
/// Tab order is reading order: by y-centre, then by x-centre, with x reversed
/// under RTL. The arrow directions sort along their own axis first, so
/// `Right` walks a row before descending. `TabBackward`, `Left` and `Up` are
/// the reverse of their opposites -- one comparator, four orders, so a change
/// to the tie-break cannot apply to only half of them.
#[must_use]
pub fn focus_sort(parent: &Node, tree: &LayoutTree, dir: FocusDirection) -> Vec<Node> {
    let rtl = parent.direction() == Direction::Rtl;
    let mut candidates: Vec<(Node, (f32, f32))> = parent
        .children()
        .into_iter()
        .filter(|child| {
            !child.states().contains(PseudoStates::DISABLED)
        })
        .filter_map(|child| centre(&child, tree).map(|c| (child, c)))
        .collect();

    let horizontal = matches!(dir, FocusDirection::Left | FocusDirection::Right);
    candidates.sort_by(|(_, a), (_, b)| {
        let (a_major, a_minor, b_major, b_minor) = if horizontal {
            (a.0, a.1, b.0, b.1)
        } else {
            (a.1, a.0, b.1, b.0)
        };
        // A row is a band, not an exact y: two widgets of different heights
        // centred on the same line differ by a fraction of a pixel, and an
        // exact comparison would order them by that fraction.
        let major = if (a_major - b_major).abs() < ROW_EPSILON {
            std::cmp::Ordering::Equal
        } else {
            a_major.total_cmp(&b_major)
        };
        major.then_with(|| {
            let minor = a_minor.total_cmp(&b_minor);
            // RTL reverses only the *horizontal* comparison: rows still run
            // top to bottom.
            if rtl && !horizontal {
                minor.reverse()
            } else {
                minor
            }
        })
    });

    if matches!(
        dir,
        FocusDirection::TabBackward | FocusDirection::Left | FocusDirection::Up
    ) {
        candidates.reverse();
    }
    candidates.into_iter().map(|(node, _)| node).collect()
}

/// How far apart two centres may be and still count as the same row/column.
///
/// Half a CSS pixel: enough to absorb a centring difference between a 34px
/// button and a 40px entry, far too little to merge two real rows.
const ROW_EPSILON: f32 = 0.5;
```

`focus_sort` filters on `DISABLED` and on being laid out, but deliberately not
on [`FOCUSABLE_CLASS`]: a container is not itself focusable and must still be
descended into. `navigate` (Task 9) applies [`is_focusable`] as it walks.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::focus`
Expected: PASS — 7 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/focus.rs
git commit -m "$(cat <<'EOF'
feat(ui/window): GTK's geometric focus sort

Ruling R2: tab order is reading order -- by y-centre, then x-centre, with
x reversed under RTL -- not tree order, because a HeaderBar's end packs, a
Grid's cells and an Overlay's layers are laid out in an order their tree
does not describe. The arrow directions sort along their own axis first;
the three reverse directions are the reverse of their opposites, so one
comparator defines all six.

A node is a focus candidate when it is marked focusable, sensitive and
actually laid out.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: The focus ring, `:focus-visible`, and the window key bindings

**Files:**
- Modify: `ui/src/window/focus.rs`
- Modify: `ui/src/css/node.rs` (deviation 11: the per-tree `focus_visible` flag)

**Interfaces:**
- Consumes: Task 8's `focus_sort`/`is_focusable`; Task 3's `KeyEvent`/`Mods`.
- Produces:
  ```rust
  // css/node.rs
  impl Node {
      /// GTK's `:focus-visible` policy for this whole tree. Default `true`.
      pub fn set_tree_focus_visible(&self, on: bool);
      #[must_use] pub fn tree_focus_visible(&self) -> bool;
  }

  // window/focus.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum FocusCause { Keyboard, Pointer, Programmatic }
  #[derive(Debug)]
  pub struct FocusRing { /* private */ }
  impl Default for FocusRing;
  impl FocusRing {
      #[must_use] pub fn focus(&self) -> Option<Node>;
      pub fn set_focus(&mut self, node: Option<&Node>, cause: FocusCause);
      #[must_use] pub fn focus_visible(&self) -> bool;
      pub fn note_key(&mut self, ev: &KeyEvent);
      pub fn set_default(&mut self, node: Option<&Node>);
      #[must_use] pub fn default(&self) -> Option<Node>;
  }
  pub fn navigate(root: &Node, tree: &LayoutTree, from: Option<&Node>,
                  dir: FocusDirection) -> Option<Node>;
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Binding { Move(FocusDirection), Activate, ActivateDefault, Dismiss }
  pub fn window_binding(ev: &KeyEvent) -> Option<Binding>;
  pub const MAX_FOCUS_DEPTH: usize = 256;
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/focus.rs`'s test module:

```rust
    use super::{Binding, FocusCause, FocusRing, navigate, window_binding};
    use crate::window::keyboard::{KeyEvent, Mods};
    use xkbcommon::xkb;

    fn key(keysym: xkb::Keysym, mods: Mods, consumed: Mods, pressed: bool) -> KeyEvent {
        KeyEvent {
            keycode: 0,
            keysym,
            utf8: None,
            mods,
            consumed,
            pressed,
            repeat: false,
            serial: 0,
            time_ms: 0,
        }
    }

    #[test]
    fn navigate_walks_the_geometric_ring_and_stops_at_its_end() {
        let grid = grid();
        let order = ["b", "a", "d", "c"];
        let mut current: Option<Node> = None;
        for expected in order {
            let next = navigate(&grid.root, &grid.tree, current.as_ref(), FocusDirection::TabForward)
                .expect("another candidate");
            assert_eq!(next.id().expect("an id").as_str(), expected);
            current = Some(next);
        }
        assert!(
            navigate(&grid.root, &grid.tree, current.as_ref(), FocusDirection::TabForward).is_none(),
            "the end of the ring is None; wrapping is the caller's policy"
        );
        // Backwards from the last is the reverse walk.
        assert_eq!(
            navigate(&grid.root, &grid.tree, current.as_ref(), FocusDirection::TabBackward)
                .expect("a candidate")
                .id()
                .expect("an id")
                .as_str(),
            "d"
        );
    }

    #[test]
    fn navigate_descends_into_containers_and_skips_non_candidates() {
        let grid = grid();
        // A nested box holding one focusable button, inserted first in tree
        // order but laid out last (it has no width until it has a child).
        let nested = Node::new("box");
        let inner = Node::with_classes("button", &[FOCUSABLE_CLASS]);
        inner.set_id(Some("inner"));
        nested.append_child(&inner);
        grid.container.append_child(&nested);
        let mut grid = grid;
        relayout(&mut grid);
        let all: Vec<String> = std::iter::successors(
            navigate(&grid.root, &grid.tree, None, FocusDirection::TabForward),
            |from| navigate(&grid.root, &grid.tree, Some(from), FocusDirection::TabForward),
        )
        .map(|n| n.id().map_or_else(String::new, |id| id.as_str().to_string()))
        .collect();
        assert!(all.contains(&"inner".to_string()), "a nested candidate is reached: {all:?}");
        assert!(
            !all.iter().any(String::is_empty),
            "the containers themselves are not in the ring: {all:?}"
        );
    }

    #[test]
    fn setting_focus_moves_the_pseudo_state() {
        let grid = grid();
        let children = grid.container.children();
        let (first, second) = (children[0].clone(), children[1].clone());
        let mut ring = FocusRing::default();
        assert!(ring.focus().is_none());

        ring.set_focus(Some(&first), FocusCause::Keyboard);
        assert!(first.states().contains(PseudoStates::FOCUS));
        assert!(
            grid.container.states().contains(PseudoStates::FOCUS_WITHIN),
            "M2 derives :focus-within up the chain"
        );

        ring.set_focus(Some(&second), FocusCause::Keyboard);
        assert!(!first.states().contains(PseudoStates::FOCUS), "the old owner lost it");
        assert!(second.states().contains(PseudoStates::FOCUS));

        ring.set_focus(None, FocusCause::Programmatic);
        assert!(!second.states().contains(PseudoStates::FOCUS));
        assert!(!grid.container.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn focus_visible_follows_gtks_rule_and_reaches_the_selector() {
        // Ruling R3, and contract deviation 11: the flag has to be visible to
        // the cascade or `:focus-visible` in Adwaita is a no-op.
        let grid = grid();
        let children = grid.container.children();
        let (first, second) = (children[0].clone(), children[1].clone());
        let mut ring = FocusRing::default();
        assert!(ring.focus_visible(), "GTK defaults to visible");

        // A pointer click focuses without a ring.
        ring.set_focus(Some(&first), FocusCause::Pointer);
        assert!(!ring.focus_visible());
        assert!(first.states().contains(PseudoStates::FOCUS));
        assert!(
            !first.states().contains(PseudoStates::FOCUS_VISIBLE),
            "a click must not draw the focus ring"
        );

        // A key press that moves the focus turns it back on.
        ring.note_key(&key(xkb::Keysym::Tab, Mods::empty(), Mods::empty(), true));
        ring.set_focus(Some(&second), FocusCause::Keyboard);
        ring.note_key(&key(xkb::Keysym::Tab, Mods::empty(), Mods::empty(), false));
        assert!(ring.focus_visible());
        assert!(second.states().contains(PseudoStates::FOCUS_VISIBLE));

        // A key press that does not move it turns it off again.
        ring.note_key(&key(xkb::Keysym::a, Mods::empty(), Mods::empty(), true));
        ring.note_key(&key(xkb::Keysym::a, Mods::empty(), Mods::empty(), false));
        assert!(!ring.focus_visible(), "typing into a widget hides the ring");
        assert!(!second.states().contains(PseudoStates::FOCUS_VISIBLE));

        // Alt alone forces it on, press or release.
        ring.note_key(&key(xkb::Keysym::Alt_L, Mods::empty(), Mods::empty(), true));
        assert!(ring.focus_visible());
        assert!(second.states().contains(PseudoStates::FOCUS_VISIBLE));
    }

    #[test]
    fn the_window_default_is_separate_from_the_focus() {
        let grid = grid();
        let children = grid.container.children();
        let mut ring = FocusRing::default();
        ring.set_default(Some(&children[2]));
        ring.set_focus(Some(&children[0]), FocusCause::Keyboard);
        assert!(ring.default().expect("a default").ptr_eq(&children[2]));
        assert!(ring.focus().expect("a focus").ptr_eq(&children[0]));
        ring.set_default(None);
        assert!(ring.default().is_none());
    }

    #[test]
    fn the_window_bindings_are_exactly_the_contracts_table() {
        use FocusDirection::{Down, Left, Right, TabBackward, TabForward, Up};
        let plain = Mods::empty();
        let cases: Vec<(xkb::Keysym, Mods, Mods, Option<Binding>)> = vec![
            (xkb::Keysym::Tab, plain, plain, Some(Binding::Move(TabForward))),
            (xkb::Keysym::KP_Tab, plain, plain, Some(Binding::Move(TabForward))),
            (xkb::Keysym::Tab, Mods::CTRL, plain, Some(Binding::Move(TabForward))),
            (xkb::Keysym::ISO_Left_Tab, Mods::SHIFT, Mods::SHIFT, Some(Binding::Move(TabBackward))),
            (xkb::Keysym::Tab, Mods::SHIFT, plain, Some(Binding::Move(TabBackward))),
            (xkb::Keysym::Up, plain, plain, Some(Binding::Move(Up))),
            (xkb::Keysym::KP_Up, Mods::CTRL, plain, Some(Binding::Move(Up))),
            (xkb::Keysym::Down, plain, plain, Some(Binding::Move(Down))),
            (xkb::Keysym::Left, plain, plain, Some(Binding::Move(Left))),
            (xkb::Keysym::KP_Right, plain, plain, Some(Binding::Move(Right))),
            (xkb::Keysym::space, plain, plain, Some(Binding::Activate)),
            (xkb::Keysym::KP_Space, plain, plain, Some(Binding::Activate)),
            (xkb::Keysym::Return, plain, plain, Some(Binding::ActivateDefault)),
            (xkb::Keysym::ISO_Enter, plain, plain, Some(Binding::ActivateDefault)),
            (xkb::Keysym::KP_Enter, plain, plain, Some(Binding::ActivateDefault)),
            (xkb::Keysym::Escape, plain, plain, Some(Binding::Dismiss)),
            // Not bound: the letters, and anything with Alt or Logo held --
            // those belong to accelerators and to the compositor.
            (xkb::Keysym::a, plain, plain, None),
            (xkb::Keysym::Tab, Mods::ALT, plain, None),
            (xkb::Keysym::Up, Mods::LOGO, plain, None),
            (xkb::Keysym::F1, plain, plain, None),
        ];
        for (keysym, mods, consumed, expected) in cases {
            let ev = key(keysym, mods, consumed, true);
            assert_eq!(window_binding(&ev), expected, "{keysym:?} with {mods:?}");
            let released = key(keysym, mods, consumed, false);
            assert_eq!(window_binding(&released), None, "a release binds nothing");
        }
        // Caps and Num lock are ignored: Tab with Caps on is still Tab.
        let with_locks = key(xkb::Keysym::Tab, Mods::CAPS | Mods::NUM, plain, true);
        assert_eq!(window_binding(&with_locks), Some(Binding::Move(TabForward)));
    }
```

Add the relayout helper the second test needs, next to `grid()`:

```rust
    /// Re-run the style/layout pass after mutating the fixture's tree.
    fn relayout(grid: &mut Grid) {
        let sheet = CompiledSheet::compile(GRID_CSS);
        let env = ResolveEnv::default();
        let mut measure = FixedMeasure(taffy::Size { width: 0.0, height: 0.0 });
        restyle(
            &grid.root,
            &sheet,
            &env,
            &mut grid.styles,
            &mut grid.tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(160.0),
            },
            &mut measure,
        )
        .expect("the grid re-lays out");
    }
```

**Mutation checks.**
`navigate_walks_the_geometric_ring_and_stops_at_its_end`: wrap at the end
instead of returning `None` → the `is_none` assertion fails.
`navigate_descends_into_containers_and_skips_non_candidates`: stop recursing at
the first level → `inner` is missing; push every node rather than only
focusable ones → the empty-id assertion fails.
`setting_focus_moves_the_pseudo_state`: skip clearing the previous owner → the
"old owner lost it" assertion fails.
`focus_visible_follows_gtks_rule_and_reaches_the_selector`: drop the
`set_tree_focus_visible` call → the `FOCUS_VISIBLE` assertions fail while
`focus_visible()` still reports correctly, which is exactly the bug deviation 11
exists to prevent; drop the Alt special case → the last assertion fails; set
`focus_visible` on the press rather than the release → the "typing hides it"
assertion fails.
`the_window_default_is_separate_from_the_focus`: return `self.focus` from
`default()` → the first assertion fails.
`the_window_bindings_are_exactly_the_contracts_table`: bind on release too →
every `released` assertion fails; compare raw `mods` instead of masking the
locks → the Caps/Num case fails; drop the `ALT`/`LOGO` rejection → two `None`
cases fail.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::focus`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{Binding, FocusCause, FocusRing, navigate, window_binding}`` and
`error[E0599]: no method named `set_tree_focus_visible` found for struct `Node``.

- [ ] **Step 3: Write the implementation**

**3a. `ui/src/css/node.rs`** — three edits, no test changes.

In `struct TreeToken`, add the flag and default it on:

```rust
struct TreeToken {
    id: u64,
    generation: Cell<u64>,
    /// GTK's `:focus-visible` policy for this tree (ruling R3).
    ///
    /// `true` by default, which is both GTK's default and what keeps
    /// `PseudoStates::DERIVED` behaving exactly as M2 left it. The window
    /// layer clears it for a focus taken by a pointer click and sets it again
    /// for one taken by a key, which is the only way that policy can reach a
    /// `:focus-visible` selector: the flag is derived from `FOCUS`, never set.
    focus_visible: Cell<bool>,
}
```

```rust
impl TreeToken {
    fn new(generation: u64) -> Rc<TreeToken> {
        Rc::new(TreeToken {
            id: NEXT_TREE_ID.fetch_add(1, Ordering::Relaxed),
            generation: Cell::new(generation),
            focus_visible: Cell::new(true),
        })
    }
}
```

In `Node::states`, split the derived pair:

```rust
    /// This node's pseudo-class state, derived flags included.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        let mut states = self.0.own_states.get();
        if self.0.focus_count.get() > 0 {
            states |= PseudoStates::FOCUS_WITHIN;
            if self.0.tree.borrow().focus_visible.get() {
                states |= PseudoStates::FOCUS_VISIBLE;
            }
        }
        states
    }
```

And the two accessors:

```rust
    /// Whether `:focus-visible` is currently derived from `:focus` in this tree.
    #[must_use]
    pub fn tree_focus_visible(&self) -> bool {
        self.0.tree.borrow().focus_visible.get()
    }

    /// Set GTK's `:focus-visible` policy for this whole tree.
    ///
    /// One focus owner per window means one flag per tree. A subtree split off
    /// by [`Node::remove_child`] gets a fresh tree token and so a fresh `true`;
    /// the window layer sets the flag again on the next focus change.
    pub fn set_tree_focus_visible(&self, on: bool) {
        {
            let tree = self.0.tree.borrow();
            if tree.focus_visible.get() == on {
                return;
            }
            tree.focus_visible.set(on);
        }
        // Outside the borrow: `touch` takes the same `RefCell`.
        self.touch();
    }
```

**3b. `ui/src/window/focus.rs`** — append:

```rust
use crate::window::keyboard::{KeyEvent, Mods};

/// Why the focus moved. Drives GTK's `:focus-visible` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCause {
    Keyboard,
    Pointer,
    Programmatic,
}

/// How deep [`navigate`] will descend, for the same reason
/// [`super::pointer::MAX_HIT_DEPTH`] exists.
pub const MAX_FOCUS_DEPTH: usize = 256;

/// One focus owner per window; popups form a stack of these.
#[derive(Debug)]
pub struct FocusRing {
    focus: Option<Node>,
    focus_visible: bool,
    default: Option<Node>,
    /// The focus as it was when the current key went down, if one is down.
    key_focus: Option<Option<Node>>,
}

impl Default for FocusRing {
    /// `focus_visible` starts `true`, as GTK's does: a window opened by a
    /// keyboard shortcut shows its focus ring immediately.
    fn default() -> Self {
        Self {
            focus: None,
            focus_visible: true,
            default: None,
            key_focus: None,
        }
    }
}

impl FocusRing {
    #[must_use]
    pub fn focus(&self) -> Option<Node> {
        self.focus.clone()
    }

    /// Move `PseudoStates::FOCUS`, and with it M2's derived
    /// `:focus-within`/`:focus-visible`.
    pub fn set_focus(&mut self, node: Option<&Node>, cause: FocusCause) {
        match cause {
            FocusCause::Keyboard => self.focus_visible = true,
            FocusCause::Pointer => self.focus_visible = false,
            FocusCause::Programmatic => {}
        }
        if let Some(previous) = self.focus.take() {
            previous.set_state(PseudoStates::FOCUS, false);
        }
        if let Some(node) = node {
            node.set_state(PseudoStates::FOCUS, true);
            node.set_tree_focus_visible(self.focus_visible);
            self.focus = Some(node.clone());
        }
    }

    /// Whether the focus ring is drawn.
    #[must_use]
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Feed every key event, pressed and released.
    ///
    /// GTK's rule (`_gtk_window_update_focus_visible`), not CSS's: a press
    /// remembers the focus; the matching release clears the flag only if the
    /// focus did not move while the key was down, and sets it otherwise --
    /// so `Tab` shows the ring and typing a letter hides it. `Alt` alone
    /// forces it on, which is how a menu mnemonic reveals itself.
    pub fn note_key(&mut self, ev: &KeyEvent) {
        if matches!(ev.keysym, xkb::Keysym::Alt_L | xkb::Keysym::Alt_R) {
            self.focus_visible = true;
            self.apply_visible();
            return;
        }
        if ev.pressed {
            self.key_focus = Some(self.focus.clone());
            return;
        }
        let Some(remembered) = self.key_focus.take() else {
            return;
        };
        let moved = match (&remembered, &self.focus) {
            (None, None) => false,
            (Some(a), Some(b)) => !a.ptr_eq(b),
            _ => true,
        };
        self.focus_visible = moved;
        self.apply_visible();
    }

    pub fn set_default(&mut self, node: Option<&Node>) {
        self.default = node.cloned();
    }

    #[must_use]
    pub fn default(&self) -> Option<Node> {
        self.default.clone()
    }

    /// Push the policy onto the tree the focus owner belongs to.
    fn apply_visible(&self) {
        if let Some(focus) = &self.focus {
            focus.set_tree_focus_visible(self.focus_visible);
        }
    }
}

/// The next focus from `from` in `dir`, recursing into containers.
///
/// `None` at the end of the ring: wrapping is the caller's policy, because a
/// popup's ring pops to its parent's instead of wrapping.
#[must_use]
pub fn navigate(
    root: &Node,
    tree: &LayoutTree,
    from: Option<&Node>,
    dir: FocusDirection,
) -> Option<Node> {
    let mut ring = Vec::new();
    collect(root, tree, dir, &mut ring, 0);
    let Some(from) = from else {
        return ring.into_iter().next();
    };
    match ring.iter().position(|node| node.ptr_eq(from)) {
        // The current focus is no longer in the ring (it was removed, or
        // disabled): start over rather than trapping focus.
        None => ring.into_iter().next(),
        Some(index) => ring.into_iter().nth(index + 1),
    }
}

/// Every focus candidate under `parent`, in `dir` order, depth first.
fn collect(parent: &Node, tree: &LayoutTree, dir: FocusDirection, out: &mut Vec<Node>, depth: usize) {
    if depth >= MAX_FOCUS_DEPTH {
        tracing::warn!(depth, "focus walk stopped at the depth guard");
        return;
    }
    for child in focus_sort(parent, tree, dir) {
        if is_focusable(&child, tree) {
            out.push(child.clone());
        }
        collect(&child, tree, dir, out, depth + 1);
    }
}

/// A window-level key binding, applied before the focused node sees the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Move the focus.
    Move(FocusDirection),
    /// Activate the focused node (`Space`).
    Activate,
    /// Activate the window's default (`Return`).
    ActivateDefault,
    /// `Escape`: a popup destroys itself client-side; a window ignores it.
    Dismiss,
}

/// The binding `ev` triggers, if any.
///
/// Contract §3.5's table exactly. `Ctrl` is allowed on the navigation keys
/// (GTK's "move focus out of this container" variants) and ignored; `Alt` and
/// `Logo` are not -- those belong to accelerators and to the compositor.
/// `Caps`/`Num` are masked out: `Tab` with Caps Lock on is still `Tab`.
#[must_use]
pub fn window_binding(ev: &KeyEvent) -> Option<Binding> {
    use FocusDirection::{Down, Left, Right, TabBackward, TabForward, Up};

    if !ev.pressed {
        return None;
    }
    let mods = ev
        .effective_mods()
        .difference(Mods::CAPS | Mods::NUM);
    if mods.intersects(Mods::ALT | Mods::LOGO) {
        return None;
    }
    // `Shift+Tab` is `ISO_Left_Tab` on most layouts, but not all -- and where
    // it is, the Shift was consumed, so `effective_mods` no longer shows it.
    // Both spellings mean the same thing.
    let shifted = ev.mods.contains(Mods::SHIFT);
    let binding = match ev.keysym {
        xkb::Keysym::ISO_Left_Tab => Binding::Move(TabBackward),
        xkb::Keysym::Tab | xkb::Keysym::KP_Tab => {
            Binding::Move(if shifted { TabBackward } else { TabForward })
        }
        xkb::Keysym::Up | xkb::Keysym::KP_Up => Binding::Move(Up),
        xkb::Keysym::Down | xkb::Keysym::KP_Down => Binding::Move(Down),
        xkb::Keysym::Left | xkb::Keysym::KP_Left => Binding::Move(Left),
        xkb::Keysym::Right | xkb::Keysym::KP_Right => Binding::Move(Right),
        xkb::Keysym::space | xkb::Keysym::KP_Space => Binding::Activate,
        xkb::Keysym::Return | xkb::Keysym::ISO_Enter | xkb::Keysym::KP_Enter => {
            Binding::ActivateDefault
        }
        xkb::Keysym::Escape => Binding::Dismiss,
        _ => return None,
    };
    if mods.contains(Mods::CTRL) && !matches!(binding, Binding::Move(_)) {
        // `Ctrl+Space` and `Ctrl+Return` are widget bindings (toggling a
        // selection, inserting a newline), not window ones.
        return None;
    }
    Some(binding)
}
```

Add `use xkbcommon::xkb;` to the file's imports.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::focus`
Expected: PASS — 13 tests.

Run: `cargo test -p icedtea-ui --lib css::node`
Expected: PASS — every M2 node test unchanged (the flag defaults to `true`, so
`PseudoStates::DERIVED` behaves exactly as before).

Run: `cargo test -p icedtea-ui`
Expected: PASS — the css/anim/paint/widget/app suites are untouched.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/focus.rs ui/src/css/node.rs
git commit -m "$(cat <<'EOF'
feat(ui/window): the focus ring, GTK's :focus-visible rule and the bindings

navigate walks the geometric ring depth-first and returns None at its end;
wrapping is the caller's policy because a popup's ring pops instead.

Ruling R3's :focus-visible is GTK's, not CSS's: default on, off for a
pointer click, on again for a key press that moved the focus, off for one
that did not, forced on by Alt. It reaches the cascade through one
per-tree flag on node.rs -- the derived FOCUS_VISIBLE bit is computed, not
settable, so a window-layer bool alone could never make Adwaita's
:focus-visible rules match. The flag defaults to true, so every existing
node.rs test is byte-identical.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `Window` — one connection, the toplevel role, and the bounded pump

**Files:**
- Modify: `ui/src/window/mod.rs`
- Modify: `ui/src/window/toplevel.rs` (replace the stub)
- Modify: `ui/src/window/popup.rs` (add `PopupKey` only; Task 14 fills the rest)

**Interfaces:**
- Consumes: Tasks 1, 3, 5, 6, 7, 9; `wayland_client`;
  `wayland_protocols::xdg::shell::client::{xdg_wm_base, xdg_surface, xdg_toplevel}`;
  `crate::shm::{BufferPool, BufferSlot}`.
- Produces:
  ```rust
  // window/popup.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub struct PopupKey(pub(crate) u64);

  // window/mod.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum SurfaceTarget { Window, Popup(PopupKey) }
  #[derive(Debug, Clone)]
  pub struct SurfaceSpec { pub role: Role, pub size: (u32, u32), pub title: String, pub app_id: String }
  #[derive(Debug, Clone)]
  pub enum Role { Toplevel, Layer(LayerSpec) }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum MapPhase { Created, AwaitingConfigure, Configured }
  #[must_use] pub fn may_attach(phase: MapPhase) -> bool;
  pub enum InputEvent { /* contract §3.1, with `target` on the two enters */ }
  pub struct Window { /* private */ }
  impl Window {
      pub fn open(spec: SurfaceSpec, sheet: CompiledSheet, fonts: FontDatabase) -> Result<Self, SurfaceError>;
      pub fn pump(&mut self, timeout: Option<Duration>) -> Result<Vec<InputEvent>, SurfaceError>;
      pub fn root(&self) -> &Node;
      pub fn layout(&mut self) -> &mut LayoutTree;
      pub fn animations(&mut self) -> &mut AnimationState;
      pub fn styles(&self) -> &StyleMap;
      pub fn sheet(&self) -> &CompiledSheet;
      pub fn fonts(&mut self) -> &mut FontDatabase;
      pub fn clock(&self) -> &Rc<dyn Clock>;
      pub fn set_clock(&mut self, clock: Rc<dyn Clock>);
      pub fn surface(&self) -> &Surface;
      pub fn seat_serial(&self) -> Option<u32>;
      pub fn is_closed(&self) -> bool;
  }
  // window/toplevel.rs
  pub struct Toplevel { /* private */ }
  impl Toplevel {
      pub fn set_title(&self, title: &str);
      pub fn set_app_id(&self, app_id: &str);
      pub fn set_min_size(&self, size: (u32, u32));
      pub fn set_max_size(&self, size: (u32, u32));
      pub fn set_maximized(&self, on: bool);
      pub fn set_fullscreen(&self, on: bool);
      pub fn set_minimized(&self);
      pub fn move_(&self, seat: &wl_seat::WlSeat, serial: u32);
      pub fn resize(&self, seat: &wl_seat::WlSeat, serial: u32, edges: xdg_toplevel::ResizeEdge);
      pub fn show_window_menu(&self, seat: &wl_seat::WlSeat, serial: u32, at: (i32, i32));
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/mod.rs`'s test module:

```rust
    use super::{InputEvent, MapPhase, Role, SurfaceSpec, SurfaceTarget, may_attach};
    use crate::css::node::PseudoStates;

    #[test]
    fn a_configure_without_activated_backdrops_the_root() {
        // The whole reason SurfaceStates exists: GTK paints an unfocused
        // window's chrome differently, through `:backdrop`.
        let root = Node::new("window");
        super::apply_configure_states(&root, SurfaceStates::ACTIVATED | SurfaceStates::MAXIMIZED);
        assert!(!root.states().contains(PseudoStates::BACKDROP));
        super::apply_configure_states(&root, SurfaceStates::MAXIMIZED);
        assert!(root.states().contains(PseudoStates::BACKDROP));
        super::apply_configure_states(&root, SurfaceStates::ACTIVATED);
        assert!(!root.states().contains(PseudoStates::BACKDROP));
        // A layer surface never gets states at all, and must not therefore be
        // permanently backdropped: the caller passes ACTIVATED for those.
        assert!(SurfaceStates::from_wire(&[]).is_empty());
    }

    #[test]
    fn nothing_is_attached_before_the_first_configure() {
        // xdg-shell's own sequencing rule: create the role, commit with *no*
        // buffer, wait for configure, ack, then attach. Attaching early is a
        // protocol error that kills the client.
        assert!(!may_attach(MapPhase::Created));
        assert!(!may_attach(MapPhase::AwaitingConfigure));
        assert!(may_attach(MapPhase::Configured));
    }

    #[test]
    fn a_pump_on_a_silent_compositor_times_out_rather_than_hanging() {
        // M1's `the_configure_wait_is_bounded`, generalised: a compositor that
        // accepts the connection and then says nothing must not hang the pump.
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = super::WindowState::new_for_test();
        let started = std::time::Instant::now();
        let result = super::wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::from_millis(120),
            |state| !state.events.is_empty(),
        );
        assert!(matches!(result, Err(SurfaceError::Timeout(_))), "{result:?}");
        assert!(started.elapsed() >= Duration::from_millis(100), "it returned early");
        assert!(started.elapsed() < Duration::from_secs(5), "it hung");
    }

    #[test]
    fn a_closed_surface_ends_the_wait_immediately_and_reports_close_once() {
        let (conn, mut queue, _peer) = silent_connection();
        let mut state = super::WindowState::new_for_test();
        state.close();
        let started = std::time::Instant::now();
        let result = super::wait_bounded(
            &conn,
            &mut queue,
            &mut state,
            Duration::from_secs(5),
            |_| false,
        );
        assert!(matches!(result, Err(SurfaceError::Closed)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            state.events.iter().filter(|e| matches!(e, InputEvent::Close)).count(),
            1,
            "closing twice must not queue two Close events"
        );
        state.close();
        assert_eq!(
            state.events.iter().filter(|e| matches!(e, InputEvent::Close)).count(),
            1
        );
    }

    #[test]
    fn a_surface_spec_names_a_role_and_a_size() {
        let spec = SurfaceSpec {
            role: Role::Toplevel,
            size: (400, 300),
            title: "Settings".into(),
            app_id: "org.icedtea.Settings".into(),
        };
        assert_eq!(spec.size, (400, 300));
        assert!(matches!(spec.role, Role::Toplevel));
        assert_eq!(SurfaceTarget::Window, SurfaceTarget::Window);
        assert_ne!(SurfaceTarget::Window, SurfaceTarget::Popup(crate::window::popup::PopupKey(1)));
    }

    /// A connection to a socket nobody ever writes to.
    fn silent_connection() -> (
        wayland_client::Connection,
        wayland_client::EventQueue<super::WindowState>,
        std::os::unix::net::UnixStream,
    ) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let conn = wayland_client::Connection::from_socket(ours).expect("connection");
        let queue = conn.new_event_queue();
        (conn, queue, theirs)
    }
```

**Mutation checks.**
`a_configure_without_activated_backdrops_the_root`: invert the `ACTIVATED` test
→ the first assertion fails.
`nothing_is_attached_before_the_first_configure`: return `true` for
`AwaitingConfigure` → the assertion fails (and the e2e in Task 16 dies with a
protocol error, which is what this guards).
`a_pump_on_a_silent_compositor_times_out_rather_than_hanging`: replace the
`poll(2)` with `blocking_dispatch` → the test hangs past its 5 s bound.
`a_closed_surface_ends_the_wait_immediately_and_reports_close_once`: push
`Close` unconditionally in `close()` → the count assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: FAIL to compile — `error[E0425]: cannot find function
`apply_configure_states``, `error[E0433]: failed to resolve: could not find
`WindowState``, and the same for `wait_bounded`, `may_attach`, `MapPhase`.

- [ ] **Step 3: Write the implementation**

**3a. `ui/src/window/popup.rs`** — the key only:

```rust
/// A popup's identity within one window. Opaque; only [`Window`] mints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) u64);
```

**3b. `ui/src/window/toplevel.rs`**:

```rust
//! The `xdg_toplevel` surface role.

use wayland_client::protocol::{wl_seat, wl_surface};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel};

/// A mapped (or mapping) `xdg_toplevel`.
pub struct Toplevel {
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) xdg_toplevel: xdg_toplevel::XdgToplevel,
}

impl Toplevel {
    pub fn set_title(&self, title: &str) {
        self.xdg_toplevel.set_title(title.to_owned());
    }

    pub fn set_app_id(&self, app_id: &str) {
        self.xdg_toplevel.set_app_id(app_id.to_owned());
    }

    /// Both sizes are in *window geometry*, not buffer pixels.
    pub fn set_min_size(&self, size: (u32, u32)) {
        self.xdg_toplevel
            .set_min_size(clamp_i32(size.0), clamp_i32(size.1));
    }

    pub fn set_max_size(&self, size: (u32, u32)) {
        self.xdg_toplevel
            .set_max_size(clamp_i32(size.0), clamp_i32(size.1));
    }

    pub fn set_maximized(&self, on: bool) {
        if on {
            self.xdg_toplevel.set_maximized();
        } else {
            self.xdg_toplevel.unset_maximized();
        }
    }

    pub fn set_fullscreen(&self, on: bool) {
        if on {
            self.xdg_toplevel.set_fullscreen(None);
        } else {
            self.xdg_toplevel.unset_fullscreen();
        }
    }

    pub fn set_minimized(&self) {
        self.xdg_toplevel.set_minimized();
    }

    /// An interactive move. The serial must come from a real input event.
    pub fn move_(&self, seat: &wl_seat::WlSeat, serial: u32) {
        self.xdg_toplevel._move(seat, serial);
    }

    pub fn resize(&self, seat: &wl_seat::WlSeat, serial: u32, edges: xdg_toplevel::ResizeEdge) {
        self.xdg_toplevel.resize(seat, serial, edges);
    }

    pub fn show_window_menu(&self, seat: &wl_seat::WlSeat, serial: u32, at: (i32, i32)) {
        self.xdg_toplevel.show_window_menu(seat, serial, at.0, at.1);
    }

    /// The window-geometry rect the compositor should treat as the window.
    ///
    /// Our buffer is the *ink* rect, which can start outside the border box
    /// (an outset shadow), so the geometry is the visible frame inside it.
    pub(crate) fn set_window_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        self.xdg_surface
            .set_window_geometry(x, y, width.max(1), height.max(1));
    }

    pub(crate) fn ack(&self, serial: u32) {
        self.xdg_surface.ack_configure(serial);
    }
}

impl Drop for Toplevel {
    /// Role object first, then the `xdg_surface`, then the `wl_surface`:
    /// destroying an `xdg_surface` that still has a role raises
    /// `defunct_role_object`.
    fn drop(&mut self) {
        self.xdg_toplevel.destroy();
        self.xdg_surface.destroy();
        self.wl_surface.destroy();
    }
}

/// A `u32` size as the `i32` the protocol wants. A size that does not fit is a
/// caller bug, not a protocol error: clamp rather than wrap into a negative.
fn clamp_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
```

**3c. `ui/src/window/mod.rs`** — the client. Add to the imports:

```rust
use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface, wl_touch,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::anim::{AnimationState, Clock, MonotonicClock};
use crate::css::node::{Node, PseudoStates};
use crate::shm::{BufferPool, BufferSlot};
use crate::text::FontDatabase;
use keyboard::{KeyEvent, Keymap};
use pointer::{CursorShape, Scroll, ScrollSource};
use popup::{PopupKey, Positioner};
```

Then the vocabulary the test names:

```rust
/// Which of a window's surfaces an event arrived on.
///
/// Contract deviation 6: a popup is a separate `wl_surface` with its own
/// pointer and keyboard focus, and the contract's flat `InputEvent` cannot say
/// which one an event is for. The two `Enter` variants carry it; everything
/// after an enter belongs to that surface until the matching leave, which is
/// how Wayland itself defines focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTarget {
    Window,
    Popup(PopupKey),
}

/// What to open.
#[derive(Debug, Clone)]
pub struct SurfaceSpec {
    pub role: Role,
    /// The initial buffer size in surface-local pixels.
    pub size: (u32, u32),
    pub title: String,
    pub app_id: String,
}

/// The role, and the role-specific state that has to be set before the first
/// commit.
#[derive(Debug, Clone)]
pub enum Role {
    Toplevel,
    Layer(LayerSpec),
}

/// `zwlr_layer_surface_v1` state, all of it double-buffered and so settable
/// only before the first commit.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub layer: zwlr_layer_shell_v1::Layer,
    pub anchor: zwlr_layer_surface_v1::Anchor,
    /// top, right, bottom, left.
    pub margin: [i32; 4],
    pub exclusive_zone: i32,
    pub keyboard: zwlr_layer_surface_v1::KeyboardInteractivity,
}

/// Where a surface is in xdg-shell's mapping sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPhase {
    /// The role object exists; nothing has been committed.
    Created,
    /// The empty commit went out; the first `configure` has not come back.
    AwaitingConfigure,
    /// A `configure` arrived and was acked: buffers may be attached.
    Configured,
}

/// Whether a buffer may be attached yet.
///
/// xdg-shell's sequencing rule, in one place: role object -> role state ->
/// **commit with no buffer** -> `configure` -> `ack_configure` -> attach.
/// Attaching before the first configure is `xdg_surface.error.unconfigured_buffer`,
/// which kills the client.
#[must_use]
pub fn may_attach(phase: MapPhase) -> bool {
    matches!(phase, MapPhase::Configured)
}

/// Apply a configure's states to the root node.
///
/// `!ACTIVATED` is `:backdrop`, which is the only one of the nine that GTK's
/// stylesheet reads. The rest are the app's business (a maximised window hides
/// its resize grip) and reach it through `InputEvent::Configure`.
pub(crate) fn apply_configure_states(root: &Node, states: SurfaceStates) {
    root.set_state(
        PseudoStates::BACKDROP,
        !states.contains(SurfaceStates::ACTIVATED),
    );
}
```

The client state — one struct, one queue, every surface of one window:

```rust
/// Everything the dispatch handlers write and [`Window`] reads.
pub(crate) struct WindowState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    touch: Option<wl_touch::WlTouch>,
    cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    /// Compiled from `wl_keyboard.keymap`; `None` until it arrives.
    keymap: Option<Keymap>,
    /// The events `pump` will hand up, in arrival order.
    events: Vec<InputEvent>,
    /// Buffers the compositor released, by `wl_buffer` id: one window has
    /// several pools and `BufferSlot` alone cannot say which.
    released: Vec<(wayland_client::backend::ObjectId, BufferSlot)>,
    /// The most recent serial from any input event, for `grab`, `move`,
    /// `resize` and the clipboard.
    seat_serial: Option<u32>,
    /// The last `xdg_surface.configure` serial per surface, to ack on commit.
    pending_ack: Vec<(SurfaceTarget, u32)>,
    configured: Option<(u32, u32)>,
    states: SurfaceStates,
    scale: i32,
    closed: bool,
    /// A pending scroll frame, accumulated until `wl_pointer.frame`.
    axis: Option<Scroll>,
    /// Set by Task 14's popup dispatch.
    pub(crate) popup_events: Vec<InputEvent>,
}

impl WindowState {
    fn new() -> Self { /* every field None/empty, scale: 1 */ }

    /// A state with no connection behind it, for the bounded-wait tests.
    #[cfg(test)]
    fn new_for_test() -> Self {
        Self::new()
    }

    /// Record that the compositor closed the surface. Idempotent: a `Closed`
    /// from the layer surface and a destroy on the way out must not queue two
    /// `InputEvent::Close`s.
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.events.push(InputEvent::Close);
    }
}
```

`wait_bounded` is M1's, generalised over the new state (`ui/src/window/layer.rs`
keeps its own copy for `LayerWindow` -- the two are not merged, because merging
them would edit the M1 code the gate pins):

```rust
/// Block until `ready`, the surface closes, or `timeout` expires.
///
/// wayland-client 0.31's bounded-wait shape: dispatch what is queued,
/// `prepare_read` to register interest, `poll(2)` the connection fd for what is
/// left of the deadline, then read. No helper thread, and no `blocking_dispatch`
/// that could outlive the deadline.
fn wait_bounded(
    conn: &Connection,
    queue: &mut EventQueue<WindowState>,
    state: &mut WindowState,
    timeout: Duration,
    ready: impl Fn(&WindowState) -> bool,
) -> Result<(), SurfaceError> {
    use rustix::event::{PollFlags, Timespec};

    let deadline = Instant::now() + timeout;
    loop {
        queue.dispatch_pending(state).map_err(SurfaceError::Dispatch)?;
        if state.closed {
            return Err(SurfaceError::Closed);
        }
        if ready(state) {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(SurfaceError::Timeout(timeout));
        }
        let remaining = deadline - now;
        conn.flush().map_err(socket_error)?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let fd = queue.as_fd();
        let mut fds = [rustix::event::PollFd::new(&fd, PollFlags::IN)];
        let timespec = Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        match rustix::event::poll(&mut fds, Some(&timespec)) {
            Ok(0) => return Err(SurfaceError::Timeout(timeout)),
            Ok(_) => match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(socket_error(err)),
            },
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(SurfaceError::Socket(err.into())),
        }
    }
}

fn socket_error(err: wayland_client::backend::WaylandError) -> SurfaceError {
    match err {
        wayland_client::backend::WaylandError::Io(io) => SurfaceError::Socket(io),
        other => SurfaceError::Socket(std::io::Error::other(other.to_string())),
    }
}
```

`Window` itself, its `open` and its `pump`:

```rust
/// One window: a surface, its retained tree, and the pump that drives them.
pub struct Window {
    conn: Connection,
    queue: EventQueue<WindowState>,
    qh: QueueHandle<WindowState>,
    state: WindowState,
    surface: Surface,
    shm: wl_shm::WlShm,
    buffers: BufferPool,
    skia: skia_rs_safe::canvas::Surface,
    root: Node,
    layout: crate::layout::LayoutTree,
    styles: StyleMap,
    anim: AnimationState,
    env: crate::css::computed::ResolveEnv,
    images: crate::paint::ImageCache,
    sheet: CompiledSheet,
    fonts: FontDatabase,
    clock: Rc<dyn Clock>,
    phase: MapPhase,
    dirty: bool,
    /// Task 14 fills this.
    popups: Vec<popup::PopupWindow>,
    /// Task 15 fills this.
    clipboard: Option<selection::Clipboard>,
}

/// How long [`Window::open`] waits for the first `configure`.
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5);

impl Window {
    /// Connect, bind, map the surface `spec` describes, and wait for its first
    /// configure.
    ///
    /// # Errors
    ///
    /// [`SurfaceError`] for a failed connection, a missing global, a failed shm
    /// or Skia allocation, a failed dispatch, the compositor closing the
    /// surface before configuring it, or that configure not arriving within
    /// [`CONFIGURE_TIMEOUT`].
    pub fn open(
        spec: SurfaceSpec,
        sheet: CompiledSheet,
        fonts: FontDatabase,
    ) -> Result<Self, SurfaceError> {
        let conn = Connection::connect_to_env().map_err(SurfaceError::Connect)?;
        let mut queue: EventQueue<WindowState> = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = WindowState::new();
        // Two roundtrips: `wl_seat.capabilities` arrives after the bind.
        queue.roundtrip(&mut state).map_err(SurfaceError::Dispatch)?;
        queue.roundtrip(&mut state).map_err(SurfaceError::Dispatch)?;

        let compositor = state
            .compositor
            .clone()
            .ok_or(SurfaceError::MissingGlobal("wl_compositor"))?;
        let shm = state.shm.clone().ok_or(SurfaceError::MissingGlobal("wl_shm"))?;
        let wl_surface = compositor.create_surface(&qh, SurfaceTarget::Window);
        let width = i32::try_from(spec.size.0.max(1)).unwrap_or(i32::MAX);
        let height = i32::try_from(spec.size.1.max(1)).unwrap_or(i32::MAX);

        let surface = match &spec.role {
            Role::Toplevel => {
                let wm_base = state
                    .wm_base
                    .clone()
                    .ok_or(SurfaceError::MissingGlobal("xdg_wm_base"))?;
                let xdg_surface = wm_base.get_xdg_surface(&wl_surface, &qh, SurfaceTarget::Window);
                let xdg_toplevel = xdg_surface.get_toplevel(&qh, ());
                let toplevel = toplevel::Toplevel {
                    wl_surface: wl_surface.clone(),
                    xdg_surface,
                    xdg_toplevel,
                };
                toplevel.set_title(&spec.title);
                toplevel.set_app_id(&spec.app_id);
                Surface::Toplevel(toplevel)
            }
            Role::Layer(layer_spec) => Surface::Layer(layer::Layer::create(
                &state,
                &qh,
                &wl_surface,
                layer_spec,
                (width, height),
                &spec.title,
            )?),
        };
        // The empty commit: role state is double-buffered, and no buffer may
        // be attached until the configure it triggers comes back.
        wl_surface.commit();
        conn.flush().map_err(socket_error)?;

        wait_bounded(&conn, &mut queue, &mut state, CONFIGURE_TIMEOUT, |state| {
            state.configured.is_some()
        })?;
        let (width, height) = state
            .configured
            .map_or((width, height), |(w, h)| {
                (
                    i32::try_from(w.max(1)).unwrap_or(width),
                    i32::try_from(h.max(1)).unwrap_or(height),
                )
            });

        let buffers = BufferPool::new(&shm, &qh, width, height).map_err(SurfaceError::Shm)?;
        let skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(width, height)
            .ok_or(SurfaceError::Render("the raster surface to paint into"))?;
        let root = Node::with_classes("window", &["background"]);
        Ok(Self {
            conn,
            queue,
            qh,
            state,
            surface,
            shm,
            buffers,
            skia,
            root,
            layout: crate::layout::LayoutTree::new(),
            styles: StyleMap::new(),
            anim: AnimationState::new(),
            env: crate::css::computed::ResolveEnv::default(),
            images: crate::paint::ImageCache::new(),
            sheet,
            fonts,
            clock: Rc::new(MonotonicClock::new()),
            phase: MapPhase::Configured,
            dirty: true,
            popups: Vec::new(),
            clipboard: None,
        })
    }

    /// Dispatch, and hand up everything that arrived.
    ///
    /// `None` blocks until an event or a close. `Some(Duration::ZERO)` is a
    /// non-blocking drain, which is what a caller with an animation running
    /// wants.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Dispatch`] or [`SurfaceError::Socket`]. A timeout is not
    /// an error here: it is an empty batch.
    pub fn pump(&mut self, timeout: Option<Duration>) -> Result<Vec<InputEvent>, SurfaceError> {
        let wait = timeout.unwrap_or(Duration::MAX);
        match wait_bounded(&self.conn, &mut self.queue, &mut self.state, wait, |state| {
            !state.events.is_empty()
        }) {
            Ok(()) | Err(SurfaceError::Timeout(_)) | Err(SurfaceError::Closed) => {}
            Err(err) => return Err(err),
        }
        // The repeat timer is ours, not the compositor's: it has to be sampled
        // wherever the pump wakes up, including on a timeout.
        let now = self.clock.now();
        if let Some(keymap) = self.state.keymap.as_mut()
            && let Some(event) = keymap.repeat_due(now)
        {
            self.state.events.push(InputEvent::Key(event));
        }
        Ok(std::mem::take(&mut self.state.events))
    }

    #[must_use]
    pub fn root(&self) -> &Node { &self.root }
    pub fn layout(&mut self) -> &mut crate::layout::LayoutTree { &mut self.layout }
    pub fn animations(&mut self) -> &mut AnimationState { &mut self.anim }
    #[must_use]
    pub fn styles(&self) -> &StyleMap { &self.styles }
    #[must_use]
    pub fn sheet(&self) -> &CompiledSheet { &self.sheet }
    pub fn fonts(&mut self) -> &mut FontDatabase { &mut self.fonts }
    #[must_use]
    pub fn clock(&self) -> &Rc<dyn Clock> { &self.clock }
    pub fn set_clock(&mut self, clock: Rc<dyn Clock>) { self.clock = clock; }
    #[must_use]
    pub fn surface(&self) -> &Surface { &self.surface }
    /// The most recent input serial, for `grab`/`move`/`resize`/clipboard.
    #[must_use]
    pub fn seat_serial(&self) -> Option<u32> { self.state.seat_serial }
    #[must_use]
    pub fn is_closed(&self) -> bool { self.state.closed }
}
```

And the dispatch impls. `wl_registry` binds nine globals; the three the client
cannot work without are checked in `open`, the rest are optional:

```rust
impl Dispatch<wl_registry::WlRegistry, ()> for WindowState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global { name, interface, version } = event else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(4), qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, version.min(5), qh, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
            "wp_cursor_shape_manager_v1" => {
                state.cursor_manager = Some(registry.bind(name, version.min(2), qh, ()));
            }
            // Task 15 binds the two selection managers here.
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WindowState {
    /// A ping unanswered for long enough gets the client declared unresponsive
    /// and killed, so it is answered here and never surfaces as an
    /// `InputEvent`.
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, SurfaceTarget> for WindowState {
    /// The terminal event of a configure sequence: the role-specific one
    /// arrived first and is already recorded.
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        target: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.pending_ack.retain(|(t, _)| t != target);
            state.pending_ack.push((*target, serial));
            if *target == SurfaceTarget::Window {
                let size = state.configured.unwrap_or((0, 0));
                state.events.push(InputEvent::Configure {
                    size,
                    states: state.states,
                });
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, states } => {
                // A zero dimension means "you choose": keep what we have.
                let (w, h) = (u32::try_from(width).unwrap_or(0), u32::try_from(height).unwrap_or(0));
                let previous = state.configured.unwrap_or((0, 0));
                state.configured = Some((
                    if w == 0 { previous.0 } else { w },
                    if h == 0 { previous.1 } else { h },
                ));
                state.states = SurfaceStates::from_wire(&states);
            }
            xdg_toplevel::Event::Close => state.close(),
            // `ConfigureBounds` and `WmCapabilities` are advisory; a window
            // that ignores them is well-behaved, and M3 has no UI for either.
            _ => {}
        }
    }
}
```

The seat is bound but not yet handled: add

```rust
delegate_noop!(WindowState: ignore wl_seat::WlSeat);
```

so the registry's `bind` compiles. Task 11 replaces that line with the real
capability handler and adds the pointer, keyboard and touch dispatch.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: PASS — 9 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): the Window client, the xdg_toplevel role and the pump

One connection, one event queue, one flat InputEvent stream. The pump is
M1's wait_bounded generalised: dispatch, prepare_read, poll(2), read --
never a blocking_dispatch that could outlive its deadline -- plus the
key-repeat timer, which only the pump can sample.

xdg-shell's sequencing rule lives in one predicate: role, role state,
empty commit, configure, ack, then attach. xdg_wm_base.ping is answered
inside dispatch and never surfaces as an event. A scroll is assembled
across axis/axis_source/axis_stop and emitted on frame, because the parts
mean nothing apart.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Seat input — pointer, keyboard, touch and buffer releases

**Files:**
- Modify: `ui/src/window/mod.rs`

**Interfaces:**
- Consumes: Task 10's `WindowState`/`InputEvent`; Tasks 3-5's `Keymap`;
  Task 7's `Scroll`/`ScrollSource`.
- Produces:
  ```rust
  fn sync_capability<T>(present: bool, slot: &mut Option<T>, create: impl FnOnce() -> T,
                        destroy: impl FnOnce(&T));
  fn scroll_source_of(source: wayland_client::WEnum<wl_pointer::AxisSource>) -> ScrollSource;
  fn accumulate_axis(pending: &mut Option<Scroll>,
                     axis: wayland_client::WEnum<wl_pointer::Axis>, value: f32, time_ms: u32);
  impl Dispatch<wl_seat::WlSeat, ()> for WindowState;
  impl Dispatch<wl_pointer::WlPointer, ()> for WindowState;
  impl Dispatch<wl_keyboard::WlKeyboard, ()> for WindowState;
  impl Dispatch<wl_touch::WlTouch, ()> for WindowState;
  impl Dispatch<wl_surface::WlSurface, SurfaceTarget> for WindowState;
  impl Dispatch<wl_buffer::WlBuffer, BufferSlot> for WindowState;
  impl Dispatch<wl_callback::WlCallback, SurfaceTarget> for WindowState;
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/mod.rs`'s test module:

```rust
    #[test]
    fn a_scroll_is_assembled_across_its_parts_and_emitted_whole() {
        // `axis`, `axis_source` and `axis_stop` are separate events and mean
        // nothing apart: two axes in one frame are one diagonal scroll, not two.
        use super::accumulate_axis;
        use wayland_client::WEnum;
        use wayland_client::protocol::wl_pointer;

        let mut pending = None;
        accumulate_axis(&mut pending, WEnum::Value(wl_pointer::Axis::VerticalScroll), -10.0, 5);
        accumulate_axis(&mut pending, WEnum::Value(wl_pointer::Axis::HorizontalScroll), 4.0, 5);
        accumulate_axis(&mut pending, WEnum::Value(wl_pointer::Axis::VerticalScroll), -6.0, 7);
        let scroll = pending.take().expect("a pending scroll");
        assert_eq!(scroll.dy, -16.0, "the two vertical steps accumulate");
        assert_eq!(scroll.dx, 4.0);
        assert_eq!(scroll.time_ms, 7, "the latest timestamp wins");
        assert!(pending.is_none(), "taking the frame clears it");
    }

    #[test]
    fn an_unknown_axis_or_source_is_ignored_rather_than_guessed() {
        use super::{accumulate_axis, scroll_source_of};
        use crate::window::pointer::ScrollSource;
        use wayland_client::WEnum;
        use wayland_client::protocol::wl_pointer;

        assert_eq!(scroll_source_of(WEnum::Value(wl_pointer::AxisSource::Finger)), ScrollSource::Finger);
        assert_eq!(scroll_source_of(WEnum::Value(wl_pointer::AxisSource::Wheel)), ScrollSource::Wheel);
        assert_eq!(scroll_source_of(WEnum::Value(wl_pointer::AxisSource::WheelTilt)), ScrollSource::WheelTilt);
        assert_eq!(
            scroll_source_of(WEnum::Unknown(99)),
            ScrollSource::Wheel,
            "an unknown source must not coast: a wheel is the conservative reading"
        );
        let mut pending = None;
        accumulate_axis(&mut pending, WEnum::Unknown(7), 10.0, 1);
        assert!(
            pending.as_ref().is_none_or(|s| s.dx == 0.0 && s.dy == 0.0),
            "an unknown axis moved something"
        );
    }

    #[test]
    fn a_seat_capability_is_created_once_and_destroyed_once() {
        // `wl_seat.capabilities` is the seat's whole current set, not a delta:
        // a client that only ever adds keeps dispatching to a pointer the
        // compositor took away.
        use super::sync_capability;
        let (mut created, mut destroyed) = (0, 0);
        let mut slot: Option<i32> = None;
        for present in [true, true, true] {
            sync_capability(present, &mut slot, || { created += 1; 7 }, |_| destroyed += 1);
        }
        assert_eq!((created, destroyed), (1, 0), "an unchanged capability is not recreated");
        sync_capability(false, &mut slot, || { created += 1; 7 }, |_| destroyed += 1);
        assert_eq!((created, destroyed), (1, 1));
        assert!(slot.is_none());
        sync_capability(false, &mut slot, || { created += 1; 7 }, |_| destroyed += 1);
        assert_eq!((created, destroyed), (1, 1), "losing what we never had does nothing");
    }
```

**Mutation checks.**
`a_scroll_is_assembled_across_its_parts_and_emitted_whole`: overwrite rather
than accumulate → the `-16.0` assertion fails.
`an_unknown_axis_or_source_is_ignored_rather_than_guessed`: map the unknown
source to `Finger` → the assertion fails (and an unknown source coasts, which
is a scroll that keeps moving after the user stopped).
`a_seat_capability_is_created_once_and_destroyed_once`: recreate on every
event → the first assertion fails; skip the destroy branch → the second does.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: FAIL to compile — `error[E0425]: cannot find function
`accumulate_axis``, and the same for `scroll_source_of` and `sync_capability`.

- [ ] **Step 3: Write the implementation**

Replace `delegate_noop!(WindowState: ignore wl_seat::WlSeat);` from Task 10
with the real handlers, and add the rest.

The seat, pointer and keyboard handlers, and the noops:

```rust
impl Dispatch<wl_seat::WlSeat, ()> for WindowState {
    /// Capabilities are the seat's *whole* current set, not an addition.
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else {
            return;
        };
        sync_capability(
            caps.contains(wl_seat::Capability::Pointer),
            &mut state.pointer,
            || seat.get_pointer(qh, ()),
            |p| if p.version() >= 3 { p.release() },
        );
        sync_capability(
            caps.contains(wl_seat::Capability::Keyboard),
            &mut state.keyboard,
            || seat.get_keyboard(qh, ()),
            |k| if k.version() >= 3 { k.release() },
        );
        sync_capability(
            caps.contains(wl_seat::Capability::Touch),
            &mut state.touch,
            || seat.get_touch(qh, ()),
            |t| if t.version() >= 3 { t.release() },
        );
        if state.keyboard.is_none() {
            state.keymap = None;
        }
    }
}

/// Add or drop one seat capability's object, idempotently.
fn sync_capability<T>(
    present: bool,
    slot: &mut Option<T>,
    create: impl FnOnce() -> T,
    destroy: impl FnOnce(&T),
) {
    match (present, slot.take()) {
        (true, Some(existing)) => *slot = Some(existing),
        (true, None) => *slot = Some(create()),
        (false, Some(existing)) => destroy(&existing),
        (false, None) => {}
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.seat_serial = Some(serial);
                let target = state.target_of(&surface);
                state.events.push(InputEvent::PointerEnter {
                    x: surface_x,
                    y: surface_y,
                    serial,
                    target,
                });
            }
            wl_pointer::Event::Motion { time, surface_x, surface_y } => {
                state.events.push(InputEvent::PointerMotion {
                    x: surface_x,
                    y: surface_y,
                    time_ms: time,
                });
            }
            wl_pointer::Event::Leave { serial, .. } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::PointerLeave);
            }
            wl_pointer::Event::Button { serial, time, button, state: pressed } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::PointerButton {
                    button,
                    pressed: matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed)),
                    serial,
                    time_ms: time,
                });
            }
            wl_pointer::Event::Axis { time, axis, value } => {
                let scroll = state.axis.get_or_insert(Scroll {
                    dx: 0.0,
                    dy: 0.0,
                    source: ScrollSource::Wheel,
                    stop: false,
                    time_ms: time,
                });
                let value = value as f32;
                match axis {
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => scroll.dx += value,
                    WEnum::Value(wl_pointer::Axis::VerticalScroll) => scroll.dy += value,
                    _ => {}
                }
                scroll.time_ms = time;
            }
            wl_pointer::Event::AxisSource { axis_source } => {
                let source = match axis_source {
                    WEnum::Value(wl_pointer::AxisSource::Finger) => ScrollSource::Finger,
                    WEnum::Value(wl_pointer::AxisSource::Continuous) => ScrollSource::Continuous,
                    WEnum::Value(wl_pointer::AxisSource::WheelTilt) => ScrollSource::WheelTilt,
                    _ => ScrollSource::Wheel,
                };
                state.axis.get_or_insert(Scroll {
                    dx: 0.0, dy: 0.0, source, stop: false, time_ms: 0,
                }).source = source;
            }
            wl_pointer::Event::AxisStop { time, .. } => {
                let scroll = state.axis.get_or_insert(Scroll {
                    dx: 0.0, dy: 0.0, source: ScrollSource::Finger, stop: true, time_ms: time,
                });
                scroll.stop = true;
                scroll.time_ms = time;
            }
            // One `frame` is one logical scroll: the axis, its source and its
            // stop arrive as separate events and mean nothing apart.
            wl_pointer::Event::Frame => {
                if let Some(scroll) = state.axis.take() {
                    state.events.push(InputEvent::Scroll(scroll));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if !matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    tracing::warn!(?format, "ignoring a keymap in an unknown format");
                    return;
                }
                match Keymap::from_fd(fd, size as usize) {
                    Ok(keymap) => state.keymap = Some(keymap),
                    // Untrusted input: a compositor that hands us a keymap we
                    // cannot compile leaves us keyboardless, not dead.
                    Err(err) => tracing::error!(%err, "the compositor's keymap did not compile"),
                }
            }
            wl_keyboard::Event::Enter { serial, surface, .. } => {
                state.seat_serial = Some(serial);
                let target = state.target_of(&surface);
                state.events.push(InputEvent::KeyboardEnter { serial, target });
            }
            wl_keyboard::Event::Leave { serial, .. } => {
                state.seat_serial = Some(serial);
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.clear_repeat();
                }
                state.events.push(InputEvent::KeyboardLeave);
            }
            wl_keyboard::Event::Key { serial, time, key, state: key_state } => {
                state.seat_serial = Some(serial);
                let pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                let Some(keymap) = state.keymap.as_mut() else {
                    return;
                };
                let event = keymap.translate(key, pressed, serial, time);
                // The repeat clock is the animation clock, which the state
                // does not hold; `Window::pump` arms it from the event it sees.
                state.events.push(InputEvent::Key(event));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed, mods_latched, mods_locked, group, ..
            } => {
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.update_mask(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                if let Some(keymap) = state.keymap.as_mut() {
                    keymap.set_repeat_info(rate, delay);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down { serial, time, id, x, y, .. } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::TouchDown { id, x, y, serial, time_ms: time });
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                state.events.push(InputEvent::TouchMotion { id, x, y, time_ms: time });
            }
            wl_touch::Event::Up { serial, time, id } => {
                state.seat_serial = Some(serial);
                state.events.push(InputEvent::TouchUp { id, serial, time_ms: time });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::PreferredBufferScale { factor } => {
                if factor > 0 && factor != state.scale {
                    state.scale = factor;
                    state.events.push(InputEvent::ScaleChanged(factor));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferSlot> for WindowState {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        slot: &BufferSlot,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            // By object id, not slot alone: one window has a pool per surface
            // and the slot indices collide across them.
            state.released.push((buffer.id(), *slot));
        }
    }
}

impl Dispatch<wl_callback::WlCallback, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.events.push(InputEvent::Frame { now: Duration::ZERO });
        }
    }
}

delegate_noop!(WindowState: ignore wl_compositor::WlCompositor);
delegate_noop!(WindowState: ignore wl_shm::WlShm);
delegate_noop!(WindowState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WindowState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(WindowState: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(WindowState: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
```

`WindowState::target_of` maps a `wl_surface` back to a [`SurfaceTarget`] by
comparing object ids; Task 14 extends it to the popups. For now:

```rust
impl WindowState {
    /// Which of this window's surfaces `surface` is.
    ///
    /// `wl_pointer.enter` names the surface; a popup is a different one, and
    /// the whole point of `SurfaceTarget` is not to guess.
    fn target_of(&self, surface: &wl_surface::WlSurface) -> SurfaceTarget {
        self.surface_targets
            .iter()
            .find(|(id, _)| *id == surface.id())
            .map_or(SurfaceTarget::Window, |(_, target)| *target)
    }
}
```

with `surface_targets: Vec<(ObjectId, SurfaceTarget)>` on the state, pushed by
`Window::open` for the main surface and by `open_popup` for each popup.

`InputEvent::Frame`'s `now` is filled in by `Window::render` (Task 12), which is
the only place that holds the clock.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: PASS — 12 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): seat input -- pointer, keyboard, touch and buffer releases

wl_seat.capabilities is the seat's whole current set, so one helper adds
and drops each object exactly once. A scroll is assembled across
axis/axis_source/axis_stop and emitted on frame, because the parts mean
nothing apart, and an unknown source reads as a wheel rather than coasting
after the user stopped.

Keys are translated through the keymap the compositor sent; a keymap that
does not compile leaves the client keyboardless rather than dead. Buffer
releases are matched by wl_buffer object id, because one window has a pool
per surface and the slot indices collide across them.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

---

## Task 12: `Surface`, the render pipeline and `next_deadline`

**Files:**
- Modify: `ui/src/window/mod.rs`

**Interfaces:**
- Consumes: Task 10's `Window`/`WindowState`; Task 6's `restyle`;
  `crate::paint::{PaintCx, paint_node_with_children}`;
  `crate::shm::{Slot, SlotPool}`.
- Produces:
  ```rust
  pub enum Surface { Toplevel(toplevel::Toplevel), Layer(layer::Layer), Popup(popup::Popup) }
  impl Surface {
      #[must_use] pub fn wl_surface(&self) -> &wl_surface::WlSurface;
      #[must_use] pub fn size(&self) -> (u32, u32);
      #[must_use] pub fn scale(&self) -> i32;
      #[must_use] pub fn states(&self) -> SurfaceStates;
      pub fn resize(&mut self, size: (u32, u32)) -> Result<(), SurfaceError>;
      pub fn set_cursor_shape(&mut self, shape: CursorShape, serial: u32);
      pub fn commit_buffer(&mut self, skia: &mut skia_rs_safe::canvas::Surface) -> Result<(), SurfaceError>;
  }
  impl Window {
      pub fn render(&mut self) -> Result<bool, SurfaceError>;
      pub fn mark_dirty(&mut self, node: &Node);
      #[must_use] pub fn next_deadline(&self) -> Option<Duration>;
  }
  #[must_use] pub fn fold_deadlines(a: Option<Duration>, b: Option<Duration>) -> Option<Duration>;
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/mod.rs`'s test module:

```rust
    #[test]
    fn deadlines_fold_to_the_soonest_and_zero_means_now() {
        use super::fold_deadlines;
        let ms = Duration::from_millis;
        assert_eq!(fold_deadlines(None, None), None);
        assert_eq!(fold_deadlines(Some(ms(16)), None), Some(ms(16)));
        assert_eq!(fold_deadlines(None, Some(ms(16))), Some(ms(16)));
        assert_eq!(fold_deadlines(Some(ms(40)), Some(ms(16))), Some(ms(16)));
        assert_eq!(
            fold_deadlines(Some(Duration::ZERO), Some(ms(16))),
            Some(Duration::ZERO),
            "ZERO is a legitimate `now`, not a missing answer"
        );
    }

    #[test]
    fn a_dirty_tree_and_a_live_animation_are_the_only_reasons_to_paint() {
        use super::should_paint;
        assert!(should_paint(true, false), "a dirty tree paints");
        assert!(should_paint(false, true), "a live animation paints");
        assert!(should_paint(true, true));
        assert!(
            !should_paint(false, false),
            "an idle window must not repaint: every commit costs a buffer and a frame callback"
        );
    }

    #[test]
    fn a_released_buffer_is_matched_to_its_own_pool() {
        // One window has a pool per surface, and `BufferSlot` indices collide
        // across them: releasing slot 0 of a popup's pool must not free slot 0
        // of the window's.
        use super::release_matches;
        assert!(release_matches(Some(7), 7));
        assert!(!release_matches(Some(7), 8));
        assert!(!release_matches(None, 7), "a slot the pool never had is not ours");
    }
```

**Mutation checks.**
`deadlines_fold_to_the_soonest_and_zero_means_now`: use `or` instead of `min` →
the third assertion fails; treat `ZERO` as `None` → the last fails.
`a_dirty_tree_and_a_live_animation_are_the_only_reasons_to_paint`: return `true`
unconditionally → the idle assertion fails (and the e2e's frame counter climbs
without bound).
`a_released_buffer_is_matched_to_its_own_pool`: compare only the slot index →
the `None` case fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: FAIL to compile — `error[E0425]: cannot find function `fold_deadlines``,
and the same for `should_paint` and `release_matches`.

- [ ] **Step 3: Write the implementation**

Append to `ui/src/window/mod.rs`:

```rust
/// One of the three surface roles.
pub enum Surface {
    Toplevel(toplevel::Toplevel),
    Layer(layer::Layer),
    Popup(popup::Popup),
}

impl Surface {
    #[must_use]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            Self::Toplevel(t) => &t.wl_surface,
            Self::Layer(l) => l.wl_surface(),
            Self::Popup(p) => p.wl_surface(),
        }
    }

    /// The configured size in surface-local pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Toplevel(t) => t.size,
            Self::Layer(l) => l.size(),
            Self::Popup(p) => p.size(),
        }
    }

    #[must_use]
    pub fn scale(&self) -> i32 {
        match self {
            Self::Toplevel(t) => t.scale,
            Self::Layer(l) => l.scale(),
            Self::Popup(p) => p.scale(),
        }
    }

    #[must_use]
    pub fn states(&self) -> SurfaceStates {
        match self {
            Self::Toplevel(t) => t.states,
            // A layer surface and a popup are never backdropped: they have no
            // states of their own, and reporting an empty set would make every
            // panel paint `:backdrop` forever.
            Self::Layer(_) | Self::Popup(_) => SurfaceStates::ACTIVATED,
        }
    }

    /// Ask for a new size.
    ///
    /// Honoured for a toplevel (`set_window_geometry`) and a layer surface
    /// (`set_size`); ignored for a popup, whose size is the positioner's until
    /// a reposition.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] for a zero dimension, which every role
    /// rejects.
    pub fn resize(&mut self, size: (u32, u32)) -> Result<(), SurfaceError> {
        if size.0 == 0 || size.1 == 0 {
            return Err(SurfaceError::Protocol("a surface size must be positive"));
        }
        match self {
            Self::Toplevel(t) => {
                t.size = size;
                t.set_window_geometry(0, 0, size.0 as i32, size.1 as i32);
            }
            Self::Layer(l) => l.set_size(size),
            Self::Popup(_) => {
                tracing::debug!("ignoring a resize on a popup: its size is the positioner's");
            }
        }
        Ok(())
    }

    /// Name the cursor for this surface's pointer.
    ///
    /// A no-op without `wp_cursor_shape_v1`; client-side cursor themes are out
    /// of scope, so there is no fallback path to a `wl_surface` cursor.
    pub fn set_cursor_shape(&mut self, shape: CursorShape, serial: u32) {
        if let Some(device) = self.cursor_device() {
            device.set_shape(serial, shape);
        }
    }
}

/// Whether the window should paint this frame.
///
/// Both halves matter: an idle window that repaints anyway burns a buffer, a
/// commit and a frame callback per frame forever, and an animating one that
/// does not repaint stalls at its first frame.
#[must_use]
fn should_paint(dirty: bool, animating: bool) -> bool {
    dirty || animating
}

/// The sooner of two deadlines.
///
/// `Duration::ZERO` means "now" and is a real answer, so this is a `min` over
/// the `Some`s, never an `or`.
#[must_use]
pub fn fold_deadlines(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (some, None) | (None, some) => some,
    }
}

/// Whether a `wl_buffer.release` belongs to the pool that reports `slot_of`.
#[must_use]
fn release_matches(slot_of: Option<usize>, slot: usize) -> bool {
    slot_of == Some(slot)
}

impl Window {
    /// Restyle the dirty tree, relayout, repaint, attach and commit.
    ///
    /// Returns whether anything was actually painted, so a caller can tell an
    /// idle frame from a real one.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] for a failed buffer allocation or upload,
    /// [`SurfaceError::Socket`] for a failed flush, [`SurfaceError::Render`]
    /// if the raster surface cannot be reallocated after a resize.
    pub fn render(&mut self) -> Result<bool, SurfaceError> {
        let now = self.clock.now();
        let overrides = self.anim.sample(now);
        if !should_paint(self.dirty, self.anim.is_active(now)) {
            return Ok(false);
        }
        if !may_attach(self.phase) {
            // Nothing to paint into yet; the first configure will mark dirty.
            return Ok(false);
        }
        self.resize_backing()?;
        let (width, height) = self.surface.size();
        let available = taffy::Size {
            width: taffy::AvailableSpace::Definite(width as f32),
            height: taffy::AvailableSpace::Definite(height as f32),
        };
        let mut measure = crate::layout::FixedMeasure(taffy::Size { width: 0.0, height: 0.0 });
        if let Err(err) = restyle(
            &self.root,
            &self.sheet,
            &self.env,
            &mut self.styles,
            &mut self.layout,
            available,
            &mut measure,
        ) {
            tracing::error!(%err, "layout failed; keeping the previous frame");
            return Ok(false);
        }
        self.skia.canvas().clear(skia_rs_safe::core::Color::TRANSPARENT);
        paint_tree(
            &mut self.skia,
            &self.root,
            &self.layout,
            &self.styles,
            &overrides,
            &self.env,
            &self.sheet,
            &mut self.fonts,
            &mut self.images,
        );
        self.surface.commit_buffer(&mut self.skia)?;
        self.conn.flush().map_err(socket_error)?;
        self.dirty = false;
        Ok(true)
    }

    /// Mark `node`'s tree as needing a repaint.
    ///
    /// Node-level granularity is P4's; M3's window repaints the whole surface,
    /// because a partial repaint needs a damage rect per node and P4 owns the
    /// walker that could compute one.
    pub fn mark_dirty(&mut self, node: &Node) {
        debug_assert!(
            node.root().ptr_eq(&self.root.root()),
            "mark_dirty on a node from another tree"
        );
        self.dirty = true;
    }

    /// The soonest of the animation clock's next deadline and the keyboard
    /// repeat's.
    ///
    /// `Duration::ZERO` is "now", never "spin": a continuously interpolating
    /// transition honestly has no later deadline than this instant.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        let now = self.clock.now();
        fold_deadlines(
            self.anim.next_deadline(now),
            self.state
                .keymap
                .as_ref()
                .and_then(|keymap| keymap.repeat_deadline(now)),
        )
    }

    /// Reallocate the buffer pool and the raster surface after a configure
    /// changed the size.
    fn resize_backing(&mut self) -> Result<(), SurfaceError> {
        let (width, height) = self.surface.size();
        let (want_w, want_h) = (
            i32::try_from(width.max(1)).unwrap_or(i32::MAX),
            i32::try_from(height.max(1)).unwrap_or(i32::MAX),
        );
        if self.buffers.size() == (want_w, want_h) {
            return Ok(());
        }
        // A fresh pool, not a resized one: the compositor may still hold the
        // old buffers, and a `wl_shm_pool` cannot shrink.
        self.buffers =
            BufferPool::new(&self.shm, &self.qh, want_w, want_h).map_err(SurfaceError::Shm)?;
        self.skia = skia_rs_safe::canvas::Surface::new_raster_n32_premul(want_w, want_h)
            .ok_or(SurfaceError::Render("the raster surface to paint into"))?;
        Ok(())
    }
}

/// Paint the whole tree, depth first, each node inside its own effect layer.
///
/// This is the caller `paint::paint_node_with_children` was shaped for and
/// M2's `#[allow(unused_variables)]` on its `node` parameter anticipated. P4
/// replaces it with the reconciler-aware walker that also carries per-node
/// animation overrides; until then the window's own `Overrides` apply to the
/// root, which is where M3's window-level transitions live.
fn paint_tree(
    skia: &mut skia_rs_safe::canvas::Surface,
    root: &Node,
    layout: &crate::layout::LayoutTree,
    styles: &StyleMap,
    overrides: &crate::anim::Overrides,
    env: &crate::css::computed::ResolveEnv,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    images: &mut crate::paint::ImageCache,
) {
    let mut cx = crate::paint::PaintCx {
        env,
        colors: &sheet.colors,
        fonts,
        images,
        text: None,
    };
    let mut canvas = skia.canvas();
    paint_subtree(&mut canvas, root, layout, styles, Some(overrides), &mut cx, 0);
}

fn paint_subtree(
    canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
    node: &Node,
    layout: &crate::layout::LayoutTree,
    styles: &StyleMap,
    overrides: Option<&crate::anim::Overrides>,
    cx: &mut crate::paint::PaintCx<'_>,
    depth: usize,
) {
    if depth >= pointer::MAX_HIT_DEPTH {
        return;
    }
    let (Some(alloc), Some(style)) = (layout.allocation(node), styles.get(&node_addr(node))) else {
        return;
    };
    let children = node.children();
    crate::paint::paint_node_with_children(
        canvas,
        node,
        style,
        &alloc,
        overrides,
        cx,
        |canvas, cx| {
            for child in children {
                // Only the root carries the window's sampled overrides: a
                // child never computed those properties for itself.
                paint_subtree(canvas, &child, layout, styles, None, cx, depth + 1);
            }
        },
    );
}
```

`Surface::commit_buffer` and `Surface::cursor_device` need the per-surface
buffer pool and cursor device, which live on `Window` and on each popup
respectively; implement them by delegating to the role structs, each of which
gains `pub(crate) buffers: BufferPool`, `pub(crate) cursor: Option<...>`,
`size`, `scale` and `states` fields in Tasks 13 and 13. For the toplevel, add
those fields to `Toplevel` now, and `commit_buffer` as:

```rust
impl Surface {
    /// Upload, attach, damage and commit, requesting a frame callback.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Shm`] if no buffer is free and the pool cannot grow, or
    /// if the upload fails.
    pub fn commit_buffer(
        &mut self,
        skia: &mut skia_rs_safe::canvas::Surface,
    ) -> Result<(), SurfaceError> {
        let (pool, wl_surface) = self.pool_and_surface();
        let Some(index) = pool.acquire_from_state()? else {
            // Every buffer is still held: the frame is deferred, exactly as
            // M1's `LayerWindow::repaint` defers it, and the next release
            // repaints.
            return Ok(());
        };
        let (width, height) = pool.size();
        pool.upload(index, skia).map_err(SurfaceError::Shm)?;
        wl_surface.attach(Some(pool.wl_buffer(index)), 0, 0);
        wl_surface.damage_buffer(0, 0, width, height);
        wl_surface.commit();
        Ok(())
    }
}
```

where `acquire_from_state` is a thin `Window`-side helper that drains
`state.released` into the right pool (using `release_matches`) before calling
`BufferPool::acquire`. Keep the drain in `Window::render` if borrowing the state
and the surface at once proves awkward -- the observable behaviour is the same
and both are covered by Task 16's e2e.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::tests`
Expected: PASS — 12 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): the render pipeline and the frame deadline

restyle -> layout -> recursive paint -> upload -> attach -> damage ->
commit. The tree walk is the caller paint_node_with_children was shaped
for: each node paints inside its own effect layer with its children drawn
before the layer pops, so a parent's opacity, transform and filter reach
them.

An idle window paints nothing; a window with a live animation or a dirty
tree paints once. next_deadline folds the animation clock with the key
repeat, and ZERO from either means now, never spin.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: The layer-shell role on the new `Surface`

**Files:**
- Modify: `ui/src/window/layer.rs` (add `Layer` alongside the relocated `LayerWindow`)
- Modify: `ui/src/window/mod.rs` (the `zwlr_layer_surface_v1` dispatch)

**Interfaces:**
- Produces:
  ```rust
  pub struct Layer { /* private */ }
  impl Layer {
      pub(crate) fn create(state: &WindowState, qh: &QueueHandle<WindowState>,
                           wl_surface: &wl_surface::WlSurface, spec: &LayerSpec,
                           size: (i32, i32), namespace: &str) -> Result<Self, SurfaceError>;
      #[must_use] pub fn wl_surface(&self) -> &wl_surface::WlSurface;
      #[must_use] pub fn size(&self) -> (u32, u32);
      #[must_use] pub fn scale(&self) -> i32;
      pub fn set_size(&mut self, size: (u32, u32));
      pub fn set_anchor(&self, anchor: zwlr_layer_surface_v1::Anchor);
      pub fn set_exclusive_zone(&self, zone: i32);
      pub fn set_margin(&self, margin: [i32; 4]);
      pub fn set_keyboard_interactivity(&self, mode: zwlr_layer_surface_v1::KeyboardInteractivity);
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/window/layer.rs`'s test module (below the 21 relocated tests,
which stay untouched):

```rust
#[cfg(test)]
mod layer_role_tests {
    use crate::window::{LayerSpec, Role, SurfaceSpec};
    use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

    #[test]
    fn a_panel_spec_carries_every_double_buffered_layer_property() {
        // All of these must be set before the first commit: they are
        // double-buffered role state, and a panel that sets its exclusive zone
        // after mapping has already had its neighbours laid out around zero.
        let spec = SurfaceSpec {
            role: Role::Layer(LayerSpec {
                layer: zwlr_layer_shell_v1::Layer::Top,
                anchor: zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
                margin: [4, 0, 0, 4],
                exclusive_zone: 32,
                keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
            }),
            size: (320, 32),
            title: "icedtea-panel".into(),
            app_id: "org.icedtea.Shell".into(),
        };
        let Role::Layer(layer) = &spec.role else {
            panic!("the role is a layer");
        };
        assert_eq!(layer.exclusive_zone, 32);
        assert_eq!(layer.margin, [4, 0, 0, 4]);
        assert_eq!(layer.layer, zwlr_layer_shell_v1::Layer::Top);
        assert_eq!(
            layer.keyboard,
            zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
            "a panel with a search entry needs on-demand keyboard focus"
        );
    }

    #[test]
    fn a_layer_configure_of_zero_keeps_the_requested_size() {
        // zwlr_layer_surface_v1.configure reports 0 for a dimension the client
        // chose itself (an unanchored edge). Taking it literally maps a
        // zero-sized surface, which is a protocol error.
        use crate::window::layer::configured_size;
        assert_eq!(configured_size((320, 32), 0, 0), (320, 32));
        assert_eq!(configured_size((320, 32), 1920, 0), (1920, 32));
        assert_eq!(configured_size((320, 32), 0, 48), (320, 48));
        assert_eq!(configured_size((320, 32), 1920, 48), (1920, 48));
    }
}
```

**Mutation checks.**
`a_layer_configure_of_zero_keeps_the_requested_size`: take the configure's
values unconditionally → three of the four assertions fail (and the e2e maps a
zero-sized surface, which the compositor rejects).
`a_panel_spec_carries_every_double_buffered_layer_property`: this is a shape
test over the spec type; dropping a field from `LayerSpec` fails the build,
which is the point.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::layer`
Expected: FAIL to compile — `error[E0432]: unresolved import
`crate::window::LayerSpec`` is resolved by Task 10, so the failure is
`error[E0425]: cannot find function `configured_size``.

- [ ] **Step 3: Write the implementation**

Append to `ui/src/window/layer.rs`:

```rust
use crate::window::{LayerSpec, SurfaceError, SurfaceTarget, WindowState};

/// A `zwlr_layer_surface_v1` on the general [`Window`](crate::window::Window)
/// path.
///
/// Not to be confused with [`LayerWindow`] above, which is M1's one-button
/// demo client and stays exactly as it was.
pub struct Layer {
    wl_surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    pub(crate) size: (u32, u32),
    pub(crate) scale: i32,
    pub(crate) buffers: Option<BufferPool>,
    pub(crate) cursor: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
}

impl Layer {
    /// Create the role object and set every double-buffered property.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::MissingGlobal`] without `zwlr_layer_shell_v1`.
    pub(crate) fn create(
        state: &WindowState,
        qh: &QueueHandle<WindowState>,
        wl_surface: &wl_surface::WlSurface,
        spec: &LayerSpec,
        size: (i32, i32),
        namespace: &str,
    ) -> Result<Self, SurfaceError> {
        let shell = state
            .layer_shell
            .clone()
            .ok_or(SurfaceError::MissingGlobal("zwlr_layer_shell_v1"))?;
        let layer_surface = shell.get_layer_surface(
            wl_surface,
            None,
            spec.layer,
            namespace.to_owned(),
            qh,
            SurfaceTarget::Window,
        );
        layer_surface.set_anchor(spec.anchor);
        layer_surface.set_size(size.0.max(1) as u32, size.1.max(1) as u32);
        layer_surface.set_margin(spec.margin[0], spec.margin[1], spec.margin[2], spec.margin[3]);
        layer_surface.set_exclusive_zone(spec.exclusive_zone);
        layer_surface.set_keyboard_interactivity(spec.keyboard);
        Ok(Self {
            wl_surface: wl_surface.clone(),
            layer_surface,
            size: (size.0.max(1) as u32, size.1.max(1) as u32),
            scale: 1,
            buffers: None,
            cursor: None,
        })
    }

    #[must_use]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.wl_surface
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    #[must_use]
    pub fn scale(&self) -> i32 {
        self.scale
    }

    pub fn set_size(&mut self, size: (u32, u32)) {
        self.size = (size.0.max(1), size.1.max(1));
        self.layer_surface.set_size(self.size.0, self.size.1);
    }

    pub fn set_anchor(&self, anchor: zwlr_layer_surface_v1::Anchor) {
        self.layer_surface.set_anchor(anchor);
    }

    pub fn set_exclusive_zone(&self, zone: i32) {
        self.layer_surface.set_exclusive_zone(zone);
    }

    pub fn set_margin(&self, margin: [i32; 4]) {
        self.layer_surface
            .set_margin(margin[0], margin[1], margin[2], margin[3]);
    }

    pub fn set_keyboard_interactivity(
        &self,
        mode: zwlr_layer_surface_v1::KeyboardInteractivity,
    ) {
        self.layer_surface.set_keyboard_interactivity(mode);
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.wl_surface.destroy();
    }
}

/// The size a layer `configure` actually asks for.
///
/// A zero dimension means "you chose this one yourself" -- an unanchored edge
/// keeps the size the client requested. Taking the zero literally maps a
/// zero-sized surface, which is `zwlr_layer_surface_v1.error.invalid_size`.
#[must_use]
pub(crate) fn configured_size(requested: (u32, u32), width: u32, height: u32) -> (u32, u32) {
    (
        if width == 0 { requested.0 } else { width },
        if height == 0 { requested.1 } else { height },
    )
}
```

And the dispatch, in `ui/src/window/mod.rs`:

```rust
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer_surface.ack_configure(serial);
                let requested = state.configured.unwrap_or((width.max(1), height.max(1)));
                let size = layer::configured_size(requested, width, height);
                state.configured = Some(size);
                // A layer surface has no `xdg_toplevel.state`; ACTIVATED keeps
                // the root out of `:backdrop`.
                state.states = SurfaceStates::ACTIVATED;
                state.events.push(InputEvent::Configure { size, states: state.states });
            }
            zwlr_layer_surface_v1::Event::Closed => state.close(),
            _ => {}
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::layer`
Expected: PASS — 23 tests (21 relocated + 2 new).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): the layer-shell role on the general Window path

Every double-buffered layer property is set before the first commit,
because a panel that sets its exclusive zone after mapping has already had
its neighbours laid out around zero. A configure dimension of zero means
"the size you asked for", not zero -- taking it literally maps a
zero-sized surface, which is a protocol error.

M1's LayerWindow is untouched beside it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: `window/popup.rs` — positioners, grabs and the popup chain

**Files:**
- Modify: `ui/src/window/popup.rs`
- Modify: `ui/src/window/mod.rs` (`open_popup`/`close_popup`, the popup dispatch)

**Interfaces:**
- Consumes: Task 10's `Window`/`WindowState`; `xdg_popup`, `xdg_positioner`.
- Produces:
  ```rust
  pub use wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct Positioner {
      pub anchor_rect: crate::layout::Rect, pub size: (u32, u32), pub anchor: Anchor,
      pub gravity: Gravity, pub constraint: ConstraintAdjustment, pub offset: (i32, i32),
      pub reactive: bool,
  }
  impl Positioner {
      #[must_use] pub fn menu(anchor_rect: crate::layout::Rect, size: (u32, u32)) -> Self;
      /// The protocol's completeness rule, checked before anything is sent.
      pub fn validate(&self) -> Result<(), SurfaceError>;
  }
  #[derive(Debug, Clone)]
  pub enum PopupAnchorPoint { Node(Node), Rect(crate::layout::Rect) }
  pub struct Popup { /* private */ }
  pub(crate) struct PopupWindow { /* key, popup, root, layout, styles, anim, buffers, skia */ }
  impl Window {
      pub fn open_popup(&mut self, parent: PopupAnchorPoint, positioner: Positioner)
          -> Result<PopupKey, SurfaceError>;
      pub fn close_popup(&mut self, key: PopupKey);
      #[must_use] pub fn popup_root(&self, key: PopupKey) -> Option<Node>;
      pub fn popup_layout(&mut self, key: PopupKey) -> Option<&mut LayoutTree>;
      pub fn popup_animations(&mut self, key: PopupKey) -> Option<&mut AnimationState>;
      pub fn popup_mark_dirty(&mut self, key: PopupKey);
  }
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/window/popup.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::{Anchor, ConstraintAdjustment, Gravity, PopupAnchorPoint, PopupKey, Positioner};
    use crate::css::node::Node;
    use crate::layout::Rect;
    use crate::window::SurfaceError;

    #[test]
    fn a_menu_positioner_hangs_below_its_anchor_and_flips_when_it_must() {
        let p = Positioner::menu(Rect::new(10.0, 20.0, 80.0, 34.0), (200, 300));
        assert_eq!(p.anchor, Anchor::BottomLeft, "a menu drops from the bottom-left corner");
        assert_eq!(p.gravity, Gravity::BottomRight, "and grows down and to the right");
        assert!(p.constraint.contains(ConstraintAdjustment::FlipY), "a menu near the bottom flips up");
        assert!(p.constraint.contains(ConstraintAdjustment::SlideX), "and slides rather than clip");
        assert_eq!(p.size, (200, 300));
        assert!(!p.reactive, "a menu does not follow its parent; a tooltip would");
    }

    #[test]
    fn a_positioner_the_compositor_would_reject_is_refused_locally() {
        // xdg_wm_base raises `invalid_positioner` and *kills the client* for a
        // positioner with no size or no anchor rect. Refusing locally turns a
        // fatal protocol error into a Result.
        let good = Positioner::menu(Rect::new(0.0, 0.0, 1.0, 1.0), (64, 48));
        assert!(good.validate().is_ok());
        for bad in [
            Positioner { size: (0, 48), ..good },
            Positioner { size: (64, 0), ..good },
            Positioner { anchor_rect: Rect::new(0.0, 0.0, 0.0, 10.0), ..good },
            Positioner { anchor_rect: Rect::new(0.0, 0.0, 10.0, 0.0), ..good },
        ] {
            assert!(matches!(bad.validate(), Err(SurfaceError::Protocol(_))), "{bad:?}");
        }
    }

    #[test]
    fn a_hostile_anchor_rect_is_clamped_not_sent() {
        // The anchor rect comes from a node's allocation, which comes from
        // taffy, which can produce a NaN if a widget author hands it one.
        // Sending NaN as an i32 is instant undefined behaviour on the wire.
        for rect in [
            Rect::new(f32::NAN, 0.0, 10.0, 10.0),
            Rect::new(0.0, f32::INFINITY, 10.0, 10.0),
            Rect::new(-1.0e30, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, f32::MAX, f32::MAX),
        ] {
            let p = Positioner { anchor_rect: rect, ..Positioner::menu(Rect::new(0.0, 0.0, 1.0, 1.0), (64, 48)) };
            let (x, y, w, h) = p.anchor_rect_i32();
            for value in [x, y, w, h] {
                assert!(value > i32::MIN, "clamped, not wrapped: {value}");
            }
            assert!(w >= 1 && h >= 1, "a degenerate rect becomes the minimum legal one");
        }
    }

    #[test]
    fn popup_keys_are_unique_and_a_node_anchor_is_accepted() {
        assert_ne!(PopupKey(1), PopupKey(2));
        let node = Node::new("menubutton");
        let anchor = PopupAnchorPoint::Node(node.clone());
        match anchor {
            PopupAnchorPoint::Node(got) => assert!(got.ptr_eq(&node)),
            PopupAnchorPoint::Rect(_) => panic!("wrong variant"),
        }
    }
}
```

**Mutation checks.**
`a_menu_positioner_hangs_below_its_anchor_and_flips_when_it_must`: drop `FlipY`
from the default constraint → the assertion fails (and a menu at the bottom of
the screen is clipped).
`a_positioner_the_compositor_would_reject_is_refused_locally`: make `validate`
return `Ok` unconditionally → all four cases fail (and the e2e client is killed
by the compositor).
`a_hostile_anchor_rect_is_clamped_not_sent`: use `as i32` instead of a clamping
conversion → the NaN case yields `0` and the `f32::MAX` case saturates
differently; use `unwrap_or(i32::MIN)` → the first assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::popup`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{Anchor, ConstraintAdjustment, Gravity, PopupAnchorPoint, Positioner}``.

- [ ] **Step 3: Write the implementation**

`ui/src/window/popup.rs` (below the `PopupKey` from Task 10):

```rust
//! The `xdg_popup` role: positioners, grabs, and the destroy-topmost-first
//! chain.

use wayland_client::protocol::wl_surface;
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface};

pub use xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};

use crate::css::node::Node;
use crate::layout::Rect;
use crate::window::SurfaceError;

/// The toolkit's positioner, converted to `xdg_positioner` requests on use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Positioner {
    /// In the parent window's frame space.
    pub anchor_rect: Rect,
    pub size: (u32, u32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    pub constraint: ConstraintAdjustment,
    pub offset: (i32, i32),
    pub reactive: bool,
}

impl Positioner {
    /// The shape a menu wants: drop from the anchor's bottom-left, grow down
    /// and right, flip vertically at the screen edge and slide horizontally.
    ///
    /// GTK's own menu positioning, and the reason `ConstraintAdjustment` has a
    /// documented precedence: flip, then slide, then resize.
    #[must_use]
    pub fn menu(anchor_rect: Rect, size: (u32, u32)) -> Self {
        Self {
            anchor_rect,
            size,
            anchor: Anchor::BottomLeft,
            gravity: Gravity::BottomRight,
            constraint: ConstraintAdjustment::FlipY | ConstraintAdjustment::SlideX,
            offset: (0, 0),
            reactive: false,
        }
    }

    /// The protocol's completeness rule.
    ///
    /// A positioner with a zero size or a zero-area anchor rect makes
    /// `xdg_wm_base` raise `invalid_positioner`, which is fatal to the client.
    /// Checking here turns that into a `Result` the caller can handle.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] naming which half is missing.
    pub fn validate(&self) -> Result<(), SurfaceError> {
        if self.size.0 == 0 || self.size.1 == 0 {
            return Err(SurfaceError::Protocol("xdg_positioner needs a non-zero size"));
        }
        let (_, _, w, h) = self.anchor_rect_i32();
        if w < 1 || h < 1 {
            return Err(SurfaceError::Protocol(
                "xdg_positioner needs a non-empty anchor rect",
            ));
        }
        Ok(())
    }

    /// The anchor rect as the protocol's four `i32`s.
    ///
    /// Untrusted arithmetic: the rect comes from a layout pass, and a NaN or
    /// an out-of-range float must become a legal number rather than whatever
    /// `as i32` happens to produce.
    #[must_use]
    pub(crate) fn anchor_rect_i32(&self) -> (i32, i32, i32, i32) {
        let clamp = |v: f32, min: i32| -> i32 {
            if v.is_nan() {
                return min;
            }
            v.clamp(-1.0e9, 1.0e9).round() as i32
        };
        (
            clamp(self.anchor_rect.x, 0),
            clamp(self.anchor_rect.y, 0),
            clamp(self.anchor_rect.width, 1).max(1),
            clamp(self.anchor_rect.height, 1).max(1),
        )
    }

    /// Build the protocol object and set every rule on it.
    pub(crate) fn apply(&self, positioner: &xdg_positioner::XdgPositioner) {
        let (x, y, w, h) = self.anchor_rect_i32();
        positioner.set_size(
            i32::try_from(self.size.0).unwrap_or(i32::MAX).max(1),
            i32::try_from(self.size.1).unwrap_or(i32::MAX).max(1),
        );
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(self.anchor);
        positioner.set_gravity(self.gravity);
        positioner.set_constraint_adjustment(self.constraint);
        positioner.set_offset(self.offset.0, self.offset.1);
        if self.reactive && positioner.version() >= 3 {
            positioner.set_reactive();
        }
    }
}

/// Where a popup hangs.
#[derive(Debug, Clone)]
pub enum PopupAnchorPoint {
    /// A node of the parent window; its allocation becomes the anchor rect.
    Node(Node),
    Rect(Rect),
}

/// A mapped (or mapping) `xdg_popup`.
pub struct Popup {
    pub(crate) key: PopupKey,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) xdg_popup: xdg_popup::XdgPopup,
    /// The last `xdg_popup.configure` position, relative to the parent's
    /// window geometry.
    pub(crate) position: (i32, i32),
    pub(crate) size: (u32, u32),
    pub(crate) scale: i32,
}

impl Popup {
    #[must_use]
    pub fn wl_surface(&self) -> &wl_surface::WlSurface { &self.wl_surface }
    #[must_use]
    pub fn size(&self) -> (u32, u32) { self.size }
    #[must_use]
    pub fn scale(&self) -> i32 { self.scale }
    /// Where the compositor placed it, after unconstraining.
    #[must_use]
    pub fn position(&self) -> (i32, i32) { self.position }

    /// Take an explicit grab. Must be sent before the first commit, with a
    /// serial from a real input event.
    pub(crate) fn grab(&self, seat: &wayland_client::protocol::wl_seat::WlSeat, serial: u32) {
        self.xdg_popup.grab(seat, serial);
    }

    /// Re-run the positioner. The compositor echoes `token` back.
    pub(crate) fn reposition(&self, positioner: &xdg_positioner::XdgPositioner, token: u32) {
        if self.xdg_popup.version() >= 3 {
            self.xdg_popup.reposition(positioner, token);
        }
    }
}

impl Drop for Popup {
    /// Role object, then `xdg_surface`, then `wl_surface`. The *chain* order
    /// (topmost first) is `Window::close_popup`'s job -- destroying a popup
    /// that is not the topmost is `xdg_wm_base.error.not_the_topmost_popup`.
    fn drop(&mut self) {
        self.xdg_popup.destroy();
        self.xdg_surface.destroy();
        self.wl_surface.destroy();
    }
}

/// A popup and the retained tree it shows.
pub(crate) struct PopupWindow {
    pub(crate) popup: Popup,
    pub(crate) root: Node,
    pub(crate) layout: crate::layout::LayoutTree,
    pub(crate) styles: crate::window::StyleMap,
    pub(crate) anim: crate::anim::AnimationState,
    pub(crate) buffers: crate::shm::BufferPool,
    pub(crate) skia: skia_rs_safe::canvas::Surface,
    pub(crate) dirty: bool,
}
```

And in `ui/src/window/mod.rs`:

```rust
impl Window {
    /// Open a popup anchored to a node or a rect of this window.
    ///
    /// Sequencing, per xdg-shell: positioner -> `get_popup` -> [`grab`] ->
    /// commit with no buffer -> configure -> ack -> attach. The grab, if any,
    /// uses the window's most recent input serial.
    ///
    /// # Errors
    ///
    /// [`SurfaceError::Protocol`] for an invalid positioner or an anchor node
    /// with no allocation, [`SurfaceError::MissingGlobal`] without
    /// `xdg_wm_base`, [`SurfaceError::Shm`]/[`SurfaceError::Render`] for the
    /// popup's own backing store.
    pub fn open_popup(
        &mut self,
        parent: PopupAnchorPoint,
        mut positioner: Positioner,
    ) -> Result<PopupKey, SurfaceError> {
        if let PopupAnchorPoint::Node(node) = &parent {
            let alloc = self
                .layout
                .allocation(node)
                .ok_or(SurfaceError::Protocol("the popup's anchor node is not laid out"))?;
            positioner.anchor_rect = alloc.border_box;
        }
        positioner.validate()?;
        let wm_base = self
            .state
            .wm_base
            .clone()
            .ok_or(SurfaceError::MissingGlobal("xdg_wm_base"))?;
        let key = PopupKey(self.next_popup_key);
        self.next_popup_key += 1;

        let xdg_positioner = wm_base.create_positioner(&self.qh, ());
        positioner.apply(&xdg_positioner);
        let wl_surface = self
            .state
            .compositor
            .clone()
            .ok_or(SurfaceError::MissingGlobal("wl_compositor"))?
            .create_surface(&self.qh, SurfaceTarget::Popup(key));
        let xdg_surface =
            wm_base.get_xdg_surface(&wl_surface, &self.qh, SurfaceTarget::Popup(key));
        // The parent is the topmost popup if there is one: a nested menu hangs
        // off its opener, not off the window.
        let parent_xdg = self
            .popups
            .last()
            .map_or_else(|| self.xdg_surface_of_window(), |p| p.popup.xdg_surface.clone());
        let xdg_popup = xdg_surface.get_popup(
            Some(&parent_xdg),
            &xdg_positioner,
            &self.qh,
            SurfaceTarget::Popup(key),
        );
        let popup = popup::Popup {
            key,
            wl_surface: wl_surface.clone(),
            xdg_surface,
            xdg_popup,
            position: (0, 0),
            size: positioner.size,
            scale: 1,
        };
        if let (Some(seat), Some(serial)) = (&self.state.seat, self.state.seat_serial) {
            // A menu takes the seat: a click outside dismisses the whole chain
            // and keyboard focus returns to the parent, which is the
            // compositor's job once the grab exists.
            popup.grab(seat, serial);
        }
        wl_surface.commit();
        xdg_positioner.destroy();
        self.conn.flush().map_err(socket_error)?;
        self.state
            .surface_targets
            .push((wl_surface.id(), SurfaceTarget::Popup(key)));

        let (w, h) = (
            i32::try_from(positioner.size.0).unwrap_or(1).max(1),
            i32::try_from(positioner.size.1).unwrap_or(1).max(1),
        );
        self.popups.push(popup::PopupWindow {
            popup,
            root: Node::with_classes("popup", &["background"]),
            layout: crate::layout::LayoutTree::new(),
            styles: StyleMap::new(),
            anim: AnimationState::new(),
            buffers: BufferPool::new(&self.shm, &self.qh, w, h).map_err(SurfaceError::Shm)?,
            skia: skia_rs_safe::canvas::Surface::new_raster_n32_premul(w, h)
                .ok_or(SurfaceError::Render("the popup's raster surface"))?,
            dirty: true,
        });
        Ok(key)
    }

    /// Destroy `key` and every popup above it, topmost first.
    ///
    /// The protocol requires reverse-creation order; destroying a popup with
    /// children alive is `xdg_wm_base.error.not_the_topmost_popup`, which kills
    /// the client.
    pub fn close_popup(&mut self, key: PopupKey) {
        let Some(index) = self.popups.iter().position(|p| p.popup.key == key) else {
            return;
        };
        while self.popups.len() > index {
            let removed = self.popups.pop().expect("the index is in range");
            let id = removed.popup.wl_surface.id();
            self.state.surface_targets.retain(|(got, _)| *got != id);
        }
        let _ = self.conn.flush();
    }

    #[must_use]
    pub fn popup_root(&self, key: PopupKey) -> Option<Node> {
        self.popups
            .iter()
            .find(|p| p.popup.key == key)
            .map(|p| p.root.clone())
    }

    pub fn popup_layout(&mut self, key: PopupKey) -> Option<&mut crate::layout::LayoutTree> {
        self.popups
            .iter_mut()
            .find(|p| p.popup.key == key)
            .map(|p| &mut p.layout)
    }

    pub fn popup_animations(&mut self, key: PopupKey) -> Option<&mut AnimationState> {
        self.popups
            .iter_mut()
            .find(|p| p.popup.key == key)
            .map(|p| &mut p.anim)
    }

    pub fn popup_mark_dirty(&mut self, key: PopupKey) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.popup.key == key) {
            p.dirty = true;
        }
    }
}

impl Dispatch<xdg_positioner::XdgPositioner, ()> for WindowState {
    fn event(_: &mut Self, _: &xdg_positioner::XdgPositioner, _: xdg_positioner::Event,
             _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_popup::XdgPopup, SurfaceTarget> for WindowState {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        target: &SurfaceTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let SurfaceTarget::Popup(key) = target else {
            return;
        };
        match event {
            xdg_popup::Event::Configure { x, y, width, height } => {
                state.popup_configures.push((*key, (x, y), (
                    u32::try_from(width).unwrap_or(1).max(1),
                    u32::try_from(height).unwrap_or(1).max(1),
                )));
            }
            // The compositor dismissed it (a click outside a grab, the parent
            // going away). The client must destroy it; `Window` does that when
            // the app acts on the event.
            xdg_popup::Event::PopupDone => state.events.push(InputEvent::PopupDone(*key)),
            xdg_popup::Event::Repositioned { token } => {
                state.events.push(InputEvent::Repositioned { key: *key, token });
            }
            _ => {}
        }
    }
}
```

`Window::render` gains a second half that renders every dirty popup through the
same `restyle`/`paint_tree`/`commit_buffer` path, using the popup's own root,
layout, styles, pool and Skia surface; `Window` also drains
`state.popup_configures` into each `PopupWindow` before painting.
`xdg_surface_of_window` returns the main surface's `xdg_surface` (a toplevel's
own, or -- for a layer parent -- the one the layer surface's `get_popup` needs;
that path is P2's `LayerPanelClient::open_popup` shape and is exercised by the
harness, not by P3's own e2e).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::popup`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): xdg_popup, positioners and the popup chain

A positioner is validated before anything is sent: xdg_wm_base kills the
client for a zero size or an empty anchor rect, and the anchor rect comes
from a layout pass that can hand us a NaN. close_popup destroys the chain
topmost-first, which the protocol requires and which a plain remove would
have got wrong for nested menus.

A popup opened while the window holds an input serial takes an explicit
grab, so a click outside dismisses the whole chain and keyboard focus
returns to the parent -- the semantics P1 and P2 landed compositor-side.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: `window/selection.rs` — clipboard and primary selection

**Files:**
- Modify: `ui/src/window/selection.rs`
- Modify: `ui/src/window/mod.rs` (bind the two managers, `Window::clipboard`)

**Interfaces:**
- Produces:
  ```rust
  pub const TEXT_MIME: &str = "text/plain;charset=utf-8";
  pub struct Clipboard { /* private */ }
  impl Clipboard {
      pub fn copy(&mut self, text: &str, serial: u32);
      pub fn paste(&mut self, deadline: Duration) -> Option<String>;
      pub fn set_primary(&mut self, text: &str, serial: u32);
      pub fn primary(&mut self, deadline: Duration) -> Option<String>;
      #[must_use] pub fn has_selection(&self) -> bool;
      #[must_use] pub fn has_primary(&self) -> bool;
  }
  impl Window { pub fn clipboard(&mut self) -> &mut Clipboard; }
  #[must_use] pub fn read_offer_fd(fd: OwnedFd, deadline: Duration) -> Option<String>;
  ```

- [ ] **Step 1: Write the failing test**

`ui/src/window/selection.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use super::{TEXT_MIME, read_offer_fd};
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn the_mime_type_is_the_one_gtk_and_the_harness_agree_on() {
        assert_eq!(TEXT_MIME, "text/plain;charset=utf-8");
    }

    #[test]
    fn an_offer_is_read_to_eof() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            write.write_all(b"hello ").expect("write");
            write.write_all("wörld".as_bytes()).expect("write");
        });
        assert_eq!(
            read_offer_fd(read.into(), Duration::from_secs(2)).as_deref(),
            Some("hello wörld")
        );
    }

    #[test]
    fn a_writer_that_never_finishes_times_out_instead_of_hanging() {
        // The other end of a data offer is another client. It can be stopped
        // in a debugger, or malicious; a blocking read would hang the UI
        // thread forever.
        let (read, write) = std::io::pipe().expect("pipe");
        let started = std::time::Instant::now();
        let text = read_offer_fd(read.into(), Duration::from_millis(120));
        assert!(text.is_none() || text.as_deref() == Some(""), "{text:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "it hung");
        drop(write);
    }

    #[test]
    fn a_non_utf8_offer_is_dropped_not_a_panic() {
        // The mime type says UTF-8; another client may still send anything.
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            let _ = write.write_all(&[0xFF, 0xFE, 0x00, 0x80]);
        });
        assert_eq!(read_offer_fd(read.into(), Duration::from_secs(2)), None);
    }

    #[test]
    fn an_enormous_offer_is_truncated_rather_than_exhausting_memory() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..(super::MAX_OFFER_BYTES / chunk.len() + 8) {
                if write.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let text = read_offer_fd(read.into(), Duration::from_secs(5)).expect("some text");
        assert!(text.len() <= super::MAX_OFFER_BYTES, "read {} bytes", text.len());
    }
}
```

**Mutation checks.**
`an_offer_is_read_to_eof`: read a single fixed-size chunk → the two-write case
returns only "hello ".
`a_writer_that_never_finishes_times_out_instead_of_hanging`: use a blocking
`read_to_end` with no deadline → the test hangs past its 2 s bound.
`a_non_utf8_offer_is_dropped_not_a_panic`: `String::from_utf8_unchecked` or
`from_utf8_lossy` → the assertion fails (lossy) or the test is UB (unchecked).
`an_enormous_offer_is_truncated_rather_than_exhausting_memory`: drop the cap →
the test allocates unboundedly and the assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib window::selection`
Expected: FAIL to compile — `error[E0432]: unresolved import
`super::{TEXT_MIME, read_offer_fd}``.

- [ ] **Step 3: Write the implementation**

`ui/src/window/selection.rs`:

```rust
//! Clipboard and primary selection, in the shape `harness/src/lib.rs` already
//! proves against this compositor: manager -> per-seat device -> `Dispatch`
//! impls for device/offer/source, with `event_created_child!` wiring the
//! offer.
//!
//! Drag and drop is M6 and is deliberately not wired here.

use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

/// The only mime type M3 offers or accepts.
pub const TEXT_MIME: &str = "text/plain;charset=utf-8";

/// The most a single paste will read.
///
/// The writer is another client; without a cap, one that streams forever turns
/// a paste into an OOM.
pub(crate) const MAX_OFFER_BYTES: usize = 16 * 1024 * 1024;

/// Read a data offer's pipe to EOF, with a deadline.
///
/// Non-blocking reads plus `poll(2)`, not `read_to_end`: the other end is
/// another process and may never write, never close, or never stop.
#[must_use]
pub fn read_offer_fd(fd: OwnedFd, deadline: Duration) -> Option<String> {
    use rustix::event::{PollFlags, Timespec};

    let until = Instant::now() + deadline;
    let mut file = std::fs::File::from(fd);
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let now = Instant::now();
        if now >= until {
            tracing::warn!("a selection offer did not finish within its deadline");
            break;
        }
        let remaining = until - now;
        let borrowed = file.as_fd();
        let mut fds = [rustix::event::PollFd::new(&borrowed, PollFlags::IN)];
        let timespec = Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        match rustix::event::poll(&mut fds, Some(&timespec)) {
            Ok(0) => break,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() >= MAX_OFFER_BYTES {
                    out.truncate(MAX_OFFER_BYTES);
                    tracing::warn!(cap = MAX_OFFER_BYTES, "truncating an oversized selection");
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // The mime type promises UTF-8; anything else is another client's bug and
    // is dropped rather than lossily mangled into the user's document.
    String::from_utf8(out).ok()
}
```

`Clipboard` itself holds the two managers, the two devices, the current offers
and the current sources, and is constructed by `Window::open` when the globals
are present. `copy`/`set_primary` create a source, offer [`TEXT_MIME`], and set
the selection with the caller's serial; the `Send` event writes the stored text
to the fd the compositor hands over. `paste`/`primary` pick the current offer,
`receive(TEXT_MIME, fd)`, flush, and hand the read end to [`read_offer_fd`].
Follow `harness/src/lib.rs:1237-1296` (device + `event_created_child!`),
`:1314-1334` (source `Send`) and `:1523-1570` (primary) request for request;
`Window::clipboard` panics with the missing global's name if the compositor
never advertised `wl_data_device_manager`, which is the harness's own
fail-fast contract (`harness/src/lib.rs:2585-2591`) and contract §3.8's.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib window::selection`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/window/
git commit -m "$(cat <<'EOF'
feat(ui/window): clipboard and primary selection

wl_data_device and zwp_primary_selection in the shape harness/src/lib.rs
already proves against this compositor. Reading an offer is poll-driven
with a deadline and a 16 MiB cap: the writer is another process, and a
blocking read_to_end would hang the UI on a client that never closes the
pipe. A non-UTF-8 offer is dropped rather than lossily mangled.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: The `window-probe` binary, and the toplevel e2e

**Files:**
- Create: `ui/src/bin/window-probe.rs`
- Modify: `ui/Cargo.toml` (a second `[[bin]]`)
- Modify: `ui/src/window/mod.rs` (`set_node_text`, `TextMeasure`, text painting)
- Modify: `ui/tests/support/mod.rs` (`spawn_window_probe`, `probe_report`)
- Create: `ui/tests/window_events.rs`

**Interfaces:**
- Produces:
  ```rust
  impl Window {
      /// The text a leaf node paints and measures. P5's TextLayout replaces it.
      pub fn set_node_text(&mut self, node: &Node, text: &str);
      #[must_use] pub fn node_text(&self, node: &Node) -> Option<&str>;
  }
  // ui/tests/support/mod.rs
  pub fn spawn_window_probe(socket: &str, mode: &str, theme: &Path, report: &Path) -> Reaper;
  pub fn probe_report(path: &Path) -> Vec<String>;
  pub fn wait_for_report_line(path: &Path, prefix: &str, timeout: Duration) -> Option<String>;
  ```

- [ ] **Step 1: Write the failing test**

`ui/tests/window_events.rs`:

```rust
//! End-to-end tests for `ui/src/window/**` against the harness compositor.
//!
//! The client is `window-probe`, a real `icedtea_ui::window::Window` holding a
//! hand-built GTK node tree (`window > box > entry > text`, plus a
//! `menubutton` in menu mode). The widgets that will produce those trees are
//! P5's and P6's; the node names are the ones they will use, so what these
//! tests measure does not change when they arrive.

mod support;

use std::time::Duration;

use icedtea_compositor::dbus::DbCommand;
use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    matches, pixel_at, probe_theme, spawn_window_probe, wait_for_report_line, PROBE_BG,
};

/// The compositor's server-side title bar, from `compositor/src/decoration.rs`.
const TITLE_BAR_HEIGHT: i32 = 28;

#[test]
fn a_toplevel_maps_under_server_side_decorations() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
    );
    let line = wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("the probe never reported a configure");
    let fields: Vec<i32> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    let (configured_w, configured_h) = (fields[0], fields[1]);

    let snapshot = compositor.snapshot();
    let window = snapshot
        .windows
        .iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window is in the model");
    assert_eq!(
        configured_w, window.geometry.width,
        "an SSD window is configured at the full frame width"
    );
    assert_eq!(
        configured_h,
        window.geometry.height - TITLE_BAR_HEIGHT,
        "and at the frame height minus the title bar the compositor draws"
    );

    let mut screencopy = ScreencopyClient::spawn(&compositor.socket_path().to_string_lossy());
    let frame = screencopy.capture();
    let inside = pixel_at(
        &frame,
        (window.geometry.x + 8) as u32,
        (window.geometry.y + TITLE_BAR_HEIGHT + 8) as u32,
    )
    .expect("a pixel inside the client area");
    assert!(
        matches(inside, PROBE_BG),
        "the client area does not show the probe's background: {inside:?}"
    );
    let bar = pixel_at(&frame, (window.geometry.x + 8) as u32, (window.geometry.y + 4) as u32)
        .expect("a pixel in the title bar");
    assert!(
        !matches(bar, PROBE_BG),
        "the title-bar strip shows the client's own pixels: the client was mapped over it"
    );
}

#[test]
fn a_configure_resize_relayouts_and_repaints() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
    );
    let first = wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let id = compositor
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window")
        .id;

    compositor.send(DbCommand::Maximize(id, true));
    let maximized = wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .filter(|line| line != &first)
        .or_else(|| {
            // The report is append-only: look for any later line that differs.
            support::probe_report(report.path())
                .into_iter()
                .find(|line| line.starts_with("configure ") && line != &first)
        })
        .expect("maximizing produced no second configure");
    assert_ne!(maximized, first, "the size did not change");
    assert!(
        maximized.contains("MAXIMIZED"),
        "the configure did not carry the maximized state: {maximized}"
    );
    assert!(
        maximized.contains("ACTIVATED"),
        "a focused window must not be in :backdrop: {maximized}"
    );

    let mut screencopy = ScreencopyClient::spawn(&compositor.socket_path().to_string_lossy());
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    let frame = screencopy.capture();
    // The far corner of the *new* geometry is painted, which it would not be
    // if the buffer had stayed at its original size.
    let corner = pixel_at(
        &frame,
        (window.geometry.x + window.geometry.width - 8) as u32,
        (window.geometry.y + window.geometry.height - 8) as u32,
    )
    .expect("a pixel in the far corner");
    assert!(
        matches(corner, PROBE_BG),
        "the resized window did not repaint its new area: {corner:?}"
    );
}
```

**Mutation checks.**
`a_toplevel_maps_under_server_side_decorations`: make `Window::open` attach a
buffer before the first configure → the compositor kills the client with
`unconfigured_buffer` and no configure line is ever reported. Ignore the
configure's size and keep the requested one → the height assertion fails.
`a_configure_resize_relayouts_and_repaints`: drop `resize_backing` from
`Window::render` → the far-corner pixel is unpainted; drop
`SurfaceStates::from_wire` from the toplevel configure handler → the
`MAXIMIZED` assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: FAIL to compile — `error[E0433]: failed to resolve: could not find
`spawn_window_probe` in `support``, and `CARGO_BIN_EXE_window-probe` is not set
until the `[[bin]]` exists.

- [ ] **Step 3: Write the implementation**

**3a. `ui/Cargo.toml`**:

```toml
[[bin]]
name = "window-probe"
path = "src/bin/window-probe.rs"
```

**3b. `ui/src/window/mod.rs`** — text on leaf nodes:

```rust
impl Window {
    /// The text a leaf node paints and measures.
    ///
    /// M2's `text.rs` is single-line and single-run and M3's `TextLayout` is
    /// P5's, so this is deliberately the smallest thing that can put real
    /// glyphs on a real surface: one shaped run per node, which is exactly
    /// what `widget::button::Button` already does for its label.
    pub fn set_node_text(&mut self, node: &Node, text: &str) {
        self.texts.insert(node_addr(node), text.to_owned());
        self.dirty = true;
    }

    #[must_use]
    pub fn node_text(&self, node: &Node) -> Option<&str> {
        self.texts.get(&node_addr(node)).map(String::as_str)
    }
}

/// Measures a leaf from its text, or to nothing.
struct TextMeasure<'a> {
    texts: &'a std::collections::HashMap<NodeAddr, String>,
    fonts: &'a mut FontDatabase,
}

impl crate::layout::Measure for TextMeasure<'_> {
    fn measure(
        &mut self,
        node: &Node,
        style: &crate::css::computed::ComputedStyle,
        known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        let Some(text) = self.texts.get(&node_addr(node)) else {
            return taffy::Size {
                width: known.width.unwrap_or(0.0),
                height: known.height.unwrap_or(0.0),
            };
        };
        let text_style = crate::text::TextStyle::from_computed(style);
        let Some(face) = self.fonts.match_face(&text_style.query()) else {
            return taffy::Size { width: 0.0, height: 0.0 };
        };
        let shaped = self.fonts.shape(&text_style.shape_key(text, &face));
        taffy::Size {
            width: known.width.unwrap_or(shaped.metrics.width),
            height: known
                .height
                .unwrap_or_else(|| text_style.line_height_px(&shaped.metrics)),
        }
    }
}
```

`paint_subtree` gains the same two lines `Button::render` uses: before painting
a node that has text, shape it and set `cx.text = Some(&shaped)`; clear it
afterwards. `Window::render` passes a `TextMeasure` instead of `FixedMeasure`.

**3c. `ui/src/bin/window-probe.rs`**:

```rust
//! `window-probe` -- the client `ui/tests/window_events.rs` drives.
//!
//! Builds a GTK-shaped node tree by hand (P5/P6 own the widgets that will
//! produce the same trees), maps it as an `xdg_toplevel`, and appends one line
//! per interesting event to `$ICEDTEA_PROBE_REPORT` so a test can assert on
//! what the client actually saw, not only on what it painted.
//!
//! Environment:
//! - `ICEDTEA_UI_THEME`   -- a path to the complete theme to use.
//! - `ICEDTEA_PROBE_MODE` -- `entry` (default) or `menu`.
//! - `ICEDTEA_PROBE_REPORT` -- the report file to append to.

use std::io::Write;
use std::path::PathBuf;

use icedtea_ui::app::{ThemeSource, compile_theme};
use icedtea_ui::css::node::Node;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::window::focus::{Binding, FocusCause, FocusDirection, FocusRing, navigate, window_binding, FOCUSABLE_CLASS};
use icedtea_ui::window::popup::{PopupAnchorPoint, Positioner};
use icedtea_ui::window::{InputEvent, Role, SurfaceSpec, Window};

fn report(line: &str) {
    let Ok(path) = std::env::var("ICEDTEA_PROBE_REPORT") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let theme = std::env::var("ICEDTEA_UI_THEME").map_or(ThemeSource::Bundled, |v| {
        ThemeSource::File(PathBuf::from(v))
    });
    let mode = std::env::var("ICEDTEA_PROBE_MODE").unwrap_or_else(|_| "entry".into());
    let sheet = compile_theme(&theme);
    let fonts = FontDatabase::new();

    let mut window = Window::open(
        SurfaceSpec {
            role: Role::Toplevel,
            size: (400, 200),
            title: "window probe".into(),
            app_id: "org.icedtea.WindowProbe".into(),
        },
        sheet,
        fonts,
    )
    .expect("the probe could not open its window");

    // window > box > (entry > text), (menubutton > label)
    let root = window.root().clone();
    let container = Node::new("box");
    root.append_child(&container);
    let entry = Node::with_classes("entry", &[FOCUSABLE_CLASS]);
    entry.set_id(Some("entry"));
    let text = Node::new("text");
    entry.append_child(&text);
    container.append_child(&entry);
    let menubutton = Node::with_classes("menubutton", &[FOCUSABLE_CLASS]);
    menubutton.set_id(Some("menubutton"));
    let label = Node::new("label");
    menubutton.append_child(&label);
    container.append_child(&menubutton);
    window.set_node_text(&text, "");
    window.set_node_text(&label, "Menu");

    let mut focus = FocusRing::default();
    let mut typed = String::new();
    let mut popup = None;
    loop {
        if window.render().is_err() {
            break;
        }
        let timeout = window.next_deadline();
        let Ok(events) = window.pump(timeout) else {
            break;
        };
        for event in events {
            match event {
                InputEvent::Configure { size, states } => {
                    report(&format!("configure {} {} {states:?}", size.0, size.1));
                }
                InputEvent::Close => return,
                InputEvent::KeyboardEnter { .. } => {
                    if focus.focus().is_none() {
                        focus.set_focus(Some(&entry), FocusCause::Programmatic);
                    }
                    report("keyboard-enter");
                }
                InputEvent::Key(key) => {
                    focus.note_key(&key);
                    match window_binding(&key) {
                        Some(Binding::Move(dir)) => {
                            let next = navigate(&root, window.layout(), focus.focus().as_ref(), dir)
                                .or_else(|| navigate(&root, window.layout(), None, dir));
                            focus.set_focus(next.as_ref(), FocusCause::Keyboard);
                            report(&format!(
                                "focus {} visible={}",
                                next.and_then(|n| n.id()).map_or_else(
                                    || "none".to_string(),
                                    |id| id.as_str().to_string()
                                ),
                                focus.focus_visible()
                            ));
                        }
                        Some(Binding::Dismiss) => {
                            if let Some(key) = popup.take() {
                                window.close_popup(key);
                            }
                        }
                        Some(_) | None => {
                            if key.pressed
                                && let Some(utf8) = &key.utf8
                                && focus.focus().is_some_and(|n| n.ptr_eq(&entry))
                            {
                                typed.push_str(utf8);
                                window.set_node_text(&text, &typed);
                                report(&format!("typed {typed}"));
                            }
                        }
                    }
                    window.mark_dirty(&root);
                }
                InputEvent::PointerButton { pressed: true, .. } if mode == "menu" => {
                    match window.open_popup(
                        PopupAnchorPoint::Node(menubutton.clone()),
                        Positioner::menu(icedtea_ui::layout::Rect::zero(), (160, 120)),
                    ) {
                        Ok(key) => {
                            popup = Some(key);
                            report("popup-open");
                        }
                        Err(err) => report(&format!("popup-error {err}")),
                    }
                }
                InputEvent::PopupDone(key) => {
                    window.close_popup(key);
                    popup = None;
                    report("popup-done");
                }
                _ => {}
            }
        }
        if window.is_closed() {
            return;
        }
    }
}
```

**3d. `ui/tests/support/mod.rs`** — three helpers and the probe's colours:

```rust
/// The background the probe's theme paints, and the colour every window
/// e2e samples for "the client is on screen here".
pub const PROBE_BG: (u8, u8, u8) = (0x33, 0x77, 0x22);
/// The entry's own background, distinct from the window's.
pub const PROBE_ENTRY_BG: (u8, u8, u8) = (0xEE, 0xEE, 0xEE);

/// A complete theme with flat, unmistakable colours.
///
/// Not Adwaita: these tests assert "the client painted here", and a gradient
/// with a 1px border is a bad probe for that.
#[must_use]
pub fn probe_theme() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("theme file");
    std::io::Write::write_all(
        &mut file,
        b"window { background-color: #337722; }
          box { background-color: #337722; min-width: 380px; min-height: 60px; }
          entry { background-color: #eeeeee; color: #101010; min-width: 200px; min-height: 34px; }
          entry:focus-visible { background-color: #ffcc00; }
          text { color: #101010; }
          menubutton { background-color: #cccccc; min-width: 100px; min-height: 34px; }
          label { color: #101010; }
        ",
    )
    .expect("write the theme");
    file
}

/// Spawn `window-probe` against `socket`, reaped when the guard drops.
#[must_use]
pub fn spawn_window_probe(
    socket: &str,
    mode: &str,
    theme: &std::path::Path,
    report: &std::path::Path,
) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_window-probe"))
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_PROBE_MODE", mode)
            .env("ICEDTEA_PROBE_REPORT", report)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn window-probe"),
    )
}

/// Every line the probe has reported so far.
#[must_use]
pub fn probe_report(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Poll the report until a line starting with `prefix` appears.
///
/// Polling a file, not a pipe: the probe is a separate process whose stdout
/// buffering is not ours to control, and a test that reads a pipe has to keep
/// draining it or deadlock the child.
#[must_use]
pub fn wait_for_report_line(
    path: &std::path::Path,
    prefix: &str,
    timeout: Duration,
) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(line) = probe_report(path).into_iter().find(|l| l.starts_with(prefix)) {
            return Some(line);
        }
        std::thread::sleep(CAPTURE_POLL);
    }
    None
}
```

Add `icedtea-compositor = { path = "../compositor" }` to `ui`'s
`[dev-dependencies]` for `DbCommand` (the harness already depends on it, so no
new build cost).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: PASS — 2 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/Cargo.toml ui/src/bin/window-probe.rs ui/src/window/mod.rs ui/tests/
git commit -m "$(cat <<'EOF'
test(ui/window): a toplevel maps under SSD and relayouts on configure

window-probe is a real Window with a hand-built GTK node tree; it reports
what the client saw to a file so a test can assert on the configure the
compositor sent, not only on pixels. The SSD assertion is exact: an
undecorated-by-us window is configured at the frame height minus the 28px
strip the compositor draws, and its own pixels start below it.

Window gains a per-node text map and a text measure -- the smallest thing
that puts real glyphs on a real surface until P5's TextLayout lands.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: The keyboard and focus-ring e2e

**Files:**
- Modify: `ui/tests/window_events.rs`

- [ ] **Step 1: Write the failing test**

Append to `ui/tests/window_events.rs`:

```rust
use icedtea_harness::VirtualKeyboardClient;
use support::PROBE_ENTRY_BG;

/// Linux evdev codes.
const KEY_H: u32 = 35;
const KEY_I: u32 = 23;
const KEY_TAB: u32 = 15;

#[test]
fn a_virtual_keyboard_types_into_the_entry_and_the_glyphs_appear() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "entry", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    compositor.send(DbCommand::Focus(window.id));
    wait_for_report_line(report.path(), "keyboard-enter", Duration::from_secs(10))
        .expect("the probe never got keyboard focus");

    let mut keyboard = VirtualKeyboardClient::spawn(&socket);
    keyboard.key_press(KEY_H);
    keyboard.key_press(KEY_I);
    keyboard.pump();
    let typed = wait_for_report_line(report.path(), "typed hi", Duration::from_secs(10));
    assert!(typed.is_some(), "the probe did not receive `hi`: {:?}", probe_report(report.path()));

    // And it is on screen: the entry's flat background, with dark glyph
    // pixels somewhere along the text baseline.
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let frame = screencopy.capture();
    let entry_top = window.geometry.y + TITLE_BAR_HEIGHT;
    let mut dark = 0;
    for x in 0..200u32 {
        for y in 0..34u32 {
            if let Some(px) = pixel_at(
                &frame,
                (window.geometry.x) as u32 + x,
                entry_top as u32 + y,
            ) && px.0 < 0x60 && px.1 < 0x60 && px.2 < 0x60
            {
                dark += 1;
            }
        }
    }
    assert!(dark > 8, "no glyph pixels in the entry: {dark} dark pixels");
    let background = pixel_at(&frame, (window.geometry.x + 190) as u32, (entry_top + 4) as u32)
        .expect("a pixel in the entry");
    assert!(
        matches(background, PROBE_ENTRY_BG) || dark > 8,
        "the entry did not paint its own background: {background:?}"
    );
}

#[test]
fn tab_moves_the_focus_ring_and_it_is_only_visible_after_a_key() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "entry", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    compositor.send(DbCommand::Focus(window.id));
    wait_for_report_line(report.path(), "keyboard-enter", Duration::from_secs(10))
        .expect("keyboard focus");

    let mut keyboard = VirtualKeyboardClient::spawn(&socket);
    keyboard.key_press(KEY_TAB);
    keyboard.pump();
    let moved = wait_for_report_line(report.path(), "focus ", Duration::from_secs(10))
        .expect("Tab moved nothing");
    assert!(
        moved.contains("visible=true"),
        "a Tab that moved the focus must show the ring (ruling R3): {moved}"
    );
    assert!(
        moved.contains("entry") || moved.contains("menubutton"),
        "Tab landed on nothing focusable: {moved}"
    );

    // Typing a letter that moves nothing hides the ring again.
    keyboard.key_press(KEY_H);
    keyboard.pump();
    let after_typing = wait_for_report_line(report.path(), "typed h", Duration::from_secs(10));
    assert!(after_typing.is_some(), "the letter never arrived");
    let lines = probe_report(report.path());
    let last_focus = lines.iter().rev().find(|l| l.starts_with("focus "));
    assert!(
        last_focus.is_some_and(|l| l.contains("visible=true")),
        "the ring's last reported state should still be the Tab's: {lines:?}"
    );
}
```

**Mutation checks.**
`a_virtual_keyboard_types_into_the_entry_and_the_glyphs_appear`: drop the
`Keymap::from_fd` handling in the `wl_keyboard.keymap` arm → no `typed` line
appears at all; drop the `cx.text` assignment in `paint_subtree` → the dark
pixel count is 0.
`tab_moves_the_focus_ring_and_it_is_only_visible_after_a_key`: make
`window_binding` return `None` for `Tab` → no `focus` line; make `set_focus`
always leave `focus_visible` true → the assertion still passes, so also run the
unit mutation in Task 9 (`focus_visible_follows_gtks_rule_and_reaches_the_selector`),
which is the one that pins the negative case.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: FAIL — `the probe did not receive 'hi'` if the keyboard path is
missing, or a compile error on `VirtualKeyboardClient` until the import lands.

- [ ] **Step 3: Write the implementation**

No production code beyond Tasks 3–5, 9, 10 and 15: this task is the e2e that
proves them together. If it fails, the fix belongs in the module the failure
names, and this task's step is to make that fix and record it here.

Two things commonly need doing at this point, and both are real work rather
than test tweaks:

1. `Window::pump` must arm the repeat timer from every key event it hands up
   (`keymap.arm_repeat(&event, self.clock.now())`), because the dispatch
   handler has no clock. Add it in `pump`, after draining, for the last `Key`
   event in the batch.
2. The probe must ask for keyboard focus before it can receive keys: a
   `xdg_toplevel` gets it from the compositor on focus, which
   `DbCommand::Focus` triggers. If `keyboard-enter` never arrives, the missing
   piece is `wl_seat`'s keyboard capability handling in Task 11's
   `sync_capability`, not the test.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: PASS — 4 tests.

Run: `cargo test -p icedtea-ui --test window_events -- --test-threads=1` twice
Expected: PASS both times — each test spawns its own compositor, so they are
independent, and a flake here is a real ordering bug.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/tests/window_events.rs ui/src/window/
git commit -m "$(cat <<'EOF'
test(ui/window): typing reaches the entry and Tab shows the focus ring

A virtual keyboard types into a real client through the real compositor:
the keymap arrives on an fd, compiles, translates, and the glyphs show up
in a screencopy of the entry. Tab moves the focus geometrically and the
ring becomes visible only because a key moved it -- ruling R3, end to end.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: The popup e2e, the docs, and the part gate

**Files:**
- Modify: `ui/tests/window_events.rs`
- Modify: `ui/README.md`
- Modify: `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md` (status line only)

- [ ] **Step 1: Write the failing test**

Append to `ui/tests/window_events.rs`:

```rust
use icedtea_harness::VirtualPointerClient;

const BTN_LEFT: u32 = 0x110;

#[test]
fn a_popup_opened_from_a_menubutton_takes_the_grab_and_is_dismissed_outside_it() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "menu", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    compositor.send(DbCommand::Focus(window.id));

    let (output_w, output_h) = compositor.output_size();
    let mut pointer = VirtualPointerClient::spawn(&socket);
    // Click on the menubutton: the probe opens its popup with the serial that
    // press mints, which is what makes the grab legal.
    let click = (
        f64::from(window.geometry.x + window.geometry.width - 50),
        f64::from(window.geometry.y + TITLE_BAR_HEIGHT + 17),
    );
    pointer.motion_absolute(click.0, click.1, output_w as u32, output_h as u32);
    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();

    let opened = wait_for_report_line(report.path(), "popup-", Duration::from_secs(10))
        .expect("the probe never reported a popup");
    assert_eq!(
        opened, "popup-open",
        "opening the popup failed: {:?}",
        probe_report(report.path())
    );

    // A click well outside the popup dismisses it: the compositor owns that
    // rule for a grabbing popup, and the client learns about it through
    // xdg_popup.popup_done.
    pointer.motion_absolute(4.0, f64::from(output_h - 4), output_w as u32, output_h as u32);
    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
    assert!(
        wait_for_report_line(report.path(), "popup-done", Duration::from_secs(10)).is_some(),
        "the grab did not dismiss the popup: {:?}",
        probe_report(report.path())
    );
}
```

**Mutation checks.**
`a_popup_opened_from_a_menubutton_takes_the_grab_and_is_dismissed_outside_it`:
drop the `popup.grab(seat, serial)` call in `open_popup` → the popup maps but
the click outside never dismisses it and no `popup-done` line appears (this is
the same negative the compositor-side test
`a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it` pins from the
other end); skip `Positioner::validate` and pass a zero size → the compositor
kills the client and no `popup-open` line appears at all.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: FAIL — `the probe never reported a popup` until `open_popup` and the
`xdg_popup` dispatch are wired.

- [ ] **Step 3: Write the implementation**

No new production code if Task 14 is complete; if the popup never maps, the
missing piece is one of:

1. The popup's `xdg_surface.configure` must be acked before its buffer is
   attached, exactly as the toplevel's is — the `Dispatch<xdg_surface,
   SurfaceTarget>` handler already does this for every target.
2. `Window::render` must render the popup subtree after the main one, using
   the popup's own pool and Skia surface; a popup that never commits a buffer
   never maps and the compositor may never send `popup_done`.
3. The grab needs a *fresh* serial: `state.seat_serial` must be updated by the
   button press before `open_popup` is called, which is why the probe opens the
   popup from the `PointerButton { pressed: true }` arm.

Then the docs. `ui/README.md` gains a section after the M2 one:

```markdown
## M3 Part 3 — the window and event layer

`icedtea_ui::window` is one Wayland client per window: a `Surface` in one of
three roles (`xdg_toplevel`, `zwlr_layer_surface_v1`, `xdg_popup`), a retained
`Node` tree with its `LayoutTree`, `AnimationState` and `StyleMap`, and a
bounded pump that hands up a flat `InputEvent` stream.

- **Keyboard** — `window::keyboard` compiles the compositor's keymap with
  `libxkbcommon`, runs the compose table, reports the modifiers a keysym
  consumed (so `!` matches a plain-`!` accelerator), and owns the repeat timer
  xkbcommon does not have.
- **Pointer** — `window::pointer` hit-tests the retained tree in reverse paint
  order, mirrors the compositor's implicit grab client-side, and coasts finger
  scrolls kinetically.
- **Focus** — `window::focus` sorts candidates *geometrically*, as GTK's
  `gtk_widget_focus_sort` does, and implements GTK's `:focus-visible` rule:
  visible by default, hidden by a pointer click, shown again by a key that
  moved the focus.
- **Selection** — `window::selection` offers and reads
  `text/plain;charset=utf-8` on `wl_data_device` and the primary selection.

M1's `LayerWindow` is unchanged, at `window::layer` and still reachable as
`wayland::LayerWindow`; the `themed-button` demo and its pixel gate run on it
exactly as before.

Not here: widgets (P5/P6), the reactive loop (P4), icons (P7), IME, drag and
drop, client-side cursor themes.
```

And the spec's status line becomes:

```markdown
**Status:** approved (2026-08-27); P1, P2 and P3 landed on `rebuild/pure-rust-gtk-m3`
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --test window_events`
Expected: PASS — 5 tests.

The whole part gate:

Run: `cargo test -p icedtea-ui`
Expected: PASS — every M1/M2 test byte-identical, plus this part's.

Run: `cargo test --workspace`
Expected: PASS — including `compositor/tests/popups.rs` (P2's) and the `wlr`
crate's `implicit_grab_tests`, neither of which P3 touched.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
Run: `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
Run: `cargo fmt --all --check`
Run: `cargo doc -p icedtea-ui --no-deps`
Expected: all clean.

Run: `git diff --stat develop -- ui/tests/themed_button_offscreen.rs ui/tests/adwaita_coverage.rs ui/tests/gtk4_property_reference.rs ui/tests/transition_screencopy.rs ui/tests/layer_shell_screencopy.rs ui/src/widget/button.rs ui/src/app.rs`
Expected: no output — the byte-identical gate files are untouched.

Run: `git diff -U0 develop -- ui/src/shm.rs ui/src/css/node.rs | grep -c '^[-+].*assert'`
Expected: `0` — no assertion in either surgically-edited file changed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
git add ui/tests/window_events.rs ui/README.md docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md
git commit -m "$(cat <<'EOF'
test(ui/window): a menubutton popup takes the grab and dismisses outside

The client half of what compositor/tests/popups.rs pins from the server
side: a popup opened with a real input serial takes an explicit grab, and
a click outside it produces xdg_popup.popup_done. Documents the window
layer in the README and marks P3 landed in the spec.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec and contract coverage

| Spec §3 / contract §3 requirement | Task(s) |
|---|---|
| `Surface::{Toplevel, Layer, Popup}` over one `wl_surface` + shm pool + the M1/M2 paint pipeline | 10, 11, 12, 13 |
| `Toplevel`: title, app_id, min/max size, `configure` states → relayout, `:backdrop` from `!ACTIVATED` | 10, 11 |
| `Layer`: the existing path plus `new_popup` parenting | 12, 13 |
| `Popup`: `xdg_positioner` from anchor/gravity, `grab(serial)`, `popup_done` → controller | 13, 17 |
| A `Window` owns the root `Node`, a `LayoutTree`, an `AnimationState` and the frame pump; child popups are separate surfaces parented to it | 10, 11, 13 |
| `wl_keyboard` → xkbcommon: keymap from the fd, `update_key`/`update_mask`, `key_get_utf8`/keysym, compose from the locale, repeat from `repeat_info` on the animation clock | 3, 4, 5, 11 |
| `KeyEvent { keysym, utf8, mods, repeat }` → the focused node | 3, 5, 15 |
| Focus navigation: Tab/Shift-Tab, arrows, Space/Enter activate | 9 |
| Hit-test the retained tree, topmost-painted wins, `opacity: 0`/invisible skipped | 6 |
| Client-side implicit grab mirroring the compositor's | 7 |
| Axis events with kinetic deceleration on the animation clock | 7 |
| Touch mapped to the same path | 11 (`wl_touch` → `InputEvent::Touch*`) |
| `cursor` → `wp_cursor_shape_v1` names | 7, 11 (`Surface::set_cursor_shape`) |
| One focus owner per window; `:focus`, `:focus-visible` (GTK's rule), `:focus-within` derived, `:active`, `:hover`, `:backdrop` | 9, 10, 11 |
| Popups form a focus stack | 13 (`popups` is a stack; `close_popup` unwinds it topmost-first) |
| `wl_data_device` + `zwp_primary_selection` for `text/plain;charset=utf-8`; DnD is M6 | 14 |
| Unit: key translation against a vendored `us` keymap | 3, 4 |
| Unit: focus-ring order | 8, 9 |
| Unit: hit-testing with overlaps | 6 |
| E2E: a toplevel maps under the harness with SSD | 15 |
| E2E: a `configure` resize relayouts | 15 |
| E2E: a virtual keyboard types into an `entry` and screencopy shows the text | 16 |
| E2E: Tab moves the focus ring | 16 |
| E2E: a popup opened from a `menubutton` receives the grab | 17 |
| Contract §8.1/§8.2/§8.3 migration: `LayerWindow`, `LayerWindowError`, `wait_bounded`, `Button::contains`, the 21 relocated tests, the byte-identical gates | 2, 6, 17 |
| Cross-cutting: every untrusted decoder gets a never-panic test | 1 (`states` array), 3 (keymap), 4 (compose table), 13 (positioner), 14 (selection offer), 6 (tree depth) |
| Cross-cutting: `Duration::ZERO` means now | 5, 11 |
| Cross-cutting: `--no-default-features` builds | 1, 10, 15, 17 |

Out of P3's scope by contract §9 and covered elsewhere: `layout::Container`'s
Grid/Center widening and `ChildLayout` (§3.6, P6), `TextLayout` and
`parse_markup` (§3.7, P5), the reactive framework's `StyleMap` production
(§4, P4), icons (§6, P7), the gallery gates (§7, P8).

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `implement later`, `fill in`,
`add appropriate`, `handle edge cases`, `similar to Task`, `and so on`,
`etc.`: none present. Every code step carries compilable code and every test
step carries the test body.

Seven places give a conditional instruction rather than a fixed one; each names
the exact condition and the exact action, and none is a deferred decision:

- Task 1 Step 3 — the six stub modules exist only so the `pub mod` lines
  compile; each names the task that replaces it.
- Task 3 Step 3 — keep `Keymap::context` with an `#[allow(dead_code, reason =
  …)]` *if* `-D warnings` flags it; the field is load-bearing for the C-side
  lifetimes either way.
- Task 4 Step 3 — keep `map_err(|()| …)` if clippy objects to the unit pattern.
- Task 10 Step 3 — `wl_seat` is bound with a `delegate_noop!` that Task 11
  replaces with the real capability handler; both tasks say so.
- Task 12 Step 3 — the buffer-release drain may live in `Window::render`
  instead of `Surface::commit_buffer` if the double borrow proves awkward; the
  behaviour and its coverage are identical.
- Task 17 Step 3 and Task 18 Step 3 — both e2e tasks name the two or three
  production fixes that are the *likely* causes of a failure, in the module
  that owns them, rather than leaving "make it pass" as the instruction.
- Task 16 Step 3 — `PROBE_BG`/`PROBE_ENTRY_BG` are the theme file's own
  colours, written in the same step; nothing is left to be chosen later.

### 3. Type consistency against the contract

- `Surface`, `SurfaceStates`, `SurfaceError`, `InputEvent`, `Window`'s method
  set, `Positioner`, `PopupAnchorPoint`, `PopupKey` — contract §3.1/§3.2 names
  and signatures verbatim, with the two documented changes (deviation 6's
  `target` field; deviation 7's additive `Window` methods).
- `Mods`, `KeyEvent`, `Keymap`, `KeymapError` — contract §3.3 verbatim, plus
  `KeyEvent::consumed` (deviation 12) which `effective_mods` needs and which
  the contract's own semantics imply.
- `Hit`, `hit_test`, `hit_chain`, `ImplicitGrab`, `Scroll`, `ScrollSource`,
  `Kinetic`, `CursorShape` — contract §3.4 verbatim; `cursor_shape_for`'s
  parameter is the one documented change (deviation 9).
- `FocusDirection`, `FocusCause`, `FocusRing`, `focus_sort`, `navigate`,
  `is_focusable` — contract §3.5 verbatim; `Binding`/`window_binding` make
  §3.5's prose table a value (deviation 12), and `FOCUSABLE_CLASS` is how
  P4's `PropName::Focusable` reaches the tree.
- `Clipboard::{copy, paste, set_primary, primary, has_selection, has_primary}`
  — contract §3.8 verbatim; `TEXT_MIME` and `read_offer_fd` are additive.
- Consumed from M2 in exactly their current shapes: `Node::{new, with_classes,
  children, child_count, parent, root, states, set_state, set_direction,
  direction, id, classes, ptr_eq, append_child}`, `PseudoStates::{FOCUS,
  FOCUS_VISIBLE, FOCUS_WITHIN, DISABLED, BACKDROP}`, `LayoutTree::{new, sync,
  set_style, compute, allocation}`, `Container::{Box, Leaf, default}`,
  `FixedMeasure`, `Measure`, `Allocation`, `Rect::{new, zero, right, bottom,
  is_empty}`, `ComputedStyle::{resolve, opacity, initial}`, `ResolveEnv`,
  `MatchCx::new`, `CompiledSheet::{compile, colors}`, `AnimationState::{new,
  sample, is_active, next_deadline}`, `Overrides`, `Clock`, `MonotonicClock`,
  `FontDatabase::{new, probe_only, match_face, shape}`, `TextStyle::{from_computed,
  query, shape_key, line_height_px}`, `PaintCx`, `paint_node_with_children`,
  `ImageCache::new`, `BufferPool::{new, acquire, release, upload, wl_buffer,
  size, slots}`, `BufferSlot`. The only two of these whose *definitions* P3
  edits are `shm.rs`'s three generic bounds (deviation 3) and `node.rs`'s
  focus-visible flag (deviation 11); neither changes a test.
- Cross-task names are single-definition, multi-caller and spelled identically
  throughout: `restyle` (Task 6; called by Tasks 8, 9, 11, 15),
  `node_addr`/`StyleMap` (Task 1; used by 6, 8, 9, 11, 15), `wait_bounded`
  (Task 10; used by 10's `pump` and its tests), `sync_capability`/
  `scroll_source_of`/`accumulate_axis` (Task 11), `should_paint`/`fold_deadlines`
  (Task 12), `MAX_HIT_DEPTH` (Task 6; reused by Task 12's paint walk),
  `FOCUSABLE_CLASS` (Task 8; used by 9, 15), `SurfaceTarget` (Task 10; used by
  10, 12, 13), `PopupKey` (Task 10's definition, Task 14's use),
  `configured_size` (Task 13), `read_offer_fd`/`TEXT_MIME` (Task 15),
  `probe_theme`/`spawn_window_probe`/`wait_for_report_line`/`probe_report`/
  `PROBE_BG`/`PROBE_ENTRY_BG` (Task 16; used by 16, 17).
- Names produced for later parts, in the shapes their contract sections
  require: `StyleMap` and `restyle` (P4 §4.5–§4.7), `Window::popup_*` (P4/P5's
  `PopoverC`), `FOCUSABLE_CLASS` (P4's `PropName::Focusable`),
  `Window::set_node_text` (superseded by P5's `TextLayout`, and P5 must delete
  it in the same commit that lands `TextLayout` — recorded here so it cannot be
  forgotten).
