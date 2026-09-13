# B1 App Launcher — Design

**Date:** 2026-09-12
**Status:** approved in brainstorming; awaiting implementation plan.
**Parent:** `2026-08-20-icedtea-de-roadmap.md` (Track B, item B1 — "the highest
everyday-leverage surface and a signature piece of the DE's identity").
**Decisions of record:** full Win7/10 shape in one milestone; bottom-anchored
by default with a top option; launch via the compositor spawn action;
power row shells out until A3 logind replaces it; dedicated layer-shell
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
  focus-loss to a normal window, or on successful launch.
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
  `NoDisplay`, `OnlyShowIn`/`NotShowIn`, localized `Name`, `Exec`
  (with field codes stripped for display, expanded for launch),
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

- **Launch** is a D-Bus call to the compositor's spawn action with the
  entry's argv (exec'd directly per the M6.0 path). Recency is recorded
  on success.
- **Power row** shells out (`loginctl lock-session`, session quit via
  the compositor quit path, `systemctl suspend/poweroff/reboot`).
  Every failure surfaces as a status line in the launcher (never
  silent), and every call site is marked for A3-logind replacement.
  Lock prefers the compositor session-lock path where available.

## Testing

- **Pure-core unit tests:** `.desktop` parse (all key types, locales,
  `NoDisplay` filtering), match ranking order, pin/tile/recency ops.
- **Offscreen render tests** (the M1/M5 pattern): rest state per theme;
  open → search → launch dismissal states.
- **Harness e2e** (both sides, real relay): open from Start, type a
  query, launch — assert the spawn request carries the entry's argv
  and recency recorded; power row dry-run asserts the exact argv that
  would run. Deletion-tested: gutting the spawn call fails the e2e.
- **Gates:** `cargo test --workspace -- --test-threads=1`,
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Out of scope (explicit)

- Files/settings search providers (search box is apps-only in v1;
  the matcher interface admits more providers later).
- Tile drag-reorder (pin/unpin + group assignment suffice; reorder is
  a follow-up, possibly riding the M6 DnD work).
- Usage-based adaptive ordering beyond recency/frequency tiebreaks.
- A3 logind backends (power row is shell-out with marked call sites).
- Session restore / "reopen last apps".
- A separate launcher process (same-process surface keeps one
  event loop, one config handle, one D-Bus connection).

## Risks

- **Fuzzy quality on huge app sets** — hundreds of `.desktop` files
  with similar names (games, Wine prefixes). Mitigation: prefix +
  word-boundary bonuses, recency tiebreak, and a ranking unit-test
  corpus drawn from the real system set.
- **Stale index** — apps installed while the shell runs. Mitigation:
  mtime-gated rescan on every open (cheap when nothing changed).
- **Power-row failure modes** — `systemctl` absent or polkit-denied.
  Mitigation: failure as status line, never silent; A3 replaces all
  of it.
- **`bar_position` may be live already** — if something honors it,
  this spec's wiring must merge with, not fork, that path. Flagged as
  the implementer's first verification step.
