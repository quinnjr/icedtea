# icedtea-wm Clipboard Manager + Night Light Design

Date: 2026-08-06

## Overview

Two shell-adjacent features that reach outside the pure-shell scope and touch
the compositor:

- **Clipboard manager** — Klipper/Clipboard-Indicator behavior, delivered as an
  independent **daemon** (`icedtea-clipboard`) that owns
  `wlr-data-control-v1` and keeps a bounded history surfaced in a taskbar
  popover.
- **Night light** — a render-time color-temperature feature inside the
  compositor (no new protocol), scheduled via config.

These extend the shell and compositor sections of
`docs/superpowers/specs/2026-08-01-icedtea-wm-design.md`. They are a separate
scope from the clock/volume/network applets (those are shell-embedded only).

## Scope (v1)

- **Clipboard daemon**: capture clipboard on change, bounded per-entry history,
  re-paste, pin/favorite, clear, copy-as-text; tray/statusbar-managed via the
  shell.
- **Night light**: warm color-temperature overlay with config-driven schedule,
  smooth transition.

Out of scope for v1:

- Rich OLE/data wrapping; clipboard sync across multiple seats.
- Persistant clipboard across reboots (memory-only history).
- A keyboard-heavy clipboard picker (UI comes via the popover).
- Multi-screen gamma differences (one tint across outputs).

## Clipboard manager

### Positioning

A separate tiny long-lived process, `icedtea-clipboard`, so a crash in its
session never harm the shell and its wl-data-control capture stays
isolated (mirrors KDE's `klipper`/GNOME's `copyq`).

### Protocol / compositor dependency

- Uses `wlr-data-control` v1 (`zwlr_data_control_manager_v1`): a client
  registers, watches the primary selection (`SelectionEvent`), and can set the
  selection back (`SetClipboard`). The compositor must **expose this
  interface** — confirm smithay 0.7 `wayland::data_control`, and if absent,
  wire the protocol from `wayland-protocols` (the plan pins the exact file).
  This is the only new compositor protocol in this scope.

### Behavior

- On a selection-change event, capture `text` (mime `text/plain`, `text/uri`
  list) and, if enabled, small images — bounding history, configurable.
- **ClipboardState** reducer: `{ history: VecDeque<Entry>, pinned: Vec<Entry>,
  clear_selection }` where Entry = typed payload + source app_id. Pure and
  unit-tested.
- Paste-from-history re-sets the selection via the data-control manager; the
  compositor applies it like any surface copy.
- Memory-only in v1; optional redb persistence is a follow-up (consistent with
  the "never crash on corrupt state" ethos if added via a config DB).
- A taskbar popover (in the shell, receiving the daemon's state over the
  existing DBus control plane or a small interface) renders the list + pin /
  clear actions. The daemon owns the data; the shell only renders.

## 2. Night light

### Where it lives

Inside the **compositor** as a render feature — the compositor already owns
composition, so no gamma protocol client is needed. `render.rs` final color
multiply by a temperature tint.

### Config & behavior

- Config via the existing redb **appearance/behavior** tables (hot-reload):
  `night_light_enabled`, `schedule: fixed{start,end}` | `auto (dusk-dawn)`,
  `temperature_night` (default ~4000K), `temperature_day` (6500K "off"),
  `transition_minutes`.
- `nightlight.rs` computes the current target temperature from `time`/`lat_lng`
  (auto) or the fixed window; the renderer interpolates the color matrix toward
  the target over the transition window (smooth, no step).
- State `NightLightState { enabled, temperature, active }` — pure, unit-tested
  over fixed clock/geo inputs. Applied as a final scene multiply in `render.rs`.

### Systemd / integration

- `icedtea-clipboard.service`: `PartOf=graphical-session.target`, starts with
  the session, `Restart=always`, after the compositor (so the protocol is up).
- No unit for night light — it is part of the compositor binary.

## Threading

- Clipboard daemon: the wlr-data-control manager + history live on one glib
  main context; heavy payload (image downscaling) on a worker; state reducer
  on the main thread. Bounded content, no shared mutation.
- Night light: compositor-render/builtin, driven by the clock on the existing
  calloop loop; no worker needed.

## Testing & gates

- Clipboard: unit-test the history/selection reducer; sim the
  SelectionChanged+SetClipboard round-trip with a test-only client; manual
  copy/paste flow.
- Night light: unit test temperature→matrix mapping and transition sequence
  with a fake clock; the renderer's final-multiply covered by a render
  smoke test.
- Gates: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`.

## Risks

- **wlr-data-control availability** in smithay 0.7 is the main integration
  risk; revalidated while pinning the plan. Fallback: transport the data via a
  small internal protocol if needed.
- Clipboard image blobs can be large; the configurable type budget and
  downscale keep the popover responsive.

## Crate layout

```
icedtea-clipboard/         — daemon
├─ Cargo.toml
├─ src/
│  ├─ main.rs
│  ├─ manager.rs           — zwlr_data_control_manager client
│  ├─ history.rs           — bounded history reducer (pure)
│  └─ worker.rs            — payload downscale/budget on worker thread
compositor/                 (night light, existing crate)
└─ src/
   ├─ nightlight.rs        — schedule → temperature → matrix
   └─ render.rs            — apply tint as final multiply (existing)
systemd/icedtea-clipboard.service
```