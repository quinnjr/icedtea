# M8 remainder (batch) — design

Date: 2026-09-11. Program: icedtea M8 remainder, batch approach A.
Governs both repos: `wlroots-sys` (wlr crate, release 0.20.33) and `icedtea`
(compositor + harness + shell). Prior art: M8 IME depth (wlr 0.20.32,
snapshots/tokens/reposition + overlay/indicator). This slice wraps every
remaining `milestone = "M8"` symbol (~114: keyboard depth, keyboard_group,
keyboard_shortcuts_inhibit, tablet_tool/pad/v2, virtual_keyboard/pointer,
transient_seat_v1) in one release, with full consumer wiring.

## 1. Background and motivation

M8's roadmap entry is "Input long tail II: keyboard, tablet, IME." IME
hardest-first shipped as 0.20.32. The remainder is the rest of the input
stack — the state machines that were never behind a flag and have lived
as raw sys calls or not at all.

Batch A carries all 114 in one release and one icedtea branch. Rationale:
single version for a coherent input API; one whole-branch review; one
R1 gate. Cost: review is ~2.5× the IME depth and any consumer-found API
tweak re-freezes the whole batch (no publish until the harness proves
it, per R1). If it grows mid-implementation, upgrade the path formally
per the brainstorming ratchet.

IME/text-input (the 6 symbols already wrapped) is explicitly out. M5-M7,
M9-M13 backlog order is unchanged.

## 2. Reframing: what gets typed

Batch A applies the IME depth precedent — type our side, not the
client's protocol — to the remaining input state machines:

1. **Aggregate reads** as snapshot pairs (pending/committed where wlroots
   double-buffers; single state where it doesn't) — owned fields, null-
   guarded, `to_string_lossy` for C strings (blessed lossy per M8).
2. **Outgoing send order** as token-consuming builders where the crate
   drives the sequence (keyboard group add/remove, inhibitor active).
   Token widening is compile-time ordering; runtime liveness still
   resolves through tables and no-ops on stale, preserving FIX-3 guards.
3. **Live tracking** as id-only or handle-only events keeping
   `Event: Copy` where possible, defaulted hooks on the dedicated handler
   trait (additive, per f2cc8a9/A12-SEMVER).

Typing the client's incoming Wayland sequence is out of scope.

## 3. Snapshot and handle types (wlr)

Constructed only by the crate from live pointers (`pub(crate)`
constructors); fields `pub` for read. Owned strings, `Option` for nulls,
`Vec` for arrays — no raw pointers escape. Handles are `Copy`-newtypes
over table keys (listener addresses or monotonic counters), same as
`InputPopupSurfaceId`.

| Type | Mirrors | Contents |
|---|---|---|
| `KeyboardState` / `PendingKeyboardState` | `wlr_keyboard.{current,pending}` | modifiers, LEDs, keymap serial, repeat info |
| `KeyboardGroup` | `wlr_keyboard_group` | member set, group handle, destroy registration |
| `ShortcutsInhibitor` | `wlr_keyboard_shortcuts_inhibitor_v1` | seat, active flag, surface, destroy reg |
| `TabletTool` / `TabletPad` | `wlr_tablet_tool_v2`, `wlr_tablet_pad_v2` | tool type, serial, axes |
| `TabletToolState` | `wlr_tablet_v2_tool_state` | proximity, tip, pressure, tilt, distance |
| `VirtualKeyboard` / `VirtualPointer` | `wlr_virtual_keyboard_v1`, `wlr_virtual_pointer_v1` | seat, device handle |
| `TransientSeat` | `wlr_transient_seat_v1` | seat, destroy reg |

Readers: `Runtime::keyboard_state()`, `pending_keyboard_state()`,
`keyboard_group_state()`, `tablet_tool_state()`,
`try_tablet_tool_from_surface()` — each returns `Option` on missing
state, same miss shape as the IME readers.

## 4. Token types (wlr)

Capability handles, `Clone`/`Copy` where `Runtime` allows, carrying
`Runtime` + keys. Compile-time order, runtime liveness.

- `KeyboardGroupChange` — produced only by group mutation paths; only its
  methods emit `send_keymap` propagation and group `done`; `finish(self)`
  is the only path to `keyboard_group_done`.
- `ToolInProximity` — produced on `proximity_in`; only its methods emit
  `motion`/`tip`/`pressure`; consumed on `proximity_out`.
- `InhibitorActivation` — produced on inhibitor `activate`; only
  `deactivate(self)` clears it.

Rewrites thread tokens through the existing handlers (keyboard-group,
tablet, shortcuts). Wire behavior is byte-identical — the existing
input tests must pass unchanged.

## 5. Events (wlr)

Additions in the A12 shape (defaulted no-ops, semver-additive):

- `Event::KeyboardGroupChanged(KeyboardGroupId)`
- `Event::ShortcutsInhibitorToggled(InhibitorId, bool)`
- `Event::TabletToolEvent(ToolId)` + `TabletPadEvent(PadId)`
- `Event::VirtualDeviceCreated(DeviceId)` (unit/handle variants)

Table lifetime uses `Registration` listeners; destroys emit via
listener-address or pending-drain, preserving FIX-3.

## 6. Compositor consumer (icedtea)

- **Keyboard indicator upgrade** — M8-7's `ime_active` stays; add
  `keyboard_layout: Option<String>` + `shortcuts_inhibited: bool` to
  `Snapshot` (additive-optional, `serde(default)`, bump contract v3→4
  with the same legacy-decode discipline; shell updated in lockstep).
- **Shortcuts inhibit wiring** — while `inhibitor.active`, the
  `SeatHandler::key` dispatcher skips compositor bindings (existing
  "dispatcher before grab" ordering, now with the inhibit gate).
- **Tablet plumbing** — cursor attach via `wlr_cursor_attach_input_device`,
  tool motion reusing `output_for_window`; pad strip/ring deltas as
  scroll-like shell events. No new surface type, no pad UI.
- **Virtual/transient** — no UI; harness gains virtual input doubles so
  e2e can drive keys/pointer synthetically; transient seats give per-test
  seat isolation.

## 7. Testing (R1 holds: publish only after downstream proof)

Harness doubles record each payload with serials (keyboard done serials,
tablet axis frames, inhibitor toggles) in the record-everything style.
New e2e (both sides, real clients):

1. Layout name reaches snapshot on group switch.
2. Inhibitor blocks a compositor binding while active, releases after.
3. Tablet tool tip + pressure reaches the expected surface.
4. Virtual keyboard types into the focused text-input.

Token order is compile-time; proof is that the relay tests pass through
the token APIs plus the grep audit that no raw `send_*` remains outside
token impls.

## 8. Rollout

- SDD task split — wlr: keyboard depth → tablet → virtual/transient →
  release 0.20.33; icedtea: wiring → overlay → indicator → e2e. Whole-
  branch review before merge (A6.2/M8 precedent). Single version 0.20.33
  (batch).
- Semver: additive only (defaulted methods, new types, no supertrait
  changes) per A12-SEMVER; patch release under the frozen-minor rule.
- Out of scope: tablet pad on-screen UI beyond input plumbing, client
  incoming-sequence typing, M5-M7/M9-M13 remainder.

## 9. Open questions (resolved during brainstorming)

- Batch vs slices: batch A chosen (single version, one R1 gate) despite
  larger review surface. Slices (B/C) documented as alternatives.
- Consumer: full — not wrapping-only. Layout name + inhibit badge are the
  visible proof, not just harness doubles.
