# B3 Notification Popups + Center — Design

**Date:** 2026-09-15
**Status:** approved in brainstorming; awaiting implementation plan.
**Parent:** `2026-08-20-icedtea-de-roadmap.md` (Track B, item B3 — notification
popups; the daemon is Track A item A4, already shipped).
**Decisions of record:** popups + center in one milestone (daemon's
`GetActive`/`GetHistory`/DND already exist); standard auto-dismiss with
urgency-scaled timeouts; dedicated top-right overlay surface (approach A —
not per-notification popups, not in-panel).

## Goal

Make notifications visible: the A4 daemon accepts and stores them today,
but nothing renders them. A top-right popup stack plus a notification
center closes one of the roadmap's four hardest daily-driver blockers.

## Architecture

One new surface in the existing shell process (the launcher-surface
pattern: `wayland-client` + `wlr-layer-shell` + toolkit `App`):

- **Surface:** `Layer::Top`, top-right anchor, no exclusive zone
  (overlay — never pushes windows), no keyboard (pointer-only
  dismissal; actions are buttons). Own `App` with stack/center states
  and its own rest gates; the bar is untouched.
- **Data flow:** cold start reads `GetActive`; live updates come from
  the daemon's `Added`/`Closed`/`ActionInvoked`/`DndChanged` signals.
  The daemon stays the single source of truth (expiry/min-heap
  server-side); the shell caches nothing beyond the current frame's
  model.
- **Center + DND:** a bell icon joins the bar's right end (badge =
  active count). Clicking toggles the same surface into center mode: a
  scrollable `GetHistory` list with clear-all, and a DND toggle bound
  to `get_do_not_disturb`/the DND signal. DND suppresses popups; the
  center still lists.

## Interaction, timeouts, theming

- Popup: icon + app name + summary + body + action buttons + close ×.
- Auto-dismiss per urgency (low ~5s, normal ~8s, critical ~20s with
  accent border), paused on hover. Click body = dismiss via
  `CloseNotification(id)`; action click = `InvokeAction(id, key)`,
  then dismiss.
- Styling reuses the GTK-CSS vocabulary (`notification`, `.critical`,
  buttons) — no new theme machinery.

## Testing

- Shell unit tests: stack ordering, timeout mapping, DND gating.
- Offscreen render tests per theme (popup + center states).
- Harness e2e: `notify-send` → popup with body/actions → click action
  → assert `ActionInvoked`; DND on → no popup, center lists.
  Deletion-tested: gutting the signal subscription fails the e2e.
- Gates: `cargo test --workspace -- --test-threads=1`,
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Out of scope (explicit)

- Per-app notification policy settings (that's B7 settings breadth).
- Notification sounds.
- Inline replies (needs a text entry + a new protocol round-trip).
- Persistence beyond the daemon's existing history.

## Risks

- **Signal/disconnect races** — daemon restarts or missed signals
  leaving a stale stack; mitigated by re-reading `GetActive` on
  reconnect and on every center open.
- **Popup storms** — a chatty app flooding the stack; mitigated by
  per-app rate limiting (cap visible popups, overflow goes to center
  only) — simple count cap in v1.
- **Focus theft** — popups take no keyboard and never raise; pointer
  only, by construction.
