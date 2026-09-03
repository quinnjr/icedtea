# M5 Part 0 — Shared Interface Contract

**Date:** 2026-09-03
**Spec:** `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
**Previous contract:** `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` — its §3
public interfaces are what M5 extends; its §10 deviations (P3-A … P8-D75) and §11
execution notes (E1–E21) are **still binding** except where a `M5-D*` amendment
below names one and supersedes it.
**Branch:** `rebuild/pure-rust-gtk-m5` (off `develop` @ `d9cee52` = M3 + wlr 0.20.29)
**Crates touched:** `ui/` (`icedtea-ui`), `settings/` (`icedtea-settings`),
`shell/` (`icedtea-shell`). `clipboard/`, `compositor/`, `harness/`, `contract/`,
`config/` and the `wlr` crate are **not** touched by M5.
**Research notes:** `.superpowers/m5-plan-notes/{ui-consumer-api,settings-crate,shell-crate,ipc-portal-loop,compositor-contract}.md`

This is the frozen interface seven independently-authored part-plans code against.
Every signature below is normative: a part may add private items freely, but may
not change a signature here without an amendment recorded in §6.

**Rules carried forward and still binding.** The spec's eleven inherited
decisions (§2 items 1–11) apply to every part. In particular: no
`gtk4`/`gtk4-layer-shell`/`glib`/`gio`/`gdk`/`pango`/`cairo` in a migrated app;
each app reuses its GTK-free pure core unchanged; the toolkit is Elm-shaped and
non-reentrant; one `Window` and one `App` per surface; `wayland-client` +
`wayland-protocols(-wlr)` directly with a hand-rolled poll loop; theming is
GTK4-theme-file compatible and an app sheet layers as a cascade origin; cursor
shape via `wp_cursor_shape_v1`; M6 scope (IME/`text-input-v3`, drag-and-drop,
`accesskit`, emoji, portals beyond `FileChooser`) is out of bounds even
opportunistically; compositor/config/contract/harness are out of scope; D-Bus
**method** names are PascalCase on the wire and **signal** names are not
(`GetHistory` vs `history_changed` — `shell/src/clip_client.rs:37,41`); and
`settings/tests/live_apply.rs` keeps spawning the **real** `icedtea-compositor`
binary, never `icedtea_harness::Compositor`.

From the M3 contract, three rulings M5 parts hit immediately and must not
re-litigate: **P4-D19** (`cx.handled` ends the whole dispatch, not its phase),
**P5-D33** (hit-testing resolves to the innermost `Instance`, so a controller's
own subnode chrome receives clicks), and **P8-D75** (the 15 universal
`PropName`s are applied centrally by the reconciler; a controller's `set_prop`
handles only its own props).

---

## 0. Module map (owner in brackets)

```
ui/src/
  window/mod.rs        [P0]  watch_fd/unwatch/WatchId/Interest, N-fd wait_bounded,
                             InputEvent::FdReady, probe_points/allocation
  window/keyboard.rs   [P0]  Keymap::base_keysym, KeyEvent::base
  window/pointer.rs    [P0]  BTN_LEFT/BTN_MIDDLE/BTN_RIGHT constants
  view/mod.rs          [P0]  EventKind::{PointerDown,PointerMotion,PointerUp},
                             Handler::PairButton, fire_pair_button, Prop::Draw widening
  view/builders.rs     [P0]  on_pointer_down/motion/up (+ _with_button)
  view/cmd.rs          [P0]  Cmd::Task
  view/app.rs          [P0]  App::new closures, with_inbox, on_fd, Inbox/InboxSender,
                             inbox drain in `run`
  view/inbox.rs        [P0]  new file: Inbox, InboxSender, WatchId re-export
  widgets/color_dialog.rs  [P0]  ColorDialogButtonC::measure, ColorDialogC::measure/paint
  widgets/check_button.rs  [P0]  CheckButtonC::paint unchecked box
  widgets/scrollbar.rs     [P0]  ScrollbarC::paint trough + slider
  widgets/drawing_area.rs  [P0]  PaintCx forwarding, pointer event forwarding
  gallery.rs           [P0]  drawing_area sample updated to the widened Draw fn
  README.md            [P0]

ui/tests/
  ingress.rs           [P0]  new: watch_fd / inbox / on_fd / Cmd::Task
  gallery_gate.rs      [P0]  KNOWN_BLANK_AT_REST 9 -> 6
  interaction_gate.rs  [P0]  pointer-event + rest-paint additions
  widget_pixels.rs     [P0]  rest-paint pixel tests; KeyEvent ctor helpers
  support/mod.rs       [P0]  Driver additions (middle click, drag-with-button)

settings/src/
  app.rs               [P1..P4]  SettingsModel, Msg, update, view, footer, nav
  ipc/mod.rs           [P1]  Workers, WorkerHandles
  ipc/reload.rs        [P1]  reload+ConfigReloaded worker
  ipc/portal.rs        [P2]  org.freedesktop.portal.FileChooser.OpenFile worker
  outputs/pump.rs      [P1]  watch_fd/on_fd glue for the second wayland connection
  pages/mod.rs         [P1]  PageId, PAGES
  pages/appearance.rs  [P1 extract | P2 view]
  pages/behavior.rs    [P2]
  pages/workspaces.rs  [P1 extract | P3 view]
  pages/keybindings.rs [P1 extract | P3 view]
  pages/displays_canvas.rs  [P1]  moved verbatim, zero edits
  pages/displays/state.rs   [P1]  the GTK-free core of old displays.rs
  pages/displays/controls.rs[P4]  per-head control panel view
  pages/displays/canvas.rs  [P4]  draw fn + pointer handling
  pages/displays/mod.rs     [P1 skeleton | P4 body]
  model.rs, compositor_reload.rs, outputs/{mod,protocol}.rs   unchanged
  main.rs              [P1]  thin binary
  DELETED at P1: outputs/client.rs

shell/src/
  panel.rs             [P5]  PanelModel, Msg, update, view
  main.rs              [P5]  rewritten
  style.rs             [P5]  style.css -> CompiledSheet overlay
  taskbar.rs, clipboard.rs, compositor_client.rs, clip_client.rs  model+IPC kept;
                             their `render`/`connect_activation` fns deleted
  DELETED at P5: bridge.rs

Execution order: P0 -> P1 -> P2 -> P3 -> P4 -> P5 -> P6. Each part's base is the
previous part's head.
```

---

## 1. P0 — toolkit pre-flight (`ui/` only)

Every item is additive to `icedtea-ui` and recorded here as an M3-contract-style
amendment. P0 ships all eleven; nothing else in `ui/` changes.

### M5-D1 — `Window::watch_fd`/`unwatch`, `InputEvent::FdReady`, the N-fd `wait_bounded`

**Carried out by:** P0 (`ui/src/window/mod.rs`).

**M3 contract §3.1 ships** a `wait_bounded` that polls exactly one fd — the
Wayland connection's (`ui/src/window/mod.rs:855`,
`let mut fds = [rustix::event::PollFd::new(&fd, PollFlags::IN)];`). An app with a
second event source has no way in.

**As amended:**

```rust
/// Opaque handle for one fd registered with [`Window::watch_fd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

/// What a watch waits for. `IN` is readability; `PRI` is out-of-band data.
/// `HUP`/`ERR` are always reported whatever the interest, and always arrive as
/// an `FdReady` — the toolkit never decides on its own that a foreign fd is
/// dead, it reports and lets the owner call [`Window::unwatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interest { Read, Write, ReadWrite }

impl Window {
    /// Register `fd` in this window's poll set. The `Window` takes ownership so
    /// the caller cannot close it out from under the loop.
    pub fn watch_fd(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId;

    /// Drop the watch and close the fd. Unknown ids are a no-op, not a panic.
    pub fn unwatch(&mut self, id: WatchId);

    /// Every live watch, in registration order. Diagnostics and tests.
    #[must_use] pub fn watches(&self) -> Vec<WatchId>;
}

pub enum InputEvent {
    // ... every M3 variant, unchanged ...
    /// A watched fd is ready. Carries no readiness flags: the owner of the fd
    /// is the only thing that knows what to do with it.
    FdReady(WatchId),
}
```

**Semantics, normative.**

1. `wait_bounded` gains a `watches: &[(WatchId, BorrowedFd, PollFlags)]` argument
   and builds a `Vec<PollFd>` whose **element 0 is always the Wayland queue fd**
   and whose elements 1.. are the watches in registration order. It keeps M3's
   exact loop shape otherwise: `dispatch_pending` → `closed` check → `ready`
   check → deadline arithmetic → `conn.flush()` → `prepare_read()` (`None` ⇒
   `continue`) → `poll` → `guard.read()` with the racing-reader `WouldBlock`
   swallow → `Errno::INTR` retry.
2. A watch whose `revents()` is non-empty pushes `InputEvent::FdReady(id)` onto
   `state.events` **before** `wait_bounded` returns, so `ready(state)` becomes
   true and the fd wakes `pump` exactly like a Wayland event does. `Ok(0)` from
   `poll` is still `SurfaceError::Timeout`; a wake caused only by a watch is
   **not** a timeout.
3. Each ready watch produces **one** `FdReady` per `pump` batch, never one per
   `revents` bit. `FdReady`s are ordered by registration order and appended
   after whatever Wayland events the same wake dispatched.
4. `unwatch` during a batch is legal: a `FdReady` already in the batch for an
   id that has since been unwatched is still delivered; `App::on_fd` skips
   callbacks for ids it no longer holds.
5. `Window::pump`'s existing post-processing (repeat sampling, `stamp_frames`,
   `Configure`/`ScaleChanged` handling) is untouched; `FdReady` falls through
   every one of those matches.
6. Zero idle wakeups: nothing here adds a timer, and `frame_deadline` is
   unchanged.

**Tests** (`ui/tests/ingress.rs`, offscreen where possible, harness where a
`Window` is needed):

- `a_watched_pipe_wakes_pump_with_fd_ready`
- `an_unwatched_fd_stops_waking_the_loop`
- `two_watched_fds_report_in_registration_order`
- `a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout`
- `unwatching_an_unknown_id_is_a_no_op`
- Mutation check: drop element 0 from the poll `Vec` and confirm
  `window_events.rs` fails; restore.

### M5-D2 — `Inbox<Msg>` / `InboxSender<Msg>`, `App::with_inbox`, `App::on_fd`

**Carried out by:** P0 (`ui/src/view/inbox.rs` new, `ui/src/view/app.rs`).

**M3 contract §4.7 has no external-message hook at all** — `App::run`'s loop is
closed over `Window::pump` (research note `ui-consumer-api.md` §2: "NOT
AVAILABLE").

**As amended:**

```rust
// ui/src/view/inbox.rs

/// The receiving half: what an `App` drains once per frame.
pub struct Inbox<Msg> { /* rx: crossbeam_channel::Receiver<Msg>, read: OwnedFd */ }

/// The sending half. `Send + Clone`, so a worker thread — or several — can hold
/// one. `send` pushes onto the channel and writes one byte to the wake pipe.
pub struct InboxSender<Msg> { /* tx: crossbeam_channel::Sender<Msg>, write: Arc<OwnedFd> */ }

impl<Msg: Send + 'static> Inbox<Msg> {
    /// Build a connected pair. The pipe is `CLOEXEC | NONBLOCK`
    /// (`rustix::pipe::pipe_with`).
    ///
    /// # Errors
    /// [`std::io::Error`] if the pipe cannot be created.
    pub fn new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)>;

    /// Another sender onto the same inbox.
    #[must_use] pub fn sender(&self) -> InboxSender<Msg>;
}

impl<Msg> InboxSender<Msg> {
    /// Queue `msg` and wake the loop. `Err` only once every `Inbox` is dropped,
    /// which is how a worker learns the app exited; it returns the message back.
    pub fn send(&self, msg: Msg) -> Result<(), SendError<Msg>>;
}

impl<Msg> Clone for InboxSender<Msg> { /* clones both halves */ }
unsafe impl<Msg: Send> Send for InboxSender<Msg> {}

