# icedtea System Backends — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Roadmap slot:** the **cross-cutting hardware/system backends** section of
`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md` (lines 141–152 — the
block that feeds Track-B **B5** indicators and **B7** settings breadth).
**Branch:** target `develop` (toolkit-independent; buildable now)

## Goal

Give icedtea a **toolkit-independent backend layer** for the five hardware/system
domains the roadmap names — **audio, network, bluetooth, power/battery,
brightness** — that both future bar indicators (Track B5) and future settings
pages (Track B7) consume. Each domain today has **"no backend at all"** (roadmap
line 58). The deliverable is a running daemon that owns the upstream integrations
(PipeWire, NetworkManager, bluez, UPower, logind) and re-exposes each domain over
a stable icedtea D-Bus surface, plus a thin Rust client for in-tree UIs.

This spec is the **backend only**. The indicator widgets, the quick-settings
popover, the OSD, and the settings pages are Track B, gated on the pure-Rust
toolkit, and are **out of scope** here — they are named only as the consumers the
D-Bus contract is shaped for.

## Decisions (from the roadmap + codebase grounding)

1. **One crate, one daemon, per-domain modules — not per-domain crates.** A new
   workspace member `system/` (crate `icedtea-system`) produces one long-lived
   daemon binary `icedtea-system`, with a module per domain
   (`audio`, `network`, `bluetooth`, `power`, `brightness`). Rationale, grounded
   in the existing `clipboard/` daemon (`clipboard/src/main.rs`,
   `clipboard/src/service.rs`):
   - The codebase's established pattern for "own a system resource, re-expose it
     over `org.icedtea.*`" is exactly one daemon per resource
     (`icedtea-clipboard` owning `org.icedtea.Clipboard`). A single system daemon
     amortizes **two** bus connections (session + system) and the PipeWire
     mainloop thread across all five domains, instead of five processes each
     paying that cost.
   - Every domain reuses the same scaffolding — a shared-state snapshot, a change
     emitter, and a command channel — so they are modules over one runtime, not
     five duplicated daemons/units.
   - Per-domain **Cargo features** (`audio`, `network`, `bluetooth`, `power`,
     `brightness`, all in `default`) give build/test isolation without process
     isolation: CI can build and test `network` without pulling `pipewire`'s
     native dependency. Rejected alternative (per-domain crates) would multiply
     bus names, systemd units, and scaffolding for isolation that features
     already provide.

2. **D-Bus is the contract boundary; a Rust client ships alongside for
   ergonomics.** The consumers are **separate processes** — the bar/indicators,
   the OSD, and the settings app are all distinct surfaces (roadmap B5/B6/B7),
   just as `icedtea-shell` is a separate process from the compositor and consumes
   `org.icedtea.WM`/`org.icedtea.Clipboard` over D-Bus
   (`shell/src/wm_client.rs`, `shell/src/clip_client.rs`). An in-process Rust API
   alone cannot serve them. So the daemon owns **`org.icedtea.System`** on the
   **session bus**, and the crate additionally exports a `client` module of thin
   `zbus` proxies (one per domain) that in-tree Rust UIs depend on directly —
   the same split the shell already uses (`WmProxy`/`ClipProxy`), lifted into the
   backend crate so every consumer shares one implementation.

3. **The IPC vocabulary lives in `contract/`, with signature-lock tests.**
   Following `contract/src/clipboard.rs` and `contract/src/types.rs`, a new
   `contract/src/system.rs` holds the well-known name, per-domain object paths
   and interface names, and the `zvariant::Type` structs that cross the wire
   (`AudioState`, `NetworkState`, `BluetoothDevice`, …). Each gets a
   `*_wire_signature_is_locked` test exactly like `ClipEntry`
   (`contract/src/clipboard.rs:42`) and `wire_signatures_are_locked`
   (`contract/src/types.rs:138`) — because a second, non-Rust consumer may
   eventually marshal these by hand, the wire encoding is part of the contract.

