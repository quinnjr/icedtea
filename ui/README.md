# `icedtea-ui`

A pure-Rust, GTK4-theme-compatible widget layer: no `gtk4`, `gio`, `glib`,
`pango`, `cairo` or `gdk`, and no Smithay. See
[the design spec](../docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md).

## What this crate covers

M1 proved one themed `button` end to end. M2 widened the CSS layer to the whole
GTK 4.22 property table. **M3 turns that engine into a widget toolkit:** real
windows and popups, a keyboard/pointer/focus model, a reactive view layer, the
GTK 4.22 core widget set with GTK-exact CSS node trees, and icon theming.

| Layer | Crate |
|---|---|
| Wayland: toplevel, layer shell, popups | `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
| Keyboard | `xkbcommon` (keymaps, compose, repeat) |
| 2D paint | `skia-rs-safe` (pure Rust) |
| Text shaping | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` |
| Font discovery | `fontconfig` — real `fc-match` parity (default feature) |
| Layout | `taffy` |
| CSS parse / match | Servo's `cssparser` + `selectors` |
| Widgets | bespoke — `css::node::Node` trees wearing GTK's node identity |

### The module map

| Module | What it owns |
|---|---|
| `css/` | The property registry, parsing, selectors, cascade, computed values (M2) |
| `anim/` | Transitions, `@keyframes`, the animation clock (M2) |
| `layout.rs` | The `taffy` bridge: box, grid and centre containers, per-child alignment |
| `paint/` | Backgrounds, borders, shadows, outlines, blur, text, icons |
| `text.rs` | Font matching, shaping caches, wrap/ellipsize/caret/selection |
| `window/` | `Surface::{Toplevel, Layer, Popup}`, keyboard, pointer, focus, selection |
| `view/` | `View`, builders, the keyed reconciler, controllers, `App`, `Cmd` |
| `widgets/` | One file per widget: node tree, controller, behaviour |
| `icons/` | freedesktop icon themes, symbolic recolouring, builtins |
| `gallery.rs` | Every widget on one page — the binary the M3 gate measures |

The property registry (`css/registry.rs`) is the spine: one static table of
114 rows — 95 longhands, 19 shorthands — drives parsing, shorthand expansion,
inheritance, computed values and interpolation. Nothing outside that module
names a property string.

## The widget set

Every widget below is a `Kind`, a builder in `view::builders`, a controller, a
GTK-exact CSS node tree with a vendored fixture, a rest-state pixel probe and —
per widget class — one driven interaction. The name is what
`gallery --widget <NAME>` takes.

