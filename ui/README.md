# `icedtea-ui`

A pure-Rust, GTK4-theme-compatible widget layer: no `gtk4`, `gio`, `glib`,
`pango`, `cairo` or `gdk`, and no Smithay. See
[the design spec](../docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md).

## What M1 covers

One themed `button`, end to end:

| Layer | Crate |
|---|---|
| Wayland + layer shell | `wayland-client`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
| 2D paint | `skia-rs-safe` (pure Rust) |
| Text | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` shaping (**not** `cosmic-text`) |
| Layout | `taffy` |
| CSS parse / match | Servo's `cssparser` + `selectors` |
| Widget | bespoke — `CssNode` wears GTK's node identity |

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

GTK4 does not have *a* stylesheet, it has a stack. `icedtea-ui` reproduces
the two layers that matter for a client, using the cascade's own source-order
rule so the later layer wins ties:

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
  `ComputedStyle` still paints a *uniform* border and takes the top side for
  all four -- M2 widens it to four sides.
- **Runner-ups are kept.** `cascade` returns `CascadedValues`: per longhand,
  every declaration that applied, sorted best-first. When the winner is a
  value this engine cannot interpret (Adwaita's
  `button.sidebar-button { border-radius: 100% }`), the next applicable
  declaration is used and the fallback is logged at debug. CSS proper would
  use the inherited or initial value here; falling back is a deliberate M1
  divergence, chosen because the property coverage is still narrow enough
  that reverting to an initial value loses more than it protects.
- **`color` and `font-size` inherit**, resolved by walking the node's
  ancestor chain (`ComputedStyle::resolve`, or `resolve_with_parent` when
  the caller already has the parent). `currentColor` resolves to the
  inherited colour on `color` itself and to the element's own computed
  colour everywhere else.
- **`@import` needs a base directory.** `parse_stylesheet_with_base(css,
  base_dir)` resolves relative imports recursively (depth <= 8, cycle-safe)
  and splices them in at the import site; `parse_stylesheet(css)` is the
  base-less wrapper and skips every import with a debug log, as does any
  `resource://` URL.
- **`background-clip`** is honoured (`border-box` default, `padding-box`,
  `content-box`). `background-origin` stays at its CSS default
  (padding-box), so a gradient is sized against the padding box however the
  clip is set.
- **Colour values are parsed from tokens**, ASCII-case-insensitively, in
  both CSS Color 3 comma syntax and CSS Color 4 space syntax with
  percentages and `/ <alpha>`. Hex literals are validated before slicing, so
  no theme input can panic the parser.
- **Negative lengths clamp to 0** for padding, border widths, radii and
  minimums.

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

## Deliberately not covered by M1

More than one widget; a real element tree (siblings, children, nth-index);
full selector and `-gtk-*` property coverage; per-side borders and
per-corner radii (the cascade carries the longhands, the computed style
reads the top side); box-shadows, radial gradients, `url()` images; GTK's
`alpha()`/`shade()`/`mix()` color functions and CSS relative color syntax;
RTL (`:dir()` matches a field nothing sets); icons; animations and
transitions; accessibility; input methods; fractional scale; surface
resize; the app framework. See the spec's M2–M6.

## Vendored files

`themes/adwaita-light.css` is GTK 4's default light theme, redistributed
under the LGPL-2.1-or-later. See [`themes/README.md`](themes/README.md).
