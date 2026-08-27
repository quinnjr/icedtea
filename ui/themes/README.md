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

Both are vendored for the same reason the light sheet is, and carry the same
LGPL-2.1-or-later terms: `tests/adwaita_coverage.rs` — the M2 gate — walks every
declaration of all three through the property registry, and it must assert
against a fixed, known theme rather than whatever GTK happens to be installed.
Neither sheet is a runtime default: `icedtea-ui` still loads the user's own
theme first and falls back to `adwaita-light.css`.

**License:** GTK is licensed under the GNU Lesser General Public License,
version 2.1 or later. This file is part of GTK and is redistributed here
under the LGPL-2.1-or-later, unmodified. See
<https://gitlab.gnome.org/GNOME/gtk/-/blob/main/COPYING>.

It is vendored so `icedtea-ui`'s tests are hermetic: they must assert
against a fixed, known theme rather than whatever GTK happens to be
installed. At runtime `icedtea-ui` prefers the user's own theme
(`$XDG_CONFIG_HOME/gtk-4.0/gtk.css`, then
`/usr/share/themes/<Name>/gtk-4.0/gtk.css`) and only falls back to this
copy.
