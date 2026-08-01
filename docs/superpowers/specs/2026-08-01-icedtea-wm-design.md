# icedtea-wm Design

Date: 2026-08-01

## Overview

icedtea-wm is a floating Wayland window manager built on Smithay (pure-Rust
compositor library) with a GTK4 shell that provides a Cinnamon/Windows 10
desktop feel: taskbar with grouped app buttons, start menu, system tray,
notifications, Alt+Tab switcher, workspace pager, and wallpaper. Configuration
is GUI-driven and stored in a redb embedded database. Control flows over a
single DBus control plane (`org.icedtea.WM`), and the whole session is composed
as systemd user units. Edge snapping (halves/quadrants) is the only "tiling"
behavior; there is no auto-tiling.

The name and original brief reference a "DWM" — the keyboard-driven ethos
carries over (shortcuts for common operations), but the interaction model is
floating-first with Win10-style snapping.

## Version constraints

- MSRV: Rust 1.94 (`rust-version = "1.94"` on every crate). All dependencies
  support it.
- Pinned majors: `smithay = "0.7"`, `zbus = "5"`, `redb = "3"`,
  `gtk4-layer-shell = "0.8"`, `calloop` via smithay.
- GTK3 layer-shell bindings (`gtk-layer-shell`) are unmaintained; GTK4 is the
  only path. No GTK3 fallback.

## Goals

- A usable floating WM with a polished Cinnamon/Win10-style shell.
- Config entirely managed through a GTK settings GUI, persisted in redb,
  applied live without restarting.
- Clean separation: compositor owns window state; shell is a pure view plus
  input source; they agree over a well-defined DBus contract.
- Robustness: shell crash never takes down the compositor; config corruption
  never prevents startup.
- First-class systemd integration: session lifecycle, crash recovery, and the
  session DBus bus are all managed by the user manager.

## Non-goals (out of scope for v1)

- Automatic tiling layouts (master/stack, splits).
- Snap-assist tile picker (the "fill the other half" overlay).
- Live window thumbnails in the Alt+Tab switcher.
- Manual (hand-edited) configuration files; the GUI is the only config writer.
- A CLI for configuration.
- Multi-session compositor or remote desktop.

## Architecture

Three runtime processes plus two support crates, all in one Cargo workspace.
The compositor, shell, and settings all attach to the session DBus bus, which
is managed by the systemd user manager. systemd also owns process lifecycle
(see Systemd integration).

```
┌──────────────────────────────────────────────────────────┐
│ compositor (Smithay)                                     │
│  ├─ window tree / workspaces / focus                     │
│  ├─ renderer: wallpaper, windows, SSD title bars,        │
│  │             edge-snap preview overlay                 │
│  ├─ input: keybindings, pointer drag/move, alt-tab hold  │
│  ├─ config: reads + hot-reloads redb                     │
│  ├─ hosts org.icedtea.WM DBus service ◄────┐             │
│  └─ systemd: graphical-session.target unit │             │
└────────────────────────────────────────────│─────────────┘
                                              │ session bus
┌────────────────────────────────────────────│─────────────┐
│ shell (GTK4, gtk4-layer-shell)             ▼             │
│  ├─ layer-surfaces: taskbar(+tray, pager), notifications,│
│  │                  alt-tab switcher                     │
│  ├─ start menu (popover) + settings launcher             │
│  └─ DBus client (gio) ── window/workspace commands       │
└──────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────┐
│ settings (GTK4)                                          │
│  ├─ writes config.redb, sends ReloadConfig over DBus     │
│  └─ launched from the start menu                         │
└──────────────────────────────────────────────────────────┘
```

Invariant: the compositor is the sole owner of window state. Every state
change flows compositor → DBus signal → shell, so the taskbar, pager, and
Alt+Tab can never disagree with reality.

## Crate layout

| Crate | Responsibility |
| --- | --- |
| `contract` | Shared types and DBus interface name/string constants. No runtime deps beyond zbus and serde. |
| `config` | Shared schema for configuration values, serialized into redb tables. |
| `compositor` | The Smithay binary; hosts `org.icedtea.WM`. |
| `shell` | The GTK4 shell binary; DBus client via gio. |
| `settings` | The GTK4 settings GUI binary. |

