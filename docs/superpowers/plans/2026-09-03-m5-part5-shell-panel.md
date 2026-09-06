# Pure-Rust GTK-themed UI — M5 Part 5: shell panel: taskbar + clipboard popover as one reactive App on a layer surface — Implementation Plan

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
`/home/joseph/Projects/icedtea/.claude/worktrees/m5/docs/superpowers/plans/2026-09-03-m5-part0-contract.md`.
Every name and signature in this plan is taken from it verbatim except the
eight items below. Each is recorded here rather than applied silently, and each
must be appended to the contract's §6 as an amendment (Task 17).

- **P5-D1 — input is routed to popup surfaces; P5 edits `ui/src/view/app.rs`.**
  Contract §5 gives every `ui/` file to P0 and tells P5 "Must not touch: `ui/`".
  But `App::run`/`run_offscreen`'s `route` (`ui/src/view/app.rs:1231`) hit-tests
  `rt.root`/`rt.instances`/`rt.layout` — the **window's** tree — and ignores
  `InputEvent::PointerEnter`'s `target: SurfaceTarget` entirely. A click inside
  a real `xdg_popup` therefore never reaches that popup's own handlers: it is
  hit-tested against the window in popup-local coordinates. Contract §3.4 makes
  the clipboard popover a real popup surface (`Cmd::OpenPopup` with a `Some`
  payload) — it must be one, because the panel's layer surface is 28 px tall
  and an embedded popover would be clipped by it — and §3.6's gate
  `the_clipboard_popover_opens_pastes_and_dismisses` requires a click inside it
  to reach `MockClip::activate`. So P5 adds surface-scoped routing. This is an
  **enumerated exception** in the same shape as P4's `drop_down.rs` exception
  (contract §7), not a precedent: it is confined to `ui/src/view/app.rs` and
  `ui/src/window/popup.rs`, it is additive, and it lands with its own test file
  `ui/tests/popup_input.rs`.
- **P5-D2 — a popup surface keeps its view closure and is re-reconciled every
  frame.** Contract §3.4 says `open_popover` "is the single source of truth"
  and spec D9 says the panel is "genuinely reactive". As shipped, `open_popup`
  (`ui/src/view/app.rs:1037`) builds the payload view **once** and drops the
  closure, so an open popover never tracks the model: a `history_changed` that
  lands while it is open, or a pin toggle inside it, cannot change what it
  shows. P5 stores the `Rc<dyn Fn() -> View<Msg>>` on `PopupSurface` and
  reconciles it each frame, exactly as the window's own tree is reconciled.
- **P5-D3 — the popover's history rows are `button`s in a `box_`, not a
  `ListBox` with `on_item_activated`.** Contract §3.4 spells the body as
  `list_box(rows).on_item_activated(..)`. Two shipped `ListBoxC` defects make
  that unusable here and neither is M5 work to fix: (a) `ListBoxC::on_event`
  sets `cx.handled = true` on `PointerDown` with no phase guard
  (`ui/src/widgets/list_box.rs:161-171`), and P4-D19 makes a handled **capture**
  end the whole dispatch — so a `pin`/`remove` button inside a row can never
  receive its own click, the ListBox activates instead; (b) `row_at` is fed
  `Event::PointerDown::local`, which P5-D33's `aim` computes relative to the
  **innermost instance** (the row's label), not to the ListBox, so the row index
  is wrong for every row but the first. Both are pre-existing M3 defects with no
  gate covering them; P5 records them and routes around them. Every popover
  interaction is therefore a plain `button` click — the best-proven path in the
  crate (`ui/tests/interaction_gate.rs`). `pub fn popover_body(m: &PanelModel)
  -> View<Msg>` keeps its contract signature.
- **P5-D4 — `App::on_popup` and `PopupEvent`; `PopupKey::raw`/`from_raw`.**
  Contract §3.1 declares `Msg::PopoverOpened(PopupKey)` and
  `Msg::PopoverDismissed(PopupKey)` and §3.4 says "an outside click produces
  `InputEvent::PopupDone` → `Msg::PopoverDismissed`", but the toolkit ships no
  mechanism for either: `Cmd::OpenPopup` returns nothing to the model, and
  `InputEvent::PopupDone` is routed to the focused **controller**
  (`ui/src/view/app.rs:1384`), never to `update`. P5 adds the hook the contract
  already assumes. `PopupKey::from_raw`/`raw` come with it so an offscreen test
  can name the key the loop minted (`PopupKey(pub(crate) u64)` is otherwise
  unnameable outside the crate).
- **P5-D5 — `App::on_frame`.** Contract §1's M5-D9 says both apps write
  `$ICEDTEA_PROBE_REPORT` lines "once per frame in which the tree changed —
  the mechanism `ui/src/bin/window-probe.rs` already uses". `window-probe`
  drives a raw `Window` loop by hand; an app that hands its window to
  `App::run` has no per-frame hook and no `&Window`. P5 adds
  `App::on_frame(impl FnMut(&Window))`. If P1 has already landed an
  identically-named-and-typed hook for settings, P5 **consumes** it and skips
  Task 4's implementation step (Task 4 opens with the check).
- **P5-D6 — `auto_exclusive_zone_enable()` becomes a literal
  `exclusive_zone: BAR_HEIGHT` (28).** Pre-declared by contract §3.2 ruling 2
  and §4.3 item 6; recorded here as landed.
- **P5-D7 — `shell/Cargo.toml` also gains `wayland-protocols-wlr`.** Contract
  §3.5 lists only `icedtea-ui` and `crossbeam-channel` as additions, but
  `LayerSpec`'s `layer`, `anchor` and `keyboard` fields are typed with
  `zwlr_layer_shell_v1::Layer`, `zwlr_layer_surface_v1::Anchor` and
  `zwlr_layer_surface_v1::KeyboardInteractivity`, and `icedtea-ui` re-exports
  none of them (`ui/src/wayland.rs` re-exports only `BTN_LEFT`, `MARGIN`,
  `CONFIGURE_TIMEOUT`, `LayerWindow*`, `AppState`). The dep is the same
  `wayland-protocols-wlr = { version = "0.3", features = ["client"] }` pin
  `ui/Cargo.toml:31` and `settings/Cargo.toml` (contract §2.1) already carry;
  it is not a GTK dependency and does not affect §4.1's audit.
- **P5-D8 — `Window::popup_position` and `Window::popup_probe_points`.** M5-D9
  gives a live window `probe_points`/`allocation`, both reading `self.root` and
  `self.layout` — the **window's** tree. A popup is a second surface with its
  own tree and its own compositor-assigned position, so §3.6's popover gate
  ("open the popover, activate row 0, assert `("activate", 10)`") has no way to
  say where row 0 is; hard-coding a coordinate is exactly what the M3 gate
  rules forbid, and deriving one from the anchor is wrong the moment the
  compositor slides a constrained popup. Two accessors on `Window`, in the same
  enumerated exception as P5-D1: `popup_position(key) -> Option<(i32, i32)>`
  (the last `xdg_popup.configure` position, which `Popup::position` already
  stores) and `popup_probe_points(key) -> Vec<ProbePoint>` (M5-D9's walk, run
  against that popup's root and layout). `Window::probe_points`'s body is
  extracted into a private `probe_points_of(root, layout)` both call, so there
  is one labelling rule, not two.

- **P5-D9 — the popover anchors on `PopupAnchorPoint::Rect`, and `PanelModel`
  gains `clip_rect`.** Contract §3.4 spells the open as
  `Cmd::OpenPopup { anchor: PopupAnchorPoint::Node(/* the `clip` button's node */), .. }`.
  `update` has neither a `&Window` nor a `Node`, so `Node` is unreachable from
  where the command is built. As shipped: `PanelModel` carries
  `pub clip_rect: Rc<Cell<Option<Rect>>>` — a field beyond §3.1's struct
  listing, and legitimately `Rc` because `PanelModel` never crosses a thread
  (only `Msg` does, and `Msg` stays `Send`) — published each frame by
  `App::on_frame` (P5-D5) from `Window::allocation("clip")`, and the popover
  opens with `PopupAnchorPoint::Rect(rect)` + `Positioner::menu(rect,
  POPOVER_SIZE)`. Everything else in §3.4 is unchanged. (Recorded on the
  consistency check's ruling E7; it was undeclared in this list at freeze.)

Two smaller notes that are **not** deviations, recorded so a reviewer does not
read them as drift:

- `CompositorUpdate` and `ClipUpdate` gain `#[derive(Clone)]`. Contract §3.1's
  own `update` body is `m.taskbar.apply(Arc::unwrap_or_clone(u))`, and
  `Arc::unwrap_or_clone` requires `T: Clone`. Every payload
  (`Snapshot`, `WindowInfo`, `WindowUpdate`, `WorkspaceInfo`, `ClipEntry`)
  already derives `Clone` in `icedtea-contract`. No signature changes; the six
  model tests move unmodified.
- `shell/src/style.rs` adds `pub fn sheet_for(theme: Theme) -> CompiledSheet`
  beside the contract's `pub fn sheet() -> CompiledSheet`. Contract §0 permits
  a part to add items freely; the rest-state gate needs light/dark/hc and
  `sheet()` reads one theme from the environment.

---

## Goal

Rebuild `icedtea-shell` on `icedtea-ui`: one `App<PanelModel, Msg>` on one
`Role::Layer` surface carrying the taskbar (workspaces + window buttons) and
the clipboard popover, with `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`,
`pango` and `cairo` gone from the crate at the first commit that touches its
`Cargo.toml`. `taskbar.rs`'s and `clipboard.rs`'s pure models, both D-Bus
clients and the `CompositorCommands`/`ClipCommands` seam are kept verbatim;
`bridge.rs` is deleted; rendering becomes genuinely reactive (keyed by
workspace id, window id and history entry id) instead of clear-and-rebuild.
`shell/tests/shell_gtk.rs`'s seven assertions are re-expressed against the same
`MockWm`/`MockClip` under `icedtea_harness::Compositor`, plus the two the GTK
test could not reach (middle-click close, a signal through the inbox) and three
new gates (rest-state screencopy light/dark/hc, the popover open→paste→dismiss
gate, and the `process::exit(1)` invariant).

## Architecture

```
                    shell process (one thread runs the loop)
┌───────────────────────────────────────────────────────────────────────┐
│ App<PanelModel, Msg>            Window (Role::Layer, 800x28, top)     │
│   update: FnMut(&mut PanelModel, Msg) -> Cmd<Msg>                     │
│   view:   Fn(&PanelModel) -> View<Msg>                                │
│     box_(H)#bar ── box_(H)#workspaces ── button#ws_<id> …             │
│                 ├─ box_(H)#windows(hexpand) ── button#window_<id> …   │
│                 ╰─ button#clip                                        │
│   .with_inbox(inbox)   .on_popup(..)   .on_frame(probe report writer) │
│                                                                       │
│   Cmd::OpenPopup{anchor: Rect(clip_rect), view: popover_rows}         │
│        └── popup surface: box_#popover ── box_#history ── row#history_<id>
│                                        ╰── button#clip_clear          │
└───▲───────────────────────────────▲───────────────────────────────────┘
    │ InboxSender<Msg>              │ InboxSender<Msg>
    │ (forward thread)              │ (forward thread)
┌───┴──────────────────────┐  ┌─────┴────────────────────────┐
│ compositor_client::spawn │  │ clip_client::spawn           │
│  async_channel<Compositor│  │  async_channel<ClipUpdate>   │
│  Update>, GetState seed, │  │  GetHistory seed,            │
│  MatchRule signal stream,│  │  history_changed signal      │
│  process::exit(1) on err │  │                              │
└──────────────────────────┘  └──────────────────────────────┘
        outbound: Rc<dyn CompositorCommands> / Rc<dyn ClipCommands>
        called from `update` through Cmd::Task, on the loop thread
```

Two worker threads already exist and are **unchanged**; what changes is where
their `async_channel::Receiver` drains to. `bridge.rs`'s
`glib::spawn_future_local` becomes one `forward` thread per client that blocks
on `recv_blocking()` and pushes onto the `InboxSender`, which writes a byte to
the wake pipe `App::run` watches (M5-D2). Outbound commands never run inside
`update`: they go out through `Cmd::Task` on the loop thread, behind the two
traits, so a test swaps in recording mocks.

## Tech Stack

| Piece | Choice | Pin |
|---|---|---|
| Toolkit | `icedtea-ui` (path dep) | workspace |
| Wayland protocol enums | `wayland-protocols-wlr` `features = ["client"]` | `0.3` (P5-D7) |
| D-Bus | `zbus` | `5` (workspace) |
| Worker→loop channel | `async-channel` (kept) + `crossbeam-channel` (inbox) | `2` / workspace `0.5` |
| Contract types | `icedtea-contract` (path dep) | workspace |
| Logging | `tracing`, `tracing-subscriber` | workspace |
| Test harness | `icedtea-harness` (`Compositor`, `VirtualPointerClient`, `ScreencopyClient`) | path dev-dep |
| Live D-Bus test | `icedtea-clipboard` (kept dev-dep) | path dev-dep |
| Edition / MSRV | 2024 / 1.94 | workspace |

## Spec

- `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
  — §2 D8 (command seam), §2 D9 (genuinely reactive), §6 (shell panel), §7
  (testing and parity gates), §11 (out of scope).
- `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` — §1 (M5-D1…D11, the
  toolkit P5 consumes), §3 (P5's whole surface), §4.1/§4.2 (the audits P6 runs
  against what P5 leaves), §5 (P5's ownership and gate), §5's cross-cutting
  rules.

## Global Constraints

1. **Pins.** `wayland-client 0.31`, `wayland-protocols(-wlr) 0.3`, `zbus 5`
   (workspace), `rustix 1`, `skia-rs-safe 0.4.0`, `taffy 0.14`, `xkbcommon 0.9`,
   `wlr 0.20.29`. No new third-party crate enters the workspace.
2. **No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gobject`, `gdk`, `pango` or
   `cairo`** in `shell/` — not in `Cargo.toml`, not in a `use`, not in a test.
   `async-channel` stays: it is not GTK and the two D-Bus clients are unchanged.
3. **Edition 2024, `rust-version = 1.94`**, both inherited from the workspace.
4. **Gates, per crate, after every task:**
   `cargo test -p icedtea-shell`, `cargo clippy -p icedtea-shell --all-targets
   -- -D warnings`, `cargo fmt --all --check`. Tasks 1–4 additionally run
   `cargo test -p icedtea-ui`, `cargo clippy -p icedtea-ui --all-targets --
   -D warnings` and `cargo clippy -p icedtea-ui --all-targets
   --no-default-features -- -D warnings`.
5. **Every M1/M2/M3 gate stays green, unchanged:** `themed_button_offscreen`,
   `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`,
   `transition_screencopy`, `window_events`, `counter_app`, `node_trees`,
   `widget_pixels`, `reconcile_props`, `gallery_gate` (light/dark/hc),
   `interaction_gate` (16/16). Tasks 1–4 name them explicitly.
6. **Parts execute in order 0 → 6.** P5's base is P4's head. Everything P0
   shipped (M5-D1 … M5-D11) is available and consumed by name:
   `Window::watch_fd`/`unwatch`/`InputEvent::FdReady`, `Inbox`/`InboxSender`/
   `App::with_inbox`/`App::on_fd`, `Cmd::Task`, `App::new`'s boxed closures,
   `EventKind::PointerDown/Motion/Up` + `Handler::PairButton` +
   `on_pointer_up_with_button` + `BTN_LEFT`/`BTN_MIDDLE`/`BTN_RIGHT`,
   `Window::probe_points`/`allocation`.
7. **Shell drops `gtk4` + `gtk4-layer-shell` in place** (spec D3): there is no
   second binary, no feature flag and no coexistence window. The first commit
   that edits `shell/Cargo.toml` also rewrites `main.rs`.
8. **`CompositorCommands`/`ClipCommands` and `MockWm`/`MockClip` are kept
   verbatim.** The trait method sets, their PascalCase wire members, the
   `version_mismatch` function and the `process::exit(1)` rule do not change.
   The mocks keep their `(String, u32)` / `(String, u64)` recording shape and
   the same `("focus", 1)` / `("activate", 10)` assertions.
9. **`bridge.rs` is deleted** (Task 5), and with it the crate's last `glib`
   reference.
10. **Commit trailer**, on every commit in this plan:

    ```
    Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
    ```

11. **No merge.** The branch is presented as ready; the owner decides.

## File Structure

| Path | Task | Change |
|---|---|---|
| `ui/src/window/popup.rs` | 1 | `PopupKey::raw`, `PopupKey::from_raw` (P5-D4) |
| `ui/src/window/mod.rs` | 15 | `Window::popup_position`, `Window::popup_probe_points` (P5-D8) |
| `ui/src/view/app.rs` | 1, 2, 3, 4 | `PopupEvent`, `App::on_popup`, surface-scoped `route`, popup re-reconcile, `App::on_frame` (P5-D1, D2, D4, D5) |
| `ui/tests/popup_input.rs` | 1, 2, 3 | **new** — the P5-D1/D2/D4 tests, offscreen |
| `ui/tests/app_frame_hook.rs` | 4 | **new** — the P5-D5 test, harness-driven |
| `shell/Cargo.toml` | 5 | gtk4 + gtk4-layer-shell out; icedtea-ui, crossbeam-channel, wayland-protocols-wlr in |
| `shell/src/lib.rs` | 5 | modules rewritten; `pub use gtk4` deleted |
| `shell/src/bridge.rs` | 5 | **deleted** |
| `shell/src/taskbar.rs` | 5 | `render` deleted, GTK imports deleted, `#[derive(Clone)]` added; model + 5 tests unchanged |
| `shell/src/clipboard.rs` | 5 | `render` + `connect_activation` deleted, GTK imports deleted, `#[derive(Clone)]` added; model + 1 test unchanged |
| `shell/src/style.rs` | 5 | **new** — `style.css` → `CompiledSheet` over the Adwaita stack |
| `shell/src/panel.rs` | 5, 6, 7, 8, 9 | **new** — `PanelModel`, `Msg`, `update`, `view`, `popover_body` |
| `shell/src/main.rs` | 5, 10, 11 | rewritten — layer surface, inbox, forward threads, probe report |
| `shell/src/compositor_client.rs` | — | untouched (worker, proxy, `version_mismatch`, `process::exit(1)`) |
| `shell/src/clip_client.rs` | — | untouched |
| `shell/style.css` | — | untouched (30 lines, five selectors) |
| `shell/tests/support/mod.rs` | 12 | **new** — the in-process panel harness |
| `shell/tests/panel.rs` | 13, 14, 15, 16 | **new** — parity, middle-click, inbox, popover gate, rest-state gates |
| `shell/tests/shell_gtk.rs` | 17 | **deleted** |
| `shell/tests/live_dbus.rs` | — | untouched |
| `shell/systemd/icedtea-shell.service` | — | untouched |
| `2026-09-03-m5-part0-contract.md` | 17 | §6 gains P5-D1 … P5-D7 |

---

## Task 1 — `PopupEvent`, `App::on_popup`, `PopupKey::raw`/`from_raw`

The model has no way to learn the key of the popup it opened, and no way to
learn that the compositor dismissed it. Both `Msg` variants contract §3.1
declares (`PopoverOpened`, `PopoverDismissed`) depend on this hook.

**Files:** `ui/src/window/popup.rs`, `ui/src/view/app.rs`,
`ui/tests/popup_input.rs` (new).

### Interfaces

**Produces:**

```rust
// ui/src/window/popup.rs
impl PopupKey {
    /// This key's opaque number. Stable for the life of the window; never
    /// reused after a popup closes.
    #[must_use]
    pub const fn raw(self) -> u64;

    /// Rebuild a key from [`PopupKey::raw`].
    ///
    /// For tests and for an app that persists a key across a fold; the loop
    /// mints offscreen keys from zero upwards, so `from_raw(0)` names the
    /// first popup a `run_offscreen` script opens.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self;
}
```

```rust
// ui/src/view/app.rs
/// What happened to one of this app's popup surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PopupEvent {
    /// `Cmd::OpenPopup` succeeded and this is the key it produced.
    Opened(PopupKey),
    /// The compositor dismissed it (`xdg_popup.popup_done`) — an outside
    /// click, a grab break, or the parent going away. The surface is already
    /// gone by the time the message reaches `update`.
    Dismissed(PopupKey),
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Turn popup lifecycle into messages. `None` drops the event.
    ///
    /// At most one hook per app; a second call replaces the first. It runs on
    /// the loop thread, and the messages it returns are queued exactly like an
    /// inbox message — never folded re-entrantly.
    #[must_use]
    pub fn on_popup(self, f: impl Fn(PopupEvent) -> Option<Msg> + 'static) -> Self;
}
```

**Consumes:** `Cmd::OpenPopup`, `Cmd::ClosePopup`, `InputEvent::PopupDone`
(M3); `App::new`'s boxed closures (M5-D4, P0).

### Step 1 — the failing test

Create `ui/tests/popup_input.rs`:

```rust
//! P5's toolkit enablement: popup keys reach the model, input reaches popup
//! surfaces, and an open popup tracks the model (contract §6 P5-D1/D2/D4).
//!
//! Offscreen throughout: `run_offscreen` builds `Cmd::OpenPopup`'s payload
//! into a real retained tree and composites it, so every property here is
//! observable with no compositor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::layout::Rect;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::{box_, button};
use icedtea_ui::view::{App, Cmd, PopupEvent, ScriptStep, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::popup::{PopupAnchorPoint, PopupKey, Positioner};
use icedtea_ui::window::{BTN_LEFT, InputEvent, SurfaceTarget};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Open,
    Opened(PopupKey),
    Dismissed(PopupKey),
    Picked(u32),
}

/// The model every test in this file drives.
#[derive(Default)]
struct Model {
    open: Option<PopupKey>,
    picked: Vec<u32>,
    rows: Vec<u32>,
}

fn sheet() -> CompiledSheet {
    CompiledSheet::compile(
        "window { background-color: #ffffff; } \
         box { background-color: #ffffff; } \
         button { min-width: 40px; min-height: 20px; background-color: #808080; }",
    )
}

#[test]
fn an_app_learns_the_key_of_the_popup_it_opened_and_of_its_dismissal() {
    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let clock = Rc::new(ManualClock::new());

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| box_(Orientation::Vertical, [button::<Msg>("row")])),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                sink.borrow_mut().push(format!("opened {}", key.raw()));
                Cmd::None
            }
            Msg::Dismissed(key) => {
                if m.open == Some(key) {
                    m.open = None;
                }
                sink.borrow_mut().push(format!("dismissed {}", key.raw()));
                Cmd::None
            }
            Msg::Picked(_) => Cmd::None,
        },
        |_m: &Model| box_(Orientation::Horizontal, [button::<Msg>("clip")]),
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
    });

    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Event(InputEvent::PopupDone(PopupKey::from_raw(0))),
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        log.borrow().as_slice(),
        ["opened 0".to_string(), "dismissed 0".to_string()],
        "on_popup must report both halves of a popup's life, in order"
    );
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-ui --test popup_input
```

Expected: a **compile** failure, not an assertion failure —

```
error[E0432]: unresolved import `icedtea_ui::view::PopupEvent`
error[E0599]: no function or associated item named `from_raw` found for struct `PopupKey`
error[E0599]: no method named `on_popup` found for struct `App`
```

### Step 3 — implement

In `ui/src/window/popup.rs`, directly after the `PopupKey` declaration:

```rust
impl PopupKey {
    /// This key's opaque number. Stable for the life of the window; never
    /// reused after a popup closes.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Rebuild a key from [`PopupKey::raw`].
    ///
    /// For tests and for an app that persists a key across a fold: the
    /// offscreen loop mints keys from zero upwards, so `from_raw(0)` names the
    /// first popup a `run_offscreen` script opens. A windowed run's keys come
    /// from `Window::open_popup`, and a key that names no live popup is inert
    /// everywhere it is accepted.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        PopupKey(raw)
    }
}
```

In `ui/src/view/app.rs`, after the `App` struct declaration:

```rust
/// What happened to one of this app's popup surfaces.
///
/// Contract §6 P5-D4: `Cmd::OpenPopup` produces a key the loop keeps to
/// itself, and `InputEvent::PopupDone` is routed to the focused *controller*,
/// so an `update` that owns the open/closed state — which contract §3.4 makes
/// the panel's single source of truth — had no way to see either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PopupEvent {
    /// `Cmd::OpenPopup` succeeded and this is the key it produced.
    Opened(PopupKey),
    /// The compositor dismissed it (`xdg_popup.popup_done`) — an outside
    /// click, a grab break, or the parent going away. The surface is already
    /// gone by the time the message reaches `update`.
    Dismissed(PopupKey),
}
```

Add the field to `App` (the post-P0 struct, beside `fd_handlers`):

```rust
    popup_hook: Option<Box<dyn Fn(PopupEvent) -> Option<Msg>>>,
```

initialise it to `None` in `App::new`, and add the builder beside
`App::on_fd`:

```rust
    /// Turn popup lifecycle into messages. `None` drops the event.
    ///
    /// At most one hook per app; a second call replaces the first. It runs on
    /// the loop thread, and the messages it returns are queued exactly like an
    /// inbox message — never folded re-entrantly.
    #[must_use]
    pub fn on_popup(mut self, f: impl Fn(PopupEvent) -> Option<Msg> + 'static) -> Self {
        self.popup_hook = Some(Box::new(f));
        self
    }
```

Add one free function beside `close_popup`:

```rust
/// Ask the app's popup hook for a message, if it has one.
fn popup_msg<M, Msg>(app: &App<M, Msg>, ev: PopupEvent) -> Option<Msg> {
    app.popup_hook.as_ref().and_then(|f| f(ev))
}
```

Fire it in **three** places.

1. `run_offscreen`'s `Cmd::OpenPopup` arm, immediately after the existing
   `open_popup(...)` call and before the arm ends:

```rust
                        if let Some(msg) = popup_msg(&self, PopupEvent::Opened(key)) {
                            rt.queue.push_back(msg);
                        }
```

2. `run`'s `Cmd::OpenPopup` arm, inside `Ok(key) => { ... }` after
   `window.popup_mark_dirty(key);`:

```rust
                            if let Some(msg) = popup_msg(&self, PopupEvent::Opened(key)) {
                                rt.queue.push_back(msg);
                            }
```

3. Dismissal. In `run`'s `for event in &events` loop, ahead of the `route`
   call, add:

```rust
                if let InputEvent::PopupDone(key) = event {
                    // The compositor has already destroyed the surface; drop
                    // our retained copy (and every popup opened after it, as
                    // xdg-shell requires) before telling the model.
                    close_popup(&mut rt, *key);
                    window.close_popup(*key);
                    if let Some(msg) = popup_msg(&self, PopupEvent::Dismissed(*key)) {
                        rt.queue.push_back(msg);
                    }
                }
```

   and in `run_offscreen`'s `ScriptStep::Event(ev)` arm, ahead of the `route`
   call:

```rust
                    if let InputEvent::PopupDone(key) = &ev {
                        close_popup(&mut rt, *key);
                        if let Some(msg) = popup_msg(&self, PopupEvent::Dismissed(*key)) {
                            rt.queue.push_back(msg);
                        }
                    }
```

`route`'s own `InputEvent::PopupDone` arm is left exactly as it is: an embedded
`PopoverC`/`MenuButtonC`/`DropDownC` still learns of a dismissal through
`Event::PopupDone`, which is what `ui/tests/interaction_gate.rs`'s drop-down
and menu-button tests depend on.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-ui --test popup_input
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test counter_app
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

Expected: `popup_input` reports `1 passed`; `interaction_gate` still reports
`16 passed`; `counter_app` unchanged; both clippy runs clean.

**Mutation check.** Delete the `PopupEvent::Dismissed` block from
`run_offscreen`'s `ScriptStep::Event` arm and rerun
`cargo test -p icedtea-ui --test popup_input`: the assertion must fail with
`["opened 0"]` against `["opened 0", "dismissed 0"]`. Restore it.

### Step 5 — commit

```
git add ui/src/window/popup.rs ui/src/view/app.rs ui/tests/popup_input.rs
git commit -m "$(cat <<'EOF'
feat(ui): report popup open and dismiss to the model

`Cmd::OpenPopup` minted a key the loop kept to itself and
`InputEvent::PopupDone` reached only the focused controller, so an app
whose model owns a popover's open state — M5 contract §3.4's rule for the
shell panel — could learn neither. `App::on_popup` turns both halves into
messages; `PopupKey::raw`/`from_raw` make a key nameable outside the crate
so an offscreen script can address the popup the loop just opened.

Recorded as M5 contract amendment P5-D4.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — route input to the surface it arrived on

A click inside a popup surface is hit-tested against the **window's** tree, in
popup-local coordinates. The clipboard popover cannot work until it is not.

**Files:** `ui/src/view/app.rs`, `ui/tests/popup_input.rs`.

### Interfaces

**Produces:** no new public API. `route` becomes surface-scoped; `Runtime`
gains two private fields.

**Consumes:** `SurfaceTarget` (`ui/src/window/mod.rs:299`),
`InputEvent::{PointerEnter, PointerLeave, KeyboardEnter, KeyboardLeave}`,
`PopupKey::from_raw` (Task 1).

### Step 1 — the failing test

Append to `ui/tests/popup_input.rs`:

```rust
#[test]
fn a_click_inside_a_popup_reaches_the_popups_own_handler() {
    let clock = Rc::new(ManualClock::new());
    let picked: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| {
                    box_(
                        Orientation::Vertical,
                        [button::<Msg>("row").on_click(Msg::Picked(7))],
                    )
                }),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                Cmd::None
            }
            Msg::Dismissed(_) => {
                m.open = None;
                Cmd::None
            }
            Msg::Picked(id) => {
                sink.borrow_mut().push(id);
                Cmd::None
            }
        },
        |_m: &Model| box_(Orientation::Horizontal, [button::<Msg>("clip")]),
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
    });

    let popup = SurfaceTarget::Popup(PopupKey::from_raw(0));
    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            // Enter the *popup* surface: its coordinates are its own, so the
            // row's centre is a few pixels in, not wherever the window's tree
            // happens to have a button.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 20.0,
                y: 10.0,
                serial: 1,
                target: popup,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        picked.borrow().as_slice(),
        [7],
        "a click on a popup surface must be hit-tested against that popup's tree"
    );
}

