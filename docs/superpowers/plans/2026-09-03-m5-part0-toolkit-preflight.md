# Pure-Rust GTK-themed UI — M5 Part 0: toolkit pre-flight: event ingress, closures, pointer EventKinds, base keysym, rest paint, live probe — Implementation Plan

> **Note for agentic workers.** This plan is written to be executed by an agent
> with no memory of the conversation that produced it. Every task below is
> self-contained: it names the files it touches, the exact test to write first,
> the command that must fail, the implementation, the command that must then
> pass, and the commit to make. Do not batch tasks, do not reorder them, and do
> not skip the "run it and watch it fail" step — that step is what proves the
> test is testing something. If a step's expected output does not match what
> you see, stop and report rather than adapting the code until it matches.

## Contract deviations

The binding contract is
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §1 (M5-D1 … M5-D11).
Every signature in it is used verbatim except for the items below. Each is
recorded in the M5 contract §6 by the task that lands it, using the amendment
id given here.

- **P0-D1 — pointer handlers fire centrally in `view::app::deliver`, not from
  `GenericC::on_event` plus per-widget forwarding.** M5-D5 §1/§4 say the three
  pointer `EventKind`s are fired from `GenericC::on_event` and that
  `DrawingAreaC::on_event` gains its own forwarding arm. `GenericC` is *not* on
  the dispatch path of a widget that has its own controller: `ButtonC`,
  `DrawingAreaC`, `ListBoxC` and thirty others replace it, so the contract's
  own consumers would not work — P5 puts `on_pointer_up_with_button` on
  `button(...)` nodes (`Kind::Button` ⇒ `ButtonC`), and P5 may not touch `ui/`.
  Shipped: one call to `fire_pointer_handlers(event, handlers)` in `deliver`,
  at `Phase::Target`, before the target node's `controller.on_event`. Every
  kind is covered by one code path, `button.rs` is not edited, and the M5-D5
  mutation check moves from "delete `DrawingAreaC`'s `PointerMotion` arm" to
  "delete the `PointerMotion` arm of `fire_pointer_handlers`".
- **P0-D2 — `Event::PointerMotion` carries button code `0`.** `Handler::PairButton`
  is `Fn(f64, f64, u32)` but `Event::PointerMotion { local }` has no button
  field. A motion fires with `button == 0` ("no button"), documented on
  `fire_pair_button` and in `ui/README.md`. `BTN_LEFT`/`RIGHT`/`MIDDLE` are all
  non-zero, so `0` is unambiguous.
- **P0-D3 — `pointer::BTN_LEFT` is a re-export of `window::layer::BTN_LEFT`, not
  a second definition.** M5-D5 lists three new `pub const`s in
  `ui/src/window/pointer.rs`. `BTN_LEFT` already exists at
  `ui/src/window/layer.rs:248` and is already re-exported as
  `window::BTN_LEFT` (`ui/src/window/mod.rs:17`) and `wayland::BTN_LEFT`. A
  second definition would give `window::BTN_LEFT` two candidates. Shipped:
  `pointer.rs` defines `BTN_RIGHT`/`BTN_MIDDLE` and re-exports `BTN_LEFT`, so
  all three are importable from one module and no existing path moves.
- **P0-D4 — `App::run` writes the `$ICEDTEA_PROBE_REPORT` lines; the app crates
  do not.** M5-D9 says "P1 adds the writer to settings; P5 to shell". Neither
  can: `App::run` owns the loop and lays the tree out into its *own*
  `Runtime::layout`, never into `Window::layout`, so an app that has handed its
  window to `App::run` has no per-frame hook and no laid-out tree to read.
  Shipped: `App::with_probe_report(path)` plus an automatic
  `$ICEDTEA_PROBE_REPORT` pickup inside `App::run`, emitting the contract's
  exact `probe <label> <x> <y>` / `alloc <id> <x> <y> <w> <h>` lines once per
  frame in which they change. `Window::probe_points`/`Window::allocation` ship
  exactly as M5-D9 specifies for raw-`Window` clients (`window-probe.rs`), and
  the labelling code is shared. P1/P5 then get the writer for free and their
  plans' "add the writer" step is "do nothing".
- **P0-D5 — no `unsafe impl Send for InboxSender`.** M5-D2's sketch includes
  `unsafe impl<Msg: Send> Send for InboxSender<Msg> {}`. `crossbeam_channel::Sender<T>`
  is `Send` for `T: Send` and `Arc<OwnedFd>` is `Send + Sync`, so the auto impl
  already holds; an unnecessary `unsafe` block is not written.
- **P0-D6 — `Inbox` keeps its own dup of the pipe's read end.** M5-D2 has
  `App::run` register "the inbox's read end" with `watch_fd`, which takes
  ownership — after which nothing could drain the pipe. Shipped: `Inbox` holds
  the read end and hands `watch_fd` a `try_clone()` of it (CLOEXEC, same open
  file description), so the poll set and the drain both see the same pipe.

Everything else — names, argument order, return types, semantics, test names —
is the contract's.

## Goal

Close every `icedtea-ui` gap M5's app migrations block on, and nothing else.
At the end of this part `icedtea-ui` can:

1. wake its loop from a foreign fd (`Window::watch_fd`, `InputEvent::FdReady`,
   an N-fd `wait_bounded`);
2. take messages from worker threads (`Inbox`/`InboxSender`, `App::with_inbox`,
   `App::on_fd`) with a normative delivery order and zero idle wakeups;
3. run side effects outside `update` (`Cmd::Task`);
4. be built from closures that capture state (`App::new` taking `impl FnMut`/`impl Fn`);
5. turn raw pointer presses, motions and releases into messages at the view
   layer, with the button code where one is needed;
6. draw text from a `DrawingArea` callback (`Prop::Draw` carrying `&mut PaintCx`);
7. report a key's level-0 keysym (`Keymap::base_keysym`, `KeyEvent::base`);
8. paint `ColorDialogButton`, `ColorDialog`, `CheckButton` and `Scrollbar` at
   rest, dropping `KNOWN_BLANK_AT_REST` from nine entries to six;
9. tell a harness test where a widget ended up on a **live** window
   (`Window::probe_points`, `Window::allocation`, and `App`'s
   `$ICEDTEA_PROBE_REPORT` writer).

Non-goals, stated so they are not drifted into: no settings code, no shell
code, no compositor/harness/`wlr` change, no new widget, no M6 scope (IME,
drag-and-drop, `accesskit`, emoji, portals), no fix for the six remaining
`KNOWN_BLANK_AT_REST` widgets, no `StackSidebar` eviction fix, no `DropDown`
list-height change (that is P4's, by the contract's §7 closed exception).

## Architecture

```
                       ┌──────────────────────────────────────────┐
worker thread ───────► │ InboxSender::send(msg)                   │
(zbus, portal, …)      │   ├─ crossbeam channel push (the queue)  │
                       │   └─ write 1 byte to the wake pipe       │
                       └───────────────┬──────────────────────────┘
                                       │ pipe read end (dup)
                                       ▼
   ┌────────────────────────────────────────────────────────────────────┐
   │ Window::pump(timeout)                                              │
   │   wait_bounded(conn, queue, state, timeout, &watches, ready)       │
   │     poll([wayland_fd, watch0, watch1, …])                          │
   │       fds[0] ready → guard.read() → dispatch_pending → wl events   │
   │       fds[n] ready → InputEvent::FdReady(WatchId(n-1))             │
   └────────────────────────────────┬───────────────────────────────────┘
                                    │ Vec<InputEvent>
                                    ▼
   ┌────────────────────────────────────────────────────────────────────┐
   │ App::run, one iteration                                            │
   │  a. inbox: drain pipe, drain channel  → rt.queue (send order)      │
   │  b. FdReady(id) → on_fd(id)()         → rt.queue                   │
   │  c. input batch → route → deliver     → rt.queue                   │
   │       deliver, Phase::Target: fire_pointer_handlers(ev, handlers)  │
   │  d. drain(): fold rt.queue through `update`, run Cmds              │
   │       Cmd::Task(f) => f()   (worker push; never blocks)            │
   │  e. rebuild → restyle_and_layout → paint                           │
   │  f. probe report: probe/alloc lines, when they changed             │
   └────────────────────────────────────────────────────────────────────┘
```

Three invariants carried by tests, not by comments:

- **Order.** inbox → `on_fd` → input batch, then one fold, then one rebuild.
- **No idle spin.** Nothing added here schedules a wakeup; `frame_deadline` is
  untouched, and a watched-but-silent fd costs zero wakeups.
- **Ownership.** `Window` owns every watched fd. The toolkit never decides a
  foreign fd is dead: `HUP`/`ERR` arrive as `FdReady` and the owner calls
  `unwatch`.

## Tech Stack

- Rust edition 2024, `rust-version` 1.94, workspace `Cargo.toml` unchanged.
- `icedtea-ui` deps as today plus (M5-D10): `rustix = { version = "1",
  features = ["fs", "event", "pipe"] }` and `crossbeam-channel.workspace = true`.
- Pinned, unchanged: `wayland-client` 0.31, `wayland-protocols` 0.32,
  `wayland-protocols-wlr` 0.3, `skia-rs-safe` 0.4.0, `taffy` 0.14,
  `xkbcommon` 0.9, `cssparser` 0.37, `selectors` 0.40, `zbus` 5 (workspace),
  `wlr` 0.20.29 (compositor side, not touched here).
- Tests: `cargo test -p icedtea-ui`; harness-driven tests use
  `icedtea-harness` (`Compositor`, `ScreencopyClient`, `VirtualPointerClient`,
  `VirtualKeyboardClient`), already a dev-dependency.

## Spec

- Milestone spec:
  `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
  (§4 is this part; D1–D9 and the eleven inherited decisions are binding).
- Interface contract (normative signatures):
  `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §1, §5 "P0".
- Predecessor contract, still binding except where an `M5-D*`/`P0-D*` amendment
  supersedes it: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md`
  (§3 interfaces, §10 P3-A…P8-D75, §11 E1–E21).
- Research notes: `.superpowers/m5-plan-notes/ui-consumer-api.md`,
  `.superpowers/m5-plan-notes/ipc-portal-loop.md`.

## Global Constraints

- **Pins.** Do not change a dependency version. The only `Cargo.toml` edit in
  this part is M5-D10's two lines in `ui/Cargo.toml`.
- **No GTK.** No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`, `pango` or
  `cairo` anywhere in a migrated app; `icedtea-ui` has never had them and does
  not gain them.
- **Edition 2024, `rust-version` 1.94**, inherited from the workspace.
- **Gates, per crate.** After every task:
  `cargo test -p icedtea-ui`, `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
  `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`,
  `cargo fmt --all --check`.
- **All M1–M3 gates stay green, unchanged:** `themed_button_offscreen`,
  `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`
  (byte-identical file — never edited), `transition_screencopy`,
  `window_events`, `counter_app`, `node_trees`, `widget_pixels`,
  `reconcile_props`, `gallery_gate` (light/dark/hc), `interaction_gate` (16/16).
- **Parts execute in order 0 → 6.** This is part 0; it consumes nothing from a
  later part and may be consumed by all of them.
- **This part changes `icedtea-ui`'s public API.** Every task that does so is a
  contract-amendment task: its commit also appends the matching subsection to
  `ui/README.md` and one record line to the M3 contract's §10 amendment index
  (`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`), pointing at the
  `M5-D*` entry that supersedes it. Nothing lands silently.
- **Commit trailer**, on every commit:
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`
- **Never** `git push`, open a PR, or merge. The merge is the owner's consent stop.

## File Structure

| File | Change | Task |
|---|---|---|
| `ui/Cargo.toml` | `rustix` gains `"pipe"`; `crossbeam-channel.workspace = true` | 3 |
| `ui/src/window/mod.rs` | `WatchId`, `Interest`, watch registry, N-fd `wait_bounded`, `InputEvent::FdReady`, `ProbePoint`, `probe_points_of`, `allocation_of`, `Window::{watch_fd,unwatch,watches,probe_points,allocation}` | 1, 17 |
| `ui/src/window/pointer.rs` | `BTN_RIGHT`, `BTN_MIDDLE`, `BTN_LEFT` re-export | 8 |
| `ui/src/window/keyboard.rs` | `Keymap::base_keysym`, `KeyEvent::base`, `translate` stamps it | 12 |
| `ui/src/window/focus.rs` | test-local `KeyEvent` literal gains `base` | 12 |
| `ui/src/view/inbox.rs` | **new** — `Inbox`, `InboxSender`, `SendError` | 3 |
| `ui/src/view/mod.rs` | `pub mod inbox` + re-exports, three `EventKind`s, `Handler::PairButton`, `Handlers::fire_pair_button`, `Prop::Draw` widening | 3, 8, 10 |
| `ui/src/view/builders.rs` | six `on_pointer_*` builders | 8 |
| `ui/src/view/cmd.rs` | `Cmd::Task` | 7 |
| `ui/src/view/app.rs` | boxed `update`/`view`, `with_inbox`, `on_fd`, `with_probe_report`, drain order, `Cmd::Task` arm, `fire_pointer_handlers`, probe writer | 4, 5, 6, 7, 9, 18 |
| `ui/src/widgets/drawing_area.rs` | `Prop::Draw`/`DrawingAreaC::draw` carry `&mut PaintCx` | 10 |
| `ui/src/widgets/color_dialog.rs` | `ColorDialogButtonC::measure`, `ColorDialogC::{measure,paint}` | 13 |
| `ui/src/widgets/check_button.rs` | unchecked box paints | 14 |
| `ui/src/widgets/scrollbar.rs` | trough + slider paint | 15 |
| `ui/src/gallery.rs` | drawing-area sample: widened draw fn, pointer handlers | 10, 11 |
| `ui/src/bin/window-probe.rs` | `ingress` mode, `--emit-probe` flag | 2, 17 |
| `ui/tests/ingress.rs` | **new** — watch_fd, inbox, `on_fd`, `Cmd::Task`, pointer, `base_keysym` | 2, 5, 6, 7, 9, 12 |
| `ui/tests/support/mod.rs` | `spawn_window_probe_emitting`, button-carrying pointer helpers | 2, 11, 17 |
| `ui/tests/widget_pixels.rs` | `base` in `KeyEvent` ctors, rest-paint pixel tests, `EventKind::ALL` count, drawing-area text | 10, 12, 13, 14, 15 |
| `ui/tests/interaction_gate.rs` | drawing-area drag, scrollbar slider paint-vs-hit | 11, 15 |
| `ui/tests/gallery_gate.rs` | `KNOWN_BLANK_AT_REST` 9 → 6, `the_readme_names_every_known_blank_widget` | 16 |
| `ui/tests/window_events.rs` | live probe-point tests | 17 |
| `ui/README.md` | one subsection per amendment + the blank-list correction | every API task, 19 |
| `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` | §10 record lines | every API task |
| `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` | §6 amendments (P0-D1…P0-D6) | 1, 3, 5, 8, 9, 17, 18 |

Twenty tasks. Files not listed are not touched; in particular
`ui/tests/layer_shell_screencopy.rs`, `ui/src/css/**`, `ui/src/anim/**`,
`ui/src/layout.rs`, `ui/src/text.rs`, `ui/src/widgets/button.rs` and every
widget not named above stay byte-identical.

---

## Task 1 — `WatchId`, `Interest`, the watch registry and the N-fd `wait_bounded` (M5-D1)

**Files**

- `ui/src/window/mod.rs` (edit)
- `ui/README.md` (edit — "External events", first half)
- `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (edit — §10 record line)
- `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (edit — §6, P0-D3 is Task 8's; nothing here)

**Interfaces**

*Consumes:* nothing (first task of the part).

*Produces:*

```rust
// ui/src/window/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interest { Read, Write, ReadWrite }

pub enum InputEvent { /* … every M3 variant … */ FdReady(WatchId) }

impl Window {
    pub fn watch_fd(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId;
    pub fn unwatch(&mut self, id: WatchId);
    #[must_use] pub fn watches(&self) -> Vec<WatchId>;
}
```

### Step 1 — the failing test

Append to `ui/src/window/mod.rs`'s existing `#[cfg(test)] mod tests` (the one
that already holds the `wait_bounded` tests around line 2913):

```rust
    #[test]
    fn interest_maps_onto_poll_flags() {
        use rustix::event::PollFlags;
        assert_eq!(super::Interest::Read.flags(), PollFlags::IN);
        assert_eq!(super::Interest::Write.flags(), PollFlags::OUT);
        assert_eq!(
            super::Interest::ReadWrite.flags(),
            PollFlags::IN | PollFlags::OUT
        );
    }

    #[test]
    fn a_watch_set_polls_the_wayland_fd_first_and_the_watches_in_registration_order() {
        // The poll set's shape is the whole of M5-D1's semantics §1: element 0
        // is always the connection, elements 1.. are the watches as registered.
        // Building it is a pure function of the registry, so it is tested
        // without a compositor; `ui/tests/ingress.rs` proves the live wake.
        use rustix::event::PollFlags;
        let (a_read, _a_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK)
                .expect("a pipe");
        let (b_read, _b_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK)
                .expect("a pipe");
        let mut watches = super::Watches::default();
        let first = watches.add(a_read, super::Interest::Read);
        let second = watches.add(b_read, super::Interest::ReadWrite);
        assert_eq!(watches.ids(), vec![first, second]);
        assert_ne!(first, second, "ids are never reused inside one window");
        let flags: Vec<PollFlags> = watches.entries().map(|w| w.flags).collect();
        assert_eq!(flags, vec![PollFlags::IN, PollFlags::IN | PollFlags::OUT]);

        watches.remove(first);
        assert_eq!(watches.ids(), vec![second]);
        // Removing an id twice, or one that was never issued, is a no-op.
        watches.remove(first);
        watches.remove(super::WatchId(4242));
        assert_eq!(watches.ids(), vec![second]);
    }
```

Run it:

```bash
cargo test -p icedtea-ui --lib window::tests::a_watch_set_polls_the_wayland_fd_first_and_the_watches_in_registration_order
```

**Expected failure:** compile error, `cannot find type `Watches` in module
`super``, plus `cannot find type `Interest``, `failed to resolve: could not
find `pipe` in `rustix``. The last one is M5-D10's feature; it is added in
Task 3, so for this task add only the `"pipe"` feature to
`ui/Cargo.toml`'s `rustix` line (the `crossbeam-channel` half stays with
Task 3):

```toml
rustix = { version = "1", features = ["fs", "event", "pipe"] }
```

Re-run: the two `cannot find` errors remain. That is the failure this task fixes.

### Step 2 — the implementation

In `ui/src/window/mod.rs`, above `wait_bounded`:

```rust
/// Opaque handle for one fd registered with [`Window::watch_fd`].
///
/// Ids are minted per window and never reused, so a stale id from a dropped
/// watch can never name a later one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

/// What a watch waits for.
///
/// `Read` is readability, `Write` writability, `ReadWrite` either. `HUP` and
/// `ERR` are always reported whatever the interest, and always arrive as an
/// [`InputEvent::FdReady`] — the toolkit never decides on its own that a
/// foreign fd is dead. It reports, and the owner calls [`Window::unwatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interest {
    /// Readable, or hung up.
    Read,
    /// Writable.
    Write,
    /// Either.
    ReadWrite,
}

impl Interest {
    fn flags(self) -> rustix::event::PollFlags {
        use rustix::event::PollFlags;
        match self {
            Interest::Read => PollFlags::IN,
            Interest::Write => PollFlags::OUT,
            Interest::ReadWrite => PollFlags::IN | PollFlags::OUT,
        }
    }
}

/// One registered fd: the window owns it until [`Window::unwatch`] drops it.
struct Watch {
    id: WatchId,
    fd: std::os::fd::OwnedFd,
    flags: rustix::event::PollFlags,
}

/// Every live watch, in registration order.
#[derive(Default)]
struct Watches {
    entries: Vec<Watch>,
    next: u64,
}