4. **Per-domain `GetState` snapshot + `Changed` signal + command methods** — the
   same shape the shell already knows how to consume. Each domain interface
   exposes: a `Get<Domain>State() -> Struct` method (seed), a `Changed(Struct)`
   signal (deltas), and imperative command methods (`SetVolume`, `ConnectWifi`,
   …). Consumers seed from the snapshot then watch the signal — precisely
   `shell/src/wm_client.rs:33` (`GetState` then a `MatchRule` stream). This is
   chosen over leaning on `org.freedesktop.DBus.Properties` because it matches
   the pattern already proven in the tree and keeps one obvious seed-then-stream
   contract per domain. (Standard `PropertiesChanged` is still emitted by zbus
   for any `#[zbus(property)]` we add, but the snapshot+`Changed` pair is the
   primary contract.)

5. **Async `zbus` for D-Bus; a dedicated thread for PipeWire.** Four of the five
   upstreams (NetworkManager, bluez, UPower/power-profiles-daemon, logind) are
   **system-bus** D-Bus services — the daemon holds one async
   `zbus::Connection::system()` and drives them with `#[zbus::proxy]`-generated
   proxies and their signal streams, all on zbus's async executor. PipeWire is
   **not** D-Bus; `pipewire-rs` runs its own mainloop and is not cleanly async,
   so it lives on a dedicated OS thread bridged by channels — structurally
   identical to how `icedtea-clipboard` runs the Wayland event loop on one thread
   and the zbus service on another, message-passing only
   (`clipboard/src/main.rs:17-31`).

6. **The daemon runs as the user (session-bus service).** Upstream system-bus
   calls are therefore made from the user's active graphical session, so polkit
   sees an active local session and default-allows the common actions
   (NetworkManager connect, bluez pair for an active session, logind
   `SetBrightness`, UPower reads). No polkit **agent** is built here (that is
   roadmap A8); actions that need interactive auth are documented per domain and
   degrade to a returned error the UI can surface.

## Architecture

```
                    session bus                         system bus
  ┌──────────────┐  org.icedtea.System   ┌───────────┐
  │ bar / OSD /  │◄────GetState/Changed──│           │──NetworkManager──►(NM)
  │ settings app │────commands──────────►│ icedtea-  │──bluez───────────►(BlueZ)
  └──────────────┘                       │  system   │──UPower/ppd──────►(UPower)
  (Track B, out of scope)                │  daemon   │──logind SetBright►(logind)
                                         │           │
  ┌──────────────┐  in-proc Rust proxies │           │   dedicated thread
  │ in-tree UI   │◄──icedtea_system::─────│           │◄──pipewire mainloop──(PW)
  └──────────────┘        client         └───────────┘
```

### Crate layout (`system/`)

```
system/
  Cargo.toml            # features: audio,network,bluetooth,power,brightness (all default)
  src/
    main.rs             # build the two connections + PW thread, register org.icedtea.System, run
    lib.rs              # pub mod client; pub use domain state re-exports
    runtime.rs          # shared: SnapshotCell<T>, change emitter, command plumbing
    service.rs          # registers each domain interface object at its path (mirrors clipboard/service.rs)
    client.rs           # zbus proxies the UIs consume (mirrors shell/src/{wm,clip}_client.rs)
    audio/     mod.rs, pipewire.rs   # PW thread + AudioBackend trait
    network/   mod.rs, nm.rs         # NetworkManager proxies
    bluetooth/ mod.rs, bluez.rs      # BlueZ proxies
    power/     mod.rs, upower.rs     # UPower + power-profiles-daemon
    brightness/mod.rs, backlight.rs  # sysfs read + logind SetBrightness write
  systemd/icedtea-system.service     # mirrors clipboard/systemd/icedtea-clipboard.service
  tests/                             # per-domain live-bus integration tests
```

Add `"system"` to `members` in the workspace `Cargo.toml`. New deps:
`pipewire = "0.8"` (feature-gated on `audio`), `zbus.workspace`,
`futures-util = "0.3"`, `tokio` (or `async-io`) only if a runtime beyond zbus's
executor is needed for the PW bridge; `icedtea-contract = { path = "../contract" }`.