<!-- widgets:begin -->
| Widget | CSS node | Notes |
|---|---|---|
| `label` | `label` | wrap, ellipsize, `<b><i><span>` markup, links |
| `spinner` | `spinner` | `:checked` while spinning, as GTK does |
| `statusbar` | `statusbar` | one-line message area |
| `level_bar` | `levelbar` | discrete/continuous fill, offset colour bands |
| `progress_bar` | `progressbar` | determinate fill, optional inline text |
| `info_bar` | `infobar` | message type styling (`.info`/`.warning`/…), close button |
| `scrollbar` | `scrollbar` | orientation, slider drag |
| `image` | `image` | icon-name or file source, painted through `paint_icon_source` |
| `picture` | `picture` | raster content, `content-fit` scaling |
| `separator` | `separator` | horizontal/vertical rule |
| `text_view` | `textview` | multi-line editable text, caret and selection |
| `scale` | `scale` | continuous drag with a live value, keyboard step |
| `drawing_area` | `widget` | GTK sets no CSS name on it; caller-supplied paint callback |
| `window_controls` | `windowcontrols` | minimise/maximise/close glyphs |
| `calendar` | `calendar.view` | month grid, day selection |
| `popover` | `popover.background` | arrow, grab-and-dismiss surface (shared with `MenuButton`/`DropDown`) |
| `button` | `button` | click, `:hover`/`:active`, Space/Enter activation |
| `toggle_button` | `button.toggle` | persists `:checked` across clicks |
| `link_button` | `button.link` | opens a URI, `:visited` styling |
| `check_button` | `checkbutton` | tri-state check, optional radio grouping |
| `menu_button` | `menubutton` | opens a `PopoverC` on click |
| `switch` | `switch` | on/off drag or click, animated thumb |
| `drop_down` | `dropdown` | popover list, type-ahead filter, keyboard pick |
| `color_dialog_button` | `colorbutton` | swatch button, opens `ColorDialog` |
| `color_dialog` | `window.dialog` | HSV picker, hex entry |
| `font_dialog_button` | `fontbutton` | opens `FontDialog` |
| `font_dialog` | `window.dialog` | family/style/size list |
| `entry` | `entry` | caret, selection, undo stack (`widgets::edit`) |
| `search_entry` | `entry.search` | debounced search-changed signal |
| `password_entry` | `entry.password` | peek-to-reveal toggle |
| `spin_button` | `spinbutton` | steppers repeat while held |
| `editable_label` | `editablelabel` | click-to-edit label/entry swap |
| `box` | `box` | linear layout, per-child alignment |
| `grid` | `grid` | row/column layout with spans |
| `center_box` | `box` | three-slot start/center/end layout |
| `scrolled_window` | `scrolledwindow` | kinetic scroll, overlay scrollbars |
| `paned` | `paned` | draggable divider between two panes |
| `frame` | `frame` | optional labelled border around one child |
| `expander` | `expander-widget` | disclosure triangle, animated reveal |
| `search_bar` | `searchbar` | reveals a `search_entry` on `/`-style trigger |
| `action_bar` | `actionbar` | start/center/end action row, focus-order gate |
| `header_bar` | `headerbar` | title, start/end widget packing |
| `notebook` | `notebook` | tabbed pages, keyboard tab switching |
| `notebook_tab` | `tab` | one `notebook` page's tab label (sub-kind) |
| `overlay` | `overlay` | stacked children, one main plus floating overlays |
| `stack` | `stack` | one visible page at a time, transition-driven swap |
| `stack_page` | `stackpage` | one `stack` page's metadata (sub-kind) |
| `stack_switcher` | `stackswitcher.stack-switcher` | button row bound to a `stack` |
| `stack_sidebar` | `stacksidebar.sidebar` | list-style page picker bound to a `stack` |
| `list_box` | `list` | selectable rows, keyboard navigation |
| `list_box_row` | `row` | one `list_box` row (sub-kind) |
| `flow_box` | `flowbox` | wrapping selectable grid of children |
| `flow_box_child` | `flowboxchild` | one `flow_box` cell (sub-kind) |
| `list_view` | `listview` | recycled rows, scroll with reuse |
| `grid_view` | `gridview` | recycled grid cells, scroll with reuse |
| `column_view` | `columnview` | multi-column recycled list |
| `column_view_column` | `button` | one `column_view` header (sub-kind) |
| `popover_menu` | `popover.background.menu` | menu-model-backed popover |
| `popover_menu_bar` | `menubar` | horizontal menu bar opening `popover_menu`s |
| `popover_menu_item` | `button.model` | one `popover_menu` row (sub-kind) |
| `window` | `window.background` | toplevel surface, title, decoration |
| `shortcuts_window` | `window.shortcuts` | grouped accelerator reference (kept, ruling R1) |
| `about_dialog` | `window.aboutdialog` | app name/version/credits dialog |
| `alert_dialog` | `window.dialog.message` | message, detail, buttons |
<!-- widgets:end -->

## Running it

```bash
cargo run -p icedtea-ui --bin themed-button
```

Environment:

- `ICEDTEA_UI_THEME` — `bundled`, or a path to a `gtk.css`. A path is a
  **whole theme**, not an override: nothing is layered onto it. Default:
  the user's own theme stack, below.
- `ICEDTEA_UI_LABEL` — the button's label.
- `ICEDTEA_UI_CLASSES` — comma-separated style classes, e.g.
  `suggested-action`.

`--print-allocation` prints the border-box allocation the binary would map
(`width height label_x label_y`) and exits without touching Wayland; the
screencopy test derives its sample coordinates from it.

## The theme stack

GTK4 does not have *a* stylesheet, it has a stack. `icedtea-ui` reproduces the
two layers that matter for a client. The later layer is a higher cascade
**origin**, which outranks specificity — not a source-order tiebreak that only
settles ties. That is GTK's model: it loads the user's `gtk.css` at
`GTK_STYLE_PROVIDER_PRIORITY_USER` (800) over the theme's 200, and a
higher-priority provider wins *regardless* of specificity, so a user
`button { background-color: #f00 }` (specificity 0,0,1) beats Adwaita's
`button:hover` (0,1,1) rather than applying only in the base state.
`!important` stays the outermost key and origins are never reversed — unlike
CSS, which ranks the user origin *below* the author's for normal declarations
and flips origin order for `!important`.