impl Watches {
    fn add(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId {
        let id = WatchId(self.next);
        self.next += 1;
        self.entries.push(Watch {
            id,
            fd,
            flags: interest.flags(),
        });
        id
    }

    /// Drop the watch and close its fd. An unknown id is a no-op, not a panic:
    /// an owner that unwatches twice is not a protocol error (M5-D1 §4).
    fn remove(&mut self, id: WatchId) {
        self.entries.retain(|w| w.id != id);
    }

    fn ids(&self) -> Vec<WatchId> {
        self.entries.iter().map(|w| w.id).collect()
    }

    fn entries(&self) -> impl Iterator<Item = &Watch> {
        self.entries.iter()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
```

Add the variant to `InputEvent`, at the end of the enum so no existing
variant's position changes:

```rust
    /// A watched fd is ready.
    ///
    /// Carries no readiness flags on purpose: the owner of the fd is the only
    /// thing that knows what to do with it, and re-encoding `revents` here
    /// would invite the toolkit to interpret a foreign protocol. One event per
    /// ready watch per [`Window::pump`] batch, in registration order, after
    /// whatever Wayland events the same wake dispatched.
    FdReady(WatchId),
```

Rewrite `wait_bounded` to take the watch set. The loop shape is M3's,
line for line, with three insertions marked `M5-D1`:

```rust
fn wait_bounded(
    conn: &Connection,
    queue: &mut EventQueue<WindowState>,
    state: &mut WindowState,
    timeout: Duration,
    // M5-D1: the extra fds this window's owner registered. Empty for
    // `Window::open`, which has no owner yet.
    watches: &Watches,
    ready: impl Fn(&WindowState) -> bool,
) -> Result<(), SurfaceError> {
    use rustix::event::{PollFlags, Timespec};
    use std::os::fd::AsFd;

    let deadline = Instant::now().checked_add(timeout);
    // M5-D1: watches that fired on the *previous* poll, published after the
    // dispatch below so a batch reads "Wayland events first, then FdReady",
    // which is the order M5-D1 §3 fixes.
    let mut fired: Vec<WatchId> = Vec::new();
    loop {
        queue
            .dispatch_pending(state)
            .map_err(SurfaceError::Dispatch)?;
        state
            .events
            .extend(fired.drain(..).map(InputEvent::FdReady));
        if state.closed {
            return Err(SurfaceError::Closed);
        }
        if ready(state) {
            return Ok(());
        }

        let now = Instant::now();
        let remaining = match deadline {
            Some(deadline) if now >= deadline => return Err(SurfaceError::Timeout(timeout)),
            Some(deadline) => Some(deadline - now),
            None => None,
        };

        conn.flush().map_err(socket_error)?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };

        let fd = queue.as_fd();
        // M5-D1: element 0 is always the Wayland queue's fd; elements 1..
        // are the watches in registration order, which is what makes
        // `fds[index + 1]` name `watches.entries().nth(index)`.
        let mut fds = Vec::with_capacity(1 + watches.entries().count());
        fds.push(rustix::event::PollFd::new(&fd, PollFlags::IN));
        for watch in watches.entries() {
            fds.push(rustix::event::PollFd::new(&watch.fd, watch.flags));
        }
        let timespec = remaining.map(|remaining| Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        });
        match rustix::event::poll(&mut fds, timespec.as_ref()) {
            Ok(0) => return Err(SurfaceError::Timeout(timeout)),
            Ok(_) => {
                // M5-D1: a wake caused only by a watch must not read the
                // Wayland socket — `guard.read()` on a socket with nothing to
                // say is at best a wasted syscall and at worst a block.
                // Dropping the guard cancels the read intent.
                if fds[0].revents().is_empty() {
                    drop(guard);
                } else {
                    match guard.read() {
                        Ok(_) => {}
                        // A racing reader on another queue drained the socket.
                        Err(wayland_client::backend::WaylandError::Io(err))
                            if err.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(err) => return Err(socket_error(err)),
                    }
                }
                // M5-D1 §2/§3: one `FdReady` per ready watch, whatever bits
                // are set (`HUP`/`ERR` included, which poll reports without
                // being asked). Published at the top of the next iteration.
                for (index, watch) in watches.entries().enumerate() {
                    if !fds[index + 1].revents().is_empty() {
                        fired.push(watch.id);
                    }
                }
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(SurfaceError::Socket(err.into())),
        }
    }
}
```

`Ok(0)` stays `Timeout`; a wake caused only by a watch reaches the top of the
loop, publishes its `FdReady`, and `ready(state)` — `!state.events.is_empty()`
for `pump` — is then true, so `pump` returns the batch rather than timing out
(M5-D1 §2).

Give `Window` the registry. In the struct, after `texts`:

```rust
    /// Foreign fds this window polls alongside its own connection (M5-D1).
    watches: Watches,
```

Initialise it in `Window::open`'s constructor literal with
`watches: Watches::default(),`, and fix the two existing `wait_bounded` call
sites:

- `Window::open`'s (mod.rs ~1035): pass `&Watches::default()` — the window is
  not built yet and nothing can have registered.
- `Window::pump`'s: pass `&self.watches`.

Then the three public methods, next to `Window::root`:

```rust
    /// Register `fd` in this window's poll set.
    ///
    /// The window takes ownership so the caller cannot close the fd out from
    /// under the loop; [`Window::unwatch`] is what closes it. Readiness is
    /// reported as [`InputEvent::FdReady`] from [`Window::pump`], and nothing
    /// is read from the fd by the toolkit — that is the owner's job.
    pub fn watch_fd(&mut self, fd: std::os::fd::OwnedFd, interest: Interest) -> WatchId {
        self.watches.add(fd, interest)
    }

    /// Drop a watch and close its fd. An unknown id is a no-op.
    pub fn unwatch(&mut self, id: WatchId) {
        self.watches.remove(id);
    }

    /// Every live watch, in registration order.
    #[must_use]
    pub fn watches(&self) -> Vec<WatchId> {
        self.watches.ids()
    }
```

`Watches::is_empty` is used by Task 18's report writer; if clippy flags it as
dead code before then, add it in Task 18 instead — do not add `#[allow]`.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --lib window::
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the two new tests pass; every existing `window::` test still
passes (`pump` behaviour is unchanged with an empty watch set); clippy clean.

### Step 4 — docs and the record

`ui/README.md`, a new subsection at the end of "## The window and event layer":

```markdown
### Watching a foreign fd

A window polls its own Wayland connection *and* any fd its owner registers:

```rust
let id = window.watch_fd(fd, Interest::Read);   // the window owns `fd` now
// … `window.pump(..)` now yields `InputEvent::FdReady(id)` when it is ready …
window.unwatch(id);                              // drops the watch, closes the fd
```

Element 0 of the poll set is always the connection, so a Wayland wake is never
starved by a chatty watch; `FdReady`s come after the Wayland events of the same
wake, one per ready watch, in registration order. `HUP` and `ERR` are reported
as readiness whatever the `Interest`: the toolkit never decides a foreign fd is
dead, it tells the owner, who calls `unwatch`. Nothing here adds a timer, so a
registered-but-silent fd costs zero wakeups.
```

M3 contract §10, appended at the end of the amendment list:

```markdown
### M5-D1 — `wait_bounded` polls N fds, not one (superseded §3.1)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §3.1's single-`PollFd`
`wait_bounded` is replaced by a set whose element 0 is the Wayland queue and
whose rest are `Window::watch_fd` registrations, reported as
`InputEvent::FdReady(WatchId)`. Full text:
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §1 M5-D1.
```

### Step 5 — commit

```bash
git add ui/Cargo.toml ui/src/window/mod.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): poll N fds, and report foreign readiness as InputEvent::FdReady

M5-D1. `wait_bounded` builds its poll set from the Wayland queue plus the
watches `Window::watch_fd` registered, in registration order; a ready watch
becomes one `InputEvent::FdReady(WatchId)` per pump batch, published after the
Wayland events of the same wake. A wake caused only by a watch cancels the
read guard rather than reading a socket with nothing to say, and is not a
timeout. `unwatch` drops the watch and closes the fd; an unknown id is a no-op.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — `window-probe`'s `ingress` mode and the live `watch_fd` tests (M5-D1)

**Files**

- `ui/src/bin/window-probe.rs` (edit)
- `ui/tests/support/mod.rs` (edit)
- `ui/tests/ingress.rs` (new)

**Interfaces**

*Consumes:* Task 1's `Window::{watch_fd, unwatch, watches}`, `Interest`,
`InputEvent::FdReady`.

*Produces:*

```rust
// ui/tests/support/mod.rs
#[must_use]
pub fn spawn_window_probe_with(
    socket: &str,
    mode: &str,
    theme: &std::path::Path,
    report: &std::path::Path,
    args: &[&str],
) -> Reaper;
```

and `window-probe`'s `ICEDTEA_PROBE_MODE=ingress`, which reports:

```
watch-registered <id>
fd-ready <id>
wakes <n>            # once, after the idle window closes
unwatched <id>
fd-quiet             # no FdReady arrived after the unwatch
```

### Step 1 — the failing test

New file `ui/tests/ingress.rs`:

```rust
//! M5 P0's ingress surface: `Window::watch_fd`, the inbox, `App::on_fd`,
//! `Cmd::Task`, the view-layer pointer events and `Keymap::base_keysym`.
//!
//! Live-`Window` tests drive `window-probe` under the harness compositor,
//! because `Window::open` reads `WAYLAND_DISPLAY` from the process
//! environment and a test process cannot set that per-test (edition 2024
//! makes `set_var` unsafe, and the tests in this binary run in parallel).
//! Everything that does not need a surface runs offscreen, in-process.

mod support;

use std::time::Duration;

use icedtea_harness::Compositor;
use support::{probe_report, probe_theme, spawn_window_probe_with, wait_for_report_line};

/// How long a probe gets to boot, map and report. The gallery gate's own
/// `GALLERY_MAP_TIMEOUT` documents why this is seconds rather than
/// milliseconds: a first anti-aliased paint is slow in a debug build.
const REPORT: Duration = Duration::from_secs(20);

#[test]
fn a_watched_pipe_wakes_pump_with_fd_ready() {
    // mutation: drop the `for (index, watch) in watches.entries()` loop from
    // `wait_bounded`'s `Ok(_)` arm; nothing ever pushes `FdReady` and this
    // test times out on `fd-ready`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    let registered = wait_for_report_line(report.path(), "watch-registered ", REPORT)
        .expect("the probe never registered its pipe");
    let id = registered
        .split_whitespace()
        .nth(1)
        .expect("the line carries the watch id")
        .to_string();
    let ready = wait_for_report_line(report.path(), "fd-ready ", REPORT)
        .expect("a byte on the watched pipe never woke pump");
    assert_eq!(
        ready.split_whitespace().nth(1),
        Some(id.as_str()),
        "the FdReady names a different watch: {ready}"
    );
}

#[test]
fn an_unwatched_fd_stops_waking_the_loop() {
    // mutation: make `Watches::remove` a no-op; the probe keeps reporting
    // `fd-ready` after the unwatch and never reports `fd-quiet`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-quiet", REPORT).is_some(),
        "the probe still saw readiness after unwatching: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let unwatched = lines
        .iter()
        .position(|l| l.starts_with("unwatched "))
        .expect("the probe unwatched");
    let quiet = lines
        .iter()
        .position(|l| l == "fd-quiet")
        .expect("the probe reported quiet");
    assert!(quiet > unwatched, "the quiet window must follow the unwatch");
    assert!(
        !lines[unwatched..].iter().any(|l| l.starts_with("fd-ready ")),
        "an FdReady arrived after the unwatch: {lines:?}"
    );
}

#[test]
fn two_watched_fds_report_in_registration_order() {
    // mutation: reverse `Watches::entries()`; the ids arrive swapped.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--two-watches"],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-pair ", REPORT).is_some(),
        "the probe never saw both fds ready in one batch: {:?}",
        probe_report(report.path())
    );
    let line = probe_report(report.path())
        .into_iter()
        .find(|l| l.starts_with("fd-pair "))
        .expect("the pair line");
    let ids: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    assert_eq!(ids.len(), 2, "the pair line carries two ids: {line}");
    assert!(
        ids[0] < ids[1],
        "FdReady must arrive in registration order, got {ids:?}"
    );
}

#[test]
fn a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout() {
    // A window with a registered but silent watch must still configure, paint
    // and report frames exactly as `window_events.rs` expects of one without.
    // mutation: return `SurfaceError::Timeout` whenever any watch is
    // registered; `frame ` never appears.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    assert!(
        wait_for_report_line(report.path(), "configure ", REPORT).is_some(),
        "a window with a silent watch never configured"
    );
    assert!(
        wait_for_report_line(report.path(), "frame ", REPORT).is_some(),
        "a window with a silent watch never painted a frame"
    );
}

#[test]
fn unwatching_an_unknown_id_is_a_no_op() {
    // mutation: `panic!` in `Watches::remove` for an id it does not hold; the
    // probe dies before reporting `unwatch-unknown-ok`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "unwatch-unknown-ok", REPORT).is_some(),
        "unwatching an unknown id was not survivable: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn an_idle_watch_costs_no_extra_wakeups() {
    // A complexity bound, not a wall-clock pin (contract §5 cross-cutting
    // rule): over a one-second idle window with a registered, silent watch,
    // `pump` must return far fewer times than a spin would produce. A spin
    // returns thousands; the frame clock alone returns tens.
    // mutation: add `Duration::ZERO` to `frame_deadline` whenever a watch is
    // registered; `wakes` climbs past the bound and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    let line = wait_for_report_line(report.path(), "wakes ", REPORT)
        .expect("the probe never closed its idle window");
    let wakes: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|f| f.parse().ok())
        .expect("the wakes line carries a count");
    assert!(
        wakes < 500,
        "an idle window with one silent watch woke {wakes} times in a second; \
         that is a spin, not a poll"
    );
}
```

Run:

```bash
cargo test -p icedtea-ui --test ingress
```

**Expected failure:** compile error — `cannot find function
`spawn_window_probe_with` in module `support``.  After Step 2's support
addition but before the probe change, the tests fail on
`the probe never registered its pipe` (the `ingress` mode does not exist, so
`window-probe` runs its default `entry` mode and reports no watch lines).

### Step 2 — the implementation

`ui/tests/support/mod.rs`, beside `spawn_window_probe` (keep that function
exactly as it is — `window_events.rs` calls it):

```rust
/// `spawn_window_probe`, plus extra argv flags.
///
/// The probe's modes are environment-driven and its *variants within a mode*
/// are argv-driven, so a test can ask for two watches or a silent one without
/// a new mode name.
#[must_use]
pub fn spawn_window_probe_with(
    socket: &str,
    mode: &str,
    theme: &std::path::Path,
    report: &std::path::Path,
    args: &[&str],
) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_window-probe"))
            .args(args)
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_PROBE_MODE", mode)
            .env("ICEDTEA_PROBE_REPORT", report)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn window-probe"),
    )
}
```

`ui/src/bin/window-probe.rs`: after the existing `mode` binding, add the
ingress branch. It runs its own loop and returns, so the entry/menu loop below
is untouched.

```rust
    let two_watches = std::env::args().any(|a| a == "--two-watches");
    let silent_watch = std::env::args().any(|a| a == "--silent-watch");
    if mode == "ingress" {
        run_ingress(window, two_watches, silent_watch);
        return;
    }
```

and, as a free function at the end of the file:

```rust
/// `ICEDTEA_PROBE_MODE=ingress`: exercise `Window::watch_fd`/`unwatch` against
/// a real compositor and report what the loop saw.
///
/// The pipes are written from a helper thread rather than from the loop, which
/// is the shape a real worker has: the loop must learn about them only through
/// `poll`.
fn run_ingress(mut window: Window, two_watches: bool, silent_watch: bool) {
    use icedtea_ui::window::Interest;
    use rustix::pipe::{PipeFlags, pipe_with};
    use std::time::{Duration, Instant};

    // An id the window cannot have live: registered, then immediately
    // retired. `WatchId` is opaque and ids are never reused, so it stays dead
    // and `unwatch`ing it twice is the "unknown id" case M5-D1 §4 names.
    let (scratch_read, _scratch_write) =
        pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("scratch pipe");
    let stale = window.watch_fd(scratch_read, Interest::Read);
    window.unwatch(stale);
    let before = window.watches().len();
    window.unwatch(stale);
    if window.watches().len() == before {
        report("unwatch-unknown-ok");
    }

    let (read_a, write_a) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("pipe a");
    let id_a = window.watch_fd(read_a, Interest::Read);
    report(&format!("watch-registered {}", id_index(id_a)));
    let mut id_b = None;
    let mut write_b = None;
    if two_watches {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("pipe b");
        let id = window.watch_fd(read, Interest::Read);
        report(&format!("watch-registered {}", id_index(id)));
        id_b = Some(id);
        write_b = Some(write);
    }

    // The writer: one byte on each pipe, a little after the loop starts, so
    // the wake is a real poll wake and not a leftover from before it.
    if !silent_watch {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            let _ = rustix::io::write(&write_a, b"x");
            if let Some(write_b) = write_b {
                let _ = rustix::io::write(&write_b, b"x");
            }
        });
    }

    let started = Instant::now();
    let mut wakes: u64 = 0;
    let mut seen_ready = false;
    let mut unwatched = false;
    let mut quiet_from: Option<Instant> = None;
    let mut frames = 0_u32;
    loop {
        if window.render().is_err() {
            return;
        }
        // A bounded wait even when nothing is scheduled, so the idle-wake
        // count below is a count of *this* loop's polls, not of a block.
        let timeout = Some(
            window
                .next_deadline()
                .unwrap_or(Duration::from_millis(50))
                .min(Duration::from_millis(50)),
        );
        let Ok(events) = window.pump(timeout) else {
            return;
        };
        wakes += 1;
        let mut batch_ready: Vec<u64> = Vec::new();
        for event in events {
            match event {
                InputEvent::Configure { size, states } => {
                    report(&format!("configure {} {} {states:?}", size.0, size.1));
                }
                InputEvent::Frame { now } => {
                    if frames < 3 {
                        frames += 1;
                        report(&format!("frame {}", now.as_micros()));
                    }
                }
                InputEvent::FdReady(id) => {
                    batch_ready.push(id_index(id));
                    report(&format!("fd-ready {}", id_index(id)));
                }
                InputEvent::Close => return,
                _ => {}
            }
        }
        if batch_ready.len() == 2 {
            report(&format!("fd-pair {} {}", batch_ready[0], batch_ready[1]));
        }
        if !batch_ready.is_empty() {
            seen_ready = true;
        }
        // Once the readiness has been observed, unwatch *without* draining the
        // pipe: a still-readable fd that no longer wakes the loop is the whole
        // point of the assertion.
        if seen_ready && !unwatched {
            window.unwatch(id_a);
            if let Some(id) = id_b {
                window.unwatch(id);
            }
            unwatched = true;
            report(&format!("unwatched {}", id_index(id_a)));
            quiet_from = Some(Instant::now());
        }
        if let Some(from) = quiet_from
            && from.elapsed() > Duration::from_millis(500)
        {
            report("fd-quiet");
            quiet_from = None;
        }
        if silent_watch && started.elapsed() > Duration::from_secs(1) {
            report(&format!("wakes {wakes}"));
            return;
        }
        if unwatched && quiet_from.is_none() && !silent_watch {
            report(&format!("wakes {wakes}"));
            return;
        }
        if window.is_closed() {
            return;
        }
    }
}

/// A `WatchId`'s report form. `WatchId` is opaque, so the probe prints its
/// `Debug` payload — the only stable, orderable thing a consumer can quote.
fn id_index(id: icedtea_ui::window::WatchId) -> u64 {
    format!("{id:?}")
        .trim_start_matches("WatchId(")
        .trim_end_matches(')')
        .parse()
        .unwrap_or(u64::MAX)
}
```

Add `use icedtea_ui::window::{InputEvent, Interest, Role, SurfaceSpec, Window};`
to the file's imports (extending the existing `window::{…}` line).

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui --test window_events
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the six ingress tests pass; `window_events` is unaffected (the
default `entry` mode is untouched).

**Mutation check (M5-D1's, run once, by hand, and restore):** delete
`fds.push(rustix::event::PollFd::new(&fd, PollFlags::IN));` — the element-0
push — from `wait_bounded`, then run `cargo test -p icedtea-ui --test
window_events`. It must fail (nothing dispatches Wayland events any more, so
no `configure` line is ever reported). Restore.

### Step 4 — commit

```bash
git add ui/src/bin/window-probe.rs ui/tests/support/mod.rs ui/tests/ingress.rs
git commit -m "$(cat <<'EOF'
test(ui): drive watch_fd against a live compositor from window-probe

M5-D1's live half: an `ingress` probe mode registers pipes, reports every
`InputEvent::FdReady`, unwatches without draining, and counts its wakes over a
one-second idle window. Six tests in the new `ui/tests/ingress.rs` pin the
wake, the registration order, the survival of an unknown `unwatch`, the fact
that a watch does not turn a Wayland wake into a timeout, and the absence of
idle spin (a complexity bound, not a wall-clock pin).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — `Inbox<Msg>` / `InboxSender<Msg>` (M5-D2 first half, M5-D10)

**Files:** `ui/Cargo.toml`, `ui/src/view/inbox.rs` (new), `ui/src/view/mod.rs`,
`ui/README.md`, `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6: P0-D5, P0-D6).

**Interfaces**

*Consumes:* nothing from earlier tasks (`rustix`'s `"pipe"` feature landed in Task 1).

*Produces:*

```rust
// ui/src/view/inbox.rs
pub struct Inbox<Msg> { /* rx, tx, read: OwnedFd */ }
pub struct InboxSender<Msg> { /* tx, write: Arc<OwnedFd> */ }
#[derive(Debug)] pub struct SendError<Msg>(pub Msg);

impl<Msg: Send + 'static> Inbox<Msg> {
    pub fn new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)>;
    #[must_use] pub fn sender(&self) -> InboxSender<Msg>;
}
impl<Msg> Inbox<Msg> {
    /// The fd `App::run` hands to `Window::watch_fd` — a `try_clone` of the
    /// read end, so the inbox keeps its own (P0-D6).
    pub(crate) fn watch_fd(&self) -> std::io::Result<std::os::fd::OwnedFd>;
    /// Read the wake pipe empty. Readiness, not payload: the channel is the queue.
    pub(crate) fn drain_pipe(&self);
    /// Every queued message, in send order, without blocking.
    pub(crate) fn drain_into(&self, out: &mut std::collections::VecDeque<Msg>);
}
impl<Msg> InboxSender<Msg> {
    pub fn send(&self, msg: Msg) -> Result<(), SendError<Msg>>;
}
impl<Msg> Clone for InboxSender<Msg> { .. }

// ui/src/view/mod.rs
pub mod inbox;
pub use inbox::{Inbox, InboxSender, SendError};
```

### Step 1 — the failing test

New `ui/src/view/inbox.rs`, tests only for now (the module's body is Step 2):

```rust
#[cfg(test)]
mod tests {
    use super::Inbox;
    use std::collections::VecDeque;

    #[test]
    fn a_sent_message_is_queued_in_send_order_and_wakes_the_pipe() {
        // mutation: drop the `rustix::io::write` from `send`; `drain_pipe`
        // reads nothing and the readiness assertion below fails.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        tx.send(1).expect("the inbox is alive");
        tx.send(2).expect("the inbox is alive");
        let mut poll = [rustix::event::PollFd::new(
            &inbox.read,
            rustix::event::PollFlags::IN,
        )];
        let ready = rustix::event::poll(&mut poll, Some(&rustix::event::Timespec::default()))
            .expect("poll");
        assert_eq!(ready, 1, "a send must make the read end readable");
        let mut queue: VecDeque<u32> = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![1, 2]);
        let ready = rustix::event::poll(&mut poll, Some(&rustix::event::Timespec::default()))
            .expect("poll");
        assert_eq!(ready, 0, "drain_pipe must leave the pipe empty");
    }

    #[test]
    fn a_sender_survives_being_cloned_across_threads() {
        // mutation: make `Clone` clone only the channel and not the pipe
        // handle; the cloned sender's writes go nowhere and this deadlocks
        // the readiness assertion in the test above rather than here — so the
        // assertion here is on ordering across threads, which still holds.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        let handles: Vec<_> = (0..4)
            .map(|n| {
                let tx = tx.clone();
                std::thread::spawn(move || tx.send(n).expect("the inbox is alive"))
            })
            .collect();
        for handle in handles {
            handle.join().expect("the worker thread finished");
        }
        let mut queue = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        let mut got: Vec<u32> = queue.into_iter().collect();
        got.sort_unstable();
        assert_eq!(got, vec![0, 1, 2, 3]);
    }

    #[test]
    fn sending_after_the_inbox_is_dropped_returns_the_message() {
        // mutation: `unwrap` the channel send; this panics instead of
        // handing the message back.
        let (inbox, tx) = Inbox::<String>::new().expect("a pipe");
        drop(inbox);
        let err = tx.send("late".to_owned()).expect_err("the inbox is gone");
        assert_eq!(err.0, "late", "the message comes back rather than vanishing");
    }

    #[test]
    fn a_full_wake_pipe_does_not_block_a_sender() {
        // A worker must never block on the loop: the byte already in the pipe
        // is enough to wake it, and the channel is the real queue (M5-D2 §4).
        // mutation: drop `PipeFlags::NONBLOCK` from `pipe_with`; this hangs.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        for n in 0..200_000 {
            tx.send(n).expect("the inbox is alive");
        }
        let mut queue = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        assert_eq!(queue.len(), 200_000, "every message survives a full pipe");
    }
}
```

```bash
cargo test -p icedtea-ui --lib view::inbox
```

**Expected failure:** `file not found for module `inbox`` until `pub mod inbox;`
is added, then `cannot find struct `Inbox`` / `unresolved import
`crossbeam_channel``.

### Step 2 — the implementation

`ui/Cargo.toml`, in `[dependencies]` (the `rustix` half is already Task 1's):

```toml
crossbeam-channel.workspace = true
```

`ui/src/view/inbox.rs`, above the test module:

```rust
//! External messages: the channel plus wake pipe an `App` drains once a frame.
//!
//! A worker thread holds an [`InboxSender`]; the loop holds the [`Inbox`].
//! `send` does two things — push the message onto an unbounded channel, then
//! write one byte to a non-blocking pipe whose read end the window polls. The
//! channel is the queue; the byte is only a wake, which is why a full pipe is
//! not an error (M5-D2 §4).

