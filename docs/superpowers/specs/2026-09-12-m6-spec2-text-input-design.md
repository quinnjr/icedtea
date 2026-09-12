# M6.1 Spec 2 — toolkit `text-input-v3` client

**Status:** draft for review, 2026-09-12. Workstream of the icedtea pure-Rust UI rebuild.
**Parent:** `2026-08-20-pure-rust-gtk-ui-design.md` (program spec; M6 item 6 — polish).
**Predecessor (DONE, on develop):** `2026-09-08-m6-1-ime-compositor-relay-design.md`
(Spec 1: `zwp_text_input_manager_v3` + `zwp_input_method_manager_v2` relay,
contract v5 IME depth + remainder consumers). This spec does **not** restate
the relay; it is the client Spec 1 unblocks.
**Regression closed:** P6-D8 — settings' wallpaper-path / workspace-name /
keybinding fields and the shell clipboard-popover search field accept direct
key events only: no CJK/compose/dead-key composition, no on-screen keyboard.
Verified: zero `text_input`/`surrounding`/`preedit` code exists in `ui/`,
`settings/`, `shell/` today.
**Out of scope:** shipping an IME/OSK (same as Spec 1); `tablet-v2`; accesskit
metadata and drag-and-drop (two sibling agents own those streams).

## 1. Approach

The toolkit's `Window` (hand-rolled on `wayland-client`, `ui/src/window/`)
grows a `text-input-v3` client next to the seat/keyboard objects it already
owns; the five entry-family controllers (`Entry`, `PasswordEntry`,
`SearchEntry`, `TextView`, `EditableLabel`) drive it through focus and edit
events they already receive. No `smithay-client-toolkit`, no `calloop` —
same shape as `settings/src/outputs/protocol.rs` and the harness doubles.

Three layers, each independently testable:

1. **`ui/src/text_input.rs` (NEW, pure logic + thin proxy wrapper).**
   No `Window`, no controller imports, so libtest covers it without a
   compositor:
   - `ContentPurpose` / `ContentHint`: toolkit-side enums with an exact
     mapping to the wire `content_type(hint, purpose)` `u32`s (verify names
     against `wayland-protocols 0.32`'s `wp::text_input::zv3::client` during
     implementation — the harness already binds this path, so the crate
     version is proven).
   - Pure buffer ops, all `#[must_use]`-clean and panic-free on untrusted
     input (part-5 global constraint; reuse `edit::clamp_to_boundary`):
     `apply_commit_string(buffer, cursor, anchor, text) -> (String, usize)`,
     `apply_delete_surrounding(buffer, cursor, anchor, before_len, after_len)`
     (lengths are **UTF-8 bytes** per the protocol — clamp to char
     boundaries, never `replace_range`-panic),
     `Preedit { text, cursor_begin, cursor_end }` as display-only state that
     never touches `buffer` until `commit_string` arrives.
   - `PendingState`: the double-buffered client state
     (`surrounding_text`, cursor, anchor, content-type, cursor-rectangle)
     with a `needs_commit` diff so unchanged state is never re-sent.
   - `TextInputConn`: thin wrapper over the live `ZwpTextInputV3` proxy
     (`enable`/`disable`/`set_surrounding_text`/`set_content_type`/
     `set_cursor_rectangle`/`commit`). Null-safe no-op when the compositor
     never advertised the manager — degraded IME, working typing.