#[derive(Debug)]
pub struct SendError<Msg>(pub Msg);
```

```rust
// ui/src/view/app.rs
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Feed `inbox`'s messages into this app's loop. At most one inbox per app;
    /// a second call replaces the first.
    #[must_use] pub fn with_inbox(self, inbox: Inbox<Msg>) -> Self
        where Msg: Send;

    /// Map a foreign fd's readiness to messages. `f` runs on the loop thread
    /// whenever `InputEvent::FdReady(id)` arrives, and its `Vec<Msg>` is
    /// enqueued in order.
    #[must_use] pub fn on_fd(self, id: WatchId, f: impl Fn() -> Vec<Msg> + 'static) -> Self;
}
```

**Semantics, normative.**

1. **`Msg: Send` is required for `with_inbox`.** Both M5 apps therefore have
   `Send` message types, which forbids `Rc` payloads: a message carrying a
   shared value carries `Arc<T>`, not `Rc<T>` (see §2.2 and §3.1).
2. `App::run` registers the inbox's read end with `Window::watch_fd(read,
   Interest::Read)` before the first `pump`, and `unwatch`es it on exit.
3. **Drain order — normative and tested.** Inside one iteration of `run`'s
   `while !rt.quit` loop, after `window.pump(wait)` returns and **before** the
   `for event in &events` routing loop:
   a. drain the wake pipe to EOF-of-readiness (a `read` loop until `WouldBlock`),
   b. drain the channel with `try_recv` until empty, pushing each `Msg` onto
      `rt.queue` **in send order**,
   c. for each `InputEvent::FdReady(id)` in `events`, in batch order, call the
      registered `on_fd` closure and push its messages onto `rt.queue`,
   d. then route the input batch as M3 already does, appending its messages
      after the inbox's.
   So: **inbox messages are applied before the frame's input batch**, and
   `on_fd`-derived messages sit between the two. `drain` is called once, at
   M3's existing point, and folds the whole queue.
4. A sender that outlives its `Inbox` never blocks and never panics: the
   channel is unbounded and the pipe write is `NONBLOCK`, and an `EAGAIN` on a
   full pipe is **ignored** — the byte already in the pipe is enough to wake the
   loop, and the channel is the queue.
5. `run_offscreen` accepts an inbox and drains it at the same point in its
   script loop, so an ingress test needs no compositor.
6. **Zero idle wakeups:** an app with an inbox and no traffic polls with exactly
   the deadline `frame_deadline` already computed.

**`ui/Cargo.toml`** gains `crossbeam-channel.workspace = true` and widens
`rustix` to `features = ["fs", "event", "pipe"]` (M5-D10).

**Tests** (`ui/tests/ingress.rs`):

- `an_inbox_message_reaches_update_on_the_next_frame`
- `inbox_messages_are_applied_in_send_order`
- `inbox_messages_are_applied_before_the_frames_input_batch`
- `a_sender_survives_being_cloned_across_threads`
- `sending_after_the_app_exits_returns_the_message_instead_of_panicking`
- `on_fd_maps_a_foreign_fd_to_messages`
- `an_app_with_an_idle_inbox_does_not_spin` (bounded wake count over a fixed wall
  clock — a complexity bound, not a timing pin)
- Mutation check: move the drain after the input routing loop; assert
  `inbox_messages_are_applied_before_the_frames_input_batch` fails; restore.

### M5-D3 — `Cmd::Task`

**Carried out by:** P0 (`ui/src/view/cmd.rs`, `ui/src/view/app.rs::drain`).

**M3 contract §4.7's `Cmd`** has no side-effect variant, so an outbound D-Bus
call would have to happen inside `update` — which spec D8 forbids ("outbound
calls run on the worker via `Cmd::Task`, never inside `update`").

**As amended:**

```rust
pub enum Cmd<Msg> {
    // ... every M3 variant, unchanged ...
    /// Run `f` once, on the loop thread, after the fold that produced it.
    ///
    /// `f` **must not block**: the intended body is a channel push to a worker
    /// thread (or a fire-and-forget call the app has already proven
    /// non-blocking). Anything whose answer matters comes back through the
    /// inbox as a `Msg`, never as a return value — `Cmd::Task` has none.
    Task(Rc<dyn Fn()>),
}
```

Executed in `drain`'s existing `match cmd` as `Cmd::Task(f) => f()`, alongside
`Cmd::Copy`/`Cmd::Focus` — i.e. after every queued message has been folded, and
outside `update`. It is **not** a window-bound command and is never returned to
`run`. `Cmd::flatten` treats it as a leaf. `run_offscreen` executes it
identically, which is what makes a `Cmd::Task` app testable offscreen.

**Tests:** `a_task_command_runs_once_after_the_fold`,
`a_task_command_runs_offscreen_too`, `a_batch_runs_its_tasks_in_order`.

### M5-D4 — `App::new` takes boxed closures

**Carried out by:** P0 (`ui/src/view/app.rs`). **Amends M3 contract §4.7
(`2026-08-27-m3-part0-contract.md:1516-1521`) by name, not by drift.**

**M3 contract §4.7 says:**

```rust
pub fn new(model: M, update: fn(&mut M, Msg) -> Cmd<Msg>, view: fn(&M) -> View<Msg>) -> Self;
```

**As amended:**

```rust
pub struct App<M, Msg> {
    model: M,
    update: Box<dyn FnMut(&mut M, Msg) -> Cmd<Msg>>,
    view: Box<dyn Fn(&M) -> View<Msg>>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
    inbox: Option<Inbox<Msg>>,
    fd_handlers: Vec<(WatchId, Box<dyn Fn() -> Vec<Msg>>)>,
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    pub fn new(
        model: M,
        update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static,
        view: impl Fn(&M) -> View<Msg> + 'static,
    ) -> Self;
}
```

**Ruling.** Both apps need captured state in `update` — settings holds a
`ReloadClient` handle and worker senders; shell holds `Rc<dyn
CompositorCommands>` / `Rc<dyn ClipCommands>` so its mocks can be swapped in a
test. A bare `fn` item cannot capture. `impl FnMut`/`impl Fn` is a strict
widening: **`fn` items still coerce**, so `ui/src/gallery.rs`'s
`App::new(model, update, scrolled_page)`, `ui/tests/counter_app.rs` and every
existing call site compile unchanged, and P0 must prove that by changing none of
them. `update` is `FnMut` (it may own worker senders and counters); `view` is
`Fn` (it is called during reconcile while the model is borrowed and must not
mutate). `App`'s `Debug` impl is unchanged (`finish_non_exhaustive`).

**Tests:** `an_fn_item_still_coerces_into_app_new` (compile-level: the counter
app and the gallery are the proof, plus one explicit assertion),
`a_closure_capturing_state_drives_the_loop`.

### M5-D5 — pointer events at the view layer: three `EventKind`s, `Handler::PairButton`, six builders, button constants

**Carried out by:** P0 (`ui/src/view/mod.rs`, `ui/src/view/builders.rs`,
`ui/src/widgets/drawing_area.rs`, `ui/src/window/pointer.rs`).

**M3 contract §4.4's `EventKind`** has eighteen kinds, none of them a raw
pointer event, and `Handler` has no button-carrying variant — so
middle-click-to-close (shell) and canvas dragging (settings Displays) have no
path from the low-level `Event::PointerDown/Up` a controller already receives up
to a `Msg`.

**As amended:**

```rust
// ui/src/view/mod.rs
pub enum EventKind {
    // ... the eighteen M3 kinds, in declaration order, unchanged ...
    PointerDown,
    PointerMotion,
    PointerUp,
}
// EventKind::ALL grows to 21, in the same declaration order.

pub enum Handler<Msg> {
    // ... the eight M3 variants, unchanged ...
    /// Local `(x, y)` plus the Linux input-event button code.
    PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>),
}

impl<Msg: Clone> Handlers<Msg> {
    /// Fire a `PairButton` **or** a `Pair` handler registered for `kind`:
    /// a `Pair` registered for a pointer kind receives `(x, y)` and the button
    /// is dropped, so `on_pointer_down` and `on_pointer_down_with_button` can
    /// coexist on different nodes without the caller choosing a fire method.
    pub fn fire_pair_button(&self, kind: EventKind, x: f64, y: f64, button: u32) -> Option<Msg>;
}
```

```rust
// ui/src/view/builders.rs — six additions, all sugar over `View::on`
pub trait ... /* the existing universal-builder impl on View<Msg> */ {
    #[must_use] fn on_pointer_down(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
    #[must_use] fn on_pointer_motion(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
    #[must_use] fn on_pointer_up(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
    #[must_use] fn on_pointer_down_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;
    #[must_use] fn on_pointer_motion_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;
    #[must_use] fn on_pointer_up_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;
}
// on_pointer_down        -> .on(EventKind::PointerDown,   Handler::Pair(..))
// on_pointer_down_with_button -> .on(EventKind::PointerDown, Handler::PairButton(..))
// … and so on for Motion and Up.
```

```rust
// ui/src/window/pointer.rs — Linux input-event codes, so no consumer hard-codes them
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
```
`ui/src/wayland.rs` keeps re-exporting `window::layer::BTN_LEFT` **unchanged** —
`ui/tests/layer_shell_screencopy.rs` is a byte-identical gate and imports it from
there.

**Semantics, normative.**

1. **Generic, not per-widget.** Any `Kind` may carry the three handlers. They
   are fired from `GenericC`'s `on_event` (the fallback controller every widget
   inherits event routing through) so a `box_`, a `button` and a `drawing_area`
   all work; a widget with its own `on_event` fires them first and then does its
   own work, and must not set `cx.handled` merely because a pointer handler was
   present.
2. **Coordinates.** `(x, y)` are `Event::PointerDown/PointerMotion/PointerUp`'s
   `local`, cast `f32 -> f64` — the border box of the node the handler is on,
   which is exactly what `Handler::Pair` already means for `on_scrolled` (M3
   deviation P6-D4).
3. **Grab semantics.** Nothing new. M3's `ImplicitGrab` already routes motion and
   the matching release to the node that took the press, so
   `PointerDown → PointerMotion* → PointerUp` is delivered to one node for the
   whole gesture even when the pointer leaves it, which is the property the
   Displays drag depends on. A `PointerUp` is delivered even when it lands
   outside the node; a `PointerLeave` mid-drag does **not** end the sequence and
   fires no handler. If the grab is broken (surface closed, popup dismissed),
   no synthetic `PointerUp` is fabricated — the app's `update` must tolerate a
   drag that never ends, which both M5 apps do by treating a fresh
   `PointerDown` as re-anchoring.
4. **`DrawingAreaC` forwards.** `DrawingAreaC::on_event` today handles only
   `Event::Configure` (`ui/src/widgets/drawing_area.rs:96-107`). It gains
   forwarding of `PointerDown`/`PointerMotion`/`PointerUp` through
   `cx.handlers.fire_pair_button(..)`, keeping its `Configure`/`Change` arm
   byte-identical. It does **not** set `cx.handled` — a canvas that swallowed
   events would break focus navigation over it.
5. `EventKind::Click` is unaffected. A left press-release on a node with both
   `on_click` and `on_pointer_up_with_button` produces **both** messages, and
   the app is responsible for the ordering-independent update (see §3.1's
   middle-click rule).

**Tests:**
- `ui/tests/ingress.rs`: `pointer_handlers_fire_down_motion_up_in_order`,
  `a_pair_handler_on_a_pointer_kind_receives_local_coordinates`,
  `a_pair_button_handler_receives_the_button_code`,
  `motion_after_a_press_reaches_the_pressed_node_through_the_grab`,
  `a_release_outside_the_node_still_reaches_it`.
- `ui/tests/interaction_gate.rs`:
  `dragging_across_a_drawing_area_reports_every_pointer_phase` (harness-driven,
  through `Driver::drag`).
- `ui/tests/widget_pixels.rs`: `event_kind_all_lists_twenty_one_kinds`.
- Mutation check: delete the `PointerMotion` forwarding arm in `DrawingAreaC`;
  the drag test fails; restore.

### M5-D6 — the `DrawingArea` draw callback carries `&mut PaintCx`

**Carried out by:** P0 (`ui/src/view/mod.rs`, `ui/src/widgets/drawing_area.rs`,
`ui/src/gallery.rs`, `ui/tests/widget_pixels.rs`).

**M3 contract §4.3 ships** `Prop::Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect)>)`. That
callback can fill and stroke, but it cannot draw **text**: `TextLayout::build`
needs `&mut FontDatabase` and there is no way to reach one from inside the
closure. The Displays canvas draws a connector name and a resolution per head
(`settings/src/pages/displays.rs:430-450`), so P4 would stall.

**As amended:**

```rust
pub enum Prop {
    // ... every other variant unchanged ...
    Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>),
}

// ui/src/widgets/drawing_area.rs
pub fn drawing_area<Msg: Clone + 'static>(
    draw: impl Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>) + 'static,
) -> View<Msg>;

pub struct DrawingAreaC {
    pub draw: Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>,
    pub content: (i32, i32),
    pub last_size: (f32, f32),
}
```

`DrawingAreaC::paint` already receives `cx: &mut PaintCx<'_>` (it is currently
`_cx`); it passes it straight through. `PaintCx` is already public and already
re-exported from `view::controller` (M3 E7), so no new type is introduced and
the closure reaches `cx.fonts`, `cx.icons`, `cx.images`, `cx.colors` and
`cx.env`. Two call sites change, both mechanically: `ui/src/gallery.rs:629` and
`ui/tests/widget_pixels.rs:1096`.

**Ruling.** Widening the callback beats every alternative: a second
`PropName::DrawTextFn` splits one paint into two, and pre-shaping text in the
model would make the canvas re-layout on every font change. `Prop`'s `Debug`
impl still prints `Draw(..)`; `Props::diff` still compares `Draw` by `Rc`
pointer, so the widening changes no diff behaviour.

**Tests:** `a_drawing_area_can_shape_text_through_its_paint_cx`
(`ui/tests/widget_pixels.rs`, offscreen, asserts non-background pixels where a
glyph run was placed); the existing `widget_pixels` drawing-area assertions keep
every number.

### M5-D7 — `Keymap::base_keysym` and `KeyEvent::base`

**Carried out by:** P0 (`ui/src/window/keyboard.rs`).

**M3 contract §3.3** keeps `keymap: xkb::Keymap` private
(`ui/src/window/keyboard.rs:181`) and exposes no level-0 lookup. `settings`'
keybinding capture needs exactly the equivalent of
`gdk::Display::translate_key(keycode, ModifierType::empty(), 0)`
(`settings/src/pages/keybindings.rs:147-152`) and has no workaround.

**As amended:**

```rust
impl Keymap {
    /// The keysym `keycode` produces at **group 0, level 0** — shift-, caps- and
    /// group-agnostic.
    ///
    /// `keycode` is the `wl_keyboard.key` evdev code; xkb's is `+8`, applied
    /// internally by the same `xkb_keycode` helper `translate` uses. A keycode
    /// the keymap does not map, or one that maps to several syms at that level,
    /// yields the first sym, or `Keysym::NoSymbol` when there is none — never a
    /// panic (contract cross-cutting rule: untrusted keymaps never panic).
    #[must_use]
    pub fn base_keysym(&self, keycode: u32) -> xkbcommon::xkb::Keysym;
}

pub struct KeyEvent {
    pub keycode: u32,
    pub keysym: xkbcommon::xkb::Keysym,
    /// `base_keysym(keycode)`, stamped by [`Keymap::translate`] so a view-layer
    /// `on_key` closure — which has a `&KeyEvent` and no `Keymap` — can
    /// normalise without reaching into the window.
    pub base: xkbcommon::xkb::Keysym,
    pub utf8: Option<String>,
    pub mods: Mods,
    pub consumed: Mods,
    pub pressed: bool,
    pub repeat: bool,
    pub serial: u32,
    pub time_ms: u32,
}
```

Implementation: `self.keymap.key_get_syms_by_level(xkb_keycode(keycode), 0, 0)`
(xkbcommon 0.9's `Keymap::key_get_syms_by_level`), `.first().copied()`
defaulting to `Keysym::NoSymbol`. `Keymap::translate` fills `base` in the same
pass it computes `keysym`; the five literal `KeyEvent` constructors
(`ui/src/widgets/headless.rs:149,170`, `ui/src/window/focus.rs:623`,
`ui/tests/widget_pixels.rs:632,649`) gain `base` set to the same value they set
`keysym` to, which is what an unmodified key already means.

**The normalisation rule — normative, and identical to
`compositor/src/input.rs:109-123`.** A settings capture resolves its keysym as:

```rust
let sym = if ev.base != xkb::keysyms::KEY_NoSymbol { ev.base } else { ev.keysym };
```

— the raw/latin keysym, falling back to the modified one **only** when the
keycode produces no base sym at all. This is required because
`icedtea_config::keys::key_name_to_keysym` always encodes the unshifted keysym
(`"KEY_q"` → `0x71`), so a capture that stored `0x51` (`XK_Q`) from a
`SUPER+SHIFT+q` press would produce a binding the compositor's `match_action`
can never fire. Modifiers come from `KeyEvent::mods` (effective state), not from
`consumed`.

**Tests** (`ui/tests/ingress.rs`, hermetic — the vendored
`ui/tests/fixtures/keymaps/us.xkb`, no compositor):

- `base_keysym_reports_the_level_zero_sym_for_a_shifted_key` — Shift held,
  `KEY_A` pressed: `keysym == XK_A`, `base == XK_a`.
- `base_keysym_is_group_and_caps_agnostic`
- `base_keysym_of_an_unmapped_keycode_is_no_symbol`
- `translate_stamps_base_on_every_key_event`
- Mutation check: change the level argument from 0 to 1; the first test fails;
  restore.

### M5-D8 — rest paint: four widgets, and `KNOWN_BLANK_AT_REST` drops to six

**Carried out by:** P0 (`ui/src/widgets/{color_dialog,check_button,scrollbar}.rs`,
`ui/tests/gallery_gate.rs`).

**M3 deviation P8-D69** exempts nine widgets from `every_widget_renders_at_rest`
(`ui/tests/gallery_gate.rs:146-158`). Four of them are widgets settings uses.

**As amended:**

```rust
impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogButtonC {
    /// Adwaita's `button.color` minimum: the swatch is chrome on a controller-
    /// owned subnode taffy never sees, so without an intrinsic size the whole
    /// button collapses (the `ScrollbarC::measure` / `ScaleC::measure` case).
    fn measure(&mut self, available: (Option<f32>, Option<f32>), cx: &mut BuildCx<'_>)
        -> Option<(f32, f32)>;   // Some((48.0, 32.0))
    fn paint(..) -> bool;        // unchanged: fills `alloc.content_box` with `self.rgba`
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogC {
    /// The chooser body's intrinsic size, from the palette grid it builds.
    fn measure(&mut self, available: (Option<f32>, Option<f32>), cx: &mut BuildCx<'_>)
        -> Option<(f32, f32)>;
    /// One filled rounded rect per `self.swatches` entry, plus `self.custom`.
    fn paint(&mut self, canvas: &mut Canvas<'_>, alloc: &Allocation,
             style: &ComputedStyle, cx: &mut PaintCx<'_>) -> bool;
}

impl<Msg: Clone + 'static> Controller<Msg> for CheckButtonC {
    /// The unchecked box is drawn too: the early `if !self.active &&
    /// !self.inconsistent { return false }` becomes a branch that strokes the
    /// indicator's border and fills its background from `style` instead of
    /// bailing. The checked/inconsistent path still goes through
    /// `paint::icon::paint_builtin` and is unchanged.
    fn paint(..) -> bool;        // now returns true in every state
}