use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

/// The receiving half: what an [`App`](crate::view::App) drains once per frame.
pub struct Inbox<Msg> {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    read: OwnedFd,
    write: Arc<OwnedFd>,
}

/// The sending half. `Send + Clone`, so a worker thread — or several — can
/// hold one.
///
/// `Send` is the auto impl: `crossbeam_channel::Sender<T>` is `Send` for
/// `T: Send` and `Arc<OwnedFd>` is `Send + Sync` (P0-D5 — the contract's
/// sketch had an `unsafe impl` that is not needed).
pub struct InboxSender<Msg> {
    tx: Sender<Msg>,
    write: Arc<OwnedFd>,
}

/// `send` failed because every [`Inbox`] is gone; the message comes back.
#[derive(Debug, PartialEq, Eq)]
pub struct SendError<Msg>(pub Msg);

impl<Msg> std::fmt::Display for SendError<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the inbox is closed")
    }
}

impl<Msg: std::fmt::Debug> std::error::Error for SendError<Msg> {}

impl<Msg: Send + 'static> Inbox<Msg> {
    /// Build a connected pair. The pipe is `CLOEXEC | NONBLOCK`.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] if the pipe cannot be created.
    pub fn new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)> {
        let (read, write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )?;
        let (tx, rx) = crossbeam_channel::unbounded();
        let write = Arc::new(write);
        let sender = InboxSender {
            tx: tx.clone(),
            write: Arc::clone(&write),
        };
        Ok((Inbox { rx, tx, read, write }, sender))
    }

    /// Another sender onto the same inbox.
    #[must_use]
    pub fn sender(&self) -> InboxSender<Msg> {
        InboxSender {
            tx: self.tx.clone(),
            write: Arc::clone(&self.write),
        }
    }
}

impl<Msg> Inbox<Msg> {
    /// The fd to register with [`Window::watch_fd`](crate::window::Window::watch_fd).
    ///
    /// A `try_clone`, not the read end itself: `watch_fd` takes ownership, and
    /// the inbox still has to read the pipe to drain it (P0-D6). Both handles
    /// share one open file description, so a byte read through either empties
    /// it for both.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] if the fd cannot be duplicated.
    pub(crate) fn watch_fd(&self) -> std::io::Result<OwnedFd> {
        self.read.try_clone()
    }

    /// Read the wake pipe empty, ignoring what was in it.
    pub(crate) fn drain_pipe(&self) {
        let mut buf = [0_u8; 256];
        loop {
            match rustix::io::read(&self.read, &mut buf[..]) {
                Ok(0) => return,
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {}
                // `WOULDBLOCK` is the empty pipe: the normal exit.
                Err(_) => return,
            }
        }
    }

    /// Every queued message, in send order, without blocking.
    pub(crate) fn drain_into(&self, out: &mut VecDeque<Msg>) {
        while let Ok(msg) = self.rx.try_recv() {
            out.push_back(msg);
        }
    }
}

impl<Msg> InboxSender<Msg> {
    /// Queue `msg` and wake the loop.
    ///
    /// # Errors
    ///
    /// [`SendError`] — carrying `msg` back — once every [`Inbox`] is dropped,
    /// which is how a worker learns the app exited.
    pub fn send(&self, msg: Msg) -> Result<(), SendError<Msg>> {
        self.tx.send(msg).map_err(|err| SendError(err.0))?;
        // One byte. An `EAGAIN` on a full pipe is deliberately ignored: the
        // byte already in it wakes the loop just as well, and blocking here
        // would block a worker on the UI thread (M5-D2 §4).
        let _ = rustix::io::write(&*self.write, b"\0");
        Ok(())
    }
}

impl<Msg> Clone for InboxSender<Msg> {
    fn clone(&self) -> Self {
        InboxSender {
            tx: self.tx.clone(),
            write: Arc::clone(&self.write),
        }
    }
}

impl<Msg> std::fmt::Debug for Inbox<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox").finish_non_exhaustive()
    }
}

impl<Msg> std::fmt::Debug for InboxSender<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxSender").finish_non_exhaustive()
    }
}
```

`ui/src/view/mod.rs`, with the other module declarations and re-exports:

```rust
pub mod inbox;
```
```rust
pub use inbox::{Inbox, InboxSender, SendError};
```

### Step 3 — run it

```bash
cargo test -p icedtea-ui --lib view::inbox
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
```

**Expected:** four tests pass. Neither new dependency is optional, so the
`--no-default-features` build is identical.

### Step 4 — docs and the record

`ui/README.md`, new subsection at the end of "## Reactive framework (`view/`)":

```markdown
### External events

A worker thread reaches the loop through an inbox:

```rust
let (inbox, tx) = Inbox::new()?;                  // tx: Send + Clone
std::thread::spawn(move || { tx.send(Msg::Reloaded)?; Ok::<_, SendError<Msg>>(()) });
App::new(model, update, view).with_inbox(inbox).run(window)?;
```

`send` pushes onto an unbounded channel and writes one byte to a wake pipe the
window polls; a full pipe is not an error, because the byte already in it wakes
the loop and the channel is the queue. A `send` after the app exits returns the
message rather than panicking. `Msg` must be `Send`, which means a message
carries `Arc<T>`, never `Rc<T>`.
```

M5 contract §6: append `### P0-D5` and `### P0-D6` with the texts from this
plan's "Contract deviations" section.

### Step 5 — commit

```bash
git add ui/Cargo.toml ui/src/view/inbox.rs ui/src/view/mod.rs ui/README.md \
        docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): add Inbox/InboxSender, a channel plus wake pipe for worker threads

M5-D2's transport half, and M5-D10's two dependency lines. `send` queues on an
unbounded crossbeam channel and writes one byte to a CLOEXEC|NONBLOCK pipe; a
full pipe is ignored, a dropped inbox hands the message back, and the inbox
keeps its own dup of the read end so the poll set and the drain see one pipe
(P0-D6). No unsafe Send impl is needed (P0-D5).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — `App::new` takes closures (M5-D4)

**Files:** `ui/src/view/app.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§10 record).

**Interfaces**

*Consumes:* nothing.

*Produces:*

```rust
pub struct App<M, Msg> {
    model: M,
    update: Box<dyn FnMut(&mut M, Msg) -> Cmd<Msg>>,
    view: Box<dyn Fn(&M) -> View<Msg>>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    pub fn new(
        model: M,
        update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static,
        view: impl Fn(&M) -> View<Msg> + 'static,
    ) -> Self;
}
```

### Step 1 — the failing test

In `ui/tests/counter_app.rs` (the file that already proves `fn` items work),
append:

```rust
/// M5-D4: an `update` may capture. Both M5 apps need it — settings holds
/// worker senders, shell holds `Rc<dyn CompositorCommands>` so a test can
/// swap the mock in.
///
/// mutation: change `App::new`'s `update` parameter back to
/// `fn(&mut M, Msg) -> Cmd<Msg>`; this stops compiling ("expected fn
/// pointer, found closure").
#[test]
fn a_closure_capturing_state_drives_the_loop() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&seen);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        0_u32,
        move |model: &mut u32, msg: u32| {
            *model += msg;
            recorder.borrow_mut().push(*model);
            Cmd::None
        },
        |model: &u32| label(&model.to_string()),
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen(
        (120, 40),
        clock,
        vec![ScriptStep::Message(2), ScriptStep::Message(3), ScriptStep::Capture],
    )
    .expect("the offscreen app runs");
    assert_eq!(frames.len(), 1);
    assert_eq!(*seen.borrow(), vec![2, 5], "the closure kept its capture");
}

/// The widening must not break the `fn`-item call sites: the gallery, this
/// file's own counter and every widget test pass plain `fn`s.
///
/// mutation: make `App::new` take `Box<dyn FnMut…>` directly instead of
/// `impl FnMut…`; every existing call site stops compiling.
#[test]
fn an_fn_item_still_coerces_into_app_new() {
    fn update(model: &mut u32, msg: u32) -> Cmd<u32> {
        *model += msg;
        Cmd::None
    }
    fn view(model: &u32) -> View<u32> {
        label(&model.to_string())
    }
    let app = App::new(7_u32, update, view);
    assert_eq!(*app.model(), 7);
}
```

Import whatever the file does not already have (`label` from
`icedtea_ui::view::builders`, `ManualClock`, `CompiledSheet`,
`BUNDLED_ADWAITA_LIGHT`, `ScriptStep`, `Cmd`, `View`).

```bash
cargo test -p icedtea-ui --test counter_app
```

**Expected failure:** `expected fn pointer `fn(&mut u32, u32) -> Cmd<u32>`,
found closure` on the first test.

### Step 2 — the implementation

In `ui/src/view/app.rs`:

```rust
pub struct App<M, Msg> {
    model: M,
    /// Boxed, not a `fn` pointer (M5-D4): both M5 apps capture — settings a
    /// `WorkerHandles`, shell an `Rc<dyn CompositorCommands>` its tests swap.
    /// `FnMut`, because an `update` may own counters and senders.
    update: Box<dyn FnMut(&mut M, Msg) -> Cmd<Msg>>,
    /// `Fn`, not `FnMut`: `view` runs during reconcile while the model is
    /// borrowed, and must not mutate.
    view: Box<dyn Fn(&M) -> View<Msg>>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
}
```

```rust
    #[must_use]
    pub fn new(
        model: M,
        update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static,
        view: impl Fn(&M) -> View<Msg> + 'static,
    ) -> Self {
        App {
            model,
            update: Box::new(update),
            view: Box::new(view),
            sheet: None,
            fonts: None,
            icons: None,
        }
    }
```

Call sites inside `app.rs` that need parenthesising or a re-borrow:

- `drain`: `let cmd = (app.update)(&mut app.model, msg);` — already
  parenthesised; `update` is now `FnMut`, and `app.update`/`app.model` are
  disjoint fields, so the borrow checker is satisfied unchanged.
- `rebuild`: `let view = (app.view)(&app.model);` — unchanged.
- `App::probe`: `let App { model, view, .. } = self;` then `view(&model)` becomes
  `(view)(&model)`; the binding is now a `Box<dyn Fn…>`, which calls the same way.

Nothing else in the crate names the fields. `Debug` is unchanged
(`finish_non_exhaustive`).

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test counter_app
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** both new tests pass and **no other file changes** — the gallery
(`App::new(model, update, scrolled_page)`), `widget_pixels`'s `run` helper (a
`fn`-pointer signature) and every controller test compile untouched. That "no
other file changed" is M5-D4's proof; if any call site needs editing, stop:
the widening is wrong.

### Step 4 — docs and the record

`ui/README.md`, "## Reactive framework (`view/`)", after the `App::new` example:

```markdown
`App::new` takes `impl FnMut(&mut M, Msg) -> Cmd<Msg>` and `impl Fn(&M) ->
View<Msg>`, so an `update` can capture worker senders or a swappable command
trait object. Plain `fn` items still coerce, which is what the gallery and the
counter test use.
```

M3 contract §10:

```markdown
### M5-D4 — `App::new` takes closures, not `fn` pointers (amends §4.7)

**Carried out by:** M5 P0. **Added:** 2026-09-03. §4.7's
`new(model, update: fn(..), view: fn(..))` (this file, l. 1516-1521) becomes
`impl FnMut`/`impl Fn`, a strict widening: every `fn`-item call site compiles
unchanged. Full text: M5 contract §1 M5-D4.
```

### Step 5 — commit

```bash
git add ui/src/view/app.rs ui/tests/counter_app.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): App::new takes impl FnMut/impl Fn, so update can capture

M5-D4, amending the M3 contract §4.7 signature by name. `update` is boxed
`FnMut` (it may own worker senders), `view` boxed `Fn` (it runs while the model
is borrowed). A strict widening: no existing `fn`-item call site changed.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — `App::with_inbox` and the drain order (M5-D2 second half)

**Files:** `ui/src/view/app.rs`, `ui/tests/ingress.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* Task 1 (`Window::watch_fd`, `Interest`, `InputEvent::FdReady`),
Task 3 (`Inbox`), Task 4 (boxed `App` fields).

*Produces:*

```rust
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Feed `inbox`'s messages into this app's loop. At most one inbox per
    /// app; a second call replaces the first.
    #[must_use] pub fn with_inbox(self, inbox: Inbox<Msg>) -> Self where Msg: Send;
}
```

with the normative drain order of M5-D2 §3: **(a)** pipe drained, **(b)**
channel drained into `rt.queue` in send order, **(c)** `on_fd` messages
(Task 6), **(d)** the input batch, then the single fold.

### Step 1 — the failing test

Append to `ui/tests/ingress.rs`:

```rust
use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::view::builders::label;
use icedtea_ui::view::{App, Cmd, Inbox, ScriptStep, View};
use icedtea_ui::window::InputEvent;
use std::rc::Rc;

/// What the ordering tests fold: every message appends its own tag, so the
/// model *is* the delivery order.
#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Inbox(u32),
    Clicked,
}

fn tag_update(model: &mut Vec<String>, msg: Tag) -> Cmd<Tag> {
    match msg {
        Tag::Inbox(n) => model.push(format!("inbox{n}")),
        Tag::Clicked => model.push("click".to_owned()),
    }
    Cmd::None
}

fn tag_view(model: &Vec<String>) -> View<Tag> {
    label(&model.join(",")).on_click(Tag::Clicked).id("target")
}

#[test]
fn inbox_messages_are_applied_in_send_order() {
    // mutation: drain the channel with `rx.recv().into_iter().rev()`; the
    // order flips and this fails.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    for n in 0..3 {
        tx.send(Tag::Inbox(n)).expect("the inbox is alive");
    }
    let (frames, model) = run_tagged(inbox, vec![ScriptStep::Capture]);
    assert_eq!(frames.len(), 1);
    assert_eq!(model, vec!["inbox0", "inbox1", "inbox2"]);
}

#[test]
fn inbox_messages_are_applied_before_the_frames_input_batch() {
    // The property P1's reload worker and P5's compositor client depend on:
    // state that arrived from a worker is folded before the click that the
    // same frame delivers, so `update` never sees a click against a stale
    // model (M5-D2 §3).
    //
    // mutation: move the inbox drain *after* the `for event in &events`
    // routing loop in `App::run`/`run_offscreen`; the order becomes
    // ["click", "inbox9"] and this fails.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    tx.send(Tag::Inbox(9)).expect("the inbox is alive");
    let (_, model) = run_tagged(
        inbox,
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(20.0, 10.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(
        model,
        vec!["inbox9", "click"],
        "the inbox message must be folded before the frame's input batch"
    );
}

#[test]
fn an_inbox_message_reaches_update_on_the_next_frame() {
    // A message sent *after* the app started still arrives: the pipe is what
    // makes the loop look.
    // mutation: never register the inbox fd in `App::run`; the live probe
    // test `an_app_inbox_message_reaches_update` (Task 6) fails, and here the
    // offscreen drain still runs, so this test's value is the send-then-step
    // sequencing rather than the wake.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    let sender = tx.clone();
    let (_, model) = run_tagged(
        inbox,
        vec![ScriptStep::Capture, {
            sender.send(Tag::Inbox(1)).expect("alive");
            ScriptStep::Capture
        }],
    );
    assert_eq!(model, vec!["inbox1"]);
}
```

plus the shared helper at the end of the file — the app is built *inside* it,
so the recorder is captured by the real `update` (M5-D4 is what allows that):

```rust
/// Build an app whose `update` mirrors the fold order into `seen`, run it
/// offscreen, and hand back the frames plus that order.
fn run_tagged(
    inbox: Inbox<Tag>,
    script: Vec<ScriptStep<Tag>>,
) -> (icedtea_ui::view::Frames, Vec<String>) {
    let seen: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = Rc::clone(&seen);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        Vec::<String>::new(),
        move |model: &mut Vec<String>, msg: Tag| {
            let cmd = tag_update(model, msg);
            recorder
                .borrow_mut()
                .push(model.last().cloned().unwrap_or_default());
            cmd
        },
        tag_view,
    )
    .with_inbox(inbox)
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen((160, 40), clock, script)
    .expect("the offscreen app runs");
    let order = seen.borrow().clone();
    (frames, order)
}
```

The three tests above call `run_tagged(inbox, script)`; write them that way —
they build no `App` of their own.

```bash
cargo test -p icedtea-ui --test ingress
```

**Expected failure:** `no method named `with_inbox` found for struct `App``.

### Step 2 — the implementation