### Shared runtime (`runtime.rs`) — one pattern, five domains

Each domain owns a `SnapshotCell<T> = Arc<Mutex<T>>` (the daemon's authoritative
current state, seedable and clonable — exactly `clipboard/service.rs`'s
`snapshot: Arc<Mutex<Vec<ClipEntry>>>`). Upstream watcher tasks/threads write the
cell and push the new snapshot onto a `crossbeam_channel` to a per-domain emitter,
which fires the `Changed` signal via the low-level `emit_signal` — the exact
mechanism in `clipboard/src/service.rs:68-74`. D-Bus method calls read the cell
(`GetState`) or issue a command upstream (`SetVolume` → PipeWire thread via a
command channel + wake, or `ConnectWifi` → an async NM proxy call).

### Domain: Audio (PipeWire, feature `audio`)

- **Upstream:** `pipewire-rs` registry + metadata on a dedicated thread. Track the
  default sink/source, their volume (channel volumes, cubic↔linear conversion) and
  mute, the list of sink/source devices, and per-application streams (node
  name + volume) via the PipeWire object registry.
- **`AudioBackend` trait** wraps the PW thread behind a channel interface
  (`set_sink_volume`, `set_sink_mute`, `set_default_sink`, `set_stream_volume`)
  so a mock can replace real PipeWire in tests — the same testability seam the
  shell uses with the `WmCommands` trait (`shell/src/wm_client.rs:79`).
- **Interface `org.icedtea.System.Audio`** at `/org/icedtea/System/Audio`:
  - `GetAudioState() -> AudioState`
  - `SetSinkVolume(volume: f64)`, `SetSinkMute(mute: b)`,
    `SetDefaultSink(id: u)`, `SetSourceVolume(volume: f64)`,
    `SetSourceMute(mute: b)`, `SetStreamVolume(node: u, volume: f64)`
  - signal `Changed(AudioState)`
- **`AudioState`** (contract): default-sink/source volume+mute, `Vec<AudioDevice
  { id: u32, name: String, description: String, is_default: bool }>` for sinks and
  sources, and `Vec<AudioStream { node: u32, app: String, volume: f64,
  muted: bool }>`.

### Domain: Network (NetworkManager, feature `network`)

- **Upstream:** `org.freedesktop.NetworkManager` on the system bus. Proxies for
  the manager, `Device`/`Device.Wireless`, `AccessPoint`, `ActiveConnection`,
  `Settings`. Watch `StateChanged` / `PropertiesChanged` streams to update the
  snapshot.
- **Interface `org.icedtea.System.Network`** at `/org/icedtea/System/Network`:
  - `GetNetworkState() -> NetworkState`
  - `ScanWifi()`, `ConnectWifi(ssid: s, psk: s)` (passphrase inline via
    `AddAndActivateConnection`; empty for open), `ActivateConnection(uuid: s)`,
    `Disconnect(device: s)`, `SetWifiEnabled(on: b)`, `SetNetworkingEnabled(on: b)`
  - signal `Changed(NetworkState)`
- **`NetworkState`** (contract): overall connectivity (mapped from NM's
  `NM_STATE_*` to a small icedtea enum), primary-connection type/name, a
  `Vec<WifiAp { ssid, bssid, strength: u8, secured: bool, active: bool,
  known: bool }>`, and a `Vec<SavedConnection { uuid, id, kind }>`.
- **Secrets note:** connecting to a **new** secured AP passes the PSK inline;
  NetworkManager itself persists it. Reconnecting to a **saved** network with a
  stored secret works without a passphrase. Registering icedtea as a NM
  **secret agent** (to be prompted for missing secrets) and any keyring
  integration are roadmap A8 — out of scope; a missing-secret connect returns an
  error the UI surfaces.

### Domain: Bluetooth (BlueZ, feature `bluetooth`)

