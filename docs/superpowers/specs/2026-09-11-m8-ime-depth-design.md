# M8 IME/text-input depth — design

Date: 2026-09-11. Program: icedtea M6.1 follow-on (M8 opens hardest-first).
Governs both repos: `wlroots-sys` (wlr crate, release 0.20.32) and `icedtea`
(compositor + harness + shell). Prior art: A6.1 (core relay, wlr 0.20.30),
A6.2 (popups + keyboard grab, wlr 0.20.31); SDD ledger holds the full
rulings history (FIX-1/2/3, R1, A12-SEMVER, M1).

## 1. Background and motivation

A6.1/A6.2 proved the relay end-to-end but left deliberate depth gaps:

- Six symbols stayed waived, re-pointed to M8 at the A13 freeze:
  `wlr_input_popup_surface_v2_try_from_wlr_surface`,
  `wlr_input_method_keyboard_grab_v2_destroy`,
  `wlr_input_method_v2_{preedit_string,delete_surrounding_text,state}`,
  `wlr_text_input_v3_state`. (Despite the A13 changelog prose, these are
  event/state *types*, not send functions — verified against bindgen output.)
- Popup placement is once-on-creation with an assumed zero size (B7
  deviations B7-D2/D3): no reposition event and no size accessor exist.
- The relay's outgoing send order (activate→surrounding→content→cause→done,
  enter→preedit→commit→delete→done) is convention + review, not mechanism:
  the unbalanced-activate class (M1's bug family) can recur by editing.

M8 opens with all IME depth because the A6 context (recovery conventions,
emit ordering, borrow discipline) is fresh. Tablet/keyboard M8 remainder is
explicitly out of this slice.

## 2. Reframing: what gets typed

The coverage roadmap's §3 rule ("encode C-side sequencing in the type
system") was written for state machines the crate drives. The relay does
not drive the client sequence — commits arrive via signal and are
forwarded. So this slice types **our side only**:

1. **Aggregate reads** as pending/committed snapshot pairs (distinct types,
   so staged data can never be mistaken for committed).
2. **Outgoing send order** as token-consuming builders (reordering or
   dropping `done` fails to compile).
3. **Live-popup tracking** as one id-only event + one size accessor.

Typing the client's incoming sequence is out of scope (we don't own it).

## 3. Snapshot types (wlr)

Constructed only by the crate from live pointers (`pub(crate)`
constructors); fields `pub` for read. Owned strings, `Option` for nulls,
`Vec` for C arrays — no raw pointers escape.

| Type | Mirrors | Contents |
|---|---|---|
| `PendingImeState` | `wlr_input_method_v2.pending` | staged preedit `(String, cursor_begin, cursor_end)`, `commit_text: Option<String>`, delete `(before, after)` |
| `CommittedImeState` | `wlr_input_method_v2.current` | same shape, committed generation |
| `PendingTextInputState` | `wlr_text_input_v3.pending` | staged surrounding/content-type |
| `CommittedTextInputState` | `wlr_text_input_v3.current` | committed generation incl. `cursor_rectangle` |

Readers: `Runtime::pending_ime_state() / committed_ime_state() -> Option<…>`
(`None` when no IME bound), same pair text-input-side keyed on the
activation-driving focus (`None` when none). The A5 re-forward path takes
`CommittedImeState`/`CommittedTextInputState` — passing staged data is a
type error.

Remaining deferred symbols:

- `Runtime::try_input_popup_surface(surface: *mut wlr_surface) -> Option<InputPopupSurfaceId>`
  (miss, never null — matches the by-id accessor convention).
- `Runtime::destroy_keyboard_grab() -> bool` (was-held-and-destroyed;
  explicit-teardown shape of `destroy_node`).

## 4. Token types (wlr)

Capability handles: `Copy`, carrying `Runtime` + ids. Compile-time *order*,
runtime *liveness* — every method resolves through the tables and no-ops on
stale entries, preserving all FIX-3/A10 guards.