## DBus control plane

The session bus is the single control plane. Transport is the standard DBus
session bus (per-user, started by systemd); the interface is
`org.icedtea.WM`, owned by the compositor. There is no other IPC channel.

### Client connection model

- Clients (shell, settings, external tools) call `GetState()` to receive a
  full `Snapshot`, then subscribe to signals for deltas. The snapshot
  envelope carries a monotonically increasing sequence number so a client can
  detect missed events and re-sync.
- A client whose connection drops keeps its UI alive and re-watches the
  service name with backoff, then re-syncs via `GetState()`.

### Methods (clients → compositor)

- `FocusWindow(id)`, `CloseWindow(id)`
- `MinimizeWindow(id, toggle)`, `MaximizeWindow(id, toggle)`,
  `FullscreenWindow(id, toggle)`
- `SetWorkspace(id)`, `MoveWindowToWorkspace(id, workspace)`
- `GetState() → Snapshot`
- `ReloadConfig()` (sent by `settings` after a commit), `Quit()`

### Signals (compositor → clients)

- `WindowOpened(id, app_id, title, pid)`
- `WindowClosed(id)`
- `WindowUpdated(id, {title?, geometry?, state?, workspace?, focused?,
  minimized?})` — partial fields.
- `WorkspaceSet(id, active)` — a workspace became current.
- `WorkspaceList(workspaces)` — rename/add/remove.
- `AltTabState(active, entries, index)` — shell renders the switcher while the
  compositor holds the Alt+Tab key.
- `ConfigReloaded(appearance)` — theme colors, bar geometry, fonts.

### Compositor-side bridge

The compositor runs Smithay's calloop event loop; zbus runs its own async
runtime. The bridge is a small channel pair: an incoming channel (DBus method
calls → compositor commands, drained by a calloop source) and an outgoing
channel (compositor events → DBus signals, driven by the zbus runtime). No
runtime fusion is attempted; the two loops communicate only through channels.

### Deliberate carve-outs

- Drag-to-edge snap preview is compositor-drawn (translucent GL rectangles);
  it tracks the pointer at frame rate and never round-trips over DBus.
- Alt+Tab is compositor-driven (it owns the keyboard); the shell is a pure
  renderer of the switcher UI.
- The shell spawns apps it launches (start menu, taskbar pins) directly; app
  launching is not a control-plane concern.

## Compositor design

### Module layout

- `main.rs` — display and event-loop setup.
- `state.rs` — event-loop state.
- `backend.rs` — DRM + libinput seat; nested Wayland backend for tests.
- `window.rs` — window and workspace model.
- `layout.rs` — floating placement and snap-geometry computation; pure
  functions, heavily unit-tested.
- `decoration.rs` — SSD rendering and button hit-testing.
- `input.rs` — keybindings, pointer drag/move, Alt+Tab hold.
- `render.rs` — wallpaper, scene assembly, snap preview.
- `config.rs` — redb reads and live reload.
- `dbus.rs` — `org.icedtea.WM` service and the channel bridge into calloop.
- `logind.rs` — suspend inhibition for fullscreen.

### State model

```
Window { id, surface, app_id, title, pid, workspace,
         geometry, maximized, minimized, fullscreen }
Workspace { id, name, focused_window }
State { windows, workspaces, outputs, seat }
```

### Rendering

In order: wallpaper (image stretched/tiled or solid color from config) →
client surfaces with SSD title bars → transient overlays (snap preview).
HiDPI handled via output scale; per-output scene graphs.

### Decorations (hybrid)

Apps that request CSD via `xdg-decoration` skip SSD. Everything else gets a
compositor title bar with minimize/maximize/close buttons,
double-click-to-maximize, and drag-to-move on the bar. Button hit-testing
lives in `decoration.rs`; theme colors come from config appearance so title
bars match the shell.

### Input and focus