`ui/src/view/app.rs`. Two more fields on `App` (the `fd_handlers` one is
Task 6's; add it now so the struct is written once):

```rust
    /// External messages (M5-D2). At most one per app.
    inbox: Option<crate::view::inbox::Inbox<Msg>>,
    /// `FdReady(id)` → messages (M5-D2's `on_fd`).
    #[allow(clippy::type_complexity, reason = "one boxed closure per watched fd")]
    fd_handlers: Vec<(crate::window::WatchId, Box<dyn Fn() -> Vec<Msg>>)>,
```

initialised `inbox: None, fd_handlers: Vec::new(),` in `App::new`, and:

```rust
    /// Feed `inbox`'s messages into this app's loop.
    ///
    /// At most one inbox per app; a second call replaces the first. `Msg: Send`
    /// is what makes a worker thread able to hold the sender, and is why an M5
    /// message carries `Arc<T>` and never `Rc<T>`.
    #[must_use]
    pub fn with_inbox(mut self, inbox: crate::view::inbox::Inbox<Msg>) -> Self
    where
        Msg: Send,
    {
        self.inbox = Some(inbox);
        self
    }
```

The drain itself, factored so `run` and `run_offscreen` cannot disagree:

```rust
/// M5-D2 §3 steps (a) and (b): read the wake pipe empty, then move every
/// queued message onto the fold queue **in send order**.
///
/// Called once per loop iteration, before any input is routed, so a message a
/// worker produced is folded against the same model the frame's clicks are.
fn drain_inbox<M, Msg>(app: &App<M, Msg>, rt: &mut Runtime<Msg>) {
    let Some(inbox) = app.inbox.as_ref() else {
        return;
    };
    inbox.drain_pipe();
    inbox.drain_into(&mut rt.queue);
}
```

In `App::run`, register the fd before the first `pump` (right after
`rebuild(..)`):

```rust
        // M5-D2 §2: the inbox's wake pipe joins the window's poll set, and
        // leaves it on the way out.
        let inbox_watch = match self.inbox.as_ref().map(crate::view::inbox::Inbox::watch_fd) {
            Some(Ok(fd)) => Some(window.watch_fd(fd, crate::window::Interest::Read)),
            Some(Err(err)) => {
                tracing::warn!(%err, "the inbox pipe could not be registered; \
                                      its messages will only be seen on other wakeups");
                None
            }
            None => None,
        };
```

and immediately after `let events = window.pump(wait)?;`:

```rust
            // M5-D2 §3: inbox first, then `on_fd`, then the input batch.
            drain_inbox(&self, &mut rt);
            for event in &events {
                if let InputEvent::FdReady(id) = event {
                    for (watch, handler) in &self.fd_handlers {
                        if watch == id {
                            rt.queue.extend(handler());
                        }
                    }
                }
            }
```

(The `fd_handlers` loop is Task 6's behaviour; it is inert until `on_fd` exists,
and writing it here keeps the drain order in one commit.)

After the `while` loop, before returning:

```rust
        if let Some(id) = inbox_watch {
            window.unwatch(id);
        }
```

In `run_offscreen`, at the top of the `for step in script` body:

```rust
            // The same drain, at the same point: an ingress test needs no
            // compositor (M5-D2 §5).
            drain_inbox(&self, &mut rt);
```

Borrowing note: `drain_inbox(&self, &mut rt)` takes `&App` while `rt` is a
separate local, so it composes with the `&mut self` uses later in the
iteration; do not inline it into a block that already holds `&mut self`.

`InputEvent::FdReady` needs no arm in `route` — its `_ => {}` wildcard already
ignores it, which is M5-D1 §5's "falls through every match".

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui --test counter_app
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the three ordering tests pass; `counter_app` and every offscreen
widget test are unaffected (no inbox ⇒ `drain_inbox` returns immediately).

**Mutation check (M5-D2's):** move the `drain_inbox(&self, &mut rt)` call in
`App::run_offscreen` to *after* the step's `ScriptStep::Event` routing;
`inbox_messages_are_applied_before_the_frames_input_batch` must fail with
`["click", "inbox9"]`. Restore.

### Step 4 — docs and the record

`ui/README.md`, extend the "External events" subsection from Task 3:

```markdown
Delivery order inside one frame is normative: the inbox's messages are folded
first, in send order, then any `on_fd` messages, then the frame's input batch —
so a click never runs against a model that has not yet seen the worker update
that arrived on the same wake. `run_offscreen` drains at the same point, which
is what makes an ingress test compositor-free.
```

M3 contract §10: `### M5-D2 — `App` grows an external-message hook (§4.7 had
none)`, one paragraph, pointing at the M5 contract.

### Step 5 — commit

```bash
git add ui/src/view/app.rs ui/tests/ingress.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): App::with_inbox, with a normative per-frame drain order

M5-D2. `run` registers the inbox's wake pipe with `Window::watch_fd` and
unwatches it on exit; every iteration drains the pipe, then the channel into
the fold queue in send order, then maps `FdReady` through `on_fd`, then routes
the input batch. `run_offscreen` drains at the same point, so the ordering
tests need no compositor.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6 — `App::on_fd` and the live inbox probe (M5-D2)

**Files:** `ui/src/view/app.rs`, `ui/src/bin/window-probe.rs`,
`ui/tests/ingress.rs`.

**Interfaces**

*Consumes:* Tasks 1, 3, 5.

*Produces:*

```rust
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Map a foreign fd's readiness to messages. `f` runs on the loop thread
    /// whenever `InputEvent::FdReady(id)` arrives; its `Vec<Msg>` is enqueued
    /// in order, after the inbox's messages and before the input batch.
    #[must_use]
    pub fn on_fd(self, id: crate::window::WatchId, f: impl Fn() -> Vec<Msg> + 'static) -> Self;
}
```

plus `window-probe`'s `ICEDTEA_PROBE_MODE=app-inbox`, which runs a real
`App` on a live `Role::Toplevel` window with an inbox and an `on_fd` watch and
reports `folded <tag>` lines.

### Step 1 — the failing test

`ui/tests/ingress.rs`:

```rust
#[test]
fn on_fd_maps_a_foreign_fd_to_messages() {
    // mutation: drop the `for (watch, handler) in &self.fd_handlers` loop from
    // `App::run`; the probe never reports `folded fd`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "app-inbox",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "folded fd", REPORT).is_some(),
        "an `on_fd` closure never produced a message: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn an_inbox_message_wakes_a_live_app() {
    // The end-to-end claim P1 and P5 rest on: a worker thread's `send` reaches
    // `update` on a running app with no polling and no timer.
    // mutation: never call `window.watch_fd` for the inbox in `App::run`; the
    // app only notices on some *other* wakeup, and with nothing else moving
    // the report line never appears inside the timeout.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "app-inbox",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "folded inbox", REPORT).is_some(),
        "a worker's message never reached update: {:?}",
        probe_report(report.path())
    );
}
```

```bash
cargo test -p icedtea-ui --test ingress -- on_fd_maps_a_foreign_fd_to_messages
```

**Expected failure:** the probe runs its `entry` mode (unknown mode name falls
through), reports nothing matching, and the assertion fails on the timeout.

### Step 2 — the implementation

`ui/src/view/app.rs`:

```rust
    /// Map a foreign fd's readiness to messages.
    ///
    /// `f` runs on the loop thread whenever [`InputEvent::FdReady`] names `id`,
    /// and its messages are enqueued in order — after the inbox's, before the
    /// frame's input batch. A handler for an id that has since been
    /// `unwatch`ed is simply never called again (M5-D1 §4).
    #[must_use]
    pub fn on_fd(mut self, id: crate::window::WatchId, f: impl Fn() -> Vec<Msg> + 'static) -> Self {
        self.fd_handlers.push((id, Box::new(f)));
        self
    }
```

`ui/src/bin/window-probe.rs`, a second new mode beside `ingress`:

```rust
    if mode == "app-inbox" {
        run_app_inbox(window);
        return;
    }
```

```rust
/// `ICEDTEA_PROBE_MODE=app-inbox`: a real `App` on this live toplevel, fed by
/// a worker thread through an inbox and by a foreign fd through `on_fd`.
///
/// This is the shape both M5 apps have, reduced to what can be asserted from
/// outside: every folded message is reported, and the app quits once it has
/// seen both.
fn run_app_inbox(mut window: Window) {
    use icedtea_ui::view::builders::label;
    use icedtea_ui::view::{App, Cmd, Inbox, View};
    use icedtea_ui::window::Interest;
    use rustix::pipe::{PipeFlags, pipe_with};
    use std::time::Duration;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        FromInbox,
        FromFd,
    }

    let (inbox, tx) = Inbox::<Msg>::new().expect("an inbox");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        let _ = tx.send(Msg::FromInbox);
    });

    let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("a pipe");
    let watch = window.watch_fd(read, Interest::Read);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        let _ = rustix::io::write(&write, b"x");
    });

    let app = App::new(
        (false, false),
        |model: &mut (bool, bool), msg: Msg| {
            match msg {
                Msg::FromInbox => {
                    model.0 = true;
                    report("folded inbox");
                }
                Msg::FromFd => {
                    model.1 = true;
                    report("folded fd");
                }
            }
            if model.0 && model.1 {
                report("folded both");
                Cmd::Quit
            } else {
                Cmd::None
            }
        },
        |model: &(bool, bool)| -> View<Msg> {
            label(if model.0 { "inbox" } else { "waiting" }).id("status")
        },
    )
    .with_inbox(inbox)
    // One message per readiness: the closure does not read the pipe, so the
    // fd stays readable and the app unwatches nothing — which is exactly the
    // "a handler decides, the toolkit does not" rule. `Cmd::Quit` above ends
    // the run before that can loop more than a few times.
    .on_fd(watch, || vec![Msg::FromFd]);

    if let Err(err) = app.run(window) {
        report(&format!("app-error {err}"));
    }
}
```

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** both new tests pass, alongside Task 2's six and Task 5's three.

### Step 4 — commit

```bash
git add ui/src/view/app.rs ui/src/bin/window-probe.rs ui/tests/ingress.rs
git commit -m "$(cat <<'EOF'
feat(ui): App::on_fd maps a watched fd's readiness to messages

M5-D2's last piece, plus the live proof: an `app-inbox` probe mode runs a real
App on a toplevel window fed by a worker thread's inbox and by a foreign pipe
through `on_fd`, and reports each folded message. Both arrive with no timer and
no polling.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7 — `Cmd::Task` (M5-D3)

**Files:** `ui/src/view/cmd.rs`, `ui/src/view/app.rs`, `ui/tests/ingress.rs`,
`ui/README.md`, `docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* Task 4 (closures), Task 5 (drain).

*Produces:*

```rust
pub enum Cmd<Msg> {
    // … every M3 variant, unchanged …
    /// Run `f` once, on the loop thread, after the fold that produced it.
    Task(Rc<dyn Fn()>),
}
```

### Step 1 — the failing test

`ui/src/view/cmd.rs`'s test module:

```rust
    #[test]
    fn a_task_is_a_flatten_leaf_and_prints_opaquely() {
        // mutation: give `Cmd::Task` a `Batch`-like arm in `flatten`; it
        // disappears from the flat list and this fails.
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = std::rc::Rc::clone(&ran);
        let cmd: Cmd<Msg> = Cmd::Batch(vec![
            Cmd::Task(std::rc::Rc::new(move || flag.set(true))),
            Cmd::Quit,
        ]);
        assert!(!cmd.is_none());
        let flat = cmd.flatten();
        assert_eq!(flat.len(), 2);
        assert_eq!(format!("{:?}", flat[0]), "Task(..)");
        assert!(!ran.get(), "flatten must not run the task");
    }
```

`ui/tests/ingress.rs`:

```rust
#[test]
fn a_task_command_runs_once_after_the_fold_offscreen_too() {
    // The seam spec D8 requires: an outbound D-Bus call leaves `update` and
    // runs on the loop *after* the fold, never inside it.
    // mutation: execute `Cmd::Task` inside `update`'s match arm instead of in
    // `drain`; `during` is observed non-empty and this fails.
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let in_update = Rc::clone(&log);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        0_u32,
        move |model: &mut u32, msg: u32| {
            *model += msg;
            in_update.borrow_mut().push(format!("fold{}", *model));
            let after = Rc::clone(&in_update);
            Cmd::Batch(vec![
                Cmd::Task(Rc::new(move || after.borrow_mut().push("task-a".into()))),
                Cmd::Task({
                    let after = Rc::clone(&in_update);
                    Rc::new(move || after.borrow_mut().push("task-b".into()))
                }),
            ])
        },
        |model: &u32| label(&model.to_string()),
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen(
        (120, 40),
        clock,
        vec![ScriptStep::Message(1), ScriptStep::Message(1), ScriptStep::Capture],
    )
    .expect("the offscreen app runs");
    assert_eq!(frames.len(), 1);
    assert_eq!(
        *log.borrow(),
        vec!["fold1", "fold2", "task-a", "task-b", "task-a", "task-b"],
        "every queued message folds first, then the batch's tasks run in order"
    );
}
```

```bash
cargo test -p icedtea-ui --lib view::cmd
cargo test -p icedtea-ui --test ingress -- a_task_command_runs_once
```

**Expected failure:** `no variant or associated item named `Task` found for
enum `Cmd``.

### Step 2 — the implementation

`ui/src/view/cmd.rs`, last variant of the enum:

```rust
    /// Run `f` once, on the loop thread, after the fold that produced it.
    ///
    /// `f` **must not block**: the intended body is a channel push to a worker
    /// thread, or a call the app has already proven non-blocking. Anything
    /// whose answer matters comes back through the inbox as a `Msg`, never as
    /// a return value — `Cmd::Task` has none.
    Task(Rc<dyn Fn()>),
```

`Debug`: `Cmd::Task(_) => f.write_str("Task(..)"),`. `flatten` needs no arm —
`other => out.push(other)` already treats it as a leaf — and `is_none`'s `_ =>
false` already covers it.

`ui/src/view/app.rs`, in `drain`'s `match cmd`, beside `Cmd::Copy`/`Cmd::Focus`:

```rust
            // Outside `update`, after every queued message has been folded
            // (M5-D3). Not window-bound, so it is never returned to `run`.
            Cmd::Task(f) => f(),
```

Both `run` and `run_offscreen` go through `drain`, so `Cmd::Task` behaves
identically offscreen — which is what makes a `Cmd::Task` app testable without
a compositor.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --lib view::cmd
cargo test -p icedtea-ui --test ingress
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** both tests pass; `drain`'s `other => unhandled.push(other)` arm
never sees a `Task`, so `run`'s "command not applicable to a window" debug log
does not fire (assert this by eye once in the offscreen test's output — it must
be absent).

### Step 4 — docs and the record

`ui/README.md`, "External events":

```markdown
`Cmd::Task(Rc<dyn Fn()>)` runs a side effect on the loop thread after the fold
that produced it — a channel push to a worker, never a blocking call. It has no
return value on purpose: an answer comes back through the inbox as a message.
```

M3 contract §10: `### M5-D3 — `Cmd` grows a side-effect leaf (§4.7 had none)`.

### Step 5 — commit

```bash
git add ui/src/view/cmd.rs ui/src/view/app.rs ui/tests/ingress.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): add Cmd::Task, a non-blocking side effect run after the fold

M5-D3 and spec D8: outbound calls leave `update` and run on the loop, in
`drain`, after every queued message has been folded. A flatten leaf, printed
opaquely, executed identically by `run` and `run_offscreen`.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8 — three pointer `EventKind`s, `Handler::PairButton`, six builders, button constants (M5-D5, P0-D2, P0-D3)

**Files:** `ui/src/view/mod.rs`, `ui/src/view/builders.rs`,
`ui/src/window/pointer.rs`, `ui/src/window/mod.rs` (one re-export line),
`ui/tests/widget_pixels.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6: P0-D2, P0-D3),
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* nothing.

*Produces:*

```rust
// ui/src/view/mod.rs
pub enum EventKind { /* the eighteen M3 kinds */ PointerDown, PointerMotion, PointerUp }
// EventKind::ALL is 21 entries, in declaration order.

pub enum Handler<Msg> { /* the eight M3 variants */ PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>) }

impl<Msg: Clone> Handlers<Msg> {
    #[must_use]
    pub fn fire_pair_button(&self, kind: EventKind, x: f64, y: f64, button: u32) -> Option<Msg>;
}

// ui/src/view/builders.rs — on View<Msg>
#[must_use] fn on_pointer_down(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
#[must_use] fn on_pointer_motion(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
#[must_use] fn on_pointer_up(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self;
#[must_use] fn on_pointer_down_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;
#[must_use] fn on_pointer_motion_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;
#[must_use] fn on_pointer_up_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self;

// ui/src/window/pointer.rs
pub use crate::window::layer::BTN_LEFT;   // P0-D3: re-export, not a redefinition
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
```

### Step 1 — the failing test

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn event_kind_all_lists_twenty_one_kinds() {
    // mutation: forget to add one of the three pointer kinds to
    // `EventKind::ALL`; the count fails and so does any dispatcher that
    // iterates ALL.
    use icedtea_ui::view::EventKind;
    assert_eq!(EventKind::ALL.len(), 21);
    for kind in [
        EventKind::PointerDown,
        EventKind::PointerMotion,
        EventKind::PointerUp,
    ] {
        assert!(EventKind::ALL.contains(&kind), "{kind:?} is missing from ALL");
    }
    // Declaration order: the three are appended, so nothing before them moved.
    assert_eq!(EventKind::ALL[0], EventKind::Click);
    assert_eq!(EventKind::ALL[18], EventKind::PointerDown);
    assert_eq!(EventKind::ALL[19], EventKind::PointerMotion);
    assert_eq!(EventKind::ALL[20], EventKind::PointerUp);
}

#[test]
fn a_pair_button_handler_falls_back_to_a_pair_handler() {
    // Why `fire_pair_button` accepts both arities: a caller that fires does
    // not know which builder the view used, so `on_pointer_down` and
    // `on_pointer_down_with_button` coexist without a second fire method.
    // mutation: delete the `Handler::Pair` arm of `fire_pair_button`; the
    // first assertion returns None.
    use icedtea_ui::view::{EventKind, Handler, Handlers};
    use std::rc::Rc;

    let mut pair: Handlers<String> = Handlers::default();
    pair.set(
        EventKind::PointerDown,
        Handler::Pair(Rc::new(|x, y| format!("{x},{y}"))),
    );
    assert_eq!(
        pair.fire_pair_button(EventKind::PointerDown, 3.0, 4.0, 0x112),
        Some("3,4".to_owned()),
        "a Pair handler on a pointer kind receives (x, y) and drops the button"
    );

    let mut with_button: Handlers<String> = Handlers::default();
    with_button.set(
        EventKind::PointerUp,
        Handler::PairButton(Rc::new(|x, y, b| format!("{x},{y},{b:#x}"))),
    );
    assert_eq!(
        with_button.fire_pair_button(EventKind::PointerUp, 1.0, 2.0, 0x112),
        Some("1,2,0x112".to_owned())
    );
    // A different kind, or an arity the binding cannot supply, fires nothing.
    assert_eq!(
        with_button.fire_pair_button(EventKind::PointerDown, 1.0, 2.0, 0x110),
        None
    );
    assert_eq!(with_button.fire_unit(EventKind::PointerUp), None);
}

#[test]
fn the_linux_button_codes_are_reachable_from_one_module() {
    // P0-D3: `BTN_LEFT` keeps its M1 home and is re-exported beside the two
    // new ones, so `window::BTN_LEFT` stays unambiguous and
    // `layer_shell_screencopy.rs`'s import is untouched.
    use icedtea_ui::window::pointer::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    assert_eq!((BTN_LEFT, BTN_RIGHT, BTN_MIDDLE), (0x110, 0x111, 0x112));
    assert_eq!(icedtea_ui::window::BTN_LEFT, BTN_LEFT);
    assert_eq!(icedtea_ui::wayland::BTN_LEFT, BTN_LEFT);
}
```

```bash
cargo test -p icedtea-ui --test widget_pixels -- event_kind_all a_pair_button the_linux_button
```

**Expected failure:** `no variant named `PointerDown` found for enum
`EventKind``, `no function or associated item named `fire_pair_button``,
`unresolved import `icedtea_ui::window::pointer::BTN_MIDDLE``.

### Step 2 — the implementation

`ui/src/view/mod.rs` — the three kinds appended to `EventKind` **and** to
`ALL`, in the same order:

```rust
    /// A raw pointer press on this node (M5-D5). Generic: any kind may carry it.
    PointerDown,
    /// A raw pointer motion, delivered to the node that took the press for as
    /// long as the implicit grab holds.
    PointerMotion,
    /// A raw pointer release, delivered even when it lands outside the node.
    PointerUp,
```

`Handler`:

```rust
    /// Local `(x, y)` plus the Linux input-event button code — the shape
    /// middle-click-to-close and a canvas drag need (M5-D5).
    PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>),
```

with arms added to `Clone` (`Handler::PairButton(f) => Handler::PairButton(Rc::clone(f))`)
and `Debug` (`Handler::PairButton(_) => f.write_str("PairButton(..)")`).

`Handlers`:

```rust
    /// Fire a `PairButton` **or** a `Pair` handler registered for `kind`.
    ///
    /// A `Pair` receives `(x, y)` and the button is dropped, so
    /// `on_pointer_down` and `on_pointer_down_with_button` can sit on
    /// different nodes without the caller choosing a fire method. As with
    /// [`Handlers::fire_pair`] there is no `Unit` fallthrough: a `Unit`
    /// binding on a pointer kind is a builder that meant a different arity.
    ///
    /// `button` is `0` for a motion, which carries none (P0-D2); no real
    /// `BTN_*` code is zero.
    #[must_use]
    pub fn fire_pair_button(&self, kind: EventKind, x: f64, y: f64, button: u32) -> Option<Msg> {
        match self.get(kind)? {
            Handler::PairButton(f) => Some(f(x, y, button)),
            Handler::Pair(f) => Some(f(x, y)),
            _ => None,
        }
    }
```

`ui/src/view/builders.rs`, in the inherent `impl<Msg: Clone + 'static> View<Msg>`
block that holds `on_scrolled` (after it, so the pointer group reads together):

```rust
    /// A raw pointer press, with the point in this node's border box.
    #[must_use]
    pub fn on_pointer_down(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerDown, Handler::Pair(Rc::new(f)))
    }

    /// A raw pointer motion. While a press is held this arrives here even when
    /// the pointer has left the node (M3's implicit grab).
    #[must_use]
    pub fn on_pointer_motion(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerMotion, Handler::Pair(Rc::new(f)))
    }

    /// A raw pointer release, delivered even when it lands outside the node.
    #[must_use]
    pub fn on_pointer_up(self, f: impl Fn(f64, f64) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerUp, Handler::Pair(Rc::new(f)))
    }

    /// [`View::on_pointer_down`], plus the Linux button code.
    #[must_use]
    pub fn on_pointer_down_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerDown, Handler::PairButton(Rc::new(f)))
    }

    /// [`View::on_pointer_motion`], plus the button code (`0` — a motion
    /// carries none).
    #[must_use]
    pub fn on_pointer_motion_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerMotion, Handler::PairButton(Rc::new(f)))
    }

    /// [`View::on_pointer_up`], plus the Linux button code — how
    /// middle-click-to-close is expressed.
    #[must_use]
    pub fn on_pointer_up_with_button(self, f: impl Fn(f64, f64, u32) -> Msg + 'static) -> Self {
        self.on(EventKind::PointerUp, Handler::PairButton(Rc::new(f)))
    }
```

`ui/src/window/pointer.rs`, at the top after the module docs:

```rust
/// `BTN_LEFT` from Linux's `input-event-codes.h`.
///
/// Re-exported rather than redefined (P0-D3): M1 put it in
/// [`crate::window::layer`], `wayland::BTN_LEFT` and `window::BTN_LEFT` name
/// that one, and `ui/tests/layer_shell_screencopy.rs` is a byte-identical gate
/// that imports it from there.
pub use crate::window::layer::BTN_LEFT;

/// `BTN_RIGHT` from Linux's `input-event-codes.h`.
pub const BTN_RIGHT: u32 = 0x111;

/// `BTN_MIDDLE` from Linux's `input-event-codes.h` — the one M5's shell panel
/// closes a window with.
pub const BTN_MIDDLE: u32 = 0x112;
```

`ui/src/window/mod.rs`, beside the existing `pub use layer::BTN_LEFT;`:

```rust
pub use pointer::{BTN_MIDDLE, BTN_RIGHT};
```

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the three new tests pass. Every `match` over `EventKind` in the
crate is either exhaustive with a `_` arm or over `ALL`, so adding kinds
compiles; if one does not, add the arm rather than a catch-all.

### Step 4 — docs and the record

`ui/README.md`, a new subsection under "## Reactive framework (`view/`)":

```markdown
### Pointer events at the view layer

`on_pointer_down` / `on_pointer_motion` / `on_pointer_up` turn raw pointer
phases into messages on **any** widget kind, with `(x, y)` in the node's own
border box. The `_with_button` variants add the Linux button code
(`window::pointer::{BTN_LEFT, BTN_RIGHT, BTN_MIDDLE}`); a motion reports `0`,
because it carries no button. M3's implicit grab already routes motion and the
matching release to the node that took the press, so
`PointerDown → PointerMotion* → PointerUp` is one node's whole gesture even
when the pointer leaves it; a release outside still arrives, a `PointerLeave`
mid-drag does not end the sequence, and a grab broken by a closed surface
fabricates no synthetic release — an app treats a fresh `PointerDown` as
re-anchoring. `EventKind::Click` is unaffected: a left press-release on a node
carrying both `on_click` and `on_pointer_up_with_button` produces both messages.
```

M3 contract §10: `### M5-D5 — three pointer `EventKind`s and a
button-carrying `Handler` (extends §4.4)`. M5 contract §6: `### P0-D2`, `### P0-D3`.

### Step 5 — commit