1. **Base theme (GTK priority 200).** `$GTK_THEME`, in GTK's `Name[:variant]`
   form. `:dark` selects `gtk-dark.css`, anything else `gtk.css`, searched in
   `~/.themes`, `$XDG_DATA_HOME/themes`, `~/.local/share/themes` and
   `/usr/share/themes`, each under `<Name>/gtk-4.0/`; a `:dark` request falls
   back to the theme's `gtk.css` if it has no dark variant. With `$GTK_THEME`
   unset — or nothing found — the base is the **vendored Adwaita**, which is
   the right answer anyway: GTK4 carries Adwaita internally rather than on
   disk.
2. **User override (GTK priority 800).** `$XDG_CONFIG_HOME/gtk-4.0/gtk.css`,
   or `$HOME/.config/gtk-4.0/gtk.css` when `XDG_CONFIG_HOME` is unset **or
   empty**. Its rules are appended after the theme's, so a file that only
   restyles `headerbar` leaves the rest of the theme intact.

Both layers are parsed with their own directory as the `@import` base, and
both are named in an `info!` line.

**Caveat, unverified.** GTK's real lookup walks providers in priority order and
takes the first match, which may rank a *normal* declaration from the user
provider (800) above an `!important` one from the theme (200) — the opposite of
what the model above does, since `important` is the outermost key here. No test
covers that combination; if a theme's `!important` rule appears to be losing to
a user override, this is the difference.

With `$GTK_THEME` unset and nothing found on disk, the vendored fallback is
chosen by the `MediaEnv` the sheet compiles under, not fixed to the light
sheet: high contrast takes `themes/adwaita-hc.css`, a dark colour scheme takes
`themes/adwaita-dark.css`, and everything else `themes/adwaita-light.css`
(`app::bundled_sheet_for`; the three are `BUNDLED_ADWAITA_HC`,
`BUNDLED_ADWAITA_DARK` and `BUNDLED_ADWAITA_LIGHT` at the crate root). High
contrast wins over the colour scheme, because GTK ships high contrast as its
own theme and the vendored copy is the light one — which is what `Adwaita:hc`
and `HighContrast` both name. `HighContrastInverse` has no vendored copy and
falls to the plain dark sheet, the closer of the two.

## CSS engine behaviour

Notes that are load-bearing for anyone reading computed values:

- **Values are token streams, not source slices.** Every declaration value
  is serialized from its tokens, so comments are gone and whitespace is
  normalized before any consumer sees it. `!important` is found with
  cssparser's own `parse_important`.
- **Shorthands are expanded at cascade time**, carrying the shorthand's own
  cascade key, and the cascade is keyed by longhand. `padding` becomes
  `padding-top/right/bottom/left`; `border`/`border-<side>` become the
  per-side `-width`/`-style`/`-color` longhands (resetting what they omit to
  `medium`/`none`/`currentColor`, so `border: none` zeroes an earlier
  width); `background` becomes `background-color` + `background-image`.
  Per-side borders and per-corner radii are first-class: `ComputedStyle`
  carries all four sides and all four corners, and the painter fills each
  side as a path between the outer and inner rounded rects, so mixed widths
  and colours join the way GTK's do.
- `background` is a full layer list. Every comma-separated layer carries its
  own image, position, size, repeat, origin, clip and blend mode; layers
  paint top-first, as CSS requires, and the colour comes from the last
  layer.
- **Invalid at computed-value time follows CSS.** `cascade` still returns
  `CascadedValues` — per longhand, every declaration that applied, sorted
  best-first — but the runner-ups are now diagnostics only. When the winning
  declaration cannot be interpreted (an unknown `@name`, a colour cycle, a
  percentage with no basis, a non-finite `calc()`), the property takes the
  inherited value if it is an inherited property and its initial value
  otherwise. M1 fell back to the runner-up; that divergence is closed.
- **`color` and `font-size` inherit**, resolved by walking the node's
  ancestor chain (`ComputedStyle::resolve`, or `resolve_with_parent` when
  the caller already has the parent). `currentColor` resolves to the
  inherited colour on `color` itself and to the element's own computed
  colour everywhere else.
- **`@import` needs a base directory.** `parse_stylesheet_with_base(css,
  base_dir)` resolves relative imports recursively (depth <= 8, cycle-safe)
  and splices them in at the import site; `parse_stylesheet(css)` is the
  base-less wrapper and skips every import with a debug log, as does any
  `resource://` URL. Media conditions on an import are **ignored**, so a
  conditional `@import ... (min-width: 100px)` applies unconditionally.