impl<Msg: Clone + 'static> Controller<Msg> for ScrollbarC {
    /// `range`/`trough`/`slider` are controller-owned subnodes with no taffy
    /// box, so nothing paints them. This paints the trough across
    /// `alloc.content_box` and the slider at the position `self.adj`/
    /// `self.value` already compute for hit-testing — the same geometry
    /// `on_event` reads through `content_rect_local`, so the drawn slider and
    /// the hit-tested slider cannot drift.
    fn paint(&mut self, canvas: &mut Canvas<'_>, alloc: &Allocation,
             style: &ComputedStyle, cx: &mut PaintCx<'_>) -> bool;
    fn measure(..) -> Option<(f32, f32)>;   // unchanged: (40,14) / (14,40)
}
```

```rust
// ui/tests/gallery_gate.rs
const KNOWN_BLANK_AT_REST: &[&str] = &[
    // Zero-area allocation.
    "window_controls",
    "font_dialog",
    "popover_menu",
    "popover_menu_bar",
    "alert_dialog",
    // Real allocation, nothing drawn into it.
    "link_button",
];
```

Exactly six, and exactly the six M5 never touches. `color_dialog`,
`check_button` and `scrollbar` leave the list, and `color_dialog_button` was
never on it.

**If the zero-area fixes share a root cause** — the "a controller whose whole
chrome is subnodes has no intrinsic size" pattern `ScaleC`/`ScrollbarC` already
document — a generic fix that also closes `font_dialog`, `popover_menu`,
`popover_menu_bar`, `alert_dialog` or `window_controls` is **welcome, recorded as
a further §6 amendment, and shrinks the list further**. It is not planned work
and no part is judged on it.

**Tests:**
- `ui/tests/gallery_gate.rs`: the three existing
  `every_widget_renders_at_rest_in_the_{light,dark,high_contrast}_theme` tests
  now cover the three de-listed widgets; the const's doc comment carries the
  mutation check (re-add `"scrollbar"`; nothing fails ⇒ the paint is not being
  exercised).
- `ui/tests/widget_pixels.rs` (offscreen, one per widget):
  `a_color_dialog_button_paints_its_swatch_at_rest`,
  `a_color_dialog_paints_its_palette_at_rest`,
  `an_unchecked_check_button_paints_its_box`,
  `a_scrollbar_paints_its_trough_and_slider`.
- `ui/tests/interaction_gate.rs`:
  `dragging_a_scrollbar_slider_moves_what_it_paints` (ties the new paint to the
  existing hit-test geometry).
- Mutation check per widget: restore the early `return false`; the matching
  pixel test fails; restore.

### M5-D9 — live-window probe: `Window::probe_points` / `Window::allocation`

**Carried out by:** P0 (`ui/src/window/mod.rs`).

**M3 deviation P8-D59/P8-D60** put probing on `App::probe`, which is
offscreen-only (`ui/src/view/app.rs:596`) and gallery-shaped. A harness test
against a **running** settings or shell process has no way to ask where a widget
ended up, so every M5 gate would have to hard-code coordinates — which the M3
gate rules forbid.

**As amended:**

```rust
/// One probe point on a live window, in window-surface coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    /// The node's `id` if it has one, else its CSS node name, indexed when the
    /// name repeats — the same labelling rule `gallery::probe_points_of` uses.
    pub label: String,
    pub x: i32,
    pub y: i32,
}

impl Window {
    /// Every laid-out node of this window's tree, labelled and centred.
    ///
    /// Same derivation as `gallery::probe_points_of` (`ui/src/gallery.rs:1333`):
    /// walk `root().descendants()`, label by id-or-node-name with a repeat
    /// index, centre of the border box floored with `f32::floor` so a shifted
    /// box shifts its centre by the same integer. A node with no allocation is
    /// skipped. Cheap: it reads the layout tree the last frame already
    /// computed, and lays nothing out.
    #[must_use] pub fn probe_points(&self) -> Vec<ProbePoint>;

    /// The border box of the node whose `Node::id()` is `id`, in window-surface
    /// coordinates. `None` for an unknown id or a node not laid out.
    #[must_use] pub fn allocation(&self, id: &str) -> Option<crate::layout::Allocation>;
}
```

Both read `self.root` and `self.layout`, which `Window` already owns
(`ui/src/window/mod.rs:901`). `App::probe`/`Probe<Msg>` are untouched, as is the
gallery's `--probe-points`/`--print-allocation` line format (M3 P8-D64).

**How M5 apps expose them.** Neither app grows a probe CLI. Both, when
`$ICEDTEA_PROBE_REPORT` is set, write `probe <label> <x> <y>` and
`alloc <id> <x> <y> <w> <h>` lines to that file once per frame in which the tree
changed — the mechanism `ui/src/bin/window-probe.rs` already uses and
`ui/tests/support/mod.rs`'s `probe_report`/`wait_for_report_line`/
`parse_probe_line`/`parse_allocation_line` already parse. P1 adds the writer to
settings; P5 to shell. Widget ids are set with `View::id(&str)` and are
**normative per page/panel** — each part's plan lists its ids, and its gates
address widgets by them.

**Tests:**
- `ui/tests/window_events.rs`: `probe_points_locate_a_live_windows_widgets`,
  `allocation_by_id_matches_the_probe_point_centre` (driven through
  `window-probe.rs`, which gains an `--emit-probe` flag).
- Mutation check: return `border_box` un-floored; the centre assertion fails;
  restore.

### M5-D10 — `ui/Cargo.toml` dependency additions

**Carried out by:** P0.

```toml
rustix = { version = "1", features = ["fs", "event", "pipe"] }   # "pipe" is new
crossbeam-channel.workspace = true                                # new
```

`rustix`'s `pipe` module is feature-gated (`rustix-1.1.4/src/lib.rs:262`) and
gives `pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK) -> (OwnedFd, OwnedFd)`
(read end first). `crossbeam-channel` is already the workspace's channel crate
(`Cargo.toml:17`, used by `harness/src/lib.rs:193,196`); `Sender<T>` is
`Send + Clone` for `T: Send` with no wrapper, which is what `InboxSender`
requires. **No third channel crate is introduced into `icedtea-ui`**;
`async-channel` stays where it already is, in the app crates' zbus workers.

`--no-default-features` must still build and `clippy -D warnings` must still pass
in both configurations — neither new dep is optional or feature-gated.

### M5-D11 — `ui/README.md`

**Carried out by:** P0. Continues M3's E16 convention (README appended per part).

Additions, as new subsections:

1. **"External events"** — under *Reactive framework (`view/`)*: `Inbox`/
   `InboxSender`, `App::with_inbox`, `App::on_fd`, `Window::watch_fd`/`unwatch`,
   `InputEvent::FdReady`, the drain order (inbox → `on_fd` → input batch), the
   `Msg: Send`/`Arc`-not-`Rc` rule, and `Cmd::Task`'s "must not block".
2. **"Closures in `App::new`"** — the M5-D4 widening, with the note that `fn`
   items still coerce.
3. **"Pointer events at the view layer"** — the three `EventKind`s,
   `Handler::PairButton`, the six builders, the grab semantics of M5-D5 §3, and
   the `BTN_*` constants.
4. **"Base-level keysyms"** — under *The window and event layer*:
   `Keymap::base_keysym`, `KeyEvent::base`, and the normalisation rule quoted
   from `compositor/src/input.rs`.
5. **"Probing a live window"** — `Window::probe_points`/`allocation`, and the
   `$ICEDTEA_PROBE_REPORT` line formats.
6. **Correction.** The existing sentence in *The M3 gates* claiming "seven
   collapse to zero-area" is wrong twice: it was six, and after M5-D8 it is
   five zero-area plus one that allocates and paints nothing (`link_button`).
   Rewrite it to name the six `KNOWN_BLANK_AT_REST` entries and say which kind of
   blank each is.

**Test:** `ui/tests/gallery_gate.rs::the_readme_widget_table_lists_every_kind`
stays green (no `Kind` is added by M5), and P0 adds
`the_readme_names_every_known_blank_widget` beside it.

---

## 2. Settings — P1 … P4

One `App<SettingsModel, Msg>` on a `Role::Toplevel` window, 480×420, app-id
`org.icedtea.Settings`, title `"icedtea Settings"`.

### 2.1 Crate layout after extraction (P1)

**Moved verbatim, zero edits, tests come with them:**

| From | To | Items | Tests |
|---|---|---|---|
| `pages/displays_canvas.rs` | unchanged path | whole file | 8 |
| `model.rs` | unchanged path | whole file | 10 |
| `compositor_reload.rs` | unchanged path | whole file | 1 |
| `outputs/{mod,protocol}.rs` | unchanged path | whole file | via `tests/outputs_client.rs` |

**Extracted out of GTK files into GTK-free siblings**, with their tests, as real
signatures (all `pub`, all in the module named):

```rust
// settings/src/pages/workspaces.rs   (6 tests move with them)
pub fn next_workspace_name(existing: &[String]) -> String;
pub fn prune_orphaned_workspace_bindings(cfg: &mut icedtea_config::Config);

// settings/src/pages/keybindings.rs  (6 tests move with them)
pub const FIXED_ACTIONS: [&str; 10];
pub fn action_list(workspace_count: usize) -> Vec<String>;
pub fn format_combo(combo: &icedtea_config::KeyCombo) -> String;
pub fn should_capture(page_visible: bool, capturing: bool) -> bool;
pub fn take_capture_reset(capturing: &mut Option<String>) -> Option<String>;
/// Replaces `unshifted_keysym(keycode, fallback)`; no GDK, no keymap handle.
/// `ev.base` is `KeyEvent::base` (M5-D7), `ev.keysym` its modified sym.
pub fn normalise_keysym(base: u32, modified: u32) -> u32;

// settings/src/pages/appearance.rs   (2 tests move, re-typed)
/// Was `hex_to_rgba(&str) -> gdk::RGBA`. Same parse (`model::valid_hex` +
/// `u8::from_str_radix`), toolkit colour type.
pub fn hex_to_rgba(s: &str) -> icedtea_ui::css::value::Rgba;
/// Was `rgba_to_hex(&gdk::RGBA) -> String`. Same `#RRGGBB` rendering.
pub fn rgba_to_hex(c: icedtea_ui::css::value::Rgba) -> String;
/// A `ColorDialogButton` carries its colour as a packed f64 (M3 P5-D23).
pub fn hex_to_packed(s: &str) -> f64;          // = ColorDialogC::pack(hex_to_rgba(s))
pub fn packed_to_hex(packed: f64) -> String;   // = rgba_to_hex(ColorDialogC::unpack(packed))

// settings/src/pages/displays/state.rs   (12 tests move with them, verbatim)
pub struct DisplaysState {
    pub heads: Vec<Head>, pub edits: Vec<HeadEdit>, pub selected: Option<usize>,
    pub dirty: bool, pub view: displays_canvas::View,
    pub drag: Option<Drag>, pub res_options: Vec<(i32, i32)>, pub refresh_options: Vec<i32>,
}
impl DisplaysState { pub fn new() -> Self; }
pub struct Reconciled { pub edits: Vec<HeadEdit>, pub selected: Option<usize>,
                        pub compatible: bool, pub dropped: bool }
pub const CANVAS_MARGIN: f64 = 16.0;
pub const SNAP_THRESHOLD: f64 = 40.0;
pub const TRANSFORM_LABELS: [&str; 8];
pub const TRANSFORM_VALUES: [i32; 8];
pub fn baseline_edit(h: &Head) -> HeadEdit;
pub fn default_mode_for(head: &Head) -> Option<ModeRequest>;
pub fn same_connector_set(a: &[Head], b: &[Head]) -> bool;
pub fn reconcile(old_heads: &[Head], old_edits: &[HeadEdit], old_selected: Option<usize>,
                 dirty: bool, new_heads: &[Head]) -> Reconciled;
pub fn distinct_resolutions(modes: &[Mode]) -> Vec<(i32, i32)>;
pub fn refreshes_for(modes: &[Mode], w: i32, h: i32) -> Vec<i32>;
pub fn format_refresh(mhz: i32) -> String;
pub fn enabled_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>);
pub fn all_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>, Vec<bool>);

/// New in P1: the drag state the GTK `GestureDrag` used to hold implicitly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag { pub head: usize, pub origin: (f64, f64), pub start: (i32, i32) }
```

`DisplaysState::new`'s body, `reconcile`'s hotplug-vs-property-update rules
(findings #10/#11), `all_rects`' 1920×1080 fallback and `default_mode_for`'s
current→preferred→first order are **not** re-derived: they move byte-for-byte,
and the twelve tests that pin them move unmodified.

**Deleted at P1:** `settings/src/outputs/client.rs` (the `gio::Socket`/`glib`
source), and with it `OutputsClient`. `tests/outputs_client.rs` drives
`OutputsConnection` directly today and is unaffected.

**`settings/Cargo.toml` after P1:**

```toml
[dependencies]
icedtea-config  = { path = "../config" }
icedtea-contract = { path = "../contract" }
icedtea-ui      = { path = "../ui" }        # new
wayland-client  = "0.31"
wayland-protocols-wlr = { version = "0.3", features = ["client"] }
rustix          = "1"
async-channel   = "2"                        # kept: outputs/protocol.rs is unchanged
crossbeam-channel.workspace = true           # new: the inbox side
zbus.workspace = true
redb.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
icedtea-harness = { path = "../harness" }
tempfile = "3"
```

**Removed at P1: `gtk4`, `gio`, `glib`.** `async-channel` is *not* GTK and stays,
because spec §5.3 keeps `outputs/protocol.rs` unchanged and its
`connect_to_env(tx: async_channel::Sender<OutputsMsg>)` signature with it.

### 2.2 `SettingsModel` and `Msg`

```rust
// settings/src/app.rs

