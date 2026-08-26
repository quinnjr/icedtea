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
