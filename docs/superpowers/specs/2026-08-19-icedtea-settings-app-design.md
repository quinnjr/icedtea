# icedtea-wm — Configuration app (`icedtea-settings`) design

Date: 2026-08-19

## Overview

A GTK4 desktop application that lets a user view and edit the compositor's
configuration and apply it live. Today the config lives in a binary `redb`
database (`~/.config/icedtea/config.redb`) with no user-facing editor — the
only way to change settings is to write Rust against `icedtea-config`. This
milestone adds `icedtea-settings`: a windowed app that loads the config,
presents every editable setting (appearance, behavior, workspaces,
keybindings), writes changes via `Config::save`, and asks the running
compositor to re-read and re-apply them over D-Bus — no restart required.

The compositor **only reads** `config.redb` in production (boot + on-demand
reload; the only `Config::save` calls in the tree are in tests), so the
settings app can safely own writes with no clobber risk.

## Decisions of record

1. **Form factor: GTK4 GUI** (user decision). A new workspace crate
   `icedtea-settings` mirroring the existing `icedtea-shell` (gtk4 0.11 +
   `gtk4`-idioms + zbus + an async-channel→glib bridge). Not a CLI/TUI.
2. **Scope: all four config sections in v1** (user decision) — Appearance,
   Behavior, Workspaces, AND Keybindings (with a key-capture widget).
3. **Apply model: save-then-apply.** Edits mutate an in-memory working-copy
   `Config`; an **Apply** button writes it via `Config::save(&db)` and then
   calls `ReloadConfig` on `org.icedtea.WM`; a **Revert** button reloads from
   disk. A "changes pending" indicator tracks working-copy ≠ saved. Not
   live-write-on-every-keystroke (that would thrash the compositor). If the
   compositor is not running, Apply still writes the redb (it applies on next
   boot) and the reload is best-effort (surfaced as a non-fatal notice).
4. **Single source of truth for the key format.** The keybinding
   key-name↔keysym translation (`key_name_to_keysym`, `keysym_to_key_name`)
   and `MODIFIER_TOKENS` move from `compositor/src/input.rs` into the shared
   `icedtea-config` crate. The compositor and the settings app both use them,
   so a captured combo is **provably** in the exact `{modifiers, key}` string
   form the compositor's matcher expects — no drift. (This is the one change
   this milestone makes to existing crates; it is a pure move + re-import,
   behavior-preserving, covered by the compositor's existing input tests.)
5. **Separate a pure core from the GTK view** (the M4.6 lesson). Load →
   edit → save → reload, the KeyCombo translation, and duplicate-binding
   detection live in plain testable functions/types; the GTK widgets are a
   thin view over them. The core gets real unit tests; the apply path gets a
   live `ReloadConfig` round-trip test against the harness compositor.
6. **Execution: SDD**, per-task review, final whole-branch review, merge
   window — never auto-merged. No crate publish (this is icedtea-only; no
   `wlr` change).

## Architecture

New crate `settings/` (`icedtea-settings`), a GTK4 binary. Depends on
`icedtea-config` (schema + load/save + the moved keysym helpers),
`icedtea-contract` (`Appearance`/`Palette` types), `gtk4`, `zbus`,
`async-channel`, `redb`. Reuses `icedtea-shell`'s patterns:

- **Startup:** `Application` with an app id (e.g. `org.icedtea.Settings`);
  on activate, `load_or_default(default_db_path())` into a working-copy
  `Config`, build the window, populate widgets from the working copy.
- **Window:** a `StackSwitcher`/`Stack` (or `Notebook`) with four pages, plus
  a header/footer bar carrying **Revert**, **Apply**, and a pending-changes
  indicator.
- **D-Bus worker + bridge:** a worker thread owns the zbus connection and the
  `ReloadConfig` call on `org.icedtea.WM`; results cross to the GTK main thread
  via an `async-channel`→`glib` bridge (mirroring `shell/src/bridge.rs`,
  `wm_client.rs`). zbus `#[interface]` methods are PascalCase on the wire —
  the call is `ReloadConfig` (the M4.6 gotcha).

### The pure core (`settings/src/model.rs`, GTK-free)

- A working-copy `Config` plus a `dirty` flag and the last-saved snapshot for
  Revert.
- `apply(&Config, db_path) -> Result<(), Error>` — `Config::save` into the
  redb at `db_path` (create if missing).
- Keybinding capture translation: `combo_from_gtk(keyval: u32, mods:
  gdk::ModifierType) -> KeyCombo` — maps GTK modifier bits to the
  `MODIFIER_TOKENS` names and `keyval` (an xkb keysym) to `"KEY_<name>"` via
  the shared `icedtea_config::keysym_to_key_name`. (The `gdk` type is the only
  GTK touch in the core; it can be reduced to `(u32, mod-bitflags)` to keep the
  translation itself GTK-free and unit-testable.)
- `duplicate_bindings(&Config) -> Vec<(combo, Vec<action>)>` — finds combos
  bound to more than one action, for conflict highlighting.
- Hex-color validation and `bar_position` domain (`"top"`/`"bottom"`).

### The GTK view (`settings/src/pages/*.rs`, `main.rs`)

- **Appearance page:** `bar_position` `DropDown` (top/bottom);
  `bar_height`/`corner_radius`/`snap_gap` `SpinButton`s;
  `palette.{background,foreground,accent}` via `ColorDialogButton` (or a
  validated hex `Entry`); `wallpaper` `Button` opening a `FileDialog`, with a
  Clear that sets `None`.