- `ImeActivation` — produced **only** by the text-input enable path.
  Only its methods emit `send_surrounding_text` / `send_content_type` /
  `send_text_change_cause`; only `finish(self)` emits `send_done`.
- `EnteredTextInput` — produced **only** by the enter path (focus relay +
  enter-on-late-bind). Only its methods emit `send_preedit_string` /
  `send_commit_string` / `send_delete_surrounding_text`; only `finish(self)`
  emits `send_done`.
- Teardown (disable/destroy/leave) takes the token by value
  (`deactivate(tok)`, `leave(tok)`): dropping an activation without `done`
  fails to compile. Stale races no-op exactly as today.
- `CommitSerial(u32)` newtype threads each IME commit into the paired
  text-input `done`; pairing asserted in harness e2e (see §7).

The A4–A7 handlers are rewritten to thread tokens. Wire behavior is
byte-identical — the 61 existing relay tests must pass **unchanged** and are
the regression gate for the rewrite. No raw `send_*` remains reachable
outside token methods (audited by grep in review).

## 5. Reposition event + size accessor (wlr)

One addition in the A12 shape (defaulted no-op on `SeatHandler`, id-only
payload keeping `Event: Copy`):

- `Event::InputMethodPopupRepositioned(InputPopupSurfaceId)`, emitted from
  the text-input commit site when ≥1 popup is tracked for the active IME:
  "the caret moved under a live popup — re-read the anchor".
- `Runtime::input_popup_size(popup) -> Option<(i32, i32)>` — attached
  buffer size, `None` before attach. Placement stops assuming zero.

## 6. Compositor consumer (icedtea)

- **Reposition arm** (`State`, next to `new_popup_surface`): re-read caret
  (translated through the focused content origin per the A6.2 review fix)
  + size, `set_node_position`, re-echo the **surface-local** rect (the A6.2
  review lesson: echo ≠ placement).
- **Preedit overlay**: scene text node near the caret showing the committed
  preedit string + cursor span, reusing `text.rs`/cosmic-text shaping from
  SSD titles. Shown/hidden/updated off IME commit events; positioned off
  the translated caret; hidden on commit-string/deactivate. No new protocol,
  no new surface type.
- **Indicator**: active IME name + compose state into the compositor
  snapshot for the shell panel. Snapshot-contract impact: stays at
  `COMPOSITOR_CONTRACT_VERSION` 2 iff the fields are additive-optional;
  otherwise the version bumps with the shell updated in lockstep (same
  branch, same tests).

## 7. Testing (R1 holds: publish only after downstream proof)

Harness doubles record preedit/commit/delete payloads + serials on both
sides. New e2e (all both-sides, real clients):

1. Reposition moves the node on caret commit (position oracle before/after).
2. Overlay shows preedit, clears on commit-string.
3. Indicator tracks activate/deactivate.
4. Serial pairing: IME commit serial → text-input `done` serial.
5. Token order is compile-time: relay tests pass *through* the token APIs.

## 8. Rollout

- SDD task split — wlr: snapshots → tokens → event+size → release 0.20.32;
  icedtea: wiring → overlay → indicator → e2e. Whole-branch review before
  merge (A6.2 precedent). **Single version 0.20.32**: tokens+snapshots are
  one coherent API; splitting would freeze half a type system.
- Semver: additive only (defaulted methods, new types, no supertrait
  changes) per A12-SEMVER; patch release under the frozen-minor rule.
- Out of scope: client incoming-sequence typing, built-in IME origination,
  tablet/keyboard M8 remainder, M5–M7 backlog order (unchanged).

## 9. Open questions (resolved during brainstorming)

- Scope: full M8 opens hardest-first with **all** IME depth (not the
  6-symbol minimum). → §1.
- Consumer: full — wiring + overlay + indicator, not harness-only. → §6.
- UI shape: preedit overlay + indicator, both. → §6.
- Approach: typed state machines (C) over accessors+polling (A);
  eventful depth included; client-side typing excluded. → §2–§5.
