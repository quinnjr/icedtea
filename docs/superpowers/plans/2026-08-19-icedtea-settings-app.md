# icedtea-settings (config app) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A GTK4 app (`icedtea-settings`) that edits every config section and applies changes live via `Config::save` + a D-Bus `ReloadConfig`.

**Architecture:** A new `settings/` workspace crate mirroring `icedtea-shell` (gtk4 + zbus + async-channel bridge). A GTK-free pure core (working-copy `Config`, save, keybinding-capture translation, conflict detection) under a thin GTK view. The keybinding key-name↔keysym helpers move into the shared `icedtea-config` crate so a captured combo is provably in the compositor's expected form.

**Tech Stack:** Rust; GTK4 0.11 (`gtk4`); `zbus`; `async-channel`; `icedtea-config`; `icedtea-contract`; `redb`; `xkbcommon`.

**Spec:** `docs/superpowers/specs/2026-08-19-icedtea-settings-app-design.md`

## Global Constraints

- **No `wlr` change, no crate publish** — icedtea-only milestone.
- **One existing-crate change only:** the keysym helpers move from `compositor/src/input.rs` to `icedtea-config` (Task 1), behavior-preserving, re-exported so compositor callers are unchanged.
- **Gates:** `cargo test --workspace -- --test-threads=1` + `cargo clippy --all-targets -- -D warnings` green. (The intermittent `icedtea-clipboard` teardown abort under full-workspace parallelism is a known pre-existing flake — scope to the touched crates + clippy if hit.)
- **Commit trailer, every commit:** `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- **zbus wire names are PascalCase** — the reload call is `ReloadConfig` on `org.icedtea.WM` (the M4.6 gotcha).
- Branch `settings-app` off `develop` (no remote; local `--no-ff` merge at the end). `rtk proxy` before `cargo` if output looks summarized. Package names are `icedtea-*`.

## Reference anchors

- Move source: `compositor/src/input.rs` — `MODIFIER_TOKENS` (:19), `key_name_to_keysym` (:35), `keysym_to_key_name` (:50), and their `#[test]`s (:444+). Compositor callers use `input::key_name_to_keysym` (state.rs tests) — keep working via a re-export.
- Config API: `icedtea_config::{load_or_default(&Path)->Config, default_db_path()->PathBuf}`; `Config::save(&Database)->Result<(),redb::Error>`; `redb::Database::create(path)`. Schema: `Config{keybindings: HashMap<String,KeyCombo>, appearance: Appearance, behavior: Behavior, workspace_names: Vec<String>}`; `KeyCombo{modifiers: Vec<String>, key: String}`; `Appearance{bar_position: String, bar_height/corner_radius/snap_gap: i32, palette: Palette{background,foreground,accent: String}, wallpaper: Option<String>}` (in `icedtea-contract`); `Behavior{raise_on_focus, hide_bar_on_fullscreen, snap_enabled: bool}`.
- Default actions (`config/src/defaults.rs`): `close, fullscreen, reload, quit, cycle:alt_tab, spawn:terminal, snap:{left,right,up,down,restore}`, generated `workspace:N` / `move_to_workspace:N`.
- Shell scaffolding to mirror: `shell/src/bridge.rs` (`channel<T>(on_main) -> Sender<T>`), `shell/src/wm_client.rs` (`WmProxy`: `zbus::blocking::Connection::session()`, `conn.call_method(Some(WM_BUS_NAME), WM_PATH, Some(WM_IFACE="org.icedtea.WM"), "MethodPascalCase", &args)`), `shell/src/main.rs` (`Application::builder().application_id(...).build()`, `connect_activate`).
- Reload D-Bus: `compositor/src/dbus.rs:188` `reload_config` → wire `ReloadConfig` (no args); emits `Event::ConfigReloaded(Appearance)` signal `ConfigReloaded`.

---

### Task 1: Move keysym/modifier helpers into `icedtea-config` (shared source of truth)