#[test]
fn a_click_on_the_window_still_reaches_the_window_after_a_popup_opened() {
    let clock = Rc::new(ManualClock::new());
    let picked: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| {
                    box_(
                        Orientation::Vertical,
                        [button::<Msg>("row").on_click(Msg::Picked(7))],
                    )
                }),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                Cmd::None
            }
            Msg::Dismissed(_) => {
                m.open = None;
                Cmd::None
            }
            Msg::Picked(id) => {
                sink.borrow_mut().push(id);
                Cmd::None
            }
        },
        |_m: &Model| {
            box_(
                Orientation::Horizontal,
                [button::<Msg>("clip").on_click(Msg::Picked(1))],
            )
        },
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
    });

    let popup = SurfaceTarget::Popup(PopupKey::from_raw(0));
    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 20.0,
                y: 10.0,
                serial: 1,
                target: popup,
            }),
            ScriptStep::Event(InputEvent::PointerLeave),
            // Back on the window: the enter carries `SurfaceTarget::Window`,
            // which is what puts routing back on the window's tree.
            ScriptStep::Event(InputEvent::pointer_enter(20.0, 10.0, 4)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 5,
                time_ms: 2,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 6,
                time_ms: 3,
            }),
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        picked.borrow().as_slice(),
        [1],
        "leaving a popup must put routing back on the window's own tree"
    );
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-ui --test popup_input
```

Expected: `an_app_learns_the_key...` still passes;
`a_click_inside_a_popup_reaches_the_popups_own_handler` fails with

```
assertion `left == right` failed: a click on a popup surface must be hit-tested against that popup's tree
  left: []
 right: [7]
```

and `a_click_on_the_window_still_reaches_the_window_after_a_popup_opened`
passes for the wrong reason (routing never left the window) — it is the
regression guard for the fix, not a driver of it.

### Step 3 — implement

Add two fields to `Runtime`, after `last_pointer`:

```rust
    /// Which surface the pointer is on, from the last `PointerEnter`.
    ///
    /// Contract §6 P5-D1: `route` hit-tests one retained tree, and a popup is
    /// a second surface with its own. Wayland's own rule is the rule here —
    /// an enter names the surface and everything up to the matching leave
    /// belongs to it.
    pointer_target: SurfaceTarget,
    /// Which surface has the keyboard, from the last `KeyboardEnter`.
    keyboard_target: SurfaceTarget,
```

Initialise both to `SurfaceTarget::Window` at all three `Runtime { .. }`
literals (`run_offscreen`, `run`, and the `#[cfg(test)]` fixture if it builds
one), and add `use crate::window::SurfaceTarget;` to the file's imports if it
is not already there.

Rename the existing `fn route` to `fn route_surface` — **body unchanged**, not
one line edited — and add the new `route` in front of it:

```rust
/// Swap popup `index`'s retained tree into the runtime's window slots.
///
/// Called twice around a routing pass, so the popup's tree is what `route_surface`
/// hit-tests, dispatches into and mutates. Destructured rather than indexed so
/// the four swaps borrow disjoint fields of `rt`.
fn swap_popup_tree<Msg>(rt: &mut Runtime<Msg>, index: usize) {
    let Runtime {
        root,
        instances,
        styles,
        layout,
        popups,
        ..
    } = rt;
    let popup = &mut popups[index];
    std::mem::swap(root, &mut popup.root);
    std::mem::swap(instances, &mut popup.instances);
    std::mem::swap(styles, &mut popup.styles);
    std::mem::swap(layout, &mut popup.layout);
}

/// Route one input event to the surface it arrived on.
///
/// Contract §6 P5-D1. `InputEvent`'s two `Enter` variants carry a
/// [`SurfaceTarget`]; everything after an enter belongs to that surface until
/// the matching leave, which is how Wayland itself defines focus and is what
/// `SurfaceTarget`'s own doc comment says. Coordinates on a popup event are
/// already popup-surface-local, so no offset arithmetic is needed: the popup's
/// tree starts at its own (0, 0), exactly as `paint_tree` paints it.
fn route<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    event: &InputEvent,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
    let target = match event {
        InputEvent::PointerEnter { target, .. } => {
            rt.pointer_target = *target;
            *target
        }
        InputEvent::PointerLeave => {
            let was = rt.pointer_target;
            rt.pointer_target = SurfaceTarget::Window;
            was
        }
        InputEvent::KeyboardEnter { target, .. } => {
            rt.keyboard_target = *target;
            *target
        }
        InputEvent::KeyboardLeave => {
            let was = rt.keyboard_target;
            rt.keyboard_target = SurfaceTarget::Window;
            was
        }
        InputEvent::Key(_) => rt.keyboard_target,
        _ => rt.pointer_target,
    };
    let index = match target {
        SurfaceTarget::Window => None,
        // A key for a popup this app no longer retains routes nowhere rather
        // than falling back to the window: the event belonged to a surface
        // that is gone, and replaying it on the window would fire the wrong
        // handler at the wrong coordinates.
        SurfaceTarget::Popup(key) => match rt.popups.iter().position(|p| p.key == key) {
            Some(index) => Some(index),
            None => return Vec::new(),
        },
    };
    let Some(index) = index else {
        return route_surface(rt, event, sheet, fonts, icons, clipboard, clock);
    };
    swap_popup_tree(rt, index);
    let out = route_surface(rt, event, sheet, fonts, icons, clipboard, clock);
    // Unconditionally, with nothing fallible between the two swaps: leaving
    // the popup's tree in the window's slots would corrupt every later frame.
    swap_popup_tree(rt, index);
    out
}
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-ui --test popup_input
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test window_events
cargo test -p icedtea-ui --test gallery_gate
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

Expected: `popup_input` `3 passed`; `interaction_gate` `16 passed`;
`window_events` and `gallery_gate` unchanged.

**Mutation check.** Delete the second `swap_popup_tree(rt, index);` (the
restore). Rerun `cargo test -p icedtea-ui --test popup_input`:
`a_click_on_the_window_still_reaches_the_window_after_a_popup_opened` must fail
with `left: []` — the window's tree is no longer in the window's slots.
Restore it.

### Step 5 — commit

```
git add ui/src/view/app.rs ui/tests/popup_input.rs
git commit -m "$(cat <<'EOF'
fix(ui): route input to the surface it arrived on

`route` hit-tested the window's retained tree for every event, ignoring
`InputEvent`'s `SurfaceTarget`, so a click inside a real popup surface was
resolved against the window in popup-local coordinates and reached nothing.
`route` now tracks pointer and keyboard focus per surface and swaps the
addressed popup's tree into the runtime for the dispatch.

Recorded as M5 contract amendment P5-D1.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — an open popup tracks the model

`Cmd::OpenPopup`'s payload is built once and the closure dropped, so an open
popover shows the model as it was when it opened. Contract §3.4 makes
`open_popover` the single source of truth and spec D9 requires the panel to be
genuinely reactive; the clipboard history changes underneath an open popover
whenever the daemon signals.

**Files:** `ui/src/view/app.rs`, `ui/tests/popup_input.rs`.

### Interfaces

**Produces:** no new public API. `PopupSurface` gains a `view` field and one
private free function `rebuild_popups` runs beside `rebuild`.

**Consumes:** `reconcile`, `containers_of`, `BuildCx` (M3).

### Step 1 — the failing test

Append to `ui/tests/popup_input.rs`:

```rust
#[test]
fn an_open_popup_is_rebuilt_from_the_model_every_fold() {
    let clock = Rc::new(ManualClock::new());
    let rows: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(vec![1, 2]));
    let payload_rows = rows.clone();
    let picked: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => {
                let source = payload_rows.clone();
                Cmd::OpenPopup {
                    anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                    positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 60)),
                    view: Rc::new(move || {
                        let children: Vec<View<Msg>> = source
                            .borrow()
                            .iter()
                            .map(|id| {
                                let id = *id;
                                button::<Msg>("row").on_click(Msg::Picked(id))
                            })
                            .collect();
                        box_(Orientation::Vertical, children)
                    }),
                }
            }
            Msg::Opened(key) => {
                m.open = Some(key);
                Cmd::None
            }
            Msg::Dismissed(_) => {
                m.open = None;
                Cmd::None
            }
            Msg::Picked(id) => {
                // The fold that changes what the popup shows.
                m.rows.push(id);
                sink.borrow_mut().push(id);
                Cmd::None
            }
        },
        |_m: &Model| box_(Orientation::Horizontal, [button::<Msg>("clip")]),
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
    });

    let popup = SurfaceTarget::Popup(PopupKey::from_raw(0));
    // Row 0's centre with two rows, then with one: the second click lands on
    // the same pixel and must reach whatever row is there *now*.
    let click = |serial: u32| {
        vec![
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 20.0,
                y: 10.0,
                serial,
                target: popup,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: serial + 1,
                time_ms: serial,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: serial + 2,
                time_ms: serial + 1,
            }),
            ScriptStep::Event(InputEvent::PointerLeave),
            ScriptStep::Advance(Duration::from_millis(16)),
        ]
    };

    let mut script = vec![
        ScriptStep::Message(Msg::Open),
        ScriptStep::Advance(Duration::from_millis(16)),
    ];
    script.extend(click(1));
    // Replace the popup's source data behind its back, then click again.
    *rows.borrow_mut() = vec![9];
    script.push(ScriptStep::Message(Msg::Picked(0)));
    script.push(ScriptStep::Advance(Duration::from_millis(16)));
    script.extend(click(11));

    app.run_offscreen((200, 80), clock, script)
        .expect("offscreen run");

    assert_eq!(
        picked.borrow().as_slice(),
        [1, 0, 9],
        "the second click must reach the rebuilt row 9, not the stale row 1"
    );
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-ui --test popup_input -- an_open_popup_is_rebuilt
```

Expected:

```
assertion `left == right` failed: the second click must reach the rebuilt row 9, not the stale row 1
  left: [1, 0, 1]
 right: [1, 0, 9]
```

### Step 3 — implement

Add the field to `PopupSurface`, after `size`:

```rust
    /// The payload `Cmd::OpenPopup` carried.
    ///
    /// Contract §6 P5-D2: kept, not dropped, so `rebuild_popups` can run it
    /// again on every fold — an open popover tracks the model exactly as the
    /// window's own tree does.
    view: Rc<dyn Fn() -> View<Msg>>,
```

In `open_popup`, set it in the `PopupSurface { .. }` literal:

```rust
        view: Rc::clone(view),
```

Add, immediately after `rebuild`:

```rust
/// Re-run every open popup's payload and reconcile it into its own tree.
///
/// The window's counterpart is [`rebuild`]; this is the same three steps —
/// describe, reconcile, refresh containers — for each popup surface, in the
/// order they were opened. Cheap when nothing changed: `reconcile` diffs, and
/// a popup whose view returns the same tree produces no ops.
fn rebuild_popups<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
) {
    for index in 0..rt.popups.len() {
        let described = (Rc::clone(&rt.popups[index].view))();
        let mut cx = BuildCx {
            sheet,
            fonts,
            icons,
            clock,
            env: &rt.env,
        };
        let popup = &mut rt.popups[index];
        reconcile(&popup.root, &mut popup.instances, vec![described], &mut cx);
        popup.containers.clear();
        popup.containers.insert(
            crate::view::render::node_addr(&popup.root),
            Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        containers_of(&popup.instances, &mut popup.containers);
    }
}
```

Call it from `drain`, one line after the existing `rebuild`:

```rust
    if folded {
        rebuild(app, rt, sheet, fonts, icons, clock);
        rebuild_popups(rt, sheet, fonts, icons, clock);
    }
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-ui --test popup_input
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test reconcile_props
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

Expected: `popup_input` `4 passed`; the whole `icedtea-ui` suite green,
`interaction_gate` still `16 passed`.

**Mutation check.** Comment out the `rebuild_popups(...)` line in `drain` and
rerun: `an_open_popup_is_rebuilt_from_the_model_every_fold` must fail with
`left: [1, 0, 1]`. Restore it.

### Step 5 — commit

```
git add ui/src/view/app.rs ui/tests/popup_input.rs
git commit -m "$(cat <<'EOF'
feat(ui): rebuild open popup surfaces on every fold

`Cmd::OpenPopup` built its payload once and dropped the closure, so an open
popup showed the model as it was when it opened -- a clipboard popover could
not follow a history change, and a control inside one could not restyle
itself. `PopupSurface` now keeps the view function and `drain` reconciles
every open popup alongside the window's own tree.

Recorded as M5 contract amendment P5-D2.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — `App::on_frame`, the live-window hook

M5-D9 says both M5 apps write `$ICEDTEA_PROBE_REPORT` lines "once per frame in
which the tree changed", using `Window::probe_points`/`Window::allocation`. An
app that hands its window to `App::run` has no per-frame callback and no
`&Window`.

**First, check what P0 and P1 already landed.** Run:

```
grep -n "pub fn on_frame\|pub fn with_probe_report" ui/src/view/app.rs
```

`with_probe_report` **will** be there: P0-D4 ships
`#[must_use] pub fn with_probe_report(self, path: std::path::PathBuf) -> Self`
plus an automatic `$ICEDTEA_PROBE_REPORT` pickup inside `App::run`, which
already emits M5-D9's `probe`/`alloc` lines for the window's tree once per
frame in which they change. **P5 does not write a second report writer** — see
Task 11, which is reduced to publishing the `clip` button's rect (and, if the
popover gate needs them, P5-D8's popup probe lines) and must not duplicate
P0's geometry lines (consistency-check ruling E1).

`on_frame` is still P5's to add, because `with_probe_report` gives the app no
`&Window` and the panel needs one for `PanelModel::clip_rect`. If the grep
prints an `on_frame` line whose signature is
`pub fn on_frame(mut self, f: impl FnMut(&crate::window::Window) + 'static) -> Self`
(a hook an earlier part landed), **skip steps 1–5 of this task entirely**,
record in the Task 17 amendment that P5-D5 was landed earlier and consumed
unchanged, and go to Task 5. If it prints nothing, continue. If it prints a
*different* signature, stop and report rather than changing it.

**Files:** `ui/src/view/app.rs`, `ui/tests/app_frame_hook.rs` (new).

### Interfaces

**Produces:**

```rust
// ui/src/view/app.rs
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Observe the live window once per rendered frame.
    ///
    /// Runs on the loop thread after the frame is painted, with the window's
    /// layout tree already settled, so [`Window::probe_points`] and
    /// [`Window::allocation`] answer for what was just drawn. It may not
    /// mutate the model and gets no way to: this is the hook M5's apps write
    /// their `$ICEDTEA_PROBE_REPORT` lines from, and the one an app caches a
    /// widget's allocation through. Never called by `run_offscreen`, which has
    /// no window.
    ///
    /// At most one hook per app; a second call replaces the first.
    #[must_use]
    pub fn on_frame(self, f: impl FnMut(&crate::window::Window) + 'static) -> Self;
}
```

**Consumes:** `Window::probe_points`, `Window::allocation`, `ProbePoint`
(M5-D9, P0).

### Step 1 — the failing test

Create `ui/tests/app_frame_hook.rs`:

```rust
//! P5's `App::on_frame` (contract §6 P5-D5): the per-frame window hook M5's
//! apps write their probe report from.
//!
//! Harness-driven because the hook's whole point is a *live* `Window`: the
//! app runs on its own thread against `icedtea_harness::Compositor`, and the
//! test thread watches a counter and a probe label the hook publishes.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_harness::Compositor;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::{box_, button};
use icedtea_ui::view::{App, Cmd, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::{Role, SurfaceSpec, Window};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Never,
}

#[test]
fn on_frame_sees_the_live_windows_probe_points() {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();

    let frames = Arc::new(AtomicUsize::new(0));
    let labels: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let frames_in = frames.clone();
    let labels_in = labels.clone();

    std::thread::spawn(move || {
        // SAFETY: this thread is the only one that touches the environment,
        // and it does so before opening any Wayland connection.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket) };
        let spec = SurfaceSpec {
            role: Role::Toplevel,
            size: (200, 60),
            title: "on-frame".to_string(),
            app_id: "org.icedtea.OnFrame".to_string(),
        };
        let sheet = CompiledSheet::compile(
            "window { background-color: #ffffff; } \
             button { min-width: 40px; min-height: 20px; background-color: #808080; }",
        );
        let Ok(window) = Window::open(spec, sheet, FontDatabase::new()) else {
            return;
        };
        let _ = App::new(
            (),
            |_m: &mut (), _msg: Msg| Cmd::None,
            |_m: &()| -> View<Msg> {
                box_(
                    Orientation::Horizontal,
                    [button::<Msg>("hello").id("greeting")],
                )
            },
        )
        .on_frame(move |w| {
            frames_in.fetch_add(1, Ordering::SeqCst);
            let mut seen = labels_in.lock().expect("labels");
            for point in w.probe_points() {
                if !seen.contains(&point.label) {
                    seen.push(point.label);
                }
            }
        })
        .run(window);
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if frames.load(Ordering::SeqCst) > 0 && !labels.lock().expect("labels").is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        frames.load(Ordering::SeqCst) > 0,
        "on_frame never ran against a live window"
    );
    let seen = labels.lock().expect("labels").clone();
    assert!(
        seen.iter().any(|l| l == "greeting"),
        "the hook must see the live window's probe points; saw {seen:?}"
    );
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-ui --test app_frame_hook
```

