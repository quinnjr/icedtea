# B1 App Launcher — Design

**Date:** 2026-09-12
**Status:** approved in brainstorming; awaiting implementation plan.
**Parent:** `2026-08-20-icedtea-de-roadmap.md` (Track B, item B1 — "the highest
everyday-leverage surface and a signature piece of the DE's identity").
**Decisions of record:** full Win7/10 shape in one milestone; bottom-anchored
by default with a top option; launch via the compositor spawn action;
power row routes through the A3 `icedtea-session` CLI (logind backend);
dedicated layer-shell
surface (approach A — not a panel popup or in-bar expander).

## Goal

Give icedtea its first way to start apps besides a terminal: a Windows
7/10 Start-menu-inspired launcher — a bottom-left-anchored layer-shell
panel opened from a Start button on the bar, with search-as-you-type,
pinned + alphabetical All-apps, groupable tiles, recent/frequent apps,
and a power/session row. Keyboard-first, mouse-friendly, themeable via
the toolkit's GTK-CSS.

## Architecture

One new surface in the existing shell process (the panel's own pattern:
`wayland-client` + `wlr-layer-shell` + toolkit `App`):

- **Anchor:** bottom-left when `Appearance.bar_position` is `bottom`
  (the default), top-left when `top`. `Layer::Top`, no exclusive zone
  (overlay — never pushes windows around).
- **Focus:** takes keyboard focus on open; closes on Escape, on
  successful launch, or on Start-button toggle. There is deliberately no
  close-on-focus-loss path: as an exclusive-keyboard layer-shell surface
  the launcher receives no focus-loss event a client can observe, so
  focus loss cannot be a close trigger. A compositor-driven unmap or
  kill is covered by the supervisor's done-channel reap, which frees the
  menu slot exactly as a self-dismissal would.
- **Start button:** joins the bar's left end and toggles the launcher.
  The bar keeps its existing rest-state gates; the launcher gets its own
  open/closed/rest gates.
- **Pure core** (`shell/src/launcher/`, no Wayland): the `.desktop`
  index, the fuzzy matcher, and the pin/tile/recency stores. The view
  renders what the core returns; every behavior is unit-testable
  without a compositor.

## Position config

`Appearance.bar_position` already exists in the contract schema. This
spec wires it (verifying first whether anything honors it yet):
`bottom` → bar bottom-anchored + launcher bottom-left; `top` → bar top
+ launcher top-left. The launcher anchor derives from the same value —
never its own setting. One knob, two surfaces follow.

## Index + stores

- **Index:** XDG `.desktop` scan (system + user dirs), honoring
  `NoDisplay`, `OnlyShowIn`/`NotShowIn`, localized `Name`,   `Exec`
  (field codes stripped for display via the shell's `display_exec`;
  verified against the compositor lookup path — `lookup_app_exec`
  strips field codes via `strip_exec_field_codes` before spawn,
  mirroring the display rule with `%%` unescaped),
  `Icon`, `Categories`. Rescanned on open with an mtime cache so
  steady-state opens do no I/O.
- **Matcher:** fuzzy match over name + keywords + exec basename;
  Lucene-style scoring is overkill — substring + word-boundary +
  prefix bonuses, ranked with recent/frequent as tiebreak signal.
- **Stores** (config schema additions beside `bar_position`):
  pinned app-id list (ordered), tile groups (ordered ids + sizes),
  recency/frequency counts (capped, pruned).

## Layout + keyboard model

Win7 left rail + Win10 right pane:

- **Search box** on top (toolkit `SearchEntry`, IME-capable from M6.1
  Spec 2) with the top hit styled `.suggested-action`.
- **Left rail:** pinned apps, then scrollable alphabetical All-apps
  with single-letter jump.
- **Right pane:** resizable, groupable tiles (pin-to-Start from any
  row) doubling as quick links (home folder, settings).
- **Bottom row:** lock / log out / suspend / restart / shut down.
- **Keyboard:** open focuses search; typing filters all lists; arrows
  move within and across panes; Enter launches; Escape closes.
- **Mouse:** click launches; right-click pins/unpins.
- **Theming:** existing GTK-CSS node vocabulary (`window`, `button`,
  `entry`, `.suggested-action`) — no new theme machinery.

## Spawn + power backends

- **Launch** is a D-Bus call to the compositor's `SpawnApp` method
  which takes an app id; the compositor resolves it against its own
  index copy and execs the result directly (per the M6.0
  `parse_spawn_argv` + `Command::spawn` path) — no argv crosses the bus
  (see the `apply_action` hardening ban on wiring `spawn` to D-Bus).
  Recency is recorded on success.
- **Power row** invokes the A3 `icedtea-session` CLI (`lock`, `suspend`,
  `reboot`, `poweroff`); log out takes the compositor quit path. The
  program is resolved once per process to an absolute path rather than
  trusting a click-time `PATH` lookup (the session crate installs it at
  `%h/.cargo/bin/icedtea-session`). Every failure surfaces as a status
  line in the launcher (never silent); lock policy (configured locker +
  session-lock path) lives in the session daemon, not here.

## Testing

- **Pure-core unit tests:** `.desktop` parse (all key types, locales,
  `NoDisplay` filtering), match ranking order, pin/tile/recency ops.
- **Offscreen render tests** (the M1/M5 pattern): rest state per theme;
  open → search → launch dismissal states.
- **Harness e2e** (both sides, real relay): open from Start, type a
  query, launch — assert the spawn request carries the app id (never
  argv — no argv crosses the bus) and recency recorded; power row dry-run asserts the exact argv that
  would run. Deletion-tested: gutting the spawn call fails the e2e.
- **Gates:** `cargo test --workspace -- --test-threads=1`,
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Out of scope (explicit)

- Files/settings search providers (search box is apps-only in v1;
  the matcher interface admits more providers later).
- Tile drag-reorder (pin/unpin + group assignment suffice; reorder is
  a follow-up, possibly riding the M6 DnD work).
- Usage-based adaptive ordering beyond recency/frequency tiebreaks.
- The logind backend implementation itself (owned by the A3 `session`
  crate; this launcher only invokes its `icedtea-session` CLI).
- Session restore / "reopen last apps".
- A separate launcher process (same-process surface keeps one
  event loop, one config handle, one D-Bus connection).

## Deferred work (B1 follow-up)

The three launcher items v1 left out. All live in the pure core
(`shell/src/launcher/`) or the view (`shell/src/launcher_view.rs`); none adds
a D-Bus method, a config table, or a dependency, and pin/unpin is untouched.

### Non-app search providers

A minimal seam: `launcher::provider::SearchProvider` (`kind` + `search`) over
`SearchResult { kind, id, title, subtitle, score, weight }`, with
`ProviderKind = App | Settings | File` and a pure `rank_results` that sorts by
score desc, weight desc, kind tie-rank (App, Settings, File), then
case-insensitive title, then id — so every provider merges into one
deterministic ranked list. A non-empty search query renders that list in the
All pane; the pinned rail and tiles stay app-only.

- **Files** (`FilesProvider`): a bounded, read-only index of
  `$XDG_DESKTOP_DIR` / `$XDG_DOCUMENTS_DIR` / `$XDG_DOWNLOAD_DIR` (falling
  back to `$HOME/Desktop|Documents|Downloads`) plus
  `$XDG_DATA_HOME/recently-used.xbel`, parsed by `parse_recent`
  (`href="file://…"` → `%XX`-decoded path, capped). Entries cap at
  `MAX_FILE_RESULTS`, newest-mtime first, which is also the ranking weight.
  Selecting a file opens it with the desktop's registered handler through a
  local `xdg-open` helper — the power row's own local-helper pattern. The
  compositor `SpawnApp` path is app-id-only and is deliberately **not**
  extended to carry a path (no raw argv over D-Bus).
- **Settings** (`SettingsProvider`): the settings pages as results whose id is
  the page name; selecting one asks the compositor to spawn
  `org.icedtea.Settings`. The page list is mirrored here (SYNC note) because
  the shell crate does not depend on `icedtea-settings`.

Tests: each provider's parse/query (recent-xbel parse, page/name match,
`scan` bounding), and a merge/rank test that pins the cross-provider order.