- **`background-clip`** is honoured (`border-box` default, `padding-box`,
  `content-box`). `background-origin` stays at its CSS default
  (padding-box), so a gradient is sized against the padding box however the
  clip is set.
- **All 37 `@define-color`s resolve.** The colour table is built unresolved and
  resolved lazily with a depth guard, so a definition may reference a name
  defined later and a cycle terminates instead of hanging. GTK's legacy
  `alpha()`/`shade()`/`mix()`/`lighter()`/`darker()` and CSS Color 5 relative
  syntax (`hsl(from … calc(s * 1.8) …)`) are all understood; M1 resolved 29 of
  37 and recorded the rest as absent.
- **`@media` blocks are parsed once and evaluated per environment.**
  `prefers-color-scheme` and `prefers-contrast` are modelled; an unknown feature
  parses and never matches; `prefers-reduced-motion` parses and always evaluates
  false. One parse can therefore be compiled under several environments, which
  is how the coverage gate compiles light, dark and high-contrast.
- **Colour values are parsed from tokens**, ASCII-case-insensitively, in
  both CSS Color 3 comma syntax and CSS Color 4 space syntax with
  percentages and `/ <alpha>`. Hex literals are validated before slicing, so
  no theme input can panic the parser.
- **Negative lengths clamp to 0** for padding, border widths, radii and
  minimums.

## Reactive framework (`view/`)

`view(&Model) -> View<Msg>` describes the UI; the reconciler diffs it into
the retained node tree from `css::node`, keeping identity — animations,
focus, shaping caches — for every child whose key and kind survive.
Behaviour lives in a `Controller` per widget kind, not in the model:

```rust
fn view(model: &Model) -> View<Msg> {
    widget::<Msg>(Kind::Box)
        .child(widget(Kind::Button).prop(PropName::Label, "+").on_click(Msg::Inc))
        .child(widget(Kind::Label).prop(PropName::Label, model.n.to_string()))
}

fn update(model: &mut Model, msg: Msg) -> Cmd<Msg> {
    match msg { Msg::Inc => model.n += 1 }
    Cmd::None
}

App::new(Model { n: 0 }, update, view).run(window)?;
```

Messages are queued and folded one at a time — an `update` that produces a
`Cmd` producing a `Msg` enqueues it; the fold never re-enters. Commands run
where they can: the clipboard, timer and focus ones inside the fold, the
window-bound ones (`SetTitle`, `Minimize`, `ToggleMaximized`, `OpenPopup`,
`ClosePopup`) on the way back out of it, so a `Cmd` an `update` returns reaches
the toplevel just as a controller's does.

Events dispatch in GTK's three phases, capture → target → bubble, and a
controller that sets `cx.handled` consumes the event: it is the last node the
event reaches, whichever phase it was in. Focus moves — from a click, from
`Cmd::Focus`, from a keyboard binding — are announced once as `FocusOut` on the
old node then `FocusIn` on the new one, which is what `on_focus_in`/
`on_focus_out` fire on. Whether a node can take focus at all comes from
`Kind::is_focusable_by_default`, overridable per view with `.focusable(bool)`;
it reaches `window::focus` as the `focusable` class, which is what the Tab ring
walks.


`App::run_offscreen` runs the identical loop against a raster surface with no
compositor, driven by a `Vec<ScriptStep<Msg>>` on a `ManualClock`; that is
what `ui/tests/counter_app.rs` and every controller unit test use.

Widget builders (`button("Ok")`, `label("Hi")`, …) arrive with P5 and P6;
`view::builders`' module docs carry the naming rule they follow.

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

## Fonts and text