Expected: a compile failure —

```
error[E0599]: no method named `on_frame` found for struct `App`
```

### Step 3 — implement

Add the field to `App`, beside `popup_hook`:

```rust
    frame_hook: Option<Box<dyn FnMut(&crate::window::Window)>>,
```

initialise it to `None` in `App::new`, and add the builder beside
`App::on_popup`:

```rust
    /// Observe the live window once per rendered frame.
    ///
    /// Runs on the loop thread after the frame is painted, with the window's
    /// layout tree already settled, so [`Window::probe_points`] and
    /// [`Window::allocation`] answer for what was just drawn. It may not
    /// mutate the model and gets no way to: this is the hook M5's apps write
    /// their `$ICEDTEA_PROBE_REPORT` lines from, and the one an app caches a
    /// widget's allocation through. Never called by `run_offscreen`, which has
    /// no window.
    ///
    /// At most one hook per app; a second call replaces the first.
    #[must_use]
    pub fn on_frame(mut self, f: impl FnMut(&crate::window::Window) + 'static) -> Self {
        self.frame_hook = Some(Box::new(f));
        self
    }
```

In `run`, immediately after the popup-painting `for index in 0..rt.popups.len()`
loop and before the loop body's closing brace:

```rust
            if let Some(hook) = self.frame_hook.as_mut() {
                hook(&window);
            }
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-ui --test app_frame_hook
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

Expected: `app_frame_hook` `1 passed`; the whole `icedtea-ui` suite green
(`gallery_gate` light/dark/hc, `interaction_gate` 16/16, `window_events`,
`counter_app`, `node_trees`, `widget_pixels`, `reconcile_props`,
`layer_shell_screencopy`, `transition_screencopy`, `themed_button_offscreen`,
`adwaita_coverage`, `gtk4_property_reference`).

**Mutation check.** Change the hook call to run only when
`rt.popups.is_empty()` is `false`. Rerun `cargo test -p icedtea-ui --test
app_frame_hook`: it must fail with "on_frame never ran against a live window".
Restore it.

### Step 5 — commit

```
git add ui/src/view/app.rs ui/tests/app_frame_hook.rs
git commit -m "$(cat <<'EOF'
feat(ui): add App::on_frame, the live-window per-frame hook

M5-D9 has both apps publish `$ICEDTEA_PROBE_REPORT` lines from
`Window::probe_points`/`allocation` once a frame, but an app that hands its
window to `App::run` had no per-frame callback and no `&Window`. `on_frame`
runs after the frame is painted with the layout settled; `run_offscreen`,
which has no window, never calls it.

Recorded as M5 contract amendment P5-D5.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — swap the dependencies, strip GTK, open the layer surface

The GTK binary stops existing here. `shell/` gains `icedtea-ui` and loses
`gtk4`/`gtk4-layer-shell`; `bridge.rs` goes; `taskbar::render`,
`clipboard::render` and `clipboard::connect_activation` go; a `PanelModel` on a
`Role::Layer` window takes their place, showing an empty bar. The models, both
D-Bus clients and all six model tests are untouched.

**Files:** `shell/Cargo.toml`, `shell/src/lib.rs`, `shell/src/bridge.rs`
(deleted), `shell/src/taskbar.rs`, `shell/src/clipboard.rs`,
`shell/src/style.rs` (new), `shell/src/panel.rs` (new), `shell/src/main.rs`,
`shell/tests/shell_gtk.rs` (temporarily neutralised — see step 3).

### Interfaces

**Produces:**

```rust
// shell/src/style.rs
/// Compile the bundled panel sheet on top of the Adwaita stack for `theme`.
///
/// The five `style.css` selectors (`#bar`, `#bar button`,
/// `#workspaces button.active`, `#bar button.focused`, `#bar button.attention`)
/// are appended to the theme's own source and compiled as one sheet under the
/// theme's `@media` environment — the same layering `icedtea_ui::gallery`'s
/// `--theme-file` override uses, and the direct replacement for
/// `style_context_add_provider_for_display` at
/// `STYLE_PROVIDER_PRIORITY_APPLICATION`.
pub fn sheet_for(theme: icedtea_ui::gallery::Theme) -> icedtea_ui::css::cascade::CompiledSheet;

/// `sheet_for` the theme `$ICEDTEA_THEME` names (`light`, `dark`, `hc`);
/// `dark` when unset or unrecognised, which is what the panel's own colours
/// assume.
pub fn sheet() -> icedtea_ui::css::cascade::CompiledSheet;

/// The theme `$ICEDTEA_THEME` names, defaulting to `Theme::Dark`.
pub fn theme_from_env() -> icedtea_ui::gallery::Theme;
```

```rust
// shell/src/panel.rs
pub const BAR_HEIGHT: i32 = 28;
pub const POPOVER_SIZE: (u32, u32) = (320, 280);

pub struct PanelModel { /* fields below */ }
impl PanelModel {
    pub fn new(
        wm: std::rc::Rc<dyn crate::compositor_client::CompositorCommands>,
        clip: std::rc::Rc<dyn crate::clip_client::ClipCommands>,
    ) -> PanelModel;
}

#[derive(Clone, Debug)]
pub enum Msg { /* contract §3.1, verbatim */ }

pub fn update(m: &mut PanelModel, msg: Msg) -> icedtea_ui::view::Cmd<Msg>;
pub fn view(m: &PanelModel) -> icedtea_ui::view::View<Msg>;

/// The command surface when there is no session bus: every request is dropped
/// with one warning. The GTK panel's `wire_taskbar` returned early and left
/// the bar inert; one `App` cannot return early from half of itself, so the
/// inertness moves behind the trait and the bar still paints.
pub struct Offline;
```

**Consumes:** `icedtea_ui::window::{Window, SurfaceSpec, Role, LayerSpec}`,
`icedtea_ui::view::{App, Cmd, View}`,
`icedtea_ui::view::builders::{box_, button}`,
`icedtea_ui::widgets::Orientation`, `icedtea_ui::text::FontDatabase`,
`icedtea_ui::gallery::Theme`, `icedtea_ui::css::cascade::CompiledSheet`,
`icedtea_ui::css::parse::parse_stylesheet_with_base`.

### Step 1 — the failing test

Create `shell/src/panel.rs` containing **only** its test module for now, so the
test names a module that does not compile yet is not the failure we are after.
Instead, put the first test where the model already lives. Append to
`shell/src/taskbar.rs`'s existing `mod tests`:

```rust
    /// M5: the update stream crosses a thread boundary as `Arc<CompositorUpdate>`
    /// (contract §3.1 makes `Msg` `Send`), and `update` unwraps it with
    /// `Arc::unwrap_or_clone`. That needs `Clone`, and a payload that silently
    /// stopped deriving it would only show up in `panel.rs`.
    #[test]
    fn a_compositor_update_can_be_cloned_out_of_an_arc() {
        let update = CompositorUpdate::Opened(win(1, "a"));
        let shared = std::sync::Arc::new(update);
        let mut m = TaskbarModel::default();
        m.apply(std::sync::Arc::unwrap_or_clone(shared.clone()));
        m.apply(std::sync::Arc::unwrap_or_clone(shared));
        assert_eq!(m.windows.len(), 1, "the same update applied twice upserts");
    }
```

and to `shell/src/clipboard.rs`'s `mod tests`:

```rust
    #[test]
    fn a_clip_update_can_be_cloned_out_of_an_arc() {
        let shared = std::sync::Arc::new(ClipUpdate::History(vec![entry(1, "a")]));
        let mut m = ClipboardModel::default();
        m.apply(std::sync::Arc::unwrap_or_clone(shared.clone()));
        m.apply(std::sync::Arc::unwrap_or_clone(shared));
        assert_eq!(m.entries.len(), 1);
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --lib
```

Expected: a compile failure —

```
error[E0277]: the trait bound `CompositorUpdate: Clone` is not satisfied
error[E0277]: the trait bound `ClipUpdate: Clone` is not satisfied
```

### Step 3 — implement

**`shell/Cargo.toml`** — replace the whole file:

```toml
[package]
name = "icedtea-shell"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
icedtea-contract = { path = "../contract" }
icedtea-ui = { path = "../ui" }
wayland-protocols-wlr = { version = "0.3", features = ["client"] }
zbus.workspace = true
async-channel = "2"
crossbeam-channel.workspace = true
futures-util = "0.3"
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
icedtea-harness = { path = "../harness" }
icedtea-clipboard = { path = "../clipboard" }
```

`gtk4` and `gtk4-layer-shell` are gone. `crossbeam-channel` moves from
dev-dependencies to dependencies (the inbox side needs it at runtime, and
`tests/live_dbus.rs` keeps compiling because a normal dependency is visible to
tests). The duplicate `icedtea-contract` dev-dependency and the unused
`wayland-client` dev-dependency go: nothing under `shell/tests` names either
after `shell_gtk.rs` is neutralised.

**`shell/src/bridge.rs`** — delete:

```
git rm shell/src/bridge.rs
```

**`shell/src/lib.rs`** — replace the whole file:

```rust
//! `icedtea-shell` internals as a library, so integration tests can build the
//! panel, drive the pure models and swap the command surface for a mock.

pub mod clip_client;
pub mod clipboard;
pub mod compositor_client;
pub mod panel;
pub mod style;
pub mod taskbar;
```

`pub use gtk4;` is gone: it existed only so `tests/shell_gtk.rs` could build
real widgets against a matching crate version.

**`shell/src/taskbar.rs`** — delete the three GTK `use` lines
(`use std::rc::Rc;`, `use gtk4::prelude::*;`, `use gtk4::{Box as GtkBox,
Button, GestureClick, Orientation};`) and the `use crate::compositor_client::CompositorCommands;`
line, delete the whole `pub fn render(...)` function and its doc comment, and
change the two enum/struct derives:

```rust
#[derive(Default, Debug, Clone)]
pub struct TaskbarModel {
```

```rust
/// The subset of `org.icedtea.Compositor` traffic the taskbar folds in. The worker
/// (`compositor_client`) translates D-Bus signals + the seed snapshot into these.
///
/// `Clone` because the update crosses a thread boundary inside an `Arc` on the
/// way to `panel::update` (contract §3.1 makes `Msg` `Send`), which unwraps it
/// with `Arc::unwrap_or_clone`.
#[derive(Debug, Clone)]
pub enum CompositorUpdate {
```

The file's module doc's trailing "the render half (Task 9) diffs this into
widgets" becomes "`panel::view` renders it". `apply`, `merge` and all five
existing tests are untouched.

**`shell/src/clipboard.rs`** — delete `use std::rc::Rc;`,
`use gtk4::prelude::*;`, the `use gtk4::{...}` line and
`use crate::clip_client::ClipCommands;`, delete `pub fn render(...)` and
`pub fn connect_activation(...)` with their doc comments, and:

```rust
#[derive(Default, Debug, Clone)]
pub struct ClipboardModel {
```

```rust
/// One clipboard-history update. `Clone` for the same `Arc::unwrap_or_clone`
/// reason `CompositorUpdate` is.
#[derive(Debug, Clone)]
pub enum ClipUpdate {
```

The module doc's "render diffs it into a `ListBox`" becomes "`panel::popover_body`
renders it". The one existing test is untouched.

**`shell/src/style.rs`** — new:

```rust
//! The panel's own style sheet, layered over the bundled Adwaita stack.
//!
//! GTK registered `style.css` display-wide with a `CssProvider` at
//! `STYLE_PROVIDER_PRIORITY_APPLICATION`. `icedtea-ui` has no display-wide
//! provider registry: a sheet belongs to a `Window`, and an app sheet layers
//! by being compiled *after* the theme in one source, which is exactly the
//! cascade origin the theme override uses.

use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::parse::parse_stylesheet_with_base;
use icedtea_ui::gallery::Theme;

/// `shell/style.css`, verbatim.
const PANEL_CSS: &str = include_str!("../style.css");

/// The theme `$ICEDTEA_THEME` names, defaulting to [`Theme::Dark`].
///
/// Dark is the default because the panel's own colours are dark
/// (`#bar { background-color: rgba(20, 20, 24, 0.92); color: #e6e6e6; }`); an
/// unreadable value is a default, never an error, per the contract's
/// untrusted-input rule.
#[must_use]
pub fn theme_from_env() -> Theme {
    std::env::var("ICEDTEA_THEME")
        .ok()
        .and_then(|name| Theme::parse(&name))
        .unwrap_or(Theme::Dark)
}

/// Compile the bundled panel sheet on top of the Adwaita stack for `theme`.
#[must_use]
pub fn sheet_for(theme: Theme) -> CompiledSheet {
    let css = format!("{}\n{PANEL_CSS}", theme.sheet());
    CompiledSheet::compile_with_env(&parse_stylesheet_with_base(&css, None), &theme.media_env())
}

/// [`sheet_for`] the theme [`theme_from_env`] resolves.
#[must_use]
pub fn sheet() -> CompiledSheet {
    sheet_for(theme_from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding F10's accent stripe is a `background-image: linear-gradient`,
    /// chosen so the focused/attention state reads without reflowing the
    /// button's metrics. If M2's engine dropped it, the two states would be
    /// invisible again and nothing else would notice.
    #[test]
    fn the_panel_sheet_keeps_the_focus_and_attention_gradients() {
        for theme in [Theme::Light, Theme::Dark, Theme::HighContrast] {
            let compiled = sheet_for(theme);
            let text = format!("{compiled:?}");
            assert!(
                text.contains("89b4fa") || text.contains("137, 180, 250"),
                "{theme:?}: the #bar button.focused accent is missing"
            );
            assert!(
                text.contains("f38ba8") || text.contains("243, 139, 168"),
                "{theme:?}: the #bar button.attention accent is missing"
            );
        }
    }
}
```

**`shell/src/panel.rs`** — new (the skeleton; Tasks 6–9 fill `view` and the
remaining `update` arms):

```rust
//! The panel: one `App<PanelModel, Msg>` on one layer surface, carrying the
//! taskbar and the clipboard popover.
//!
//! `view` is pure and keyed — workspace id, window id, history entry id — so
//! the reconciler keeps hover and focus identity across an update. The GTK
//! panel cleared and rebuilt its containers instead; that was a workaround for
//! having no reconciler, not a design (spec D9).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use icedtea_contract::ClipEntry;
use icedtea_ui::layout::Rect;
use icedtea_ui::view::builders::{box_, button};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::popup::PopupKey;

use crate::clip_client::ClipCommands;
use crate::clipboard::{ClipUpdate, ClipboardModel};
use crate::compositor_client::{CompositorCommands, CompositorUpdate};
use crate::taskbar::TaskbarModel;

/// The bar's committed height, and its exclusive zone.
///
/// gtk4-layer-shell's `auto_exclusive_zone_enable()` has no `LayerSpec`
/// equivalent — the field is a concrete `i32` — so the panel names the height
/// `set_size_request(-1, 28)` used to force (contract §6 P5-D6).
pub const BAR_HEIGHT: i32 = 28;

/// The clipboard popover's surface size.
pub const POPOVER_SIZE: (u32, u32) = (320, 280);

pub struct PanelModel {
    /// Verbatim from `taskbar.rs`; `apply`/`merge` and their 5 tests unchanged.
    pub taskbar: TaskbarModel,
    /// Verbatim from `clipboard.rs`; `apply` and its 1 test unchanged.
    pub clipboard: ClipboardModel,
    /// The single source of truth for the clipboard popover.
    pub open_popover: Option<PopupKey>,
    /// Kept behind the traits so the tests can pass mocks (spec D8). `Rc`, not
    /// `Arc`: they never cross a thread — `update` runs on the loop thread and
    /// so does `Cmd::Task`.
    pub wm: Rc<dyn CompositorCommands>,
    pub clip: Rc<dyn ClipCommands>,
    pub bar_height: i32,
    /// The `clip` button's border box, republished once a frame by the
    /// `App::on_frame` hook. `Cmd::OpenPopup`'s anchor rect: `update` has no
    /// `&Window`, and `PopupAnchorPoint::Node` needs a `Node` a handler cannot
    /// hand it.
    pub clip_rect: Rc<Cell<Option<Rect>>>,
    /// A shared mirror of `clipboard.entries`, refreshed by `update` on every
    /// history change. The popover's body is a `Cmd::OpenPopup` payload the
    /// loop re-runs each frame (contract §6 P5-D2) and therefore cannot borrow
    /// the model; this is what it reads instead.
    pub history: Rc<RefCell<Vec<ClipEntry>>>,
}

impl PanelModel {
    #[must_use]
    pub fn new(wm: Rc<dyn CompositorCommands>, clip: Rc<dyn ClipCommands>) -> PanelModel {
        PanelModel {
            taskbar: TaskbarModel::default(),
            clipboard: ClipboardModel::default(),
            open_popover: None,
            wm,
            clip,
            bar_height: BAR_HEIGHT,
            clip_rect: Rc::new(Cell::new(None)),
            history: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// One compositor update, from the inbox. `Arc`, not `Rc`: `Msg` is `Send`.
    Compositor(Arc<CompositorUpdate>),
    /// One clipboard history update, from the inbox.
    Clip(Arc<ClipUpdate>),

    WorkspaceClicked(u32),
    WindowClicked(u32),
    /// Every pointer release on a window button; `update` acts only on
    /// `BTN_MIDDLE`. Left releases arrive here too and are ignored — the focus
    /// path is `WindowClicked`, fired by `EventKind::Click`.
    WindowPointerUp { id: u32, button: u32 },

    ClipButtonClicked,
    PopoverOpened(PopupKey),
    PopoverDismissed(PopupKey),
    ClipActivated(u64),
    ClipPinToggled { id: u64, pinned: bool },
    ClipRemoved(u64),
    ClipCleared,
}

/// Contract §3.1: the inbox carries `Msg` across a thread, so it must be `Send`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Msg>();
};

/// The command surface when there is no session bus.
///
/// The GTK panel's `wire_taskbar`/`wire_clipboard` returned early on a proxy
/// failure and left that half of the bar inert. One `App` cannot return early
/// from half of itself, so the inertness moves behind the traits: the bar still
/// opens, still paints and still reserves its exclusive zone, and every command
/// is dropped with one warning.
#[derive(Debug, Default)]
pub struct Offline;

impl CompositorCommands for Offline {
    fn focus_window(&self, id: u32) {
        tracing::warn!(id, "no session bus; focus_window dropped");
    }
    fn close_window(&self, id: u32) {
        tracing::warn!(id, "no session bus; close_window dropped");
    }
    fn set_workspace(&self, id: u32) {
        tracing::warn!(id, "no session bus; set_workspace dropped");
    }
}

impl ClipCommands for Offline {
    fn activate(&self, id: u64) {
        tracing::warn!(id, "no session bus; activate dropped");
    }
    fn pin(&self, id: u64, on: bool) {
        tracing::warn!(id, on, "no session bus; pin dropped");
    }
    fn remove(&self, id: u64) {
        tracing::warn!(id, "no session bus; remove dropped");
    }
    fn clear(&self) {
        tracing::warn!("no session bus; clear dropped");
    }
}

/// Fold one message. Never blocks and never calls D-Bus: every outbound
/// command leaves as a `Cmd::Task`, which the loop runs after the fold.
pub fn update(m: &mut PanelModel, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::Compositor(u) => {
            m.taskbar.apply(Arc::unwrap_or_clone(u));
            Cmd::None
        }
        Msg::Clip(u) => {
            m.clipboard.apply(Arc::unwrap_or_clone(u));
            *m.history.borrow_mut() = m.clipboard.entries.clone();
            Cmd::None
        }
        Msg::WorkspaceClicked(_)
        | Msg::WindowClicked(_)
        | Msg::WindowPointerUp { .. }
        | Msg::ClipButtonClicked
        | Msg::PopoverOpened(_)
        | Msg::PopoverDismissed(_)
        | Msg::ClipActivated(_)
        | Msg::ClipPinToggled { .. }
        | Msg::ClipRemoved(_)
        | Msg::ClipCleared => Cmd::None,
    }
}

/// The whole bar. `#bar` is what `style.css`'s first selector names.
pub fn view(_m: &PanelModel) -> View<Msg> {
    box_(Orientation::Horizontal, Vec::<View<Msg>>::new()).id("bar")
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo};

    /// A recording `CompositorCommands`. `RefCell`, not `Mutex`: these unit
    /// tests are single-threaded, and so is `update`. The cross-thread mock
    /// the harness tests need lives in `shell/tests/support/mod.rs`.
    #[derive(Default)]
    pub(crate) struct MockWm {
        pub(crate) calls: RefCell<Vec<(String, u32)>>,
    }

    impl CompositorCommands for MockWm {
        fn focus_window(&self, id: u32) {
            self.calls.borrow_mut().push(("focus".into(), id));
        }
        fn close_window(&self, id: u32) {
            self.calls.borrow_mut().push(("close".into(), id));
        }
        fn set_workspace(&self, id: u32) {
            self.calls.borrow_mut().push(("workspace".into(), id));
        }
    }

    #[derive(Default)]
    pub(crate) struct MockClip {
        pub(crate) calls: RefCell<Vec<(String, u64)>>,
    }

    impl ClipCommands for MockClip {
        fn activate(&self, id: u64) {
            self.calls.borrow_mut().push(("activate".into(), id));
        }
        fn pin(&self, id: u64, _on: bool) {
            self.calls.borrow_mut().push(("pin".into(), id));
        }
        fn remove(&self, id: u64) {
            self.calls.borrow_mut().push(("remove".into(), id));
        }
        fn clear(&self) {
            self.calls.borrow_mut().push(("clear".into(), 0));
        }
    }

    pub(crate) fn win(id: u32, title: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: "app".into(),
            title: title.into(),
            pid: 0,
            workspace: 0,
            geometry: Rectangle {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
            attention: false,
        }
    }

    pub(crate) fn snapshot(windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Snapshot {
        Snapshot {
            seq: 1,
            windows,
            workspaces,
            active_workspace: 0,
        }
    }

    /// The mocks, the model constructor and the two recording vectors, in one
    /// place, so every test below reads the same way.
    pub(crate) fn panel() -> (PanelModel, Rc<MockWm>, Rc<MockClip>) {
        let wm = Rc::new(MockWm::default());
        let clip = Rc::new(MockClip::default());
        let model = PanelModel::new(wm.clone(), clip.clone());
        (model, wm, clip)
    }

    #[test]
    fn a_fresh_panel_renders_the_bar() {
        let (m, _, _) = panel();
        let v = view(&m);
        assert_eq!(
            v.props.str(icedtea_ui::view::PropName::Id),
            Some("bar"),
            "style.css's first selector is #bar"
        );
    }

    #[test]
    fn a_compositor_snapshot_reaches_the_taskbar_model() {
        let (mut m, _, _) = panel();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(snapshot(
                vec![win(1, "One"), win(2, "Two")],
                vec![WorkspaceInfo {
                    id: 0,
                    name: String::new(),
                }],
            )))),
        );
        assert_eq!(
            m.taskbar
                .windows
                .iter()
                .map(|w| w.id.0)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn a_history_update_reaches_both_the_model_and_the_popover_mirror() {
        let (mut m, _, _) = panel();
        let _ = update(
            &mut m,
            Msg::Clip(Arc::new(ClipUpdate::History(vec![ClipEntry {
                id: 10,
                kind: icedtea_contract::ClipKind::Text,
                preview: "copied text".into(),
                mime: "text/plain".into(),
                pinned: false,
                source_app: None,
            }]))),
        );
        assert_eq!(m.clipboard.entries.len(), 1);
        assert_eq!(
            m.history.borrow().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![10],
            "the popover's mirror must follow the model"
        );
    }
}
```

**`shell/src/main.rs`** — replace the whole file:

```rust
//! `icedtea-shell` — a layer-shell panel: a window/workspace taskbar driven by
//! `org.icedtea.Compositor`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. One `App` on one surface, one loop thread; D-Bus
//! runs on its own workers and reaches the loop through the inbox.

use std::rc::Rc;

use icedtea_shell::clip_client::{ClipCommands, ClipProxy};
use icedtea_shell::compositor_client::{CompositorCommands, CompositorProxy};
use icedtea_shell::panel::{self, BAR_HEIGHT, Offline, PanelModel};
use icedtea_shell::style;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::App;
use icedtea_ui::window::{LayerSpec, Role, SurfaceSpec, Window};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = run() {
        tracing::error!(%err, "icedtea-shell exited");
        std::process::exit(1);
    }
}

/// The panel's surface: anchored left/right/top, `Layer::Top`, no keyboard.
///
/// Every value is the gtk4-layer-shell call it replaces. Anchored **top**, not
/// bottom: the source comment stands — a bottom bar lands below the visible
/// area on a display whose viewport is shorter than the reported output (a VM
/// console). `keyboard: None` matches GTK4 layer-shell's unset default; the
/// panel takes no keyboard focus, and the popover's search field has no IME
/// until M6. `exclusive_zone` is the literal `BAR_HEIGHT` because `LayerSpec`
/// has no "auto" (contract §6 P5-D6). The initial size is `(800, 28)`, the
/// pair `set_default_size(800, 28)` + `set_size_request(-1, 28)` forced: a
/// 0-height layer surface never commits a real buffer.
fn spec() -> SurfaceSpec {
    SurfaceSpec {
        role: Role::Layer(LayerSpec {
            layer: zwlr_layer_shell_v1::Layer::Top,
            anchor: zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right
                | zwlr_layer_surface_v1::Anchor::Top,
            margin: [0, 0, 0, 0],
            exclusive_zone: BAR_HEIGHT,
            keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::None,
        }),
        #[allow(
            clippy::cast_sign_loss,
            reason = "BAR_HEIGHT is a positive literal constant"
        )]
        size: (800, BAR_HEIGHT as u32),
        // A layer surface has no `namespace` field: `title` is the namespace.
        title: "icedtea-shell".to_string(),
        app_id: "org.icedtea.Shell".to_string(),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let wm: Rc<dyn CompositorCommands> = match CompositorProxy::new() {
        Ok(proxy) => Rc::new(proxy),
        Err(err) => {
            tracing::error!(%err, "no session bus; window commands disabled");
            Rc::new(Offline)
        }
    };
    let clip: Rc<dyn ClipCommands> = match ClipProxy::new() {
        Ok(proxy) => Rc::new(proxy),
        Err(err) => {
            tracing::error!(%err, "no session bus; clipboard commands disabled");
            Rc::new(Offline)
        }
    };

    let window = Window::open(spec(), style::sheet(), FontDatabase::new())?;
    App::new(PanelModel::new(wm, clip), panel::update, panel::view).run(window)?;
    Ok(())
}
```

**`shell/tests/shell_gtk.rs`** — it names `gtk4` and cannot compile now. It is
deleted for good in Task 17, but the crate must build at every commit, so
neutralise it here by replacing the whole file with:

```rust
//! Superseded by `tests/panel.rs` (M5 P5). Deleted in Task 17, once the
//! replacement asserts everything this file did.
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --lib
cargo build -p icedtea-shell
cargo tree -p icedtea-shell -e normal -i gtk4
cargo tree -p icedtea-shell -e normal -i gtk4-layer-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: `cargo test --lib` reports `9 passed` (taskbar 5 + its new Arc test,
clipboard 1 + its new Arc test, `compositor_client::version_mismatch` 1, the
style-sheet gradient test 1, and panel's 3 — count the run's own total and
record it in the commit; the point is that the six pre-existing model tests are
among them, unmodified). Both `cargo tree -i` invocations must print
`error: package ID specification ... did not match any packages`. Clippy clean.

**Mutation check.** Change `theme_from_env`'s fallback from `Theme::Dark` to
`Theme::parse("nonsense").unwrap()` — it will not compile, which is the point:
substitute instead a fallback of `Theme::Light` and confirm
`the_panel_sheet_keeps_the_focus_and_attention_gradients` still passes (it
iterates all three themes, so it is not the fallback's guard); then delete the
`#bar button.focused` rule from `shell/style.css` and confirm that test fails
with "the #bar button.focused accent is missing". Restore both.

### Step 5 — commit

```
git add shell/Cargo.toml shell/src/lib.rs shell/src/main.rs shell/src/panel.rs \
        shell/src/style.rs shell/src/taskbar.rs shell/src/clipboard.rs shell/tests/shell_gtk.rs
git rm shell/src/bridge.rs
git commit -m "$(cat <<'EOF'
refactor(shell): drop gtk4 and open the panel on icedtea-ui

The panel becomes one `App<PanelModel, Msg>` on one `Role::Layer` surface:
gtk4, gtk4-layer-shell and the glib bridge are gone, `taskbar::render`,
`clipboard::render` and `clipboard::connect_activation` with them, and
`style.css` compiles onto the bundled Adwaita stack instead of registering a
display-wide CssProvider. The pure models, both D-Bus clients and all six
model tests are untouched; the two update enums gain `Clone` because the
inbox carries them across a thread inside an `Arc`.

`auto_exclusive_zone_enable()` becomes the literal 28 `set_size_request` used
to force (M5 contract amendment P5-D6); `wayland-protocols-wlr` joins the
crate so `LayerSpec`'s fields are nameable (P5-D7).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6 — the taskbar view: workspaces, windows, and the clip button

Contract §3.3: `box_(Horizontal, [workspaces(m), windows(m), clip_button(m)])
.id("bar")`, keyed by workspace id and window id, with the label and class
rules of `taskbar::render` carried over exactly.

**Files:** `shell/src/panel.rs`.

### Interfaces

**Produces:**

```rust
// shell/src/panel.rs
pub fn view(m: &PanelModel) -> View<Msg>;   // body filled in
fn workspaces(m: &PanelModel) -> View<Msg>;
fn windows(m: &PanelModel) -> View<Msg>;
fn clip_button(m: &PanelModel) -> View<Msg>;
```

Widget ids, **normative** — every gate in Tasks 13–16 addresses these and no
others: `bar`, `workspaces`, `windows`, `clip`, `ws_<workspace id>`,
`window_<window id>`.

**Consumes:** `icedtea_ui::view::builders::{box_, button}`, `View::key`,
`View::id`, `View::class`, `View::hexpand`, `ButtonExt`-free `on_click`
(universal builder), `Cmd::Task` (M5-D3, P0).

### Step 1 — the failing test

Append to `shell/src/panel.rs`'s `mod tests`:

```rust
    use icedtea_ui::view::{EventKind, PropName};

    /// Find the first descendant of `v` whose `id` prop is `id`.
    fn by_id<'a>(v: &'a View<Msg>, id: &str) -> Option<&'a View<Msg>> {
        if v.props.str(PropName::Id) == Some(id) {
            return Some(v);
        }
        v.children.iter().find_map(|c| by_id(c, id))
    }

    /// The `Label` prop of every direct child of the container `id` names.
    fn labels(v: &View<Msg>, id: &str) -> Vec<String> {
        by_id(v, id)
            .expect("container")
            .children
            .iter()
            .map(|c| c.props.str(PropName::Label).unwrap_or_default().to_string())
            .collect()
    }

    fn seeded() -> (PanelModel, Rc<MockWm>, Rc<MockClip>) {
        let (mut m, wm, clip) = panel();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(snapshot(
                vec![win(1, "One"), win(2, "Two")],
                vec![
                    WorkspaceInfo {
                        id: 0,
                        name: String::new(),
                    },
                    WorkspaceInfo {
                        id: 1,
                        name: "web".into(),
                    },
                ],
            )))),
        );
        (m, wm, clip)
    }

    #[test]
    fn the_bar_holds_workspaces_windows_and_the_clip_button() {
        let (m, _, _) = seeded();
        let v = view(&m);
        assert_eq!(v.props.str(PropName::Id), Some("bar"));
        let ids: Vec<Option<&str>> = v
            .children
            .iter()
            .map(|c| c.props.str(PropName::Id))
            .collect();
        assert_eq!(
            ids,
            vec![Some("workspaces"), Some("windows"), Some("clip")],
            "contract §3.3's order: workspaces, windows, clip"
        );
    }

    #[test]
    fn a_window_button_is_labelled_by_title_then_app_id() {
        let (mut m, _, _) = seeded();
        assert_eq!(labels(&view(&m), "windows"), vec!["One", "Two"]);
        // A window with no title falls back to its app id, verbatim from
        // `taskbar::render`.
        let mut untitled = win(3, "");
        untitled.app_id = "org.example.Thing".into();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Opened(untitled))),
        );
        assert_eq!(
            labels(&view(&m), "windows"),
            vec!["One", "Two", "org.example.Thing"]
        );
    }

    #[test]
    fn a_workspace_button_is_labelled_by_name_then_one_indexed_id() {
        let (m, _, _) = seeded();
        assert_eq!(
            labels(&view(&m), "workspaces"),
            vec!["1", "web"],
            "an unnamed workspace shows `id + 1`; a named one shows its name"
        );
    }

    #[test]
    fn the_active_workspace_and_the_focused_window_carry_their_classes() {
        let (mut m, _, _) = seeded();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::WorkspaceSet {
                id: 1,
                active: true,
            })),
        );
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Updated {
                id: 2,
                update: icedtea_contract::WindowUpdate {
                    focused: Some(true),
                    attention: Some(true),
                    ..Default::default()
                },
            })),
        );
        let v = view(&m);
        let classes = |id: &str| -> Vec<String> {
            match by_id(&v, id).expect("button").props.get(PropName::Classes) {
                Some(icedtea_ui::view::Prop::Classes(list)) => {
                    list.iter().map(|c| c.to_string()).collect()
                }
                _ => Vec::new(),
            }
        };
        assert!(classes("ws_1").contains(&"active".to_string()));
        assert!(!classes("ws_0").contains(&"active".to_string()));
        assert!(classes("window_2").contains(&"focused".to_string()));
        assert!(classes("window_2").contains(&"attention".to_string()));
        assert!(classes("window_1").is_empty());
    }

    #[test]
    fn clicking_a_window_button_reaches_focus_window_on_the_command_surface() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "window_1")
            .expect("window_1")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(msg, Msg::WindowClicked(1)));
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert_eq!(wm.calls.borrow().as_slice(), [("focus".to_string(), 1)]);
    }

    #[test]
    fn clicking_a_workspace_button_reaches_set_workspace() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "ws_1")
            .expect("ws_1")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(msg, Msg::WorkspaceClicked(1)));
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert_eq!(wm.calls.borrow().as_slice(), [("workspace".to_string(), 1)]);
    }
