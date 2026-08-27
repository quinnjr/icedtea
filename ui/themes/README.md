# Vendored theme files

## `adwaita-light.css`

Verbatim extract of GTK 4's default light theme:

    gresource extract /usr/lib/libgtk-4.so.1 \
        /org/gtk/libgtk/theme/Default/Default-light.css

Extracted 2026-08-25 from GTK 4 as packaged on Arch Linux (1,941 lines,
37 `@define-color` declarations).

## `adwaita-dark.css`

Verbatim extract of GTK 4's default dark theme:

    gresource extract /usr/lib/libgtk-4.so.1 \
        /org/gtk/libgtk/theme/Default/Default-dark.css

Extracted 2026-08-26 from GTK 4.22.4 as packaged on Arch Linux (1,929 lines,
37 `@define-color` declarations).

## `adwaita-hc.css`

Verbatim extract of GTK 4's default high-contrast theme:

    gresource extract /usr/lib/libgtk-4.so.1 \
        /org/gtk/libgtk/theme/Default/Default-hc.css

Extracted 2026-08-26 from GTK 4.22.4 as packaged on Arch Linux (1,944 lines,
37 `@define-color` declarations).

## Why these are vendored

All three sheets are here so `icedtea-ui`'s tests are hermetic:
`tests/adwaita_coverage.rs` — the M2 gate — walks every declaration of all
three through the property registry, and it must assert against a fixed,
known theme rather than whatever GTK happens to be installed.

None of them is a runtime default. At runtime `icedtea-ui` prefers the
user's own theme (`$GTK_THEME`'s
`/usr/share/themes/<Name>/gtk-4.0/gtk.css`, with
`$XDG_CONFIG_HOME/gtk-4.0/gtk.css` layered over it) and only falls back to
`adwaita-light.css`.

## License

GTK is licensed under the GNU Lesser General Public License, version 2.1 or
later. All three files are part of GTK and are redistributed here under the
LGPL-2.1-or-later, unmodified. See
<https://gitlab.gnome.org/GNOME/gtk/-/blob/main/COPYING>.