- **Font discovery is real fontconfig.** `text::FontDatabase` builds one
  `FcPattern` per query carrying the whole `font-family` list in priority order
  plus `FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/`FC_PIXEL_SIZE`, and calls
  `FcFontMatch` — so generic families, user aliases and everything in
  `~/.config/fontconfig` resolve exactly as `fc-match` resolves them. The tests
  assert that `sans-serif` resolves to *something*, never to a fixed family, and
  compare against the `fc-match` binary where it is installed.
- **The CSS and fontconfig scales are not the same scale.** `font-weight` 700 is
  `FC_WEIGHT` 200 and 400 is 80; `font-stretch: 87.5%` is `FC_WIDTH` 87. The
  conversions are table lookups with piecewise-linear interpolation
  (`css_weight_to_fc`, `css_stretch_to_fc`, `css_style_to_fc_slant`), not casts.
- **`fontconfig` is a cargo feature, on by default.**
  `default = ["fontconfig"]`; building with `--no-default-features` does not
  link `libfontconfig` at all, and `FontDatabase::new` *is*
  `FontDatabase::probe_only`. That is what a stripped container or a build
  that must not take the C dependency wants. Both configurations are gated:
  `cargo clippy -p icedtea-ui --no-default-features --all-targets` runs in CI
  alongside the default one.
- **`FontDatabase::probe_only()` is the fallback**, and the whole of what M1
  had: the fixed `FONT_CANDIDATES` path list. It is used when the feature is
  off, and when it is on but `FcInit` fails (a machine with no fontconfig), so
  the UI always has a face.
- **Three caches, dropped together by `clear_caches`:** query → `FontFace`,
  `(path, index)` → `Typeface`, and `ShapeKey` → `ShapedText`. A shaping key
  covers the text, face, size, letter-spacing, feature and variation settings
  and `text-transform` — every input that can change the run.
- **`text-transform` runs before shaping**, as it does in GTK, including
  `full-width` and `full-size-kana`. `letter-spacing` is added after every
  glyph (CSS 2.1's rule, so the trailing step is part of the measured width) via
  `TextBlobBuilder::add_positioned_run`.
- **Known limits of this shaper stack**, warned once at runtime rather than
  silently dropped: `font-feature-settings` and `font-variation-settings` cannot
  reach `rustybuzz` through `skia-rs-text` 0.4.0 (its `Shaper` passes an empty
  feature slice and exposes no variation axes), and a face index inside a font
  collection cannot be selected (`Typeface::from_data` takes no index). Both
  values still parse, compute and enter the shaping key, so the day the shaper
  grows the API the cache is already keyed correctly.

## Layout, paint and Wayland behaviour

- **`min-width`/`min-height` are content-box minimums**, as they are in GTK
  (and unlike CSS's own `min-width` on a border-box element):

  ```text
  content    = max(intrinsic, min)
  border box = content + padding + border
  ```

  Adwaita's "Click me" is therefore 78x34 (`max(17, 24) + 4 + 4 + 1 + 1`
  high) and an empty Adwaita button 36x34 (`max(0, 16) + 9 + 9 + 1 + 1`
  wide) — the sizes GTK 4.22 allocates. `ButtonLayout` keeps one `taffy`
  tree and overwrites its styles per restyle.
- **Labels are shaped once per `(text, font-size)`** and cached on the
  widget with their metrics; `paint_button` takes the shaped label rather
  than reshaping it per frame.
- **The surface covers the widget's ink rect, not its border box.** A shadow,
  an outline or a blur reaches outside the border box, and a buffer sized to
  the border box clipped it away. `LayerWindow` sizes the buffer to
  `Button::ink_rect` and shifts the widget tree by the negation of that rect's
  origin (`render_origin`), so an ink rect starting left of or above the border
  box still lands inside the surface. `themed-button --print-allocation` still
  reports the *border-box* allocation, which is what the screencopy test's
  sample coordinates are derived from.
- **The surface is double-buffered.** A `wl_buffer` belongs to the
  compositor from the commit that attaches it until `wl_buffer.release`, so
  the pool starts with two buffers, hands out only free ones, grows to three
  if both are busy, and defers a frame rather than painting into a buffer in
  use. Buffers, their `wl_shm_pool`s and their memfds are destroyed on drop,
  as are the `wl_pointer`, the layer surface and the `wl_surface`.
- **The configure wait is bounded**: 5 s (`wayland::CONFIGURE_TIMEOUT`),
  via `prepare_read` + `poll(2)` on the queue's fd, returning
  `LayerWindowError::Timeout`. A `closed` event ends it immediately with
  `LayerWindowError::Closed`.
- **Pointer state follows GTK.** Only `BTN_LEFT` presses the widget; the
  press (`held`) is tracked separately from `:hover`, so dragging off the
  button drops the paint but not the press and dragging back on re-arms
  `:active`; a release anywhere ends it. `wl_seat.capabilities` is treated
  as the seat's whole current set: losing the pointer releases it and clears
  hover and active, regaining it binds a new one.

## The gallery

```bash
cargo run -p icedtea-ui --bin gallery                 # every widget, light Adwaita
cargo run -p icedtea-ui --bin gallery -- --theme dark
cargo run -p icedtea-ui --bin gallery -- --widget check_button
```

One scrollable page, one `frame` per widget, in `Kind::all()` order. The page is
built by *iterating* `Kind::all()`, so a widget added without a gallery entry
does not compile — the gallery is the toolkit's completeness measure, not a
demo.

| Option | Meaning |
|---|---|
| `--theme <light\|dark\|hc>` | Which bundled sheet to compile. Default: light. |
| `--theme-file <PATH>` | A sheet on disk, used *whole*, instead of a bundled one. |
| `--widget <NAME>` | Render exactly one widget, alone, at the origin. |
| `--list` | Print every widget name, one per line, and exit. |
| `--probe-points` | Print `<widget> <label> <x> <y>` for every probe point and exit. |
| `--print-allocation` | Print `<widget> <x> <y> <width> <height>` per entry and exit. |
| `--size <WxH>` | Surface size. Default: 1280x800. |
| `--scroll <PX>` | Scroll the page before the first frame. |
| `--scale <N>` | Output scale, for HiDPI probes. Default: 1. |

`--list`, `--probe-points` and `--print-allocation` never touch Wayland: they
run the same view → reconcile → restyle → layout pipeline the app loop runs and
print what it produced. Everything else maps a `zwlr_layer_shell_v1` overlay
anchored top-left, so on-screen coordinates are page coordinates.

**Probe points** are derived, never declared: `"root"` is the widget's own node,
every descendant is labelled by its CSS node name, and a name that occurs more
than once is indexed (`tab0`, `image1`). That is how the gates learn where to
sample — no test in this crate hard-codes a coordinate.

Every folded message is printed as `msg <line>` on stdout and flushed
immediately, which is how the interaction gate asserts on the model from
outside the process.

## The M3 gates

```bash
cargo test -p icedtea-ui --test gallery_gate       # rest state, 3 themes
cargo test -p icedtea-ui --test interaction_gate   # 16 interactions, 15 driven
cargo test -p icedtea-ui --test node_trees         # GTK node-tree conformance
```

- `tests/gallery_gate.rs` walks the page in surface-height slices under the
  harness compositor and asserts every widget paints something in light, dark
  and high-contrast Adwaita — except the nine entries listed in that file's
  `KNOWN_BLANK_AT_REST`, which are measured, not asserted on, because they
  render nothing today (seven collapse to a zero-area allocation, and
  `link_button`/`check_button` draw nothing into a real one; see contract
  amendment P8-D69). The list was fifteen until the M3 close-out's first fix
  wave, which made the universal size request reach every kind (P8-D75) and
  gave a recycling view's pooled rows a measure and a paint, so
  `progress_bar`, `stack_switcher`, `stack_sidebar`, `list_view`, `grid_view`
  and `about_dialog` are now asserted on like everything else. Every entry,
  exempt or not, must still appear whole in some slice. It further asserts that every `Kind` appears in `--list`, on the
  page and (for sub-kinds) inside its parent's node tree; that at least one
  probe point per widget differs between light and dark; that every widget's
  node tree matches its vendored GTK 4.22 fixture; and that this README's
  table lists every `Kind`.
- `tests/interaction_gate.rs` drives interactions with a virtual pointer and
  keyboard. **All sixteen of the contract's interactions ship; fifteen run**:
  click, toggle, check,
  switch, the pointer-click-without-focus-ring rule, typing with a placed
  caret, debounced search, password peek, held spin repeat, scale drag,
  drop-down pick, expander disclosure, stack-page switch, menu-button popover,
  and Tab in geometric order. Each of the last ten landed with the
  pre-existing production defect it was RED on — none is asserted around.
  The sixteenth, `scrolling_a_list_view_recycles_rows_without_losing_
  selection`, shipped `#[ignore]`d in M3 for a transport gap below this crate
  — `wlr` 0.20.28 forwarded no `wl_pointer.axis` to any client — and runs
  since the 0.20.29 bump; the same interaction is also proven offscreen by
  `widgets::list_view::tests::pixels::scrolling_recycles_the_pooled_rows_and_
  keeps_the_selection`. Contract amendments P8-D71, P8-D72 and P8-D74 carry
  the whole trail.