```

and, above them in the same `mod tests`, the one helper every command
assertion needs:

```rust
    /// Run a `Cmd`'s side effects the way the loop does: `Cmd::Task` bodies,
    /// in order, after the fold. Anything else is inert here — `Cmd::OpenPopup`
    /// and `Cmd::ClosePopup` need a window, and the tests that care about them
    /// assert on the command's shape instead.
    fn run_tasks(cmd: Cmd<Msg>) {
        match cmd {
            Cmd::Task(f) => f(),
            Cmd::Batch(list) => {
                for c in list {
                    run_tasks(c);
                }
            }
            _ => {}
        }
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --lib
```

Expected: six failures. `the_bar_holds_workspaces_windows_and_the_clip_button`
fails with `left: []` against the three ids; the label tests panic in
`labels` with `container`; the class test panics with `button`; the two click
tests panic with `a click handler`.

### Step 3 — implement

Replace `panel::view` and add the three private builders:

```rust
/// The whole bar. `#bar` is what `style.css`'s first selector names.
pub fn view(m: &PanelModel) -> View<Msg> {
    box_(
        Orientation::Horizontal,
        [workspaces(m), windows(m), clip_button(m)],
    )
    .id("bar")
}

/// One button per workspace, keyed by workspace id.
///
/// Label, one-indexed fallback and the `active` class are `taskbar::render`'s
/// rules verbatim; what changed is that they are now computed from the model
/// on every `view` instead of being written into a widget on every rebuild.
fn workspaces(m: &PanelModel) -> View<Msg> {
    let buttons: Vec<View<Msg>> = m
        .taskbar
        .workspaces
        .iter()
        .map(|ws| {
            let id = ws.id;
            let label = if ws.name.is_empty() {
                (id + 1).to_string()
            } else {
                ws.name.clone()
            };
            let view = button(&label)
                .key(u64::from(id))
                .id(&format!("ws_{id}"))
                .on_click(Msg::WorkspaceClicked(id));
            if id == m.taskbar.active_workspace {
                view.class("active")
            } else {
                view
            }
        })
        .collect();
    box_(Orientation::Horizontal, buttons).id("workspaces")
}

/// One button per window, keyed by window id.
///
/// `hexpand` lives here, not on a wrapper: the GTK panel kept the taskbar in
/// its own child box only so the clipboard button survived
/// `taskbar::render`'s clear-and-rebuild. There is no rebuild to survive.
///
/// `attention` surfaces the compositor's refused-activation flag
/// (`State::request_activate`), which is the whole point of the bit.
fn windows(m: &PanelModel) -> View<Msg> {
    let buttons: Vec<View<Msg>> = m
        .taskbar
        .windows
        .iter()
        .map(|w| {
            let id = w.id.0;
            let label = if w.title.is_empty() {
                w.app_id.clone()
            } else {
                w.title.clone()
            };
            let mut view = button(&label)
                .key(u64::from(id))
                .id(&format!("window_{id}"))
                .on_click(Msg::WindowClicked(id));
            if w.focused {
                view = view.class("focused");
            }
            if w.attention {
                view = view.class("attention");
            }
            view
        })
        .collect();
    box_(Orientation::Horizontal, buttons)
        .id("windows")
        .hexpand(true)
}

/// The clipboard popover's trigger. `active` while the popover is open, the
/// same class `#workspaces button.active` uses for the same "this is the one"
/// meaning.
fn clip_button(m: &PanelModel) -> View<Msg> {
    let view = button("clip").id("clip").on_click(Msg::ClipButtonClicked);
    if m.open_popover.is_some() {
        view.class("active")
    } else {
        view
    }
}
```

and replace the two catch-all `update` arms with the real ones:

```rust
        Msg::WorkspaceClicked(id) => {
            let wm = m.wm.clone();
            Cmd::Task(Rc::new(move || wm.set_workspace(id)))
        }
        Msg::WindowClicked(id) => {
            let wm = m.wm.clone();
            Cmd::Task(Rc::new(move || wm.focus_window(id)))
        }
        Msg::WindowPointerUp { .. }
        | Msg::ClipButtonClicked
        | Msg::PopoverOpened(_)
        | Msg::PopoverDismissed(_)
        | Msg::ClipActivated(_)
        | Msg::ClipPinToggled { .. }
        | Msg::ClipRemoved(_)
        | Msg::ClipCleared => Cmd::None,
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --lib
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: every test in the crate green, the six new ones among them.

**Mutation check.** In `windows`, swap the label fallback to always use
`w.app_id`: `a_window_button_is_labelled_by_title_then_app_id` must fail with
`["app", "app", "org.example.Thing"]`. Restore. Then drop the
`if id == m.taskbar.active_workspace` branch in `workspaces`:
`the_active_workspace_and_the_focused_window_carry_their_classes` must fail on
the `ws_1` assertion. Restore.

### Step 5 — commit

```
git add shell/src/panel.rs
git commit -m "$(cat <<'EOF'
feat(shell): render the taskbar reactively from the model

`view` emits [workspaces][windows][clip] keyed by workspace id and window id,
so the reconciler keeps hover and focus identity across an update instead of
clearing and rebuilding the containers. Label derivation (title-or-app_id,
name-or-one-indexed-id) and the active/focused/attention classes are
`taskbar::render`'s rules verbatim; `hexpand` moves onto `#windows` now that
there is no rebuild for the clipboard button to survive. Clicks leave as
`Cmd::Task` onto the `CompositorCommands` seam, never from inside `update`.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7 — middle-click closes a window

`taskbar::render` added a `GestureClick` with `set_button(2)` per window
button. The toolkit equivalent is P0's `on_pointer_up_with_button` (M5-D5);
`update` acts only on `BTN_MIDDLE`.

**Files:** `shell/src/panel.rs`.

### Interfaces

**Produces:** the `Msg::WindowPointerUp { id, button }` arm of `update`, and
the `on_pointer_up_with_button` binding on every window button.

**Consumes:** `View::on_pointer_up_with_button` and
`icedtea_ui::window::pointer::{BTN_LEFT, BTN_MIDDLE}` (M5-D5, P0);
`Handlers::fire_pair_button` (M5-D5, P0).

### Step 1 — the failing test

Append to `mod tests`:

```rust
    #[test]
    fn middle_clicking_a_window_button_closes_it() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "window_2")
            .expect("window_2")
            .handlers
            .fire_pair_button(
                EventKind::PointerUp,
                4.0,
                4.0,
                icedtea_ui::window::pointer::BTN_MIDDLE,
            )
            .expect("a pointer-up handler");
        assert!(matches!(
            msg,
            Msg::WindowPointerUp {
                id: 2,
                button: 0x112
            }
        ));
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert_eq!(wm.calls.borrow().as_slice(), [("close".to_string(), 2)]);
    }

    #[test]
    fn a_left_release_on_a_window_button_closes_nothing() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "window_2")
            .expect("window_2")
            .handlers
            .fire_pair_button(
                EventKind::PointerUp,
                4.0,
                4.0,
                icedtea_ui::window::pointer::BTN_LEFT,
            )
            .expect("a pointer-up handler");
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert!(
            wm.calls.borrow().is_empty(),
            "a left release is the focus path's business, not close's: {:?}",
            wm.calls.borrow()
        );
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --lib -- middle_clicking a_left_release
```

Expected: both panic at `a pointer-up handler` — nothing is bound to
`EventKind::PointerUp` yet.

### Step 3 — implement

In `windows`, after the `.on_click(...)`:

```rust
                .on_pointer_up_with_button(move |_, _, button| Msg::WindowPointerUp {
                    id,
                    button,
                });
```

(the binding sits on the same `let mut view = button(..)` expression, before
the `focused`/`attention` branches).

In `update`, replace the `Msg::WindowPointerUp { .. }` catch-all with:

```rust
        // Middle-click closes. A left release arrives here too and is ignored:
        // focus is `EventKind::Click`'s job, and a node carrying both handlers
        // produces both messages (M5-D5 §5), so this arm must be
        // order-independent and must not act on `BTN_LEFT`.
        Msg::WindowPointerUp { id, button } if button == BTN_MIDDLE => {
            let wm = m.wm.clone();
            Cmd::Task(Rc::new(move || wm.close_window(id)))
        }
        Msg::WindowPointerUp { .. } => Cmd::None,
```

and add `use icedtea_ui::window::pointer::BTN_MIDDLE;` to the file's imports.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --lib
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Change the guard to `button == BTN_LEFT`:
`middle_clicking_a_window_button_closes_it` must fail with an empty calls
vector, and `a_left_release_on_a_window_button_closes_nothing` must fail with
`[("close", 2)]`. Restore.

### Step 5 — commit

```
git add shell/src/panel.rs
git commit -m "$(cat <<'EOF'
feat(shell): middle-click a window button to close it

The GTK panel used a GestureClick pinned to button 2; the toolkit equivalent
is M5-D5's `on_pointer_up_with_button`, with `update` acting only on
BTN_MIDDLE. A left release reaches the same handler and is ignored -- focus
stays on EventKind::Click -- so the two orderings are indistinguishable.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8 — open, track and dismiss the clipboard popover

Contract §3.4: `Msg::ClipButtonClicked` opens a popup anchored to the `clip`
button when none is open and closes the open one otherwise; `open_popover` is
the only place the state lives; an outside click produces
`InputEvent::PopupDone` → `Msg::PopoverDismissed`.

**Files:** `shell/src/panel.rs`.

### Interfaces

**Produces:**

```rust
// shell/src/panel.rs
/// The popover's body, as a `Cmd::OpenPopup` payload can build it.
///
/// Reads the shared `history` mirror, not the model: the loop re-runs this
/// closure on every fold (contract §6 P5-D2) and it cannot borrow `PanelModel`.
pub fn popover_rows(entries: &[ClipEntry]) -> View<Msg>;

/// Contract §3.4's spelling, for callers that do hold the model.
pub fn popover_body(m: &PanelModel) -> View<Msg>;
```

**Consumes:** `Cmd::OpenPopup`/`Cmd::ClosePopup`,
`icedtea_ui::window::popup::{PopupAnchorPoint, Positioner}`, `PopupEvent` and
`App::on_popup` (P5-D4, Task 1).

### Step 1 — the failing test

Append to `mod tests`:

```rust
    use icedtea_ui::window::popup::PopupAnchorPoint;

    #[test]
    fn the_clip_button_opens_a_popup_anchored_to_its_own_box() {
        let (mut m, _, _) = seeded();
        m.clip_rect.set(Some(Rect::new(700.0, 0.0, 40.0, 28.0)));
        let cmd = update(&mut m, Msg::ClipButtonClicked);
        match cmd {
            Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(rect),
                positioner,
                ..
            } => {
                assert_eq!(
                    (rect.x, rect.y, rect.width, rect.height),
                    (700.0, 0.0, 40.0, 28.0),
                    "the anchor is the clip button's own border box"
                );
                assert_eq!(
                    format!("{positioner:?}").contains("320"),
                    true,
                    "the popover is POPOVER_SIZE wide: {positioner:?}"
                );
            }
            other => panic!("expected an OpenPopup, got {other:?}"),
        }
        assert!(
            m.open_popover.is_none(),
            "the key is not known until the loop reports it back"
        );
    }

    #[test]
    fn the_open_popover_key_comes_back_through_a_message_and_marks_the_button() {
        let (mut m, _, _) = seeded();
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        assert_eq!(m.open_popover, Some(key));
        let v = view(&m);
        let classes = match by_id(&v, "clip").expect("clip").props.get(PropName::Classes) {
            Some(icedtea_ui::view::Prop::Classes(list)) => {
                list.iter().map(|c| c.to_string()).collect::<Vec<_>>()
            }
            _ => Vec::new(),
        };
        assert!(classes.contains(&"active".to_string()));
    }

    #[test]
    fn a_second_click_on_the_clip_button_closes_the_open_popover() {
        let (mut m, _, _) = seeded();
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        let cmd = update(&mut m, Msg::ClipButtonClicked);
        assert!(
            matches!(cmd, Cmd::ClosePopup(k) if k == key),
            "expected ClosePopup({key:?}), got {cmd:?}"
        );
    }

    #[test]
    fn a_dismissal_clears_the_state_only_for_the_key_that_was_dismissed() {
        let (mut m, _, _) = seeded();
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        let _ = update(&mut m, Msg::PopoverDismissed(PopupKey::from_raw(9)));
        assert_eq!(
            m.open_popover,
            Some(key),
            "a stale key from an already-closed popup must not clear the live one"
        );
        let _ = update(&mut m, Msg::PopoverDismissed(key));
        assert_eq!(m.open_popover, None);
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --lib -- popover popup clip_button
```

Expected: `the_clip_button_opens_a_popup_anchored_to_its_own_box` panics with
`expected an OpenPopup, got None`;
`the_open_popover_key_comes_back_through_a_message_and_marks_the_button` fails
its `assert_eq!(m.open_popover, Some(key))` with `None`;
`a_second_click...` panics with `expected ClosePopup(...), got None`;
`a_dismissal...` fails with `None` on the first assertion.

### Step 3 — implement

Add to the imports:

```rust
use icedtea_ui::window::popup::{PopupAnchorPoint, Positioner};
```

Replace the four popover arms of `update`:

```rust
        Msg::ClipButtonClicked => match m.open_popover {
            Some(key) => Cmd::ClosePopup(key),
            None => {
                // `update` has no `&Window`, and `PopupAnchorPoint::Node`
                // wants a `Node` no handler can hand it, so the anchor is the
                // rect the `App::on_frame` hook published for `#clip` last
                // frame. Before the first frame there is none: fall back to a
                // zero-width rect at the bar's right-hand end, which is where
                // the button will be.
                let anchor = m.clip_rect.get().unwrap_or_else(|| {
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "BAR_HEIGHT is a small positive constant"
                    )]
                    Rect::new(0.0, 0.0, 1.0, m.bar_height as f32)
                });
                let history = m.history.clone();
                Cmd::OpenPopup {
                    anchor: PopupAnchorPoint::Rect(anchor),
                    positioner: Positioner::menu(anchor, POPOVER_SIZE),
                    view: Rc::new(move || popover_rows(&history.borrow())),
                }
            }
        },
        Msg::PopoverOpened(key) => {
            m.open_popover = Some(key);
            Cmd::None
        }
        Msg::PopoverDismissed(key) => {
            // Only for the key that was dismissed: a stale `popup_done` for a
            // popup already replaced must not close the live one.
            if m.open_popover == Some(key) {
                m.open_popover = None;
            }
            Cmd::None
        }