2. **`ui/src/window/` integration (registry + dispatch + pump).**
   - Bind `zwp_text_input_manager_v3` in the registry handler (optional
     global, like `primary_manager`: absence is not fatal); create one
     `ZwpTextInputV3` per seat after the seat binds.
   - `Dispatch<ZwpTextInputV3>` impl **records only**
     (`preedit_string`/`commit_string`/`delete_surrounding_text`/`done` into
     `Vec`s on `WindowState`). No `unwrap`/`expect` in these callbacks —
     they run on C frames; tests assert after the run (same discipline as
     the harness `ClientState` doubles and the job's explicit gate).
   - New `InputEvent::ImePreedit { text, cursor_begin, cursor_end }`,
     `InputEvent::ImeCommit(String)`, `InputEvent::ImeDelete { before, after }`
     variants, drained by `Window::pump` after Wayland dispatch. `done`
     serials are recorded; batching applies all pending IME events at `done`.
   - `Window::ime_enable(hint, purpose)`, `Window::ime_disable()`,
     `Window::ime_sync(buffer, cursor, anchor, cursor_rect)` push the
     enable→surrounding/content-type/cursor-rect→commit sequence through
     `TextInputConn`.
3. **Widget controllers + apps.**
   - `Event::FocusIn` → `ime_enable` + initial `ime_sync`;
     `Event::FocusOut` → `ime_disable`. Every `Changed` outcome from
     `TextEditState::key` → `ime_sync` with the fresh buffer/cursor.
     `Event::Ime*` (new controller-level events, name TBD at implementation)
     → apply preedit/commit/delete to `TextEditState`, reshape, fire
     `Change` on commit only (preedit is display-only).
   - Per-widget content types: `PasswordEntry` → purpose password;
     `SearchEntry` → hint none/purpose normal (search purpose if the wire
     enum has one — verify); `Entry`/`TextView`/`EditableLabel` → normal.
   - `settings`: the three fields already construct `entry()` — they inherit
     IME once the toolkit speaks it; set purpose/hint props where known
     (keybinding-capture field: verify which widget it uses during
     implementation — it may need a purpose/hint, not a new widget).
   - `shell`: the clipboard popover (`panel::popover_rows`) has **no**
     search field today — add a `search_entry` on top + live row filtering
     (new `Msg::ClipSearch(String)`), which is the field this spec's e2e
     composes into.

Cursor rectangle: entry controllers already compute caret geometry for paint
(`layout.caret_rect(cursor)`); `ime_sync` carries the caret rect in
surface-local coordinates so the compositor can place candidate popups
(Spec 1 A6.2 consumes `cursor_rectangle`).

## 2. Wire flow

```
 focus Entry            type                     IME preedit              IME commit
 ───────────            ────                     ───────────              ──────────
 FocusIn → enable ─┐
 ime_sync:          ├→ commit ─→ relay ─→ IM.activate + surrounding ─→ IM shows candidates
  surrounding,      ┘
  content_type,
  cursor_rect
                                              preedit_string ─→ done ─→ display preedit (no buffer write)
                                              commit_string + done ─→ buffer write, cursor advance, fire Change
                                              delete_surrounding + done ─→ buffer delete (byte lengths, clamped)
 edit key → ime_sync (new surrounding) ─→ commit ─→ relay re-forwards (Spec 1 test: reforwards updated surrounding)
 FocusOut → disable + commit ─→ relay ─→ IM.deactivate
```

Ordering rules (from Spec 1 + protocol):
- `enable` precedes any state; every state change ends in `commit`.
- `surrounding_text(cursor, anchor)` echo the widget's real caret/selection
  (`anchor = cursor` when no selection).
- IME events are applied at `done` granularity; `done` serials only recorded.
- Second-IME `unavailable`, keyboard-grab intercept, popup placement: relay
  side, already tested — the client just keeps typing state consistent
  (preedit cleared on `leave`/focus loss).

## 3. Widget integration points

| File | Change |
|---|---|
| `ui/src/text_input.rs` (NEW) | pure ops + `PendingState` + `TextInputConn` + content enums; unit tests in-file |
| `ui/src/lib.rs` | `pub mod text_input;` |
| `ui/src/window/mod.rs` | manager/text-input fields on `WindowState`, registry bind, `Dispatch`, `InputEvent::Ime*`, `Window::ime_*` methods |
| `ui/src/view/controller.rs` | new `Event::Ime*` variants (or reuse path TBD — pin during TDD) |
| `ui/src/view/cmd.rs` | new `Cmd::ImeEnable/ContentHint…`, `Cmd::ImeSync{…}`, `Cmd::ImeDisable` (App forwards to `Window`) |
| `ui/src/widgets/edit.rs` | `apply_preedit/apply_commit/apply_delete` on `TextEditState` (wrap the pure ops + reshape + undo record on commit) |
| `ui/src/widgets/entry.rs`, `password_entry.rs`, `search_entry.rs`, `text_view.rs`, `editable_label.rs` | FocusIn/Out → enable/disable cmds; Changed → sync cmd; Ime events → apply + Change on commit |
| `settings/src/pages/appearance.rs`, `workspaces.rs`, `keybindings.rs` | purpose/hint props on the three fields (no new widgets expected) |
| `shell/src/panel.rs` | `search_entry` + `Msg::ClipSearch` + filter in `popover_rows` call sites |
| `compositor/tests/m6_spec2_text_input.rs` (NEW) | relay-level contract tests with harness doubles (do NOT edit `client_protocol.rs`) |
| `ui/tests/m6_spec2_text_input_e2e.rs` (NEW) | toolkit `Window` + `InputMethodClient` e2e through the real relay (`ui` dev-deps already have harness+compositor; compositor dev-deps do not have `icedtea-ui`, so the toolkit e2e cannot live in `compositor/tests`) |
| `harness/src/lib.rs` | append-only `// M6-Spec2` section at EOF **only if** a double is missing something (not expected — `TextInputClient` + `InputMethodClient` already cover enable/commit/disable/preedit/delete/done) |

## 4. Test plan (TDD: failing test → pass, bite-sized commits)

1. `ui/src/text_input.rs` unit tests: commit-string splice (mid-CJK cursor,
   selection replace), delete-surrounding byte-length clamping at char
   boundaries, preedit display-only (buffer untouched), `PendingState` diff
   (no commit when unchanged), content-enum wire values, null-conn no-op.
2. `edit.rs` apply tests: commit records one undo step; preedit→commit
   coalesces; delete clamps; `set_text` (model write) clears preedit.
3. Controller tests (in-file, existing style): FocusIn emits enable+sync,
   FocusOut emits disable, Changed emits sync, ImeCommit applies + fires
   Change, ImePreedit applies display-only.
4. `compositor/tests/m6_spec2_text_input.rs`: relay contract the client
   depends on, with doubles only — enable→activate→surrounding echo,
   updated-surrounding re-forward, IME preedit/commit/delete→app, disable→
   deactivate. (Overlaps Spec 1's tests deliberately: this file pins the
   client's wire assumptions; `client_protocol.rs` stays untouched.)
5. `ui/tests/m6_spec2_text_input_e2e.rs`: boot harness `Compositor`,
   `Window::open_at_path` with an Entry focused, `enable` path asserted via
   `InputMethodClient.wait_until(surroundings…)`; then
   `InputMethodClient.send_commit(preedit/commit)` → pump window → assert
   the Entry buffer holds the committed string. **This closes P6-D8.**
   Settings fields + shell search inherit the same controllers; cover with
   controller-level Change-assertion tests, not full app boots.
6. Gates: `cargo test -p icedtea-ui -p icedtea-compositor`,
   `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
   Known pre-existing failures (3 `icedtea-ui` gallery_gate render tests,
   1 unused-`ids` warning in `shell/src/panel.rs` m8 test) are not mine.

## 5. Risks

- `wayland-protocols 0.32` client-side `zv3` enum/module names may differ
  from the harness-side usage — verify by compiling against the same crate
  the harness uses before writing proxy code.
- `Window::pump` → App → controller round trip for IME events must not
  deadlock the queue: IME events are drained as data, applied at the App
  layer like `Key` events are today.
- Sibling streams (accesskit, DnD) share `ui/` — this spec touches only
  `text_input`, entry-family controllers, `view/cmd.rs`, `view/controller.rs`
  event enum, and `window/mod.rs` IME sections; shared helpers go in the new
  module, never in another stream's likely files.
