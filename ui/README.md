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

- `ICEDTEA_UI_THEME` — `bundled`, or a path to a `gtk.css`. Default: the
  user's installed GTK4 theme (`$XDG_CONFIG_HOME/gtk-4.0/gtk.css`, then
  `/usr/share/themes/$GTK_THEME/gtk-4.0/gtk.css`, then
  `/usr/share/themes/Adwaita/gtk-4.0/gtk.css`), falling back to the
  bundled copy.
- `ICEDTEA_UI_LABEL` — the button's label.
- `ICEDTEA_UI_CLASSES` — comma-separated style classes, e.g.
  `suggested-action`.

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
  harness compositor.

## Deliberately not covered by M1

More than one widget; full selector and `-gtk-*` property coverage;
box-shadows, radial gradients, `url()` images; GTK's `alpha()`/`shade()`/
`mix()` color functions and CSS relative color syntax; icons; animations
and transitions; accessibility; input methods; fractional scale; the app
framework. See the spec's M2–M6.

## Vendored files

`themes/adwaita-light.css` is GTK 4's default light theme, redistributed
under the LGPL-2.1-or-later. See [`themes/README.md`](themes/README.md).