- Colours are pinned in exactly one place — `tests/themed_button_offscreen.rs`,
  the M1 gate. The gallery gates assert *change*, not constants: 64 pinned
  colours would be a fixture to maintain, not a gate.

## Tests

```bash
cargo test -p icedtea-ui
```

- `tests/themed_button_offscreen.rs` is the **load-bearing gate**: computed
  values equal Adwaita's resolved values, the padding-gutter column (x=4) on
  the centre row equals the color the theme declares (the centred label
  covers the geometric centre pixel, so the gutter column is sampled
  instead), a pixel outside `border-radius: 5px` is transparent, and toggling
  `:hover`/`:active` changes both. No compositor needed.
- `tests/layer_shell_screencopy.rs` proves the Wayland seam against the
  harness compositor: the button on screen, `:hover` driven by a real
  pointer, and the press/drag-off/drag-back cycle. It derives its geometry
  from `themed-button --print-allocation` rather than hardcoding it.
- `tests/support/mod.rs` holds the shared child reaper, the hermetic
  `themed-button` command and the allocation probe.
- `tests/adwaita_coverage.rs` is the **M2 gate**: GTK 4.22's Adwaita light, dark
  and high-contrast, walked declaration by declaration — including the ones
  inside `@keyframes` and `@media` — through the property registry, asserting
  **0 unknown properties, 0 unparseable declarations and 37/37 `@define-color`s
  resolved** on each sheet. It names what it cannot read rather than counting,
  and carries negative controls so that a green run means something.