```bash
git add ui/src/view/mod.rs ui/src/view/builders.rs ui/src/window/pointer.rs \
        ui/src/window/mod.rs ui/tests/widget_pixels.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md \
        docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): pointer events at the view layer, with an optional button code

M5-D5: EventKind::{PointerDown,PointerMotion,PointerUp} (ALL is 21),
Handler::PairButton, Handlers::fire_pair_button (a Pair binding still fires,
button dropped), six builders, and BTN_RIGHT/BTN_MIDDLE beside a re-exported
BTN_LEFT (P0-D3). A motion reports button 0 (P0-D2). Nothing fires them yet.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9 — firing the pointer handlers (M5-D5, P0-D1)

**Files:** `ui/src/view/app.rs`, `ui/tests/ingress.rs`,
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6: P0-D1).

**Interfaces**

*Consumes:* Task 8.

*Produces:*

```rust
// ui/src/view/app.rs — private, called from `deliver`
fn fire_pointer_handlers<Msg: Clone>(event: &Event, handlers: &Handlers<Msg>) -> Vec<Msg>;
```

### Step 1 — the failing test

`ui/tests/ingress.rs`:

```rust
/// The whole pointer gesture, on a plain `box_` — proof it is generic and not
/// a `DrawingArea` special case.
///
/// mutation: delete the `Event::PointerMotion` arm of
/// `fire_pointer_handlers`; the motion tag disappears and this fails (this is
/// M5-D5's mutation check, relocated by P0-D1).
#[test]
fn pointer_handlers_fire_down_motion_up_in_order_with_local_coordinates() {
    use icedtea_ui::view::builders::{box_, label};
    use icedtea_ui::widgets::Orientation;
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&log);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        (),
        move |_m: &mut (), msg: String| {
            recorder.borrow_mut().push(msg);
            Cmd::None
        },
        |_m: &()| -> View<String> {
            box_(Orientation::Vertical, [label("canvas")])
                .hexpand(true)
                .vexpand(true)
                .id("canvas")
                .on_pointer_down(|x, y| format!("down {x} {y}"))
                .on_pointer_motion(|x, y| format!("motion {x} {y}"))
                .on_pointer_up_with_button(|x, y, b| format!("up {x} {y} {b:#x}"))
        },
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen(
        (200, 100),
        clock,
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(20.0, 30.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion {
                x: 40.0,
                y: 50.0,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::pointer::BTN_MIDDLE,
                pressed: false,
                serial: 3,
                time_ms: 2,
            }),
            ScriptStep::Capture,
        ],
    )
    .expect("the offscreen app runs");
    assert_eq!(frames.len(), 1);
    let seen = log.borrow().clone();
    assert_eq!(
        seen,
        vec![
            "motion 20 30".to_owned(),
            "down 20 30".to_owned(),
            "motion 40 50".to_owned(),
            "up 40 50 0x112".to_owned(),
        ],
        "phases arrive in order, in the node's own coordinates, with the button"
    );
}

/// The property the Displays drag depends on: once a node has the press, the
/// motion and the release are its, even outside its box.
///
/// mutation: fire the handlers at `Phase::Bubble` instead of `Phase::Target`;
/// the grabbed node is the target, so the release outside it never fires.
#[test]
fn a_release_outside_the_node_still_reaches_it_through_the_grab() {
    use icedtea_ui::view::builders::{box_, label};
    use icedtea_ui::widgets::Orientation;
    use std::cell::RefCell;

    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&log);
    let clock = Rc::new(ManualClock::new());
    let _ = App::new(
        (),
        move |_m: &mut (), msg: String| {
            recorder.borrow_mut().push(msg);
            Cmd::None
        },
        |_m: &()| -> View<String> {
            // A small, centred canvas: (190, 90) is outside it.
            box_(Orientation::Vertical, [label("canvas")])
                .width_request(40)
                .height_request(20)
                .id("canvas")
                .on_pointer_down(|_, _| "down".to_owned())
                .on_pointer_up(|_, _| "up".to_owned())
        },
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen(
        (200, 100),
        clock,
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 50.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion {
                x: 190.0,
                y: 90.0,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 2,
            }),
            ScriptStep::Capture,
        ],
    )
    .expect("the offscreen app runs");
    let seen = log.borrow().clone();
    assert_eq!(
        seen.iter().filter(|m| *m == "down").count(),
        1,
        "one press: {seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|m| *m == "up").count(),
        1,
        "the release outside the box still reached the pressed node: {seen:?}"
    );
}
```

```bash
cargo test -p icedtea-ui --test ingress -- pointer_handlers a_release_outside
```

**Expected failure:** both fail with an empty `seen` vector — the handlers are
registered but nothing fires them.

### Step 2 — the implementation

`ui/src/view/app.rs`, above `deliver`:

```rust
/// M5-D5's three pointer kinds, fired from one place (P0-D1).
///
/// The contract's text puts this in `GenericC::on_event` plus a forwarding arm
/// per widget. `GenericC` is not on the dispatch path of a widget that has its
/// own controller — `ButtonC`, `DrawingAreaC` and thirty others replace it —
/// and P5 puts `on_pointer_up_with_button` on `button(..)` nodes while being
/// forbidden from touching `ui/`. So the firing lives in `deliver`, once, for
/// every kind alike, and no widget file is edited to opt in.
///
/// `button` is `0` for a motion, which carries none (P0-D2).
fn fire_pointer_handlers<Msg: Clone>(event: &Event, handlers: &Handlers<Msg>) -> Vec<Msg> {
    let (kind, local, button) = match event {
        Event::PointerDown { local, button, .. } => (EventKind::PointerDown, *local, *button),
        Event::PointerMotion { local } => (EventKind::PointerMotion, *local, 0),
        Event::PointerUp { local, button, .. } => (EventKind::PointerUp, *local, *button),
        _ => return Vec::new(),
    };
    handlers
        .fire_pair_button(kind, f64::from(local.0), f64::from(local.1), button)
        .into_iter()
        .collect()
}
```

and inside `deliver`'s per-node body, immediately before
`out.extend(controller.on_event(event, &mut ecx));`:

```rust
            // M5-D5/P0-D1: the node the event is aimed at — the innermost
            // `Instance` (P5-D33) — gets its pointer handlers fired before its
            // controller runs, so a controller that sets `cx.handled` (as
            // `GenericC` does on every left press) cannot swallow them.
            if phase == Phase::Target {
                out.extend(fire_pointer_handlers(event, &*handlers));
            }
```

`handlers` is the destructured `&mut Handlers<Msg>`; reborrow it immutably as
shown so `ecx` can still take it.

Imports at the top of `app.rs` gain `EventKind` and `Handlers` if they are not
already there.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** both new tests pass. Every existing interaction test is
unaffected: no existing view registers a pointer kind, so `fire_pair_button`
returns `None` on every dispatch.

**Mutation check:** delete the `Event::PointerMotion` arm from
`fire_pointer_handlers`; `pointer_handlers_fire_down_motion_up_in_order_with_local_coordinates`
fails. Restore.

### Step 4 — the record

M5 contract §6: `### P0-D1`, with the deviation text from the top of this plan.

### Step 5 — commit

```bash
git add ui/src/view/app.rs ui/tests/ingress.rs \
        docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): fire the pointer handlers from deliver, for every widget kind

M5-D5, by P0-D1's route: one call in `deliver` at Phase::Target rather than
`GenericC::on_event` plus per-widget forwarding, because a widget with its own
controller never reaches GenericC and P5 needs the handlers on `button(..)`.
Local coordinates, grab-following motion and a release outside the node are all
covered offscreen.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10 — the `DrawingArea` draw callback carries `&mut PaintCx` (M5-D6)

**Files:** `ui/src/view/mod.rs`, `ui/src/widgets/drawing_area.rs`,
`ui/src/gallery.rs`, `ui/tests/widget_pixels.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* nothing.

*Produces:*

```rust
pub enum Prop { /* … */ Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>) }

pub fn drawing_area<Msg: Clone + 'static>(
    draw: impl Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>) + 'static,
) -> View<Msg>;

pub struct DrawingAreaC {
    pub draw: Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>,
    pub content: (i32, i32),
    pub last_size: (f32, f32),
}
```

### Step 1 — the failing test

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_drawing_area_can_shape_text_through_its_paint_cx() {
    // P4's Displays canvas draws a connector name and a resolution per head;
    // without `&mut PaintCx` the callback cannot reach a FontDatabase and
    // cannot shape a glyph at all.
    // mutation: pass a fresh, empty `PaintCx` instead of the one `paint`
    // received; the shaping finds no font and the row stays flat.
    use icedtea_ui::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, Rgba};
    use icedtea_ui::text::{FontQuery, ShapeKey};
    use icedtea_ui::view::builders::drawing_area;
    use icedtea_ui::widgets::drawing_area::DrawingAreaExt;

    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| {
            drawing_area(|canvas, rect, cx| {
                // A white ground, so a glyph is the only dark ink.
                canvas.draw_rect(
                    &rect.to_skia(),
                    &icedtea_ui::paint::fill_paint(Rgba {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                );
                let families = [FontFamily::Generic(GenericFamily::SansSerif)];
                let query = FontQuery {
                    families: &families,
                    weight: 400.0,
                    style: FontStyle::Normal,
                    stretch: 100.0,
                    size_px: 24.0,
                };
                let Some(face) = cx.fonts.match_face(&query) else {
                    return;
                };
                let shaped = cx.fonts.shape(&ShapeKey {
                    text: "HH",
                    face: &face,
                    size_px: 24.0,
                    letter_spacing_px: 0.0,
                    features: &[],
                    variations: &[],
                    transform: Keyword::None,
                });
                if let Some(blob) = shaped.blob.as_ref() {
                    canvas.draw_text_blob(
                        blob,
                        rect.x + 4.0,
                        rect.y + 32.0,
                        &icedtea_ui::paint::fill_paint(Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                    );
                }
            })
            .content_width(120)
            .content_height(48)
            .hexpand(true)
            .vexpand(true)
        },
        (120, 48),
        vec![ScriptStep::Capture],
    );
    let mut dark = 0;
    for x in 0..120 {
        for y in 0..48 {
            if let Some(px) = frames.pixel(0, x, y)
                && (u32::from(px.0) + u32::from(px.1) + u32::from(px.2)) < 300
            {
                dark += 1;
            }
        }
    }
    assert!(dark > 20, "no glyph ink on the canvas: {dark} dark pixels");
}
```

`FontDatabase::match_face` misses only when the machine has no sans-serif face
at all, in which case the closure returns early and the assertion fails loudly
rather than silently passing — which is the right failure for a font-stack
regression too.

```bash
cargo test -p icedtea-ui --test widget_pixels -- a_drawing_area_can_shape_text
```

**Expected failure:** `this function takes 2 arguments but 3 arguments were
supplied` on the closure.

### Step 2 — the implementation

`ui/src/view/mod.rs`:

```rust
    /// `GtkDrawingArea`'s paint callback: the canvas, the rect to fill, and
    /// the paint context — the last so a callback can shape text, resolve an
    /// icon or read the theme's colours (M5-D6).
    Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>),
```

`Prop`'s `Debug` still prints `Draw(..)` and `Props::diff` still compares by
`Rc` pointer, so the widening changes no diff behaviour.

`ui/src/widgets/drawing_area.rs`: the constructor's parameter type, the
`DrawingAreaC::draw` field type, the `build`/`set_prop` fallback closure
(`Rc::new(|_, _, _| {})`), and `paint`:

```rust
    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        self.last_size = (content.width, content.height);
        // Cloned out of `self` first: the callback borrows nothing of the
        // controller, and holding `&self.draw` across the call would conflict
        // with the `&mut self` this method holds.
        let draw = Rc::clone(&self.draw);
        draw(canvas, content, cx);
        true
    }
```

Two call sites move mechanically:

- `ui/src/gallery.rs`'s `Kind::DrawingArea` sample — the closure gains a third
  parameter it ignores for now (Task 11 uses it):
  `|canvas: &mut skia_rs_safe::canvas::Canvas<'_>, rect: Rect, _cx: &mut icedtea_ui::paint::PaintCx<'_>| { .. }`
  (inside the crate, `crate::paint::PaintCx`).
- `ui/tests/widget_pixels.rs`'s
  `a_drawing_area_runs_its_callback_against_the_allocated_rect` — same, and its
  pixel assertions keep every number.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test gallery_gate
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the new test passes and the existing drawing-area pixel test
still asserts `(255, 0, 0)` at (32, 32).

### Step 4 — docs and the record

`ui/README.md`, in the widget catalogue's `drawing_area` paragraph (or, if it
has none, at the end of "### Pointer events at the view layer"):

```markdown
A `drawing_area`'s callback is `Fn(&mut Canvas, Rect, &mut PaintCx)`: the third
argument is the same paint context every controller's `paint` receives, so a
canvas can shape text, resolve an icon and read the theme's colours instead of
being limited to fills and strokes.
```

M3 contract §10: `### M5-D6 — `Prop::Draw` carries the paint context (widens §4.3)`.

### Step 5 — commit

```bash
git add ui/src/view/mod.rs ui/src/widgets/drawing_area.rs ui/src/gallery.rs \
        ui/tests/widget_pixels.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): a DrawingArea's draw callback receives &mut PaintCx

M5-D6. `Prop::Draw` widens to `Fn(&mut Canvas, Rect, &mut PaintCx)` so a canvas
can shape text — P4's Displays view draws a connector name and a resolution per
head. Two mechanical call sites; `Props::diff` still compares by Rc pointer, so
no diff behaviour changes.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11 — the gallery's drawing-area sample reports pointer phases, and the interaction gate drags it (M5-D5)

**Files:** `ui/src/gallery.rs`, `ui/tests/support/mod.rs`,
`ui/tests/interaction_gate.rs`.

**Interfaces**

*Consumes:* Tasks 8, 9, 10.

*Produces:*

```rust
// ui/tests/support/mod.rs — Driver additions
impl Driver {
    /// Press `button` (a `BTN_*` code) at `(x, y)`.
    pub fn press_button(&mut self, x: i32, y: i32, button: u32);
    /// Release `button` at `(x, y)`.
    pub fn release_button(&mut self, x: i32, y: i32, button: u32);
    /// Press and release `button` at `(x, y)` — middle-click, in one call.
    pub fn click_button(&mut self, x: i32, y: i32, button: u32);
    /// [`Driver::drag`], with `button` instead of `BTN_LEFT`.
    pub fn drag_with_button(&mut self, from: (i32, i32), to: (i32, i32), button: u32);
}
```

and the gallery's `drawing_area` sample, which now reports
`changed drawing_area down|motion|up <x> <y> <button>` per phase.

### Step 1 — the failing test

`ui/tests/interaction_gate.rs`:

```rust
/// The gesture P4's Displays canvas is built on: press, move, release, all
/// delivered to the canvas with its own coordinates, through the compositor.
///
/// Mutation check: delete the `Event::PointerMotion` arm of
/// `view::app::fire_pointer_handlers`; the `motion` line never appears and
/// this fails. Restore.
#[test]
fn dragging_across_a_drawing_area_reports_every_pointer_phase() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drawing_area");
    let (x, y) = driver.point("drawing_area", "root");
    let alloc = driver.allocation("drawing_area");
    // Stay inside the canvas: a quarter of its width to the right.
    let target = (x + (alloc.width as i32 / 4).max(4), y);

    driver.drag((x, y), target);

    assert!(
        gallery.wait_msg("changed drawing_area down", REACT),
        "no press reached the canvas; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg("changed drawing_area motion", REACT),
        "no motion reached the canvas; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg("changed drawing_area up", REACT),
        "no release reached the canvas; got {:?}",
        gallery.messages()
    );
    let lines = gallery.messages();
    let phase_of = |needle: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(&format!("changed drawing_area {needle}")))
    };
    assert!(
        phase_of("down") < phase_of("motion") && phase_of("motion") < phase_of("up"),
        "phases arrived out of order: {lines:?}"
    );
}

/// The button code survives the trip: a middle press on the canvas reports
/// `0x112`, which is how P5's middle-click-to-close is expressed.
///
/// Mutation check: fire `Handler::Pair` instead of `Handler::PairButton` in
/// `fire_pair_button`'s first arm; the code becomes the left button's and
/// this fails. Restore.
#[test]
fn a_middle_click_on_a_drawing_area_reports_the_middle_button_code() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drawing_area");
    let (x, y) = driver.point("drawing_area", "root");

    driver.click_button(x, y, icedtea_ui::window::pointer::BTN_MIDDLE);

    assert!(
        gallery.wait_msg("changed drawing_area up", REACT),
        "no release reached the canvas; got {:?}",
        gallery.messages()
    );
    let up = gallery
        .messages()
        .into_iter()
        .find(|l| l.starts_with("changed drawing_area up"))
        .expect("the release line");
    assert!(
        up.ends_with(" 274"),
        "the release must carry BTN_MIDDLE (274 decimal): {up}"
    );
}
```

```bash
cargo test -p icedtea-ui --test interaction_gate -- drawing_area
```

**Expected failure:** first `no method named `click_button``; after Step 2's
support half, `no press reached the canvas` (the sample has no handlers).

### Step 2 — the implementation

`ui/tests/support/mod.rs`, beside `press`/`release`/`click`/`drag`; the
existing four keep their bodies and delegate:

```rust
    /// Press `button` (a Linux `BTN_*` code) at `(x, y)`.
    pub fn press_button(&mut self, x: i32, y: i32, button: u32) {
        self.move_to(x, y);
        self.pointer.button(button, true);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Release `button` at `(x, y)`.
    pub fn release_button(&mut self, x: i32, y: i32, button: u32) {
        self.move_to(x, y);
        self.pointer.button(button, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release `button` at `(x, y)`.
    pub fn click_button(&mut self, x: i32, y: i32, button: u32) {
        self.press_button(x, y, button);
        self.release_button(x, y, button);
    }

    /// [`Driver::drag`] with a button other than the left one.
    pub fn drag_with_button(&mut self, from: (i32, i32), to: (i32, i32), button: u32) {
        self.press_button(from.0, from.1, button);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release_button(to.0, to.1, button);
    }
```

and rewrite the existing three as one-liners over them, so there is one
implementation of "press at a point":

```rust
    /// Press the left button at `(x, y)`.
    pub fn press(&mut self, x: i32, y: i32) {
        self.press_button(x, y, icedtea_ui::wayland::BTN_LEFT);
    }

    /// Release the left button at `(x, y)`.
    pub fn release(&mut self, x: i32, y: i32) {
        self.release_button(x, y, icedtea_ui::wayland::BTN_LEFT);
    }
```

(`click` and `drag` already call `press`/`release`; leave them alone.)

`ui/src/gallery.rs`, the `Kind::DrawingArea` sample — the paint body is
unchanged, the handlers are new:

```rust
        Kind::DrawingArea => Sample::Own(
            w::drawing_area(
                |canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
                 rect: Rect,
                 _cx: &mut crate::paint::PaintCx<'_>| {
                    let mut paint = Paint::new();
                    paint.set_color32(Color(0xFF33_D17A));
                    canvas.draw_rect(&rect.to_skia(), &paint);
                },
            )
            .width_request(64)
            .height_request(48)
            // The interaction gate reads these back off stdout. `Changed`
            // rather than a new `GalleryMsg` variant: the payload is already a
            // flat string and `log_line` folds control characters, so the
            // gate's substring matching needs nothing new.
            .on_pointer_down_with_button(|x, y, b| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("down {x} {y} {b}"))
            })
            .on_pointer_motion(|x, y| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("motion {x} {y} 0"))
            })
            .on_pointer_up_with_button(|x, y, b| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("up {x} {y} {b}"))
            }),
        ),
```

`Changed` writes into `model.texts`, which the drawing-area sample does not
read, so the tree does not change shape between phases and the reconciler has
nothing to do — the gate is asserting messages, not pixels.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test gallery_gate
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the two new interaction tests pass, the existing sixteen stay
green (16/16 + 2 = 18 tests in the file), and `gallery_gate` is unaffected —
`drawing_area` paints exactly what it painted before.

### Step 4 — commit

```bash
git add ui/src/gallery.rs ui/tests/support/mod.rs ui/tests/interaction_gate.rs
git commit -m "$(cat <<'EOF'
test(ui): drive a real drag across the gallery's drawing area

M5-D5's harness half. The sample reports every pointer phase with its local
coordinates and the button code; the gate drags across it and middle-clicks it.
`Driver` grows press/release/click/drag variants that take a BTN_* code, and
the existing left-button helpers now delegate to them.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12 — `Keymap::base_keysym` and `KeyEvent::base` (M5-D7)

**Files:** `ui/src/window/keyboard.rs`, `ui/src/window/focus.rs`,
`ui/src/widgets/headless.rs`, `ui/tests/widget_pixels.rs`,
`ui/tests/ingress.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* nothing.

*Produces:*

```rust
impl Keymap {
    /// The keysym `keycode` produces at group 0, level 0.
    #[must_use] pub fn base_keysym(&self, keycode: u32) -> xkbcommon::xkb::Keysym;
}

pub struct KeyEvent {
    pub keycode: u32,
    pub keysym: xkbcommon::xkb::Keysym,
    /// `base_keysym(keycode)`, stamped by `Keymap::translate`.
    pub base: xkbcommon::xkb::Keysym,
    // … utf8, mods, consumed, pressed, repeat, serial, time_ms unchanged …
}
```

### Step 1 — the failing test

`ui/tests/ingress.rs` (hermetic — the vendored `us.xkb`, no compositor):

```rust
/// The vendored US keymap every keyboard test in the crate uses.
fn us_keymap() -> icedtea_ui::window::keyboard::Keymap {
    icedtea_ui::window::keyboard::Keymap::from_string(include_str!(
        "fixtures/keymaps/us.xkb"
    ))
    .expect("the vendored us keymap compiles")
}

/// evdev keycodes (xkb's are these + 8, applied inside the keymap).
const KEY_A_CODE: u32 = 30;
const KEY_LEFTSHIFT: u32 = 42;

#[test]
fn base_keysym_reports_the_level_zero_sym_for_a_shifted_key() {
    // Settings' keybinding capture stores the *unshifted* sym, because
    // `icedtea_config::keys::key_name_to_keysym` always encodes that one;
    // storing XK_A from SUPER+SHIFT+a would produce a binding the compositor
    // can never match (M5-D7's normalisation rule).
    // mutation: change the level argument from 0 to 1 in `base_keysym`; the
    // base becomes `A` and this fails.
    use xkbcommon::xkb::keysyms;

    let mut keymap = us_keymap();
    keymap.update_key(KEY_LEFTSHIFT, true);
    keymap.update_mask(1, 0, 0, 0); // Shift depressed
    let ev = keymap.translate(KEY_A_CODE, true, 1, 0);
    assert_eq!(ev.keysym.raw(), keysyms::KEY_A, "the modified sym is `A`");
    assert_eq!(ev.base.raw(), keysyms::KEY_a, "the base sym is `a`");
    assert_eq!(
        keymap.base_keysym(KEY_A_CODE).raw(),
        keysyms::KEY_a,
        "and asking the keymap directly gives the same answer"
    );
}

#[test]
fn base_keysym_is_group_and_caps_agnostic() {
    // mutation: read the sym from `state.key_get_one_sym` instead of the
    // keymap's level-0 lookup; Caps Lock flips it and this fails.
    use xkbcommon::xkb::keysyms;

    let mut keymap = us_keymap();
    keymap.update_mask(2, 0, 2, 1); // caps latched+locked, group 1
    assert_eq!(keymap.base_keysym(KEY_A_CODE).raw(), keysyms::KEY_a);
}

