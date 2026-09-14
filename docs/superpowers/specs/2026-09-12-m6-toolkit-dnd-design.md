# M6 toolkit drag-and-drop — design

Date: 2026-09-12. Workstream: `feature/m6-toolkit-dnd` (base `develop@0be7ec1`).
Scope: the **toolkit side** of drag-and-drop in the pure-Rust `icedtea-ui`
crate (`ui/`). Verified starting point: DnD exists only as comment mentions
(seat grab types, Xwayland bridge notes in `compositor/src/state.rs`); the
wlr-side M4.2 port is done but nothing toolkit-side exists.

Prior art (semantics only, not code):
`docs/superpowers/specs/2026-08-14-wlr-port-m4.2-drag-and-drop-design.md`
owns the crate/compositor protocol machine
(offer → enter → motion → drop, grab-serial validation, drag-icon scene
node). This spec mirrors its *rules* inside one client surface and states
exactly where the toolkit stops and compositor seat work begins (§7).

## 1. Goal

Press-drag-drop **within one toolkit surface** moves data from a drag source
widget to a drop target widget, with GTK-parity gesture semantics:

- press (`BTN_LEFT`) arms a potential drag; motion past a threshold starts it;
  release over an accepting target delivers; release elsewhere or `Escape`
  cancels with no stuck state.
- MIME-typed payloads with source/target negotiation (§3).
- Target highlight while hovered mid-drag; source click suppression once a
  drag began (a drag is never also a click).

Proven by, not framework-only: one real source→target pair (draggable chip →
drop target) driven through the real `App` dispatch, asserting payload,
highlight states, and both cancel paths (§8).

## 2. Session lifecycle

```
press BTN_LEFT on source ──▶ ARMED (origin, serial, payload stashed)
        │ motion, dist ≤ 8px: stay ARMED (normal hover/click path untouched)
        │ motion, dist > 8px: ──▶ DRAGGING (DragStart to source; click disarmed)
        │ release while ARMED: ──▶ IDLE (normal click proceeds — zero behaviour change)
        ▼ DRAGGING
motion ──▶ hit-test IGNORING the implicit grab ──▶ target enter/leave/motion
release over accepting target ──▶ DROP ──▶ IDLE
release over nothing/non-acceptor ──▶ CANCEL ──▶ IDLE
Escape while DRAGGING ──▶ CANCEL ──▶ IDLE
```

Rules:

- **Threshold:** Euclidean distance `> 8.0px` from the press point
  (`dnd::DRAG_THRESHOLD_PX`; GTK's `gtk_drag_check_threshold` parity).
- **One drag at a time.** A second `BTN_LEFT` press while armed/dragging is
  ignored for DnD (no re-arm); non-left buttons never arm.
- **Grab coexistence.** The existing `ImplicitGrab` is untouched: press still
  records the grab target, motion/release still reach the source controller.
  DnD delivery is *additional* routing in `route_input`, never a
  restructuring of input dispatch. During a drag the source keeps receiving
  its normal motion/up events (documented, §5 deviation D1).
- **Click suppression.** On `DragStart` the source controller clears its
  press latch, so the completing release cannot fire `Click`/`Activate`.
  Release-while-armed (no drag) clicks exactly as today.
- **Cleanup invariant.** Every exit (drop, cancel, `Escape`) clears the
  runtime drag slot *and* notifies the source (`DragEnd{dropped}`) and the
  target (`Drop` or `DragLeave`). There is no path that leaves `DRAGGING`
  with no buttons held: `BTN_LEFT` release always settles.

## 3. MIME types and negotiation

`dnd::DragPayload` is an ordered list of `(mime, bytes)` pairs.

- v1 offers: `text/plain` (UTF-8), `text/uri-list` (UTF-8, CR/LF-separated).
  Constants `dnd::TEXT_PLAIN`, `dnd::TEXT_URI_LIST`.
- `DragPayload::data_for(accepted: &[&str]) -> Option<(&str, &[u8])>`
  returns the **first offered pair whose MIME the target accepts** —
  source preference order wins, exactly like `wl_data_device` offer
  negotiation narrowed to one surface.
- The `Drop` event carries the negotiated `(mime, bytes)`; the `Text`
  handler binding decodes lossy UTF-8 (fine for both v1 types).
- Reserved, not implemented: `application/x-icedtea-widget` (row
  reorder payloads) and any binary flavor — the negotiation function
  already carries them; only decoding/display is v1-scoped.

## 4. Source/target widget contract

### 4.1 Props (additive, `PropName::all_m6()` — `ALL` stays the contract's 97)

| Prop | Value | Meaning |
|---|---|---|
| `DragSource` | `Prop::Str(text)` | Node offers `text` as `text/plain`; presence = drag source. |
| `DropAccept` | `Prop::Bool(true)` | Accepts `text/plain`. |
| `DropAccept` | `Prop::Str("text/plain, text/uri-list")` | Accepts the listed MIME set (comma-separated). |

Builders: `View::drag_source(text)`, `View::drop_accept(mimes)`,
`on_drag_start`, `on_drag_enter`, `on_drag_motion`, `on_drag_leave`,
`on_drop(f: Fn(&str) -> Msg)`, `on_drag_end(f: Fn(bool) -> Msg)`.

### 4.2 Controller events (additive, defaulted — existing widgets unaffected)

`Controller` gains two defaulted methods: `drag_offer() -> Option<DragPayload>`
(`None`) and `drop_accepts(mimes) -> bool` (`false`). `GenericC` implements
both from the §4.1 props, so **any** kind without a dedicated controller can
be a source, a target, or both.

`controller::Event` gains `DragStart`, `DragEnter`, `DragMotion{local}`,
`DragLeave`, `Drop{mime, data}`, `DragEnd{dropped}`. `EventKind` gains the
matching six bindings (`DragStart…DragEnd`, appended after `PointerUp` so
existing indices never move; `ALL` 21 → 27).

The behavior behind `drag_offer`/`drop_accepts` and those six arms is
`view::controller::DndState`; `GenericC` holds one, and a dedicated
controller opts in by holding one too (see §6). `Controller::cancel_press`,
called by `deliver` on `DragStart`, is the one per-controller hook: it clears
that controller's own press latch so the completing release is a drop/cancel.

### 4.3 Highlight

While a drag hovers an accepting target, `GenericC` adds the
`drop-target-active` class (`dnd::DROP_ACTIVE_CLASS`) and removes it on
`DragLeave`/`Drop`/cancel — the same next-frame-restyle path `:hover` uses.
Theme styling of the class is theme business (Adwaita has none); the
toolkit guarantees the class, tests assert it.

## 5. Runtime routing (`view/app.rs`, additive arms in `route_input`)

- Press: after the existing grab/PointerDown work, query the pressed
  instance's `drag_offer()`; `Some` arms the drag slot (source node, payload,
  origin, serial). `None` → exactly today's behavior.