```

and add the body builders (Task 9 fills the rows; this task's stub is a real,
tested tree, not a placeholder — an empty history genuinely renders an empty
list and a Clear button):

```rust
/// The popover's body, as a `Cmd::OpenPopup` payload can build it.
///
/// Reads a slice, not the model: the loop re-runs this closure on every fold
/// (contract §6 P5-D2), and a payload closure cannot borrow `PanelModel`.
#[must_use]
pub fn popover_rows(_entries: &[ClipEntry]) -> View<Msg> {
    box_(
        Orientation::Vertical,
        [
            box_(Orientation::Vertical, Vec::<View<Msg>>::new()).id("history"),
            button("Clear").id("clip_clear").on_click(Msg::ClipCleared),
        ],
    )
    .id("popover")
}

/// Contract §3.4's spelling, for callers that do hold the model.
#[must_use]
pub fn popover_body(m: &PanelModel) -> View<Msg> {
    popover_rows(&m.history.borrow())
}
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --lib
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Drop the `if m.open_popover == Some(key)` guard from
`Msg::PopoverDismissed` and clear unconditionally:
`a_dismissal_clears_the_state_only_for_the_key_that_was_dismissed` must fail on
its first assertion with `None`. Restore.

### Step 5 — commit

```
git add shell/src/panel.rs
git commit -m "$(cat <<'EOF'
feat(shell): open and track the clipboard popover from the model

`Msg::ClipButtonClicked` opens a real popup surface anchored to the `clip`
button's own box -- the rect the per-frame window hook publishes, because
`update` has no window and `PopupAnchorPoint::Node` wants a node no handler
can supply -- and closes the open one on a second click. The key arrives back
as `Msg::PopoverOpened` and an outside click as `Msg::PopoverDismissed`, both
through `App::on_popup`, so `open_popover` is the only place the open/closed
state lives.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9 — the popover's history rows

One row per history entry, keyed by entry id: a wide button that pastes, a
pin/unpin toggle and a remove button, plus the footer's Clear.

This is contract §3.4's list, built from `button`s in a `box_` rather than a
`ListBox` — see **P5-D3** at the top of this plan for the two shipped
`ListBoxC` defects that rule the `ListBox` out. `Msg::ClipRemoved` gains a
trigger it did not have under the contract's Delete-key spelling either: the
panel's layer surface takes no keyboard focus (`KeyboardInteractivity::None`),
so a key-driven remove could never fire.

**Files:** `shell/src/panel.rs`.

### Interfaces

**Produces:** `popover_rows`'s real body and the four clipboard arms of
`update`.

Widget ids, **normative**: `popover`, `history`, `history_<entry id>`,
`history_open_<entry id>`, `history_pin_<entry id>`,
`history_del_<entry id>`, `clip_clear`.

**Consumes:** `Cmd::Task`, `Cmd::Batch`, `Cmd::ClosePopup`,
`View::halign`/`hexpand`, `icedtea_ui::layout::Align`.

### Step 1 — the failing test

Append to `mod tests`:

```rust
    fn clip_entry(id: u64, preview: &str, pinned: bool) -> ClipEntry {
        ClipEntry {
            id,
            kind: icedtea_contract::ClipKind::Text,
            preview: preview.into(),
            mime: "text/plain".into(),
            pinned,
            source_app: None,
        }
    }

    fn with_history(entries: Vec<ClipEntry>) -> (PanelModel, Rc<MockWm>, Rc<MockClip>) {
        let (mut m, wm, clip) = seeded();
        let _ = update(&mut m, Msg::Clip(Arc::new(ClipUpdate::History(entries))));
        (m, wm, clip)
    }

    #[test]
    fn the_popover_shows_one_row_per_history_entry_keyed_by_id() {
        let (m, _, _) = with_history(vec![
            clip_entry(10, "copied text", false),
            clip_entry(11, "second entry", true),
        ]);
        let body = popover_body(&m);
        let rows = by_id(&body, "history").expect("history").children.len();
        assert_eq!(rows, 2, "expected two history rows");
        assert!(by_id(&body, "history_10").is_some());
        assert!(by_id(&body, "history_11").is_some());
        assert_eq!(
            by_id(&body, "history_open_10")
                .expect("row 10's paste button")
                .props
                .str(PropName::Label),
            Some("copied text")
        );
        assert_eq!(
            by_id(&body, "history_pin_11")
                .expect("row 11's pin button")
                .props
                .str(PropName::Label),
            Some("unpin"),
            "a pinned entry offers `unpin`, exactly as `clipboard::render` did"
        );
        assert_eq!(
            by_id(&body, "history_pin_10")
                .expect("row 10's pin button")
                .props
                .str(PropName::Label),
            Some("pin")
        );
    }

    #[test]
    fn a_history_replacement_replaces_the_rows() {
        let (mut m, _, _) = with_history(vec![
            clip_entry(10, "copied text", false),
            clip_entry(11, "second entry", false),
        ]);
        let _ = update(
            &mut m,
            Msg::Clip(Arc::new(ClipUpdate::History(vec![clip_entry(
                12, "only", false,
            )]))),
        );
        let body = popover_body(&m);
        assert_eq!(
            by_id(&body, "history").expect("history").children.len(),
            1,
            "history update did not replace rows"
        );
        assert!(by_id(&body, "history_12").is_some());
    }

    #[test]
    fn activating_a_row_pastes_it_and_closes_the_popover() {
        let (mut m, _, clip) = with_history(vec![clip_entry(10, "copied text", false)]);
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        let body = popover_body(&m);
        let msg = by_id(&body, "history_open_10")
            .expect("row 10's paste button")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(msg, Msg::ClipActivated(10)));
        let cmd = update(&mut m, msg);
        assert!(
            format!("{cmd:?}").contains("ClosePopup"),
            "activating a row must dismiss the popover: {cmd:?}"
        );
        run_tasks(cmd);
        assert_eq!(clip.calls.borrow().as_slice(), [("activate".to_string(), 10)]);
    }

    #[test]
    fn pinning_and_removing_a_row_reach_the_clip_command_surface() {
        let (mut m, _, clip) = with_history(vec![clip_entry(10, "copied text", false)]);
        let body = popover_body(&m);
        let pin = by_id(&body, "history_pin_10")
            .expect("pin")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(
            pin,
            Msg::ClipPinToggled {
                id: 10,
                pinned: true
            }
        ));
        run_tasks(update(&mut m, pin));

        let del = by_id(&body, "history_del_10")
            .expect("remove")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(del, Msg::ClipRemoved(10)));
        run_tasks(update(&mut m, del));

        let clear = by_id(&body, "clip_clear")
            .expect("clear")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(clear, Msg::ClipCleared));
        run_tasks(update(&mut m, clear));

        assert_eq!(
            clip.calls.borrow().as_slice(),
            [
                ("pin".to_string(), 10),
                ("remove".to_string(), 10),
                ("clear".to_string(), 0)
            ]
        );
    }

    /// The popover stays open for pin and remove: the daemon answers with a
    /// `history_changed`, the mirror follows, and P5-D2 rebuilds the open
    /// surface. Only a paste dismisses.
    #[test]
    fn pinning_leaves_the_popover_open() {
        let (mut m, _, _) = with_history(vec![clip_entry(10, "copied text", false)]);
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        let cmd = update(
            &mut m,
            Msg::ClipPinToggled {
                id: 10,
                pinned: true,
            },
        );
        assert!(
            !format!("{cmd:?}").contains("ClosePopup"),
            "pin must not dismiss the popover: {cmd:?}"
        );
        assert_eq!(m.open_popover, Some(key));
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --lib -- history popover pinning activating
```

Expected: `the_popover_shows_one_row_per_history_entry_keyed_by_id` fails with
`left: 0, right: 2`; `a_history_replacement_replaces_the_rows` fails with
`left: 0`; `activating_a_row_pastes_it_and_closes_the_popover` and
`pinning_and_removing_a_row_reach_the_clip_command_surface` panic at
`row 10's paste button` / `pin`; `pinning_leaves_the_popover_open` passes
already (nothing closes anything yet) and is the regression guard.

### Step 3 — implement

Add `use icedtea_ui::layout::Align;` to the imports and replace
`popover_rows`:

```rust
/// The popover's body, as a `Cmd::OpenPopup` payload can build it.
///
/// Reads a slice, not the model: the loop re-runs this closure on every fold
/// (contract §6 P5-D2), and a payload closure cannot borrow `PanelModel`.
///
/// Rows are buttons in a box rather than a `ListBox` (contract §6 P5-D3): a
/// `ListBoxC` swallows the press in the capture phase, so a control inside a
/// row can never receive its own click, and it reads the row index from
/// coordinates that are local to the innermost instance rather than to itself.
/// Three plain buttons per row is the path the interaction gate already
/// proves, and it gives `remove` a trigger that a keyboard-less layer surface
/// could never have offered.
#[must_use]
pub fn popover_rows(entries: &[ClipEntry]) -> View<Msg> {
    let rows: Vec<View<Msg>> = entries
        .iter()
        .map(|entry| {
            let id = entry.id;
            let pinned = entry.pinned;
            box_(
                Orientation::Horizontal,
                [
                    button(&entry.preview)
                        .id(&format!("history_open_{id}"))
                        .hexpand(true)
                        .halign(Align::Start)
                        .on_click(Msg::ClipActivated(id)),
                    button(if pinned { "unpin" } else { "pin" })
                        .id(&format!("history_pin_{id}"))
                        .on_click(Msg::ClipPinToggled {
                            id,
                            pinned: !pinned,
                        }),
                    button("remove")
                        .id(&format!("history_del_{id}"))
                        .on_click(Msg::ClipRemoved(id)),
                ],
            )
            .key(id)
            .id(&format!("history_{id}"))
        })
        .collect();
    box_(
        Orientation::Vertical,
        [
            box_(Orientation::Vertical, rows).id("history"),
            button("Clear").id("clip_clear").on_click(Msg::ClipCleared),
        ],
    )
    .id("popover")
}
```

Replace the four clipboard arms of `update`:

```rust
        // A paste dismisses; pin, remove and clear do not — the daemon answers
        // each with a `history_changed`, the mirror follows it, and P5-D2
        // rebuilds the open surface in place.
        Msg::ClipActivated(id) => {
            let clip = m.clip.clone();
            let task = Cmd::Task(Rc::new(move || clip.activate(id)));
            match m.open_popover {
                Some(key) => Cmd::Batch(vec![task, Cmd::ClosePopup(key)]),
                None => task,
            }
        }
        Msg::ClipPinToggled { id, pinned } => {
            let clip = m.clip.clone();
            Cmd::Task(Rc::new(move || clip.pin(id, pinned)))
        }
        Msg::ClipRemoved(id) => {
            let clip = m.clip.clone();
            Cmd::Task(Rc::new(move || clip.remove(id)))
        }
        Msg::ClipCleared => {
            let clip = m.clip.clone();
            Cmd::Task(Rc::new(move || clip.clear()))
        }
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --lib
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Drop the `match m.open_popover` from `Msg::ClipActivated`
and always return the bare task:
`activating_a_row_pastes_it_and_closes_the_popover` must fail with
"activating a row must dismiss the popover: Task(..)". Restore. Then invert the
pin label (`if pinned { "pin" } else { "unpin" }`):
`the_popover_shows_one_row_per_history_entry_keyed_by_id` must fail on the
`history_pin_11` assertion. Restore.

### Step 5 — commit

```
git add shell/src/panel.rs
git commit -m "$(cat <<'EOF'
feat(shell): render the clipboard history inside the popover

One row per entry, keyed by entry id: a wide paste button, a pin/unpin
toggle and a remove button, plus the footer's Clear. Every commanded action
leaves through `Cmd::Task` on the `ClipCommands` seam; a paste also dismisses
the popover, while pin, remove and clear leave it open for the
`history_changed` that follows.

Rows are buttons in a box rather than a `ListBox` (M5 contract amendment
P5-D3): `ListBoxC` swallows the press in the capture phase, so a control
inside a row never receives its own click, and it reads the row index from
coordinates local to the innermost instance rather than to itself.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10 — wire the D-Bus workers into the inbox

`bridge.rs`'s `glib::spawn_future_local` is replaced by one adapter thread per
client: it blocks on the client's `async_channel::Receiver` and pushes onto the
`InboxSender`, which wakes the loop through its pipe. Both clients are
otherwise untouched, `process::exit(1)` included.

**Files:** `shell/src/main.rs`.

### Interfaces

**Produces:**

```rust
// shell/src/main.rs
/// Drain `rx` on its own thread, wrapping each value into a `Msg` for the
/// app's inbox. Ends when the channel closes or the app drops its `Inbox`.
fn forward<T: Send + 'static>(
    rx: async_channel::Receiver<T>,
    tx: icedtea_ui::view::InboxSender<icedtea_shell::panel::Msg>,
    wrap: impl Fn(T) -> icedtea_shell::panel::Msg + Send + 'static,
) -> std::thread::JoinHandle<()>;
```

**Consumes:** `Inbox::new`, `InboxSender::send`, `App::with_inbox` (M5-D2, P0);
`App::on_popup` (P5-D4, Task 1); `compositor_client::spawn`,
`clip_client::spawn` (unchanged).

### Step 1 — the failing test

Add to the bottom of `shell/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use icedtea_shell::panel::Msg;
    use icedtea_ui::view::Inbox;

    /// Finding F7 is an architectural invariant, not a style preference: the
    /// shell and compositor marshal the contract types independently, so a
    /// `SignatureMismatch` on `GetState` must take the process down —
    /// `Restart=always` in `shell/systemd/icedtea-shell.service` is what then
    /// replaces the stale binary with a matching build. Logging and continuing
    /// would leave a permanently stale taskbar that systemd never restarts.
    /// A source-level assertion because the call path needs a live session bus
    /// and a running compositor.
    #[test]
    fn the_fatal_worker_path_still_exits() {
        let source = include_str!("compositor_client.rs");
        assert!(
            source.contains("std::process::exit(1)"),
            "compositor_client::spawn must still kill the process on a fatal \
             worker error (finding F7); no part may soften this into a log"
        );
    }

    /// A forward thread must not outlive the app: once the `Inbox` is dropped
    /// every `send` fails, and the thread's job is to notice and end. A leaked
    /// thread per client would keep a D-Bus connection alive after the panel
    /// closed.
    #[test]
    fn a_forward_thread_ends_when_the_app_drops_its_inbox() {
        let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
        let (value_tx, value_rx) = async_channel::unbounded::<u32>();
        let handle = super::forward(value_rx, tx, Msg::WorkspaceClicked);
        drop(inbox);
        // Enough traffic that the thread must attempt at least one send.
        for i in 0..4 {
            value_tx.send_blocking(i).expect("queue");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "the forward thread must end once the inbox is gone"
        );
        handle.join().expect("forward thread");
    }
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --bin icedtea-shell
```

Expected: a compile failure —

```
error[E0425]: cannot find function `forward` in module `super`
```

(`the_fatal_worker_path_still_exits` cannot even be reached; it passes as soon
as the module compiles, and is a guard, not a driver.)

### Step 3 — implement

Add to `shell/src/main.rs`'s imports:

```rust
use icedtea_shell::clip_client;
use icedtea_shell::clipboard::ClipUpdate;
use icedtea_shell::compositor_client;
use icedtea_shell::panel::Msg;
use icedtea_shell::taskbar::CompositorUpdate;
use icedtea_ui::view::{Inbox, InboxSender, PopupEvent};
use std::sync::Arc;
```

Add the adapter above `run`:

```rust
/// Drain `rx` on its own thread, wrapping each value into a `Msg` for the
/// app's inbox.
///
/// The replacement for `bridge.rs`'s `glib::spawn_future_local`: there is no
/// GLib main loop to post onto, and the `App`'s loop is a hand-rolled poll
/// over the Wayland fd plus whatever `Window::watch_fd` was given. The inbox
/// *is* that hook — `send` queues the message and writes one byte to the pipe
/// the loop already polls.
///
/// `recv_blocking` rather than an async runtime: this thread has exactly one
/// job, the D-Bus client already owns its own executor, and a blocking receive
/// costs nothing while idle. A failed send means the app dropped its `Inbox`,
/// which is how the thread learns to end.
fn forward<T: Send + 'static>(
    rx: async_channel::Receiver<T>,
    tx: InboxSender<Msg>,
    wrap: impl Fn(T) -> Msg + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(value) = rx.recv_blocking() {
            if tx.send(wrap(value)).is_err() {
                break;
            }
        }
    })
}
```

and replace `run`'s body from `let window = ...` onwards:

```rust
    let window = Window::open(spec(), style::sheet(), FontDatabase::new())?;
    let (inbox, tx) = Inbox::<Msg>::new()?;

    // Both clients keep their own `async_channel`, their own worker thread and
    // their own connection; only where their output lands has changed.
    let (comp_tx, comp_rx) = async_channel::unbounded::<CompositorUpdate>();
    let _comp_forward = forward(comp_rx, tx.clone(), |u| Msg::Compositor(Arc::new(u)));
    compositor_client::spawn(comp_tx);

    let (clip_tx, clip_rx) = async_channel::unbounded::<ClipUpdate>();
    let _clip_forward = forward(clip_rx, tx, |u| Msg::Clip(Arc::new(u)));
    clip_client::spawn(clip_tx);

    App::new(PanelModel::new(wm, clip), panel::update, panel::view)
        .with_inbox(inbox)
        .on_popup(|ev| match ev {
            PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
            PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
        })
        .run(window)?;
    Ok(())
```

The two `JoinHandle`s are bound rather than dropped so the threads are named in
a backtrace; they are detached by design — the process ends when `run` returns,
and a forward thread ends on its own when the inbox goes with it.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell
cargo build -p icedtea-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: the binary's two unit tests pass alongside the library's.

**Mutation check.** Change `forward`'s `if tx.send(..).is_err() { break }` to
`let _ = tx.send(..);`: `a_forward_thread_ends_when_the_app_drops_its_inbox`
must fail with "the forward thread must end once the inbox is gone". Restore.
Then, in a scratch copy only, replace `compositor_client.rs`'s
`std::process::exit(1)` with `return;` and confirm
`the_fatal_worker_path_still_exits` fails; restore with `git checkout --
shell/src/compositor_client.rs`.

### Step 5 — commit

```
git add shell/src/main.rs
git commit -m "$(cat <<'EOF'
feat(shell): feed both D-Bus workers into the app's inbox

`bridge.rs`'s glib local future becomes one adapter thread per client: it
blocks on the client's async_channel and pushes onto the `InboxSender`, whose
pipe wakes the loop. Both clients, their PascalCase members, their signal
names and `compositor_client`'s `process::exit(1)` rule are untouched, and a
source-level test now guards that rule. Popup lifecycle arrives through
`App::on_popup`, so the model owns the popover's open state.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11 — publish the panel's geometry

M5-D9: when `$ICEDTEA_PROBE_REPORT` is set, `probe <label> <x> <y>` and
`alloc <id> <x> <y> <w> <h>` lines are written once per frame in which the tree
changed. **`App::run` already does that** (P0-D4's `with_probe_report` plus the
env pickup), so `report_lines` here exists only for the lines P0's writer
cannot produce — the popup's own probe points (P5-D8), if the popover gate
needs them — and for publishing the `clip` button's box into
`PanelModel::clip_rect`, which is the popover's anchor. Do **not** re-emit the
window's `probe`/`alloc` lines from `on_frame`: they would double every line in
the report and the driver's parser would see each frame twice
(consistency-check ruling E1). If the popover gate reads only the window's
lines, `report_lines` collapses to the `clip_rect` publication and its unit
test with it.

**Files:** `shell/src/main.rs`.

### Interfaces

**Produces:**

```rust
// shell/src/main.rs
/// The report lines for one frame, in `Window::probe_points` order: a `probe`
/// line per point, then an `alloc` line for every label the window can resolve
/// to a laid-out node.
fn report_lines(
    points: &[icedtea_ui::window::ProbePoint],
    allocation: impl Fn(&str) -> Option<icedtea_ui::layout::Allocation>,
) -> Vec<String>;
```

**Consumes:** `App::on_frame` (P5-D5, Task 4), `Window::probe_points`,
`Window::allocation`, `ProbePoint` (M5-D9, P0).

### Step 1 — the failing test

Append to `shell/src/main.rs`'s `mod tests`:

```rust
    use icedtea_ui::layout::{Allocation, Rect};
    use icedtea_ui::window::ProbePoint;

    fn alloc(x: f32, y: f32, w: f32, h: f32) -> Allocation {
        Allocation {
            border_box: Rect::new(x, y, w, h),
            content_box: Rect::new(x, y, w, h),
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    #[test]
    fn the_report_names_every_probe_point_and_every_resolvable_allocation() {
        let points = vec![
            ProbePoint {
                label: "bar".into(),
                x: 400,
                y: 14,
            },
            ProbePoint {
                label: "clip".into(),
                x: 720,
                y: 14,
            },
            ProbePoint {
                label: "label".into(),
                x: 12,
                y: 14,
            },
        ];
        let lines = super::report_lines(&points, |id| match id {
            "bar" => Some(alloc(0.0, 0.0, 800.0, 28.0)),
            "clip" => Some(alloc(700.0, 0.0, 40.0, 28.0)),
            _ => None,
        });
        assert_eq!(
            lines,
            vec![
                "probe bar 400 14".to_string(),
                "probe clip 720 14".to_string(),
                "probe label 12 14".to_string(),
                "alloc bar 0 0 800 28".to_string(),
                "alloc clip 700 0 40 28".to_string(),
            ],
            "probes first in probe order, then one alloc per resolvable label"
        );
    }

    #[test]
    fn a_label_with_no_allocation_contributes_no_alloc_line() {
        let points = vec![ProbePoint {
            label: "ghost".into(),
            x: 1,
            y: 2,
        }];
        let lines = super::report_lines(&points, |_| None);
        assert_eq!(lines, vec!["probe ghost 1 2".to_string()]);
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --bin icedtea-shell
```

Expected:

```
error[E0425]: cannot find function `report_lines` in module `super`
```

### Step 3 — implement

Add above `run`:

```rust
/// The report lines for one frame.
///
/// A pure function of what the window just laid out, so the formatting is
/// testable without a compositor: `probe <label> <x> <y>` per probe point in
/// `Window::probe_points` order, then `alloc <id> <x> <y> <w> <h>` for every
/// label the window resolves to a laid-out node. The two formats are the ones
/// `ui/tests/support/mod.rs`'s `parse_probe_line`/`parse_allocation_line`
/// already parse (M5-D9).
///
/// Coordinates are truncated to whole pixels: a probe point is already an
/// integer, and an allocation's fractional part is below what a virtual
/// pointer can address.
fn report_lines(
    points: &[icedtea_ui::window::ProbePoint],
    allocation: impl Fn(&str) -> Option<icedtea_ui::layout::Allocation>,
) -> Vec<String> {
    let mut lines: Vec<String> = points
        .iter()
        .map(|p| format!("probe {} {} {}", p.label, p.x, p.y))
        .collect();
    for point in points {
        if let Some(a) = allocation(&point.label) {
            let b = a.border_box;
            lines.push(format!(
                "alloc {} {} {} {} {}",
                point.label, b.x as i32, b.y as i32, b.width as i32, b.height as i32
            ));
        }
    }
    lines
}
```

with the cast lint allowed at the function, since a negative or huge
coordinate is a layout bug the report should show rather than clamp:

```rust
#[allow(
    clippy::cast_possible_truncation,
    reason = "report coordinates are whole pixels; a fractional part is below what a pointer can address"
)]
```

Then, in `run`, build the hook and attach it. Insert before the `App::new`
call:

```rust
    // M5-D9: the geometry a harness gate addresses widgets by. Also the
    // popover's anchor: `update` has no `&Window`, so the `clip` button's box
    // is published here into the cell the model shares with it.
    let model = PanelModel::new(wm, clip);
    let clip_rect = model.clip_rect.clone();
    let report_path = std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from);
    let mut last_report: Vec<String> = Vec::new();
```

and change the `App::new(...)` chain to:

```rust
    App::new(model, panel::update, panel::view)
        .with_inbox(inbox)
        .on_popup(|ev| match ev {
            PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
            PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
        })
        .on_frame(move |w| {
            clip_rect.set(w.allocation("clip").map(|a| a.border_box));
            let Some(path) = report_path.as_ref() else {
                return;
            };
            let lines = report_lines(&w.probe_points(), |id| w.allocation(id));
            // Once per frame *in which the tree changed*: an unchanged report
            // is not rewritten, so a test that waits for a line never races a
            // rewrite of the same content.
            if lines == last_report {
                return;
            }
            last_report.clone_from(&lines);
            if let Err(err) = std::fs::write(path, lines.join("\n") + "\n") {
                tracing::warn!(%err, "cannot write $ICEDTEA_PROBE_REPORT");
            }
        })
        .run(window)?;
    Ok(())
```

The whole report is rewritten each time rather than appended: the panel's tree
is small, a reader wants the *current* geometry, and an appending report would
make "the second `probe clip` line" ambiguous after a relayout.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell
cargo build -p icedtea-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Drop the `if lines == last_report { return; }` guard and
confirm the two unit tests still pass (they exercise `report_lines`, not the
hook) — then restore it, because Task 12's `wait_for_report_line` depends on it
and Task 13's first assertion would flake without it. Then make `report_lines`
emit the alloc lines interleaved with the probes:
`the_report_names_every_probe_point_and_every_resolvable_allocation` must fail
on the vector comparison. Restore.

### Step 5 — commit

```
git add shell/src/main.rs
git commit -m "$(cat <<'EOF'
feat(shell): publish the panel's geometry each frame

`App::on_frame` writes M5-D9's `probe`/`alloc` lines to
`$ICEDTEA_PROBE_REPORT` whenever the laid-out tree changes, so a harness gate
addresses the bar's widgets by id instead of hard-coding coordinates, and it
publishes the `clip` button's border box into the cell `update` reads as the
popover's anchor rect.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12 — the harness scaffolding

`shell/tests/shell_gtk.rs` built widgets in its own process, called
`taskbar::render` by hand and emitted GTK signals. Its replacement runs the
**real** panel — the same `SurfaceSpec`, the same `view`/`update`, the same
sheet — on a thread inside the test process, against
`icedtea_harness::Compositor`, with the recording mocks behind the two traits
and a `VirtualPointerClient` for real clicks. The panel's geometry comes back
through `$ICEDTEA_PROBE_REPORT`, so no gate hard-codes a coordinate.

**Files:** `shell/src/panel.rs`, `shell/src/main.rs`,
`shell/tests/support/mod.rs` (new).

### Interfaces

**Produces:**

```rust
// shell/src/panel.rs — moved in from main.rs so the harness drives the
// same surface and the same report writer the binary does.
pub fn spec() -> icedtea_ui::window::SurfaceSpec;
pub fn report_lines(
    points: &[icedtea_ui::window::ProbePoint],
    allocation: impl Fn(&str) -> Option<icedtea_ui::layout::Allocation>,
) -> Vec<String>;
/// The `App::on_frame` hook: publishes `#clip`'s box into `clip_rect` and,
/// when `report` is `Some`, rewrites the probe report on every change.
pub fn frame_hook(
    clip_rect: Rc<Cell<Option<Rect>>>,
    report: Option<std::path::PathBuf>,
) -> impl FnMut(&icedtea_ui::window::Window) + 'static;
```

```rust
// shell/tests/support/mod.rs
pub struct MockWm(pub Arc<Mutex<Vec<(String, u32)>>>);   // Clone
pub struct MockClip(pub Arc<Mutex<Vec<(String, u64)>>>); // Clone
pub struct TempDir(PathBuf);
pub struct Panel { /* see below */ }

impl Panel {
    pub fn spawn(theme: Theme) -> Panel;
    pub fn send(&self, msg: Msg);
    pub fn wm_calls(&self) -> Vec<(String, u32)>;
    pub fn clip_calls(&self) -> Vec<(String, u64)>;
    pub fn report(&self) -> Vec<String>;
    pub fn wait_for(&self, prefix: &str) -> String;
    pub fn wait_until(&self, want: impl Fn(&[String]) -> bool, what: &str);
    pub fn point(&self, label: &str) -> (i32, i32);
    pub fn allocation(&self, id: &str) -> (i32, i32, i32, i32);
    pub fn labels_under(&self, container: &str) -> Vec<String>;
    pub fn click(&mut self, x: i32, y: i32);
    pub fn click_button(&mut self, x: i32, y: i32, button: u32);
    pub fn capture(&mut self) -> CapturedFrame;
    pub fn background(&self) -> (u8, u8, u8);
    pub fn output(&self) -> (u32, u32);
}
```

**Consumes:** `icedtea_harness::{Compositor, ScreencopyClient,
VirtualPointerClient, CapturedFrame}`, `icedtea_ui::shm::pixel_rgb`,
`Inbox`/`InboxSender` (M5-D2), `App::on_popup`/`on_frame` (P5-D4/D5).

### Step 1 — the failing test

Create `shell/tests/panel.rs` with the harness's own smoke test, which is what
proves the scaffolding before anything is asserted through it:

```rust
//! The panel, end to end: the real `view`/`update` on a real layer surface
//! under `icedtea_harness::Compositor`, driven by a virtual pointer, with the
//! command surface behind recording mocks.
//!
//! Replaces `tests/shell_gtk.rs`, which introspected a GTK widget tree in
//! its own process. Every coordinate here comes from the panel's own
//! `$ICEDTEA_PROBE_REPORT` (M5-D9), never from a literal.

mod support;

use icedtea_ui::gallery::Theme;
use support::Panel;

#[test]
fn the_panel_opens_a_layer_surface_and_reports_its_geometry() {
    let panel = Panel::spawn(Theme::Dark);
    let bar = panel.wait_for("alloc bar ");
    let fields: Vec<&str> = bar.split_whitespace().collect();
    assert_eq!(fields[0], "alloc");
    assert_eq!(fields[1], "bar");
    assert_eq!(
        fields[5], "28",
        "the bar commits its BAR_HEIGHT, which is also its exclusive zone: {bar}"
    );
    let (_, _, w, _) = panel.allocation("bar");
    assert!(
        w >= 100,
        "anchored left+right, the bar spans the output: {bar}"
    );
}
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --test panel
```

Expected: a compile failure — `error[E0583]: file not found for module
support`.

### Step 3a — move `spec`, `report_lines` and the frame hook into the library

The harness must drive the surface the binary opens, not a copy that can
drift. In `shell/src/panel.rs`, add `use icedtea_ui::window::{LayerSpec, Role,
SurfaceSpec, Window};` and
`use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1,
zwlr_layer_surface_v1};`, then move `spec()` and `report_lines()` here from
`main.rs` **byte for byte** (doc comments included), change both to `pub`, and
add the hook that binds them together:

```rust
/// The `App::on_frame` hook the binary and the harness both install.
///
/// Two jobs, one pass over the laid-out tree: publish `#clip`'s border box
/// into the cell `update` reads as the popover's anchor, and — when
/// `$ICEDTEA_PROBE_REPORT` named a path — rewrite the report whenever the
/// geometry changed. Unchanged geometry is not rewritten, so a reader waiting
/// for a line never races a rewrite of the same content.
pub fn frame_hook(
    clip_rect: Rc<Cell<Option<Rect>>>,
    report: Option<std::path::PathBuf>,
) -> impl FnMut(&Window) + 'static {
    let mut last: Vec<String> = Vec::new();
    move |w: &Window| {
        clip_rect.set(w.allocation("clip").map(|a| a.border_box));
        let Some(path) = report.as_ref() else {
            return;
        };
        let lines = report_lines(&w.probe_points(), |id| w.allocation(id));
        if lines == last {
            return;
        }
        last.clone_from(&lines);
        if let Err(err) = std::fs::write(path, lines.join("\n") + "\n") {
            tracing::warn!(%err, "cannot write $ICEDTEA_PROBE_REPORT");
        }
    }
}
```

Move the two `report_lines` unit tests out of `main.rs`'s `mod tests` into
`panel.rs`'s, unmodified except that `super::report_lines` becomes
`report_lines`. `main.rs` keeps `the_fatal_worker_path_still_exits` and
`a_forward_thread_ends_when_the_app_drops_its_inbox`, and its `run` shrinks to:

```rust
    let model = PanelModel::new(wm, clip);
    let clip_rect = model.clip_rect.clone();
    let report = std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from);
    let window = Window::open(panel::spec(), style::sheet(), FontDatabase::new())?;
    // ... inbox and the two forwards, unchanged ...
    App::new(model, panel::update, panel::view)
        .with_inbox(inbox)
        .on_popup(|ev| match ev {
            PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
            PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
        })
        .on_frame(panel::frame_hook(clip_rect, report))
        .run(window)?;
    Ok(())