#[test]
fn base_keysym_of_an_unmapped_keycode_is_no_symbol() {
    // Untrusted keymaps never panic (contract cross-cutting rule).
    // mutation: `expect` the first sym in `base_keysym`; this panics.
    use xkbcommon::xkb::keysyms;

    let keymap = us_keymap();
    for keycode in [0_u32, 9_999, u32::MAX] {
        assert_eq!(
            keymap.base_keysym(keycode).raw(),
            keysyms::KEY_NoSymbol,
            "keycode {keycode} is not in the us keymap"
        );
    }
}

#[test]
fn translate_stamps_base_on_every_key_event() {
    // mutation: leave `base` set to `keysym` in `translate`; the shifted case
    // in the first test fails, and this one still passes — which is why both
    // exist.
    let mut keymap = us_keymap();
    let press = keymap.translate(KEY_A_CODE, true, 1, 0);
    let release = keymap.translate(KEY_A_CODE, false, 2, 1);
    assert_eq!(press.base, press.keysym, "an unmodified press: base == keysym");
    assert_eq!(release.base, release.keysym);
}
```

```bash
cargo test -p icedtea-ui --test ingress -- base_keysym translate_stamps
```

**Expected failure:** `no method named `base_keysym` found for struct
`Keymap``, and `no field `base` on type `KeyEvent``.

### Step 2 — the implementation

`ui/src/window/keyboard.rs`:

```rust
    /// The `base_keysym(keycode)` of this event's keycode.
    ///
    /// Stamped by [`Keymap::translate`] so a view-layer `on_key` closure —
    /// which has a `&KeyEvent` and no `Keymap` — can normalise a capture
    /// without reaching into the window.
    pub base: xkb::Keysym,
```

(placed directly after `keysym`, matching M5-D7's field order), and:

```rust
    /// The keysym `keycode` produces at **group 0, level 0** — shift-, caps-
    /// and group-agnostic.
    ///
    /// `keycode` is the `wl_keyboard.key` evdev code; xkb's is `+8`, applied
    /// internally by the same helper [`Keymap::translate`] uses. A keycode the
    /// keymap does not map, or one that maps to several syms at that level,
    /// yields the first sym, or `Keysym::NoSymbol` when there is none — never
    /// a panic.
    ///
    /// This is the equivalent of GDK's `translate_key(keycode, 0, 0)`, and the
    /// only thing an accelerator capture may store: `icedtea_config`'s
    /// `key_name_to_keysym` always encodes the unshifted keysym, so a capture
    /// that stored `XK_Q` from `SUPER+SHIFT+q` would produce a binding the
    /// compositor's `match_action` can never fire.
    #[must_use]
    pub fn base_keysym(&self, keycode: u32) -> xkb::Keysym {
        self.keymap
            .key_get_syms_by_level(xkb_keycode(keycode), 0, 0)
            .first()
            .copied()
            .unwrap_or_else(|| xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
    }
```

In `translate`, compute it in the same pass and put it in the literal:

```rust
        let base = self.base_keysym(keycode);
```
```rust
        KeyEvent {
            keycode,
            keysym,
            base,
            utf8,
            // …
        }
```

The five literal `KeyEvent` constructors gain `base` set to the same value they
set `keysym` to — which is what an unmodified key already means:

- `ui/src/widgets/headless.rs:149` and `:170`
- `ui/src/window/focus.rs:623`
- `ui/tests/widget_pixels.rs:632` (`key_char`) and `:649` (`key_named`)

Each is one line, e.g. in `key_char`:

```rust
        keysym: xkbcommon::xkb::Keysym::from(u32::from(ch)),
        base: xkbcommon::xkb::Keysym::from(u32::from(ch)),
```

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the four new tests pass and every keyboard test in
`window::keyboard`'s own module still does — `base` is additive and no
existing assertion reads it.

**Mutation check:** change `key_get_syms_by_level(.., 0, 0)` to `(.., 0, 1)`;
`base_keysym_reports_the_level_zero_sym_for_a_shifted_key` fails. Restore.

### Step 4 — docs and the record

`ui/README.md`, "## The window and event layer", after the Keyboard bullet:

```markdown
### Base-level keysyms

`Keymap::base_keysym(keycode)` is the sym a key produces at group 0, level 0 —
GDK's `translate_key(keycode, 0, 0)` — and every `KeyEvent` carries it as
`base`. An accelerator capture normalises with it, exactly as the compositor
matches:

```rust
let sym = if ev.base != xkb::keysyms::KEY_NoSymbol { ev.base } else { ev.keysym };
```

The raw/latin sym, falling back to the modified one only when the keycode
produces no base sym at all; modifiers come from `KeyEvent::mods`, the
effective state, not from `consumed`. This is what makes a binding captured
from `SUPER+SHIFT+q` match the `KEY_q` the config file encodes.
```

M3 contract §10: `### M5-D7 — `Keymap` exposes a level-0 lookup and `KeyEvent`
carries it (extends §3.3)`.

### Step 5 — commit

```bash
git add ui/src/window/keyboard.rs ui/src/window/focus.rs ui/src/widgets/headless.rs \
        ui/tests/widget_pixels.rs ui/tests/ingress.rs ui/README.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): Keymap::base_keysym, stamped onto every KeyEvent as `base`

M5-D7. Level 0, group 0 — GDK's translate_key(keycode, 0, 0) — so settings'
keybinding capture can store the unshifted keysym the config format and the
compositor's match_action both require. An unmapped keycode yields NoSymbol
rather than panicking. Four hermetic tests on the vendored us keymap.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13 — `ColorDialogButton` and `ColorDialog` paint at rest (M5-D8, first two)

**Files:** `ui/src/widgets/color_dialog.rs`, `ui/tests/widget_pixels.rs`.

**Interfaces**

*Consumes:* nothing.

*Produces:*

```rust
impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogButtonC {
    fn measure(&mut self, available: (Option<f32>, Option<f32>), cx: &mut BuildCx<'_>)
        -> Option<(f32, f32)>;                       // Some((48.0, 32.0))
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogC {
    fn measure(&mut self, available: (Option<f32>, Option<f32>), cx: &mut BuildCx<'_>)
        -> Option<(f32, f32)>;                       // the palette grid's size
    fn paint(&mut self, canvas: &mut Canvas<'_>, alloc: &Allocation,
             style: &ComputedStyle, cx: &mut PaintCx<'_>) -> bool;
}

impl ColorDialogC {
    /// The palette grid, in `content`'s coordinates: one rect per swatch,
    /// then the custom colour's. `measure` and `paint` share it, so the drawn
    /// grid and the measured box cannot drift.
    #[must_use] pub fn grid(&self, content: Rect) -> Vec<(Rgba, Rect)>;
    /// The grid's intrinsic size.
    #[must_use] pub fn intrinsic(&self) -> (f32, f32);
}
```

### Step 1 — the failing test

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_color_dialog_button_paints_its_swatch_at_rest() {
    // `ColorDialogButtonC::paint` has always filled `alloc.content_box` with
    // its colour; what it lacked was an intrinsic size, so with all of its
    // chrome on controller-owned subnodes taffy never saw, the whole button
    // collapsed — the `ScrollbarC::measure` / `ScaleC::measure` case.
    // mutation: delete `ColorDialogButtonC::measure`; the box is 0x0 and no
    // red pixel appears.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::color_dialog_button;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;

    let red = Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let packed = ColorDialogC::pack(red);
    let frames = run(
        packed,
        |_m: &mut f64, _msg: ()| Cmd::None,
        |model: &f64| color_dialog_button(*model),
        (80, 60),
        vec![ScriptStep::Capture],
    );
    let mut reds = 0;
    for x in 0..80 {
        for y in 0..60 {
            if frames.pixel(0, x, y).map(|p| (p.0, p.1, p.2)) == Some((255, 0, 0)) {
                reds += 1;
            }
        }
    }
    assert!(
        reds >= 48 * 32 / 2,
        "the swatch did not fill its button: {reds} red pixels"
    );
}

#[test]
fn a_color_dialog_paints_its_palette_at_rest() {
    // mutation: return `false` from `ColorDialogC::paint`; the frame is one
    // flat colour and `has_ink` fails.
    use icedtea_ui::view::builders::color_dialog;

    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| color_dialog(),
        (320, 240),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (320, 240)), "the palette painted nothing");
    // More than one palette colour, not just a wash: count distinct pixels.
    let mut seen = std::collections::HashSet::new();
    for x in 0..320 {
        for y in 0..240 {
            if let Some(px) = frames.pixel(0, x, y) {
                seen.insert(px);
            }
        }
    }
    assert!(
        seen.len() > 8,
        "a palette grid must show many colours, saw {}",
        seen.len()
    );
}

#[test]
fn a_color_dialogs_measured_box_is_the_grid_it_paints() {
    // The drift guard: `measure` and `paint` read one geometry function.
    // mutation: hard-code a different column count in `intrinsic`; this fails.
    use icedtea_ui::layout::Rect;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;
    use icedtea_ui::view::Props;
    use icedtea_ui::css::node::Node;

    let node = Node::new("window");
    let mut controller = build_controller_for_test::<ColorDialogC>(&node, &Props::default());
    let (w, h) = controller.intrinsic();
    let cells = controller.grid(Rect::new(0.0, 0.0, w, h));
    assert_eq!(
        cells.len(),
        controller.palette.len() + usize::from(controller.custom.is_some()),
        "every palette entry gets a cell"
    );
    for (_, rect) in &cells {
        assert!(
            rect.x >= 0.0 && rect.y >= 0.0 && rect.x + rect.width <= w + 0.01
                && rect.y + rect.height <= h + 0.01,
            "cell {rect:?} escapes the measured {w}x{h} box"
        );
    }
}
```

`build_controller_for_test` is this file's existing pattern for building a
controller with a `BuildCx` — the `ColorDialogC` unit test in
`ui/src/widgets/color_dialog.rs` shows the six lines it needs (`CompiledSheet::compile`,
`FontDatabase::probe_only`, `IconTheme::with_name_and_roots`, `ManualClock`,
`ResolveEnv::default`, then `<ColorDialogC as Controller<usize>>::build`). Add
it as a local helper in `widget_pixels.rs` if it is not already there, with
that body.

```bash
cargo test -p icedtea-ui --test widget_pixels -- color_dialog
```

**Expected failure:** the first two fail on the pixel counts (the widgets
collapse / paint nothing); the third fails to compile — `no method named
`intrinsic``.

### Step 2 — the implementation

`ui/src/widgets/color_dialog.rs`:

```rust
/// One palette cell, in px. Adwaita's `colorswatch` minimum.
const SWATCH_PX: f32 = 24.0;
/// The gap between cells.
const SWATCH_GAP_PX: f32 = 4.0;
/// Cells per row — GTK's own palette is nine hues by five steps.
const SWATCH_COLUMNS: usize = 9;
```

```rust
impl ColorDialogC {
    /// The palette grid in `content`'s coordinate space: one rect per palette
    /// entry, row-major, then the custom colour on a row of its own.
    ///
    /// `measure` and `paint` both read this, so what is drawn and what is
    /// measured cannot drift (the defect that put `color_dialog` on
    /// `KNOWN_BLANK_AT_REST`: its whole body was subnodes taffy never sized).
    #[must_use]
    pub fn grid(&self, content: Rect) -> Vec<(Rgba, Rect)> {
        let step = SWATCH_PX + SWATCH_GAP_PX;
        let mut cells = Vec::with_capacity(self.palette.len() + 1);
        for (index, colour) in self.palette.iter().enumerate() {
            let column = index % SWATCH_COLUMNS;
            let row = index / SWATCH_COLUMNS;
            cells.push((
                *colour,
                Rect::new(
                    content.x + column as f32 * step,
                    content.y + row as f32 * step,
                    SWATCH_PX,
                    SWATCH_PX,
                ),
            ));
        }
        if let Some(custom) = self.custom {
            let row = self.palette.len().div_ceil(SWATCH_COLUMNS);
            cells.push((
                custom,
                Rect::new(content.x, content.y + row as f32 * step, SWATCH_PX, SWATCH_PX),
            ));
        }
        cells
    }

    /// The grid's intrinsic size: nine columns, as many rows as the palette
    /// needs, plus one for the custom colour once there is one.
    #[must_use]
    pub fn intrinsic(&self) -> (f32, f32) {
        let step = SWATCH_PX + SWATCH_GAP_PX;
        let columns = SWATCH_COLUMNS.min(self.palette.len().max(1));
        let rows = self.palette.len().div_ceil(SWATCH_COLUMNS) + usize::from(self.custom.is_some());
        (
            columns as f32 * step - SWATCH_GAP_PX,
            (rows.max(1) as f32) * step - SWATCH_GAP_PX,
        )
    }
}
```

In `impl Controller for ColorDialogButtonC`, before `paint`:

```rust
    /// Adwaita's `button.color` minimum.
    ///
    /// The swatch is chrome on a controller-owned subnode taffy never sees, so
    /// without an intrinsic size the whole button collapses to 0x0 — the same
    /// case `ScrollbarC::measure` and `ScaleC::measure` document.
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some((48.0, 32.0))
    }
```

In `impl Controller for ColorDialogC`:

```rust
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some(self.intrinsic())
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        _style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        let mut painted = false;
        for (colour, rect) in self.grid(content) {
            if rect.x + rect.width > content.x + content.width
                || rect.y + rect.height > content.y + content.height
            {
                // A grid larger than the box it was given: clip by dropping
                // the cells that do not fit, rather than overdrawing a
                // neighbour's chrome.
                continue;
            }
            canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(colour));
            painted = true;
        }
        painted
    }
```

`ColorDialogC::on_event` is untouched: its swatch hit-testing walks the
subnodes and is exercised by the module's own laid-out unit test.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --lib widgets::color_dialog
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the three new tests pass; `clicking_a_palette_swatch_selects_its_colour`
still passes (it lays the subnodes out itself with `FixedMeasure`).

**Mutation check:** delete `ColorDialogButtonC::measure`;
`a_color_dialog_button_paints_its_swatch_at_rest` fails with 0 red pixels.
Restore.

### Step 4 — commit

```bash
git add ui/src/widgets/color_dialog.rs ui/tests/widget_pixels.rs
git commit -m "$(cat <<'EOF'
feat(ui): ColorDialogButton measures, and ColorDialog paints its palette

M5-D8, first two of four. The button gains Adwaita's 48x32 minimum so its
swatch has a box to fill; the dialog gains one geometry function that both
`measure` and `paint` read, so the drawn grid and the measured box cannot
drift. Settings' three colour buttons are the consumers.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14 — an unchecked `CheckButton` paints its box (M5-D8, third)

**Files:** `ui/src/widgets/check_button.rs`, `ui/tests/widget_pixels.rs`.

**Interfaces**

*Consumes:* nothing.

*Produces:* `CheckButtonC::paint` returns `true` in every state.

### Step 1 — the failing test

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn an_unchecked_check_button_paints_its_box() {
    // GTK draws the empty indicator: a background and a 1px border. Without
    // it a settings page full of unchecked boxes shows nothing at all, which
    // is why `check_button` was on KNOWN_BLANK_AT_REST.
    // mutation: restore the early `if !self.active && !self.inconsistent {
    // return false }`; this fails with a flat frame.
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;

    let frames = run(
        false,
        |model: &mut bool, on: bool| {
            *model = on;
            Cmd::None
        },
        // An empty label on purpose: `GenericC` shapes no glyphs for it, so
        // any ink in the frame is the indicator this task draws.
        |model: &bool| check_button("").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert!(
        has_ink(&frames, 0, (120, 40)),
        "an unchecked check button painted nothing"
    );
}

#[test]
fn a_checked_check_button_still_paints_the_builtin() {
    // The regression guard for the branch above: the checked path must still
    // go through `paint::icon::paint_builtin`, and must differ from unchecked.
    // mutation: return early for the *checked* state instead; the two frames
    // become identical and this fails.
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;

    let unchecked = run(
        false,
        |_m: &mut bool, _on: bool| Cmd::None,
        |model: &bool| check_button("Check").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    let checked = run(
        true,
        |_m: &mut bool, _on: bool| Cmd::None,
        |model: &bool| check_button("Check").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    let differs = (0..120).any(|x| {
        (0..40).any(|y| unchecked.pixel(0, x, y) != checked.pixel(0, x, y))
    });
    assert!(differs, "checked and unchecked paint the same pixels");
}
```

```bash
cargo test -p icedtea-ui --test widget_pixels -- check_button
```

**Expected failure:** `an_unchecked_check_button_paints_its_box` fails —
`has_ink` is `false`, because with an empty label nothing in the tree draws
anything at all while `paint` returns early. The second test fails on
`checked and unchecked paint the same pixels` for the same reason.

### Step 2 — the implementation

`ui/src/widgets/check_button.rs`, replacing the early return:

```rust
        let indicator = alloc.content_box;
        if indicator.is_empty() {
            return false;
        }
        if !self.active && !self.inconsistent {
            // GTK paints the empty indicator too: `check` has a background and
            // a 1px border in every theme. Reading both off `style` is what
            // keeps light/dark/high-contrast right without a second palette
            // here — the same "read the colour off `style`, not off the
            // subnode" convention `ProgressBarC::paint` and `LevelBarC::paint`
            // follow.
            let colour = style.color();
            let fill = crate::css::value::Rgba {
                a: colour.a * 0.15,
                ..colour
            };
            canvas.draw_rect(&indicator.to_skia(), &crate::paint::fill_paint(fill));
            let mut border = crate::paint::fill_paint(style.border_colors()[0]);
            border.set_style(skia_rs_safe::paint::Style::Stroke);
            border.set_stroke_width(1.0);
            canvas.draw_rect(&indicator.to_skia(), &border);
            return true;
        }
        // The indicator's own allocation is the `check` node's, and the glyph
        // goes through `paint_builtin` rather than `Builtin::draw` so it
        // honours the same `-gtk-icon-transform`/`-gtk-icon-filter`/
        // `-gtk-icon-shadow` stack an icon file does (contract §6).
        crate::paint::icon::paint_builtin(canvas, self.builtin(), indicator, style, cx);
        true
```

`style` and `cx` are already parameters; drop the leading underscore if the
signature has one.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test interaction_gate -- check_button
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** both new tests pass, and
`checking_a_check_button_paints_the_builtin_check` still passes — it asserts
that the `check` subnode's pixel *changes* on click, which an unchecked box
that now paints something still satisfies (the check glyph differs from the
empty box).

### Step 4 — commit

```bash
git add ui/src/widgets/check_button.rs ui/tests/widget_pixels.rs
git commit -m "$(cat <<'EOF'
feat(ui): an unchecked CheckButton paints its indicator

M5-D8, third of four. The early `return false` becomes the empty-box branch:
background at 15% of the resolved colour plus a 1px border from `style`, in
every theme. The checked and inconsistent paths still go through
paint::icon::paint_builtin, unchanged.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15 — `Scrollbar` paints its trough and slider (M5-D8, fourth)

**Files:** `ui/src/widgets/scrollbar.rs`, `ui/tests/widget_pixels.rs`,
`ui/tests/interaction_gate.rs`.

**Interfaces**

*Consumes:* nothing.

*Produces:* `ScrollbarC::paint`, using the same `slider_rect` geometry
`on_event` hit-tests with. `measure` is unchanged (`(40,14)`/`(14,40)`).

### Step 1 — the failing test

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_scrollbar_paints_its_trough_and_slider() {
    // `range`/`trough`/`slider` are controller-owned subnodes with no taffy
    // box, so M2's box painting never reaches them: this controller is the
    // only thing that can draw a scrollbar at all.
    // mutation: return `false` from `ScrollbarC::paint`; the frame is flat.
    use icedtea_ui::view::builders::scrollbar;
    use icedtea_ui::widgets::Orientation;
    use icedtea_ui::widgets::scrollbar::ScrollbarExt;

    let frames = run(
        0.0_f64,
        |model: &mut f64, v: f64| {
            *model = v;
            Cmd::None
        },
        |model: &f64| {
            scrollbar(Orientation::Horizontal)
                .lower(0.0)
                .upper(100.0)
                .page_size(20.0)
                .value(*model)
                .hexpand(true)
        },
        (200, 40),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (200, 40)), "the scrollbar painted nothing");
    let mut seen = std::collections::HashSet::new();
    for x in 0..200 {
        for y in 0..40 {
            if let Some(px) = frames.pixel(0, x, y) {
                seen.insert(px);
            }
        }
    }
    assert!(
        seen.len() >= 3,
        "trough and slider must differ from the background, saw {} colours",
        seen.len()
    );
}

#[test]
fn a_scrollbars_slider_moves_with_its_value() {
    // The paint reads the same `slider_rect` the hit test does, so a value
    // change moves what is drawn.
    // mutation: paint the slider at a fixed x; the two frames match and this
    // fails.
    use icedtea_ui::view::builders::scrollbar;
    use icedtea_ui::widgets::Orientation;
    use icedtea_ui::widgets::scrollbar::ScrollbarExt;

    fn bar(model: &f64) -> View<f64> {
        scrollbar(Orientation::Horizontal)
            .lower(0.0)
            .upper(100.0)
            .page_size(20.0)
            .value(*model)
            .hexpand(true)
    }
    let left = run(0.0_f64, |_m: &mut f64, _v: f64| Cmd::None, bar, (200, 40), vec![ScriptStep::Capture]);
    let right = run(80.0_f64, |_m: &mut f64, _v: f64| Cmd::None, bar, (200, 40), vec![ScriptStep::Capture]);
    let differs = (0..200).any(|x| (0..40).any(|y| left.pixel(0, x, y) != right.pixel(0, x, y)));
    assert!(differs, "the slider did not move with the value");
}
```

`ui/tests/interaction_gate.rs`:

```rust
/// Ties the new paint to the hit geometry: dragging the slider must move the
/// pixels the widget draws, not just the value it reports.
///
/// Mutation check: paint the slider from `self.value` while hit-testing from
/// `self.adj.value` (they can disagree once `sanitized` clamps); the drag
/// reports a value but the pixels do not follow, and this fails. Restore.
#[test]
fn dragging_a_scrollbar_slider_moves_what_it_paints() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "scrollbar");
    let alloc = driver.allocation("scrollbar");
    let (x, y) = driver.point("scrollbar", "root");
    let left_edge = alloc.x as i32 + 2;
    let target = (alloc.x as i32 + alloc.width as i32 - 3, y);
    let before = driver.row(left_edge, target.0, y);

    driver.drag((x, y), target);

    assert!(
        gallery
            .messages()
            .iter()
            .any(|line| line.starts_with("value scrollbar ")),
        "the drag reported no value; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_row_change(left_edge, target.0, y, &before);
    assert!(
        !support::row_matches(&after, &before),
        "the slider's pixels did not move with the drag"
    );
}
```

```bash
cargo test -p icedtea-ui --test widget_pixels -- scrollbar
cargo test -p icedtea-ui --test interaction_gate -- scrollbar
```

**Expected failure:** all three fail — the widget draws nothing, so `has_ink`
is false and the rows never change.

### Step 2 — the implementation

`ui/src/widgets/scrollbar.rs`, a new `paint` in the `Controller` impl (after
`measure`):

```rust
    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // `range`/`trough`/`slider` are subnodes this controller appends
        // itself, so they have no taffy box and M2's box painting never
        // reaches them: nothing draws a scrollbar but this. The geometry is
        // `slider_rect` over the leaf's own content box — the *same* rect
        // `on_event` hit-tests through `content_rect_local` — so the drawn
        // slider and the grabbed slider cannot drift.
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        let colour = style.color();
        let trough_colour = crate::css::value::Rgba {
            a: colour.a * 0.15,
            ..colour
        };
        canvas.draw_rect(&content.to_skia(), &crate::paint::fill_paint(trough_colour));
        let local = Rect::new(0.0, 0.0, content.width, content.height);
        let slider = self.slider_rect(local, self.orientation);
        let slider = Rect::new(
            content.x + slider.x,
            content.y + slider.y,
            slider.width,
            slider.height,
        );
        if slider.is_empty() {
            return true;
        }
        canvas.draw_rect(&slider.to_skia(), &crate::paint::fill_paint(colour));
        true
    }
