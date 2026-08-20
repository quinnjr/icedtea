# icedtea xdg-desktop-portal Integration — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Roadmap slot:** Track A, item **A5** (`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`)
**Branch:** target `develop` (compositor/system track; toolkit-independent except FileChooser)

## Goal

Give icedtea working **xdg-desktop-portal** support so apps — especially
sandboxed/Flatpak ones — can screen-share, take screenshots, follow the
desktop's dark-mode/accent, and (later) use native file dialogs. Do it in a way
consistent with the polished/pure-Rust/no-GNOME direction.

## Decisions (from brainstorming)

1. **Hybrid strategy.** Reuse **`xdg-desktop-portal-wlr`** (xdpw) for
   **ScreenCast + Screenshot** now — it is wlroots-native (not GNOME) and works
   with icedtea's existing `wlr-screencopy`. Build a **native Rust backend**
   (`xdg-desktop-portal-icedtea`) for **Settings** now and **FileChooser** later.
   Never pull in `xdg-desktop-portal-gtk` (that *is* GTK/GNOME).
2. **Target all four interfaces**, phased: Settings (native, first) →
   ScreenCast + Screenshot (xdpw integration) → FileChooser (native, gated on the
   pure-Rust toolkit / Track B).

## Architecture — one frontend, split backends

The system `xdg-desktop-portal` frontend routes each `org.freedesktop.impl.
portal.*` interface to a backend per a `portals.conf` selected by
`XDG_CURRENT_DESKTOP=icedtea`:

| Interface | Backend | Nature |
|---|---|---|
| `org.freedesktop.impl.portal.ScreenCast` | `wlr` (xdpw) | config + verify (screencopy + PipeWire) |
| `org.freedesktop.impl.portal.Screenshot` | `wlr` (xdpw) | config + verify |
| `org.freedesktop.impl.portal.Settings` | **`icedtea`** (new) | native Rust, ships first |
| `org.freedesktop.impl.portal.FileChooser` | **`icedtea`** (new) | native Rust, **gated on the toolkit** |

## Phase 1 — native Settings backend (first buildable deliverable)

**New crate `portal/`** producing the binary `xdg-desktop-portal-icedtea`, a
`zbus` service on the **session bus** with well-known name
`org.freedesktop.impl.portal.desktop.icedtea`, object path
`/org/freedesktop/portal/desktop`, implementing
**`org.freedesktop.impl.portal.Settings`** (version 2):

- **Methods:**
  - `ReadAll(namespaces: as) -> (a{sa{sv}})` — namespace → {key → variant}.
  - `ReadOne(namespace: s, key: s) -> (v)` (v2) and `Read(...)` (v1 compat).
- **Signal:** `SettingChanged(namespace: s, key: s, value: v)`.
- **Namespace `org.freedesktop.appearance`:**
  - `color-scheme: u` — `0` no-preference, `1` prefer-dark, `2` prefer-light.
  - `accent-color: (ddd)` — RGB doubles in `0.0..=1.0`.

**Source of truth:** icedtea's appearance config (reuse `icedtea-config` +
`icedtea-contract::Appearance`).
- On startup, load appearance via the config crate (`default_db_path`); compute
  `accent-color` from `appearance.palette.accent` (hex → RGB doubles) and
  `color-scheme` from the background palette's luminance (dark background ⇒
  prefer-dark), with room for an explicit config field later.
- **Live updates:** subscribe to `org.icedtea.WM`'s `ConfigReloaded(Appearance)`
  signal (already emitted when the settings app applies), recompute, and emit
  `SettingChanged` for any changed key — so GTK4/Qt6/Chromium/Firefox re-theme
  live when the user changes the desktop theme.

**Load-bearing tests:**
- Unit: `ReadAll`/`ReadOne` return the correct `color-scheme`/`accent-color` for
  a given `Appearance` (hex→RGB and luminance→scheme are pure, non-vacuously
  tested — e.g. a known dark bg ⇒ `1`, a known accent hex ⇒ the right triple).