pub struct SettingsModel {
    /// The working/saved pair, `apply()` and the displays-preserving guard.
    /// `model.rs` is untouched; dirty is `working != saved`, computed, never a flag.
    pub model: crate::model::Model,
    pub db_path: std::path::PathBuf,
    pub page: crate::pages::PageId,
    pub status: String,
    /// The action whose binding is being captured, if any.
    pub capturing: Option<String>,
    /// Conflicting bindings, recomputed from `working` after every edit.
    pub conflicts: Vec<(icedtea_config::KeyCombo, Vec<String>)>,
    pub wallpaper_text: String,
    pub wallpaper_error: Option<String>,
    pub portal_available: bool,
    pub displays: crate::pages::displays::state::DisplaysState,
    pub displays_status: String,
    pub displays_in_flight: bool,
    pub outputs_available: bool,
    /// Worker senders. Held so `update` can `Cmd::Task` onto them.
    pub workers: crate::ipc::WorkerHandles,
}

#[derive(Clone, Debug)]
pub enum Msg {
    // --- navigation and footer -------------------------------------------
    PageSelected(usize),
    Apply,
    Revert,
    /// From the reload worker, through the inbox.
    Applied(Result<crate::compositor_reload::ReloadOutcome, String>),
    /// The compositor emitted `ConfigReloaded`; refresh nothing, note it.
    ConfigReloaded,

    // --- Appearance (P2) --------------------------------------------------
    BarPositionSelected(usize),
    BarHeightChanged(f64),
    CornerRadiusChanged(f64),
    SnapGapChanged(f64),
    BackgroundPicked(f64),      // packed Rgba, M3 P5-D23
    ForegroundPicked(f64),
    AccentPicked(f64),
    WallpaperEdited(String),
    WallpaperBrowse,
    WallpaperChosen(std::path::PathBuf),
    /// No portal, or the user cancelled. Carries a message for the status line.
    WallpaperPickerFailed(String),
    WallpaperCleared,

    // --- Behavior (P2) ----------------------------------------------------
    RaiseOnFocusToggled(bool),
    HideBarOnFullscreenToggled(bool),
    SnapEnabledToggled(bool),

    // --- Workspaces (P3) --------------------------------------------------
    WorkspaceRenamed(usize, String),
    WorkspaceRemoved(usize),
    WorkspaceAdded,

    // --- Keybindings (P3) -------------------------------------------------
    CaptureArmed(String),
    CaptureCancelled,
    /// Already normalised: `keysym` is `normalise_keysym(ev.base, ev.keysym)`.
    KeyCaptured { keysym: u32, mods: crate::model::CaptureMods },

    // --- Displays (P4) ----------------------------------------------------
    /// One protocol message, from `on_fd` on the outputs connection.
    /// `Arc`, not `Rc`: `Msg` is `Send` (M5-D2).
    Outputs(std::sync::Arc<crate::outputs::OutputsMsg>),
    HeadSelected(usize),
    HeadEnabledToggled(bool),
    ResolutionSelected(usize),
    RefreshSelected(usize),
    TransformSelected(usize),
    HeadScaleChanged(f64),
    /// Canvas coordinates, from the `DrawingArea`'s pointer handlers.
    HeadDragBegan(f64, f64),
    HeadDragged(f64, f64),
    HeadDragEnded(f64, f64),
    DisplaysTest,
    DisplaysApply,
    DisplaysRevert,
}
```

**`Msg` is `Send`.** Every payload is owned or `Arc`; no `Rc` appears in a
variant. This is M5-D2's requirement and is checked by a static assertion
(`const _: fn() = || { fn assert_send<T: Send>() {} assert_send::<Msg>(); };`).

`update` is `impl FnMut(&mut SettingsModel, Msg) -> Cmd<Msg>`, one `match` with
one arm group per page, all in `settings/src/app.rs`. It never blocks and never
calls D-Bus: `Apply` returns `Cmd::Task` (see §2.4).

**Recorded as a §6 deviation at P1, not ported:** `tests/appearance_gtk.rs`'s
`populate_never_writes_a_widget_fallback_back_into_the_model`. The GTK
`Ctx`/`Page`/`populating: Rc<Cell<bool>>` machinery
(`settings/src/pages/mod.rs:28-43`) exists because a programmatic widget write
fires the same handler a user edit does. On an Elm loop there is no write-back
at all — `view` renders `model.working` and a handler only runs from real input —
so the property is true by construction and there is nothing left to guard.
`Ctx`, `Page`, `Page::refresh`, `populating` and the `keybindings_page_slot`
forward-reference (`settings/src/main.rs:96-110`) are all deleted.

### 2.3 `view` decomposition

One function per page, each `pub fn` in its page module, each returning
`View<Msg>`; `app::view` composes them. Every page function is pure and takes
`&SettingsModel`.

```rust
// settings/src/pages/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId { Appearance, Behavior, Workspaces, Keybindings, Displays }
impl PageId {
    pub const ALL: [PageId; 5];
    #[must_use] pub fn name(self) -> &'static str;   // "appearance", …  (stack page name)
    #[must_use] pub fn title(self) -> &'static str;  // "Appearance", …  (switcher label)
    #[must_use] pub fn from_index(i: usize) -> PageId;
}

// settings/src/app.rs
pub fn view(m: &SettingsModel) -> View<Msg>;
fn nav(m: &SettingsModel) -> View<Msg>;      // stack_switcher over PageId::ALL
fn footer(m: &SettingsModel) -> View<Msg>;   // status label + Revert + Apply

// one per page
pub fn crate::pages::appearance::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::behavior::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::workspaces::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::keybindings::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::displays::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::displays::controls::view(m: &SettingsModel) -> View<Msg>;
pub fn crate::pages::displays::canvas::view(m: &SettingsModel) -> View<Msg>;
```

**Root shape** (`app::view`), using M3's builders verbatim:

```rust
box_(Orientation::Vertical, [
    nav(m),                                   // stack_switcher(pages)
    stack([
        stack_page("appearance",  "Appearance",  pages::appearance::view(m)),
        stack_page("behavior",    "Behavior",    pages::behavior::view(m)),
        stack_page("workspaces",  "Workspaces",  pages::workspaces::view(m)),
        stack_page("keybindings", "Keybindings", pages::keybindings::view(m)),
        stack_page("displays",    "Displays",    pages::displays::view(m)),
    ])
    .visible_child(m.page.name())
    .id("pages"),
    footer(m),
])
.id("root")
.on_key(|ev| pages::keybindings::capture_key(ev))   // window-root capture, P3
```

**Navigation.** `stack_switcher(pages)` where `pages: Rc<[StackPageInfo]>` is
built from `PageId::ALL`, `.on_selected(|i| Msg::PageSelected(i))`, `.id("nav")`.
Spec D7: `Stack` + `StackSwitcher`, not `StackSidebar` — its eviction defect is
out of scope (spec §11).

**Footer.** `box_(Orientation::Horizontal, [status, revert, apply])` with 8px
margins, `.id("footer")`:
- `label(&m.status).id("status")`, `hexpand`, `halign: Start`. The dirty
  indicator is `if m.model.is_dirty() { "Unsaved changes" } else { &m.status }` —
  computed, exactly the GTK `update_footer` closure's semantics
  (`settings/src/main.rs:73-84`) with the `Rc<dyn Fn()>` gone.
- `button("Revert").id("revert").on_click(Msg::Revert)`, `.sensitive(dirty)`.
- `button("Apply").id("apply").on_click(Msg::Apply)`, `.sensitive(dirty)`.

**Widget ids** (normative; the gates address these):
`root`, `nav`, `pages`, `footer`, `status`, `revert`, `apply`;
per page, `appearance.{wallpaper_entry, wallpaper_browse, wallpaper_clear,
background, foreground, accent, bar_position, bar_height, corner_radius}`,
`behavior.{raise_on_focus, hide_bar_on_fullscreen, snap_enabled, snap_gap}`,
`workspaces.{list, add}` plus per-row `ws_name_<i>` / `ws_remove_<i>`,
`keybindings.{list}` plus per-row `kb_row_<action>` / `kb_set_<action>`,
`displays.{canvas, enabled, resolution, refresh, transform, scale, position,
test, revert, apply, status, unavailable}`. Ids are dotted-free strings, e.g.
`"appearance_wallpaper_entry"`; each part's plan spells the literals.

**Keyed reconciliation.** Every list child carries a `View::key`: workspaces by
row index (rows are positional and renameable — the model's own ordering is the
identity), keybindings by action string, the displays head panel by connector
name. Without keys a rename would rebuild the `Entry` and lose the caret.

### 2.4 IPC: the worker thread and inbox wiring (P1)

```rust
// settings/src/ipc/mod.rs
use icedtea_ui::view::{Inbox, InboxSender};

/// Everything `update` needs to reach a worker. Cloneable, `Send`-free (it
/// lives on the loop thread); each field is a channel *sender*.
#[derive(Clone)]
pub struct WorkerHandles {
    reload: crossbeam_channel::Sender<ReloadRequest>,
    portal: crossbeam_channel::Sender<PortalRequest>,
}

impl WorkerHandles {
    /// Queue a save+reload. Never blocks: the answer arrives as
    /// `Msg::Applied` on the inbox.
    pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf);
    /// Queue a portal `OpenFile`. The answer arrives as `Msg::WallpaperChosen`
    /// or `Msg::WallpaperPickerFailed`.
    pub fn choose_wallpaper(&self, current: Option<std::path::PathBuf>);
}

/// Start both workers against `tx` and return their handles.
pub fn spawn(tx: InboxSender<crate::app::Msg>) -> WorkerHandles;
```

```rust
// settings/src/ipc/reload.rs
pub enum ReloadRequest { Apply { cfg: icedtea_config::Config, db_path: std::path::PathBuf } }

/// One `std::thread::spawn`. Owns a `compositor_reload::ReloadClient` (blocking
/// zbus) for the request side, and a second async `zbus::Connection` +
/// `MessageStream` for the `ConfigReloaded` signal on
/// `org.icedtea.Compositor` at `COMPOSITOR_PATH` — the exact
/// `MatchRule::builder().msg_type(Signal).interface(..).path(..)` pattern
/// `shell/src/compositor_client.rs:98-103` proves.
///
/// Per request: `compositor_reload::apply_and_reload(&cfg, &db_path, &client)`,
/// then `tx.send(Msg::Applied(result.map_err(|e| e.to_string())))`.
/// Per signal:  `tx.send(Msg::ConfigReloaded)`.
///
/// A send error means the app exited: the thread returns. Unlike shell's
/// worker it does **not** `process::exit` — settings is a foreground app with
/// no `Restart=always` unit (there is no `settings.service`), and a dead
/// reload worker must leave the window usable.
pub fn spawn(rx: crossbeam_channel::Receiver<ReloadRequest>,
             tx: InboxSender<crate::app::Msg>) -> std::thread::JoinHandle<()>;
```

`compositor_reload.rs` itself is unchanged — `ReloadClient::new`,
`ReloadClient::reload`, `apply_and_reload`, `ReloadOutcome` and its one test are
verbatim, and the `"ReloadConfig"` PascalCase wire member with them.

**`Msg::Apply` in `update`:**

```rust
Msg::Apply => {
    let cfg = m.model.working.clone();
    let (h, db) = (m.workers.clone(), m.db_path.clone());
    m.status = "Applying…".into();
    Cmd::Task(Rc::new(move || h.apply(cfg.clone(), db.clone())))
}
Msg::Applied(Ok(ReloadOutcome::Reloaded)) => {
    m.model.saved = m.model.working.clone(); m.status = "Applied".into(); Cmd::None
}
Msg::Applied(Ok(ReloadOutcome::CompositorAbsent)) => {
    m.model.saved = m.model.working.clone();
    m.status = "Saved; will apply when the compositor starts".into(); Cmd::None
}
Msg::Applied(Err(e)) => { m.status = format!("Failed to save: {e}"); Cmd::None }
```

Three status strings, one saved-snapshot rule per outcome — the same three
branches `settings/src/main.rs:136-158` ships, and the model stays dirty on
`Err`. `Msg::Revert` calls `m.model.revert(&m.db_path)` and clears
`m.status`; **Displays is not reverted by it** (it owns its own Revert, exactly
as `main.rs:165-174` documents).

### 2.5 The outputs connection: `watch_fd` + `on_fd` (P1)

```rust
// settings/src/outputs/pump.rs
use icedtea_ui::window::{Interest, WatchId, Window};

/// Own the second `wayland-client` connection and its readiness fd.
pub struct OutputsPump {
    conn: std::rc::Rc<std::cell::RefCell<crate::outputs::OutputsConnection>>,
    rx: async_channel::Receiver<crate::outputs::OutputsMsg>,
    watch: WatchId,
}

impl OutputsPump {
    /// Connect, register the queue fd with `window`, and return the pump plus
    /// its `WatchId`.
    ///
    /// `rustix::io::dup(conn.queue().as_fd())` gives the `OwnedFd`
    /// `Window::watch_fd` takes ownership of — the original stays with the
    /// `EventQueue`, the same dup `outputs/client.rs:61` did for `gio::Socket`.
    /// A connect failure (no compositor, no output-management global, fd
    /// exhaustion) is **not** fatal: it returns `Ok(None)` and the page shows
    /// its "output management unavailable" state, which is the exact
    /// degradation the deleted `OutputsClient` had.
    ///
    /// # Errors
    /// Only if `dup` and the connection both succeeded but registration could
    /// not (never, today) — the connect path returns `Ok(None)`.
    pub fn attach(window: &mut Window) -> std::io::Result<Option<OutputsPump>>;

    #[must_use] pub fn watch(&self) -> WatchId;

    /// The `App::on_fd` body. Non-blocking, and never `blocking_dispatch`.
    ///
    /// `dispatch_pending()`; if it dispatched nothing, `read_and_dispatch()`
    /// (whose `prepare_read`/`WouldBlock` handling is `protocol.rs:367-379`'s,
    /// unchanged). Then drain `self.rx` with `try_recv` until empty and map
    /// each to `Msg::Outputs(Arc::new(m))`. A dispatch error calls
    /// `notify_disconnected()` and yields the resulting `Disconnected` message;
    /// the watch is left registered so the app can decide.
    pub fn drain(&self) -> Vec<crate::app::Msg>;

    /// Fire-and-forget, from `Cmd::Task`.
    pub fn test_configuration(&self, edits: &[crate::outputs::HeadEdit]);
    pub fn build_and_send_configuration(&self, edits: &[crate::outputs::HeadEdit]);
    pub fn flush(&self);
}
```

Wired in `main.rs` as:

```rust
let mut window = Window::open(spec, sheet, fonts)?;
let (inbox, tx) = Inbox::new()?;
let workers = ipc::spawn(tx);
let pump = outputs::pump::OutputsPump::attach(&mut window)?;
let watch = pump.as_ref().map(OutputsPump::watch);
let mut app = App::new(SettingsModel::new(db_path, workers, pump.clone()), app::update, app::view)
    .with_inbox(inbox);