```

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test interaction_gate
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the two pixel tests and the new gate pass; the existing
`dragging_a_scrollbar_slider_reports_the_new_value` in `widget_pixels.rs` still
passes unchanged (the hit geometry did not move).

**Mutation check:** restore an unconditional `return false` at the top of
`paint`; `a_scrollbar_paints_its_trough_and_slider` fails. Restore.

### Step 4 — commit

```bash
git add ui/src/widgets/scrollbar.rs ui/tests/widget_pixels.rs ui/tests/interaction_gate.rs
git commit -m "$(cat <<'EOF'
feat(ui): a Scrollbar paints its trough and slider

M5-D8, last of four. The paint reads `slider_rect` over the leaf's own content
box — the same geometry `on_event` hit-tests through `content_rect_local` — so
the drawn slider and the grabbed slider are one rect. A new interaction gate
drags the slider and asserts the pixels follow.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16 — `KNOWN_BLANK_AT_REST` drops to six (M5-D8)

**Files:** `ui/tests/gallery_gate.rs`, `ui/README.md`.

**Interfaces**

*Consumes:* Tasks 13, 14, 15.

*Produces:*

```rust
const KNOWN_BLANK_AT_REST: &[&str] = &[
    "window_controls", "font_dialog", "popover_menu", "popover_menu_bar",
    "alert_dialog", "link_button",
];
```

plus `gallery_gate::the_readme_names_every_known_blank_widget`.

### Step 1 — the failing test

`ui/tests/gallery_gate.rs`, beside `the_readme_widget_table_lists_every_kind`:

```rust
/// The README's blank-widget prose and the const cannot drift.
///
/// mutation: drop one name from the README paragraph; this fails and names it.
#[test]
fn the_readme_names_every_known_blank_widget() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("ui/README.md is readable");
    let section = readme
        .split_once("<!-- known-blank:begin -->")
        .expect("the README has a `<!-- known-blank:begin -->` marker")
        .1
        .split_once("<!-- known-blank:end -->")
        .expect("the README has a `<!-- known-blank:end -->` marker")
        .0;
    for widget in KNOWN_BLANK_AT_REST {
        assert!(
            section.contains(&format!("`{widget}`")),
            "the README's blank-widget list does not name `{widget}`"
        );
    }
    let named = section.matches('`').count() / 2;
    assert_eq!(
        named,
        KNOWN_BLANK_AT_REST.len(),
        "the README names {named} blank widgets, the const has {}",
        KNOWN_BLANK_AT_REST.len()
    );
}
```

```bash
cargo test -p icedtea-ui --test gallery_gate -- the_readme_names_every_known_blank_widget
```

**Expected failure:** `the README has a `<!-- known-blank:begin -->` marker`
(the markers do not exist yet).

### Step 2 — the implementation

`ui/tests/gallery_gate.rs`: shorten the const to the six, and rewrite its doc
comment's mutation note (the old one names `scrollbar`, which is no longer on
the list):

```rust
/// …existing prose about why these are measured rather than asserted on…
///
/// M5-D8 removed `color_dialog`, `check_button` and `scrollbar`: all three now
/// paint at rest (`ColorDialogC::paint`, `CheckButtonC::paint`'s empty-box
/// branch, `ScrollbarC::paint`). `color_dialog_button` was never on the list.
/// The six that remain are the ones M5 does not touch.
///
/// Mutation check: re-add `"scrollbar"`; nothing fails, which shows the entry
/// would now be hiding a widget that paints — that is why it is gone. The
/// opposite check is the real one: delete `ScrollbarC::paint` and the
/// light-theme test fails with "scrollbar painted nothing in the light theme".
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

`ui/README.md`, "## The M3 gates" — replace the sentence that says "except the
nine entries … (seven collapse to a zero-area allocation…)" with:

```markdown
  and high-contrast Adwaita — except the six entries listed in that file's
  `KNOWN_BLANK_AT_REST`, which are measured, not asserted on, because they
  render nothing today:
<!-- known-blank:begin -->
  five collapse to a zero-area allocation (`window_controls`, `font_dialog`,
  `popover_menu`, `popover_menu_bar`, `alert_dialog`), and one has a real
  allocation it draws nothing into (`link_button`).
<!-- known-blank:end -->
  The list was fifteen until the M3 close-out's first fix wave (P8-D75 and the
  pooled-row measure), and nine until M5-D8 gave `color_dialog`,
  `check_button` and `scrollbar` a rest paint. Every entry, exempt or not, must
  still appear whole in some slice.
```

Keep the rest of that bullet — the `--list`, probe-point, node-tree and
README-table claims — exactly as it is.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test gallery_gate
```

**Expected:** all three `every_widget_renders_at_rest_in_the_*_theme` tests
pass with the three de-listed widgets now asserted on, and the new README test
passes. This is the slowest gate in the crate (screencopy under the harness,
three themes); expect minutes.

### Step 4 — commit

```bash
git add ui/tests/gallery_gate.rs ui/README.md
git commit -m "$(cat <<'EOF'
test(ui): KNOWN_BLANK_AT_REST drops from nine entries to six

M5-D8's close: color_dialog, check_button and scrollbar now paint at rest and
are asserted on in all three themes. The README's blank list is marked up and
pinned by a new gallery_gate test, and its stale "seven collapse to zero-area"
sentence is corrected to five plus one.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17 — `Window::probe_points` / `Window::allocation` (M5-D9)

**Files:** `ui/src/window/mod.rs`, `ui/src/bin/window-probe.rs`,
`ui/tests/support/mod.rs`, `ui/tests/window_events.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`.

**Interfaces**

*Consumes:* Task 2's `spawn_window_probe_with`.

*Produces:*

```rust
// ui/src/window/mod.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint { pub label: String, pub x: i32, pub y: i32 }

/// Label-and-centre every laid-out node under `root`. Shared by
/// `Window::probe_points` and `App`'s report writer (Task 18).
#[must_use]
pub fn probe_points_of(root: &Node, layout: &crate::layout::LayoutTree) -> Vec<ProbePoint>;

/// The border box of the node whose `Node::id()` is `id`.
#[must_use]
pub fn allocation_of(
    root: &Node,
    layout: &crate::layout::LayoutTree,
    id: &str,
) -> Option<crate::layout::Allocation>;

impl Window {
    #[must_use] pub fn probe_points(&self) -> Vec<ProbePoint>;
    #[must_use] pub fn allocation(&self, id: &str) -> Option<crate::layout::Allocation>;
}
```

and `window-probe --emit-probe`, which writes `probe <label> <x> <y>` and
`alloc <id> <x> <y> <w> <h>` lines once the tree is laid out.

### Step 1 — the failing test

`ui/tests/window_events.rs`:

```rust
#[test]
fn probe_points_locate_a_live_windows_widgets() {
    // Every M5 gate addresses widgets by id rather than by hard-coded
    // coordinates; on a live window `App::probe` (offscreen-only) cannot
    // answer, so `Window::probe_points` must.
    // mutation: return `Vec::new()` from `probe_points_of`; no `probe ` line
    // is ever written and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = support::spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
        &["--emit-probe"],
    );
    assert!(
        wait_for_report_line(report.path(), "probe ", Duration::from_secs(20)).is_some(),
        "the probe emitted no probe points: {:?}",
        probe_report(report.path())
    );
    let points: Vec<support::ProbePoint> = probe_report(report.path())
        .into_iter()
        .filter_map(|line| line.strip_prefix("probe ").map(str::to_owned))
        // `parse_probe_line` wants `<widget> <label> <x> <y>`; a window's
        // lines have no widget column, so the label stands in for both.
        .filter_map(|rest| support::parse_probe_line(&format!("window {rest}")))
        .collect();
    assert!(
        points.iter().any(|p| p.label == "entry"),
        "the entry is not among the probe points: {points:?}"
    );
    assert!(
        points.iter().any(|p| p.label == "menubutton"),
        "the menubutton is not among the probe points: {points:?}"
    );
    for point in &points {
        assert!(
            point.x >= 0 && point.y >= 0,
            "a probe point must be inside the surface: {point:?}"
        );
    }
}

#[test]
fn allocation_by_id_matches_the_probe_point_centre() {
    // mutation: return the border box un-floored (`as i32` on the raw centre)
    // in `probe_points_of`; a half-pixel centre rounds the other way and the
    // equality below fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = support::spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
        &["--emit-probe"],
    );
    assert!(
        wait_for_report_line(report.path(), "alloc entry ", Duration::from_secs(20)).is_some(),
        "no allocation line for the entry: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let alloc = lines
        .iter()
        .find_map(|l| l.strip_prefix("alloc "))
        .and_then(support::parse_allocation_line)
        .expect("an allocation line parses");
    let point = lines
        .iter()
        .find_map(|l| l.strip_prefix("probe "))
        .map(|rest| format!("window {rest}"))
        .and_then(|line| support::parse_probe_line(&line))
        .expect("a probe line parses");
    assert_eq!(alloc.widget, "entry", "the first alloc line is the entry's");
    assert_eq!(point.label, "entry", "and the first probe line matches it");
    assert_eq!(
        (point.x, point.y),
        (
            (alloc.x + alloc.width / 2.0).floor() as i32,
            (alloc.y + alloc.height / 2.0).floor() as i32
        ),
        "the probe point is the floored centre of the allocation"
    );
}
```

```bash
cargo test -p icedtea-ui --test window_events -- probe_points allocation_by_id
```

**Expected failure:** `the probe emitted no probe points` — the flag does
nothing yet.

### Step 2 — the implementation

`ui/src/window/mod.rs`:

```rust
/// One probe point on a live window, in window-surface coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    /// The node's `id` if it has one, else its CSS node name, indexed when the
    /// name repeats — `gallery::probe_points_of`'s labelling rule.
    pub label: String,
    /// Centre of the border box, floored.
    pub x: i32,
    /// Centre of the border box, floored.
    pub y: i32,
}

/// Label and centre every laid-out node under `root`.
///
/// Same derivation as `gallery::probe_points_of`: walk `root.descendants()`,
/// label by id-or-node-name with a repeat index, take the centre of the border
/// box **floored** (`f32::floor` before the cast, not `as i32`, which
/// truncates toward zero, and not `round`, which ties away from it: only
/// flooring makes an integer shift of a box shift its centre by that same
/// integer). A node with no allocation is skipped. Cheap: it reads the layout
/// tree the last frame already computed and lays nothing out.
#[must_use]
pub fn probe_points_of(root: &Node, layout: &crate::layout::LayoutTree) -> Vec<ProbePoint> {
    use std::collections::BTreeMap;

    let centre = |label: String, alloc: &crate::layout::Allocation| {
        let r = alloc.border_box;
        ProbePoint {
            label,
            x: (r.x + r.width / 2.0).floor() as i32,
            y: (r.y + r.height / 2.0).floor() as i32,
        }
    };
    let label_of = |node: &Node, index: usize, repeats: bool| -> String {
        node.id().map_or_else(
            || {
                let name = node.name();
                if repeats {
                    format!("{name}{index}")
                } else {
                    name.to_string()
                }
            },
            |id| id.to_string(),
        )
    };

    let mut points = Vec::new();
    if let Some(alloc) = layout.allocation(root) {
        points.push(centre("root".to_string(), &alloc));
    }
    let descendants: Vec<Node> = root.descendants().collect();
    let mut counts: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        *counts.entry(node.name()).or_default() += 1;
    }
    let mut seen: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        let name = node.name();
        let index = seen.entry(name.clone()).or_default();
        let repeats = counts.get(&name).copied().unwrap_or(0) > 1;
        let label = label_of(node, *index, repeats);
        *index += 1;
        if let Some(alloc) = layout.allocation(node) {
            points.push(centre(label, &alloc));
        }
    }
    points
}

/// The allocation of the node under `root` whose [`Node::id`] is `id`.
#[must_use]
pub fn allocation_of(
    root: &Node,
    layout: &crate::layout::LayoutTree,
    id: &str,
) -> Option<crate::layout::Allocation> {
    root.descendants()
        .find(|node| node.id().is_some_and(|found| &*found == id))
        .and_then(|node| layout.allocation(&node))
}
```

and on `Window`, beside `root`:

```rust
    /// Every laid-out node of this window's tree, labelled and centred.
    ///
    /// The live counterpart of `App::probe`, which is offscreen-only: a
    /// harness test against a running client has no other way to ask where a
    /// widget ended up, and the gate rules forbid hard-coded coordinates.
    /// Reads the tree the last [`Window::render`] laid out.
    #[must_use]
    pub fn probe_points(&self) -> Vec<ProbePoint> {
        probe_points_of(&self.root, &self.layout)
    }

    /// The border box of the node whose [`Node::id`] is `id`, in
    /// window-surface coordinates. `None` for an unknown id or a node that has
    /// not been laid out.
    #[must_use]
    pub fn allocation(&self, id: &str) -> Option<crate::layout::Allocation> {
        allocation_of(&self.root, &self.layout, id)
    }
```

`App::probe`/`Probe<Msg>` and the gallery's `--probe-points`/`--print-allocation`
line format are untouched.

`ui/src/bin/window-probe.rs`: read the flag beside the others,

```rust
    let emit_probe = std::env::args().any(|a| a == "--emit-probe");
```

and, in the entry/menu loop, once per iteration after `window.render()`:

```rust
        if emit_probe && !emitted_probe {
            let points = window.probe_points();
            if points.len() > 1 {
                // Only once the tree has really been laid out: before the
                // first configure every node is at the origin with no size.
                for point in &points {
                    report(&format!("probe {} {} {}", point.label, point.x, point.y));
                }
                for id in ["entry", "menubutton"] {
                    if let Some(alloc) = window.allocation(id) {
                        let r = alloc.border_box;
                        report(&format!(
                            "alloc {id} {} {} {} {}",
                            r.x, r.y, r.width, r.height
                        ));
                    }
                }
                emitted_probe = true;
            }
        }
```

with `let mut emitted_probe = false;` beside `let mut frames = 0_u32;`.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test window_events
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the two new tests pass and the existing `window_events` tests are
untouched (the flag is off unless passed).

**Mutation check:** replace `(r.x + r.width / 2.0).floor() as i32` with
`(r.x + r.width / 2.0) as i32`; `allocation_by_id_matches_the_probe_point_centre`
fails on any box whose centre lands on a half pixel. Restore.

### Step 4 — docs and the record

`ui/README.md`, "## The window and event layer", after "### Watching a foreign fd":

```markdown
### Probing a live window

`Window::probe_points()` labels and centres every laid-out node — id, else CSS
node name with a repeat index — and `Window::allocation(id)` returns one node's
border box, both in window-surface coordinates and both read off the tree the
last frame laid out. A client that sets `$ICEDTEA_PROBE_REPORT` writes them as
`probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines, which
`ui/tests/support/mod.rs` parses back. This is how a harness test addresses a
widget on a *running* app: `App::probe` is offscreen-only.
```

M3 contract §10: `### M5-D9 — probing works on a live `Window`, not only
offscreen (extends P8-D59/P8-D60)`.

### Step 5 — commit

```bash
git add ui/src/window/mod.rs ui/src/bin/window-probe.rs ui/tests/window_events.rs \
        ui/README.md docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): probe points and allocations on a live Window

M5-D9. `probe_points_of`/`allocation_of` label and centre a laid-out tree with
the gallery's own rule (floored centres, id-or-name with a repeat index), and
`Window` exposes both. `window-probe --emit-probe` writes the report lines
`ui/tests/support` already parses, and two window_events tests pin the labels
and the centre-vs-allocation identity.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18 — `App` writes the probe report (M5-D9, P0-D4)

**Files:** `ui/src/view/app.rs`, `ui/src/bin/window-probe.rs`,
`ui/tests/ingress.rs`, `ui/README.md`,
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6: P0-D4).

**Interfaces**

*Consumes:* Tasks 5, 6, 17.

*Produces:*

```rust
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Write `probe`/`alloc` report lines to `path` once per frame in which
    /// they change. `$ICEDTEA_PROBE_REPORT` sets this by default.
    #[must_use] pub fn with_probe_report(self, path: std::path::PathBuf) -> Self;
}
```

### Step 1 — the failing test

`ui/tests/ingress.rs`:

```rust
#[test]
fn a_running_app_writes_its_probe_report() {
    // Why the toolkit does this and not the app (P0-D4): `App::run` owns the
    // loop and lays the tree out into its own Runtime, so an app that has
    // handed over its window has no frame hook and no laid-out tree. Every M5
    // gate addresses widgets by id, so without this P1..P5 have no gates.
    // mutation: never call the report writer in `App::run`; no `probe ` line
    // appears and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "app-inbox",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "alloc status ", REPORT).is_some(),
        "the running app never reported its `status` allocation: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let probes: Vec<&String> = lines.iter().filter(|l| l.starts_with("probe ")).collect();
    assert!(
        probes.iter().any(|l| l.starts_with("probe status ")),
        "the id-labelled probe point is missing: {probes:?}"
    );
    // Written when they change, not once per frame: a settled app must not
    // grow the file without bound.
    let settled = lines.iter().filter(|l| l.starts_with("probe status ")).count();
    assert!(
        settled <= 4,
        "the report repeats unchanged lines ({settled} copies); it must only \
         write when the tree changed"
    );
}
```

```bash
cargo test -p icedtea-ui --test ingress -- a_running_app_writes_its_probe_report
```

**Expected failure:** `the running app never reported its `status` allocation`.

### Step 2 — the implementation

`ui/src/view/app.rs`. A third new field on `App`:

```rust
    /// Where `run` writes `probe`/`alloc` lines, when asked (M5-D9, P0-D4).
    probe_report: Option<std::path::PathBuf>,
```

`App::new` initialises it from the environment, so an app opts in by being run
with `$ICEDTEA_PROBE_REPORT` set and needs no code of its own:

```rust
            probe_report: std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from),
```

```rust
    /// Write `probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines to
    /// `path`, once per frame in which they change.
    ///
    /// `App::new` already picks `$ICEDTEA_PROBE_REPORT` up; this is the
    /// explicit form. The lines are exactly what `ui/tests/support/mod.rs`
    /// parses, and the labels are `Window::probe_points`'.
    #[must_use]
    pub fn with_probe_report(mut self, path: std::path::PathBuf) -> Self {
        self.probe_report = Some(path);
        self
    }
```

The writer, a free function beside `frame_deadline`:

```rust
/// Append `lines` to `path` when they differ from `last`.
///
/// Deduplicated by content: a settled app writes nothing, so the file stays
/// bounded no matter how long the app runs, and a test that greps for a line
/// still finds it.
fn write_probe_report(path: &std::path::Path, lines: &[String], last: &mut Vec<String>) {
    use std::io::Write;

    if lines == last.as_slice() {
        return;
    }
    last.clear();
    last.extend_from_slice(lines);
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    for line in lines {
        let _ = writeln!(file, "{line}");
    }
    let _ = file.flush();
}

/// The report lines for one laid-out tree: every probe point, then one
/// `alloc` line per node that has an id.
fn probe_report_lines<Msg>(rt: &Runtime<Msg>) -> Vec<String> {
    let mut lines: Vec<String> = crate::window::probe_points_of(&rt.root, &rt.layout)
        .into_iter()
        .map(|p| format!("probe {} {} {}", p.label, p.x, p.y))
        .collect();
    for node in rt.root.descendants() {
        let Some(id) = node.id() else { continue };
        let Some(alloc) = rt.layout.allocation(&node) else {
            continue;
        };
        let r = alloc.border_box;
        lines.push(format!("alloc {id} {} {} {} {}", r.x, r.y, r.width, r.height));
    }
    lines
}
```

In `App::run`, `let mut reported: Vec<String> = Vec::new();` before the loop,
and after `restyle_and_layout(..)` — the tree is laid out, the frame is about
to be painted — :

```rust
            if let Some(path) = self.probe_report.as_deref() {
                let lines = probe_report_lines(&rt);
                write_probe_report(path, &lines, &mut reported);
            }
```

`run_offscreen` does not write: an offscreen test reads `App::probe` or the
returned frames directly.

`ui/src/bin/window-probe.rs`: `run_app_inbox`'s view already carries
`.id("status")`, so nothing changes there — the report appears because the
probe process is spawned with `$ICEDTEA_PROBE_REPORT` set, which is exactly how
`settings` and `shell` will be run under their gates.

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui --test gallery_gate
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```

**Expected:** the new test passes. `gallery_gate` still passes and its timings
do not change: the gallery is never spawned with `$ICEDTEA_PROBE_REPORT`, so
the writer is inert there.

### Step 4 — docs and the record