### Tile drag-reorder

Tiles ride the M6 toolkit DnD (`ui/src/dnd.rs`) directly: each tile is a
`GenericC` node (`Kind::ListBoxRow`, M6-D2's source/target kind) carrying
`DragSource` = app id and `DropAccept = "text/plain"`, plus `on_drop`. A drop
folds `LauncherMsg::ReorderTile { source, target }`; `TileStore::reorder`
removes the source from its group and inserts it at the target's slot in the
target's group (cross-group move included; a group left empty is pruned). The
order round-trips through the existing `LauncherConfig.tile_groups` on the
next close write-back, exactly like pin/unpin. Dragging arms M6's drag latch,
so that release is a drop, never a click.

Tests: `TileStore::reorder` unit tests (within-group, cross-group, no-op) plus
an integration test that reorders, closes, reloads the saved config, and
asserts the re-seeded view renders the new order.

### Adaptive ordering

The recency/frequency tiebreak becomes a pure `RecencyStore::adaptive_score`:
`weight = count * (DECAY_WINDOW + 1) / (DECAY_WINDOW + age)`, `DECAY_WINDOW = 32`,
`age = seq - last_seen` (saturating). Integer-only and deterministic: frequency
lifts the weight, staleness decays it, a never-launched app scores 0. `Matcher`
uses it as the score-tie tiebreak in place of the raw count.

Tests: formula unit tests (frequency doubles at equal age; recent beats stale
at equal count; a stale single-use decays to 0) and the matcher ordering.

## Risks

- **Fuzzy quality on huge app sets** — hundreds of `.desktop` files
  with similar names (games, Wine prefixes). Mitigation: prefix +
  word-boundary bonuses, recency tiebreak, and a ranking unit-test
  corpus drawn from the real system set.
- **Stale index** — apps installed while the shell runs. Mitigation:
  mtime-gated rescan on every open (cheap when nothing changed).
- **Power-row failure modes** — `icedtea-session` absent, or the
  session daemon unreachable / logind refusing.
  Mitigation: failure as status line, never silent.
- **`bar_position` may be live already** — if something honors it,
  this spec's wiring must merge with, not fork, that path. Flagged as
  the implementer's first verification step.