if let (Some(id), Some(p)) = (watch, pump) {
    app = app.on_fd(id, move || p.drain());
}
app.run(window)
```

`outputs/protocol.rs` is not edited. Its `OutputsMsg` variants
(`HeadsChanged`, `ManagerUnavailable`, `Disconnected`, `ApplySucceeded { is_test }`,
`ApplyFailed`, `ApplyCancelled`) map onto `Msg::Outputs` and are handled in
`update` exactly as the glib source's `match` handled them
(`settings/src/pages/displays.rs:836-1023`): `HeadsChanged` → `reconcile`,
`ManagerUnavailable`/`Disconnected` → `outputs_available = false`,
`ApplySucceeded`/`ApplyFailed`/`ApplyCancelled` → clear `displays_in_flight` and
set `displays_status`. `ConfigData { is_test }` still tags which request a reply
answers; `displays_in_flight` is never used to infer it.

### 2.6 P2 — Appearance and Behavior

**Appearance.** `grid([...])` of labelled rows (`labeled_row`'s GTK helper
becomes a private `fn row(label: &str, control: View<Msg>) -> View<Msg>` in the
page module):

| Control | Builder | Handler → `Msg` |
|---|---|---|
| Wallpaper path | `entry(&m.wallpaper_text).id("appearance_wallpaper_entry")` | `.on_change(\|s\| Msg::WallpaperEdited(s.into()))` |
| Browse | `button("Choose…")` | `.on_click(Msg::WallpaperBrowse)` |
| Clear | `button("Clear")` | `.on_click(Msg::WallpaperCleared)` |
| Preview | `picture(...)` or a `label` carrying `m.wallpaper_error` | — |
| Background/Foreground/Accent | `color_dialog_button(hex_to_packed(&hex))` ×3 | `.on_value_changed(\|p\| Msg::BackgroundPicked(p))` etc. (M3 P5-D23: the colour rides `Handler::Float` as a packed f64) |
| Bar position | `drop_down(&model::BAR_POSITIONS)` `.selected(i)` | `.on_selected(\|i\| Msg::BarPositionSelected(i))` |
| Bar height / Corner radius | `spin_button(v, lo, hi).step(1.0)` ×2 | `.on_value_changed(...)` |

**The portal FileChooser flow (D5).** `Msg::WallpaperBrowse` returns
`Cmd::Task(move || workers.choose_wallpaper(current))`. The portal worker:

```rust
// settings/src/ipc/portal.rs
pub enum PortalRequest { OpenFile { current: Option<std::path::PathBuf> } }

/// Bus `org.freedesktop.portal.Desktop`, path `/org/freedesktop/portal/desktop`,
/// interface `org.freedesktop.portal.FileChooser`, method `OpenFile(
///   parent_window: &str, title: &str, options: HashMap<&str, Value>)
///   -> OwnedObjectPath`.
///
/// Options: `"filters"` (`a(sa(us))`, image MIME types), `"current_folder"`
/// (`ay`, NUL-terminated). The method returns a *request* path; the answer is
/// the `org.freedesktop.portal.Request.Response` signal on that path,
/// `(response: u32, results: HashMap<String, Value>)`, with `results["uris"]`
/// on `response == 0`. Subscribed with a `MatchRule` filtered to that path —
/// the same builder shape `compositor_client.rs:98-103` uses. Bounded wait; a
/// second request supersedes the first.
///
/// Every failure mode is one `Msg::WallpaperPickerFailed(reason)`, never a
/// panic and never a hang: no session bus, no `org.freedesktop.portal.Desktop`
/// owner, no `FileChooser` interface, a non-zero `response` (user cancelled),
/// a `uris` list that is empty or not a `file://` URI, or the timeout. The
/// `ReloadClient::new`'s `Option<Connection>`-with-`.ok()` pattern
/// (`compositor_reload.rs:38-42`) is the model.
///
/// Success: percent-decode the `file://` URI to a `PathBuf` and send
/// `Msg::WallpaperChosen(path)`.
pub fn spawn(rx: crossbeam_channel::Receiver<PortalRequest>,
             tx: InboxSender<crate::app::Msg>) -> std::thread::JoinHandle<()>;
```

**Fallback, always live.** The `Entry` is not a fallback that appears when the
portal is missing — it is always there and always authoritative.
`Msg::WallpaperEdited(s)` validates: non-empty, path exists, extension in
`{png, jpg, jpeg, webp, bmp}`; a failure sets `wallpaper_error` and leaves
`working.appearance.wallpaper` untouched; a success writes it and clears the
error. `Msg::WallpaperChosen(p)` runs the same validation on the portal's answer.
`Msg::WallpaperPickerFailed(r)` sets `status` to `r` and sets
`portal_available = false`, which greys the Browse button for the rest of the
session.

**Behavior.** Three `switch(bool)` rows (`raise_on_focus`,
`hide_bar_on_fullscreen`, `snap_enabled`) each `.on_toggle(|b| ...)`, plus
`spin_button` for `snap_gap` — the same four controls
`settings/src/pages/behavior.rs:20-87` wires, with the `connect_state_set`
write-backs becoming three `Msg` arms.

**P2 tests.** Kept: `appearance.rs`'s two hex round-trip tests, re-expressed
against the new signatures (`hex_rgba_round_trips`,
`invalid_hex_falls_back_to_black_without_panicking`), plus
`packed_round_trips_through_the_color_dialog_packing`. New (unit):
`wallpaper_validation_rejects_a_missing_file`,
`wallpaper_validation_rejects_an_unknown_extension`,
`a_failed_picker_disables_browse_without_touching_the_model`,
`portal_uri_decoding_handles_percent_escapes`. New (gate, `settings/tests/`):
`appearance_page_paints_every_probe_point_at_rest` and
`behavior_page_paints_every_probe_point_at_rest`, light/dark/hc.
Deleted: `settings/tests/appearance_gtk.rs`.

### 2.7 P3 — Workspaces and Keybindings

**Workspaces.** `box_(Vertical, [list_box(rows), button("Add")])`, one
`list_box_row` per name containing `entry(name).id(format!("ws_name_{i}"))`
`.on_change(move |s| Msg::WorkspaceRenamed(i, s.into()))` and
`button("Remove").id(format!("ws_remove_{i}"))`
`.on_click(Msg::WorkspaceRemoved(i))`, each row `.key(i)`. Remove is
`.sensitive(names.len() > 1)` — the GTK `set_remove_sensitivity` rule, now
computed. `Msg::WorkspaceAdded` pushes `next_workspace_name(&names)`;
`Msg::WorkspaceRemoved(i)` removes and then calls
`prune_orphaned_workspace_bindings(&mut m.model.working)`. The GTK
`RebuildSlot`/`on_workspaces_changed` forward-reference disappears: the
Keybindings page reads `working.workspace_names.len()` on every `view`, so it
cannot go stale.

**Keybindings.** `scrolled_window(box_(Vertical, rows))` with one row per
`action_list(workspace_count)` entry, each `.key(action.clone())`:
`label(action)`, `label(&format_combo(combo))` carrying the `conflict` class
when `m.conflicts` names it, and
`button(if capturing_this { "Press a key…" } else { "Set" })`
`.id(format!("kb_set_{action}"))` `.on_click(Msg::CaptureArmed(action.clone()))`.

Capture is model state plus one window-root `on_key`:

```rust
// settings/src/pages/keybindings.rs
/// The window-root `on_key` handler. `None` means "not ours, let it through".
pub fn capture_key(ev: &icedtea_ui::window::keyboard::KeyEvent) -> Option<crate::app::Msg>;
```

Rules, normative:
1. Returns `None` unless `ev.pressed` and `!ev.repeat`.
2. `Escape` → `Some(Msg::CaptureCancelled)`.
3. Otherwise `Some(Msg::KeyCaptured { keysym: normalise_keysym(ev.base.raw(),
   ev.keysym.raw()), mods: CaptureMods { ctrl, alt, shift, logo } })` read off
   `ev.mods` (`Mods::CTRL`/`ALT`/`SHIFT`/`LOGO`), never `ev.consumed`.
4. `update`'s `Msg::KeyCaptured` arm returns `Cmd::None` and does nothing when
   `m.capturing.is_none()` — the "page visible and armed" gate `should_capture`
   used to enforce, now trivially true because `capturing` is only set from the
   Keybindings page and `Msg::PageSelected` clears it (the `connect_unmap` /
   `EventControllerFocus` reset, now a one-line arm).
5. `model::combo_from_keysym(keysym, mods)` returning `None` (a lone modifier)
   leaves `capturing` set — stay armed and wait for the real key, the GTK
   behaviour verbatim. On `Some(combo)`, insert into
   `working.keybindings[action]`, clear `capturing`, recompute
   `m.conflicts = model::duplicate_bindings(&working)`.
6. Conflicts never block Apply. The `install_conflict_css` GTK provider is
   replaced by a `.conflict` rule in the settings app sheet (§2.9).

**P3 tests.** Kept: `workspaces.rs`'s 6 and `keybindings.rs`'s 6, verbatim after
the move. Re-expressed: `settings/tests/keybindings_gtk.rs` →
`settings/tests/keybindings.rs::shift_a_while_capturing_records_base_a_with_shift`
— a `VirtualKeyboardClient`-driven harness test that does
`key_down(KEY_LEFTSHIFT)`, `key_press(KEY_A)`, `key_up(KEY_LEFTSHIFT)` (the
injector has no `modifiers()` method; modifier state is derived from which
keycodes are down) and asserts through the app's probe report that the stored
combo is `{ modifiers: ["SHIFT"], key: "KEY_a" }`. New:
`a_lone_modifier_press_leaves_the_capture_armed`,
`escape_cancels_a_capture`, `switching_pages_cancels_a_capture`,
`removing_a_workspace_prunes_its_bindings`, plus the rest-state gates
`workspaces_page_paints_every_probe_point_at_rest` and
`keybindings_page_paints_every_probe_point_at_rest`. Deleted:
`settings/tests/keybindings_gtk.rs`.

### 2.8 P4 — Displays

**The canvas.** `drawing_area(draw).id("displays_canvas")` with pointer
handlers:

```rust
// settings/src/pages/displays/canvas.rs
/// The paint callback. `displays_canvas`'s maths is used **verbatim**:
/// `all_rects(st)` -> `compute_view(&rects, rect.width, rect.height,
/// CANVAS_MARGIN)` -> `View::to_canvas` per head. The computed `View` is
/// stashed back into the model by `Msg::HeadDragBegan`'s recomputation, not by
/// the paint (a paint callback may not mutate the model).
///
/// The cairo calls become skia: `cr.set_source_rgb` + `cr.rectangle` +
/// `cr.fill` -> `canvas.draw_rect(&r.to_skia(), &paint::fill_paint(rgba))`;
/// `cr.set_line_width` + `cr.stroke` -> a `Paint` with `set_stroke_width` in
/// `Style::Stroke`; `cr.set_font_size` + `cr.move_to` + `cr.show_text` ->
/// `TextLayout::build(text, &style, cx.fonts, None, WrapMode::None,
/// Ellipsize::End).draw(canvas, origin, colour)` — which is why M5-D6 widened
/// the callback to carry `&mut PaintCx`.
///
/// Colours, dimensions and label layout are the eight literal RGB triples and
/// two label offsets of `settings/src/pages/displays.rs:390-452`, unchanged.
pub fn draw(state: DisplaysSnapshot)
    -> impl Fn(&mut Canvas<'_>, Rect, &mut PaintCx<'_>) + 'static;
```

`DisplaysSnapshot` is a cheap `Clone` of exactly what the paint reads (rects,
labels, enabled flags, selection) — the draw closure is rebuilt each `view` and
must not capture the model.

**Pointer → drag, normative:**

```
on_pointer_down(|x, y| Msg::HeadDragBegan(x, y))
on_pointer_motion(|x, y| Msg::HeadDragged(x, y))
on_pointer_up(|x, y| Msg::HeadDragEnded(x, y))
```

- `HeadDragBegan(x, y)`: recompute `view = compute_view(&rects, w, h,
  CANVAS_MARGIN)` from the current allocation, `hit_test(&rects, &view, x, y)`;
  on a hit set `selected` and `drag = Some(Drag { head, origin: (x, y), start:
  edit.position })`; on a miss clear `drag` (selection is left alone).
- `HeadDragged(x, y)`: with a `drag`, `view.canvas_delta_to_layout(x - origin.0,
  y - origin.1)` → a layout delta; the dragged rect at `start + delta`; then
  `snap(dragged, &others, SNAP_THRESHOLD)` against the **other enabled** rects,
  written into `edits[head].position`; `dirty = true`.
- `HeadDragEnded(..)`: one last `HeadDragged` fold, then `drag = None`.
- A `PointerUp` outside the canvas still arrives (M5-D5 §3 grab semantics), so
  a drag can never wedge.

`hit_test`, `compute_view`, `snap`, `View::to_canvas`,
`View::canvas_delta_to_layout` and `head_rect` are called, never reimplemented.

**Per-head control panel** (`displays/controls.rs`): `switch(enabled)`
`.id("displays_enabled")`; `drop_down(&res_labels).selected(i)`
`.id("displays_resolution")`; `drop_down(&refresh_labels)`
`.id("displays_refresh")`; `drop_down(&TRANSFORM_LABELS)`
`.id("displays_transform")` (all eight variants, finding #8);
`spin_button(scale, 0.5, 4.0).step(0.25).digits(2)` `.id("displays_scale")`;
`label(&position_text)` `.id("displays_position")`. `default_mode_for` is called
whenever a head gains a concrete mode (enable, or a refresh pick) — findings
#9/#14, unchanged. Its own footer: `button("Test")`, `button("Revert")`,
`button("Apply")` and a status `label`; `Test`/`Apply` go out through
`Cmd::Task` on the `OutputsPump` and set `displays_in_flight`.

**The `DropDown` list-height rule — P4 owns it.** M3 P7-D54 embeds a
`DropDown`'s list at a **fixed 240 px** (`ui/src/widgets/drop_down.rs:350`),
which clips inside a 420 px-tall window that also has a switcher and two footers.
P4's fix, in `ui/src/widgets/drop_down.rs`, recorded as a §6 amendment when it
lands:

```rust
// Size the retained list to its content, capped, and let it scroll.
const DROP_DOWN_ROW_PX: f32 = 34.0;      // Adwaita's list row height, hand-resolved
const DROP_DOWN_MAX_PX: u32 = 240;       // the old constant, now a ceiling
let rows = self.items.len() as f32;
let height = (rows * DROP_DOWN_ROW_PX).ceil().clamp(1.0, f32::from(DROP_DOWN_MAX_PX)) as u32;
self.popover.open(PopupAnchorPoint::Node(self.button.clone()),
                  (rect.width.max(1.0) as u32, height), None, cx);