- Integration (`zbus`, the repo's `live_dbus` pattern): start the backend on a
  private bus, call `org.freedesktop.impl.portal.Settings.ReadAll`, assert the
  appearance values; drive a `ConfigReloaded` and assert a `SettingChanged` with
  the new value.

## Phase 2 — ScreenCast + Screenshot via xdpw (integration)

Mostly configuration + verification, not new compositor code:
- Ship a default xdpw config and the routing so the frontend sends ScreenCast/
  Screenshot to `wlr`.
- **Verify** icedtea's `wlr-screencopy` satisfies xdpw end-to-end with a real
  consumer (Firefox/OBS/`grim`). Note: no `export-dmabuf`/`linux-dmabuf`
  zero-copy path yet, so capture is the **shm** path — correct but a perf
  follow-up (adding dmabuf export is a separate compositor/wlr task).
- **PipeWire** is a runtime session dependency (xdpw streams through it);
  document it and ensure the session brings it up.
- **Output/region picker:** xdpw needs a chooser for which output/window/region
  to share. Near-term: an external chooser (slurp-style); a native picker is a
  later Track-B surface.

## Phase 3 — native FileChooser (gated on the toolkit / Track B)

`org.freedesktop.impl.portal.FileChooser` (`OpenFile`/`SaveFile`/`SaveFiles`)
needs a GUI file dialog, so it lands on the pure-Rust toolkit. Designed here,
built when the toolkit can render a file browser; until then FileChooser stays
unrouted (apps fall back to their own non-portal path).

## Session wiring & packaging

- `data/xdg-desktop-portal-icedtea.portal` (installed under
  `/usr/share/xdg-desktop-portal/portals/`):
  ```
  [portal]
  DBusName=org.freedesktop.impl.portal.desktop.icedtea
  Interfaces=org.freedesktop.impl.portal.Settings;
  UseIn=icedtea
  ```
  (Add `FileChooser` to `Interfaces` when Phase 3 lands.)
- `data/icedtea-portals.conf` (installed as
  `/usr/share/xdg-desktop-portal/icedtea-portals.conf`):
  ```
  [preferred]
  default=none
  org.freedesktop.impl.portal.ScreenCast=wlr
  org.freedesktop.impl.portal.Screenshot=wlr
  org.freedesktop.impl.portal.Settings=icedtea
  ```
- A D-Bus-activatable **systemd user service** for the backend
  (`org.freedesktop.impl.portal.desktop.icedtea.service` + the systemd unit),
  matching how `shell`/`clipboard` are started.
- The session must export `XDG_CURRENT_DESKTOP=icedtea` so the frontend selects
  our `portals.conf`.
- **Runtime deps documented:** `xdg-desktop-portal`, `xdg-desktop-portal-wlr`,
  `pipewire` (for screencast).

## Risks

- **xdpw ↔ screencopy fidelity/perf** — shm path works; dmabuf export is a
  follow-up for zero-copy performance. Verify with a real consumer early.
- **PipeWire** must run in the session (screencast dep).
- **`portals.conf` format is version-sensitive** across xdg-desktop-portal
  releases; pin the tested behavior and document the minimum version.
- **Settings key/namespace correctness** — must match what GTK4/Qt6 read
  (`org.freedesktop.appearance` `color-scheme`); well-specified, low risk.
- **FileChooser** blocked on the toolkit (accepted; deferred).
- **`color-scheme` derivation** from luminance is a heuristic; a later explicit
  config field makes it exact.

## Out of scope (for now)

- Other portal interfaces (Inhibit, GlobalShortcuts, Wallpaper, Background,
  Notification, RemoteDesktop, Account, Print, Secret) — add as needed later.
- dmabuf/zero-copy screencast path (separate compositor/wlr follow-up).
- A native screencast output/region picker (a Track-B surface later).

## Next step

On approval, turn **Phase 1 (native Settings backend)** into an implementation
plan (writing-plans) — it is the concrete, toolkit-independent, testable first
deliverable and immediately makes apps follow icedtea's dark-mode/accent.
