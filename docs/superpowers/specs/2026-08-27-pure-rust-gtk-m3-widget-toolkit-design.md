# Pure-Rust GTK-themed UI — M3: Widget Toolkit, Popups, Windows, Icons — Design

**Date:** 2026-08-27
**Status:** approved in brainstorming (2026-08-27); awaiting owner spec review
**Branch:** `rebuild/pure-rust-gtk-m3` (off `develop` @ 8df998e)
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` (§Decomposition M3 + M4)
**Previous milestone:** M2 — `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
(engine, node tree, layout/paint/animation, fontconfig; `develop` @ 8df998e)
**Crates:** `ui/` (`icedtea-ui`), `compositor/`, `harness/`; `wlr` (wloots-sys) → 0.20.28

## Goal

Turn M2's styled-node engine into a **working widget toolkit**: real windows
(xdg-toplevel and layer-shell) and **popups** (xdg-popup, end to end from the
`wlr` crate through the compositor), keyboard/pointer/focus handling, a
**reactive view framework**, the **GTK 4.22 core widget set** with GTK-exact CSS
node trees, and **icon theming** (the parent plan's M4, folded in). M5 then
migrates `settings`, `shell` and `clipboard` onto it.

## Decisions (settled in brainstorming, 2026-08-27)

1. **xdg-popup is part of M3**, not a separate milestone: `wlr` 0.20.28 +
   compositor placement/grab/dismiss + harness support + the toolkit's
   `Popup` surface. Found as a compositor gap on 2026-08-26 (the `wlr` crate had
   no popup support at all).
2. **Reactive app model:** `view(&Model) -> View` description trees, keyed
   reconciliation into M2's retained `Node`s (identity survives), per-kind
   controllers producing `Msg`s folded by `update` (Elm loop). Not
   GTK-style retained objects with setters; not signals; not immediate mode.
3. **Keyboard via libxkbcommon** (`xkbcommon` 0.9 crate; native lib, like
   fontconfig).
4. **Widget set = GTK 4.22's gallery minus deprecated and service-bound
   widgets** (exact list in §5).
5. **Icon theming (parent M4) folds into M3**: freedesktop icon themes,
   symbolic recolouring, builtins, HiDPI. Client-side cursor themes are out
   (icedtea supports `wp_cursor_shape_v1`).
6. **Framework architecture = keyed view tree diffed into `Node`s** (approach
   A), with behaviour in controllers attached to retained nodes.

## Section 1 — Architecture & parts

One spec, executed as a contract + eight part-plans, parts in order:

| Part | Scope | Repo |
|---|---|---|
| P1 | `wlr` 0.20.28: xdg-popup on toplevels and layer surfaces, positioner geometry, `unconstrain`, `reposition`, scene subtree, explicit grab semantics, `ToplevelHandler::{new_popup, popup_reposition, popup_destroyed}` (defaulted); ledger/docs; publish (consent) | wloots-sys |
| P2 | compositor popup placement/stacking/focus + harness popup client + `compositor/tests/popups.rs` | icedtea |
| P3 | `ui/src/window/`: `Surface::{Toplevel, Layer, Popup}`, xkbcommon keyboard, pointer/touch routing, focus model, clipboard/primary selection | ui |
| P4 | `ui/src/view/`: `View`, builders, keyed reconciler, controllers, `App` loop, `Cmd` | ui |
| P5 | widgets: display, buttons, entries | ui |
| P6 | widgets: containers, lists, popovers/menus, windows/dialogs | ui |
| P7 | `ui/src/icons/`: icon-theme lookup, SVG/PNG render, symbolic recolour, builtins, `-gtk-icon-*` | ui |
| P8 | `gallery` binary + `ui/tests/gallery_gate.rs` + interaction gate + README/spec status | ui |

Module layout added to `ui/src/`: `window/{mod,toplevel,layer,popup,keyboard,pointer,focus,selection}.rs`,
`view/{mod,builders,reconcile,controller,app,cmd}.rs`, `widgets/<one file per widget>.rs`,
`icons/{mod,theme,lookup,render,builtin,symbolic}.rs`, `bin/gallery.rs`.

## Section 2 — Popups: `wlr` crate + compositor (P1/P2)

**Crate.** Listeners on `wlr_xdg_shell.events.new_popup` and
`wlr_layer_surface_v1.events.new_popup`. `PopupId`-keyed handle:
`Runtime::popup(id) -> Popup { parent: PopupParent::{Toplevel(ToplevelId) |
Layer(LayerId) | Popup(PopupId)}, anchor_rect, positioner: PositionerRules,
reactive, grab_requested }`. Scene subtree via
`wlr_scene_xdg_surface_create(parent_tree, popup.base)` so popups stack with
their parent. `Popup::unconstrain(box)` → `wlr_xdg_popup_unconstrain_from_box`;
`Popup::position()` → `wlr_xdg_popup_get_position`; `reposition` re-runs the
rules and re-sends configure. **Grab:** wlroots creates `wlr_xdg_popup_grab`
on the client's `grab` request; the crate applies the seat-grab semantics —
pointer/keyboard delivered to the popup chain; a press outside the topmost
popup destroys the chain (`popup_done`); keyboard focus returns to the grab's
previous focus. An explicit popup grab supersedes the implicit pointer grab
(the crate's existing rule). Handler methods defaulted (additive 0.20.x).

**Compositor.** On `new_popup`: constraint box = the parent's output usable
area (a layer parent's output for panels); `unconstrain`; record the popup
under its parent for z-order and `window_at_point` (above the parent, below
other layer overlays). Grabbing popups take keyboard focus; on destroy, focus
returns to the parent, not the pointer position. Session lock: popups on hidden
windows get no input. Escape dismissal is client-side (GTK sends
`popup.destroy`); the compositor synthesises nothing.

**Harness.** `TestClient::open_popup { anchor_rect, size, gravity, grab }`,
`popup_configured() -> Option<(i32,i32,u32,u32)>`, `popup_done()`,
`open_popup_from_popup`. `compositor/tests/popups.rs`: positioned under a
toplevel (geometry = positioner rules); flipped when it would leave the
output; under a layer panel; nested chain; grab: click outside dismisses the
whole chain and keyboard focus returns to the parent; the implicit-grab tests
keep passing.

## Section 3 — Window & event layer (P3)

**Surfaces.** `window::Surface` over one `wl_surface` + shm pool + the M1/M2
paint pipeline: `Toplevel` (`xdg_toplevel`: title, app_id, min/max size,
`configure` states → relayout + `:backdrop`/`:fullscreen` states; decorations
are the compositor's SSD), `Layer` (existing path + `new_popup` parenting),
`Popup` (`xdg_popup` with an `xdg_positioner` from the toolkit's
anchor/gravity; `grab(serial)` for menus; `popup_done` → controller). A
`Window` owns the root `Node`, a `LayoutTree`, an `AnimationState` and the
frame pump; child popups are separate `Surface`s parented to it.

**Keyboard.** `wl_keyboard` → `xkbcommon` (`Context`, keymap from the
`keymap` fd, `State::update_key/update_mask`, `key_get_utf8/keysym`, compose
table from the locale, repeat from `repeat_info` on the animation clock).
`KeyEvent { keysym, utf8, mods, repeat }` → focused node's controller;
unhandled → focus navigation (Tab/Shift-Tab over focusable nodes in tree
order; arrows in opted-in containers; Space/Enter activate) → `Msg::Key`.

**Pointer/touch.** Hit-test the retained tree (per-node allocations,
topmost-painted wins, `opacity: 0`/invisible skipped); client-side implicit
grab mirrors the compositor's (motion/release to the pressed node); axis
events to the nearest scrollable ancestor with kinetic deceleration on the
animation clock; touch maps to the same path; `cursor` CSS property →
`wp_cursor_shape_v1` names.

**Focus & states.** One focus owner per window; `:focus`, `:focus-visible`
(only after keyboard navigation, as GTK), `:focus-within` derived (M2),
`:active` while pressed, `:hover` from enter/leave, `:backdrop` from
`activated`. Popups form a focus stack.

**Selection.** `wl_data_device` copy/paste (`text/plain;charset=utf-8`) and
`zwp_primary_selection` for entries. DnD is M6.

**Tests.** Unit: key translation with a vendored `us` keymap; focus-ring
order; hit-testing with overlaps. E2E: a toplevel maps under the harness
with SSD; `configure` resize relayouts; a virtual keyboard types into an
entry and screencopy shows the text; Tab moves the focus ring; a popup opened
from a menubutton receives the grab.

## Section 4 — Reactive framework (P4)

**Types.** `View { kind: Kind, key: Option<Key>, props: Props, children:
Vec<View>, handlers: Handlers<Msg> }` from typed builders
(`button().label("Ok").on_click(Msg::Ok)`, `entry().text(&m.name)
.on_change(Msg::Name)`, `list(items.map(|i| row(i).key(i.id)))`). `Kind` =
one variant per widget; `Props` = typed map (strings, bools, numbers,
`IconRef`, CSS classes/id, `visible`, `sensitive`, alignment/expand/margins
→ node classes + inline styles); `Handlers` = per-event message producers
(`on_click`, `on_change(String)`, `on_toggle(bool)`, `on_activate`,
`on_selected(usize)`, …).

**Reconciler.** `reconcile(parent, prev, next)`: keyed LCS with moves — same
key+kind → diff props (`set_prop` only on change → restyle via generations);
new → build `Node` + controller; missing → remove (controllers drop;
animations finish per `fill-mode`); reordered → `move_child`; unkeyed children
match positionally by kind. Identity survives: animations, focus, caches,
tree ids, attached popups.

**Controllers.** One per `Kind`, owning behaviour state the model shouldn't:
press state, entry cursor/selection/undo, scroll offset, expander animation,
dropdown open state, spin repeat timer. They emit `Msg`s; the app's
`update(&mut Model, Msg) -> Cmd` folds them; `view` is pure. `Cmd` covers
timers (`Cmd::after`), clipboard, popup open/close.

**Loop.** `App::run(surface, model, update, view)`: event → controllers →
`Msg` → `update` → `view` → reconcile → restyle/layout/paint (dirty nodes
only; `AnimationState` ticks feed frames). Messages are queued, never nested.

**Tests.** Reconciler property tests (random keyed edits preserve identity
where keys match; op minimality); controller unit tests on a `ManualClock`;
an offscreen "counter app" driven by synthetic events with pixel assertions;
mutation checks on the diff paths.

## Section 5 — Widget set (P5 display/buttons/entries · P6 containers/lists/popovers/windows)

**Scope rule.** Every widget in GTK 4.22's gallery except deprecated or
service-bound ones. **Excluded:** `ComboBox`, `ComboBoxText`, `TreeView`,
`IconView`, `Assistant`, `VolumeButton`, `AppChooserButton`,
`AppChooserDialog`, legacy `Dialog`, `MessageDialog`, `ColorChooserDialog`,
`FontChooserDialog`, `FileChooserDialog` (deprecated); `GLArea`, `Video`,
`MediaControls` (GL/gstreamer); `LockButton` (polkit);
`PageSetupUnixDialog`, `PrintUnixDialog` (CUPS); `EmojiChooser` (M6 with IM).

**In scope (~48):**
- Display: Label (wrap/ellipsize; markup subset `<b><i><span>`), Spinner,
  Statusbar, LevelBar, ProgressBar (+pulse), InfoBar, Scrollbar, Image,
  Picture, Separator, TextView (cursor/selection/wrap/undo; plain text),
  Scale (marks, fill level), DrawingArea (app paint callback on a skia
  canvas), WindowControls, PopoverMenuBar, Calendar, PopoverMenu.
- Buttons: Button, ToggleButton, LinkButton, CheckButton (+radio groups),
  MenuButton, Switch, DropDown (list model + popover + search),
  ColorDialogButton + ColorDialog, FontDialogButton + FontDialog.
- Entries: Entry (placeholder, icons, progress, undo, selection),
  SearchEntry, PasswordEntry (peek), SpinButton (repeat, wrap, digits),
  EditableLabel.
- Containers: Box, Grid (spans, `border-spacing`), CenterBox, ScrolledWindow
  (overlay scrollbars, overshoot, kinetic), Paned, Frame, Expander,
  SearchBar, ActionBar, HeaderBar (title widget, packs, WindowControls per
  `gtk-decoration-layout`), Notebook (tabs, scrolling, reorder),
  ListBox/FlowBox (selection modes, activatable rows),
  ListView/GridView/ColumnView (recycling over a `ListModel`, selection,
  sortable columns), Overlay, Stack + StackSwitcher + StackSidebar
  (transitions via M2 animation), Popover (P3 popups; modal + non-modal).
- Windows: Window, ShortcutsWindow, AboutDialog (a `Window` preset),
  `ColorDialog`/`FontDialog`/`AlertDialog`.

Each widget: GTK-exact CSS node tree (name, subnodes — `button > label`,
`entry > text`, `checkbutton > check`, `scrolledwindow > scrollbar.horizontal >
range > trough > slider` …), style classes and pseudo-states, a `Kind` +
builder + controller, GTK keyboard/pointer behaviour, and an Adwaita pixel
test at rest plus one interaction state.

## Section 6 — Icon & cursor theming (P7)

**Lookup.** freedesktop Icon Theme Spec: `index.theme` (directories, sizes,
scales, `Type=Fixed|Scalable|Threshold`, `Context`, `Inherits` → `hicolor`),
search path `$XDG_DATA_HOME/icons`, `~/.icons`, `$XDG_DATA_DIRS/icons`,
`/usr/share/pixmaps`; theme name from `settings.ini` `gtk-icon-theme-name`
(default `Adwaita`); `(name, size, scale, symbolic)` lookup with the spec's
closest-size rule and fallback chain (`-symbolic` → regular →
`image-missing`); cache by `(theme, name, size, scale)`.

**Rendering.** SVG via `skia-rs-svg` (decode failure → `image-missing`,
logged once), PNG via `skia-rs-codec`; symbolic recolouring per GTK: the
SVG's `.success/.warning/.error/default` fills mapped from `-gtk-icon-palette`
(default = the node's `color`) at render time; `-gtk-icon-size`,
`-gtk-icon-transform`, `-gtk-icon-shadow`, `-gtk-icon-filter`,
`-gtk-icon-style`; HiDPI via `-gtk-scaled()` + output scale. `IconRef` (M2)
resolves here; `image`/`picture` and `-gtk-icon-source` share one
`paint_icon`.

**Builtins.** `-gtk-icon-source: builtin` shapes (check, radio,
indeterminate, arrows, expander, spin +/−) as skia paths matching GTK's
`gtkcssimagebuiltin` geometry.

**Cursors.** Client-side cursor themes are out of scope: the toolkit maps the
`cursor` property to `wp_cursor_shape_v1` names (icedtea supports it).

**Tests.** Vendored mini icon theme (index.theme, two dirs, one symbolic +
one regular SVG, a PNG) for hermetic lookup/fallback/scale tests; a recolour
pixel test (symbolic icon under `color: #3584e4`); the installed Adwaita icon
theme exercised only when present.

## Section 7 — Testing strategy, gates, out of scope

**The M3 gate (P8).** `gallery` binary (`cargo run -p icedtea-ui --bin
gallery`) rendering every in-scope widget at rest on a scrollable page under
the harness compositor with Adwaita light/dark/hc; `ui/tests/gallery_gate.rs`
screencopies each widget's derivable probe points per theme — a widget missing
from the gallery fails. **Interaction gate:** virtual pointer/keyboard drive
one interaction per widget class (click, toggle, type, open a dropdown and
pick, drag a scale, scroll a list, Tab through a form) with screencopy/model
assertions.

**Per-part gates.** P1: crate tests, coverage audit, clippy/doc `-D warnings`,
dry-run, publish consent. P2: `compositor/tests/popups.rs` ×3 deterministic.
P3: hermetic keymap/focus/hit-test units + e2e. P4: reconciler property tests
+ counter app. P5/P6: per widget one pixel test at rest + one state, and a
node-tree conformance test (`node_tree_of(widget)` vs fixtures vendored from
GTK's "CSS nodes" docs). P7: vendored mini icon theme.

**Cross-cutting.** Every load-bearing test records a mutation check; timing
guards only as generous complexity bounds; untrusted input (themes, icon
files, keymaps, popup positioners) never panics; `cargo test --workspace`,
clippy `-D warnings` (default + `--no-default-features`), `cargo fmt --all
--check`.

**Out of scope.** IME/`text-input-v3` and emoji (M6), DnD (M6), `accesskit`
(M6), markup beyond `<b><i><span>`, GL/video widgets, portals (file/print/app
choosers), client-side cursor themes, printing, XWayland clients.

**Execution.** Contract + eight part-plans, parts sequential P1 → P8, per-task
reviews, whole-part reviews and fix waves, whole-milestone review; P1's
publish and the final merge are consent stops.

## Risks

- **Scope** — the largest milestone (~48 widgets + popups + windows +
  framework + icons); mitigated by the eight-part decomposition and the
  gallery gate that measures completeness.
- **Popup grab semantics across two repos** — mitigated by P2's e2e in the
  harness before any toolkit code depends on it.
- **TextView/Entry editing correctness** — plain text only; undo and
  selection tested with a `ManualClock` and virtual keyboard.
- **Reconciler bugs** — property-tested; identity preservation is asserted
  through animations/focus surviving edits.
- **Icon theme variance** — hermetic mini theme for tests; real themes only
  opportunistically.

## Open items for M5

App migration order stays settings → shell → clipboard; `DrawingArea` exists
for the settings displays page; the shell's tray/status widgets may need
`StatusNotifierItem` (D-Bus) glue, decided in M5's spec.
