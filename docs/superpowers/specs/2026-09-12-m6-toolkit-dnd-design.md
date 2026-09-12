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

## 6. What is NOT in scope

- **Touch drags.** Pointer only. (M4.2 covers touch crate-side; a toolkit
  touch-drag arm is a follow-up, same shape as §5's motion arm.)
- **Drag icons / drag cursors.** No pixmap follows the pointer v1; noted
  as the first visual follow-up (needs paint-layer work, not input work).
- **Popup-grab interaction.** Starting a drag out of an open popup, or
  opening a popup mid-drag, is unspecified v1 (same as GTK's "don't do
  that" — no crash path: the popup grab and the drag slot are independent
  state and each cleans up after itself).
- **Source unmount mid-drag.** If reconcile drops the source node while
  dragging, the slot holds a detached `Node`; release still settles to
  IDLE (target delivery by node path best-effort). Asserted by the cancel
  tests never sticking, not by unmount injection.

## 7. The toolkit/compositor cut (explicit)

**Toolkit-internal (this workstream):** everything in §§2–5 — one surface,
no Wayland round-trip, no seat involvement. The press serial is *recorded*
in the armed slot but never validated in-process.

**Compositor remainder (spec'd here, NOT implemented):** cross-client /
cross-surface drags need, per M4.2's machine:

1. On `DragStart`, offer the payload to the seat: validate the recorded
   press serial against the seat's grant table (M4.2 decision 3 — invalid
   serial ⇒ refuse, no drag), then `wl_data_device.start_drag` on the
   client's data device.
2. Forward motion/drop across surfaces as `enter`/`motion`/`drop` on the
   destination client's data device; move the bytes source→dest on `receive`.
3. Render the drag icon (`wlr_scene_drag_icon`, M4.2) — the compositor-side
   answer to §6's missing drag image.

The seam: `dnd::DragSeat` — `offer_drag(&SeatDragRequest{mimes, serial})
-> SeatDragResult` — with `StubSeat` (scripted accept/refuse + call log)
for tests. The runtime holds **no** seat v1; when the compositor wires up,
it injects a real seat at drag start and refuses toolkit-only delivery for
targets outside the surface. Unit tests prove the request shape (serial =
the arming press serial; MIME list = offered list) against the stub.

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
- Deletion-verified: drop the `DragStart` latch-clear ⇒ the post-drag
  release clicks (test 1's click-counter assertion fails).

## 9. Gates

`cargo test -p icedtea-ui`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --all --check`. Compositor untouched (zero changes expected);
harness untouched (offscreen `ScriptStep` events suffice — no synthetic
seat events needed). Known pre-existing: 3 `icedtea-ui` `gallery_gate`
render failures, not ours, not chased.
