# Pure-Rust GTK UI rebuild — Milestone 5: App migrations

**Status:** implemented on `rebuild/pure-rust-gtk-m5` (parts 0–6 of
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md`); `gtk4`,
`gtk4-layer-shell`, `glib`, `gio`, `gdk4`, `pango` and `cairo` are gone from
the workspace lockfile; awaiting owner review before merge
**Parent:** `2026-08-20-pure-rust-gtk-ui-design.md` (program spec; M5 is item 5 of
its decomposition). **Predecessor:** `2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
(M3, merged to develop @ `488e7a1`; wlr 0.20.29 follow-up @ `d9cee52`).
**Branch:** `rebuild/pure-rust-gtk-m5` (off develop `d9cee52`).

## 1. Goal

Rebuild the two GTK applications on `icedtea-ui`, dropping `gtk4`,
`gtk4-layer-shell`, `glib`, `gio`, `gdk`, `pango` and `cairo` from the workspace
entirely, with every pure core reused verbatim and behavioural parity proven by
the same mock-driven assertions the GTK tests use today.

The parent spec lists M5 as "settings, then shell/bar, then the clipboard shell".
The understand pass corrected that framing:

- **Settings** (`settings/`, 4,422 lines; ~1,500 GTK-bound across five pages) is
  the richest widget exerciser and the first target.
- **Shell panel** (`shell/`, 900 lines) is the taskbar *and* a ~100-line
  clipboard popover sharing one layer-shell window, one process, one style sheet.
  They are one migration, not two.
- **The clipboard daemon** (`clipboard/`) has zero GTK and needs no work.

So M5 is settings → shell panel. Both migrations swap dependencies **in place**:
the GTK binary stops existing at the first commit of each app's part. The primary
checkout only ever sees the finished migration, because all M5 work happens in a
worktree and merges as one PR — the coexistence cost the alternative designs
(second binary target, sibling crates, feature flag) paid for is moot under the
project's worktree-plus-single-PR rule.

## 2. Decisions

Settled by the parent spec and the M3 contract, restated so M5 honours them:

1. No `gtk4`/`gio`/`glib`/`gobject`/`pango`/`cairo`/`gdk` in a migrated app.
   `gtk4-layer-shell` goes with it: `icedtea-ui` owns `zwlr_layer_shell_v1`.
2. Each app reuses its GTK-free pure core unchanged; only the view layer is rebuilt.
3. The toolkit is Elm-shaped: `view(&M) -> View<Msg>`, keyed reconciliation into a
   retained node tree, `update(&mut M, Msg) -> Cmd<Msg>`, non-reentrant.
4. One `Window` and one `App` per surface; roles `Toplevel` / `Layer` / `Popup`.
5. `wayland-client` + `wayland-protocols(-wlr)` directly; hand-rolled poll loop; no
   Smithay/sctk/calloop.
6. Theming is GTK4 theme-file compatible; app style sheets layer as a cascade origin.
7. Cursor shape via `wp_cursor_shape_v1`; no client-side cursor themes.
8. M6 scope — IME/`text-input-v3`, drag-and-drop, `accesskit`, emoji, portal
   integration beyond the file chooser — is out of bounds, even opportunistically.
9. Compositor, config, contract and harness crates are out of UI-rebuild scope.
10. D-Bus wire names are PascalCase (zbus `#[interface]`); the live tests exist
    because this shipped as a bug once.
11. `settings/tests/live_apply.rs` keeps spawning the **real** `icedtea-compositor`
    binary; `icedtea_harness::Compositor` deliberately omits the config-reload worker.

Decided in this brainstorm (owner's picks):

| # | Decision | Choice |
|---|---|---|
| D1 | Milestone shape | Toolkit pre-flight part (P0) closing exactly what M5 blocks on, then settings with Displays last, then the shell panel, then a deletion close-out. Seven parts. |
| D2 | External-event ingress | `Window::watch_fd` + wake-pipe + `App::with_inbox` channel. No 16 ms polling; no re-binding of protocols onto the toolkit's connection. |
| D3 | Coexistence | Swap in place. |
| D4 | Monitor-layout canvas | `DrawingArea` + pointer `EventKind`s; `displays_canvas.rs` reused verbatim. |
| D5 | Wallpaper picker | `org.freedesktop.portal.FileChooser.OpenFile` over zbus from the app; validated path `Entry` with preview as the no-portal fallback. |
| D6 | Parity | Port the mock-driven assertions to harness-driven toolkit tests; add rest-state screencopy and interaction gates per page/panel. |
| D7 | Settings navigation | Keep `Stack` + `StackSwitcher` (settings has no libadwaita; there is no `Adw*` idiom to keep or drop). `StackSidebar` revisited after its eviction defect is fixed. |
| D8 | Command seam | Keep `CompositorCommands`/`ClipCommands` and their mocks; delete `bridge.rs` once ingress exists; outbound calls run on the worker via `Cmd::Task`, never inside `update`. |
| D9 | Shell rendering | Genuinely reactive; clear-and-rebuild was a GTK workaround. |

## 3. Architecture

```
settings binary                        shell binary
┌──────────────────────────┐           ┌──────────────────────────┐
│ App<SettingsModel, Msg>  │           │ App<PanelModel, Msg>     │
│  Role::Toplevel 480x420  │           │  Role::Layer top/L|R|T   │
│  view: Stack+Switcher    │           │  view: ws | windows | clip│
│  ├ Appearance            │           │  popover: Cmd::OpenPopup │
│  ├ Behavior              │           └────┬──────────┬──────────┘
│  ├ Workspaces            │                │ inbox    │ inbox
│  ├ Keybindings           │     compositor_client.rs  clip_client.rs
│  └ Displays (DrawingArea)│     (worker thread, D-Bus)  (worker, D-Bus)
└───┬──────────┬───────────┘
    │ inbox    │ watch_fd
 reload worker  outputs EventQueue fd
 (zbus)         (second wayland-client conn, zwlr_output_management_v1)
```

Both apps are one `App` on one `Window`. Every non-Wayland event source reaches the
loop through P0's ingress: worker threads post `Msg`s on the inbox channel and
wake the loop through a pipe; a second Wayland connection registers its queue fd.
`update` never blocks: outbound D-Bus goes to the worker via `Cmd::Task`.

## 4. P0 — Toolkit pre-flight (`ui/` only)

All additive. Each is a contract §10 amendment (`M5-D*`) with its own test.

### 4.1 External-event ingress

- `Window::watch_fd(&mut self, fd: OwnedFd, interest: Interest) -> WatchId` and
  `Window::unwatch(WatchId)`. `wait_bounded` polls `[wayland_fd, watched…]`
  instead of today's one-element array (`ui/src/window/mod.rs:855`). `pump`
  reports fired watches as `InputEvent::FdReady(WatchId)`.
- `App::with_inbox(self, inbox: Inbox<Msg>) -> Self`. `Inbox::new() -> (Inbox,
  Sender)`; `Sender: Send + Clone`, `send(msg)` pushes to the channel and writes one
  byte to the wake-pipe. `App::run` registers the pipe's read end with `watch_fd`;
  on its `FdReady` the App drains the pipe and the channel and feeds every `Msg`
  through `update` before the next frame.
- `App::on_fd(self, WatchId, impl Fn() -> Vec<Msg> + 'static) -> Self` maps a
  foreign fd's readiness to messages (the outputs `EventQueue`: dispatch, then
  emit `Msg::Outputs(event)` per event).
- Zero idle wakeups. Delivery order: inbox messages are applied in send order,
  before the input batch of the frame that woke.

### 4.2 Closures

`App::new(model, update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static, view:
impl Fn(&M) -> View<Msg> + 'static)`. Stored boxed. `fn` items still coerce, so
the gallery, the counter app and every existing test compile unchanged. This
amends the contract §3 signature at `2026-08-27-m3-part0-contract.md:1516-1521`
by name, not by drift.

### 4.3 Pointer events at the view layer

`EventKind::PointerDown`, `PointerMotion`, `PointerUp` fired with the existing
`Handler::Pair` (local coordinates). A button-carrying variant,
`Handler::PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>)`, for handlers that must
filter by button (middle-click-to-close). `DrawingAreaC::on_event` forwards the
controller-layer pointer events it already receives (`ui/src/widgets/drawing_area.rs`
handles only `Configure` today); the implicit grab already keeps motion flowing
after a press. Generic: any kind may carry the handlers; `Button` uses
`PairButton` in P5.

### 4.4 Base-level keysym

`Keymap::base_keysym(&self, keycode: u32) -> Keysym` — level 0, group 0 — the
equivalent of `gdk::Display::translate_key(keycode, 0, 0)` that
`settings/src/pages/keybindings.rs` uses. `xkb::Keymap` is a private field
(`ui/src/window/keyboard.rs:181`); an app has no workaround without this method.
Tested on the hermetic vendored `us` keymap: Shift+`a` reports base `a`.

### 4.5 Rest paint

`ColorDialogButton`/`ColorDialog` gain an intrinsic measure and a swatch paint;
`CheckButton` paints its unchecked box; `Scrollbar` paints trough and slider.
`KNOWN_BLANK_AT_REST` in `ui/tests/gallery_gate.rs` drops from nine to the six M5
never touches (`window_controls`, `font_dialog`, `alert_dialog`, `popover_menu`,
`popover_menu_bar`, `link_button`). If the zero-area fixes share a root cause,
the extra closures are welcome and recorded, not planned.

### 4.6 Live-window probe

`Window::probe_points()` / `Window::allocation(id)` on the live path (today
`App::probe` is offscreen-only, `ui/src/view/app.rs:348`), so harness tests
address widgets by id exactly as `gallery_gate.rs` does.

### 4.7 Docs

`ui/README.md`: the ingress/closure/pointer/keysym additions, and the stale
"seven collapse to zero-area" sentence (it is six; `scrollbar` allocates and
paints nothing) corrected.

## 5. Settings (P1–P4)

### 5.1 Shape

One `App<SettingsModel, Msg>` on a `Role::Toplevel` window, 480×420, app-id
`org.icedtea.Settings`. `SettingsModel` wraps `model::Model` (working/saved
diffing, `apply()`, the displays-preserving reload guard, 10 tests — untouched)
plus UI state: selected page, keybinding capture arming, displays edit state.
`view(&model)` is pure: `Stack` + `StackSwitcher` over five pages and a shared
footer (status label, Revert, Apply). The GTK `Ctx`/`Page` dirty contract
disappears: dirty is `model.working != model.saved`, computed. The
"populate never writes a widget fallback back into the model" property that
`appearance_gtk.rs` guards is true by construction on an Elm loop; recorded as a
deviation, not ported.

### 5.2 P1 — skeleton and pure-core extraction

Mechanical and first: the GTK-free logic embedded in GTK files moves to sibling
modules with its tests — `workspaces::{next_workspace_name,
prune_orphaned_workspace_bindings}`, `keybindings::{format_combo, action_list,
should_capture, take_capture_reset}`, `displays::{baseline_edit, default_mode_for,
same_connector_set, reconcile, distinct_resolutions, refreshes_for, enabled_rects,
all_rects}`. `displays.rs` (1,312 lines, one `build()` closure interleaving drag
maths, protocol wiring and control repopulation) is additionally split into
control-panel / message / canvas modules. Then: deps swapped, window opened,
navigation and footer live, pages stubbed, `apply()` wired to the reload worker
through the inbox. This is the first `App::run` on a `Role::Toplevel` window
(the raw `Window` API is proven there by `ui/tests/window_events.rs`; `App` has
only ever driven a `Role::Layer` gallery).

### 5.3 IPC

`compositor_reload.rs` (zbus blocking `ReloadConfig`) and the `ConfigReloaded`
observation run on a worker feeding the inbox. The outputs client keeps its own
`wayland-client` connection (`outputs/protocol.rs`, 682 lines, unchanged); its
`EventQueue` fd goes through `watch_fd` + `on_fd`. redb config path unchanged.

### 5.4 Pages

- **P2 Appearance:** wallpaper `Entry` + portal picker (worker thread calls
  `OpenFile`; the response URI becomes a `Msg`; no portal → `Entry` validation
  with existence/format check and a preview), three `ColorDialogButton`s, bar
  position `DropDown`, height/radius `SpinButton`s.
- **P2 Behavior:** three `Switch`es, snap-gap `SpinButton`.
- **P3 Workspaces:** `ListBox` of rows (`Entry` + Remove), Add.
- **P3 Keybindings:** `ScrolledWindow` of action rows; capture is
  `model.capturing: Option<Action>` plus an `on_key` at the window root using
  `Keymap::base_keysym`, normalised exactly as `compositor/src/input.rs` matches.
- **P4 Displays:** `DrawingArea` canvas with `displays_canvas::{compute_view,
  hit_test, snap}` verbatim and `PointerDown/Motion/Up`; per-head panel
  (enabled `Switch`; resolution / refresh / transform / scale `DropDown`s); its
  own Test/Revert/Apply footer as today. Known risk owned here: an embedded
  `DropDown` list is a fixed 240 px (`ui/src/widgets/drop_down.rs:350`,
  contract P7-D54) and will clip in a 420 px-tall window — P4 sizes lists to
  content with a max-height scroll.

## 6. Shell panel (P5)

### 6.1 Shape

One `App<PanelModel, Msg>` on `Role::Layer(LayerSpec { layer: Top, anchor:
Left | Right | Top, margin: [0; 4], exclusive_zone: <bar height>, keyboard: None })`
— the path `ui/src/gallery.rs` proves end-to-end. `PanelModel = { taskbar:
TaskbarModel, clipboard: ClipboardModel, open_popover: Option<Popover> }`;
both models leave their GTK files verbatim (`apply()`/`merge()` and six tests are
GTK-free logic embedded in `taskbar.rs`/`clipboard.rs`).

### 6.2 Reactive rendering

`view(&panel)` emits `[workspaces][window buttons][clipboard button]`; the keyed
reconciler (keys: workspace id, window id, history entry id) diffs it, so focus
and hover identity survive updates. Button classes `active`/`focused`/`attention`
come off the model. Middle-click-to-close uses `Handler::PairButton` on the
window buttons.

### 6.3 IPC

`compositor_client.rs` and `clip_client.rs` (worker thread, `CompositorCommands`
/ `ClipCommands`, `CompositorProxy`, version check against
`COMPOSITOR_CONTRACT_VERSION`) are kept as-is; their channel output is the inbox:
`CompositorUpdate` and `history_changed` become `Msg`s. `bridge.rs` is deleted.
The deliberate `process::exit(1)` on a post-connect D-Bus error survives (systemd
`Restart=always` relies on it). Outbound commands stay behind the traits, called
from `update` via `Cmd::Task` on the worker.

### 6.4 Clipboard popover

`Cmd::OpenPopup`/`ClosePopup` anchored to the clipboard button (a popup off a
`Layer` root — the best-proven surface path in the crate). A `ListBox` of history
entries: click pastes, Delete removes; `open_popover` is the single source of
truth. The search field has no IME until M6.

### 6.5 Styling

`shell/style.css` (30 lines) compiles to a `CompiledSheet` layered over the
Adwaita stack as a user-origin overlay — the mechanism the theme override uses.

## 7. Testing and parity gates

**Kept verbatim (toolkit-agnostic):** settings `model.rs` 10, `displays_canvas.rs`
8, `displays.rs` 12, `workspaces.rs` 6, `keybindings.rs` 6, `compositor_reload.rs`
1, `tests/outputs_client.rs`, `tests/live_apply.rs` (real compositor); shell
`taskbar` 5, `clipboard` 1, `compositor_client` 1, `tests/live_dbus.rs`.

**Re-expressed on the toolkit (behaviour, not widget tree):**

- `shell/tests/shell_gtk.rs` → `shell/tests/panel.rs`: under the harness, a
  virtual-pointer click on window 7's button reaches `MockWm::focus_window(7)`;
  middle-click → `close_window(7)`; a workspace click → `set_workspace(n)`; a
  `WindowOpened` signal through the inbox adds a button. Same mocks, same
  `(action, id)` assertions.
- `settings/tests/keybindings_gtk.rs` → a `VirtualKeyboardClient`-driven test:
  Shift+`a` while capturing records base `a` with Shift in mods.
- `settings/tests/appearance_gtk.rs` → not ported (true by construction; §10 record).

**New gates (the `gallery_gate` / `interaction_gate` pattern):**

- Rest-state screencopy per settings page and for the panel, light/dark/hc: every
  probe point paints; no `KNOWN_BLANK` exemptions for app widgets.
- Interaction gates: displays drag-to-snap moves a head and the model's rect
  updates; a colour pick changes a swatch; Apply reaches `ReloadConfig` on the
  mock; the clipboard popover opens, a click pastes, an outside click dismisses.
- P0 API tests: `watch_fd` wakes `pump` from a foreign fd; inbox delivery order;
  pointer events on `DrawingArea` through a grab; `base_keysym` on the hermetic
  keymap; three rest-paint pixel tests.

Screencopy gates cost minutes each under the harness: they run per part, and the
close-out runs the full set once.

## 8. Close-out (P6)

`cargo tree -p icedtea-settings -p icedtea-shell` shows no `gtk4`,
`gtk4-layer-shell`, `glib`, `gio`, `gdk`, `pango`, `cairo`; the `*_gtk.rs` tests
are gone; the full gate set runs; the M5 contract §10 records every deviation:
the populate-guard by construction, the IME regression, the DropDown clipping
resolution, the contract §3 `App::new` amendment, and anything the parts add.
The M5 branch merges via one PR after the owner's review window.

## 9. Process

Same pipeline as M3: this spec → `writing-plans` (contract part 0 + seven part
plans, judged, consistency-checked) → the execute-part workflow per part in the
M5 worktree (implement / review / fix per task, whole-part Opus review + fix
wave) → whole-M5 review → `/lex-review` at the merge window. Each part's base
is the previous part's head; deferred minors ride forward in the notes. P0's part
review reads the P4 and P5 plans to confirm the APIs meet their needs. P0 is a
`ui/` change (workspace-internal crate), not a publish.

## 10. Risks

| Risk | Answer |
|---|---|
| P0 under-scoped → P4/P5 stall | P0's plan is judged against P4/P5's needs; P0's part review reads their plans. |
| The six zero-area widgets share one layout root cause | P0 fixes ColorDialog's; a generic fix closes the rest for free and is recorded. |
| `displays.rs` ported without a split | The split is P1's job, before any page ports. |
| `DropDown` fixed 240 px list clips in settings | P4: size to content, max-height scroll. |
| Harness CI time | Gates per part; full set at close-out only. |
| IME regression at deletion | Stated here and in the PR; M6 by policy. |
| `App::run` has never driven a `Toplevel` | P1's skeleton is exactly that, first. |
| `live_apply.rs` moved onto the harness by mistake | Decision 11 above; the plan names the real binary. |

## 11. Out of scope

IME / `text-input-v3`, drag-and-drop, `accesskit`, emoji, portal integration
beyond `FileChooser`, a native file dialog, the six untouched `KNOWN_BLANK`
widgets, `StackSidebar`'s eviction defect, the M2-inherited background-layer
`currentColor` gap, and the compositor's `sh -c` spawn action (pre-existing,
flagged by the M3 security review; not UI work).
