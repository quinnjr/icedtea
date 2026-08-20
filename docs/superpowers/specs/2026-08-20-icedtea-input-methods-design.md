# icedtea-wm — A6: Input Methods (text-input-v3 + input-method-v2)

Date: 2026-08-20

## Overview

A6 (roadmap `docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`, Track A)
adds the two Wayland protocols that make CJK/IME text composition and
on-screen-keyboard (OSK) integration work:

- **`zwp_text_input_manager_v3`** — bound by ordinary applications (a text
  field in a GTK4/Qt6 app). A `zwp_text_input_v3` object tells the compositor
  "I am an editable field with focus," reports surrounding text, cursor
  position, content type (hint/purpose: password, digits, email, terminal,
  …), and the on-screen cursor rectangle; it receives `preedit_string`,
  `commit_string`, and `delete_surrounding_text` events back.
- **`zwp_input_method_manager_v2`** — bound by exactly one privileged client
  per seat: the IME (fcitx5, ibus, or an on-screen keyboard like squeekboard).
  A `zwp_input_method_v2` object is told when it becomes `activate`d/
  `deactivate`d, receives the same surrounding-text/content-type/cursor-rect
  context, and sends back `preedit_string`/`commit_string`/
  `delete_surrounding_text`, optionally opens **popup surfaces** (candidate
  windows) and optionally requests a **keyboard grab** (raw physical key
  pass-through, e.g. for dead-key composition).

**The compositor is the relay between the two.** Unlike every other protocol
already in the tree, wlroots 0.20 does **not** ship a built-in relay helper —
`/usr/include/wlroots-0.20/wlr/types/wlr_text_input_v3.h` and
`wlr_input_method_v2.h` define only the two object types and their
`_send_*` functions; there is no `wlr_input_method_relay` (older wlroots had
one and removed it — sway and river each carry their own relay code today).
So this is the first protocol batch since M4 where the `wlr` crate has to
implement genuine cross-object routing logic, not just create-a-global-and-
forward-signals.

## Decisions of record

1. **Scope: text-input-v3 core relay + input-method-v2 core relay + popup
   surfaces + keyboard-grab forwarding, in one additive `wlr` release
   (0.20.23).** tablet-v2 is explicitly **not** in this batch — the roadmap
   lists it as "later" (roadmap line 99-100) because it is unrelated
   protocol-wise (graphics-tablet stylus input, no text relay) and is a
   smaller, self-contained follow-on. See "Out of scope."