- **Upstream:** `org.bluez` on the system bus via the ObjectManager
  (`GetManagedObjects` + `InterfacesAdded`/`Removed` + per-object
  `PropertiesChanged`). Track the default adapter (powered, discovering,
  discoverable) and known/nearby devices (`org.bluez.Device1`).
- **Interface `org.icedtea.System.Bluetooth`** at `/org/icedtea/System/Bluetooth`:
  - `GetBluetoothState() -> BluetoothState`
  - `SetPowered(on: b)`, `StartDiscovery()`, `StopDiscovery()`,
    `Pair(address: s)`, `ConnectDevice(address: s)`,
    `DisconnectDevice(address: s)`, `RemoveDevice(address: s)`
  - signal `Changed(BluetoothState)`
- **`BluetoothState`** (contract): `powered`, `discovering`, and a
  `Vec<BluetoothDevice { address, name, icon, paired, connected, trusted,
  battery: Option<u8> }>`.
- **Pairing note:** BlueZ pairing may require an **agent** for PIN/passkey
  confirmation. Near-term the daemon registers a **`NoInputNoOutput`** agent
  (auto-accept, "just works" pairing), sufficient for headsets/mice; an
  interactive pairing agent that prompts the user is a Track-B surface + A8
  follow-up. Documented, not built for interactive confirm here.

### Domain: Power / Battery (UPower + power-profiles-daemon, feature `power`)

- **Upstream:** `org.freedesktop.UPower` (the `DisplayDevice` for aggregate
  battery: percentage, state, time-to-empty/full; on-battery vs AC via
  `OnBattery`) and, when present, `net.hadess.PowerProfiles` (active profile +
  available profiles). Watch each's `PropertiesChanged`.
- **Interface `org.icedtea.System.Power`** at `/org/icedtea/System/Power`:
  - `GetPowerState() -> PowerState`
  - `SetPowerProfile(profile: s)` (`power-saver`/`balanced`/`performance`; no-op
    error if ppd absent)
  - signal `Changed(PowerState)`
- **`PowerState`** (contract): `on_battery: bool`, `percentage: f64`,
  `state` (charging/discharging/full mapped to an enum), `time_to_empty_secs:
  Option<i64>`, `time_to_full_secs: Option<i64>`, `has_battery: bool`,
  `active_profile: Option<String>`, `available_profiles: Vec<String>`.
- **Boundary with roadmap A3 (logind session client):** suspend/hibernate,
  lid-close, idle→lock, and inhibitors are the **A3** sub-project, not this one.
  This domain consumes logind **only** for `SetBrightness` (below) and consumes
  UPower/ppd for the *battery/profile read model*. Power **actions** (suspend
  etc.) are A3's `org.icedtea` surface; the battery indicator's data is here.

### Domain: Brightness (logind + sysfs, feature `brightness`)

- **Read:** enumerate `/sys/class/backlight/*` (and `/sys/class/leds/*::kbd_backlight`
  for keyboard backlight) — `max_brightness` and `brightness` are world-readable,
  so reads need no privilege. A `udev`/inotify or short poll refreshes on external
  change.
- **Write:** `org.freedesktop.login1.Session.SetBrightness(subsystem: s,
  name: s, brightness: u)` on the system bus — logind performs the privileged
  write for the active session's user without a udev rule or root. Falls back to a
  documented sysfs write path (requires a shipped udev rule granting the
  `video`/`seat` group write) only where logind's method is unavailable.
- **Interface `org.icedtea.System.Brightness`** at `/org/icedtea/System/Brightness`:
  - `GetBrightnessState() -> BrightnessState`
  - `SetBrightness(device: s, value: u)` (raw), `SetBrightnessPercent(device: s,
    percent: f64)` (daemon converts against `max_brightness`)
  - signal `Changed(BrightnessState)`
- **`BrightnessState`** (contract): `Vec<Backlight { name, subsystem, brightness:
  u32, max: u32, percent: f64 }>`.

### Client module (`client.rs`)