One focused window per workspace; clicks focus and (per `raise_on_focus`)
raise. Pointer drag on an SSD title bar moves a window. Alt+Tab held cycles
focus compositor-side; the shell renders the switcher; on release focus
commits.

### Edge snapping

Halves and quadrants with a configurable snap gap. Dragging a window to a screen
edge or corner shows a translucent preview and, on release, snaps. Keyboard
snapping (Super+arrows / Super+keypad). Snap remembers pre-snap geometry so
drag-back-to-restore works. `layout.rs` computes zones and restored geometry —
all pure and unit-tested. Edge snapping can be disabled in behavior config.

### Workspaces

N workspaces (count configurable, default 4), each with a name and its own
focused window. By default Super+1..N switches and Super+Ctrl+1..N moves the
focused window; both are rebindable in the settings GUI. Windows cannot exist
outside a workspace.

### Fullscreen

A focused fullscreen window hides the bar (config toggle) and suppresses SSD;
notification and tray overlays still float above it.

## Shell design

### Stack

GTK4 + `gtk4-layer-shell`, no libadwaita — plain GTK4 widgets driven by a
custom CSS theme built from config appearance.

### Threading

The shell uses glib's native DBus integration (`gio::DBusConnection` +
`DBusProxy`) on the glib main context, so compositor signals and GTK share one
thread. No mutex in the state model. Signals reduce into a state tree, then
widgets are invalidated.

### State model

```
ShellState { windows: HashMap<Id, Window>, workspaces, active_workspace,
             alt_tab: Option<AltTab> }
```

The reducer (snapshot → apply delta) is a pure function, unit-tested with
scripted DBus signal sequences.

### Surfaces

- Taskbar (bottom, exclusive zone): grouped-by-app_id buttons — icon plus
  count badge; click focuses the app's most-recently-used window; middle-click
  closes it; right-click context menu (close / move-to-workspace). Active app
  highlighted. System tray (StatusNotifier via zbus) at the right end.
  Workspace pager widget included.
- Start menu: GTK popover anchored to the taskbar — search box, pinned apps,
  full app list (parsed .desktop files), power menu (logout/reboot/shutdown
  via loginctl). The shell spawns launched apps directly. Includes a Settings
  entry that launches the `settings` binary.
- Notifications (top-right overlay surface): implemented over
  `org.freedesktop.Notifications` (zbus), click-to-focus the source window,
  auto-dismiss.
- Alt+Tab switcher (centered overlay surface): shown/hidden on `AltTabState`
  events. v1 shows app icons and titles, not live thumbnails (live grabs need
  a wlr-screencopy-style protocol; future work).

## Settings GUI

A regular GTK window launched from the start menu. Reads and writes
`$XDG_CONFIG_HOME/icedtea/config.redb`; after a commit it calls
`ReloadConfig()` on `org.icedtea.WM`, and the compositor re-reads the DB and
emits `ConfigReloaded` to the shell.

Widgets:

- Keybinding recorder: capture-a-key widget for each action.
- Color pickers for the palette (background, foreground, accent).
- Wallpaper file chooser with live preview (also solid-color option).
- Bar position, height, corner radius.
- Snap gap (padding around snapped windows).
- Workspace names list (add/remove/rename).
- Behavior toggles: raise-on-focus, hide-bar-on-fullscreen, edge snapping.
- "Reset to defaults" rewrites the tables to defaults, then reloads.

## Configuration (redb)

- DB file: `$XDG_CONFIG_HOME/icedtea/config.redb`.
- `config` crate owns the schema (shared serde types for values), serialized
  into redb tables:
  - `keybindings` — action → KeyCombo
  - `appearance` — bar position/height, corner radius, snap gap, palette,
    wallpaper path
  - `behavior` — raise-on-focus, hide-bar-on-fullscreen, edge-snapping toggle
  - `workspaces` — ordered workspace names
  - `meta` — schema version for future migrations
- On read failure or DB corruption, compositor falls back to baked-in defaults
  rather than crashing.