2. **The relay lives in the `wlr` crate, not the compositor**, continuing the
   M4.5 precedent (pointer-constraints decision #2): the crate already owns
   the seat, keyboard focus, and the `on_key`/`on_modifiers` chokepoint
   (`backend.rs:5332-5420`) and the keyboard-focus chokepoints
   (`Runtime::focus_toplevel_keyboard`/`clear_keyboard_focus`,
   `runtime.rs:6739-6797`) that text-input enter/leave must hook. Building the
   relay outside those chokepoints would mean duplicating focus-tracking the
   crate already has. The compositor's job is exactly what it is for every
   other protocol in `lib.rs`: create the two globals at boot, non-fatal.
3. **At most one bound input-method per seat.** If a second client binds
   `zwp_input_method_manager_v2` and creates a `zwp_input_method_v2` while one
   is already live for the seat, the crate immediately calls
   `wlr_input_method_v2_send_unavailable` on the newcomer and does not track
   it — the documented pattern for "another IME/OSK is already running"
   (mirrors sway's `input_method_relay.c`). This is a real scenario in
   practice: a user may have both a desktop IME (fcitx5) and an OSK
   (squeekboard) auto-started; the second loses.
4. **Text-input focus follows keyboard focus, gated on the surface's client
   having a live text-input.** A `zwp_text_input_v3` only gets `enter`/`leave`
   when its owning surface gains/loses **keyboard** focus (not pointer focus —
   text-input-v3 explicitly does not participate in pointer-focus, per the
   protocol XML). This reuses the existing keyboard-focus chokepoints from
   decision #2, the same way M4.5 hung pointer-constraint activation off the
   existing pointer-focus chokepoint.
5. **Enable/commit/disable relay is state-diffed, not event-passthrough.**
   `wlr_text_input_v3` and `wlr_input_method_v2` each carry `pending`/
   `current` state structs the library diffs internally (`current_serial`,
   `active_features`); the crate's relay reacts to the text-input's `enable`/
   `commit`/`disable` **signals** (which wlroots emits after its own
   pending→current diff), reads the resulting `current` state, and forwards
   it to the input-method with the IME-side `_send_surrounding_text`/
   `_send_content_type`/`_send_text_change_cause`/`_send_done` calls — never
   reads text-input `pending` directly, matching how M1-M4 always drive off
   wlroots' own post-diff state rather than re-deriving it.
6. **Keyboard-grab forwarding is a branch in the existing `on_key`/
   `on_modifiers` chokepoint, keyed on "does the seat's tracked input-method
   have a live keyboard grab."** When it does, physical key/modifier events go
   to `wlr_input_method_keyboard_grab_v2_send_key`/`send_modifiers` **instead
   of** `wlr_seat_keyboard_notify_key`/`notify_modifiers` (the grab intercepts,
   it does not duplicate) — the IME decides whether/what to forward to the
   focused text-input via `commit_string`. When no grab is active, physical
   keys flow exactly as today (unrelated to text-input entirely — plain
   latin-script typing is not intercepted, only composition-sensitive input
   is, and only while an IME chooses to grab).
7. **Popup-surface placement is new compositor-owned scene work — the one
   exception to decision #2.** `wlr_input_popup_surface_v2` is not an
   `xdg_popup` (no `xdg_positioner`, so none of the existing
   `wlr_scene_xdg_surface_create` auto-placement in `backend.rs:3886` applies)
   and has no analogue anywhere else in the tree. The crate exposes the raw
   object + its `new_popup_surface`/`destroy` signals and the
   `wlr_input_popup_surface_v2_send_text_input_rectangle` call; the
   **compositor** places it in the scene graph (anchored below the focused
   text-input's `cursor_rectangle`, clamped to the output) the same way
   `compositor/src/render.rs` already places other compositor-managed scene
   nodes (the background rect, decorations) rather than relying on a
   protocol-driven auto-placement helper.
8. **Split-phase publish → consume**, per M1-M4.5: one additive `wlr` 0.20.x
   release (**0.20.23**), API-review verdict held before `cargo publish`, then
   the compositor bumps and consumes. `[patch.crates-io]` during development,
   published pin at the end.
9. **Execution: SDD**, per-task review on Opus, final whole-branch review,
   merge window presented — never auto-merged. wlroots-sys integrates via a
   PR into `develop`; coordinate with any parallel wlroots-sys session via
   SendMessage before pushing to `develop` (per the M4.5 precedent — the repo
   is shared).

## Architecture

### Protocol objects (from `/usr/include/wlroots-0.20/wlr/types/`)

`wlr_text_input_v3` (per `wlr_text_input_manager_v3`, one per client bind):
`seat`, `focused_surface`, `pending`/`current` state (`surrounding.{text,
cursor,anchor}`, `text_change_cause`, `content_type.{hint,purpose}`,
`cursor_rectangle`, `features` bitfield), `pending_enabled`/`current_enabled`.
Signals: `enable`, `commit`, `disable`, `destroy`. Compositor-side sends:
`wlr_text_input_v3_send_{enter,leave,preedit_string,commit_string,
delete_surrounding_text,done}`.

`wlr_input_method_v2` (per `wlr_input_method_manager_v2`, at most one tracked
per seat per decision #3): `seat`, `seat_client`, `pending`/`current` state
(`preedit`, `commit_text`, `delete{before_length,after_length}`), `active`/
`client_active`, `popup_surfaces` list, `keyboard_grab`. Signals: `commit`,
`new_popup_surface`, `grab_keyboard`, `destroy`. Compositor-side sends:
`wlr_input_method_v2_send_{activate,deactivate,surrounding_text,
content_type,text_change_cause,done,unavailable}`.

`wlr_input_popup_surface_v2`: `resource`, `input_method`, `surface`. Signal:
`destroy`. Compositor-side send: `_send_text_input_rectangle(popup, box)`.

`wlr_input_method_keyboard_grab_v2`: `resource`, `input_method`, `keyboard`.
Signal: `destroy`. Compositor-side sends: `_send_key`, `_send_modifiers`,
`_set_keyboard`, and `_destroy` to release it.

### `wlr` crate additions (one additive 0.20.23 release)

**`RuntimeInner` fields** (mirroring the pointer-constraints manager fields at
`runtime.rs:200-205`):
- `text_input_manager: RefCell<Option<NonNull<sys::wlr_text_input_manager_v3>>>`
- `input_method_manager: RefCell<Option<NonNull<sys::wlr_input_method_manager_v2>>>`
- `text_inputs: RefCell<HashMap<ListenerAddr, TextInputEntry>>` — every live
  `zwp_text_input_v3`, keyed the M4.4-lesson way (decision #5 of M4.4/M4.5: key
  destroy-tracking on the destroy listener's own address, never on signal
  `data`), holding the raw `NonNull<wlr_text_input_v3>` and its owning client
  (`wl_client` pointer, needed to match against the newly-keyboard-focused
  surface's client — the same `wl_resource_get_client`/surface-to-client
  lookup the seat code already does for keyboard enter).
- `input_method: RefCell<Option<InputMethodEntry>>` — the single tracked
  input-method for the seat (decision #3); `None` when no IME is bound.
  `InputMethodEntry` holds the raw pointer, the currently-focused text-input's
  listener key (so `commit` from the IME knows which text-input to relay to),
  and the current keyboard-grab `NonNull` (`Option`, set when `grab_keyboard`
  fires, cleared on its `destroy`).
- `input_method_popups: RefCell<HashMap<ListenerAddr, PopupEntry>>` — live
  popup surfaces and their scene-tree node, for decision #7.

**`Runtime::create_text_input_manager(&Display) -> Result<()>`** and
**`Runtime::create_input_method_manager(&Display) -> Result<()>`** — mirror
`create_pointer_constraints_manager` (`runtime.rs:4798`): double-create guard,
`sys::wlr_text_input_manager_v3_create`/`wlr_input_method_manager_v2_create`,
store the `NonNull`, register the manager's `new_text_input`/`new_input_method`
listener.

**`on_new_text_input`**: register `enable`/`commit`/`disable`/`destroy`
listeners (destroy keyed by the destroy listener's address per decision #5's
citation of the M4.4 lesson); insert into `text_inputs`. No immediate `enter`
— that is driven by the keyboard-focus chokepoint below, so a text-input
created *after* its surface already has keyboard focus still gets entered
correctly (same "activate on current state, not just on the future edge"
requirement M4.5 called out for pointer constraints created after focus was
already established).

**`on_new_input_method`**: if `input_method.borrow().is_some()`,
`wlr_input_method_v2_send_unavailable` and return (decision #3). Otherwise
register `commit`/`new_popup_surface`/`grab_keyboard`/`destroy` listeners,
store as the seat's `InputMethodEntry`.

**Keyboard-focus chokepoint additions**, in `Runtime::focus_toplevel_keyboard`
(`runtime.rs:6739`) and `Runtime::clear_keyboard_focus` (`runtime.rs:6797`),
right where `wlr_seat_keyboard_notify_enter` already fires:
- On the *outgoing* focused surface: if its client owns a `current_enabled`
  text-input, `wlr_text_input_v3_send_leave`; if an input-method is tracked
  and was activated for it, `wlr_input_method_v2_send_deactivate` +
  `send_done`, and clear the `InputMethodEntry`'s focused-text-input key.
- On the *incoming* focused surface: if its client owns a text-input,
  `wlr_text_input_v3_send_enter`. Activation of the IME itself still waits for
  that text-input's own `enable` signal (decision #5) — entering a surface
  does not by itself mean the field is ready for composition, exactly as
  `zwp_text_input_v3`'s protocol requires (`enter` and `enable` are
  independent — a client can enter, then later enable/disable repeatedly, e.g.
  moving focus between a password field and a normal one within the same
  surface).

**`on_text_input_enable`**: if this text-input's surface currently holds
keyboard focus and a seat input-method is tracked: record it as the
`InputMethodEntry`'s focused text-input, `wlr_input_method_v2_send_activate`,
then forward `current` state (`send_surrounding_text`, `send_content_type`),
`send_done`.

**`on_text_input_commit`**: if this text-input is the `InputMethodEntry`'s
currently-focused one, forward the diffed `current` state the same way
(`send_surrounding_text`/`send_content_type`/`send_text_change_cause` as
changed, `send_done`).

**`on_text_input_disable`**: if this was the focused one,
`wlr_input_method_v2_send_deactivate` + `send_done`, clear the focused-key.

**`on_input_method_commit`**: relay the IME's `current` state to whichever
text-input the `InputMethodEntry` has recorded as focused (a no-op, correctly,
if none — the IME committed while nothing was focused/enabled):
`wlr_text_input_v3_send_preedit_string`, `_send_commit_string`,
`_send_delete_surrounding_text` as populated, then `_send_done`.

**`on_input_method_new_popup_surface`**: store in `input_method_popups`,
register its `destroy` listener; hand the compositor a way to place it (see
compositor additions) — the crate does not position it itself (decision #7),
it only exposes the object and a `Runtime::input_method_popup_surface(&self)
-> Option<*mut wlr_surface>` style accessor plus
`Runtime::send_input_popup_rectangle(popup_id, Rectangle)` wrapping
`wlr_input_popup_surface_v2_send_text_input_rectangle`.

**`on_input_method_grab_keyboard`**: store the grab `NonNull` on the
`InputMethodEntry`; `wlr_input_method_keyboard_grab_v2_set_keyboard` with the
seat's active keyboard (mirrors the "one logical keyboard identity" comment
already in `on_key`, `backend.rs:5341-5344`). Register the grab's `destroy`
listener to clear the field.

**`on_key`/`on_modifiers` branch** (`backend.rs:5332-5420`): before the
existing `wlr_seat_keyboard_notify_key`/`notify_modifiers` calls, check
`InputMethodEntry`'s grab field; if `Some`, call
`wlr_input_method_keyboard_grab_v2_send_key`/`_send_modifiers` on the grab
**instead of** the seat calls (decision #6) and return — the dispatcher's
`Event::Key` emission to `S: Handlers` (keybindings, alt-tab, etc., the
existing `session.dispatcher.emit` call just above the seat-notify call)
still runs unconditionally first, so icedtea's own keybindings (Super+Tab,
etc.) are unaffected by a grab; only the *forwarding* target changes.

### Compositor additions

- `compositor/src/lib.rs` and `harness/src/lib.rs` (both boot paths): two new
  non-fatal `create_*` calls alongside the existing block (`lib.rs:94-113`),
  in the same tone ("without it, IME/OSK clients simply cannot attach"):
  ```
  if let Err(err) = runtime.create_text_input_manager(&display) { ... }
  if let Err(err) = runtime.create_input_method_manager(&display) { ... }
  ```
- `compositor/src/render.rs` (or a small new `compositor/src/input_method.rs`
  alongside `input.rs`, matching the existing one-module-per-concern layout):
  popup-surface placement (decision #7). On
  `Runtime::input_method_popup_created` (a new event the crate surfaces
  through the existing `S: Handlers` dispatch — the same mechanism
  `ToplevelHandler`/`OutputHandler` already use, so this needs one new trait
  method, e.g. `InputMethodHandler::new_popup_surface`/`popup_surface_destroyed`,
  added to `handler.rs` the same additive way `SeatHandler::session_lock_changed`
  was added in M4.4), create a scene node for the popup surface parented above
  the currently-focused toplevel's band, positioned from the focused
  text-input's last-known `cursor_rectangle` (surface-local → output
  coordinates via the same transform `render.rs` already applies for
  decorations), clamped so the popup never renders off-output; call
  `Runtime::send_input_popup_rectangle` once placed so the IME can lay out
  candidates relative to it. Re-position on every subsequent `commit` that
  changes the cursor rectangle.
- `state.rs`: no `DbCommand` routing needed for the relay itself (it is
  entirely crate-internal per decision #2); one read-only `DbCommand` for
  tests only, mirroring M4.5's `CursorPosition` reply command — e.g.
  `DbCommand::InputMethodActive { reply }` exposing whether the tracked
  `InputMethodEntry` is currently activated, so the harness can assert
  activation without a real IME UI.

### Relation to `zwp_virtual_keyboard_manager_v1` (already present)

Virtual-keyboard (`lib.rs:94-98`, already shipped) lets a client inject raw
keysyms as if from a physical keyboard — no text context, no composition
state, just "press this key." It is the *injection* path; text-input/
input-method is the *composition* path. They are complementary, not
redundant, and real IMEs commonly use **both at once**: fcitx5/ibus use
input-method-v2 to know what field has focus and to get surrounding-text
context for candidate selection, and squeekboard (the GNOME/Phosh on-screen
keyboard) uses input-method-v2 to know when to show/hide and to place its
popup, but falls back to injecting via `zwp_virtual_keyboard_manager_v1` for
apps that never bound `zwp_text_input_v3` at all (most Wayland apps outside
GTK4/Qt6 still don't) — otherwise the OSK would be unable to type into them.
The two globals this milestone adds do not change virtual-keyboard's wiring
at all; A6 is additive on top of it.

## Decomposition into milestones

- **A6.1 — text-input-v3 + input-method-v2 core relay.** Both managers, the
  `text_inputs`/`InputMethodEntry` bookkeeping, the keyboard-focus-chokepoint
  enter/leave and enable/disable/commit relay (decisions #2-#5), the
  at-most-one-IME rule (#3). No popups, no keyboard grab yet — an IME can
  compose and commit text into a focused field, which is the CJK use case,
  the highest-value slice. `wlr` 0.20.23.
- **A6.2 — popup surfaces + keyboard-grab forwarding.** `wlr_input_popup_surface_v2`
  placement (crate exposure + compositor scene placement, decision #7) and
  the `on_key`/`on_modifiers` grab branch (decision #6). This is what makes
  candidate windows visible and dead-key/compose-key IMEs work; layered on
  A6.1 rather than folded in because it is the part with genuinely new
  compositor-owned scene code (everything else is crate-internal), so it
  benefits from landing and being reviewed as its own additive `wlr` release
  (0.20.24) once A6.1 is consumed and stable.
- **Follow-on (separate, not in A6): tablet-v2** (`zwp_tablet_manager_v2` +
  `zwp_tablet_seat_v2` + `zwp_tablet_tool_v2`/`zwp_tablet_pad_v2`) — graphics
  tablet (stylus/pen, pressure, tilt, pad buttons). Explicitly called out as
  smaller and unrelated in the roadmap (roadmap line 99-100); it shares no
  code with the text-input/input-method relay (it is a pointer-family
  protocol, closer in shape to M4.5's pointer-constraints than to A6) and
  should be scoped and speced on its own when it comes up.

## Testing (automated-only)

Extend `harness` + `compositor/tests/client_protocol.rs`, following the
`PointerConstraintsClient`/`VirtualPointerClient` pattern from M4.5/M4.2.

New harness clients:
- `TextInputClient` — binds `zwp_text_input_manager_v3`, maps a surface (so it
  can gain keyboard focus through the existing focus-injection path used
  throughout the harness), creates a `zwp_text_input_v3`, can `enable`/
  `commit`/`disable`, and records received `enter`/`leave`/`preedit_string`/
  `commit_string`/`delete_surrounding_text`/`done` events.
- `InputMethodClient` — binds `zwp_input_method_manager_v2`, creates a
  `zwp_input_method_v2`, records `activate`/`deactivate`/`surrounding_text`/
  `content_type`/`done`/`unavailable` events, can send `commit_string`/
  `preedit_string`/`delete_surrounding_text` + `commit`, can request
  `grab_keyboard`, can create a popup surface.

A6.1 tests:
1. **Globals advertised.** `zwp_text_input_manager_v3` and
   `zwp_input_method_manager_v2` appear in `advertised_globals`.
2. **Enter/leave follows keyboard focus, not pointer focus.** Map a
   `TextInputClient` surface, move the virtual pointer over a *different*
   surface (pointer focus elsewhere) and give this surface keyboard focus via
   the harness's existing focus-injection path; assert `enter` fired despite
   pointer focus being elsewhere — the non-vacuous half is a baseline
   assertion that pointer-only presence over the surface, without keyboard
   focus, does **not** fire `enter`.
3. **Enable → activate → surrounding-text relay.** With an `InputMethodClient`
   bound first, focus + `enable` + `commit` (setting surrounding text and a
   cursor rectangle) on a `TextInputClient`; assert the IME receives
   `activate`, matching `surrounding_text`, and `done`, in order.
4. **IME commit → app relay.** With both bound and the text-input
   focused+enabled, have the `InputMethodClient` send `preedit_string` +
   `commit_string` + `commit`; assert the `TextInputClient` receives matching
   `preedit_string`/`commit_string`/`done` events.
5. **Disable/leave deactivates the IME.** `disable` the text-input (still
   focused); assert the IME receives `deactivate`. Move keyboard focus away
   entirely; assert the text-input receives `leave`.
6. **Second input-method is refused.** Bind a second `InputMethodClient` while
   the first is live; assert the second receives `unavailable` and never
   receives `activate` even when a text-input enables.
7. **`DbCommand::InputMethodActive`** reflects true only while an
   enabled+focused text-input has driven an `activate` that hasn't since been
   deactivated — used as the oracle for tests 3/5/6 instead of relying only on
   client-side event capture, the same "assert on both sides" discipline
   M4.4's `SessionLocked` reply command established.

A6.2 tests (once A6.1 lands):
8. **Popup surface is positioned and told its anchor rectangle.** Create a
   popup surface from the `InputMethodClient` while focused+enabled with a
   known `cursor_rectangle`; assert the popup receives
   `text_input_rectangle` and that the compositor placed its scene node (a
   `DbCommand` accessor for the popup's placed output-space position, mirroring
   `CursorPosition`).
9. **Keyboard grab intercepts physical keys.** Request `grab_keyboard` on the
   `InputMethodClient`; inject a physical key via the harness's existing
   keyboard-injection path; assert the grab receives `key`/`modifiers` and the
   focused text-input's surface (via the normal `wl_keyboard` protocol, not
   text-input) receives **nothing** — the non-vacuous half is the same
   assertion run *before* the grab is requested, showing the key **does**
   reach the surface normally.
10. **icedtea keybindings still fire during a grab.** With a grab active,
    inject the compositor's alt-tab keybinding; assert alt-tab still triggers
    (decision #6's "dispatcher runs first, forwarding target changes"
    guarantee) — this is the one test that would catch a refactor that
    accidentally short-circuits the `session.dispatcher.emit` call instead of
    only the forwarding call after it.

**Gates:** `cargo test --workspace` + `cargo clippy --all-targets -- -D
warnings` (icedtea); `cargo test -p wlr` + clippy + `RUSTDOCFLAGS="-D warnings"
cargo doc -p wlr --no-deps` + `cargo fmt --all --check` (wlroots-sys) — green
on the freeze commit before each publish (0.20.23 for A6.1, 0.20.24 for A6.2).

## Success criteria

1. `zwp_text_input_manager_v3` and `zwp_input_method_manager_v2` are
   advertised (automated).
2. Text-input enter/leave tracks keyboard focus only, independent of pointer
   focus (automated, non-vacuous).
3. Enable/commit/disable on a focused text-input correctly drives
   activate/surrounding-text/deactivate on the bound input-method, and the
   IME's preedit/commit/delete-surrounding relay lands on the app's
   text-input — both directions automated.
4. At most one input-method is ever active per seat; a second bound client is
   told `unavailable` (automated).
5. (A6.2) Popup surfaces are placed in the scene graph near the focused
   text-input's cursor rectangle and told their anchor rectangle (automated).
6. (A6.2) A requested keyboard grab intercepts physical keys before they
   reach the focused surface's normal `wl_keyboard`, while icedtea's own
   keybindings continue to fire (automated, non-vacuous both ways).
7. The crate additions are semver-additive, published as two `0.20.x`
   releases (0.20.23, 0.20.24); the compositor consumes each; all gates green
   in both repos; final whole-branch review clean or resolved; merge window
   presented — never auto-merged.

## Out of scope (explicit)

- **tablet-v2** — separate follow-on, see "Decomposition into milestones."
- **Any actual IME or OSK application** — fcitx5/ibus/squeekboard are
  third-party clients; this milestone only makes the compositor speak the two
  protocols correctly. No IME ships in this repo.
- **A visual on-screen-keyboard shell surface** — that is Track B (gated on
  the pure-Rust toolkit rebuild per the roadmap's two-track split); this
  milestone is Track A only, protocol plumbing.
- **Changing virtual-keyboard's existing behavior** — A6 is additive on top of
  it, per the "Relation to virtual-keyboard" section; no change to
  `create_virtual_keyboard_manager` or its handling.
- **Multi-seat input-method semantics** — icedtea runs a single headless seat
  (`seat0`, `lib.rs:130-132`), same scope limitation M4.5 already noted for
  pointer constraints.
- **Content-type-driven compositor behavior** (e.g. auto-hiding an OSK for a
  `terminal` purpose field, or blocking history/prediction for a `password`
  hint) — the protocol data (`content_type.{hint,purpose}`) is relayed
  faithfully to the IME, which is the party expected to act on it; the
  compositor does not interpret it itself. A future OSK shell surface (Track
  B) may choose to read it back via its own text-input/input-method binding.
- **`wp_text_input_unstable_v1`/`v2`** (the older, pre-v3 protocols some
  legacy clients still bind) — v3 only, matching wlroots 0.20's own scope
  (only `wlr_text_input_v3.h` ships; no v1/v2 headers exist in
  `/usr/include/wlroots-0.20/wlr/types/`).

## Risks

- **No wlroots-provided relay to crib from.** Every other protocol batch in
  this tree (M1-M4.5) wraps a wlroots helper that already does the hard part;
  here the crate is writing genuine new relay logic. Mitigation: the design
  mirrors sway's `input_method_relay.c` policy (single active IME, activate-
  on-enable not activate-on-enter, deactivate-on-focus-loss) rather than
  inventing a new one, and the non-vacuous enter/leave and activate/deactivate
  tests (A6.1 tests 2, 3, 5) pin that policy down.
- **Keyboard-grab forwarding sits in the hottest possible path** (`on_key`,
  every keystroke, `backend.rs:5332`). A wrong branch could silently eat every
  keystroke compositor-wide the moment any IME grabs. Mitigation: test 9's
  before/after non-vacuous pair, plus test 10 pinning that the
  `session.dispatcher.emit` call (icedtea's own keybindings) is untouched by
  the branch — only the trailing forward-to-seat-vs-forward-to-grab choice
  changes, matching decision #6 precisely.
- **Popup placement has no wlroots geometry helper** (decision #7) — unlike
  `wlr_scene_xdg_surface_create`'s automatic positioner-driven placement, the
  compositor computes the anchor itself from the text-input's
  `cursor_rectangle`. Mitigation: reuse the coordinate-transform code
  `render.rs` already has for decoration placement rather than writing new
  math, and clamp-to-output the same way multi-monitor output migration
  already clamps window geometry.
- **Destroy-listener `data` keying**, the now-familiar M4.4/M4.5 lesson,
  applies to four new destroy paths at once here (text-input, input-method,
  popup-surface, keyboard-grab) instead of one. Mitigation: verify each
  signal's `data` payload against wlroots source before wiring (not assumed
  from the header alone), same discipline as M4.4/M4.5, and key every one of
  the four listener maps on the listener's own address per decision #5's
  citation.
- **Second-IME-refused path is easy to get backwards** (refuse the new one vs.
  evict the old one) — decision #3 refuses the new one, matching sway; a
  regression here silently breaks whichever IME started first every time both
  fcitx5 and an OSK autostart. Mitigation: test 6 pins the direction.

## Execution

New `wlr` releases 0.20.23 (A6.1) and 0.20.24 (A6.2). Branch
`feature/wlr-a6.1`/`feature/wlr-a6.2` off `develop` (wlroots-sys, coordinated
with any parallel session per decision #9) and matching `a6.1`/`a6.2` branches
off `develop` (icedtea). Per-task review on Opus; split-phase publish
discipline; final whole-branch review; wlroots-sys via a PR into `develop`
(CI: test/miri/msrv/fmt); merge window presented for the user's go/no-go —
never auto-merged.