```

A list longer than the cap scrolls (the retained body already sits under a
scrollable subtree); a two-item list is 68 px, not 240. This is the **only** edit
P4 makes under `ui/`, and it must keep
`ui/tests/interaction_gate.rs::opening_a_drop_down_and_picking_an_item_updates_the_button`
green unchanged.

**P4 tests.** Kept verbatim: `displays_canvas.rs`'s 8 and
`displays/state.rs`'s 12. New (unit, no compositor):
`a_drag_that_hits_nothing_leaves_the_selection_alone`,
`a_drag_snaps_the_dragged_head_against_its_neighbour`,
`a_release_outside_the_canvas_ends_the_drag`,
`enabling_a_mode_less_head_gives_it_a_default_mode`,
`an_apply_reply_clears_in_flight_by_its_own_is_test_tag`.
New (gate): `displays_page_paints_every_probe_point_at_rest`;
`dragging_a_head_snaps_it_and_updates_the_model` (interaction, `Driver::drag`
between two `Window::probe_points` labels, asserting the model's rect through the
probe report); `a_colour_pick_changes_the_swatch` (Appearance, interaction);
`apply_reaches_reload_config_on_the_mock` (a recording `ReloadClient` seam);
`a_drop_down_list_fits_inside_the_settings_window` (the P7-D54 regression).

Kept untouched and **not** moved onto the harness:
`settings/tests/live_apply.rs` — it spawns the real `icedtea-compositor` binary
(inherited decision 11), and `settings/tests/outputs_client.rs`.

### 2.9 The settings style sheet

Settings had no `CssProvider` beyond `install_conflict_css`. P3 adds
`settings/style.css` (the one `.conflict { color: #f38ba8; font-weight: bold; }`
rule, plus any page-local spacing a part needs), compiled and layered as a
user-origin overlay over the Adwaita stack — the same mechanism §3.5 describes
for shell, and the same one the theme override uses. It is passed to
`Window::open`'s `sheet` argument, not to `App::with_sheet` (M3 deviation D11:
`with_sheet` only bites for `run_offscreen`).

---

## 3. Shell panel — P5

One `App<PanelModel, Msg>` on one `Role::Layer` window, one process, one sheet.
`shell/src/bridge.rs` is **deleted**.

### 3.1 `PanelModel` and `Msg`

```rust
// shell/src/panel.rs

pub struct PanelModel {
    /// Verbatim from `taskbar.rs`; `apply`/`merge` and their 5 tests unchanged.
    pub taskbar: crate::taskbar::TaskbarModel,
    /// Verbatim from `clipboard.rs`; `apply` and its 1 test unchanged.
    pub clipboard: crate::clipboard::ClipboardModel,
    /// The single source of truth for the clipboard popover.
    pub open_popover: Option<icedtea_ui::window::PopupKey>,
    /// Kept behind the traits so the tests can pass mocks (spec D8).
    pub wm: std::rc::Rc<dyn crate::compositor_client::CompositorCommands>,
    pub clip: std::rc::Rc<dyn crate::clip_client::ClipCommands>,
    pub bar_height: i32,
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// One compositor update, from the inbox. `Arc`, not `Rc`: `Msg` is `Send`.
    Compositor(std::sync::Arc<crate::taskbar::CompositorUpdate>),
    /// One clipboard history update, from the inbox.
    Clip(std::sync::Arc<crate::clipboard::ClipUpdate>),

    WorkspaceClicked(u32),
    WindowClicked(u32),
    /// Every pointer release on a window button; `update` acts only on
    /// `BTN_MIDDLE`. Left releases arrive here too and are ignored — the focus
    /// path is `WindowClicked`, fired by `EventKind::Click`.
    WindowPointerUp { id: u32, button: u32 },

    ClipButtonClicked,
    PopoverOpened(icedtea_ui::window::PopupKey),
    PopoverDismissed(icedtea_ui::window::PopupKey),
    ClipActivated(u64),
    ClipPinToggled { id: u64, pinned: bool },
    ClipRemoved(u64),
    ClipCleared,
}
```

`update: impl FnMut(&mut PanelModel, Msg) -> Cmd<Msg>`:

- `Compositor(u)` → `m.taskbar.apply(Arc::unwrap_or_clone(u))`, `Cmd::None`.
- `Clip(u)` → `m.clipboard.apply(Arc::unwrap_or_clone(u))`, `Cmd::None`.
- `WorkspaceClicked(id)` / `WindowClicked(id)` → `Cmd::Task` calling
  `wm.set_workspace(id)` / `wm.focus_window(id)`.
- `WindowPointerUp { id, button: BTN_MIDDLE }` → `Cmd::Task` calling
  `wm.close_window(id)`; any other button → `Cmd::None`.
- `ClipButtonClicked` → `Cmd::OpenPopup { .. }` (§3.4) when
  `open_popover.is_none()`, else `Cmd::ClosePopup(key)`.
- `PopoverDismissed(key)` → clear `open_popover` if it matches.
- `ClipActivated`/`ClipPinToggled`/`ClipRemoved`/`ClipCleared` → `Cmd::Task`
  onto the `ClipCommands` trait, then `Cmd::ClosePopup` for `ClipActivated`.

The trait objects are `Rc<dyn …>` because they never cross a thread — `update`
runs on the loop thread, and `Cmd::Task` runs there too. That is why `Msg` (which
does cross) carries no trait object.

### 3.2 The layer surface

```rust
const BAR_HEIGHT: i32 = 28;

let spec = SurfaceSpec {
    role: Role::Layer(LayerSpec {
        layer:  zwlr_layer_shell_v1::Layer::Top,
        anchor: zwlr_layer_surface_v1::Anchor::Left
              | zwlr_layer_surface_v1::Anchor::Right
              | zwlr_layer_surface_v1::Anchor::Top,
        margin: [0, 0, 0, 0],
        exclusive_zone: BAR_HEIGHT,
        keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::None,
    }),
    size:   (800, BAR_HEIGHT as u32),
    title:  "icedtea-shell".to_string(),     // the layer namespace (no `namespace` field)
    app_id: "org.icedtea.Shell".to_string(),
};
```

Every value is the gtk4-layer-shell call it replaces
(`shell/src/main.rs:49-84`), with three rulings:

1. **Anchored `Left | Right | Top`, not bottom.** The source comment stands: a
   bottom anchor lands the bar below the visible area on a display whose
   viewport is shorter than the reported output.
2. **`auto_exclusive_zone_enable()` has no `LayerSpec` equivalent** — the field
   is a concrete `i32`, so shell passes `BAR_HEIGHT` (28), the height
   `set_size_request(-1, 28)` forced. Recorded as a §6 deviation.
3. **`keyboard: None`** matches GTK4's unset default; the panel takes no keyboard
   focus and the popover's search field has no IME until M6 (spec §11).

The surface path is `ui/src/gallery.rs:1438-1462`'s idiom exactly:
`SurfaceSpec` → `Window::open` → `App::new(..).with_inbox(..).run(window)`.

### 3.3 `view` decomposition and keys

```rust
pub fn view(m: &PanelModel) -> View<Msg>;
fn workspaces(m: &PanelModel) -> View<Msg>;
fn windows(m: &PanelModel) -> View<Msg>;
fn clip_button(m: &PanelModel) -> View<Msg>;
/// The popover body, built by the `Cmd::OpenPopup` payload closure.
pub fn popover_body(m: &PanelModel) -> View<Msg>;
```

```
box_(Horizontal, [ workspaces(m), windows(m), clip_button(m) ]).id("bar")
```

- `workspaces`: `box_(Horizontal, buttons).id("workspaces")`, one
  `button(&label).key(ws.id)` per `WorkspaceInfo`, label `ws.name` if non-empty
  else `(ws.id + 1).to_string()` (1-indexed display, verbatim),
  `.classes(&["active"])` iff `ws.id == m.taskbar.active_workspace`,
  `.on_click(Msg::WorkspaceClicked(ws.id))`.
- `windows`: `box_(Horizontal, buttons).id("windows")`, one
  `button(&label).key(w.id.0).id(format!("window_{}", w.id.0))` per
  `WindowInfo`, label `w.title` if non-empty else `w.app_id`, classes
  `focused` iff `w.focused` and `attention` iff `w.attention`,
  `.on_click(Msg::WindowClicked(id))` **and**
  `.on_pointer_up_with_button(move |_, _, b| Msg::WindowPointerUp { id, button: b })`.
- `clip_button`: `button("clip").id("clip")` `.on_click(Msg::ClipButtonClicked)`,
  `.classes(&["active"])` while `open_popover.is_some()`.

**Keys are normative** (spec D9, genuinely reactive — clear-and-rebuild was a GTK
workaround): workspace id, window id, history entry id. `hexpand` moves from the
GTK "separate `taskbar_box` child so the clipboard button survives the rebuild"
trick onto `windows`; there is no rebuild to survive.

### 3.4 The clipboard popover

`Msg::ClipButtonClicked` returns:

```rust
Cmd::OpenPopup {
    anchor: PopupAnchorPoint::Node(/* the `clip` button's node */),
    positioner: Positioner::menu(Rect::zero(), (320, 280)),
    view: Rc::new(move || popover_body(&snapshot)),
}
```

`popover_body` is `box_(Vertical, [list_box(rows).id("history"), button("Clear")])`,
one `list_box_row(...).key(entry.id).id(format!("history_{}", entry.id))` per
`ClipEntry` containing `label(&entry.preview)` (halign Start, hexpand, ellipsized)
and `button(if entry.pinned { "unpin" } else { "pin" })`. The `ListBox` carries
`.on_item_activated(|i| Msg::ClipActivated(ids[i]))` — M3 confirms
`ListBoxC` fires `EventKind::Activate` with an index
(`ui/src/widgets/list_box.rs:168`), which is `connect_row_activated`'s exact
equivalent, and it is what replaces `clipboard::connect_activation`. Delete
removes: `Msg::ClipRemoved(id)`. An outside click produces
`InputEvent::PopupDone` → `Msg::PopoverDismissed`, and `open_popover` is the only
place the open/closed state lives.

A popup off a `Layer` root is the best-proven surface path in the crate
(M3 §2, `compositor/tests/popups.rs`), and `Cmd::OpenPopup`'s payload is fully
wired (M3 P7-D54) — `Some(view)` maps a real `xdg_popup`.

### 3.5 IPC and styling

`compositor_client.rs` and `clip_client.rs` are **kept as-is**: `spawn`,
`run`, `version_mismatch`, the `MatchRule`/`MessageStream` dispatch by member
name, the `CompositorProxy`/`ClipProxy` blocking command connections, the
PascalCase methods (`GetState`, `FocusWindow`, `CloseWindow`, `SetWorkspace`,
`GetHistory`, `Activate`, `Pin`, `Remove`, `Clear`) and the lowercase signal
(`history_changed`), and the `Version` property check against
`icedtea_contract::COMPOSITOR_CONTRACT_VERSION`.

`bridge.rs`'s `glib::spawn_future_local` is replaced by one adapter thread per
client, in `shell/src/main.rs`:

```rust
fn forward<T: Send + 'static>(
    rx: async_channel::Receiver<T>,
    tx: icedtea_ui::view::InboxSender<Msg>,
    wrap: impl Fn(T) -> Msg + Send + 'static,
) -> std::thread::JoinHandle<()>;
// while let Ok(v) = rx.recv_blocking() { if tx.send(wrap(v)).is_err() { break } }
```

**The `process::exit(1)` rule survives verbatim.** `compositor_client::spawn`'s
fatal-error path still calls `std::process::exit(1)` — the shell and compositor
marshal contract types independently, a `SignatureMismatch` on `GetState` must
kill the process, and `Restart=always` in
`shell/systemd/icedtea-shell.service` is what replaces it with a matching
build. Logging-and-continuing would leave a permanently stale taskbar that
systemd never restarts. No part may soften this into a `tracing::error!`.

**Styling.** `shell/style.css` (30 lines, five selectors: `#bar`, `#bar button`,
`#workspaces button.active`, `#bar button.focused`, `#bar button.attention`)
compiles to a `CompiledSheet` layered over the Adwaita stack as a **user-origin
overlay** — the mechanism the theme override uses, and the direct replacement for
`style_context_add_provider_for_display` at
`STYLE_PROVIDER_PRIORITY_APPLICATION`. It is handed to `Window::open`'s `sheet`
argument.

```rust
// shell/src/style.rs
/// Compile the bundled sheet on top of the Adwaita stack for `theme`.
///
/// The `.focused`/`.attention` rules use `background-image:
/// linear-gradient(...)` as a left-accent stripe (finding F10 — a border would
/// reflow button metrics). If M2's CSS engine does not resolve that gradient on
/// a `<button>` background layer, P5 records the gap as a §6 deviation and
/// substitutes a `box-shadow: inset` stripe that reflows nothing; it does not
/// change the rule to a border.
pub fn sheet() -> icedtea_ui::css::cascade::CompiledSheet;
```

`shell/src/lib.rs` loses `pub use gtk4;` (it existed only so
`tests/shell_gtk.rs` could build real widgets against a matching crate version).

**`shell/Cargo.toml` after P5:** `gtk4` and `gtk4-layer-shell` removed;
`icedtea-ui = { path = "../ui" }` and `crossbeam-channel.workspace = true`
added; `async-channel`, `futures-util`, `zbus`, `icedtea-contract`, `tracing`,
`tracing-subscriber` kept; dev-deps unchanged.

### 3.6 P5 tests

**Kept verbatim:** `taskbar.rs`'s 5 model tests (`apply`/`merge`, including the
`attention` raise-and-clear round trip and `WorkspaceSet`-only-when-active),
`clipboard.rs`'s 1, `compositor_client.rs`'s `version_mismatch` test,
`shell/tests/live_dbus.rs` (real `ClipProxy` against a real
`icedtea-clipboard` `service::spawn`; the PascalCase-vs-snake_case regression).

**Re-expressed:** `shell/tests/shell_gtk.rs` →
`shell/tests/panel.rs::a_panel_click_reaches_the_command_surface`, under
`icedtea_harness::Compositor` with a `VirtualPointerClient`, the **same**
`MockWm`/`MockClip` recording mocks and the **same** `(action, id)` assertions,
in the same order:

1. seed `Snapshot { windows: [win(1,"One"), win(2,"Two")], workspaces: [{id:0}],
   active_workspace: 0 }` through the inbox; assert the `windows` box's button
   labels are `["One", "Two"]` (read from `Window::probe_points`, not a GTK tree
   walk);
2. click the button labelled `"One"`; assert `("focus", 1)`;
3. `Closed(1)` through the inbox; assert the labels are `["Two"]`;
4. click workspace 0; assert a `"workspace"` entry;
5. seed `History([clip_entry(10,…), clip_entry(11,…)])`; open the popover;
   assert two rows;
6. activate row 0; assert `("activate", 10)`;
7. `History([clip_entry(12,…)])`; assert one row.

Plus the two the GTK test could not reach:
`middle_clicking_a_window_button_closes_it` (`Driver` press/release with
`BTN_MIDDLE`, asserting `("close", 7)`) and
`a_window_opened_signal_through_the_inbox_adds_a_button`.

**New gates:** `panel_paints_every_probe_point_at_rest` (light/dark/hc);
`the_clipboard_popover_opens_pastes_and_dismisses` (open → click a row →
`("activate", id)` → outside click → the popover is gone and `open_popover` is
`None`).

**Deleted:** `shell/tests/shell_gtk.rs`, `shell/src/bridge.rs`,
`taskbar::render`, `clipboard::render`, `clipboard::connect_activation`.

---

## 4. P6 — close-out

### 4.1 Dependency audit

```
cargo tree -p icedtea-settings -p icedtea-shell -e normal
cargo tree -p icedtea-settings -e normal -i gtk4      # must print "package ID not found"
cargo tree -p icedtea-shell    -e normal -i gtk4
```

For each of `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`, `gdk4`, `pango`,
`cairo`, `cairo-rs`, `gobject`, `glib-sys`, `gtk4-sys`: the `-i` invert query must
find no package in either crate's **normal** (non-dev) tree. The workspace-wide
check is:

```
cargo tree -e normal --workspace | grep -Ei '\b(gtk4|gtk4-layer-shell|glib|gio|gdk|pango|cairo)\b'
```

which must be empty. `grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::'
settings/src settings/tests shell/src shell/tests` must also be empty.

### 4.2 Deleted files

```
settings/src/outputs/client.rs        (P1)  glib::Source + gio::Socket integration
settings/src/pages/displays.rs        (P1)  split into pages/displays/{mod,state,controls,canvas}.rs
settings/tests/appearance_gtk.rs      (P2)  property true by construction; §6 record
settings/tests/keybindings_gtk.rs     (P3)  re-expressed as settings/tests/keybindings.rs
shell/src/bridge.rs                   (P5)  glib::spawn_future_local worker bridge
shell/tests/shell_gtk.rs              (P5)  re-expressed as shell/tests/panel.rs
```

And these items, deleted in place: `settings/src/pages/mod.rs`'s `Ctx`, `Page`,
`Ctx::mark_dirty`, `Page::refresh`; `pages/keybindings.rs`'s `unshifted_keysym`,
`install_conflict_css`, `sync_rows`, `reset_capture`; `pages/workspaces.rs`'s
`Row`, `set_remove_sensitivity`; every page's `build()`; `taskbar::render`;
`clipboard::{render, connect_activation}`; `shell/src/lib.rs`'s `pub use gtk4;`.

### 4.3 The M5 deviations record

`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §6 (this file) is the
ledger. P6 appends, in this order, whatever is not already there:

