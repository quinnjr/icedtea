# icedtea

A **Wayland desktop environment** for Linux, written in Rust.

icedtea is a wlroots-based compositor together with the system services a
desktop needs — clipboard, notifications, and a GTK4 settings & multi-monitor
display app — built on [`wlr`](https://crates.io/crates/wlr), a safe Rust
wrapper over wlroots 0.20.

## Components

| Crate | What it is |
|-------|------------|
| `icedtea-compositor` | The Wayland compositor: window management, decorations, workspaces, XWayland, and the standard client-compat protocols. Serves `org.icedtea.Compositor` for control and window enumeration. |
| `icedtea-clipboard` | Headless clipboard-history daemon (`org.icedtea.Clipboard`). |
| `icedtea-notifications` | Notification daemon serving `org.freedesktop.Notifications` + an `org.icedtea.Notifications` query/DND surface. |
| `icedtea-settings` | GTK4 settings & multi-monitor display-manager app. |
| `icedtea-shell` | Shell surfaces (taskbar) driving the compositor over D-Bus. |
| `icedtea-contract` | Shared IPC vocabulary (D-Bus names, wire types) every component depends on. |
| `icedtea-config` | Configuration model. |
| `icedtea-harness` | Test harness that boots a headless compositor and drives it as a real Wayland/D-Bus client. |

## Status

Under active development. The window-management core is solid; the current
work builds out the desktop-environment layer — XWayland (X11 app support),
client-compatibility protocols, notifications, and system integration. See
`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md` for the roadmap.

## Building

```sh
cargo build --workspace
cargo test -p icedtea-compositor
```

The compositor requires system wlroots 0.20 (with XWayland headers for X11
support); the settings and shell apps require GTK4.

## License

See the workspace manifest.