One `zbus`-proxy struct per domain (`AudioClient`, `NetworkClient`, …), each with
a blocking command surface and an async seed+watch spawner, mirroring
`shell/src/wm_client.rs` (`WmProxy` for commands, `spawn` for the signal worker).
In-tree UIs (Track B) add `icedtea-system = { path = "../system" }` and consume
these instead of hand-writing proxies — the one place member names live, so a
rename can't drift (the exact class of bug `shell/tests/live_dbus.rs` was written
to catch).

## Decomposition into milestones

Each milestone is an independently reviewable, testable slice. **S0 first**, then
the five domains in the roadmap's stated priority order; domains are mutually
independent (separate modules, features, object paths) and can be parallelized.

- **S0 — Skeleton, contract vocabulary, runtime, packaging.** Add the `system`
  workspace member and `contract/src/system.rs` (bus name `org.icedtea.System`,
  the five object paths + interface names, and *all* state structs with
  `*_wire_signature_is_locked` tests). Build the shared `runtime.rs`
  (snapshot cell + emitter + command plumbing), `service.rs` registration, the
  two-connection + PW-thread `main.rs` skeleton with **no** real domains wired,
  the `client.rs` scaffold, the `icedtea-system.service` systemd unit
  (copy of `clipboard/systemd/icedtea-clipboard.service`, `After=` the
  compositor's `org.icedtea.WM`), and Cargo features. Exit: daemon starts, owns
  the name, an empty `GetState` per domain answers over a live session bus.

- **S1 — Audio (PipeWire).** PW thread + registry tracking + `AudioBackend`
  trait + `org.icedtea.System.Audio`. Volume/mute/default-device + per-app
  streams. (Roadmap: "PipeWire — volume, device selection, per-app.")

- **S2 — Network (NetworkManager).** NM proxies + `org.icedtea.System.Network`.
  Wi-Fi scan/connect, ethernet/VPN state, enable toggles. (Roadmap: "Wi-Fi/
  ethernet/VPN state + connect.")

- **S3 — Bluetooth (BlueZ).** ObjectManager tracking + NoInputNoOutput agent +
  `org.icedtea.System.Bluetooth`. Power/discover/pair/connect. (Roadmap:
  "pairing/connect.")

- **S4 — Power/Battery (UPower + ppd).** DisplayDevice + power-profiles +
  `org.icedtea.System.Power`. (Roadmap: "battery %, profiles, suspend policy" —
  suspend policy itself is A3; battery %/profiles here.)

- **S5 — Brightness (logind + sysfs).** Backlight enumeration + logind
  `SetBrightness` + `org.icedtea.System.Brightness`. (Roadmap: "backlight via
  logind `SetBrightness` (or sysfs).")

## Testing

Grounded in the repo's two existing patterns: **signature-lock unit tests** in
`contract/` and **live-bus round-trip integration tests** (`shell/tests/live_dbus.rs`,
`settings/tests/live_apply.rs`).

- **Contract signature locks (S0).** One `assert_eq!(State::SIGNATURE.to_string(),
  "…")` per new struct, exactly like `contract/src/types.rs:139` — so a field
  reorder or a dropped `option-as-array` feature breaks the ABI here, loudly.

- **Pure-conversion unit tests (per domain).** The lossy math each domain does is
  pure and gets non-vacuous tests: audio cubic↔linear volume round-trip and
  channel-average; NM `NM_STATE_*`/`NM_ACTIVE_CONNECTION_STATE_*` → icedtea enum
  (known constant ⇒ known variant); UPower `State` enum + percentage clamp;
  brightness raw↔percent against a known `max_brightness` (e.g. `max=255`,
  `raw=128 ⇒ 50.2%`, and back); Wi-Fi strength byte → bars.

- **Dependency-injected upstream for live tests.** The domain constructors take
  the **upstream `zbus::Connection` (or bus address) as a parameter** rather than
  hard-calling `Connection::system()`. Integration tests then stand up a **mock
  upstream** (a small zbus service implementing just the NM/BlueZ/UPower/logind
  methods and properties the domain reads/calls) on a **private bus**, register
  the icedtea domain interface pointed at it, and assert: `GetState` returns the
  mapped snapshot; a mock `PropertiesChanged`/signal drives a `Changed` with the
  new value; a command method (`SetSinkVolume`, `ConnectWifi`, `SetBrightness`)
  arrives at the mock as the right call. This is `shell/tests/live_dbus.rs`
  generalized to an injected upstream — and the same trait-mock seam
  (`AudioBackend`, like `WmCommands`) covers PipeWire, which cannot be faked on a
  bus.

- **Skips, made visible.** Tests that truly need a live PipeWire or real hardware
  print `SKIP: …` and return when unavailable, exactly as
  `shell/tests/live_dbus.rs:39` skips when the bus is down — never a silent pass.

- **Member-name drift guard.** The `client.rs` proxies are exercised against the
  real daemon in at least one live test per domain, so a PascalCase/snake_case
  member drift fails in CI (the bug class the shell's live test exists for).

## Risks

- **PipeWire integration is the heaviest lift** — `pipewire-rs` is a thin binding
  over a C mainloop, not async, and the object-registry model (nodes/ports/
  metadata) is intricate; per-app stream tracking especially. Mitigation: the
  `AudioBackend` trait isolates it, S1 is scoped so volume/mute/default lands
  before per-app streams, and the native `libpipewire` dep is feature-gated so it
  never blocks the other four domains' build/test.
- **polkit-gated actions.** Some NM/BlueZ actions and non-logind brightness
  writes may need interactive auth. The daemon-as-active-session default-allows
  the common cases; anything that isn't returns an error the UI surfaces. A real
  polkit agent is roadmap A8.
- **Optional/absent upstreams.** `power-profiles-daemon` is frequently not
  installed; a device may have no battery or no backlight. Every domain must
  degrade gracefully (empty/absent snapshot, command returns a clean error) and
  never fail daemon startup — S0 proves empty-state startup first.
- **Secrets.** Wi-Fi/VPN secret storage and a NM secret agent are deferred to A8;
  near-term connect passes secrets inline (NM persists them). BlueZ interactive
  pairing likewise deferred (NoInputNoOutput agent only).
- **System-bus availability in CI.** Integration tests inject a **private** bus +
  mock upstream so they don't need a real system NM/BlueZ/UPower; the visible-skip
  path covers environments without even a session bus.
- **Version churn** across PipeWire / NetworkManager / BlueZ D-Bus APIs — pin the
  proxy interface versions we target and document minimum versions, as the portal
  spec does for `portals.conf`.
- **Overlap with A3 (logind session).** Cleanly bounded: this spec consumes
  logind only for `SetBrightness` and consumes UPower for battery; A3 owns
  suspend/lid/idle/inhibitors. If A3 later also wants a system-bus logind proxy,
  the two share `contract/` vocabulary but keep separate object paths.

## Out of scope (for now)

- **All UI** — bar indicators (B5), quick-settings popover, OSD (B6), and the
  audio/network/bluetooth/power settings pages (B7). This spec stops at the D-Bus
  contract + Rust client the UIs will consume.
- **logind session actions** (suspend/hibernate/lid/idle-lock/inhibitors) —
  roadmap A3.
- **Secret service / keyring and a polkit agent** — roadmap A8; Wi-Fi secret
  agent, VPN import, and interactive BlueZ pairing ride along there.
- **Low-battery / device-connected notifications** — those are emitted by the
  notification daemon (A4) consuming this backend's `Changed` signals, not by the
  backend itself.
- **A general power-management policy engine** (auto-suspend thresholds, per-
  profile automation) — beyond the read/set model here.

## Next step

On approval, turn **S0 (skeleton + contract vocabulary + runtime + packaging)**
into an implementation plan (writing-plans). It is the concrete, toolkit-
independent first slice: it lands the `org.icedtea.System` surface, the shared
snapshot/emitter/command runtime, the signature-locked contract, and the systemd
unit — after which the five domains are independent, parallelizable milestones.