- **Behavior page:** three `Switch`es bound to the three bools.
- **Workspaces page:** a list of name `Entry` rows with Add/Remove/reorder;
  Remove is disabled at one row (`load_or_default` rejects an empty list).
- **Keybindings page:** a scrolled list, one row per action (the fixed set
  `close`, `fullscreen`, `reload`, `quit`, `cycle:alt_tab`, `spawn:terminal`,
  `snap:{left,right,up,down,restore}`, plus generated `workspace:N` /
  `move_to_workspace:N` up to the workspace count). Each row shows the current
  combo and a **Set** button that enters capture mode (an
  `EventControllerKey` grabs the next non-modifier keypress + the active
  modifier mask, builds a `KeyCombo` via `combo_from_gtk`, and displays it).
  Rows whose combo collides with another action are highlighted
  (`duplicate_bindings`); an **Esc** cancels capture.

Every widget writes back into the working-copy `Config` and sets `dirty`.
**Apply** calls `core::apply(&working, db_path)` then dispatches `ReloadConfig`
through the worker; on success clears `dirty` and snapshots. **Revert**
reloads `load_or_default` and repopulates.

## Testing (automated-only where feasible)

- **Core unit tests (`settings/src/model.rs`):** save→reload round-trips a
  full `Config` through a temp redb (mirrors `config`'s own
  `save_then_load_round_trips`); `combo_from_gtk` produces `{modifiers, key}`
  strings that `icedtea_config::key_name_to_keysym` resolves to the SAME
  keysym the capture started from (this is the non-vacuous proof the format
  matches the compositor); `duplicate_bindings` finds a planted conflict;
  hex/`bar_position` validation accept/reject.
- **Shared-helper move:** the compositor's existing `input.rs` tests for
  `key_name_to_keysym`/`keysym_to_key_name` move/extend with the code into
  `config` and stay green (the move is behavior-preserving).
- **Live apply round-trip (`settings/tests/` or the harness):** spawn the
  harness compositor, write a changed `Config` (e.g. a new `accent`) via
  `core::apply`, call `ReloadConfig` on `org.icedtea.WM`, and assert the
  compositor emits `ConfigReloaded` carrying the new appearance — proving the
  full write→reload→apply path end to end. (Reuses the M4.6 harness that runs
  GTK/D-Bus clients against a headless compositor.)
- **GTK view logic:** where the M4.6 headless-GTK harness allows, a smoke
  test that the window builds and populates from a known config, and that a
  captured key updates the working copy. GTK-widget coverage is best-effort;
  the core + the live round-trip carry the load-bearing assertions.
- **Gates:** `cargo test --workspace` + `cargo clippy --all-targets -- -D
  warnings` green.

## Success criteria

1. `icedtea-settings` launches, loads the current config, and populates all
   four pages (Appearance, Behavior, Workspaces, Keybindings) from it.
2. Editing a setting and clicking **Apply** writes the redb and a running
   compositor re-applies it — proven automatically by a `ReloadConfig` →
   `ConfigReloaded` round-trip carrying the changed value.
3. A keybinding captured in the app serializes to a `{modifiers, key}` combo
   that the compositor's matcher resolves to the intended keysym (verified by
   the core test against `key_name_to_keysym`) — i.e. a rebind actually fires.
4. **Revert** discards unsaved edits back to the on-disk config; the
   pending-changes indicator reflects working-copy ≠ saved.
5. A missing or corrupt `config.redb` loads as defaults without panicking
   (inherited from `load_or_default`).
6. The `key_name_to_keysym`/`keysym_to_key_name`/`MODIFIER_TOKENS` move to
   `config` is behavior-preserving (compositor input tests green); no `wlr`
   change; all gates green; final review clean; merge window presented.

## Out of scope (explicit)

- Editing config that isn't in the `Config` schema (there is none beyond the
  four sections).
- A CLI/TUI front-end (a possible later addition over the same core).
- Live-preview-on-every-keystroke (save-then-apply only).
- Theming the settings app itself; packaging/installation (.desktop file,
  systemd unit) beyond building the binary — a possible follow-up.
- Multi-monitor/per-output config (the schema is global).
- Adding NEW settings to the schema (this app edits the existing schema; new
  settings are a compositor-side change first).

## Risks

- **Key-format drift (the load-bearing risk) — mitigated by design.** A
  captured combo must match what the compositor's matcher expects or a rebind
  silently won't fire. Moving `keysym_to_key_name`/`key_name_to_keysym` into
  the shared `config` crate makes the app and compositor use one translation;
  the core test asserts a captured keysym round-trips through the app's
  serialization back to the same keysym. GTK4 `keyval` is an xkb keysym, so
  the mapping is direct.
- **GTK testability.** GTK widgets resist unit testing; mitigated by the
  pure-core split (M4.6 pattern) — the load-bearing logic is GTK-free and
  tested; the live D-Bus round-trip proves the apply path without driving
  widgets.
- **Compositor not running.** Apply must still persist and degrade the reload
  gracefully (write succeeds, reload reported as "compositor not running,
  changes apply on next start") — not an error.
- **redb write vs. the compositor's read.** The compositor never writes the DB
  in production (verified), and `redb` transactions are ACID, so a save while
  the compositor holds a read is safe; the reload re-opens and reads the new
  committed state.
- **Wallpaper path / color validity.** Validate hex colors and treat an empty
  wallpaper as `None`; a bad path is the compositor's existing
  decode-failure-is-non-fatal path, not this app's concern to verify beyond
  writing what the user picked.

## Execution

Branch `settings-app` off `develop`. New crate `settings/` added to the
workspace `members`. Per-task review; final whole-branch review; merge window
presented for the user's go/no-go — never auto-merged. No crate publish.