```

with `main.rs`'s now-unused imports (`LayerSpec`, `Role`, `SurfaceSpec`, the
two protocol modules, `ProbePoint`, `Allocation`) deleted.

### Step 3b — the support module

Create `shell/tests/support/mod.rs`:

```rust
//! Shared scaffolding for `icedtea-shell`'s integration tests: one compositor,
//! one real panel on its own thread, recording mocks behind the two command
//! traits, and a pointer to click with.
//!
//! Not every test uses every helper, hence the blanket `dead_code` allow: this
//! module is compiled once per test binary that declares it.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use icedtea_contract::{
    ClipEntry, ClipKind, Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo,
};
use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};
use icedtea_shell::clip_client::ClipCommands;
use icedtea_shell::compositor_client::CompositorCommands;
use icedtea_shell::panel::{self, Msg, PanelModel};
use icedtea_shell::style;
use icedtea_ui::gallery::Theme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox, InboxSender, PopupEvent};
use icedtea_ui::window::Window;

/// How long any wait in this module gives the compositor and the panel.
///
/// A generous complexity bound, never a timing pin: the panel has to connect,
/// take a configure, lay out and paint before its first report exists.
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// `WAYLAND_DISPLAY` and `ICEDTEA_PROBE_REPORT` are process-global and every
/// test in this binary runs in the same process, so the window between setting
/// them and connecting is serialised. Each panel has its own compositor and
/// its own report file; only the handover is shared.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A recording `CompositorCommands`, shared with the panel's thread.
///
/// `Arc<Mutex<_>>`, unlike `panel.rs`'s unit-test mock: the panel runs on its
/// own thread here and the assertions read from the test's.
#[derive(Clone, Default)]
pub struct MockWm(pub Arc<Mutex<Vec<(String, u32)>>>);

impl CompositorCommands for MockWm {
    fn focus_window(&self, id: u32) {
        self.0.lock().expect("wm calls").push(("focus".into(), id));
    }
    fn close_window(&self, id: u32) {
        self.0.lock().expect("wm calls").push(("close".into(), id));
    }
    fn set_workspace(&self, id: u32) {
        self.0
            .lock()
            .expect("wm calls")
            .push(("workspace".into(), id));
    }
}

/// A recording `ClipCommands`, shared with the panel's thread.
#[derive(Clone, Default)]
pub struct MockClip(pub Arc<Mutex<Vec<(String, u64)>>>);

impl ClipCommands for MockClip {
    fn activate(&self, id: u64) {
        self.0
            .lock()
            .expect("clip calls")
            .push(("activate".into(), id));
    }
    fn pin(&self, id: u64, _on: bool) {
        self.0.lock().expect("clip calls").push(("pin".into(), id));
    }
    fn remove(&self, id: u64) {
        self.0
            .lock()
            .expect("clip calls")
            .push(("remove".into(), id));
    }
    fn clear(&self) {
        self.0.lock().expect("clip calls").push(("clear".into(), 0));
    }
}

/// A directory removed when the test ends, however it ends.
///
/// Hand-rolled rather than `tempfile`: contract §3.5 keeps shell's
/// dev-dependencies as they are, and this is ten lines.
pub struct TempDir(PathBuf);

impl TempDir {
    #[must_use]
    pub fn new(tag: &str) -> TempDir {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "icedtea-shell-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One compositor, one running panel, one pointer and one screencopy client.
pub struct Panel {
    pub wm: MockWm,
    pub clip: MockClip,
    tx: InboxSender<Msg>,
    pointer: VirtualPointerClient,
    screencopy: ScreencopyClient,
    report: PathBuf,
    output: (u32, u32),
    background: (u8, u8, u8),
    _dir: TempDir,
    _panel: JoinHandle<()>,
    // Dropped last: killing the compositor first would take the panel's
    // connection out from under it mid-assertion.
    _compositor: Compositor,
}

impl Panel {
    /// Boot a compositor and run the real panel against it under `theme`.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output or screencopy, or if the panel
    /// never publishes its first report.
    #[must_use]
    pub fn spawn(theme: Theme) -> Panel {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let (ow, oh) = compositor.output_size();
        let output = (ow as u32, oh as u32);

        let dir = TempDir::new(theme.name());
        let report = dir.path().join("report");

        let wm = MockWm::default();
        let clip = MockClip::default();
        let (handshake_tx, handshake_rx) = mpsc::channel::<InboxSender<Msg>>();

        let thread_wm = wm.clone();
        let thread_clip = clip.clone();
        let thread_socket = socket.clone();
        let thread_report = report.clone();
        let panel_thread = std::thread::spawn(move || {
            let window = {
                let _guard = ENV_LOCK.lock().expect("env lock");
                // SAFETY: the lock makes this the only thread setting or
                // reading these variables for the duration of the connect.
                unsafe {
                    std::env::set_var("WAYLAND_DISPLAY", &thread_socket);
                }
                match Window::open(
                    panel::spec(),
                    style::sheet_for(theme),
                    FontDatabase::new(),
                ) {
                    Ok(window) => window,
                    Err(err) => panic!("the panel could not open its layer surface: {err}"),
                }
            };
            let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
            if handshake_tx.send(tx).is_err() {
                return;
            }
            let model = PanelModel::new(
                Rc::new(thread_wm) as Rc<dyn CompositorCommands>,
                Rc::new(thread_clip) as Rc<dyn ClipCommands>,
            );
            let clip_rect = model.clip_rect.clone();
            let _ = App::new(model, panel::update, panel::view)
                .with_inbox(inbox)
                .on_popup(|ev| match ev {
                    PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
                    PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
                })
                .on_frame(panel::frame_hook(clip_rect, Some(thread_report)))
                .run(window);
        });

        let tx = handshake_rx
            .recv_timeout(TIMEOUT)
            .expect("the panel never opened its window");
        let mut screencopy = ScreencopyClient::spawn(&socket);
        let empty = screencopy.capture();
        // The bar is anchored to the top, so the bottom-right corner is
        // always wallpaper: that is the background every "did it paint?"
        // assertion compares against.
        let background = pixel(&empty, output.0 - 3, output.1 - 3).expect("background probe");
        let pointer = VirtualPointerClient::spawn(&socket);

        let panel = Panel {
            wm,
            clip,
            tx,
            pointer,
            screencopy,
            report,
            output,
            background,
            _dir: dir,
            _panel: panel_thread,
            _compositor: compositor,
        };
        let _ = panel.wait_for("alloc bar ");
        panel
    }

    /// Push a message onto the panel's inbox — what a D-Bus worker does.
    pub fn send(&self, msg: Msg) {
        self.tx.send(msg).expect("the panel is still running");
    }

    #[must_use]
    pub fn wm_calls(&self) -> Vec<(String, u32)> {
        self.wm.0.lock().expect("wm calls").clone()
    }

    #[must_use]
    pub fn clip_calls(&self) -> Vec<(String, u64)> {
        self.clip.0.lock().expect("clip calls").clone()
    }

    /// Every line of the panel's current report.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Poll the report until a line starting with `prefix` appears.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`]; the message lists what the report
    /// does hold, which is what makes a renamed id a readable failure.
    pub fn wait_for(&self, prefix: &str) -> String {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(line) = self.report().into_iter().find(|l| l.starts_with(prefix)) {
                return line;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "no report line starting with {prefix:?} within {TIMEOUT:?}; report holds {:?}",
            self.report()
        );
    }

    /// Poll the report until `want` accepts it.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_until(&self, want: impl Fn(&[String]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.report()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; report holds {:?}",
            self.report()
        );
    }

    /// The output-space centre of the probe point `label`.
    ///
    /// The layer surface is anchored left, right and top with zero margins, so
    /// its own (0, 0) is the output's: a window coordinate *is* an output
    /// coordinate here, and no offset arithmetic is needed.
    ///
    /// # Panics
    ///
    /// If the panel exposes no such probe point.
    #[must_use]
    pub fn point(&self, label: &str) -> (i32, i32) {
        let line = self.wait_for(&format!("probe {label} "));
        let fields: Vec<&str> = line.split_whitespace().collect();
        (
            fields[2].parse().expect("probe x"),
            fields[3].parse().expect("probe y"),
        )
    }

    /// `id`'s border box in output coordinates, as `(x, y, width, height)`.
    ///
    /// # Panics
    ///
    /// If the panel reports no allocation for `id`.
    #[must_use]
    pub fn allocation(&self, id: &str) -> (i32, i32, i32, i32) {
        let line = self.wait_for(&format!("alloc {id} "));
        let f: Vec<&str> = line.split_whitespace().collect();
        (
            f[2].parse().expect("x"),
            f[3].parse().expect("y"),
            f[4].parse().expect("w"),
            f[5].parse().expect("h"),
        )
    }

    /// The ids the report holds for direct children of `container`, in
    /// left-to-right order — the replacement for `shell_gtk.rs`'s `labels()`
    /// walk of a GTK widget tree.
    ///
    /// Membership is by prefix and geometry: `window_*`/`ws_*`/`history_*`
    /// ids are unique per entity, and an id whose box sits inside
    /// `container`'s box is one of its children. Ordered by `x` for a
    /// horizontal container and by `y` for a vertical one, which is decided by
    /// which of the container's dimensions is the larger.
    #[must_use]
    pub fn labels_under(&self, container: &str, prefix: &str) -> Vec<String> {
        let (cx, cy, cw, ch) = self.allocation(container);
        let mut found: Vec<(i32, i32, String)> = Vec::new();
        for line in self.report() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() != 6 || f[0] != "alloc" || !f[1].starts_with(prefix) {
                continue;
            }
            let (x, y): (i32, i32) = (f[2].parse().unwrap_or(0), f[3].parse().unwrap_or(0));
            if x >= cx && x < cx + cw && y >= cy && y < cy + ch {
                found.push((x, y, f[1].to_string()));
            }
        }
        if cw >= ch {
            found.sort_by_key(|(x, _, _)| *x);
        } else {
            found.sort_by_key(|(_, y, _)| *y);
        }
        found.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Left-click at `(x, y)`.
    pub fn click(&mut self, x: i32, y: i32) {
        self.click_button(x, y, icedtea_ui::window::pointer::BTN_LEFT);
    }

    /// Press and release `button` at `(x, y)`.
    ///
    /// The settle between the motion and the button repeats
    /// `ui/tests/support/mod.rs`'s `Driver::move_to` rationale verbatim: the
    /// compositor's assignment of pointer focus to the surface under the
    /// cursor is not synchronous with the `motion_absolute` that triggers it,
    /// and `wlr_seat_pointer_notify_button` drops a button with no focused
    /// surface silently.
    pub fn click_button(&mut self, x: i32, y: i32, button: u32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(25));
            self.pointer.pump();
        }
        self.pointer.button(button, true);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.button(button, false);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.pump();
    }

    /// One screencopy frame of the whole output.
    pub fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }

    #[must_use]
    pub fn background(&self) -> (u8, u8, u8) {
        self.background
    }

    #[must_use]
    pub fn output(&self) -> (u32, u32) {
        self.output
    }
}

/// The RGB of `(x, y)` in a captured frame, or `None` outside it.
#[must_use]
pub fn pixel(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture — `ui/tests/support/mod.rs`'s constant, for the
/// same format-conversion reason.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// Whether `a` and `b` are the same colour within [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn same(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let close = |p: u8, q: u8| i32::from(p).abs_diff(i32::from(q)) <= u32::from(SCREENCOPY_TOLERANCE);
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

/// Whether anything inside `rect` differs from `background`.
#[must_use]
pub fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h).any(|py| {
        (x..x + w).any(|px| {
            pixel(frame, px as u32, py as u32).is_some_and(|got| !same(got, background))
        })
    })
}

// --- fixtures, verbatim from `tests/shell_gtk.rs` -------------------------

#[must_use]
pub fn win(id: u32, title: &str) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: "app".into(),
        title: title.into(),
        pid: 0,
        workspace: 0,
        geometry: Rectangle {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        maximized: false,
        minimized: false,
        fullscreen: false,
        focused: false,
        attention: false,
    }
}

