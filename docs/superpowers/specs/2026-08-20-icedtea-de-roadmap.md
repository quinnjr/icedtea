# icedtea Desktop Environment — Audit & Roadmap

**Date:** 2026-08-20
**Status:** proposed roadmap — for refinement, then issue creation
**Scope:** the whole project (compositor + system services + shell/UI), spanning the
current codebase and the pure-Rust UI rebuild.

## Direction (decided)

- **Goal: a polished DE *product for others*** — optimize for coherence and
  completeness over speed-to-usable. Consequence: **the pure-Rust UI toolkit
  (the rebuild) is the UI substrate throughout; no throwaway GTK shell
  surfaces** are built as interim.
- **XWayland is early / high priority** — without it a large share of real
  software won't run, which caps how "daily-driver" icedtea can be.
- This document is the reference roadmap; the prioritized items become an epic +
  tiered GitHub issues after refinement.

The "no throwaway GTK" decision forces a **two-track structure**: compositor and
system-service work is toolkit-independent and starts now; every visible shell
surface is built on the pure-Rust toolkit and is therefore **gated on the
rebuild reaching widget breadth** (rebuild milestone M3).

---

## Audit — where we are (branch `develop` @ a518bfb)

### Solid and well-tested (the window-manager core)

- **Compositor** on the `wlr` crate 0.20.22: floating WM with edge/corner
  **snapping** (half + quadrant tiling), **maximize/fullscreen/minimize** with
  separate restore geometries, **4 configurable global workspaces**,
  click-to-focus, configurable **alt-tab**, interactive **move/resize** (SSD
  title bar + client-initiated), **server-side decorations** (min/max/close, CSD
  detection), real **multi-monitor** (hot-unplug migration, per-output maximize),
  cascade placement.
- **Protocols present:** xdg-shell, xdg-decoration, wlr-layer-shell,
  output-management, screencopy, session-lock, idle-notify (+ idle-inhibit),
  pointer-constraints, relative-pointer, data-control, primary-selection,
  virtual-keyboard, virtual-pointer.
- **Wallpaper** (compositor-side, multi-output, live reload), a real
  **clipboard manager** daemon (wlr-data-control, history/pin, `org.icedtea.
  Clipboard`), a **taskbar** (workspaces + window list), and a rich
  `org.icedtea.WM` D-Bus surface (focus/close/min/max/fullscreen/workspace/
  move/get_state/reload/quit + WindowOpened/Closed/Updated/WorkspaceSet/List/
  AltTabState/ConfigReloaded signals).
- **Settings app** (5 pages): Appearance, Behavior, Workspaces, Keybindings,
  Displays (the `zwlr_output_management_v1` drag canvas).
- Notably high test coverage; disciplined, review-driven development.

### Gaps by layer (the DE around the core)

| Layer | Present | Missing (the work) |
|---|---|---|
| **Compositor / client-compat** | protocols above | **XWayland**; **foreign-toplevel-management**; **fractional-scale + viewporter**; **cursor-shape**; **xdg-activation**; **gamma-control**; **text-input-v3 + input-method-v2**; tablet-v2; content-type/single-pixel-buffer/presentation-time; confirm xdg-output/presentation-time from the wlr core |
| **Session & system plumbing** | — | **logind/seat** (suspend, lid-close, lock-on-sleep, inhibitors); **notification daemon** (`org.freedesktop.Notifications`); **xdg-desktop-portal + PipeWire** (Flatpak file-chooser, screencast/screen-share); **polkit agent**; **secret service/keyring**; XDG autostart |
| **Shell surfaces (visible desktop)** | bar: taskbar + clipboard popover | **app launcher** (today apps can only start from a terminal); **system tray / StatusNotifierItem host**; **notification popups**; **lock-screen UI** (protocol done, no password prompt); **OSD** (volume/brightness); clock/calendar; battery/network/volume/bluetooth indicators; quick-settings |
| **Settings + hardware controls** | 5 pages above | **no backend at all** for audio (PipeWire), network (NetworkManager), bluetooth (bluez), brightness (backlight), power/battery (UPower); no settings pages/schema for keyboard-layout, sound, network, bluetooth, power, date/time, default apps, notifications |
| **WM niceties** | global workspaces, click-focus | per-output workspaces; focus-follows-mouse option; (a real tiling engine, if wanted — taste-dependent) |

**Four hardest blockers to daily use:** no app launcher · no XWayland · no
notifications · no lock-screen UI. (Existing but *unimplemented* design docs:
shell-applets clock/volume/MPRIS/network, and night-light.)

---

## Roadmap — two parallel tracks + the enabler

### Track C (enabler, critical path for UI) — the pure-Rust UI rebuild