- `tests/gtk4_property_reference.rs` pins the registry against a vendored
  fixture of the GTK 4.22 property table: 114 rows of name, longhand/shorthand
  kind and inherited flag, asserted both ways — nothing missing, nothing
  invented — in registry order.

## Deliberately not covered by M3

Input methods and `text-input-v3`, the emoji chooser, drag and drop, and
`accesskit` accessibility (M6). Markup beyond `<b><i><span>`. GL and video
widgets (`GLArea`, `Video`, `MediaControls`), the portal-backed choosers
(file, print, app) and `LockButton`, and the deprecated widgets GTK 4.22 itself
retired — except `Statusbar`, `InfoBar` and `ShortcutsWindow`, which Adwaita
still styles and which the M3 contract keeps (ruling R1). Client-side cursor
themes: the toolkit maps the `cursor` property to `wp_cursor_shape_v1` names
instead. XWayland clients. Fractional scale. The app migrations onto this
toolkit — settings, then shell, then clipboard — are M5.

## Vendored files

`themes/adwaita-light.css`, `themes/adwaita-dark.css` and
`themes/adwaita-hc.css` are GTK 4's default light, dark and high-contrast
themes, redistributed under the LGPL-2.1-or-later. See
[`themes/README.md`](themes/README.md). `tests/fixtures/gtk4.22-css-properties.txt`
is a transcription of GTK 4.22's CSS property reference, with the doc URL in its
header.

## The window and event layer

`icedtea_ui::window` is one Wayland client per window: a `Surface` in one of
three roles (`xdg_toplevel`, `zwlr_layer_surface_v1`, `xdg_popup`), a retained
`Node` tree with its `LayoutTree`, `AnimationState` and `StyleMap`, and a
bounded pump that hands up a flat `InputEvent` stream.

- **Keyboard** — `window::keyboard` compiles the compositor's keymap with
  `libxkbcommon`, runs the compose table, reports the modifiers a keysym
  consumed (so `!` matches a plain-`!` accelerator), and owns the repeat timer
  xkbcommon does not have.
- **Pointer** — `window::pointer` hit-tests the retained tree in reverse paint
  order, mirrors the compositor's implicit grab client-side, and coasts finger
  scrolls kinetically.
- **Focus** — `window::focus` sorts candidates *geometrically*, as GTK's
  `gtk_widget_focus_sort` does, and implements GTK's `:focus-visible` rule:
  visible by default, hidden by a pointer click, shown again by a key that
  moved the focus.
- **Selection** — `window::selection` offers and reads
  `text/plain;charset=utf-8` on `wl_data_device` and the primary selection.

M1's `LayerWindow` is unchanged, at `window::layer` and still reachable as
`wayland::LayerWindow`; the `themed-button` demo and its pixel gate run on it
exactly as before.

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

Not covered here: input methods (`text-input-v3`), drag and drop, client-side
cursor themes (M6/see below).

## The P5 half of the widget catalogue, in detail