#[must_use]
pub fn snapshot(windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Snapshot {
    Snapshot {
        seq: 1,
        windows,
        workspaces,
        active_workspace: 0,
    }
}

#[must_use]
pub fn clip_entry(id: u64, preview: &str) -> ClipEntry {
    ClipEntry {
        id,
        kind: ClipKind::Text,
        preview: preview.into(),
        mime: "text/plain".into(),
        pinned: false,
        source_app: None,
    }
}
```

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --test panel
cargo test -p icedtea-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: `the_panel_opens_a_layer_surface_and_reports_its_geometry` passes;
the library's and binary's unit tests are unchanged in number and result.

**Mutation check.** Change `panel::spec`'s `exclusive_zone` from `BAR_HEIGHT`
to `0` — the smoke test still passes (the zone is not the bar's height), so
instead change `spec`'s `size` to `(800, 40)`: the test must fail with
`fields[5]` being `40`. Restore. Then change `frame_hook`'s
`w.allocation("clip")` to `w.allocation("nope")` and confirm no test fails
*yet* — that is Task 15's job, and the note belongs in this task's commit so
the reviewer knows the anchor is not yet covered.

### Step 5 — commit

```
git add shell/src/panel.rs shell/src/main.rs shell/tests/support/mod.rs shell/tests/panel.rs
git commit -m "$(cat <<'EOF'
test(shell): run the real panel under the harness

`tests/support` boots `icedtea_harness::Compositor`, runs the real
`panel::view`/`update` on its own thread against it with the recording mocks
behind `CompositorCommands`/`ClipCommands`, and reads every coordinate back
from the panel's own `$ICEDTEA_PROBE_REPORT`. `spec`, `report_lines` and the
frame hook move into the library so the harness drives exactly the surface
the binary opens.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13 — the parity test: a panel click reaches the command surface

Contract §3.6's re-expression of `shell_gtk.rs`, steps 1–4: the same seed
snapshot, the same `["One", "Two"]`, the same `("focus", 1)`, the same
`["Two"]` after a close, the same `"workspace"` entry — asserted through the
panel's own report and a real pointer instead of a GTK tree walk and
`emit_clicked`.

**Files:** `shell/tests/panel.rs`.

### Interfaces

**Consumes:** `support::Panel` (Task 12), `panel::Msg`,
`taskbar::CompositorUpdate`, `support::{win, snapshot}`.

### Step 1 — the failing test

Append to `shell/tests/panel.rs`:

```rust
use std::sync::Arc;

use icedtea_shell::panel::Msg;
use icedtea_shell::taskbar::CompositorUpdate;
use support::{snapshot, win};
use icedtea_contract::WorkspaceInfo;

#[test]
fn a_panel_click_reaches_the_command_surface() {
    let mut panel = Panel::spawn(Theme::Dark);

    // 1. Seed exactly what `shell_gtk.rs` seeded — through the inbox, which is
    //    the path a real `GetState` reply takes.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_until(
        |lines| {
            lines.iter().any(|l| l.starts_with("alloc window_1 "))
                && lines.iter().any(|l| l.starts_with("alloc window_2 "))
        },
        "both window buttons appeared",
    );
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string(), "window_2".to_string()],
        "two window buttons, in model order"
    );

    // 2. Click the button for window 1 and assert it reached `focus_window(1)`.
    let (x, y) = panel.point("window_1");
    panel.click(x, y);
    panel.wait_until(|_| !panel_calls_empty(&panel), "the click reached a command");
    assert!(
        panel.wm_calls().contains(&("focus".to_string(), 1)),
        "expected (\"focus\", 1); calls were {:?}",
        panel.wm_calls()
    );

    // 3. Close window 1 and assert its button is gone.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Closed(1))));
    panel.wait_until(
        |lines| !lines.iter().any(|l| l.starts_with("alloc window_1 ")),
        "window 1's button was dropped",
    );
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_2".to_string()],
        "the closed window's button is gone and the other survives"
    );

    // 4. Click workspace 0 and assert a `workspace` entry.
    let (wx, wy) = panel.point("ws_0");
    panel.click(wx, wy);
    panel.wait_until(
        |_| {
            panel
                .wm_calls()
                .iter()
                .any(|(action, _)| action == "workspace")
        },
        "the workspace click reached a command",
    );
    assert!(
        panel
            .wm_calls()
            .iter()
            .any(|(action, id)| action == "workspace" && *id == 0),
        "expected a (\"workspace\", 0) entry; calls were {:?}",
        panel.wm_calls()
    );
}

/// `wait_until` takes a closure over the report; these two assertions wait on
/// the mocks instead, so they need a predicate of their own.
fn panel_calls_empty(panel: &Panel) -> bool {
    panel.wm_calls().is_empty()
}
```

Add to `support::Panel` the mock-side wait the test needs (this is part of the
task, not a separate one):

```rust
    /// Poll the recorded commands until `want` accepts them.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_for_calls(&self, want: impl Fn(&[(String, u32)]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.wm_calls()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; wm calls were {:?}",
            self.wm_calls()
        );
    }

    /// The `ClipCommands` counterpart.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_for_clip_calls(&self, want: impl Fn(&[(String, u64)]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.clip_calls()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; clip calls were {:?}",
            self.clip_calls()
        );
    }
```

and use them in place of the two `wait_until(|_| ...)` calls and the local
`panel_calls_empty` helper, which is then deleted:

```rust
    panel.wait_for_calls(
        |calls| calls.contains(&("focus".to_string(), 1)),
        "the click reached focus_window(1)",
    );
```

```rust
    panel.wait_for_calls(
        |calls| calls.iter().any(|(action, id)| action == "workspace" && *id == 0),
        "the workspace click reached set_workspace(0)",
    );
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --test panel -- a_panel_click_reaches_the_command_surface
```

Expected: a compile failure first —

```
error[E0599]: no method named `wait_for_calls` found for struct `Panel`
```

Add the two helpers, rerun, and the test must pass. If it does **not** pass on
the first run after the helpers are added, that is a real defect in Tasks 5–12
and not something this task papers over: stop and report.

### Step 3 — implement

There is nothing to implement in `src/`: Tasks 5–12 built everything this test
asserts. Only `shell/tests/support/mod.rs` gains the two `wait_for_*_calls`
helpers above.

This is the point of the task — the parity bar is asserted against code that
was written to a plan, not adjusted until the test agreed with it. If an
assertion fails here, the fix belongs in the task that owns the behaviour.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --test panel
cargo test -p icedtea-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** In `panel::windows`, change
`.on_click(Msg::WindowClicked(id))` to `.on_click(Msg::WindowClicked(id + 1))`:
the test must fail at "the click reached focus_window(1)" with the calls vector
showing `("focus", 2)`. Restore. Then make `TaskbarModel::apply`'s
`CompositorUpdate::Closed` arm a no-op: the test must fail at "window 1's
button was dropped". Restore.

### Step 5 — commit

```
git add shell/tests/panel.rs shell/tests/support/mod.rs
git commit -m "$(cat <<'EOF'
test(shell): re-express the GTK parity test on the toolkit

The same seed snapshot, the same ["One", "Two"], the same ("focus", 1), the
same ["Two"] after a close and the same "workspace" entry as
`tests/shell_gtk.rs` asserted -- now through a real pointer click on a real
layer surface, with the button coordinates read from the panel's own probe
report rather than walked out of a GTK widget tree.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14 — the two the GTK test could not reach

Middle-click-to-close was wired in `taskbar.rs` and covered by nothing; a
signal arriving while the panel is running had no path at all under the GTK
test, which called `render` by hand.

**Files:** `shell/tests/panel.rs`.

### Interfaces

**Consumes:** `Panel::click_button`, `icedtea_ui::window::pointer::BTN_MIDDLE`,
`Msg::Compositor`, `CompositorUpdate::{Opened, WorkspaceList}`.

### Step 1 — the failing test

Append to `shell/tests/panel.rs`:

```rust
#[test]
fn middle_clicking_a_window_button_closes_it() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(7, "Seven")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    let (x, y) = panel.point("window_7");
    panel.click_button(x, y, icedtea_ui::window::pointer::BTN_MIDDLE);
    panel.wait_for_calls(
        |calls| calls.contains(&("close".to_string(), 7)),
        "the middle click reached close_window(7)",
    );
    assert!(
        !panel.wm_calls().contains(&("focus".to_string(), 7)),
        "a middle click must not also focus: {:?}",
        panel.wm_calls()
    );
}

#[test]
fn a_window_opened_signal_through_the_inbox_adds_a_button() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_for("alloc window_1 ");
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string()]
    );

    // What `compositor_client`'s `WindowOpened` arm produces, arriving the way
    // the forward thread delivers it.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Opened(win(
        4, "Four",
    )))));
    panel.wait_for("alloc window_4 ");
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string(), "window_4".to_string()],
        "an inbox signal must add a button without a rebuild of the bar"
    );

    // And the new button works: a click on it reaches the command surface,
    // which is what a clear-and-rebuild would have silently broken by
    // dropping the handler.
    let (x, y) = panel.point("window_4");
    panel.click(x, y);
    panel.wait_for_calls(
        |calls| calls.contains(&("focus".to_string(), 4)),
        "the freshly added button is live",
    );
}
```

### Step 2 — run it and watch it fail

Both should pass immediately against Tasks 7 and 10's code. Prove they are
load-bearing by breaking each first, in this order, and confirming the exact
failure before restoring:

- In `panel::update`, change the middle-click guard to
  `button == BTN_LEFT`. Run
  `cargo test -p icedtea-shell --test panel -- middle_clicking`: it must fail
  with "the middle click reached close_window(7) did not happen". Restore.
- In `panel::windows`, drop `.key(u64::from(id))` from the button. Run
  `cargo test -p icedtea-shell --test panel -- a_window_opened_signal`: the
  reconciler now rebuilds by position, and the final click assertion must fail
  ("the freshly added button is live") because the handler bound to the reused
  node is the old one. Restore.

### Step 3 — implement

Nothing in `src/`. Both behaviours were built in Tasks 7 and 10; this task is
the coverage the GTK test could not provide, and its step 2 is the proof.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --test panel
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Recorded in step 2: two mutations, two named failures, both
restored.

### Step 5 — commit

```
git add shell/tests/panel.rs
git commit -m "$(cat <<'EOF'
test(shell): cover middle-click close and inbox-driven updates

Two behaviours `tests/shell_gtk.rs` could not reach: it called `render` by
hand, so no signal ever arrived while the panel was running, and it never
exercised the GestureClick pinned to button 2. Both now run against the live
panel, and the second asserts the freshly added button is clickable -- the
property keyed reconciliation buys and a clear-and-rebuild would lose.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15 — the clipboard popover gate

Contract §3.6's remaining parity steps (5–7) and the new gate: open the
popover, assert two rows, activate row 0 and assert `("activate", 10)`, replace
the history and assert one row **while it is still open**, then dismiss it with
an outside click and assert the model let go.

**Files:** `ui/src/window/mod.rs`, `shell/src/panel.rs`,
`shell/tests/support/mod.rs`, `shell/tests/panel.rs`.

### Interfaces

**Produces:**

```rust
// ui/src/window/mod.rs
impl Window {
    /// Where the compositor last placed popup `key`, in this window's frame
    /// space. `None` for an unknown key or one that has taken no configure.
    #[must_use]
    pub fn popup_position(&self, key: crate::window::popup::PopupKey) -> Option<(i32, i32)>;

    /// [`Window::probe_points`] for one popup's own tree, in that popup's
    /// surface coordinates.
    #[must_use]
    pub fn popup_probe_points(&self, key: crate::window::popup::PopupKey) -> Vec<ProbePoint>;
}
```

```rust
// shell/src/panel.rs — `report_lines` gains the popup half
pub fn report_lines(
    points: &[icedtea_ui::window::ProbePoint],
    allocation: impl Fn(&str) -> Option<icedtea_ui::layout::Allocation>,
    popup: Option<((i32, i32), Vec<icedtea_ui::window::ProbePoint>)>,
) -> Vec<String>;
```

The third line format, beside M5-D9's two: `popup <label> <x> <y>`, in
**output** coordinates — the popup's position plus the point's own offset — so
a gate clicks it the same way it clicks a bar button.

**Consumes:** `PanelModel::open_popover` (the frame hook reads it to know which
popup to report).

### Step 1 — the failing test

Append to `shell/tests/panel.rs`:

```rust
use icedtea_shell::clipboard::ClipUpdate;
use support::clip_entry;

#[test]
fn the_clipboard_popover_opens_pastes_and_dismisses() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.send(Msg::Clip(Arc::new(ClipUpdate::History(vec![
        clip_entry(10, "copied text"),
        clip_entry(11, "second entry"),
    ]))));
    panel.wait_for("alloc clip ");

    // 5. Open the popover and assert two rows.
    let (cx, cy) = panel.point("clip");
    panel.click(cx, cy);
    panel.wait_until(
        |lines| {
            lines.iter().any(|l| l.starts_with("popup history_open_10 "))
                && lines.iter().any(|l| l.starts_with("popup history_open_11 "))
        },
        "the popover opened with two rows",
    );

    // 6. Activate row 0 and assert it pasted entry 10.
    let (rx, ry) = panel.popup_point("history_open_10");
    panel.click(rx, ry);
    panel.wait_for_clip_calls(
        |calls| calls.contains(&("activate".to_string(), 10)),
        "activating row 0 reached activate(10)",
    );

    // The paste dismissed the popover: no popup lines remain.
    panel.wait_until(
        |lines| !lines.iter().any(|l| l.starts_with("popup ")),
        "the popover closed after a paste",
    );

    // 7. Reopen, replace the history underneath it, and assert one row —
    //    the open surface tracks the model (contract §6 P5-D2).
    panel.click(cx, cy);
    panel.wait_until(
        |lines| lines.iter().any(|l| l.starts_with("popup history_open_10 ")),
        "the popover reopened",
    );
    panel.send(Msg::Clip(Arc::new(ClipUpdate::History(vec![clip_entry(
        12, "only",
    )]))));
    panel.wait_until(
        |lines| {
            lines.iter().any(|l| l.starts_with("popup history_open_12 "))
                && !lines.iter().any(|l| l.starts_with("popup history_open_10 "))
        },
        "the open popover followed the history update",
    );

    // 8. An outside click dismisses it, and the model lets go: the `clip`
    //    button drops its `active` class, which is the model's open state
    //    made visible.
    let (ox, oy) = (panel.output().0 as i32 / 2, panel.output().1 as i32 - 20);
    panel.click(ox, oy);
    panel.wait_until(
        |lines| !lines.iter().any(|l| l.starts_with("popup ")),
        "an outside click dismissed the popover",
    );
    assert!(
        !panel.clip_calls().iter().any(|(a, _)| a == "clear"),
        "dismissing must not have pressed anything: {:?}",
        panel.clip_calls()
    );
}
```

and to `support::Panel`:

```rust
    /// The output-space centre of a probe point inside the open popover.
    ///
    /// # Panics
    ///
    /// If the popover exposes no such point.
    #[must_use]
    pub fn popup_point(&self, label: &str) -> (i32, i32) {
        let line = self.wait_for(&format!("popup {label} "));
        let fields: Vec<&str> = line.split_whitespace().collect();
        (
            fields[2].parse().expect("popup x"),
            fields[3].parse().expect("popup y"),
        )
    }
```

### Step 2 — run it and watch it fail

```
cargo test -p icedtea-shell --test panel -- the_clipboard_popover
```

Expected: a compile failure (`no method named popup_point`), then, once the
helper is added, a failure at "the popover opened with two rows did not happen
within 30s; report holds [...]" — the report has no `popup ` lines at all.

### Step 3 — implement

**`ui/src/window/mod.rs`.** Extract `Window::probe_points`'s body into a
private free function and add the two popup accessors:

```rust
/// M5-D9's walk, over any surface's retained tree.
///
/// Shared by [`Window::probe_points`] and [`Window::popup_probe_points`] so
/// the labelling rule — id, else CSS node name, indexed when the name repeats,
/// centre of the border box floored — exists once.
fn probe_points_of(root: &Node, layout: &crate::layout::LayoutTree) -> Vec<ProbePoint> {
    /* the exact body `Window::probe_points` shipped in P0, with `self.root`
       and `self.layout` replaced by the two parameters */
}

impl Window {
    /// Where the compositor last placed popup `key`, in this window's frame
    /// space. `None` for an unknown key or one that has taken no configure.
    #[must_use]
    pub fn popup_position(&self, key: PopupKey) -> Option<(i32, i32)> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| p.popup().map(super::popup::Popup::position))
    }

    /// [`Window::probe_points`] for one popup's own tree, in that popup's
    /// surface coordinates.
    #[must_use]
    pub fn popup_probe_points(&self, key: PopupKey) -> Vec<ProbePoint> {
        self.popups
            .iter()
            .find(|p| p.key == key)
            .map_or_else(Vec::new, |p| probe_points_of(&p.root, &p.layout))
    }
}
```

`Window::probe_points` becomes `probe_points_of(&self.root, &self.layout)` —
its own test in `ui/tests/window_events.rs`
(`probe_points_locate_a_live_windows_widgets`) is the guard that the extraction
changed nothing.

**`shell/src/panel.rs`.** Widen `report_lines` and teach the frame hook to feed
it. `report_lines` gains a third parameter and one loop:

```rust
    if let Some((origin, points)) = popup {
        for p in points {
            lines.push(format!("popup {} {} {}", p.label, origin.0 + p.x, origin.1 + p.y));
        }
    }
```

with the doc comment gaining:

```
/// `popup <label> <x> <y>` for the open popover's own tree, in **output**
/// coordinates — the compositor-assigned popup position plus the point's own
/// offset — so a gate clicks a popover row exactly as it clicks a bar button.
/// Absent entirely when no popover is open, which is how a gate asserts a
/// dismissal.
```

`frame_hook` gains the key it must ask about. It cannot read `PanelModel`
(the hook outlives the borrow), so it takes the same shared cell treatment the
anchor rect gets:

```rust
pub fn frame_hook(
    clip_rect: Rc<Cell<Option<Rect>>>,
    open_popover: Rc<Cell<Option<PopupKey>>>,
    report: Option<std::path::PathBuf>,
) -> impl FnMut(&Window) + 'static {
    let mut last: Vec<String> = Vec::new();
    move |w: &Window| {
        clip_rect.set(w.allocation("clip").map(|a| a.border_box));
        let Some(path) = report.as_ref() else {
            return;
        };
        let popup = open_popover.get().and_then(|key| {
            w.popup_position(key)
                .map(|origin| (origin, w.popup_probe_points(key)))
        });
        let lines = report_lines(&w.probe_points(), |id| w.allocation(id), popup);
        if lines == last {
            return;
        }
        last.clone_from(&lines);
        if let Err(err) = std::fs::write(path, lines.join("\n") + "\n") {
            tracing::warn!(%err, "cannot write $ICEDTEA_PROBE_REPORT");
        }
    }
}
```

`PanelModel` gains the cell beside `clip_rect`, and `update` keeps it in step
with `open_popover` — one field, two readers, never two truths:

```rust
    /// `open_popover`, published for the `App::on_frame` hook, which runs
    /// outside any borrow of the model. `update` writes both together and
    /// nothing else writes either.
    pub open_popover_cell: Rc<Cell<Option<PopupKey>>>,
```

```rust
        Msg::PopoverOpened(key) => {
            m.open_popover = Some(key);
            m.open_popover_cell.set(Some(key));
            Cmd::None
        }
        Msg::PopoverDismissed(key) => {
            if m.open_popover == Some(key) {
                m.open_popover = None;
                m.open_popover_cell.set(None);
            }
            Cmd::None
        }
```

Add one unit test to `panel.rs`'s `mod tests` pinning that invariant:

```rust
    #[test]
    fn the_published_popover_key_never_disagrees_with_the_model() {
        let (mut m, _, _) = panel();
        let key = PopupKey::from_raw(3);
        let _ = update(&mut m, Msg::PopoverOpened(key));
        assert_eq!(m.open_popover_cell.get(), m.open_popover);
        let _ = update(&mut m, Msg::PopoverDismissed(PopupKey::from_raw(9)));
        assert_eq!(m.open_popover_cell.get(), m.open_popover);
        let _ = update(&mut m, Msg::PopoverDismissed(key));
        assert_eq!(m.open_popover_cell.get(), m.open_popover);
        assert_eq!(m.open_popover_cell.get(), None);
    }
```

and update `report_lines`' two existing unit tests to pass `None` as the third
argument.

**`shell/src/main.rs` and `shell/tests/support/mod.rs`** both change their
`frame_hook` call to pass `model.open_popover_cell.clone()` as the second
argument.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --test panel
cargo test -p icedtea-shell
cargo test -p icedtea-ui --test window_events
cargo test -p icedtea-ui --test popup_input
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

**Mutation check.** Three, each with a named failure:

- In `panel::update`'s `Msg::ClipActivated`, drop the `Cmd::ClosePopup`:
  "the popover closed after a paste" must fail.
- In `ui/src/view/app.rs`, revert `drain`'s `rebuild_popups` call (Task 3):
  "the open popover followed the history update" must fail — the open surface
  keeps showing entries 10 and 11.
- In `ui/src/view/app.rs`, remove the `PopupEvent::Dismissed` block from
  `run`'s event loop (Task 1): "an outside click dismissed the popover" must
  fail, because the model never lets go and the hook keeps reporting the popup.

Restore all three.

### Step 5 — commit

```
git add ui/src/window/mod.rs shell/src/panel.rs shell/src/main.rs \
        shell/tests/support/mod.rs shell/tests/panel.rs
git commit -m "$(cat <<'EOF'
test(shell): gate the clipboard popover end to end

Open the popover with a real click, assert two rows, activate row 0 and see
`("activate", 10)` on the mock, replace the history underneath the open
surface and see it follow, then dismiss it with an outside click and see the
model let go. Row coordinates come from the popover's own probe points --
`Window::popup_probe_points`/`popup_position`, M5 contract amendment P5-D8 --
because M5-D9's probe walks the window's tree and a popup is a second surface
the compositor may have slid.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16 — the rest-state gate

Contract §3.6: `panel_paints_every_probe_point_at_rest`, light/dark/hc. Every
probe point the panel exposes must paint something; there are no
`KNOWN_BLANK` exemptions for app widgets.

**Files:** `shell/tests/panel.rs`.

### Interfaces

**Consumes:** `Panel::{capture, background, allocation, report}`,
`support::paints_something`, `icedtea_ui::gallery::Theme`.

### Step 1 — the failing test

Append to `shell/tests/panel.rs`:

```rust
/// Every widget the panel puts on screen paints something at rest.
///
/// The gallery gate's rule, applied to an app: sample each reported
/// allocation and require at least one pixel that is not the wallpaper. No
/// exemption list — an app widget that renders nothing is a bug, not a known
/// gap (spec §7).
fn panel_paints_every_probe_point_at_rest(theme: Theme) {
    let mut panel = Panel::spawn(theme);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![
                WorkspaceInfo {
                    id: 0,
                    name: String::new(),
                },
                WorkspaceInfo {
                    id: 1,
                    name: "web".into(),
                },
            ],
        ),
    ))));
    for id in ["bar", "workspaces", "windows", "clip", "ws_0", "ws_1", "window_1", "window_2"] {
        panel.wait_for(&format!("alloc {id} "));
    }

    let background = panel.background();
    let frame = panel.capture();
    let mut blank: Vec<String> = Vec::new();
    for line in panel.report() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 6 || f[0] != "alloc" {
            continue;
        }
        let rect = (
            f[2].parse().unwrap_or(0),
            f[3].parse().unwrap_or(0),
            f[4].parse().unwrap_or(0),
            f[5].parse().unwrap_or(0),
        );
        if !support::paints_something(&frame, rect, background) {
            blank.push(format!("{} at {rect:?}", f[1]));
        }
    }
    assert!(
        blank.is_empty(),
        "{}: these widgets painted nothing at rest: {blank:?}",
        theme.name()
    );
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_light_theme() {
    panel_paints_every_probe_point_at_rest(Theme::Light);
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_dark_theme() {
    panel_paints_every_probe_point_at_rest(Theme::Dark);
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    panel_paints_every_probe_point_at_rest(Theme::HighContrast);
}

/// The focused and attention states are visible, not just set.
///
/// Finding F10: `taskbar::render` added both classes and nothing styled
/// either, so the compositor's focus and attention bits were invisible on the
/// bar. `style.css`'s left-accent gradients are what fixed that, and this is
/// what keeps them fixed — a CSS engine that silently dropped
/// `background-image: linear-gradient` on a button would pass every other test
/// in this file.
#[test]
fn a_focused_window_button_looks_different_from_an_unfocused_one() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_for("alloc window_2 ");

    let sample = |panel: &mut Panel, id: &str| -> (u8, u8, u8) {
        let (x, y, _, h) = panel.allocation(id);
        let frame = panel.capture();
        // Two pixels in from the left edge: the accent stripe is 3px wide.
        support::pixel(&frame, (x + 1) as u32, (y + h / 2) as u32).expect("inside the frame")
    };
    let before = sample(&mut panel, "window_1");

    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Updated {
        id: 1,
        update: icedtea_contract::WindowUpdate {
            focused: Some(true),
            ..Default::default()
        },
    })));
    // The class change does not move anything, so wait on the pixel.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut after = before;
    while std::time::Instant::now() < deadline && support::same(after, before) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        after = sample(&mut panel, "window_1");
    }
    assert!(
        !support::same(after, before),
        "the `focused` class must change what the button paints: {before:?} -> {after:?}"
    );
}
```

### Step 2 — run it and watch it fail

Run each first with the panel sheet deliberately broken, to prove the gate
bites. Comment out the `#bar` rule in `shell/style.css` and run:

```
cargo test -p icedtea-shell --test panel -- panel_paints_every_probe_point
```

Expected: all three theme tests fail listing `bar`, `workspaces`, `windows` and
the buttons as blank — with no background of its own the bar is wallpaper.
Restore `style.css` and rerun: all three must pass.

Then comment out the `#bar button.focused` rule and run:

```
cargo test -p icedtea-shell --test panel -- a_focused_window_button
```

Expected: it fails with "the `focused` class must change what the button
paints". Restore `style.css`.

### Step 3 — implement

Nothing in `src/`. The gate is the deliverable; step 2 is its proof.

### Step 4 — run it and watch it pass

```
cargo test -p icedtea-shell --test panel
cargo test -p icedtea-shell
cargo clippy -p icedtea-shell --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: the `panel` test binary reports every test in Tasks 12–16 passing.

**Mutation check.** Recorded in step 2: two mutations of `shell/style.css`,
two named failures, both restored.

### Step 5 — commit

```
git add shell/tests/panel.rs
git commit -m "$(cat <<'EOF'
test(shell): gate the panel's rest state in all three themes

Every allocation the panel reports must paint at least one non-wallpaper
pixel, in light, dark and high contrast, with no exemption list -- the
gallery gate's rule applied to an app. A fourth test pins finding F10's
left-accent gradient: the `focused` class must change what a window button
paints, which is the whole reason that rule exists.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17 — delete the GTK test, audit the tree, record the amendments

P5's own close-out: `shell_gtk.rs` goes for good now that everything it
asserted is asserted elsewhere, the dependency audit runs against P5's gate
(contract §5), and the eight deviations at the top of this plan are appended to
the contract's §6.

**Files:** `shell/tests/shell_gtk.rs` (deleted),
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md`.

### Interfaces

**Produces:** the contract's §6 entries `P5-D1` … `P5-D8`.

**Consumes:** everything Tasks 1–16 built.

### Step 1 — the failing test

The "test" here is the audit itself, and it must be run before the deletion so
its output is about the shipped crate rather than about a file that is on its
way out. Run:

```
cargo tree -p icedtea-shell -e normal -i gtk4
cargo tree -p icedtea-shell -e normal -i gtk4-layer-shell
cargo tree -p icedtea-shell -e normal -i glib
cargo tree -p icedtea-shell -e normal -i gio
cargo tree -p icedtea-shell -e normal -i gdk4
cargo tree -p icedtea-shell -e normal -i pango
cargo tree -p icedtea-shell -e normal -i cairo-rs
grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' shell/src shell/tests
```

Expected **before** the deletion: every `cargo tree -i` prints
`error: package ID specification ... did not match any packages`, and the
`grep` prints the one surviving hit — `shell/tests/shell_gtk.rs`'s neutralised
comment, which names `panel.rs`, not `gtk4`. If the grep prints anything else,
stop and report: a GTK reference survived a task that claimed to remove it.

### Step 2 — delete and re-audit

```
git rm shell/tests/shell_gtk.rs
grep -rn 'gtk4\|gtk4-layer-shell\|glib\|gio\|gdk\|pango\|cairo' shell/src shell/tests shell/Cargo.toml
```

Expected: **no output at all**.

Then the workspace-level check contract §4.1 will run at P6, exercised early
here so P5 does not hand P6 a surprise:

```
cargo tree -e normal --workspace | grep -Ei '\b(gtk4|gtk4-layer-shell|glib|gio|gdk|pango|cairo)\b'
```

Expected: at P5 this may still print `icedtea-settings`' rows if P1–P4 left
any; it must print **no row whose path runs through `icedtea-shell`**. Record
what it prints in the commit message.

### Step 3 — record the amendments

Append to `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §6, in this
order and in the M3 §10 shape (**Carried out by / Contract says / As shipped /
Ruling**):

```markdown
### P5-D1 — input is routed to the surface it arrived on; P5 edits `ui/src/view/app.rs`

**Carried out by:** P5 (`ui/src/view/app.rs`, `ui/tests/popup_input.rs`).

**Contract says** (§5, P5's "Must not touch"): `ui/`.

**As shipped:** `route` tracks `pointer_target`/`keyboard_target` from
`InputEvent`'s two `Enter` variants and swaps the addressed popup's retained
tree into the runtime for the dispatch.

**Ruling.** §3.4 makes the clipboard popover a real popup surface — it must be
one, the panel's layer surface being 28 px tall — and §3.6's gate requires a
click inside it to reach `MockClip::activate`. As shipped, `route` hit-tested
the window's tree for every event and ignored `SurfaceTarget` entirely, so that
click reached nothing. An enumerated exception in the same shape as P4's
`drop_down.rs` exception (§7), additive, with its own test file. Not a
precedent for further app-part edits under `ui/`.

### P5-D2 — a popup surface keeps its view closure and is re-reconciled every fold
### P5-D3 — the popover's history rows are buttons in a box, not a `ListBox`
### P5-D4 — `App::on_popup`/`PopupEvent`; `PopupKey::raw`/`from_raw`
### P5-D5 — `App::on_frame`
### P5-D6 — `auto_exclusive_zone_enable()` becomes a literal `exclusive_zone: 28`
### P5-D7 — `shell/Cargo.toml` also gains `wayland-protocols-wlr`
### P5-D8 — `Window::popup_position` and `Window::popup_probe_points`
```

each expanded from the corresponding bullet in this plan's "Contract
deviations" section — the bullets are already written in the required shape and
are copied, not paraphrased. If Task 4's opening check found `App::on_frame`
already landed by P1, P5-D5's entry says so and names P1 as the carrier instead.

Two further §6 records, from this plan's "not deviations" notes:

```markdown
### P5-D9 — `CompositorUpdate` and `ClipUpdate` derive `Clone`

**Ruling.** §3.1's own `update` body is `Arc::unwrap_or_clone(u)`, which
requires it. Every payload already derives `Clone` in `icedtea-contract`; no
signature changed and the six model tests moved unmodified.

### P5-D10 — two `ListBoxC` defects recorded, not fixed

**Ruling.** M5 is not the milestone that fixes them, and P5 routes around them
(P5-D3). Recorded here so M6 has them written down: (a) `ListBoxC::on_event`
sets `cx.handled` on `PointerDown` with no phase guard, and P4-D19 makes a
handled capture end the whole dispatch, so a control inside a row never
receives its own click; (b) `row_at` is fed `Event::PointerDown::local`, which
P5-D33's `aim` computes relative to the innermost instance, so the resolved
row index is wrong for every row but the first.
```

### Step 4 — the full gate

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo doc --workspace --no-deps
cargo test -p icedtea-ui
cargo test -p icedtea-shell
```

and, named because they are P5's inherited gates:

- M1/M2: `themed_button_offscreen`, `adwaita_coverage`,
  `gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`.
- M3: `window_events`, `counter_app`, `node_trees`, `widget_pixels`,
  `reconcile_props`, `gallery_gate` (light/dark/hc), `interaction_gate` — still
  16/16.
- P5's own: `ui/tests/popup_input.rs` (4), `ui/tests/app_frame_hook.rs` (1),
  `shell/tests/panel.rs` (the smoke test, the parity test, the two Task 14
  tests, the popover gate, three rest-state gates and the focus-accent test),
  `shell` unit tests (taskbar 6, clipboard 2, compositor_client 1, style 1,
  panel's own), `shell/tests/live_dbus.rs` — **untouched**, still skipping
  visibly when the session bus is unavailable or the name is owned.

Run `shell/tests/live_dbus.rs` explicitly and read its output:

```
cargo test -p icedtea-shell --test live_dbus -- --nocapture
```

Expected: either the two `Command` assertions pass against a real
`icedtea-clipboard` service, or the visible `SKIP:` line. A silent pass is not
acceptable evidence.

### Step 5 — commit

```
git add docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git rm shell/tests/shell_gtk.rs
git commit -m "$(cat <<'EOF'
chore(shell): delete the GTK test and record P5's amendments

`tests/shell_gtk.rs` goes: `tests/panel.rs` asserts everything it did, plus
middle-click close, inbox-driven updates, the popover end to end and the
rest state in three themes. `cargo tree -e normal -i` finds no gtk4,
gtk4-layer-shell, glib, gio, gdk, pango or cairo in icedtea-shell, and no
source file names one.

The M5 contract's §6 gains P5-D1 … P5-D10.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### Spec and contract coverage

| Requirement | Source | Task | Evidence |
|---|---|---|---|
| One `App<PanelModel, Msg>` on one `Role::Layer` surface | spec §6.1, contract §3.2 | 5 | `panel::spec()`, `main::run`; `the_panel_opens_a_layer_surface_and_reports_its_geometry` |
| `Layer::Top`, anchor `Left\|Right\|Top`, margins 0, `keyboard: None` | contract §3.2 | 5 | `panel::spec()`; the three rulings quoted in its doc comment |
| `exclusive_zone` a literal `BAR_HEIGHT` = 28 | contract §3.2 ruling 2, §4.3(6) | 5 | P5-D6; smoke test asserts the committed height |
| `PanelModel` wraps both models verbatim | contract §3.1 | 5 | `PanelModel`; taskbar's 5 and clipboard's 1 test unmodified |
| `Msg` is `Send`, `Arc` not `Rc` | contract §3.1, §5 | 5 | the `assert_send::<Msg>()` static assertion |
| `Compositor(u)` → `taskbar.apply(Arc::unwrap_or_clone(u))` | contract §3.1 | 5 | `a_compositor_snapshot_reaches_the_taskbar_model` |
| `Clip(u)` → `clipboard.apply(..)` | contract §3.1 | 5 | `a_history_update_reaches_both_the_model_and_the_popover_mirror` |
| `WorkspaceClicked`/`WindowClicked` → `Cmd::Task` on the traits | contract §3.1 | 6 | `clicking_a_window_button_reaches_focus_window_on_the_command_surface`, `clicking_a_workspace_button_reaches_set_workspace` |
| `WindowPointerUp` acts only on `BTN_MIDDLE` | contract §3.1 | 7 | `middle_clicking_a_window_button_closes_it`, `a_left_release_on_a_window_button_closes_nothing` |
| `ClipButtonClicked` opens or closes | contract §3.1 | 8 | `the_clip_button_opens_a_popup_anchored_to_its_own_box`, `a_second_click_on_the_clip_button_closes_the_open_popover` |
| `PopoverDismissed` clears only its own key | contract §3.1 | 8 | `a_dismissal_clears_the_state_only_for_the_key_that_was_dismissed` |
| `ClipActivated` also closes the popover | contract §3.1 | 9 | `activating_a_row_pastes_it_and_closes_the_popover` |
| `ClipPinToggled`/`ClipRemoved`/`ClipCleared` → `Cmd::Task` | contract §3.1 | 9 | `pinning_and_removing_a_row_reach_the_clip_command_surface` |
| `view` = `[workspaces][windows][clip]`, `#bar` | contract §3.3 | 6 | `the_bar_holds_workspaces_windows_and_the_clip_button` |
| Workspace label: name else `id + 1` | contract §3.3 | 6 | `a_workspace_button_is_labelled_by_name_then_one_indexed_id` |
| Window label: title else `app_id` | contract §3.3 | 6 | `a_window_button_is_labelled_by_title_then_app_id` |
| `active`/`focused`/`attention` classes | contract §3.3 | 6 | `the_active_workspace_and_the_focused_window_carry_their_classes` |
| Keys: workspace id, window id, entry id | contract §3.3, spec D9 | 6, 9 | `a_window_opened_signal_through_the_inbox_adds_a_button` (Task 14, key mutation) |
| `hexpand` on `#windows` | contract §3.3 | 6 | `windows`' doc comment; the `.hexpand(true)` call |
| `clip` carries `active` while open | contract §3.3 | 8 | `the_open_popover_key_comes_back_through_a_message_and_marks_the_button` |
| Popover is a real popup anchored to the button | contract §3.4 | 8 | `the_clip_button_opens_a_popup_anchored_to_its_own_box` |
| `open_popover` is the single source of truth | contract §3.4 | 8, 15 | `the_published_popover_key_never_disagrees_with_the_model` |
| Outside click → `PopupDone` → `PopoverDismissed` | contract §3.4 | 1, 15 | `an_app_learns_the_key...`; "an outside click dismissed the popover" |
| Both D-Bus clients kept as-is; channel output is the inbox | contract §3.5 | 10 | `main::forward`; no edit to either client file |
| `bridge.rs` deleted | contract §3.5 | 5 | `git rm shell/src/bridge.rs` |
| `process::exit(1)` survives verbatim | contract §3.5, §5 | 10 | `the_fatal_worker_path_still_exits` |
| `shell/src/lib.rs` loses `pub use gtk4` | contract §3.5 | 5 | the rewritten `lib.rs` |
| `style.css` → `CompiledSheet` overlay | contract §3.5 | 5 | `style::sheet_for`; `the_panel_sheet_keeps_the_focus_and_attention_gradients` |
| The `.focused`/`.attention` gradients resolve | contract §3.5 | 5, 16 | the sheet test and `a_focused_window_button_looks_different_from_an_unfocused_one` |
| `Cargo.toml`: gtk4 + gtk4-layer-shell out, icedtea-ui + crossbeam-channel in | contract §3.5 | 5, 17 | `cargo tree -e normal -i` audits |
| Parity: 7 assertions, same mocks, same `(action, id)` | contract §3.6 | 13, 15 | `a_panel_click_reaches_the_command_surface`, `the_clipboard_popover_opens_pastes_and_dismisses` |
| `middle_clicking_a_window_button_closes_it` | contract §3.6 | 14 | named test |
| `a_window_opened_signal_through_the_inbox_adds_a_button` | contract §3.6 | 14 | named test |
| `panel_paints_every_probe_point_at_rest`, light/dark/hc | contract §3.6, spec §7 | 16 | three named tests |
| `the_clipboard_popover_opens_pastes_and_dismisses` | contract §3.6 | 15 | named test |
| Kept verbatim: taskbar 5, clipboard 1, `version_mismatch`, `live_dbus` | contract §3.6 | 5, 17 | unmodified files; the Task 17 run of `live_dbus` |
| Deleted: `shell_gtk.rs`, `bridge.rs`, `taskbar::render`, `clipboard::{render, connect_activation}` | contract §3.6, §4.2 | 5, 17 | the two `git rm`s and the strip in Task 5 |
| Widget ids normative per panel | contract §1 M5-D9 | 6, 9 | the two "Widget ids, normative" lists |
| `$ICEDTEA_PROBE_REPORT` writer | contract §1 M5-D9 | 11, 12, 15 | `panel::report_lines`, `panel::frame_hook`, their unit tests |
| Consumes M5-D1/D2/D3/D4/D5/D9/D10 | contract §5 | 5, 7, 10, 11 | `with_inbox`, `Cmd::Task`, boxed closures, `on_pointer_up_with_button`, `BTN_MIDDLE`, `probe_points`/`allocation` |
| Untouched: `settings/`, `clipboard/`, the two client bodies, `live_dbus.rs`, the systemd unit | contract §5 | — | no task names them; Task 17's grep is the check |
| Every load-bearing test records a mutation check | contract §5 | 1–16 | each task's "Mutation check" block |
| Timing assertions are complexity bounds | contract §5 | 12 | `TIMEOUT`/`POLL` and their doc comments |
| Untrusted input never panics | contract §5 | 5, 11 | `theme_from_env`'s fallback; `report_lines`' `None` allocation path; the deserialize-and-drop behaviour of both clients, untouched |
| No M6 scope | contract §5, spec §11 | — | no IME, no drag-and-drop, no `accesskit`, no emoji, no portal, no `StackSidebar`, no `sh -c` |

Two contract items P5 deliberately does **not** implement, both recorded above
rather than silently dropped: §3.4's `list_box`/`on_item_activated` spelling
(P5-D3) and its Delete-key remove (which a `KeyboardInteractivity::None`
surface can never receive — the remove button replaces it, and `Msg::ClipRemoved`
keeps its contract shape).

### Placeholder scan

Checked, and clean:

- **No `TODO`, `FIXME`, `TBD`, `XXX`, `unimplemented!`, `todo!` or "later"** in
  any code block in this plan.
- **No "similar to Task N"**: every task's code is written out. The two places
  that could have been elisions are called out instead — Task 12 Step 3a moves
  `spec` and `report_lines` "byte for byte", naming the source, and Task 15's
  `probe_points_of` says explicitly that its body is "the exact body
  `Window::probe_points` shipped in P0, with `self.root` and `self.layout`
  replaced by the two parameters".
- **No undefined types.** Every type named in a code block is either defined in
  this plan (`PanelModel`, `Msg`, `Offline`, `MockWm`, `MockClip`, `TempDir`,
  `Panel`, `PopupEvent`), imported from a crate with its path spelled
  (`icedtea_contract::{ClipEntry, ClipKind, Snapshot, WindowId, WindowInfo,
  WindowUpdate, WorkspaceInfo, Rectangle}`, `icedtea_harness::{Compositor,
  ScreencopyClient, VirtualPointerClient, CapturedFrame}`,
  `icedtea_ui::{gallery::Theme, layout::{Align, Allocation, Rect},
  text::FontDatabase, view::{App, Cmd, View, Inbox, InboxSender, EventKind,
  PropName, Prop, ScriptStep}, widgets::Orientation, window::{Window,
  SurfaceSpec, Role, LayerSpec, ProbePoint, InputEvent, SurfaceTarget,
  pointer::{BTN_LEFT, BTN_MIDDLE}, popup::{PopupKey, PopupAnchorPoint,
  Positioner}}, css::cascade::CompiledSheet, css::parse::parse_stylesheet_with_base,
  shm::pixel_rgb}`), or carried forward from `shell/`'s own untouched modules
  (`TaskbarModel`, `CompositorUpdate`, `ClipboardModel`, `ClipUpdate`,
  `CompositorCommands`, `ClipCommands`, `CompositorProxy`, `ClipProxy`).
- **No prose-only code step.** Every "implement" step is source, not
  description; every "run it and fail" step names the expected message.
- **Every task commits**, with the trailer, and no task batches two commits.

### Type consistency against the contract

| Contract §3 name | Contract type | This plan | Same? |
|---|---|---|---|
| `PanelModel.taskbar` | `crate::taskbar::TaskbarModel` | same | yes |
| `PanelModel.clipboard` | `crate::clipboard::ClipboardModel` | same | yes |
| `PanelModel.open_popover` | `Option<icedtea_ui::window::PopupKey>` | `Option<PopupKey>`, imported from `icedtea_ui::window::popup` | yes — `window::PopupKey` is that re-export |
| `PanelModel.wm` | `Rc<dyn CompositorCommands>` | same | yes |
| `PanelModel.clip` | `Rc<dyn ClipCommands>` | same | yes |
| `PanelModel.bar_height` | `i32` | same | yes |
| `Msg::Compositor` | `Arc<crate::taskbar::CompositorUpdate>` | same | yes |
| `Msg::Clip` | `Arc<crate::clipboard::ClipUpdate>` | same | yes |
| `Msg::WorkspaceClicked` / `WindowClicked` | `u32` | same | yes |
| `Msg::WindowPointerUp` | `{ id: u32, button: u32 }` | same | yes |
| `Msg::PopoverOpened` / `PopoverDismissed` | `PopupKey` | same | yes |
| `Msg::ClipActivated` / `ClipRemoved` | `u64` | same | yes |
| `Msg::ClipPinToggled` | `{ id: u64, pinned: bool }` | same | yes |
| `Msg::ClipCleared` / `ClipButtonClicked` | unit | same | yes |
| `update` | `impl FnMut(&mut PanelModel, Msg) -> Cmd<Msg>` | `fn update(&mut PanelModel, Msg) -> Cmd<Msg>` | yes — an `fn` item coerces (M5-D4) |
| `view` | `pub fn view(m: &PanelModel) -> View<Msg>` | same | yes |
| `workspaces` / `windows` / `clip_button` | `fn (&PanelModel) -> View<Msg>` | same, private | yes |
| `popover_body` | `pub fn popover_body(m: &PanelModel) -> View<Msg>` | same, plus `popover_rows(&[ClipEntry])` | yes — additive |
| `style::sheet` | `pub fn sheet() -> CompiledSheet` | same, plus `sheet_for(Theme)` and `theme_from_env()` | yes — additive |
| `forward` | `fn forward<T: Send + 'static>(async_channel::Receiver<T>, InboxSender<Msg>, impl Fn(T) -> Msg + Send + 'static) -> JoinHandle<()>` | same | yes |
| `SurfaceSpec` fields | `role`, `size`, `title`, `app_id` | same, with `title: "icedtea-shell"`, `app_id: "org.icedtea.Shell"` | yes |
| `LayerSpec` fields | `layer`, `anchor`, `margin`, `exclusive_zone`, `keyboard` | same values as contract §3.2 | yes |
| `BAR_HEIGHT` | `const BAR_HEIGHT: i32 = 28` | `pub const`, in `panel.rs` | yes — `pub` so `main.rs` and the gates can name it |
| `Cmd::OpenPopup` payload | `{ anchor, positioner, view }` | same, with `anchor: PopupAnchorPoint::Rect` | **differs from §3.4's `PopupAnchorPoint::Node`** — see P5-D8's neighbour note below |

The last row is the one type-level divergence from a contract snippet, and it
is not a signature change: `Cmd::OpenPopup`'s `anchor` field keeps its
`PopupAnchorPoint` type and the `Rect` variant is one the enum already ships.
§3.4 writes `PopupAnchorPoint::Node(/* the clip button's node */)` with the
node elided precisely because there is no way for a handler or `update` to
obtain one — no `Msg` carries a `Node`, and `update` has no `&Window`. The
`Rect` variant with the box `App::on_frame` publishes is the same anchor,
resolved one frame earlier; `open_popup` itself resolves a `Node` anchor to
`layout.allocation(node).border_box`, which is exactly the rect being passed.
Recorded in Task 17 as part of P5-D5's rationale rather than as a ninth
deviation, because the contract's own text elides the node it cannot supply.
