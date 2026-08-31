# `icedtea-ui`

A pure-Rust, GTK4-theme-compatible widget layer: no `gtk4`, `gio`, `glib`,
`pango`, `cairo` or `gdk`, and no Smithay. See
[the design spec](../docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md).

## What this crate covers

M1 proved one themed `button` end to end. M2 widened the CSS layer to the whole
GTK 4.22 property table: every property in GTK's CSS reference parses, cascades,
inherits, computes, paints and animates on a widget-independent styled-node
tree.

| Layer | Crate |
|---|---|
| Wayland + layer shell | `wayland-client`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
| 2D paint | `skia-rs-safe` (pure Rust) |
| Text shaping | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` (**not** `cosmic-text`) |
| Font discovery | `fontconfig` — real `fc-match` parity (default feature; see below) |
| Layout | `taffy` |
| CSS parse / match | Servo's `cssparser` + `selectors` |
| Widget | bespoke — a `css::node::Node` tree wearing GTK's node identity |

The property registry (`css/registry.rs`) is the spine: one static table of
114 rows — 95 longhands, 19 shorthands — drives parsing, shorthand expansion,
inheritance, computed values and interpolation. Nothing outside that module
names a property string.

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

## Deliberately not covered by M2

Icon *drawing* — every `-gtk-icon-*` value parses, computes and is stored, but
nothing rasterizes an icon yet (M4). Widgets beyond the button behaviour that
carries the M1 gate, the focus/event model, grid and centre layouts, and text
editing (M3). Accessibility, input methods, drag and drop (M6). `url()` images
beyond PNG, and SVG only where `skia-rs-svg` decodes it — anything else is
recorded unresolved and paints nothing rather than erroring. `@media` features
beyond `prefers-color-scheme` and `prefers-contrast`. Fractional scale and
surface resize. `font-feature-settings`/`font-variation-settings` reaching the
shaper, and font-collection face indices (see **Fonts and text** above). See the
spec's M3–M6.

## Vendored files

`themes/adwaita-light.css`, `themes/adwaita-dark.css` and
`themes/adwaita-hc.css` are GTK 4's default light, dark and high-contrast
themes, redistributed under the LGPL-2.1-or-later. See
[`themes/README.md`](themes/README.md). `tests/fixtures/gtk4.22-css-properties.txt`
is a transcription of GTK 4.22's CSS property reference, with the doc URL in its
header.

## M3 Part 3 — the window and event layer

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

Not here: widgets (P5/P6), the reactive loop (P4), icons (P7), IME, drag and
drop, client-side cursor themes.