**Files:** Modify `config/Cargo.toml` (add `xkbcommon`), `config/src/lib.rs` (add the moved items + tests, or a new `config/src/keys.rs` module), `compositor/src/input.rs` (delete the 3 items, `pub use` them from config), `compositor/Cargo.toml` (unchanged — already has xkbcommon).

**Interfaces:** Produces `icedtea_config::{MODIFIER_TOKENS, key_name_to_keysym, keysym_to_key_name}` (consumed by Task 2's core and re-exported by the compositor).

- [ ] **Step 1: Add `xkbcommon` to `config/Cargo.toml`** (match the compositor's version, `xkbcommon = "0.8"`).
- [ ] **Step 2: Move the three items** — `MODIFIER_TOKENS`, `key_name_to_keysym`, `keysym_to_key_name` — verbatim (with their doc comments) from `compositor/src/input.rs` into `icedtea-config` (a new `config/src/keys.rs` re-exported from `lib.rs`, or directly in `lib.rs`). Move their unit tests too (the `#[test]`s at input.rs:444+ that exercise these three — leave any tests that exercise the *matcher* or `Modifiers` bitflags in input.rs).
- [ ] **Step 3: Re-export from the compositor.** In `compositor/src/input.rs`, replace the removed definitions with `pub use icedtea_config::{MODIFIER_TOKENS, key_name_to_keysym, keysym_to_key_name};` so every existing caller (`input::key_name_to_keysym`, etc.) compiles unchanged.
- [ ] **Step 4: Verify behavior-preserving.** `cargo test -p icedtea-config` (moved tests pass) and `cargo test -p icedtea-compositor` (input matcher + the state.rs tests using `input::key_name_to_keysym` still green). `cargo build --workspace`. clippy clean.
- [ ] **Step 5: Commit.**

```bash
git add config/Cargo.toml config/src/ compositor/src/input.rs
git commit -m "$(printf 'refactor(config): move keysym/modifier helpers from compositor input\n\nkey_name_to_keysym / keysym_to_key_name / MODIFIER_TOKENS move to the\nshared icedtea-config crate (single source of truth for the KeyCombo\nformat); compositor input.rs re-exports them. Behavior-preserving.\n\nCo-Authored-By: Claude Fable 5 <noreply@anthropic.com>')"
```

---

### Task 2: `icedtea-settings` crate + pure core (GTK-free) + unit tests

**Files:** Create `settings/Cargo.toml`, `settings/src/lib.rs`, `settings/src/model.rs`. Modify root `Cargo.toml` (`members`).

**Interfaces:** Produces the core `model` API consumed by every later task: the working-copy state, `apply`, `combo_from_keysym`, `duplicate_bindings`, validation.

- [ ] **Step 1: Add the crate.** Root `Cargo.toml` `members` gains `"settings"`. `settings/Cargo.toml`:

```toml
[package]
name = "icedtea-settings"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "icedtea-settings"
path = "src/main.rs"

[dependencies]
icedtea-config = { path = "../config" }
icedtea-contract = { path = "../contract" }
gtk4 = "0.11"
zbus.workspace = true
async-channel = "2"
redb.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
icedtea-harness = { path = "../harness" }
```

(Add `src/main.rs` as a stub `fn main() {}` in this task so the bin target builds; Task 4 fills it.)

- [ ] **Step 2: `model.rs` — working-copy state.** A `Model` holding the working-copy `Config`, a `saved: Config` snapshot, and `is_dirty(&self) -> bool` (`self.working != self.saved`). `Model::load(db_path) -> Self` via `load_or_default`; `Model::revert(&mut self, db_path)` reloads. `Config` derives `PartialEq` already (confirm; it does — the D-Bus test uses it).
- [ ] **Step 3: `apply`.** `pub fn apply(cfg: &Config, db_path: &Path) -> Result<(), redb::Error>` — `let db = Database::create(db_path)?; cfg.save(&db)`. On success the caller snapshots `saved = working`.
- [ ] **Step 4: Keybinding capture translation (the load-bearing one).** `pub fn combo_from_keysym(keysym: u32, modifiers: CaptureMods) -> Option<KeyCombo>` where `CaptureMods` is a plain `{ ctrl, alt, shift, logo: bool }` (GTK-free — the GTK view converts `gdk::ModifierType` to this). Build `modifiers: Vec<String>` from the set bits using the `MODIFIER_TOKENS` spelling (`"SUPER"` for logo, `"CTRL"`, `"ALT"`, `"SHIFT"`), and `key: keysym_to_key_name(keysym)`. Return `None` if the keysym is a bare modifier (so a lone Shift press doesn't bind).
- [ ] **Step 5: Conflict + validation helpers.** `pub fn duplicate_bindings(cfg: &Config) -> Vec<(KeyCombo, Vec<String>)>` (combos bound to >1 action). `pub fn valid_hex(s: &str) -> bool` (`#RRGGBB`). `pub const BAR_POSITIONS: [&str; 2] = ["top", "bottom"]`.
- [ ] **Step 6: Unit tests** (`settings/src/model.rs` `mod tests`):
  - `save_then_load_round_trips`: build a non-default `Config`, `apply` to a temp redb, `load_or_default` it back, assert equal (mirror `config`'s own round-trip test; use `tempfile` or a unique temp path).
  - `combo_round_trips_to_the_compositor_format` (NON-VACUOUS, the key-format proof): for a set of keysyms (e.g. `key_name_to_keysym("KEY_q")`, `"KEY_Return"`, `"KEY_F5"`), call `combo_from_keysym(sym, mods)` and assert `key_name_to_keysym(&combo.key) == sym` — i.e. the app's serialization resolves back to the SAME keysym the compositor's matcher would use. Also assert the modifier tokens are all in `MODIFIER_TOKENS`.
  - `lone_modifier_does_not_bind`: `combo_from_keysym(<Shift keysym>, ...)` is `None`.
  - `duplicate_bindings_finds_a_planted_conflict`.
  - `valid_hex` accept/reject; `BAR_POSITIONS` domain.
- [ ] **Step 7: Gates + commit.** `cargo test -p icedtea-settings`, clippy. Commit `feat(settings): crate scaffold + pure config-editing core`.

---

### Task 3: D-Bus reload client + worker→GTK bridge

**Files:** Create `settings/src/bridge.rs`, `settings/src/wm_reload.rs`.

**Interfaces:** Produces `bridge::channel` and a `ReloadClient` (`reload() -> ReloadOutcome`) used by Task 4's Apply.

- [ ] **Step 1: `bridge.rs`** — copy `shell/src/bridge.rs` verbatim (`pub fn channel<T: 'static>(on_main) -> async_channel::Sender<T>`).
- [ ] **Step 2: `wm_reload.rs`** — a `ReloadClient` mirroring `WmProxy`:

```rust
const WM_BUS_NAME: &str = "org.icedtea.WM";
const WM_PATH: &str = "/org/icedtea/WM";     // confirm against shell/src/wm_client.rs
const WM_IFACE: &str = "org.icedtea.WM";

pub enum ReloadOutcome { Reloaded, CompositorAbsent }

pub struct ReloadClient { conn: Option<zbus::blocking::Connection> }

impl ReloadClient {
    pub fn new() -> Self {
        ReloadClient { conn: zbus::blocking::Connection::session().ok() }
    }
    /// Ask the running compositor to re-read config. Best-effort: if the bus
    /// or the WM service is absent, report `CompositorAbsent` (the redb write
    /// already happened; the change applies on next compositor start).
    pub fn reload(&self) -> ReloadOutcome {
        let Some(conn) = &self.conn else { return ReloadOutcome::CompositorAbsent };
        match conn.call_method(Some(WM_BUS_NAME), WM_PATH, Some(WM_IFACE), "ReloadConfig", &()) {
            Ok(_) => ReloadOutcome::Reloaded,
            Err(_) => ReloadOutcome::CompositorAbsent,
        }
    }
}
```

Confirm `WM_PATH` and `WM_BUS_NAME` against `shell/src/wm_client.rs`'s constants (reuse the exact strings).

- [ ] **Step 3: `apply_and_reload` convenience** (in `model.rs` or here): `pub fn apply_and_reload(cfg: &Config, db_path: &Path, client: &ReloadClient) -> Result<ReloadOutcome, redb::Error>` — `apply(cfg, db_path)?; Ok(client.reload())`.
- [ ] **Step 4: Gates + commit.** clippy; a trivial test that `ReloadClient::new()` doesn't panic without a bus. Commit `feat(settings): D-Bus ReloadConfig client + worker bridge`.

---

### Task 4: GTK app shell + Appearance + Behavior pages

**Files:** Create `settings/src/main.rs`, `settings/src/pages/mod.rs`, `settings/src/pages/appearance.rs`, `settings/src/pages/behavior.rs`.

**Interfaces:** Consumes `model` + `ReloadClient`. Produces the running window.

- [ ] **Step 1: `main.rs`** — `Application::builder().application_id("org.icedtea.Settings").build()`; `connect_activate(build_window)`. `build_window`: `Model::load(default_db_path())` behind an `Rc<RefCell<Model>>`; an `ApplicationWindow` with a `Box` containing a `StackSwitcher`+`Stack` (pages) and a footer `Box` with a dirty `Label`, **Revert** `Button`, **Apply** `Button`. Apply: `apply_and_reload(&model.working, &db_path, &client)` → on `Ok(Reloaded)` clear dirty + snapshot; on `Ok(CompositorAbsent)` clear dirty + show "saved; will apply when the compositor starts"; on `Err` show the error. Revert: `model.revert()` + repopulate pages. Wire a shared `mark_dirty()` closure that updates the label + enables Apply/Revert.
- [ ] **Step 2: Appearance page** (`appearance.rs`): build a `Grid`/`Box` with: `bar_position` `DropDown::from_strings(&BAR_POSITIONS)` (select current); `bar_height`/`corner_radius`/`snap_gap` `SpinButton`s (sane ranges, e.g. 0–256); `palette.background/foreground/accent` via `ColorDialogButton` (parse/emit `#RRGGBB`; validate with `valid_hex` on manual entry if using an `Entry` instead); `wallpaper` a `Button` opening `gtk4::FileDialog` (sets `Some(path)`) + a Clear button (`None`). Every widget's `connect_*` handler writes into `model.working.appearance.*` and calls `mark_dirty()`.
- [ ] **Step 3: Behavior page** (`behavior.rs`): three `Switch` rows for `raise_on_focus`, `hide_bar_on_fullscreen`, `snap_enabled`; `connect_state_set` writes the bool + `mark_dirty()`.
- [ ] **Step 4: Populate-from-model helper** so Revert can repopulate (a `refresh()` per page reading `model.working`). Guard against the write-back handlers firing during programmatic repopulation (a `populating` flag) so Revert doesn't re-mark dirty.
- [ ] **Step 5: Gates + commit.** `cargo build -p icedtea-settings`; `cargo clippy`. (Running the GUI is manual; if the M4.6 headless-GTK harness is usable, add a smoke test that `build_window` populates from a known config — else defer to the manual note.) Commit `feat(settings): GTK window + Appearance + Behavior pages`.

---

### Task 5: Workspaces + Keybindings pages (with key-capture)

**Files:** Create `settings/src/pages/workspaces.rs`, `settings/src/pages/keybindings.rs`.

- [ ] **Step 1: Workspaces page:** a vertical list of rows, each an `Entry` (the name) + a Remove `Button`; an Add `Button` appends a new row. `connect_changed` writes `model.working.workspace_names` + `mark_dirty()`. Remove is insensitive when only one row remains (`load_or_default` rejects empty). Reorder (up/down buttons) optional; if included, keep names + the generated `workspace:N` binding rows consistent.
- [ ] **Step 2: Keybindings page:** a `ScrolledWindow` over a list, one row per action. The action set = the fixed actions (`close, fullscreen, reload, quit, cycle:alt_tab, spawn:terminal, snap:{left,right,up,down,restore}`) plus generated `workspace:N`/`move_to_workspace:N` for `1..=workspace_names.len()`. Each row: an action `Label`, a combo `Label` (current `KeyCombo` rendered `SUPER+SHIFT+q`), and a **Set** `Button`.
- [ ] **Step 3: Key capture:** clicking **Set** puts the row in capture mode — attach a `gtk4::EventControllerKey` (to the window or a modal capture area), grab keyboard focus, and on the next key-pressed event: read `keyval` (an xkb keysym) and the `gdk::ModifierType` state, convert the modifier bits to `CaptureMods { ctrl, alt, shift, logo }`, call `combo_from_keysym(keyval, mods)`. If `Some(combo)`, write it into `model.working.keybindings[action]`, update the combo label, `mark_dirty()`, and end capture. If `None` (lone modifier), stay in capture. `Esc` cancels capture without changing the binding.
- [ ] **Step 4: Conflict highlight:** after any binding change, run `duplicate_bindings(&model.working)` and add a CSS class (e.g. a red label) to every row whose combo collides; clear it when resolved. Purely advisory — do not block Apply (the compositor tolerates duplicate bindings; last-writer semantics are its concern).
- [ ] **Step 5: refresh() + repopulate** for Revert, same `populating` guard as Task 4.
- [ ] **Step 6: Gates + commit.** build + clippy. Commit `feat(settings): Workspaces + Keybindings pages (key-capture + conflict view)`.

---

### Task 6: Live apply round-trip test (write → ReloadConfig → ConfigReloaded)

**Files:** Create `settings/tests/live_apply.rs` (mirrors `shell/tests/live_dbus.rs` from M4.6).

**Interfaces:** Consumes the harness compositor + the settings core.

- [ ] **Step 1: The test.** Spawn the harness compositor (`icedtea_harness::Compositor::spawn`) pointed at a temp `config.redb` (use the harness's config-path hook — `State::config_path`/`config_path` env, see `state.rs:2303`; if the harness doesn't expose one, set `XDG_CONFIG_HOME` to a temp dir before spawn so `default_db_path()` resolves there). Load the config, change `appearance.palette.accent` to a known new value, `model::apply(&cfg, &db_path)`, then call `ReloadConfig` on `org.icedtea.WM` (via `ReloadClient` or a raw zbus call), and assert the compositor emits a `ConfigReloaded` signal whose `Appearance` carries the new accent (subscribe to the `org.icedtea.WM` `ConfigReloaded` signal the way `shell/src/wm_client.rs` subscribes to WM signals). 

```rust
/// Editing the config and asking the compositor to reload applies it live:
/// a changed accent round-trips through save -> ReloadConfig -> ConfigReloaded.
#[test]
fn apply_reloads_the_running_compositor() {
    // temp XDG_CONFIG_HOME so default_db_path() -> temp/icedtea/config.redb
    // spawn harness compositor (reads that path at boot)
    // subscribe to org.icedtea.WM ConfigReloaded
    // load_or_default -> set accent -> apply -> ReloadConfig
    // assert a ConfigReloaded arrives with appearance.palette.accent == new
}
```

**Non-vacuity:** assert the accent in the received `ConfigReloaded` equals the NEW value (not the default) — a no-op reload or a dropped write fails it. If subscribing to the signal is fiddly, the fallback is to call the WM `GetState`/appearance accessor after reload and assert the new accent — still end-to-end, still non-vacuous.

- [ ] **Step 2: Gates + commit.** `cargo test -p icedtea-settings --test live_apply -- --test-threads=1` green; workspace test + clippy. Commit `test(settings): live apply round-trip (ReloadConfig -> ConfigReloaded)`.

---

## After the last task

- **Final whole-branch review** on the most capable available model (crate correctness, the keysym move's behavior-preservation, the key-format round-trip, the apply path, no unintended compositor behavior change).
- **Merge window** for the user's go/no-go — never auto-merged. Local `--no-ff` merge into `develop`. No crate publish.
- Note the deferred follow-ups (packaging `.desktop`/install; a CLI over the same core; reorder UX) for the user.