Already specced (`docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`;
issues #1–#7): M1 proving slice → M2 GTK-CSS engine → **M3 widget toolkit
breadth** → M4 icons → M5 app migrations → M6 polish. **M3 is the gate that
unblocks Track B** — every DE shell surface is a new surface/app on this toolkit.

### Track A — compositor & system services (toolkit-independent; start now)

Runs in parallel with Track C; each protocol batch is an additive `wlr` publish
(0.20.23+), coordinated with any parallel wlroots-sys sessions.

- **A1. XWayland** *(early, biggest lift)* — wrap wlroots XWayland in the `wlr`
  crate (wlroots-sys → publish) → compositor integration: X11 surface mapping,
  SSD/decoration parity, focus + stacking, clipboard/primary bridge, DND bridge,
  override-redirect handling, HiDPI. Spike the wlr-crate cost first.
- **A2. Client-compat protocol batch** — `xdg-activation` (focus-steal
  prevention/attention), `cursor-shape`, `fractional-scale` + `viewporter`
  (crisp HiDPI), `content-type`, `single-pixel-buffer`, `presentation-time`;
  **`foreign-toplevel-management`** (standard docks/pagers — and our own tray/
  taskbar can consume it); **`gamma-control`** (night-light backend). Confirm
  `xdg-output`/`presentation-time` coverage from the wlr core.
- **A3. logind session/seat client** — `org.freedesktop.login1`: seat/session,
  suspend/hibernate, lid-close, idle→lock (ties to session-lock), inhibitors.
- **A4. Notification daemon** — a standalone `org.freedesktop.Notifications`
  service crate (state, categories, actions, DND, persistence). The *popup UI*
  is B3; this is the backend so `notify-send`/libnotify stop silently failing.
- **A5. xdg-desktop-portal-icedtea + PipeWire** — portal backend for
  screenshot/screencast/file-chooser (Flatpak + screen-sharing). Screencopy is
  already present; wire the portal D-Bus surface + PipeWire streams.
- **A6. Input methods** — `text-input-v3` + `input-method-v2` (CJK/IME, on-screen
  keyboards); `tablet-v2` later.
- **A7. WM model extensions** — per-output workspaces; focus-follows-mouse
  option; (optional) a tiling layout mode.
- **A8. Privilege & secrets** — polkit agent + secret-service/keyring backend
  (their prompt/vault UIs are Track B).

### Track B — shell & UI surfaces (on the pure-Rust toolkit; gated on Track C M3)

No throwaway GTK: these are surfaces/apps on `icedtea-ui`. Ordered by leverage.

- **B1. App launcher — Windows 7/10 Start-menu inspired** *(explicit design
  direction)*. A bottom-left-anchored layer-shell panel opened from a Start
  button on the bar, styled and structured after the Win7/Win10 Start menu:
  - a **search box** (fuzzy `.desktop` search-as-you-type; also files/settings
    later) — Win7-style "type to find",
  - a **pinned apps** list + an **All apps** alphabetical/scrollable list (Win10
    left rail), with **recent/frequently-used** apps,
  - a **tiles / shortcuts** area (Win10 right pane — resizable, groupable tiles;
    pin-to-Start), doubling as quick links (user folders, settings),
  - a **power/session** control row (lock / log out / suspend / restart / shut
    down), Win7's bottom-right actions.
  Keyboard-first (open, type, arrow, enter), mouse-friendly, themeable via the
  toolkit's GTK-CSS. Backed by an XDG `.desktop` index + a pin/tile store in
  config. This is the highest everyday-leverage surface and a signature piece of
  the DE's identity, so it gets its own detailed spec.
- **B2. Lock-screen UI** — session-lock client with a PAM password prompt
  (compositor side already done).
- **B3. Notification popups** — layer-shell surfaces rendering the A4 daemon's
  notifications; stacking, actions, DND.
- **B4. System tray / StatusNotifierItem host** — in the bar, for third-party
  apps (Discord/Telegram/etc.).
- **B5. Bar indicators + clock/calendar + quick-settings** — audio, network,
  battery, bluetooth, brightness, clock; each backed by the system backends
  below. A quick-settings popover.
- **B6. OSD** — volume/brightness on-screen display.
- **B7. Settings breadth** — new toolkit-native pages **and** config schema/
  backends: keyboard layout/input, sound, network, bluetooth, power, date/time,
  default apps, notification policy.
- **B8. First-party apps** *(later, optional)* — terminal, file manager,
  screenshot/record tool, image viewer.

### Cross-cutting — hardware/system backends (feed both settings pages and indicators)

Rust D-Bus/library clients, shared by Track B UIs:

- **Audio:** PipeWire (`pipewire-rs`) — volume, device selection, per-app.
- **Network:** NetworkManager (D-Bus) — Wi-Fi/ethernet/VPN state + connect.
- **Bluetooth:** bluez (D-Bus) — pairing/connect.
- **Power/battery:** UPower + logind — battery %, profiles, suspend policy.
- **Brightness:** backlight via logind `SetBrightness` (or sysfs).

**Build backends before their UIs** — the A-track/system backends land first,
then the B-track indicators/pages consume them.

---

## Sequencing & dependencies

- **Now, in parallel:** push Track C toward **M3** (unblocks all of Track B) AND
  run Track A (start **A1 XWayland**, plus A3 logind, A4 notification daemon
  backend, A5 portal — all toolkit-independent).
- **After Track C M3:** Track B opens — B1 launcher, B2 lock UI, B3 notifications
  UI, then indicators/settings as their backends (cross-cutting) come online.
- **Publish cadence:** batch additive `wlr` protocol work (A1/A2) into as few
  releases as sensible; coordinate with parallel wlroots-sys sessions; each
  publish is a hard-stop for consent (project rule).

## Risks

- **XWayland is a large `wlr`-crate lift** — spike its cost early even though
  it's high priority; it gates a real slice of app compatibility.
- **Track B is fully gated on the toolkit** — if the rebuild stalls, the visible
  DE stalls. Accepted per the "no throwaway GTK" decision; the mitigation is to
  keep Track A delivering value (compat + plumbing) meanwhile.
- **Backend breadth** (PipeWire/NM/bluez/UPower) is a lot of D-Bus integration
  surface; each is an independent sub-project with its own spec.
- **Scope** — this is a multi-quarter program; every lettered item is an
  independently-specced sub-project, sized like the milestones already shipped.

## Out of scope (for now)

- Display manager / greeter (login), OS installer, distro packaging.
- A full tiling layout engine (noted as optional A7; taste-dependent).
- Making icedtea a general-purpose third-party toolkit (the UI toolkit is scoped
  to icedtea's own apps first).

## Track-A sub-project specs

Designed out (2026-08-20) into per-sub-project specs alongside this roadmap:
`icedtea-xwayland-design.md` (A1), `icedtea-compat-protocols-design.md` (A2),
`icedtea-logind-session-design.md` (A3), `icedtea-notifications-daemon-design.md`
(A4), `icedtea-xdg-portal-design.md` (A5), `icedtea-input-methods-design.md`
(A6), `icedtea-wm-extensions-design.md` (A7), and the cross-cutting
`icedtea-system-backends-design.md`. Notable: **XWayland's FFI is already bound
in `wlr-sys`** (default-on `xwayland` subsystem; `wlr_xwayland`/`wlr_xwm` tracked
as unwrapped in the coverage ledger), so A1 is a safe-wrapper + integration job,
not a bindgen expedition — a real de-risker.

## Cross-cutting coordination (from the design review)

The seven specs are individually coherent, but three coordination constraints
bind them and MUST be honored when they become issues/plans:

1. **`wlr` version numbers are allocated at publish time, not fixed in specs.**
   A1, A2, and A6 each illustratively claim `0.20.23`/`.24`/`.25`, but every
   publish is a strictly-sequential additive `0.20.x` and the parallel wlr-port
   M5 scene work publishes into the same space — so at most one lands on any
   given number. **Strike hard-coded versions from issues; keep a single
   version-allocation ledger and claim-in-order at publish time.**
2. **The `contract` crate needs a single coordinated signature bump, not three
   parallel edits.** A2 (window attention/urgent bit → `WindowInfo`/
   `WindowUpdate`), A3 (`Event::SessionLockChanged`), and A7 (per-output fields
   on `WorkspaceInfo`/`WorkspaceSet`/`Snapshot` — a **wire-breaking** change) all
   touch `contract/src/{types.rs,event.rs}` and the single
   `wire_signatures_are_locked` test; run in parallel they conflict every time.
   Land the contract changes in one coordinated bump (or a strict order:
   A7 wire-break first, then A2/A3 additive).
3. **Shared `login1` proxy.** A3 (session) and the system-backends brightness
   domain both open an `org.freedesktop.login1` proxy — factor a shared
   login1-client helper rather than hand-rolling it twice.

## Gaps the review surfaced (need owners)

- **A8 (polkit agent + secret-service/keyring) has no spec yet**, but A3 and
  system-backends both depend on it (saved-Wi-Fi secrets, interactive BlueZ
  pairing, privilege prompts). Needs its own design before those features
  complete.
- **XDG autostart** (roadmap plumbing row) is designed by none of the seven —
  today only icedtea's own daemons start (systemd user units); third-party
  `.desktop` autostart has no runner.
- **Low-battery / device-connected notification wiring** is unowned:
  system-backends assumes A4 consumes `org.icedtea.System` `Changed` signals, but
  A4 doesn't mention it. Assign this bridge to a milestone (recommend A4
  consuming the system backend).

## Next steps

1. Refine this roadmap.
2. Create the epic + tiered issues (Track A sub-projects + the coordination
   constraints + the A8/autostart/notification-bridge gaps) on
   `quinnjr/icedtea-wm`.
3. Immediate parallel starts (respecting the coordination constraints above):
   **A1 XWayland spike**, **A4 notification daemon**, **A3 logind session**,
   and **A2 Batch-1** (passive/scene-internal protocols — highest leverage,
   lowest risk).