1. `M5-D1 … M5-D11` — P0's amendments, marked landed with their commit.
2. The populate-guard: not ported, true by construction (§2.2).
3. The IME regression: `text-input-v3` is M6 by policy; both apps' text entries
   lose IME at deletion. Stated in the PR body as well as here.
4. The `DropDown` clipping resolution (§2.8's content-sized list, capped).
5. The M3 contract §3 `App::new` amendment (M5-D4), cross-referenced from the M3
   contract's own §10 by a one-line pointer.
6. `auto_exclusive_zone_enable()` → a literal `exclusive_zone: 28` (§3.2).
7. `async-channel` retained in both app crates (not a GTK dependency; keeps
   `outputs/protocol.rs` and the two shell clients unchanged).
8. Anything the parts add, each in the M3 §10 shape: **Carried out by / Added /
   Contract says / As shipped / Ruling**.

### 4.4 Final gate list

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo doc --workspace --no-deps                       # -D warnings
cargo test --workspace
```

and, named explicitly because they are the milestone's inherited gates and must
be green at close-out:

M1/M2: `themed_button_offscreen`, `adwaita_coverage`,
`gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`.
M3: `window_events`, `counter_app`, `node_trees`, `widget_pixels`,
`reconcile_props`, `gallery_gate` (all light/dark/hc), `interaction_gate`
(16/16).
M5 new: `ui/tests/ingress.rs`; settings' five rest-state page gates plus its
interaction gates; `shell/tests/panel.rs` plus the panel rest-state and popover
gates; and the kept-verbatim suites — `settings/tests/outputs_client.rs`,
`settings/tests/live_apply.rs` (real compositor binary),
`shell/tests/live_dbus.rs`.

Screencopy gates cost minutes each: they run **per part** for that part's pages,
and the **full set once** at close-out.

---

## 5. Part boundaries

Dependency order: **P0 → P1 → P2 → P3 → P4 → P5 → P6.** Strictly sequential;
each part's base is the previous part's head. Per-task reviews, whole-part Opus
review and fix wave, then the whole-M5 review and `/lex-review` at the merge
window. **The merge is a consent stop** — the branch is presented as ready and
waits for the owner.

### P0 — toolkit pre-flight
**Owns (ui):** `ui/src/window/{mod,keyboard,pointer}.rs`,
`ui/src/view/{mod,builders,cmd,app}.rs`, `ui/src/view/inbox.rs` (new),
`ui/src/widgets/{color_dialog,check_button,scrollbar,drawing_area}.rs`,
`ui/src/gallery.rs` (the one drawing-area sample), `ui/Cargo.toml`,
`ui/README.md`, `ui/tests/ingress.rs` (new), `ui/tests/gallery_gate.rs`
(the const), `ui/tests/interaction_gate.rs`, `ui/tests/widget_pixels.rs`,
`ui/tests/support/mod.rs`, `ui/src/bin/window-probe.rs`.
**Implements:** §1 (M5-D1 … M5-D11).
**Consumes:** nothing outside M3's shipped `ui/`.
**Must not touch:** `settings/`, `shell/`, `compositor/`, `harness/`, the `wlr`
crate, `ui/src/css/**`, `ui/src/anim/**`, `ui/src/layout.rs`, `ui/src/text.rs`,
any widget not named above, `ui/tests/layer_shell_screencopy.rs` (byte-identical
gate).
**Gate:** §1's per-item tests; every M1/M2/M3 gate green **unchanged**;
`clippy -D warnings` with and without default features; `fmt --check`.
**Review rule (spec §9, binding):** P0's whole-part review **must read the P4
and P5 plans** and confirm each API meets their stated needs — the ingress drain
order against P1/P4's outputs pump, `Handler::PairButton` against P5's
middle-click, the widened `Prop::Draw` against P4's canvas text, and
`Window::probe_points` against every gate. A P0 that ships without that read is
not done.

### P1 — settings skeleton and pure-core extraction
**Owns (settings):** `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/app.rs`
(new), `src/ipc/{mod,reload}.rs` (new), `src/outputs/{mod,pump}.rs`,
`src/pages/mod.rs`, the extraction edits in
`src/pages/{appearance,workspaces,keybindings}.rs`, the
`src/pages/displays.rs` → `src/pages/displays/{mod,state}.rs` split,
`src/pages/displays_canvas.rs` (moved, zero edits), the deletion of
`src/outputs/client.rs`, `settings/style.css`.
**Implements:** §2.1, §2.2 (model/Msg shape), §2.3 (root, nav, footer; pages
stubbed), §2.4, §2.5.
**Consumes:** §1 (M5-D1, D2, D3, D4, D9, D10).
**Must not touch:** `ui/`, `shell/`, `model.rs`, `compositor_reload.rs`,
`outputs/protocol.rs`, `tests/live_apply.rs`, `tests/outputs_client.rs`.
**Gate:** every moved test green **unmodified** (8 + 10 + 1 + 12 + 6 + 6);
the window opens under the harness and its footer/nav respond;
`the_first_toplevel_app_run_paints_and_navigates` (this is `App::run`'s first
`Role::Toplevel` — the named risk); `cargo tree` shows no `gtk4`/`gio`/`glib`.

### P2 — Appearance and Behavior
**Owns (settings):** `src/pages/{appearance,behavior}.rs` view halves,
`src/ipc/portal.rs` (new), the deletion of `tests/appearance_gtk.rs`,
`tests/appearance.rs` (new).
**Implements:** §2.6.
**Consumes:** §1 (D3, D4), §2.1–§2.5.
**Must not touch:** other pages; `ui/`.
**Gate:** §2.6's unit + rest-state list; the portal worker degrades on every
named failure mode without panicking or hanging.

### P3 — Workspaces and Keybindings
**Owns (settings):** `src/pages/{workspaces,keybindings}.rs` view halves,
`settings/style.css`'s `.conflict` rule, `tests/keybindings.rs` (new), the
deletion of `tests/keybindings_gtk.rs`.
**Implements:** §2.7.
**Consumes:** §1 (D5, D7), §2.1–§2.5.
**Must not touch:** other pages; `ui/`.
**Gate:** §2.7's list, including the `VirtualKeyboardClient` Shift+`a` proof and
a mutation check on `normalise_keysym` (return `modified` unconditionally; the
test must fail).

### P4 — Displays
**Owns (settings):** `src/pages/displays/{mod,controls,canvas}.rs`,
`tests/displays.rs` (new). **Plus one `ui/` edit:**
`ui/src/widgets/drop_down.rs`'s list height (§2.8), recorded as a §6 amendment.
**Implements:** §2.8.
**Consumes:** §1 (D5, D6, D9), §2.1–§2.5, `displays_canvas` and
`displays/state` verbatim.
**Must not touch:** `displays_canvas.rs` and `displays/state.rs` (call them,
never edit them); any other `ui/` file.
**Gate:** §2.8's list; `interaction_gate.rs` still 16/16 after the drop-down
change; the twenty moved tests still pass byte-identically.

### P5 — shell panel
**Owns (shell):** `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/panel.rs`
(new), `src/style.rs` (new), the `render`/`connect_activation` deletions in
`src/{taskbar,clipboard}.rs`, the deletion of `src/bridge.rs`,
`tests/panel.rs` (new), the deletion of `tests/shell_gtk.rs`.
**Implements:** §3.
**Consumes:** §1 (D1–D5, D9, D10).
**Must not touch:** `settings/`, `ui/`, `clipboard/`,
`src/{compositor_client,clip_client}.rs`'s worker/proxy bodies (only their
consumers move), `tests/live_dbus.rs`, the systemd unit.
**Gate:** §3.6's list; the `process::exit(1)` path is asserted present by a
source-level test (`the_fatal_worker_path_still_exits`) as well as by review;
`cargo tree` shows no `gtk4`/`gtk4-layer-shell`.

### P6 — close-out
**Owns:** §4's audits, the deletion sweep, this file's §6, `ui/README.md`'s and
the spec's status lines, the PR body.
**Implements:** §4.
**Consumes:** everything.
**Must not touch:** any behaviour — a gate failure at close-out is fixed in the
owning part's fix wave, not in the close-out.
**Gate:** §4.4 in full, once.

### Contract items: implemented vs consumed

| Item | Implemented by | Consumed by |
|---|---|---|
| M5-D1 `watch_fd`/`FdReady`/N-fd poll | P0 | P1 (outputs pump), P5 (inbox pipe) |
| M5-D2 `Inbox`/`with_inbox`/`on_fd` | P0 | P1, P2 (portal), P5 |
| M5-D3 `Cmd::Task` | P0 | P1, P2, P4, P5 |
| M5-D4 `App::new` closures | P0 | P1, P5 |
| M5-D5 pointer events + `PairButton` | P0 | P4 (canvas drag), P5 (middle click) |
| M5-D6 `Prop::Draw` carries `PaintCx` | P0 | P4 |
| M5-D7 `base_keysym` / `KeyEvent::base` | P0 | P3 |
| M5-D8 rest paint, `KNOWN_BLANK` → 6 | P0 | P2 (colour buttons), P3 (scrolled lists) |
| M5-D9 `probe_points`/`allocation` | P0 | every gate in P1–P5 |
| M5-D10 `ui/Cargo.toml` deps | P0 | P0 |
| M5-D11 `ui/README.md` | P0 | P6 |
| §2.1 extracted pure cores | P1 | P2, P3, P4 |
| §2.2 `SettingsModel`/`Msg` | P1 | P2, P3, P4 |
| §2.3 nav + footer + page fns | P1 (frame) | P2, P3, P4 (bodies) |
| §2.4 reload worker | P1 | P2, P4 |
| §2.5 outputs pump | P1 | P4 |
| §2.6 portal worker | P2 | P2 |
| §2.8 `DropDown` list height | P4 | P2 (bar position), P4 |
| §3 `PanelModel`/`Msg`/view/popover | P5 | P6 |
| §4 audits + deviation record | P6 | — |

### Cross-cutting rules (every part)

- Every load-bearing test records a mutation check: break the code the test
  claims to cover, confirm the test fails, restore.
- Timing assertions are generous complexity bounds, never wall-clock pins.
- Untrusted input — themes, icon files, keymaps, portal replies, D-Bus signal
  bodies, config files, output-management heads — **never panics**. A malformed
  value is dropped, logged once, and replaced by the initial/fallback.
- `Duration::ZERO` from any `next_deadline` means "now", never "spin".
- No part changes a signature in this contract without a §6 amendment.
- No part reaches into M6 scope (IME, drag-and-drop, `accesskit`, emoji, portals
  beyond `FileChooser`, `StackSidebar`'s eviction defect, the M2-inherited
  background-layer `currentColor` gap, the compositor's `sh -c` spawn) — not even
  opportunistically, and not "while I was in there".
- Every `Msg` type an app feeds through an inbox is `Send` and carries `Arc`,
  never `Rc`.

---

## 6. Amendments

*(Empty at freeze. Every deviation a part discovers is appended here as
`### <Part>-D<n> — <one-line ruling>` with the conflicting texts quoted, the
ruling, and which part carries it out — the same shape as the M3 contract's §10.
P0's eleven items above are the pre-declared set and are already numbered
`M5-D1 … M5-D11`; a part's own discoveries start at `P1-D1`, `P4-D1`, and so on,
so the two sets never collide.)*

---

## 7. Execution notes

*(Added by the consistency checker after all seven part plans are written, from
a full cross-read for: matched Implements/Consumes, one-part-per-contract-item,
migration and gate agreement, disjoint file ownership, and a
forward-reference-free execution order. **Written 2026-09-03; E1–E14 below are
binding on the parts they name.** Small mismatches were fixed in the part plans
in place and are cross-referenced from the ruling that covers them.)*

Two forward references are already discharged and must not be re-raised:

- **P4 edits one `ui/` file** (`drop_down.rs`, §2.8) although §5 otherwise gives
  `ui/` to P0. This is a closed, enumerated exception: the correct list height
  is only knowable once a real settings page is laid out in a 420 px window, so
  P0 cannot pick it and P4 must. It is recorded in §6 when it lands, and it is
  not a precedent for further app-part edits under `ui/`.
- **P1 is the first `App::run` on a `Role::Toplevel` window.** The raw `Window`
  API is proven there by `ui/tests/window_events.rs`, but `App` has only ever
  driven the `Role::Layer` gallery. P1's skeleton is exactly that, first, before
  a single page is ported, and its gate names the test.


### E1 — the `$ICEDTEA_PROBE_REPORT` writer belongs to P0; no app part writes a second one

M5-D9 says "P1 adds the writer to settings; P5 to shell". P0 supersedes that
with **P0-D4**: `App::with_probe_report(path)` plus an automatic
`$ICEDTEA_PROBE_REPORT` pickup inside `App::run`, emitting M5-D9's exact
`probe <label> <x> <y>` / `alloc <id> <x> <y> <w> <h>` lines once per frame in
which they change. Three plans then claimed the same writer:

* **P1-D4** declared `App::with_probe_report` as a P1 addition to `ui/`.
  **Ruling: P0 owns it.** P1's Task 9 asserts the signature and the line format,
  wires the app, and implements nothing under `ui/` for this item; P1-D4 is
  recorded in §6 as *consumed from P0*, not as landed by P1. (Fixed in P1's
  deviation list.) `Inbox::try_recv` (**P1-D4a**) is unaffected — P0 ships no
  public `try_recv` — and stays a genuine P1 addition.
* **P5-D5** declared `App::on_frame`, checking only "whether P1 already landed
  this". **Ruling: `on_frame` stays P5's** (`with_probe_report` hands the app no
  `&Window`, and P5 needs one for `PanelModel::clip_rect` and for P5-D8's popup
  accessors), but P5's Task 11 must **not** re-emit the window's `probe`/`alloc`
  lines from it: doing so doubles every line in the report. Task 11 is reduced
  to the `clip_rect` publication plus, only if the popover gate needs them,
  P5-D8's popup probe lines. (Fixed in P5 Tasks 4 and 11.)

P3's "Interfaces this part consumes" block, which attributed the report to
`settings/src/main.rs`, was corrected to name `App::run`.

### E2 — `SettingsModel.outputs` is `Option<OutputsPump>` (P1-D2), not `Option<Rc<OutputsPump>>`

§2.5's wiring snippet passes `pump.clone()` into a three-argument
`SettingsModel::new`; §2.2's field list omits the field entirely. **P1-D2
governs**: `pub outputs: Option<crate::outputs::pump::OutputsPump>` (no `Rc` —
`OutputsPump` is `Clone`), set by
`SettingsModel::new(db_path, workers).with_outputs(pump.clone())`. **P4-D7**
declared a different type (`Option<std::rc::Rc<OutputsPump>>`) and a third
constructor argument "where P1 has not". Ruling: P4 consumes P1's shape
verbatim; `m.outputs.clone()` in `displays::submit` compiles against it
unchanged. If P4 finds a different shape on P1's head, it stops and reports
rather than declaring a second one. (Fixed in P4-D7 and P4 Task 7 Step 4.)

P4-D8 (both pump submits return `Result` and are called from `update`) stands:
it is a later part narrowing an earlier part's file it explicitly owns
(`settings/src/outputs/pump.rs` is listed in P4's File Structure), and its
ruling against spec D8 is sound — the call is a wayland request build plus
`flush`, not a blocking D-Bus round trip.

### E3 — the two settings environment knobs are P2's

`$ICEDTEA_UI_THEME` was claimed by both **P2-D7** and **P3-D6**;
`ICEDTEA_SETTINGS_PAGE` by both **P2-D8** and **P4-D9**. P2 runs first, so **P2
owns and lands both**, in P3-D6's spelling for the theme knob (the variable
names a *path to a complete base theme file*, `window-probe.rs`'s convention,
with `settings/style.css` still layered over it as the app origin; the gate
writes `BUNDLED_ADWAITA_{LIGHT,DARK,HC}` to a temp file). P3 and P4 verify and
consume; neither re-lands either knob. (Fixed in P2-D7, P3-D6 and P4-D9.)

### E4 — `a_colour_pick_changes_the_swatch` is P2's gate; P4 keeps only the Apply gate

§2.8 lists both cross-page gates under P4. **P2-D4** moves the colour-pick gate
to P2 and **P4-D12** claimed it as well — a duplicate, and an unbuildable one:
P2-D11 replaces the Appearance swatches with `button_from(drawing_area(..))`,
so P4's version (written against `ColorDialogButtonC` and an
`appearance_accent_dialog` allocation) would assert against widgets that no
longer exist on the page. **Ruling: P2 owns `a_colour_pick_changes_the_swatch`**
in `settings/tests/appearance.rs`; P4 owns
`apply_reaches_reload_config_on_the_mock` in `settings/tests/interaction.rs`.
(Fixed: P4's Task 13 and its file table lost the colour-pick test.)

### E5 — `WorkerHandles` is split P1 (reload) / P2 (portal); there is no P1 portal stub

§2.4 is headed `(P1)` and shows both halves. **P1-D7** ships the `reload` half
only and declares the rest binding on P2; **P2-D1** assumed instead that P1 had
to have left a compiling `ipc/portal.rs` stub. Ruling: **P1-D7 governs.** P1
ships `WorkerHandles { reload }` with `apply`, `ipc::spawn` starting one worker,
and `handles_for_test` returning `(WorkerHandles, Receiver<ReloadRequest>)`. P2
*creates* `settings/src/ipc/portal.rs`, adds the `portal` field and
`choose_wallpaper` with §2.4's exact signatures, extends `ipc::spawn` to start
the portal worker, and extends `handles_for_test`'s tuple with the portal
receiver. §2.4 is the union across P1+P2; no name, argument type or blocking
guarantee in it changes. (Fixed in P2-D1.)

### E6 — four enumerated `ui/` exceptions, not one

§5 gives `ui/` to P0 and §7 discharges exactly one forward reference (P4's
`drop_down.rs`). The written plans add three more. Ruling: all four are
accepted, all four are enumerated and closed, and no further app-part `ui/`
edit may be made without a new §6 amendment:

| Part | `ui/` files | Why P0 cannot |
|---|---|---|
| P1 | `view/inbox.rs` (`Inbox::try_recv`), `tests/ingress.rs` | P1-D4a: only a consumer with a worker round trip needs to observe the inbox without a running `App` |
| P4 | `widgets/drop_down.rs` | §7's pre-discharged exception (P4-D5); the right list height is knowable only from a real page in a 420 px window |
| P5 | `view/app.rs`, `window/popup.rs`, `window/mod.rs`, new `tests/popup_input.rs`, `tests/app_frame_hook.rs` | P5-D1/D2/D4/D5/D8: §3.4's real-`xdg_popup` clipboard popover cannot work without surface-scoped input routing, a re-reconciled popup view, `App::on_popup`, a per-frame `&Window` hook and popup-scoped probe accessors — none of which M5-D1…D11 provides and none of which P0 could specify blind |
| P0 | everything else | — |

**Binding on P6.** Task 5's Step 3 ("every hunk attributable to P0 … or P4's
single `drop_down.rs`") is widened: P1's and P5's `ui/` hunks above are
expected, and `ui/tests/popup_input.rs`, `ui/tests/app_frame_hook.rs` and
`ui/tests/ingress.rs` are expected new files. `ui/tests/layer_shell_screencopy.rs`
stays byte-identical, and the eight untouched M1–M3 gate files stay
byte-identical, exactly as P6 already asserts.

### E7 — P5's popover anchors on a `Rect`, and `PanelModel` gains `clip_rect`

§3.4 spells the open with `PopupAnchorPoint::Node(/* the clip button's node */)`.
P5's tasks all use `PopupAnchorPoint::Rect(clip_rect)` plus a new
`PanelModel::clip_rect: Rc<Cell<Option<Rect>>>` field, published from
`App::on_frame` — a real deviation from §3.1's struct listing and §3.4's open,
and the only one P5's own deviation list missed. Ruling: accepted (`update` has
neither a `&Window` nor a `Node`, so `Node` is unreachable from where the
command is built), the `Rc` is sound because `PanelModel` never crosses a thread
while `Msg` stays `Send`, and it is now recorded as **P5-D9** for P6 to collect.
(Fixed: added to P5's deviation list.)

### E8 — `settings/style.css` is created by P1 and appended to by P3 and P4

§2.9 says "P3 adds `settings/style.css`" while §5 lists the file under P1's
*Owns*. As planned: **P1 creates** it (Task 10) and hands it to `Window::open`'s
`sheet` argument, **P3 adds** the `.conflict` rule (§2.7 rule 6), **P4 appends**
one `#displays_canvas { padding: 0; border: 0 none; margin: 0; }` reset
(P4-D4, which §2.9's "any page-local spacing a part needs" already anticipates).
No part rewrites another part's rules. Nothing to fix; recorded so the three
appenders are not read as three owners.

### E9 — `settings/src/app.rs` is shared by arm group, and the groups are disjoint

§0's module map assigns `app.rs` to `[P1..P4]`; §5's per-part *Owns* lists omit
it for P2–P4, which each declared their own deviation (P2-D12, P3-D2, P4-D6).
Ruling: §0 governs, the three deviations are accepted, and the split is by
**arm group and nothing else**:

* P1 — the struct, `update`'s skeleton, `view`/`nav`/`footer`, and the
  navigation/footer arms (`PageSelected`, `Apply`, `Revert`, `Applied`,
  `ConfigReloaded`, `Outputs` stub).
* P2 — the Appearance and Behavior arms, `ColorSlot`, `SettingsModel::color_picker`.
* P3 — the Workspaces and Keybindings arms, `PageSelected`'s `capturing` reset,
  the root `.on_key` arming, and the two page-`view` call sites.
* P4 — the fourteen Displays arms and the `state` probe lines.

A part that needs to change a line outside its own group stops and reports.
The same rule covers `settings/src/main.rs` (P1 writes it; P2 adds the two env
knobs; P4 touches nothing there once E3 is applied).

Related, and accepted: **P1-D1** builds `Msg` up per part instead of freezing
all of §2.2 at P1. §2.2 stays normative as the **union** across P1–P4 — no
variant's name, payload type or `Send`-ness may change — and the
`assert_send::<Msg>()` static assertion lands with P1 and is re-run by every
later part that adds a variant.

### E10 — settings test support: three homes, deliberately

`settings/tests/support/mod.rs` is created by P1 (P1-D6) and extended by P2
(P2-D2, whose Task 9 signatures are normative for later extenders); P3 keeps its
scaffolding inside `settings/tests/keybindings.rs` (P3-D4); P4 adds a separate
`settings/tests/support/displays.rs` pulled in by `#[path]` (P4-D1). Ruling:
accepted — the three are non-overlapping files and no part edits another's — but
**P4 and P3 must not add a second definition of a name P2's Task 9 already
publishes** in `support/mod.rs`; where the helper already exists, use it.

### E11 — the probe report is a multi-kind, append-only, forward-compatible log

Four rulings pile onto M5-D9's two line kinds and must agree:

* **P1-D5** — append-only, in `frame <n>` blocks, so several writers can share
  one file; readers strip the leading `alloc ` before
  `support::parse_allocation_line` (which takes five whitespace fields).
* **P3-D3** — `binding <action> <mods|-> <key>`, written by
  `pages::keybindings::apply_capture`.
* **P4-D10** — `state <key> <value…>` for `displays.{position,dirty,selected,in_flight}`.
* **P0-D4/E1** — `probe` and `alloc` come from `App::run`, not from the app.

Binding on every reader: **parse by leading keyword and ignore unknown kinds**,
so a part that adds a line kind never breaks another part's driver. Binding on
every writer: append, never truncate; one line per record; no line kind is
removed once a gate reads it.

### E12 — every spec §7 test is owned exactly once

Checked line by line against the seven plans; the map below is the answer, and
no test appears twice or nowhere.

| Spec §7 item | Owner |
|---|---|
| kept: `model.rs` 10, `displays_canvas.rs` 8, `displays.rs` 12 (→ `displays/state.rs`), `workspaces.rs` 6, `keybindings.rs` 6, `compositor_reload.rs` 1 | P1 (moved, unmodified; P4 re-runs the 20 displays tests at its close-out) |
| kept: `settings/tests/{outputs_client,live_apply}.rs` | untouched by every part; re-run by P6 |
| kept: shell `taskbar` 5, `clipboard` 1, `compositor_client` 1, `tests/live_dbus.rs` | P5 (moved/untouched) |
| re-expressed: `shell_gtk.rs` → `shell/tests/panel.rs` | P5 |
| re-expressed: `keybindings_gtk.rs` → `settings/tests/keybindings.rs` | P3 |
| not ported: `appearance_gtk.rs` | P2 (deletes; §2.2's record) |
| rest-state gates: appearance, behavior | P2 |
| rest-state gates: workspaces, keybindings | P3 |
| rest-state gate: displays | P4 |
| rest-state gate: panel | P5 |
| interaction: displays drag-to-snap, drop-down fit | P4 |
| interaction: colour pick | P2 (E4) |
| interaction: Apply reaches `ReloadConfig` | P4 |
| interaction: popover open → paste → dismiss | P5 |
| P0 API tests (`ingress.rs`, pointer/grab, `base_keysym`, rest-paint pixels, live probe) | P0 |
| first `Role::Toplevel` `App::run` | P1 |

One consumer note, not a defect: **M5-D8**'s §5 table lists P2 as a consumer of
the `ColorDialog`/`ColorDialogButton` rest paint. After P2-D11 the Appearance
page no longer uses those widgets, so P0's rest paint for them is exercised only
by `gallery_gate` and `widget_pixels`. M5-D8 still ships in full — the
`KNOWN_BLANK_AT_REST` list must still reach exactly six — and P2's consumption
line simply lapses.

### E13 — gate rules agree across the seven plans

Verified identical in every plan: the M1/M2/M3 green-and-unchanged list
(including `interaction_gate` 16/16 and `layer_shell_screencopy` byte-identical);
`cargo clippy … -D warnings` plus the `icedtea-ui --no-default-features` run
wherever `ui/` is touched (P1 conditions it on touching `ui/`, P3 runs it before
its final commit, P2/P4/P5 run it per task — all acceptable); `cargo fmt --all
--check` before every commit; per-part screencopy gates with the full set once at
close-out; mutation checks recorded on every load-bearing test; untrusted input
never panics; `Msg: Send` with `Arc` never `Rc`; M6 scope refused.

**`settings/tests/live_apply.rs` keeps spawning the real `icedtea-compositor`
binary** (inherited decision 11) is restated by P1, P2, P3, P4 and P6 and
contradicted by none. P5 does not touch `settings/`; the rule reaches it only
through this contract's cross-cutting list, which is enough.

### E14 — execution order 0 → 6 needs no later symbol earlier

Walked in order. Every `Consumes` in P1–P6 names a symbol produced by a strictly
earlier part, after the rulings above are applied: P1 consumes only M5-D1/D2/D3/
D4/D9/D10 (all P0); P2 consumes P0 + P1's §2.1–§2.5; P3 consumes P0 + P1 + P2's
theme knob; P4 consumes P0 + P1's pump/model + P2's env knobs and page ids; P5
consumes P0's toolkit and adds its own `ui/` items before it uses them (Tasks
1–4 precede Task 5); P6 consumes everything and produces no symbol. The only
backward-looking claims in the freeze — P2-D1's "P1 stub", P4-D7's "where P1 has
not", P4-D9's and P3-D6's env knobs, P5-D5's "if P1 landed it" — are the ones
E1–E5 close.