- Motion: advance the session. On threshold-cross deliver `DragStart` to the
  source. While dragging, hit-test with `hit_chain` **bypassing `aim`'s
  grab override**, take the innermost instance whose controller
  `drop_accepts(offered)`; on change deliver `DragLeave`/`DragEnter`, and
  `DragMotion` to the current target each motion.
- Release `BTN_LEFT`: if armed-only, clear the slot (click proceeds). If
  dragging, deliver `Drop` (negotiated payload) to the target if any, then
  `DragEnd{dropped}` to the source, and clear.
- `Escape` keypress while dragging cancels (`DragLeave` + `DragEnd{false}`,
  clear) before normal key routing.

Deviation D1 (documented, not hidden): during a drag the source *also*
keeps its normal grabbed motion/up events. Rationale: the grab owns button
semantics toolkit-wide (popup grabs, press tracking); DnD must not fork
it. No existing controller misbehaves on extra motion (all motion arms are
idempotent state writes), and click-after-drag is closed by §2's latch
clear, covered by a deletion-verified test.

## 6. Follow-ups and exclusions

- **Dedicated widget controllers — adopted in part (shipped).** v1
  sources/targets were `GenericC` nodes only (deviation M6-D2: e.g.
  `ListBoxRow`; `Button`/`Box`/`Label` have their own controllers whose trait
  defaults kept them out). The follow-up ships: the three fields and the six
  `Event` arms live in a shared `DndState` that a dedicated controller holds,
  and `Controller::cancel_press` clears that controller's own press latch on
  `DragStart` (so a drag is never also a click). Adopted: `ButtonC`, `EntryC`,
  `LabelC`, `ListBoxC`. The rest of the kind table is the same two-line
  delegation (`self.dnd.set_prop`, `self.dnd.on_event`) plus a `cancel_press`
  override where the controller latches a press; it is left to the widget
  owners as the remaining mechanical work.