- Multi-process model: single writer (settings), single reader (compositor).
  The shell never opens the DB; it receives appearance over DBus.
- redb has no change-watch API, so the writer signals the compositor via
  `ReloadConfig()` after each commit.

## Systemd integration

The session is composed of systemd user units against `graphical-session.target`
— the standard pattern used by sway/Hyprland. The user manager owns process
lifecycle, so the DBus session bus is guaranteed present
(`dbus-broker.service`) and no `dbus-run-session` wrapper is needed.

### Units

- `icedtea-compositor.service` — runs the compositor; hosts `org.icedtea.WM`.
  `WantedBy=graphical-session.target`, `After=graphical-session-pre.target`,
  `Restart=on-failure`.
- `icedtea-shell.service` — runs the shell. `WantedBy=graphical-session.target`,
  `After=icedtea-compositor.service`, `Restart=always`. The existing
  reconnect-with-backoff logic tolerates compositor restarts.
- Both units set
  `Environment=XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=icedtea XDG_SESSION_DESKTOP=icedtea`.
- `settings` is not a service — it is a regular window launched on demand from
  the start menu (D-Bus activation is future work).
- `/usr/share/wayland-sessions/icedtea.desktop` registers the session so
  GDM/lightdm/sddm can list it.

### Integration points

- **Autostart:** explicitly not re-implemented. `systemd-xdg-autostart-generator`
  launches `.desktop` Autostart entries as user units; the start menu only
  lists and launches apps on demand.
- **Power menu:** `loginctl` (lock/logout/reboot/shutdown) — a logind client in
  the shell.
- **Fullscreen inhibit:** a focused fullscreen window holds a logind
  `Inhibit` (suspend) so the screen does not blank/suspend during video; a
  small `org.freedesktop.login1` client in the compositor.
- **App launching:** spawned apps inherit the user-manager scope. Optional
  follow-up: `systemd-run --user` for launch-and-manage (stop/restart from the
  taskbar) — future work.

### Crash policy

- Compositor exit tears the session down (`PartOf=graphical-session.target`).
- Shell restart is independent (`Restart=always`); a dead shell never kills the
  WM, and a dead compositor does not wedge the shell — it reconnects on restart.

## Error handling

- Shell/settings disconnect: the client keeps its UI alive and re-watches the
  service name with backoff, then re-syncs via `GetState()`. The WM keeps
  running without a shell.
- Config corruption: compositor uses defaults; settings shows an error and
  offers reset-to-defaults on open.
- App crash isolation: a crashing client window is unmapped and cleaned up; the
  compositor and shell remain unaffected.

## Testing

- `layout.rs`: unit tests for placement and snap-zone geometry, including
  HiDPI and multi-monitor scenarios.
- `contract`: zbus type round-trip tests for every method/signal signature.
- `config`: table round-trip tests and default-fallback tests.
- Shell: unit tests for the state reducer against scripted DBus signal
  sequences; `.desktop` parsing helpers.
- Integration: nested Smithay Wayland backend opens surfaces and verifies
  workspace/focus logic; the DBus bridge is exercised against a real session
  bus in CI where available.
- Manual: full session with real apps, dragging, snapping, shell behavior,
  and systemd restart/recovery paths.

## Future work

- Live window thumbnails in Alt+Tab (screencopy-style protocol).
- Snap-assist tile picker.
- CLI configuration interface (writes redb, mirrors the settings GUI).
- D-Bus activation for the settings app.
- `systemd-run --user` app launching with taskbar stop/restart.
- Multi-monitor wallpaper per-output settings.
- Session persistence / restore.

## Risks

- Smithay API churn (active project; version pin recommended).
- The zbus ↔ calloop channel bridge is the main integration risk; it is small
  and isolated, but if it fights the event loop, an alternative is running the
  DBus service on a dedicated thread with a bounded channel.
- SSD rendering is a substantial slice of work; the hybrid mode limits the
  surface area (CSD-requesting apps are skipped).
- Full scope in one plan carries mid-course redesign risk; the DBus contract
  and pure-function boundaries are the pressure-release points.