`ui/README.md`, extend "### Probing a live window":

```markdown
An app driven by `App::run` gets this for free: with `$ICEDTEA_PROBE_REPORT`
set (or `App::with_probe_report(path)`), the loop writes those lines once per
frame in which they change — deduplicated, so a settled app writes nothing.
That is what M5's settings and shell gates read.
```

M5 contract §6: `### P0-D4`, with this plan's deviation text — including the
consequence for P1 and P5 (their "add the writer" step is satisfied by the
toolkit; they only set ids).

### Step 5 — commit

```bash
git add ui/src/view/app.rs ui/tests/ingress.rs ui/README.md \
        docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'EOF'
feat(ui): App::run writes the $ICEDTEA_PROBE_REPORT lines itself

M5-D9 by P0-D4's route: an app that hands its window to `App::run` has no
per-frame hook and no laid-out tree of its own, so the loop emits the
probe/alloc lines — deduplicated, so a settled app writes nothing. P1 and P5
then only have to set widget ids, which is what their gates address.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 19 — `ui/README.md` review pass (M5-D11)

**Files:** `ui/README.md`, `ui/tests/gallery_gate.rs` (assertion only).

**Interfaces**

*Consumes:* every earlier task's README subsection.

*Produces:* one coherent document — M5-D11's six required additions, present
and consistent.

### Step 1 — the failing test

`ui/tests/gallery_gate.rs`:

```rust
/// M5-D11: the README documents every M5 P0 addition. A cheap, exact check —
/// the names are the API, so a rename that skips the docs fails here.
///
/// mutation: delete the "External events" heading from the README; this fails
/// and names it.
#[test]
fn the_readme_documents_the_m5_toolkit_additions() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("ui/README.md is readable");
    for needle in [
        "### External events",
        "### Pointer events at the view layer",
        "### Base-level keysyms",
        "### Probing a live window",
        "### Watching a foreign fd",
        "Inbox",
        "InboxSender",
        "App::with_inbox",
        "App::on_fd",
        "Window::watch_fd",
        "InputEvent::FdReady",
        "Cmd::Task",
        "impl FnMut",
        "Handler::PairButton",
        "on_pointer_up_with_button",
        "BTN_MIDDLE",
        "Keymap::base_keysym",
        "Window::probe_points",
        "$ICEDTEA_PROBE_REPORT",
    ] {
        assert!(
            readme.contains(needle),
            "ui/README.md does not document `{needle}`"
        );
    }
    assert!(
        !readme.contains("seven collapse to a zero-area allocation"),
        "the stale blank-widget sentence is still in the README"
    );
}
```

```bash
cargo test -p icedtea-ui --test gallery_gate -- the_readme_documents
```

**Expected failure:** whichever needles the earlier tasks' subsections did not
already land — run it and read the list; it is the to-do for Step 2.

### Step 2 — the implementation

Reread `ui/README.md` end to end and make the six M5-D11 additions coherent
rather than six appended paragraphs:

1. **"External events"** (Tasks 3, 5, 7) — one subsection covering `Inbox`/
   `InboxSender`, `App::with_inbox`, `App::on_fd`, the drain order, the
   `Msg: Send`/`Arc`-not-`Rc` rule and `Cmd::Task`'s "must not block". Move
   `App::on_fd`'s sentence in from wherever Task 6 left it.
2. **"Closures in `App::new`"** (Task 4) — a short paragraph in the reactive
   section; `fn` items still coerce.
3. **"Pointer events at the view layer"** (Tasks 8, 9) — the three kinds,
   `Handler::PairButton`, the six builders, the grab semantics, `BTN_*`.
4. **"Base-level keysyms"** (Task 12) — in the window/event section, with the
   normalisation rule quoted.
5. **"Probing a live window"** (Tasks 17, 18).
6. **The correction** (Task 16) — already made; verify the markers are intact
   and the count reads five zero-area plus one that allocates and paints
   nothing.

Also update the "## The module map" entry for `view/` to mention `inbox.rs`,
and the `interaction_gate` line in "## The M3 gates" from "16 interactions" to
the new count (16 + the three M5 additions: drawing-area drag, middle click,
scrollbar slider paint = 19).

### Step 3 — run it

```bash
cargo test -p icedtea-ui --test gallery_gate
```

**Expected:** `the_readme_documents_the_m5_toolkit_additions`,
`the_readme_names_every_known_blank_widget` and
`the_readme_widget_table_lists_every_kind` all pass.

### Step 4 — commit

```bash
git add ui/README.md ui/tests/gallery_gate.rs
git commit -m "$(cat <<'EOF'
docs(ui): fold the M5 P0 additions into the README, and pin them

M5-D11. External events, closures in App::new, view-layer pointer events,
base-level keysyms and live-window probing each get their own subsection, the
module map lists view/inbox.rs, the gate counts are corrected, and a
gallery_gate test asserts every one of those names is documented.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 20 — the part gate

**Files:** none (verification only), except a fix if something is red.

**Interfaces**

*Consumes:* Tasks 1–19.

*Produces:* a green part.

### Step 1 — run the whole gate set

```bash
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo doc -p icedtea-ui --no-deps
cargo test -p icedtea-ui
cargo test --workspace
```

and, named because they are the milestone's inherited gates:

```bash
cargo test -p icedtea-ui --test themed_button_offscreen
cargo test -p icedtea-ui --test adwaita_coverage
cargo test -p icedtea-ui --test gtk4_property_reference
cargo test -p icedtea-ui --test layer_shell_screencopy
cargo test -p icedtea-ui --test transition_screencopy
cargo test -p icedtea-ui --test window_events
cargo test -p icedtea-ui --test counter_app
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test reconcile_props
cargo test -p icedtea-ui --test gallery_gate
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test ingress
```

**Expected:** everything green. `interaction_gate` is now 19 tests, all
passing; `gallery_gate` asserts on three widgets it used to exempt.

### Step 2 — the byte-identical gate

```bash
git diff --stat develop -- ui/tests/layer_shell_screencopy.rs
```

**Expected:** empty. That file is a byte-identical M1 gate; if it changed,
revert it and find what forced the change (almost certainly a `BTN_LEFT` path
move, which P0-D3 exists to prevent).

### Step 3 — the ownership audit

```bash
git diff --name-only develop
```

**Expected:** exactly the files in this plan's File Structure table, and
nothing under `settings/`, `shell/`, `clipboard/`, `compositor/`, `harness/`,
`contract/`, `config/`, `ui/src/css/`, `ui/src/anim/`, `ui/src/layout.rs`,
`ui/src/text.rs`, or any widget module other than
`color_dialog`/`check_button`/`scrollbar`/`drawing_area`.

### Step 4 — the P4/P5 read (spec §9, binding for the part review)

Before declaring the part done, re-read
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §2.5, §2.8 and §3 and
confirm, in writing, in the part-review notes:

- **§2.5 outputs pump vs the ingress drain order.** `OutputsPump::attach`
  needs `Window::watch_fd(dup) -> WatchId` before `App::new`, and
  `App::on_fd(id, move || pump.drain())` after — both exist and take those
  types; the pump's messages land before the frame's input batch, which is what
  makes a `HeadsChanged` reconcile before a click on the canvas is folded.
- **§2.8 Displays canvas.** `drawing_area(|canvas, rect, cx| ..)` can shape the
  connector name (Task 10), and `on_pointer_down`/`_motion`/`_up` deliver the
  whole drag to the canvas through the grab (Tasks 8, 9).
- **§3.3 shell window buttons.** `button(..).on_pointer_up_with_button(..)`
  fires — P0-D1 is what makes that true, since `ButtonC` is not `GenericC`.
- **Every gate in §5.** `Window::probe_points`, `Window::allocation` and the
  `App` report writer address widgets by id on a live window (Tasks 17, 18).

If any of these does not hold, the part is not done: fix it here rather than
leaving P1–P5 to discover it.

### Step 5 — commit

Only if Steps 1–4 produced a fix:

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(ui): <what the gate caught>

Found by M5 P0's part gate: <one line>.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

Otherwise nothing to commit — the part is green as it stands. **Do not merge,
push, or open a PR.**

---

## Self-Review

### Spec and contract coverage

| Contract item | Spec §4 | Task(s) | Test(s) | Deviation |
|---|---|---|---|---|
| M5-D1 `watch_fd`/`unwatch`/`watches`, `Interest`, N-fd `wait_bounded`, `InputEvent::FdReady` | 4.1 | 1, 2 | `interest_maps_onto_poll_flags`, `a_watch_set_polls_the_wayland_fd_first_…`, `a_watched_pipe_wakes_pump_with_fd_ready`, `an_unwatched_fd_stops_waking_the_loop`, `two_watched_fds_report_in_registration_order`, `a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout`, `unwatching_an_unknown_id_is_a_no_op`, `an_idle_watch_costs_no_extra_wakeups` | — |
| M5-D2 `Inbox`/`InboxSender`, `with_inbox`, `on_fd`, drain order | 4.1 | 3, 5, 6 | `a_sent_message_is_queued_in_send_order_and_wakes_the_pipe`, `a_sender_survives_being_cloned_across_threads`, `sending_after_the_inbox_is_dropped_returns_the_message`, `a_full_wake_pipe_does_not_block_a_sender`, `inbox_messages_are_applied_in_send_order`, `inbox_messages_are_applied_before_the_frames_input_batch`, `an_inbox_message_reaches_update_on_the_next_frame`, `on_fd_maps_a_foreign_fd_to_messages`, `an_inbox_message_wakes_a_live_app` | P0-D5, P0-D6 |
| M5-D3 `Cmd::Task` | 4.1 (D8) | 7 | `a_task_is_a_flatten_leaf_and_prints_opaquely`, `a_task_command_runs_once_after_the_fold_offscreen_too` | — |
| M5-D4 `App::new` closures | 4.2 | 4 | `a_closure_capturing_state_drives_the_loop`, `an_fn_item_still_coerces_into_app_new` | — |
| M5-D5 pointer kinds, `PairButton`, builders, `BTN_*` | 4.3 | 8, 9, 11 | `event_kind_all_lists_twenty_one_kinds`, `a_pair_button_handler_falls_back_to_a_pair_handler`, `the_linux_button_codes_are_reachable_from_one_module`, `pointer_handlers_fire_down_motion_up_in_order_with_local_coordinates`, `a_release_outside_the_node_still_reaches_it_through_the_grab`, `dragging_across_a_drawing_area_reports_every_pointer_phase`, `a_middle_click_on_a_drawing_area_reports_the_middle_button_code` | P0-D1, P0-D2, P0-D3 |
| M5-D6 `Prop::Draw` carries `PaintCx` | 4.3 (P4 need) | 10 | `a_drawing_area_can_shape_text_through_its_paint_cx`, plus the unchanged `a_drawing_area_runs_its_callback_against_the_allocated_rect` | — |
| M5-D7 `base_keysym`, `KeyEvent::base` | 4.4 | 12 | `base_keysym_reports_the_level_zero_sym_for_a_shifted_key`, `base_keysym_is_group_and_caps_agnostic`, `base_keysym_of_an_unmapped_keycode_is_no_symbol`, `translate_stamps_base_on_every_key_event` | — |
| M5-D8 rest paint ×4, `KNOWN_BLANK_AT_REST` → 6 | 4.5 | 13, 14, 15, 16 | `a_color_dialog_button_paints_its_swatch_at_rest`, `a_color_dialog_paints_its_palette_at_rest`, `a_color_dialogs_measured_box_is_the_grid_it_paints`, `an_unchecked_check_button_paints_its_box`, `a_checked_check_button_still_paints_the_builtin`, `a_scrollbar_paints_its_trough_and_slider`, `a_scrollbars_slider_moves_with_its_value`, `dragging_a_scrollbar_slider_moves_what_it_paints`, the three `every_widget_renders_at_rest_in_the_*_theme`, `the_readme_names_every_known_blank_widget` | — |
| M5-D9 `probe_points`/`allocation` on a live window | 4.6 | 17, 18 | `probe_points_locate_a_live_windows_widgets`, `allocation_by_id_matches_the_probe_point_centre`, `a_running_app_writes_its_probe_report` | P0-D4 |
| M5-D10 `ui/Cargo.toml` deps | — | 1 (`pipe`), 3 (`crossbeam-channel`) | the whole suite, plus both clippy configurations | — |
| M5-D11 `ui/README.md` | 4.7 | every API task, 19 | `the_readme_documents_the_m5_toolkit_additions`, `the_readme_names_every_known_blank_widget`, `the_readme_widget_table_lists_every_kind` | — |

Spec §7's three P0 bullets: "`watch_fd` wakes `pump` from a foreign fd" (Task 2),
"inbox delivery order" (Task 5), "pointer events on `DrawingArea` through a
grab" (Tasks 9, 11), "`base_keysym` on the hermetic keymap" (Task 12), "three
rest-paint pixel tests" (Tasks 13, 14, 15 — four, in fact: the colour button and
the dialog are separate). Nothing in §4 is unclaimed, and no task implements
anything §4 does not name.

### Mutation checks

Every load-bearing test states one, in its own body or in the task's Step 3:

| Test | Mutation |
|---|---|
| `a_watched_pipe_wakes_pump_with_fd_ready` | drop the fired-watch loop from `wait_bounded` |
| `an_unwatched_fd_stops_waking_the_loop` | make `Watches::remove` a no-op |
| `two_watched_fds_report_in_registration_order` | reverse `Watches::entries()` |
| `a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout` | return `Timeout` whenever a watch is registered |
| `unwatching_an_unknown_id_is_a_no_op` | panic in `Watches::remove` on a missing id |
| `an_idle_watch_costs_no_extra_wakeups` | add `Duration::ZERO` to `frame_deadline` with a watch registered |
| `window_events` (whole file) | drop element 0 from the poll set — M5-D1's own check |
| `a_sent_message_is_queued_in_send_order_and_wakes_the_pipe` | drop the pipe write from `send` |
| `a_full_wake_pipe_does_not_block_a_sender` | drop `PipeFlags::NONBLOCK` |
| `sending_after_the_inbox_is_dropped_returns_the_message` | `unwrap` the channel send |
| `inbox_messages_are_applied_in_send_order` | drain the channel in reverse |
| `inbox_messages_are_applied_before_the_frames_input_batch` | move the drain after the routing loop — M5-D2's own check |
| `on_fd_maps_a_foreign_fd_to_messages` | drop the `fd_handlers` loop from `run` |
| `an_inbox_message_wakes_a_live_app` | never register the inbox fd |
| `a_task_is_a_flatten_leaf_and_prints_opaquely` | give `Task` a `Batch`-like `flatten` arm |
| `a_task_command_runs_once_after_the_fold_offscreen_too` | run the task inside `update` |
| `a_closure_capturing_state_drives_the_loop` | revert `App::new` to `fn` pointers |
| `an_fn_item_still_coerces_into_app_new` | take `Box<dyn FnMut>` directly |
| `event_kind_all_lists_twenty_one_kinds` | omit a kind from `ALL` |
| `a_pair_button_handler_falls_back_to_a_pair_handler` | delete the `Handler::Pair` arm of `fire_pair_button` |
| `pointer_handlers_fire_down_motion_up_in_order_…` | delete the `PointerMotion` arm of `fire_pointer_handlers` — M5-D5's check, relocated by P0-D1 |
| `a_release_outside_the_node_still_reaches_it_through_the_grab` | fire at `Phase::Bubble` instead of `Phase::Target` |
| `dragging_across_a_drawing_area_reports_every_pointer_phase` | same `PointerMotion` deletion, end to end |
| `a_middle_click_on_a_drawing_area_reports_the_middle_button_code` | fire `Pair` instead of `PairButton` |
| `a_drawing_area_can_shape_text_through_its_paint_cx` | pass a fresh empty `PaintCx` |
| `base_keysym_reports_the_level_zero_sym_for_a_shifted_key` | level 0 → 1 — M5-D7's check |
| `base_keysym_is_group_and_caps_agnostic` | read from `state.key_get_one_sym` |
| `base_keysym_of_an_unmapped_keycode_is_no_symbol` | `expect` the first sym |
| `a_color_dialog_button_paints_its_swatch_at_rest` | delete `ColorDialogButtonC::measure` |
| `a_color_dialog_paints_its_palette_at_rest` | return `false` from `ColorDialogC::paint` |
| `a_color_dialogs_measured_box_is_the_grid_it_paints` | hard-code a different column count in `intrinsic` |
| `an_unchecked_check_button_paints_its_box` | restore the early `return false` — M5-D8's per-widget check |
| `a_checked_check_button_still_paints_the_builtin` | return early for the checked state |
| `a_scrollbar_paints_its_trough_and_slider` | return `false` from `ScrollbarC::paint` |
| `a_scrollbars_slider_moves_with_its_value` | paint the slider at a fixed x |
| `dragging_a_scrollbar_slider_moves_what_it_paints` | paint from `self.value` while hit-testing `self.adj.value` |
| `every_widget_renders_at_rest_in_the_*` | delete any of the three new paints |
| `probe_points_locate_a_live_windows_widgets` | return `Vec::new()` from `probe_points_of` |
| `allocation_by_id_matches_the_probe_point_centre` | drop the `floor` — M5-D9's check |
| `a_running_app_writes_its_probe_report` | never call the writer in `run` |
| `the_readme_*` (three) | delete the named heading / list entry |

### Pure-core extractions

None. This part moves no code between crates and extracts no pure core — that
rule applies to P1's `settings/` extraction (contract §2.1), not here. The two
tests that move at all are `widget_pixels.rs`'s `key_char`/`key_named`
constructors, which gain one field each (Task 12) and stay where they are.

### App-page gates

None. No app page exists until P1; this part's harness gates are toolkit gates
(`gallery_gate`, `interaction_gate`, `window_events`, `ingress`). The rest-state
screencopy work this part does own is `KNOWN_BLANK_AT_REST` dropping to six,
which is `gallery_gate`'s three theme runs (Task 16). The interaction gates
spec §7 names for P0 — pointer events through a grab, and the rest-paint pixel
tests — are Tasks 9, 11, 13, 14, 15.

### Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `similar to Task`, `as above`,
`etc.` inside code, `...` inside a code block standing in for real code, and
prose-only steps:

- No `TBD`/`TODO`/`FIXME`.
- No "similar to Task N" cross-reference: every task's code is written out.
- The only `…` inside code blocks are in *signature summaries* of enums whose
  other variants are explicitly unchanged (`// … every M3 variant, unchanged …`),
  which is the contract's own convention and names no code to invent.
- Every step that changes code shows the code. The two steps that do not —
  Task 19's README pass and Task 20's gate — are a documentation edit against
  an assertion list and a command list respectively.
- Every type named in a signature exists today or is produced by an earlier,
  numbered task: `WatchId`, `Interest`, `Watch`, `Watches`, `ProbePoint`
  (Tasks 1, 17); `Inbox`, `InboxSender`, `SendError` (Task 3); everything else
  (`Node`, `LayoutTree`, `Allocation`, `Rect`, `Rgba`, `PaintCx`, `BuildCx`,
  `ComputedStyle`, `Canvas`, `Handlers`, `Handler`, `EventKind`, `Event`,
  `Phase`, `Cmd`, `View`, `Frames`, `ScriptStep`, `ManualClock`,
  `CompiledSheet`, `FontQuery`, `ShapeKey`, `FontFamily`, `GenericFamily`,
  `FontStyle`, `Keyword`, `xkb::Keysym`, `Compositor`, `Driver`, `Reaper`,
  `EntryAllocation`) is shipped in the crate or the harness today.

### Type consistency against the contract

- `Window::watch_fd(&mut self, fd: OwnedFd, interest: Interest) -> WatchId`,
  `unwatch(WatchId)`, `watches() -> Vec<WatchId>` — as M5-D1.
- `InputEvent::FdReady(WatchId)`, last variant — as M5-D1.
- `Inbox::new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)>`,
  `Inbox::sender() -> InboxSender<Msg>`,
  `InboxSender::send(&self, Msg) -> Result<(), SendError<Msg>>`,
  `SendError<Msg>(pub Msg)` — as M5-D2, minus the unneeded `unsafe impl` (P0-D5)
  and plus the `pub(crate)` drain/dup helpers `App` needs (P0-D6).
- `App::with_inbox(self, Inbox<Msg>) -> Self where Msg: Send`,
  `App::on_fd(self, WatchId, impl Fn() -> Vec<Msg> + 'static) -> Self` — as M5-D2.
  The `App` struct gains exactly M5-D4's fields plus `probe_report` (P0-D4).
- `Cmd::Task(Rc<dyn Fn()>)` — as M5-D3.
- `App::new(model, impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static, impl Fn(&M) -> View<Msg> + 'static)` — as M5-D4.
- `EventKind::{PointerDown, PointerMotion, PointerUp}` appended in that order,
  `ALL` at 21; `Handler::PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>)`;
  `Handlers::fire_pair_button(&self, EventKind, f64, f64, u32) -> Option<Msg>`;
  the six builders with the contract's exact names and argument types — as M5-D5.
- `Prop::Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut PaintCx<'_>)>)`,
  `drawing_area(impl Fn(&mut Canvas<'_>, Rect, &mut PaintCx<'_>) + 'static)`,
  `DrawingAreaC { draw, content, last_size }` — as M5-D6.
- `Keymap::base_keysym(&self, u32) -> xkb::Keysym`; `KeyEvent.base: xkb::Keysym`
  placed after `keysym` — as M5-D7.
- `ColorDialogButtonC::measure -> Some((48.0, 32.0))`; `ColorDialogC::measure`
  from the palette grid; `ColorDialogC::paint`; `CheckButtonC::paint` true in
  every state; `ScrollbarC::paint` with `measure` unchanged — as M5-D8. The two
  extra `ColorDialogC` helpers (`grid`, `intrinsic`) are private-in-spirit
  additions the contract explicitly permits ("a part may add private items
  freely"); they are `pub` only so the drift test can read them.
- `ProbePoint { label, x, y }`, `Window::probe_points() -> Vec<ProbePoint>`,
  `Window::allocation(&self, &str) -> Option<Allocation>` — as M5-D9, plus the
  two shared free functions and `App::with_probe_report` (P0-D4).
- `ui/Cargo.toml`: `rustix` features `["fs", "event", "pipe"]`,
  `crossbeam-channel.workspace = true`, neither optional — as M5-D10.

No signature in this plan contradicts the contract without a numbered deviation
at the top of this file, and each of those six deviations is recorded in the M5
contract §6 by the task that lands it.
