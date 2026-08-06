# icedtea-wm Shell Applets — Calendar/Clock, Volume/Media, Network Design

Date: 2026-08-06

## Overview

Three taskbar applets that live *inside* the existing GTK4 shell process,
extending the shell section of
`docs/superpowers/specs/2026-08-01-icedtea-wm-design.md`. They add the
everyday "system" controls that make a desktop feel complete: a clock/calendar,
volume + media control, and a network indicator. They are **shell-embedded** —
no new process, no new systemd unit, no new compositor protocol
(clipboard/night-light live separately in the "compositor-dependent" doc).

All three share the shell's conventions: gio/DBus on the glib main context,
plain GTK4 + custom CSS, a pure reducer per applet, message-passing-only worker
threads, and the shell's crash-independent lifecycle (`Restart=always`).

## Scope (v1)

- **Clock + calendar popover**: taskbar clock, a `Gtk::Calendar` popover.
- **Volume/media**: taskbar volume icon + a mixer popover, mute, per-app
  volumes, media-keys → play/pause/next/prev, now-playing text via
  `org.mpris.MediaPlayer2`.
- **Network indicator**: tray icon + Wi-Fi list popover, connect/disconnect/
  forget, wired + Wi-Fi status, over NetworkManager (`org.freedesktop.NetworkManager`).

Out of scope for v1:

- Brightness / night light (compositor-dependent → sibling scope).
- Clipboard manager (sibling scope).
- A color/theme picker beyond what the shell already has.
- OS settings: audio-input controls beyond a single mic toggle.
- Notification-center hooks (the calendar is display-only).
- Password prompting in network connect (Wallet hook is later).

## Architecture

All three applets share one contract inside the shell:

```
shell process (GTK4)
 ├─ statusbar applet container  — icon → popover scaffolding, CSS
 ├─ applets/
 │   ├─ clock.rs    → pure formatter + timer
 │   ├─ volume.rs   → pipewire client + Mpris proxy
 │   └─ network.rs  → NetworkManager client
 └─ redb-less state: pure per-applet reducer, unit-tested
```

Backends (all clients, none require new compositor support):

- Clock: pure `glib::DateTime`; nothing external.
- Volume: **PipeWire** (v1, single backend — confirmed) for sink/listening/
  keys, plus **`org.mpris.MediaPlayer2`** over DBus for now-playing + transport
  (using MPRIS). 
- Network: **NetworkManager** via gio `DBusConnection` (objects
  `org.freedesktop.NetworkManager`, Devices methods, `Connect`/`Disconnect`,
  ActiveConnections). No own store; NM owns the keyring API.

Threading: each applet is a small pure reducer fed by glib callback events;
no blocking work on the UI thread; gio/pipewire lib on the main context.
Crypto/time: none.

## Clock / calendar

- Taskbar label: `HH:MM`, tooltip date, format from shell config
  (24h/12h + date style). Pure formatter unit-tested with fixed instants.
- Click → popover with `Gtk::Calendar` (today highlighted) and — when later
  wired to a calendar backend — today's events. **v1: calendar only**, no event
  source, no writing; theme handled by CSS.

## Volume / media

New `volume.rs` applet + `audio` worker in the shell:

- Icon: mute indicator; slider override; `show volumes` toggle.
- Mixer popover: master slider, per-sink/per-app channel sliders, mute,
  a refresh of PipeWire node names; device switch (sink list).
- Media keys: the shell forwards key presses (`XF86AudioPlay` etc. — already
  part of compositor input handling for the window manager) → Mpris
  `CallMethod("PlayPause"/"Next"/"Previous")` on the MPRIS player proxy; if
  none, show a "no player" toast.
- Now-playing: watch `org.mpris.MediaPlayer2` NameOwnerChanged +
  `PropertiesChanged`; display title/artist + transport buttons in a mini
  player card at the top of the mix.
- State reducer `VolumeState { pipes, active_sink, volume, muted,
  now_playing? }` — pure, canned-event unit tests.
- PipeWire bindings: confirm the `pipewire` crate at MSRV 1.94 surface we need
  (room for a thin wrapper in `volume.rs`).

## Network

New `network.rs` in the shell, an `org.freedesktop.NetworkManager` client:

- Tray icon shows strength/state (wired vs Wi‑Fi vs off), animated conns.
- Popover: active connection name; Wi‑Fi list (SSID, BSSID, signal, locked);
  connect/disconnect/forget; enable airplane-mode-light; refresh.
- Secured networks: show connect → optional password prompt for absent saved
  secrets. **v1** uses NetworkManager's own stored secrets (no prompting); the
  "ask the wallet" flow is a documented follow-up hook.
- State `NetworkState`: devices, active_conns, scan results; reducer over NM
  signals, unit-tested with canned events.

## Compositor integration (none required)

Nothing in this doc touches the compositor, wl protocols, or new systemd
units — its only requirements are the PipeWire library + MPRIS DBus (could be
present) and NetworkManager availability. This scope is buildable as soon as
the shell crate exists.

## Testing & gates

- Each `applets::*` reducer unit-tested with scripted backend events (clock
  formatting, volume transitions, network state edges).
- Integration: the shell's existing pipeline; `pipewire` plumbing covered by a
  manual smoke `/ smoker`.
- Gates: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`.

## Crate layout (inside `shell/`)

```
shell/
├─ … existing …
└─ src/
   ├─ statusbar/        — applet container + popovers
   ├─ applets/
   │  ├─ clock.rs       — timer + formatter
   │  ├─ calendar.rs    — Gtk::Calendar view
   │  ├─ volume.rs      — PipeWire worker + MPRIS
   │  └─ network.rs     — NetworkManager client
```