- **Touch drags.** Pointer only. (M4.2 covers touch crate-side; a toolkit
  touch-drag arm is a follow-up, same shape as §5's motion arm.)
- **Drag icons / drag cursors.** No pixmap follows the pointer: `start_drag`
  passes no icon surface, so M4.2's `wlr_scene_drag_icon` has nothing to
  render. Noted as the first visual follow-up (needs paint-layer work, not
  input work).
- **Popup-grab interaction.** Starting a drag out of an open popup, or
  opening a popup mid-drag, is unspecified v1 (same as GTK's "don't do
  that" — no crash path: the popup grab and the drag slot are independent
  state and each cleans up after itself).
- **Source unmount mid-drag.** If reconcile drops the source node while
  dragging, the slot holds a detached `Node`; release still settles to
  IDLE (target delivery by node path best-effort). Asserted by the cancel
  tests never sticking, not by unmount injection.

## 7. The toolkit/compositor cut (explicit)

**Toolkit-internal:** everything in §§2–5 — one surface, no Wayland
round-trip, no seat involvement — still runs when there is no seat. The
press serial is *recorded* in the armed slot and handed to the seat at the
threshold crossing.

**Compositor seam (now implemented, source half):** cross-surface drags use
M4.2's machine:

1. At the threshold crossing the runtime offers the drag to
   `dnd::DragSeat` — `offer_drag(&SeatDragRequest{serial, payload})` (the
   MIME list is derived from the payload via `SeatDragRequest::mimes()`) —
   with the recorded arming press serial, the offered MIME list, and the
   bytes. The offer is issued whenever a seat is present. `Accepted` means
   only that `wl_data_device.start_drag` was *called* (the compositor may
   still reject it); it does **not** suppress the toolkit's own in-surface
   hit-testing, because §7's destination half is not wired and disabling the
   in-surface path would leave every drop dead.
2. `ui/src/window/selection.rs`'s `ClientSeat` is the real implementation:
   it creates a `wl_data_source`, offers the MIMEs, stashes the payload for
   `wl_data_source.send`, and calls `wl_data_device.start_drag` on the
   window's surface. wlroots validates the serial against M4.2 decision 3's
   grant table and emits the seat events; the compositor routes
   `enter`/`motion`/`drop` across surfaces. The toolkit source's `send`
   serves the bytes (the drag half of the existing `WlDataSource` handler).
3. `App::run` builds this seat off the window's data device
   (`Window::take_drag_seat`); `App::with_drag_seat` lets a test inject a
   scripted one. `StubSeat` still tests the toolkit half.

**Compositor/toolkit remainder (NOT implemented):**

- **Destination-in routing.** A drag delivered *into* this client from
  elsewhere is not routed to toolkit targets yet: `wl_data_device.enter`/
  `motion`/`drop` on the destination side are ignored by `WindowState`. The
  source half works, and a same-surface drop keeps using §§2–5: the toolkit
  always runs its own in-surface hit-testing, and offloading the source to
  the seat does not turn that off (the compositor's destination half would
  otherwise be the only delivery path, and it does not exist yet).
  Cross-surface drops into this client remain the gap.
- **Drag icon.** `start_drag` passes `None` for the icon surface, so M4.2's
  `wlr_scene_drag_icon` has nothing to render. A toolkit drag image is the
  paint-layer follow-up §6 names.
- **Compositor-reported outcome.** When a drag is offloaded, the toolkit's
  own `DragEnd{dropped}` still reports its in-surface result (now the real
  outcome of a same-surface drop, since in-surface routing continues); for a
  cross-surface drop the authoritative drop is the compositor's
  (`wl_data_source` `dnd_drop_performed`/`dnd_finished`), which `ClientSeat`
  uses only to retire the source. Folding that back into a model message is
  the follow-up.

The seam: `dnd::DragSeat` — `offer_drag(&SeatDragRequest{serial,
payload}) -> SeatDragResult` (MIMEs via `SeatDragRequest::mimes()`) — with
`StubSeat` (scripted accept/refuse + call log) for the toolkit half and
`ClientSeat` for a live window. Unit tests prove the request shape (serial =
the arming press serial; MIME list = the offered list; bytes = the payload)
against a recording stub; an accepting stub no longer changes the toolkit
half, because its result does not gate in-surface routing.

## 8. Tests

- `ui/src/dnd.rs` unit tests: threshold boundary (at/below/above 8px),
  negotiation order (source preference, no intersection ⇒ `None`),
  session transitions incl. double-press-ignored, `StubSeat` accept/refuse
  + request shape.
- `ui/tests/dnd.rs` (NEW, offscreen `App` + `ScriptStep` pointer events,
  real geometry — no hard-coded coordinates beyond reading allocations
  from the layout the app itself builds):
  1. press → move past threshold → move over target → release delivers the
     payload text to the model AND the target carried `drop-target-active`
     exactly while hovered (enter sets, leave/drop clears).
  2. release outside any target: no `Drop`, source got `DragEnd(false)`,
     no stuck state (a subsequent plain click still clicks).
  3. `Escape` mid-drag: same cancelled shape as (2).
  4. sub-threshold press-release: plain click, never a drag (no
     `DragStart`, click fires).
  5. a dedicated `Button` source dropping on a dedicated `Entry` target
     (per-kind adoption), and a drag released back over its source proving
     `cancel_press` keeps it from also clicking.
  6. a recording `DragSeat`: a `Refused` seat leaves the drag internal and
     sees the request shape (serial = the arming press serial, mimes, bytes);
     an `Accepted` seat issues the offer but leaves the toolkit's in-surface
     routing unchanged, so `DragEnter`/`DragMotion`/`Drop` still reach the
     target (`Accepted` means "start_drag was called", not "the compositor
     took over delivery").
- Deletion-verified: drop the `DragStart` latch-clear ⇒ the post-drag
  release clicks (test 1's click-counter assertion fails).

## 9. Gates

`cargo test -p icedtea-ui -p icedtea-compositor`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo fmt --all --check`. Compositor untouched (the seat bridge is entirely
client-side); harness untouched (offscreen `ScriptStep` events plus the
recording `DragSeat` suffice — no synthetic seat events needed).