`icedtea_ui::widgets` builds one `Controller` per `Kind`, dispatched by
`widgets::build_controller`; `view::builders` re-exports each widget's free
constructor and `*Ext` setter trait under the name §4.4 requires. Behaviour
(steppers repeating while held, a search entry's debounce, an entry's caret
and selection, a drop-down's type-ahead filter) lives on the controller, not
duplicated per widget: `Entry`, `SearchEntry`, `PasswordEntry` and
`SpinButton` all share one `TextEditState`/`UndoStack` engine
(`widgets::edit`); `MenuButton`, `DropDown` and the two dialog buttons all
embed the same `PopoverC` surface.

Every kind below is checked two ways: `tests/node_trees.rs` renders its
retained `Node` subtree in GTK's own notation and matches it, node by node
and class by class, against the fixture vendored verbatim from GTK 4.22.4's
sources (`tests/fixtures/gtk4.22-node-trees/<fixture>.txt`); `tests/
widget_pixels.rs` runs it through `App::run_offscreen` against Adwaita light
and asserts on the rasterized pixels for both its rest state and its
interactions. `every_p5_kind_has_a_fixture_and_matches_it_with_default_props`
and `no_p5_kind_falls_through_to_the_unimplemented_controller` in
`node_trees.rs`, and `every_p5_kind_renders_at_rest_without_panicking` in
`widget_pixels.rs`, are exhaustive gates over `Kind::all()`'s P5 half: adding
a kind without a fixture, without a dispatch arm, or whose default-props tree
panics fails a named test rather than passing silently. P6 extends both
files with its own 32 kinds; P8's gallery gate wires the whole set together.

| Kind | CSS node | Builder | Fixture |
| --- | --- | --- | --- |
| `Label` | `label` | `label(text)` | `label.txt` |
| `Spinner` | `spinner` | `spinner()` | `spinner.txt` |
| `Statusbar` | `statusbar` | `statusbar()` | `statusbar.txt` |
| `LevelBar` | `levelbar` | `level_bar()` | `level_bar.txt` |
| `ProgressBar` | `progressbar` | `progress_bar()` | `progress_bar.txt` |
| `InfoBar` | `infobar` | `info_bar()` | `info_bar.txt` |
| `Scrollbar` | `scrollbar` | `scrollbar()` | `scrollbar.txt` |
| `Image` | `image` | `image`/`image_named` | `image.txt` |
| `Picture` | `picture` | `picture`/`picture_from_bytes` | `picture.txt` |
| `Separator` | `separator` | `separator()` | `separator.txt` |
| `TextView` | `textview` | `text_view()` | `text_view.txt` |
| `Scale` | `scale` | `scale(lower, upper)` | `scale.txt` |
| `DrawingArea` | `widget` | `drawing_area()` | `drawing_area.txt` |
| `WindowControls` | `windowcontrols` | `window_controls()` | `window_controls.txt` |
| `Calendar` | `calendar` | `calendar()` | `calendar.txt` |
| `Popover` | `popover` | `popover(..)` | `popover.txt` |
| `Button` | `button` | `button`/`button_from` | `button.txt` |
| `ToggleButton` | `button.toggle` | `toggle_button()` | `toggle_button.txt` |
| `LinkButton` | `button.link` | `link_button()` | `link_button.txt` |
| `CheckButton` | `checkbutton` | `check_button()` | `check_button.txt` |
| `MenuButton` | `menubutton` | `menu_button(..)` | `menu_button.txt` |
| `Switch` | `switch` | `switch()` | `switch.txt` |
| `DropDown` | `dropdown` | `drop_down`/`drop_down_from` | `drop_down.txt` |
| `ColorDialogButton` | `colorbutton` | `color_dialog_button()` | `color_dialog_button.txt` |
| `ColorDialog` | `window.dialog` | `color_dialog()` | `color_dialog.txt` |
| `FontDialogButton` | `fontbutton` | `font_dialog_button()` | `font_dialog_button.txt` |
| `FontDialog` | `window.dialog` | `font_dialog()` | `font_dialog.txt` |
| `Entry` | `entry` | `entry()` | `entry.txt` |
| `SearchEntry` | `entry.search` | `search_entry()` | `search_entry.txt` |
| `PasswordEntry` | `entry.password` | `password_entry()` | `password_entry.txt` |
| `SpinButton` | `spinbutton` | `spin_button(lower, upper)` | `spin_button.txt` |
| `EditableLabel` | `editablelabel` | `editable_label(text)` | `editable_label.txt` |

This table only carries the constructor signature and fixture path for P5's
original 32 kinds; P6's remaining 32 (containers, lists, menus, dialogs) are
listed with their CSS node and one-line behaviour in **The widget set** above,
without a per-kind builder-signature row here.
