# icedtea Compositor Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bootable Smithay floating WM that owns windows, workspaces, focus, SSD title bars, wallpaper, edge snapping, and an `org.icedtea.WM` DBus service — with no shell yet.

**Architecture:** One Cargo workspace of three crates. `contract` holds the DBus wire types shared by every process. `config` owns the redb schema and default-fallback loading. `compositor` is the Smithay binary: it renders via smithay's GL renderer, drives input with libinput (real) or the nested wayland backend (dev/tests), keeps window state in a pure `WindowManager` model, and exposes control through a zbus `org.icedtea.WM` session-bus service bridged to the compositor's calloop loop by a channel pair.

**Tech Stack:** Rust 1.94 (MSRV), `smithay = "0.7"` (DRM + libinput + nested-wayland backends, GL renderer, wayland-frontend, layer-shell, xdg-decoration features), `zbus = "5"` (blocking API, dedicated thread), `redb = "3"`, `serde`/`serde_json`, `crossbeam-channel`, `tracing`.

**Reference skeleton:** Smithay's own sample compositor **`anvil`** (in the smithay repo, pinned to the same 0.7 tag). Every smithay-glue step cites the exact anvil file to copy from. Where a signature differs from this plan, follow the pinned anvil source — it is the ground truth for the 0.7 API.

## Global Constraints

- MSRV 1.94: `rust-version = "1.94"` in the workspace root and every crate. Edition 2024.
- Versions (workspace deps, pinned): `smithay = "0.7"`, `zbus = "5"`, `redb = "3"`, `serde = "1"`, `serde_json = "1"`, `tracing = "0.1"`, `tracing-subscriber = "0.3"`, `crossbeam-channel = "0.5"`, `image = "0.25"`.
- DBus service: name `org.icedtea.WM`, object path `/org/icedtea/WM`, on the session bus.
- Config DB: `$XDG_CONFIG_HOME/icedtea/config.redb`; any read failure falls back to baked-in defaults (never crash on config).
- Single writer of config is a future crate (settings); the compositor only reads.
- Window/workspace/focus state lives only in the compositor; `contract::Event` is the single outbound channel (DBus signals and tests both consume it).
- No unsafe in icedtea code except `unsafe { redb::Database::create }` / `Database::open` (required by redb's mmap API), each with a comment.
- Nested backend (`--nested`) is the dev/test backend; DRM + libinput is the real-session backend.
- Follow `anvil` for all smithay glue; copy the relevant feature list from `anvil/Cargo.toml`.

---

### Task 1: Workspace and crate scaffolding

**Files:**
- Rewrite: `Cargo.toml` (workspace root)
- Create: `contract/Cargo.toml`, `contract/src/lib.rs`
- Create: `config/Cargo.toml`, `config/src/lib.rs`
- Create: `compositor/Cargo.toml`, `compositor/src/main.rs`

**Interfaces:**
- Produces: `contract::WM_BUS_NAME: &str`, `contract::WM_PATH: &str`; empty crate skeletons that `cargo build` and `cargo test` pass.

- [ ] **Step 1: Convert the root manifest to a virtual workspace**

Replace the current root `Cargo.toml` (which is a package) with:

```toml
[workspace]
resolver = "2"
members = ["contract", "config", "compositor"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.94"

[workspace.dependencies]
smithay = { version = "0.7", features = [] }
zbus = "5"
redb = "3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
crossbeam-channel = "0.5"
image = "0.25"
```

- [ ] **Step 2: Create the `contract` crate skeleton**

`contract/Cargo.toml`:

```toml
[package]
name = "icedtea-contract"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
serde.workspace = true
```

`contract/src/lib.rs`:

```rust
pub const WM_BUS_NAME: &str = "org.icedtea.WM";
pub const WM_PATH: &str = "/org/icedtea/WM";
```

- [ ] **Step 3: Create the `config` crate skeleton**

`config/Cargo.toml`:

```toml
[package]
name = "icedtea-config"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
icedtea-contract = { path = "../contract" }
serde.workspace = true
serde_json.workspace = true
redb.workspace = true
```

`config/src/lib.rs`:

```rust
pub const SCHEMA_VERSION: u64 = 1;
```

- [ ] **Step 4: Create the `compositor` crate skeleton**

`compositor/Cargo.toml`:

```toml
[package]
name = "icedtea-compositor"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
icedtea-contract = { path = "../contract" }
icedtea-config = { path = "../config" }
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
crossbeam-channel.workspace = true
image.workspace = true
zbus.workspace = true
redb.workspace = true
```

`compositor/src/main.rs`:

```rust
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("icedtea compositor starting");
}
```

- [ ] **Step 5: Build and test the workspace**

Run: `cargo build && cargo test`
Expected: build succeeds; `cargo test` reports "0 passed" (or all tests pass) across the workspace.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock contract config compositor
git commit -m "chore: scaffold icedtea workspace with contract, config, compositor crates"
```

---

### Task 2: `contract` wire types and the `Event` enum

**Files:**
- Create: `contract/src/types.rs`
- Create: `contract/src/event.rs`
- Modify: `contract/src/lib.rs`
- Modify: `contract/Cargo.toml` (add `zvariant` for the DBus signatures)

**Interfaces:**
- Consumes: Task 1 constants.
- Produces (exact names later tasks rely on):
  - `contract::WindowId(pub u32)` — `Copy`, `Eq`, `Ord`, `Serialize`, `Deserialize`, `zvariant::Type`
  - `contract::Rectangle { x: i32, y: i32, width: i32, height: i32 }`
  - `contract::WindowInfo { id: WindowId, app_id: String, title: String, pid: u32, workspace: u32, geometry: Rectangle, maximized: bool, minimized: bool, fullscreen: bool, focused: bool }`
  - `contract::WorkspaceInfo { id: u32, name: String }`
  - `contract::Snapshot { seq: u64, windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>, active_workspace: u32 }`
  - `contract::WindowUpdate { title: Option<String>, geometry: Option<Rectangle>, workspace: Option<u32>, maximized: Option<bool>, minimized: Option<bool>, fullscreen: Option<bool>, focused: Option<bool> }` — `Default`
  - `contract::AltTabState { active: bool, entries: Vec<WindowId>, index: usize }`
  - `contract::Palette { background: String, foreground: String, accent: String }`
  - `contract::Appearance { bar_position: String, bar_height: i32, corner_radius: i32, snap_gap: i32, palette: Palette, wallpaper: Option<String> }`
  - `contract::Event` enum, variants carry the same structs above.

- [ ] **Step 1: Add `zvariant` to `contract`**

In `contract/Cargo.toml` add:

```toml
zvariant = "5"
```

- [ ] **Step 2: Write the wire types**

`contract/src/types.rs`:

```rust
use serde::{Deserialize, Serialize};
use zvariant::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[zvariant(signature = "u")]
pub struct WindowId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[zvariant(signature = "(iiii)")]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rectangle {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct WindowInfo {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub pid: u32,
    pub workspace: u32,
    pub geometry: Rectangle,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct WorkspaceInfo {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Snapshot {
    pub seq: u64,
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct WindowUpdate {
    pub title: Option<String>,
    pub geometry: Option<Rectangle>,
    pub workspace: Option<u32>,
    pub maximized: Option<bool>,
    pub minimized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub focused: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AltTabState {
    pub active: bool,
    pub entries: Vec<WindowId>,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Appearance {
    pub bar_position: String,
    pub bar_height: i32,
    pub corner_radius: i32,
    pub snap_gap: i32,
    pub palette: Palette,
    pub wallpaper: Option<String>,
}
```

- [ ] **Step 3: Write the internal `Event` enum**

`contract/src/event.rs`:

```rust
use crate::{AltTabState, Appearance, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    WindowOpened(WindowInfo),
    WindowClosed(WindowId),
    WindowUpdated { id: WindowId, update: WindowUpdate },
    WorkspaceSet { id: u32, active: bool },
    WorkspaceList(Vec<WorkspaceInfo>),
    AltTabState(AltTabState),
    ConfigReloaded(Appearance),
}
```

`contract/src/lib.rs` add:

```rust
pub mod event;
pub mod types;

pub use event::Event;
pub use types::*;
```

- [ ] **Step 4: Write round-trip tests**

`contract/src/types.rs` append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_window() -> WindowInfo {
        WindowInfo {
            id: WindowId(1),
            app_id: "org.gnome.Calculator".into(),
            title: "Calculator".into(),
            pid: 1234,
            workspace: 0,
            geometry: Rectangle { x: 100, y: 100, width: 400, height: 300 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: true,
        }
    }

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            seq: 7,
            windows: vec![sample_window()],
            workspaces: vec![
                WorkspaceInfo { id: 0, name: "1".into() },
                WorkspaceInfo { id: 1, name: "2".into() },
            ],
            active_workspace: 0,
        }
    }

    #[test]
    fn snapshot_json_round_trip() {
        let s = sample_snapshot();
        let json = serde_json::to_string(&s).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn window_info_zvariant_round_trip() {
        let w = sample_window();
        let encoded = zvariant::to_bytes(zvariant::EncodingContext::<zvariant::LE>::new_dbus(), &w).unwrap();
        let decoded: WindowInfo = zvariant::from_slice(&encoded, zvariant::EncodingContext::<zvariant::LE>::new_dbus()).unwrap();
        assert_eq!(w, decoded);
    }

    #[test]
    fn window_update_default_is_all_none() {
        let u = WindowUpdate::default();
        assert!(u.title.is_none() && u.geometry.is_none() && u.workspace.is_none());
        assert!(u.maximized.is_none() && u.minimized.is_none() && u.fullscreen.is_none());
        assert!(u.focused.is_none());
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p icedtea-contract`
Expected: all 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add contract
git commit -m "feat(contract): define DBus wire types and internal event enum"
```

---

### Task 3: `config` crate — redb store with default fallback

**Files:**
- Create: `config/src/defaults.rs`
- Create: `config/src/schema.rs`
- Modify: `config/src/lib.rs`

**Interfaces:**
- Consumes: `contract::Appearance`, Task 1 constants.
- Produces:
  - `config::KeyCombo { modifiers: Vec<String>, key: String }` — `Serialize`, `Deserialize`, `PartialEq`, `Clone`
  - `config::Behavior { raise_on_focus: bool, hide_bar_on_fullscreen: bool, snap_enabled: bool }`
  - `config::Config { keybindings: HashMap<String, KeyCombo>, appearance: Appearance, behavior: Behavior, workspace_names: Vec<String> }`
  - `config::default_config() -> Config`
  - `config::default_db_path() -> PathBuf`
  - `config::load_or_default(db_path: &Path) -> Config` — never fails; falls back per-field
  - `config::Config::save(&self, db: &redb::Database) -> Result<(), redb::Error>` — used by tests now, by `settings` later
  - `config::open(db_path: &Path) -> Result<redb::Database, redb::Error>`

- [ ] **Step 1: Write the defaults**

`config/src/defaults.rs`:

```rust
use std::collections::HashMap;

use contract::Appearance;
use crate::{Behavior, Config, KeyCombo};

pub fn default_config() -> Config {
    let mut keybindings = HashMap::new();
    let insert = |map: &mut HashMap<String, KeyCombo>, action: &str, mods: &[&str], key: &str| {
        map.insert(
            action.to_string(),
            KeyCombo {
                modifiers: mods.iter().map(|m| m.to_string()).collect(),
                key: key.to_string(),
            },
        );
    };
    insert(&mut keybindings, "close", &["SUPER"], "KEY_q");
    insert(&mut keybindings, "fullscreen", &["SUPER"], "KEY_f");
    insert(&mut keybindings, "reload", &["SUPER", "SHIFT"], "KEY_r");
    insert(&mut keybindings, "quit", &["SUPER", "SHIFT"], "KEY_q");
    insert(&mut keybindings, "cycle:alt_tab", &["SUPER"], "KEY_Tab");
    insert(&mut keybindings, "spawn:terminal", &["SUPER"], "KEY_Return");
    insert(&mut keybindings, "snap:left", &["SUPER"], "KEY_Left");
    insert(&mut keybindings, "snap:right", &["SUPER"], "KEY_Right");
    insert(&mut keybindings, "snap:up", &["SUPER"], "KEY_Up");
    insert(&mut keybindings, "snap:down", &["SUPER"], "KEY_Down");
    insert(&mut keybindings, "snap:restore", &["SUPER", "SHIFT"], "KEY_Left");
    for n in 1..=9u32 {
        let key = format!("KEY_{n}");
        insert(&mut keybindings, &format!("workspace:{n}"), &["SUPER"], &key);
        insert(&mut keybindings, &format!("move_to_workspace:{n}"), &["SUPER", "CTRL"], &key);
    }

    Config {
        keybindings,
        appearance: Appearance {
            bar_position: "bottom".into(),
            bar_height: 42,
            corner_radius: 8,
            snap_gap: 8,
            palette: contract::Palette {
                background: "#1e1e2e".into(),
                foreground: "#cdd6f4".into(),
                accent: "#89b4fa".into(),
            },
            wallpaper: None,
        },
        behavior: Behavior {
            raise_on_focus: true,
            hide_bar_on_fullscreen: true,
            snap_enabled: true,
        },
        workspace_names: vec!["1".into(), "2".into(), "3".into(), "4".into()],
    }
}
```

- [ ] **Step 2: Write the redb schema**

`config/src/schema.rs`:

```rust
use redb::TableDefinition;

pub const DB_META: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("meta");
pub const DB_KEYBINDINGS: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("keybindings");
pub const DB_APPEARANCE: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("appearance");
pub const DB_BEHAVIOR: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("behavior");
pub const DB_WORKSPACES: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("workspaces");

pub const KEY_SCHEMA_VERSION: &str = "schema_version";
pub const KEY_APPEARANCE: &str = "appearance";
pub const KEY_BEHAVIOR: &str = "behavior";
pub const KEY_WORKSPACES: &str = "workspaces";
pub const KEY_ACTION_COUNT: &str = "action_count";
pub const KEY_ACTION: &str = "action:";
```

Every table stores JSON bytes (`&[u8]`). The keybindings table stores one row per action: key = `action:<name>`, value = `KeyCombo` JSON. `KEY_ACTION_COUNT` guards partial writes.

- [ ] **Step 3: Write load/save with per-field fallback**

`config/src/lib.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use redb::{Database, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod defaults;
pub mod schema;

use schema::*;

pub use defaults::default_config;
pub use contract::Appearance;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyCombo {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub raise_on_focus: bool,
    pub hide_bar_on_fullscreen: bool,
    pub snap_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub keybindings: HashMap<String, KeyCombo>,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub workspace_names: Vec<String>,
}

pub fn default_db_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().expect("HOME set").join(".config"));
    base.join("icedtea").join("config.redb")
}

/// Open the config DB. Missing file -> creates it. Corrupt/IO errors bubble up
/// to the caller (which chooses defaults). `create` is unsafe only because redb
/// mmaps the file.
pub fn open(db_path: &Path) -> Result<Database, redb::Error> {
    if let Some(dir) = db_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // SAFETY: redb requires an unsafe block for its mmap-based create/open.
    unsafe { Database::create(db_path) }
}

fn read_json<T: DeserializeOwned>(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &'static str,
) -> Option<T> {
    table.get(key).ok().flatten().and_then(|v| serde_json::from_slice(v.value()).ok())
}

/// Load config, falling back to defaults for every individual field that is
/// missing or unparsable. Never returns Err.
pub fn load_or_default(db_path: &Path) -> Config {
    let default = default_config();
    let db = match open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!("config db unavailable ({e}), using defaults");
            return default;
        }
    };
    let Ok(read_txn) = db.begin_read() else { return default };
    let Ok(table) = read_txn.open_table(DB_APPEARANCE) else { return default };

    let mut cfg = default;
    if let Some(appearance) = read_json::<Appearance>(&table, KEY_APPEARANCE) {
        cfg.appearance = appearance;
    }
    if let Ok(behavior_table) = read_txn.open_table(DB_BEHAVIOR) {
        if let Some(b) = read_json::<Behavior>(&behavior_table, KEY_BEHAVIOR) {
            cfg.behavior = b;
        }
    }
    if let Ok(ws_table) = read_txn.open_table(DB_WORKSPACES) {
        if let Some(names) = read_json::<Vec<String>>(&ws_table, KEY_WORKSPACES) {
            if !names.is_empty() {
                cfg.workspace_names = names;
            }
        }
    }
    if let Ok(kb_table) = read_txn.open_table(DB_KEYBINDINGS) {
        if read_json::<u64>(&kb_table, KEY_ACTION_COUNT).unwrap_or(0) > 0 {
            let mut keybindings = HashMap::new();
            let mut cursor = kb_table.range(KEY_ACTION.as_bytes()..).ok();
            while let Some(Ok((k, v))) = cursor.as_mut().and_then(|c| c.next()) {
                if let Ok(action) = std::str::from_utf8(k.value()) {
                    if let Some(action) = action.strip_prefix(KEY_ACTION) {
                        if let Some(combo) = serde_json::from_slice::<KeyCombo>(v.value()).ok() {
                            keybindings.insert(action.to_string(), combo);
                        }
                    }
                }
            }
            if !keybindings.is_empty() {
                cfg.keybindings = keybindings;
            }
        }
    }
    cfg
}

impl Config {
    pub fn save(&self, db: &Database) -> Result<(), redb::Error> {
        let write_txn = db.begin_write()?;
        {
            let mut meta = write_txn.open_table(DB_META)?;
            meta.insert(KEY_SCHEMA_VERSION, serde_json::to_vec(&crate::SCHEMA_VERSION).unwrap().as_slice())?;
            let mut appearance = write_txn.open_table(DB_APPEARANCE)?;
            appearance.insert(KEY_APPEARANCE, serde_json::to_vec(&self.appearance).unwrap().as_slice())?;
            let mut behavior = write_txn.open_table(DB_BEHAVIOR)?;
            behavior.insert(KEY_BEHAVIOR, serde_json::to_vec(&self.behavior).unwrap().as_slice())?;
            let mut workspaces = write_txn.open_table(DB_WORKSPACES)?;
            workspaces.insert(KEY_WORKSPACES, serde_json::to_vec(&self.workspace_names).unwrap().as_slice())?;
            let mut keybindings = write_txn.open_table(DB_KEYBINDINGS)?;
            keybindings.insert(KEY_ACTION_COUNT, serde_json::to_vec(&(self.keybindings.len() as u64)).unwrap().as_slice())?;
            for (action, combo) in &self.keybindings {
                let key = format!("{KEY_ACTION}{action}");
                keybindings.insert(key.as_str(), serde_json::to_vec(combo).unwrap().as_slice())?;
            }
        }
        write_txn.commit()
    }
}
```

Note: redb's `TableDefinition` for `&'static [u8]` values requires inserting byte slices whose lifetime outlives the write txn. `serde_json::to_vec` returns owned data — hoist each `Vec<u8>` into a `let` binding before `insert` so it lives long enough. The compiler will guide the exact shape; this is the only fiddly spot.

- [ ] **Step 4: Write tests**

`config/src/lib.rs` append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_parseable() {
        let cfg = default_config();
        assert_eq!(cfg.workspace_names, vec!["1", "2", "3", "4"]);
        assert!(cfg.keybindings.contains_key("close"));
        assert!(cfg.behavior.snap_enabled);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        let cfg = default_config();
        cfg.save(&db).unwrap();
        drop(db);

        let loaded = load_or_default(&path);
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn missing_db_returns_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.redb");
        let cfg = load_or_default(&path);
        assert_eq!(cfg, default_config());
    }

    #[test]
    fn corrupt_appearance_falls_back_per_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        {
            let write_txn = db.begin_write().unwrap();
            let mut table = write_txn.open_table(DB_APPEARANCE).unwrap();
            table.insert(KEY_APPEARANCE, b"not json".as_slice()).unwrap();
            write_txn.commit().unwrap();
        }
        drop(db);
        let cfg = load_or_default(&path);
        // Entire appearance value unparsable -> whole Appearance defaults.
        assert_eq!(cfg.appearance, default_config().appearance);
    }
}
```

Add `tempfile = "3"` as a dev-dependency of `config`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p icedtea-config`
Expected: all 4 tests pass. Fix any redb lifetime errors the compiler surfaces.

- [ ] **Step 6: Commit**

```bash
git add config
git commit -m "feat(config): redb-backed config store with per-field default fallback"
```

---

### Task 4: `window.rs` — window and workspace model

**Files:**
- Create: `compositor/src/window.rs`
- Modify: `compositor/src/main.rs` (module declaration)

**Interfaces:**
- Consumes: `contract::{WindowId, Rectangle, WindowInfo, WorkspaceInfo, Snapshot, Event, WindowUpdate, AltTabState}`; `config::Config`.
- Produces:
  - `compositor::window::{Window, Workspace, WindowManager}`
  - `WindowManager::new(workspace_names: Vec<String>) -> Self`
  - `add_window(&mut self, app_id: &str, title: &str, pid: u32) -> WindowId`
  - `remove_window(&mut self, id: WindowId) -> Option<Window>`
  - `get(&self, id: WindowId) -> Option<&Window>`
  - `set_title`, `set_geometry`, `set_maximized`, `set_minimized`, `set_fullscreen`, each `(&mut self, id: WindowId, value: …) -> Option<()>`
  - `set_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()>`
  - `focus(&mut self, id: WindowId) -> Option<()>` — un-focuses the previous focused window
  - `focused_window(&self) -> Option<&Window>`
  - `set_active_workspace(&mut self, id: u32) -> bool`
  - `active_workspace(&self) -> u32`
  - `windows(&self) -> impl Iterator<Item = &Window>` (ordered by focus MRU)
  - `windows_in_workspace(&self, ws: u32) -> Vec<&Window>`
  - `to_info(&self, w: &Window) -> WindowInfo`, `workspace_info(&self) -> Vec<WorkspaceInfo>`
  - `snapshot(&self) -> Snapshot`
  - `note_event(&mut self) -> &mut Vec<Event>` — mutation helper that bumps `seq`

- [ ] **Step 1: Write the failing model tests**

`compositor/src/window.rs` (with tests at the bottom, written first):

```rust
use std::collections::BTreeMap;

use icedtea_contract as contract;
use contract::{AltTabState, Event, Rectangle, Snapshot, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};

#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub pid: u32,
    pub workspace: u32,
    pub geometry: Rectangle,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: u32,
    pub name: String,
    pub focused_window: Option<WindowId>,
}

pub struct WindowManager {
    windows: BTreeMap<WindowId, Window>,
    workspaces: Vec<Workspace>,
    active_workspace: u32,
    next_id: u32,
    seq: u64,
    /// Pending events drained by the compositor each frame.
    pub pending_events: Vec<Event>,
}

impl WindowManager {
    pub fn new(workspace_names: Vec<String>) -> Self {
        let workspaces = workspace_names
            .iter()
            .enumerate()
            .map(|(i, name)| Workspace { id: i as u32, name: name.clone(), focused_window: None })
            .collect();
        Self {
            windows: BTreeMap::new(),
            workspaces,
            active_workspace: 0,
            next_id: 1,
            seq: 0,
            pending_events: Vec::new(),
        }
    }

    fn bump(&mut self) {
        self.seq += 1;
    }

    fn emit(&mut self, event: Event) {
        self.bump();
        self.pending_events.push(event);
    }

    pub fn add_window(&mut self, app_id: &str, title: &str, pid: u32) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let window = Window {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
            pid,
            workspace: self.active_workspace,
            geometry: Rectangle { x: 0, y: 0, width: 0, height: 0 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
        };
        self.windows.insert(id, window.clone());
        self.emit(Event::WindowOpened(self.to_info(&self.windows[&id])));
        self.focus(id);
        id
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        let window = self.windows.remove(&id)?;
        let workspace = window.workspace;
        let was_focused = self.workspace_mut(workspace).focused_window == Some(id);
        if was_focused {
            self.workspace_mut(workspace).focused_window = None;
        }
        self.emit(Event::WindowClosed(id));
        Some(window)
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn set_title(&mut self, id: WindowId, title: String) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.title = title.clone();
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { title: Some(title), ..Default::default() } });
        Some(())
    }

    pub fn set_geometry(&mut self, id: WindowId, geometry: Rectangle) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.geometry = geometry;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { geometry: Some(geometry), ..Default::default() } });
        Some(())
    }

    pub fn set_maximized(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.maximized = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { maximized: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_minimized(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.minimized = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { minimized: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_fullscreen(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.fullscreen = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { fullscreen: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()> {
        if !self.workspace_exists(workspace) {
            return None;
        }
        let w = self.windows.get_mut(&id)?;
        w.workspace = workspace;
        w.focused = false;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { workspace: Some(workspace), focused: Some(false), ..Default::default() } });
        Some(())
    }

    pub fn focus(&mut self, id: WindowId) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        if w.minimized {
            w.minimized = false;
        }
        let (ws, was_focused) = {
            let w = self.windows.get(&id)?;
            (w.workspace, w.focused)
        };
        if was_focused {
            return Some(());
        }
        // Clear prior focus on the same workspace.
        let old = self.workspace_mut(ws).focused_window.replace(id);
        if let Some(old_id) = old {
            if old_id != id {
                if let Some(old_w) = self.windows.get_mut(&old_id) {
                    old_w.focused = false;
                    self.emit(Event::WindowUpdated { id: old_id, update: WindowUpdate { focused: Some(false), ..Default::default() } });
                }
            }
        }
        let w = self.windows.get_mut(&id)?;
        w.focused = true;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { focused: Some(true), ..Default::default() } });
        Some(())
    }

    pub fn focused_window(&self) -> Option<&Window> {
        self.workspace(self.active_workspace).focused_window.and_then(|id| self.windows.get(&id))
    }

    pub fn set_active_workspace(&mut self, id: u32) -> bool {
        if !self.workspace_exists(id) {
            return false;
        }
        self.active_workspace = id;
        self.emit(Event::WorkspaceSet { id, active: true });
        true
    }

    pub fn active_workspace(&self) -> u32 {
        self.active_workspace
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }

    pub fn windows_in_workspace(&self, ws: u32) -> Vec<&Window> {
        self.windows.values().filter(|w| w.workspace == ws).collect()
    }

    pub fn to_info(&self, w: &Window) -> WindowInfo {
        WindowInfo {
            id: w.id,
            app_id: w.app_id.clone(),
            title: w.title.clone(),
            pid: w.pid,
            workspace: w.workspace,
            geometry: w.geometry,
            maximized: w.maximized,
            minimized: w.minimized,
            fullscreen: w.fullscreen,
            focused: w.focused,
        }
    }

    pub fn workspace_info(&self) -> Vec<WorkspaceInfo> {
        self.workspaces.iter().map(|w| WorkspaceInfo { id: w.id, name: w.name.clone() }).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            seq: self.seq,
            windows: self.windows.values().map(|w| self.to_info(w)).collect(),
            workspaces: self.workspace_info(),
            active_workspace: self.active_workspace,
        }
    }

    pub fn alt_tab_entries(&self) -> Vec<WindowId> {
        self.windows_in_workspace(self.active_workspace)
            .into_iter()
            .filter(|w| !w.minimized)
            .map(|w| w.id)
            .collect()
    }

    fn workspace(&self, id: u32) -> Option<&Workspace> {
        self.workspaces.get(id as usize)
    }

    fn workspace_mut(&mut self, id: u32) -> &mut Workspace {
        &mut self.workspaces[id as usize]
    }

    fn workspace_exists(&self, id: u32) -> bool {
        id as usize < self.workspaces.len()
    }
}
```

Write the tests in a `#[cfg(test)] mod tests` block **before** the implementation compiles (TDD):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> WindowManager {
        WindowManager::new(vec!["1".into(), "2".into()])
    }

    #[test]
    fn add_focuses_window_and_emits_opened() {
        let mut m = mgr();
        let id = m.add_window("app", "title", 1);
        assert!(m.get(id).unwrap().focused);
        assert!(matches!(m.pending_events.first(), Some(Event::WindowOpened(_))));
    }

    #[test]
    fn focus_unfocuses_previous() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        let b = m.add_window("b", "b", 2);
        assert!(!m.get(a).unwrap().focused);
        assert!(m.get(b).unwrap().focused);
    }

    #[test]
    fn move_to_workspace_keeps_focus_valid() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        m.set_workspace(a, 1).unwrap();
        assert_eq!(m.get(a).unwrap().workspace, 1);
        assert!(!m.get(a).unwrap().focused);
    }

    #[test]
    fn remove_clears_focus_to_none() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        m.remove_window(a).unwrap();
        assert!(m.focused_window().is_none());
    }

    #[test]
    fn snapshot_matches_state() {
        let mut m = mgr();
        let id = m.add_window("app", "t", 7);
        let snap = m.snapshot();
        assert_eq!(snap.active_workspace, 0);
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].id, id);
        assert_eq!(snap.windows[0].pid, 7);
        assert_eq!(snap.workspaces.len(), 2);
    }

    #[test]
    fn alt_tab_skips_minimized() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        let b = m.add_window("b", "b", 2);
        m.set_minimized(a, true).unwrap();
        assert_eq!(m.alt_tab_entries(), vec![b]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib window`
Expected: compile errors / test failures (module not yet declared, functions missing).

- [ ] **Step 3: Declare the module and implement**

Add `pub mod window;` to `compositor/src/main.rs`. Fix any borrow-checker issues the implementation above surfaces (the model above is the target shape; adjust locally where the compiler disagrees, keeping the public signatures identical).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-compositor --lib window`
Expected: all 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add compositor/src/window.rs compositor/src/main.rs
git commit -m "feat(compositor): window and workspace model with event emission"
```

---

### Task 5: `layout.rs` — snap zones and placement geometry

**Files:**
- Create: `compositor/src/layout.rs`
- Modify: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `contract::Rectangle`; `contract::Point`-free — use `(i32, i32)`.
- Produces:
  - `layout::SnapZone` enum: `Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight`
  - `snap_zone_for_point(output: Rectangle, point: (i32, i32), threshold: i32) -> Option<SnapZone>`
  - `snapped_geometry(output: Rectangle, zone: SnapZone, gap: i32) -> Rectangle`
  - `restored_geometry(original: Rectangle, snapped: Rectangle) -> Rectangle`
  - `cascade_point(occupied: &[Rectangle], size: (i32, i32), step: i32) -> (i32, i32)`
  - `is_edge_point(output: Rectangle, point: (i32, i32), threshold: i32) -> bool`

- [ ] **Step 1: Write the failing tests**

`compositor/src/layout.rs` (tests first):

```rust
use icedtea_contract::Rectangle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZone { Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight }

pub fn snap_zone_for_point(output: Rectangle, point: (i32, i32), threshold: i32) -> Option<SnapZone> {
    let (x, y) = point;
    let near_left = x <= output.x + threshold;
    let near_right = x >= output.x + output.width - threshold;
    let near_top = y <= output.y + threshold;
    let near_bottom = y >= output.y + output.height - threshold;

    let edge = |h: bool, v: bool| match (h, v) {
        (true, true) => None,
        (true, false) => Some(SnapZone::Left),
        (false, true) => Some(SnapZone::Right),
        (false, false) => None,
    };
    let corner = |h: bool, v: bool| match (h, v) {
        (true, true) => Some(SnapZone::TopLeft),
        (true, false) => Some(SnapZone::TopRight),
        (false, true) => Some(SnapZone::BottomLeft),
        (false, false) => Some(SnapZone::BottomRight),
    };

    // Corners take priority over edges.
    let in_corner = (near_left || near_right) && (near_top || near_bottom);
    if in_corner {
        return corner(near_left, near_top);
    }
    if near_left || near_right {
        return edge(near_left, near_right);
    }
    if near_top || near_bottom {
        return edge(!near_bottom, near_top);
    }
    None
}

pub fn snapped_geometry(output: Rectangle, zone: SnapZone, gap: i32) -> Rectangle {
    let w = output.width / 2;
    let h = output.height / 2;
    let (left_x, top_y, right_x, bottom_y) = (output.x, output.y, output.x + w, output.y + h);
    match zone {
        SnapZone::Left => Rectangle { x: output.x + gap, y: output.y + gap, width: w - 2 * gap, height: output.height - 2 * gap },
        SnapZone::Right => Rectangle { x: right_x + gap, y: output.y + gap, width: w - 2 * gap, height: output.height - 2 * gap },
        SnapZone::Top => Rectangle { x: output.x + gap, y: output.y + gap, width: output.width - 2 * gap, height: h - 2 * gap },
        SnapZone::Bottom => Rectangle { x: output.x + gap, y: bottom_y + gap, width: output.width - 2 * gap, height: h - 2 * gap },
        SnapZone::TopLeft => Rectangle { x: left_x + gap, y: top_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::TopRight => Rectangle { x: right_x + gap, y: top_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::BottomLeft => Rectangle { x: left_x + gap, y: bottom_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::BottomRight => Rectangle { x: right_x + gap, y: bottom_y + gap, width: w - 2 * gap, height: h - 2 * gap },
    }
}

pub fn restored_geometry(original: Rectangle, _snapped: Rectangle) -> Rectangle {
    original
}

pub fn is_edge_point(output: Rectangle, point: (i32, i32), threshold: i32) -> bool {
    snap_zone_for_point(output, point, threshold).is_some()
}

pub fn cascade_point(occupied: &[Rectangle], size: (i32, i32), step: i32) -> (i32, i32) {
    let base = (occupied.len() as i32 * step, occupied.len() as i32 * step);
    (base.0, base.1)
}
```

Append tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: Rectangle = Rectangle { x: 0, y: 0, width: 1000, height: 800 };

    #[test]
    fn left_edge_snaps_left_half() {
        let g = snapped_geometry(OUTPUT, SnapZone::Left, 8);
        assert_eq!(g.x, 8);
        assert_eq!(g.width, 1000 / 2 - 16);
        assert_eq!(g.height, 800 - 16);
    }

    #[test]
    fn quadrant_geometry() {
        let g = snapped_geometry(OUTPUT, SnapZone::TopRight, 0);
        assert_eq!(g.x, 500);
        assert_eq!(g.y, 0);
        assert_eq!(g.width, 500);
        assert_eq!(g.height, 400);
    }

    #[test]
    fn corner_wins_over_edge() {
        assert_eq!(snap_zone_for_point(OUTPUT, (0, 0), 10), Some(SnapZone::TopLeft));
        assert_eq!(snap_zone_for_point(OUTPUT, (999, 799), 10), Some(SnapZone::BottomRight));
        assert_eq!(snap_zone_for_point(OUTPUT, (500, 0), 10), Some(SnapZone::Top));
    }

    #[test]
    fn center_is_not_a_zone() {
        assert_eq!(snap_zone_for_point(OUTPUT, (500, 400), 10), None);
    }

    #[test]
    fn gap_keeps_window_inside_output() {
        for zone in [SnapZone::Left, SnapZone::Right, SnapZone::Top, SnapZone::Bottom] {
            let g = snapped_geometry(OUTPUT, zone, 8);
            assert!(g.x >= 0 && g.y >= 0);
            assert!(g.x + g.width <= OUTPUT.width);
            assert!(g.y + g.height <= OUTPUT.height);
        }
    }

    #[test]
    fn restore_returns_original() {
        let orig = Rectangle { x: 10, y: 10, width: 200, height: 100 };
        assert_eq!(restored_geometry(orig, snapped_geometry(OUTPUT, SnapZone::Left, 8)), orig);
    }

    #[test]
    fn cascade_steps_by_count() {
        let occupied = vec![Rectangle { x: 0, y: 0, width: 100, height: 100 }];
        assert_eq!(cascade_point(&occupied, (200, 100), 24), (24, 24));
        assert_eq!(cascade_point(&[], (200, 100), 24), (0, 0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib layout`
Expected: fails — module not declared.

- [ ] **Step 3: Declare module**

Add `pub mod layout;` to `compositor/src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-compositor --lib layout`
Expected: all 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add compositor/src/layout.rs compositor/src/main.rs
git commit -m "feat(compositor): snap zone and placement geometry"
```

---

### Task 6: `decoration.rs` — SSD metrics and hit-testing

**Files:**
- Create: `compositor/src/decoration.rs`
- Modify: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `contract::Rectangle`.
- Produces:
  - `decoration::TITLE_BAR_HEIGHT: i32 = 28`
  - `decoration::BUTTON_WIDTH: i32 = 40`
  - `decoration::DecorationAction` enum: `Minimize, Maximize, Close, Move, None`
  - `title_bar_rect(geometry: Rectangle) -> Rectangle`
  - `button_rects(geometry: Rectangle) -> [Rectangle; 3]` (min, max, close, right-aligned)
  - `hit_test(geometry: Rectangle, local: (i32, i32)) -> DecorationAction`
  - `is_csd(app_id: &str, requested: Option<bool>) -> bool`

- [ ] **Step 1: Write the failing tests**

`compositor/src/decoration.rs`:

```rust
use icedtea_contract::Rectangle;

pub const TITLE_BAR_HEIGHT: i32 = 28;
pub const BUTTON_WIDTH: i32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationAction { Minimize, Maximize, Close, Move, None }

pub fn title_bar_rect(geometry: Rectangle) -> Rectangle {
    Rectangle { x: geometry.x, y: geometry.y, width: geometry.width, height: TITLE_BAR_HEIGHT }
}

pub fn button_rects(geometry: Rectangle) -> [Rectangle; 3] {
    let right = geometry.x + geometry.width;
    let y = geometry.y;
    [
        Rectangle { x: right - 3 * BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
        Rectangle { x: right - 2 * BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
        Rectangle { x: right - BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
    ]
}

pub fn hit_test(geometry: Rectangle, local: (i32, i32)) -> DecorationAction {
    let bar = title_bar_rect(geometry);
    if !bar.contains(local.0, local.1) {
        return DecorationAction::None;
    }
    for (i, r) in button_rects(geometry).iter().enumerate() {
        if r.contains(local.0, local.1) {
            return match i { 0 => DecorationAction::Minimize, 1 => DecorationAction::Maximize, _ => DecorationAction::Close };
        }
    }
    DecorationAction::Move
}

pub fn is_csd(app_id: &str, requested: Option<bool>) -> bool {
    // Explicit request wins; otherwise assume GTK-style apps use CSD.
    requested.unwrap_or(app_id.starts_with("org.gtk") || app_id.contains("gtk4"))
}
```

Append tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const GEO: Rectangle = Rectangle { x: 50, y: 50, width: 600, height: 400 };

    #[test]
    fn bar_is_top_strip() {
        let bar = title_bar_rect(GEO);
        assert_eq!(bar, Rectangle { x: 50, y: 50, width: 600, height: TITLE_BAR_HEIGHT });
    }

    #[test]
    fn close_button_is_rightmost() {
        let rects = button_rects(GEO);
        assert_eq!(rects[2].x, 50 + 600 - BUTTON_WIDTH);
    }

    #[test]
    fn buttons_and_move_areas() {
        let rects = button_rects(GEO);
        let inside_close = (rects[2].x + 1, rects[2].y + 1);
        assert_eq!(hit_test(GEO, inside_close), DecorationAction::Close);
        assert_eq!(hit_test(GEO, (100, 55)), DecorationAction::Move);
    }

    #[test]
    fn below_bar_is_none() {
        assert_eq!(hit_test(GEO, (100, 200)), DecorationAction::None);
    }

    #[test]
    fn csd_negotiation() {
        assert!(is_csd("org.gtk.MyApp", None));
        assert!(!is_csd("org.example.C", Some(false)));
        assert!(is_csd("anything", Some(true)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib decoration`
Expected: fails — module not declared.

- [ ] **Step 3: Declare module**

Add `pub mod decoration;` to `compositor/src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-compositor --lib decoration`
Expected: all 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add compositor/src/decoration.rs compositor/src/main.rs
git commit -m "feat(compositor): SSD metrics and decoration hit-testing"
```

---

### Task 7: Compositor bootstrap — state, backends, and the event loop

**Files:**
- Modify: `compositor/Cargo.toml` (add smithay with features)
- Create: `compositor/src/state.rs`
- Create: `compositor/src/backend.rs`
- Rewrite: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `WindowManager` (Task 4), `Config`/`load_or_default` (Task 3).
- Produces:
  - `state::State` — the smithay event-loop state. Holds `pub window_manager: WindowManager`, `pub config: Config`, `pub dbus_tx: crossbeam_channel::Sender<Event>` (the event fan-out), and output/seat data.
  - `state::State::new(config: Config) -> Self`
  - `backend::Backend` — wraps a smithay backend; `Backend::init_nested()`, `Backend::init_drm()` following anvil.
  - `main.rs` — arg parsing (`--nested` vs default DRM), tracing init, `State::new(load_or_default(&default_db_path()))`, event loop run.
  - The compositor emits `contract::Event`s onto the `dbus_tx` channel whenever the model mutates.
  - The compositor exposes globals: `wl_compositor`, `wl_subcompositor`, `wl_shm`, `wl_seat`, `wl_output`, `xdg_wm_base`, `zwlr_layer_shell_v1`, `zxdg_decoration_manager_v1`.

- [ ] **Step 1: Add smithay with the right features**

In `compositor/Cargo.toml`, replace `smithay`'s workspace line with an explicit feature set, **mirroring anvil's** `Cargo.toml` from the pinned 0.7 tag:

```toml
smithay = { version = "0.7", default-features = false, features = [
  "backend_drm", "backend_egl", "backend_libinput", "backend_session",
  "backend_wayland", "renderer_gl", "wayland_frontend", "layer_shell",
  "xdg_decoration", "xwayland",
] }
```

Drop `xwayland` if anvil 0.7 does not enable it by default; keep the set that anvil's `Cargo.toml` uses for DRM + libinput + GL + wayland frontend. (The `backend_wayland` feature is required for `--nested` and for the Layer Shell to be testable.)

- [ ] **Step 2: Copy the anvil bootstrap into `backend.rs` and `state.rs`**

Copy the following from anvil (pinned 0.7), then adapt per the notes:

| anvil file | what to take | adapt |
| --- | --- | --- |
| `anvil/src/main.rs` | `init_backend`, the `--backend nested\|drm` arg parse, and the main event loop `EventLoop::new(...)` + `loop` | keep `nested` and `drm` only; drop `winit`/`x11` if anvil splits them |
| `anvil/src/state.rs` | the `State` struct, `init_state` (creating globals for compositor, shm, seat, output, xdg_shell, layer_shell, xdg_decoration) | replace anvil's own window/space logic with `WindowManager`; add `config` and `dbus_tx` fields |
| `anvil/src/output_handling.rs` | `Output`, output creation from the backend | unchanged |
| `anvil/src/shell_handling.rs` | the xdg-toplevel `Dispatch` handlers (map/commit/unmap/destroy, `set_title`, `set_app_id`) | on map → `window_manager.add_window(app_id, title, pid)`; on title change → `set_title`; forward resulting `pending_events` to `dbus_tx` |

Key adaptations, spelled out:

1. `State` must hold `pub window_manager: WindowManager`, `pub config: Config`, and `pub dbus_tx: crossbeam_channel::Sender<Event>`. Add a helper:
   ```rust
   pub fn emit_pending(&mut self) {
       for ev in self.window_manager.pending_events.drain(..) {
           let _ = self.dbus_tx.send(ev);
       }
   }
   ```
2. In the xdg toplevel map handler: read `app_id` and `title` from the client surface, call `window_manager.add_window`, then `emit_pending()`.
3. In `set_title` handler: `window_manager.set_title(id, title)` then `emit_pending()`.
4. `backend.rs` exposes `enum Backend { Nested(...), Drm(...) }` with `fn init_nested() -> Self` and `fn init_drm() -> Self`, both copied from anvil's `init_backend` branches. The nested branch must set `WAYLAND_DISPLAY` for child clients (anvil's `--backend nested` already does).
5. `main.rs`:
   ```rust
   fn main() {
       tracing_subscriber::fmt().with_env_filter(...).init();
       let config = icedtea_config::load_or_default(&icedtea_config::default_db_path());
       let (dbus_tx, _dbus_rx) = crossbeam_channel::unbounded::<Event>();
       let state = state::State::new(config, dbus_tx);
       // anvil's event loop setup, calling State::new(...)
   }
   ```

- [ ] **Step 3: Build**

Run: `cargo build -p icedtea-compositor`
Expected: compiles. Fix signature drift against the pinned anvil by consulting the 0.7 source; the public names from `Interfaces` must not change.

- [ ] **Step 4: Manual smoke test — nested boot**

Run (terminal A): `RUST_LOG=info cargo run -p icedtea-compositor -- --nested`
Expected: logs the nested display name (e.g. `wayland-1`).

Run (terminal B), if a wayland client is available:
```bash
WAYLAND_DISPLAY=<the logged name> foot
```
Expected: a foot window opens inside the nested compositor; the compositor logs a `WindowOpened` event and the window is present in `GetState` once Task 12 lands.

- [ ] **Step 5: Unit test the event fan-out seam**

Add to `state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_config::default_config;

    #[test]
    fn state_emits_pending_events_on_channel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1);
        state.emit_pending();
        assert!(matches!(rx.try_recv(), Ok(Event::WindowOpened(_))));
    }
}
```

Run: `cargo test -p icedtea-compositor --lib state`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add compositor/Cargo.toml compositor/src/main.rs compositor/src/state.rs compositor/src/backend.rs
git commit -m "feat(compositor): boot a smithay compositor with window model and event fan-out"
```

---

### Task 8: `render.rs` — wallpaper, scene assembly, and snap preview

**Files:**
- Create: `compositor/src/render.rs`
- Modify: `compositor/src/state.rs` (render_output hook)
- Modify: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `layout::{snapped_geometry, snap_zone_for_point, is_edge_point}` (Task 5), `decoration::{title_bar_rect, button_rects}` (Task 6), `contract::{Appearance, Rectangle}`.
- Produces:
  - `render::SceneLayer` enum: `Wallpaper, Windows, SnapPreview` — used for z-ordering tests.
  - `render::scene_order(windows: &[&Window], snap_active: bool) -> Vec<SceneLayer>` — pure, tested.
  - `render::wallpaper_color(appearance: &Appearance) -> [f32; 4]`
  - `render::draw_frame(...)` — glues smithay's renderer into anvil's `render_output` path.
  - The wallpaper renders from `appearance.wallpaper` (image, stretched) or `appearance.palette.background` (solid). The snap preview is a translucent accent rectangle over the target zone while a drag is in snap-preview state.

- [ ] **Step 1: Write the failing scene-order test**

`compositor/src/render.rs` (tests first):

```rust
use icedtea_contract::{Appearance, Rectangle};
use crate::window::Window;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneLayer { Wallpaper, Windows, SnapPreview }

pub fn scene_order(windows: &[&Window], snap_active: bool) -> Vec<SceneLayer> {
    let mut order = vec![SceneLayer::Wallpaper];
    if !windows.is_empty() {
        order.push(SceneLayer::Windows);
    }
    if snap_active {
        order.push(SceneLayer::SnapPreview);
    }
    order
}

pub fn wallpaper_color(appearance: &Appearance) -> [f32; 4] {
    hex_to_rgba(&appearance.palette.background)
}

pub fn hex_to_rgba(hex: &str) -> [f32; 4] {
    let hex = hex.trim_start_matches('#');
    let v = u32::from_str_radix(hex, 16).unwrap_or(0x000000);
    let r = ((v >> 16) & 0xff) as f32 / 255.0;
    let g = ((v >> 8) & 0xff) as f32 / 255.0;
    let b = (v & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}
```

Append tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_window() -> Window {
        Window {
            id: icedtea_contract::WindowId(1),
            app_id: "a".into(), title: "t".into(), pid: 1,
            workspace: 0, geometry: Rectangle { x: 0, y: 0, width: 10, height: 10 },
            maximized: false, minimized: false, fullscreen: false, focused: true,
        }
    }

    #[test]
    fn order_has_wallpaper_first_snap_last() {
        let w = fake_window();
        let order = scene_order(&[&w], true);
        assert_eq!(order[0], SceneLayer::Wallpaper);
        assert_eq!(order.last(), Some(&SceneLayer::SnapPreview));
        assert!(order.contains(&SceneLayer::Windows));
    }

    #[test]
    fn no_windows_skips_window_layer() {
        assert_eq!(scene_order(&[], true), vec![SceneLayer::Wallpaper, SceneLayer::SnapPreview]);
        assert_eq!(scene_order(&[], false), vec![SceneLayer::Wallpaper]);
    }

    #[test]
    fn hex_conversion() {
        assert_eq!(hex_to_rgba("#ff0000"), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(hex_to_rgba("#1e1e2e"), [30.0 / 255.0, 30.0 / 255.0, 46.0 / 255.0, 1.0]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib render`
Expected: fails — module not declared.

- [ ] **Step 3: Declare module and implement rendering glue**

Add `pub mod render;` to `compositor/src/main.rs`.

Implement `draw_frame` by copying anvil's `render_output` (from `anvil/src/state.rs` or `anvil/src/render_elements.rs`) and adapting:

- The scene tree from anvil (`AnvilState`'s `render_output`) becomes: render wallpaper element first (solid `[f32;4]` from `wallpaper_color`, or the loaded `image` stretched via `smithay::backend::renderer::element::solid::SolidColorRenderElement` / texture element — follow anvil's `render_elements.rs` `Wallpaper` element if present, else draw a `SolidColorRenderElement`), then each window's `WaylandSurfaceRenderElement`, then, when the drag machine is in `Preview` state (Task 9), a translucent accent `SolidColorRenderElement` covering the target `snapped_geometry`.
- Use `crate::decoration` metrics to render SSD strips (full SSD rendering lands in Task 10; here, wire the element slot and leave the geometry hook in place).

- [ ] **Step 4: Run tests**

Run: `cargo test -p icedtea-compositor --lib render`
Expected: all 3 tests pass.

- [ ] **Step 5: Manual check**

Run: `RUST_LOG=info cargo run -p icedtea-compositor -- --nested`
Expected: nested compositor boots and, with a client open, shows a solid background color behind the client window. (SSD and preview are visually verified in Tasks 10 and 11.)

- [ ] **Step 6: Commit**

```bash
git add compositor/src/render.rs compositor/src/main.rs
git commit -m "feat(compositor): wallpaper, scene ordering, and snap preview rendering"
```

---

### Task 9: `input.rs` — keybinding matching and drag/alt-tab state machines

**Files:**
- Create: `compositor/src/input.rs`
- Modify: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `config::{Config, KeyCombo}`, `contract::Rectangle`, `layout::{snap_zone_for_point, snapped_geometry, SnapZone}`.
- Produces:
  - `input::Modifiers` bitflags (`SUPER, CTRL, ALT, SHIFT`)
  - `input::key_name_to_keysym(name: &str) -> u32` and `keysym_to_key_name(keysym: u32) -> String` — small table for the keys the defaults use (`KEY_Return`, `KEY_Tab`, `KEY_Left/Right/Up/Down`, `KEY_1..KEY_9`, `KEY_q/f/r`), falling back to `xkbcommon`-style `(keysym >> 8)` name lookup for anything else.
  - `input::match_action(bindings: &HashMap<String, KeyCombo>, mods: Modifiers, keysym: u32) -> Option<String>` — returns the action name or `None`.
  - `input::AltTabMachine` with `fn start(&mut self, entries: Vec<WindowId>)`, `fn step(&mut self, next: bool) -> usize` (returns new index), `fn end(&mut self) -> Option<WindowId>` (index → entry).
  - `input::DragMachine` with `fn begin(&mut self, window_id, pointer_pos, grab_offset)`, `fn motion(&mut self, pointer_pos, output: Rectangle, threshold: i32)`, `fn end(&mut self) -> DragResult`, plus `drag_result()` for preview state.
  - `input::DragResult { Moved, Snapped { zone: SnapZone }, Restored }`

- [ ] **Step 1: Write the failing tests**

`compositor/src/input.rs` (tests first):

```rust
use std::collections::HashMap;

use icedtea_contract::{Rectangle, WindowId};
use crate::layout::{snap_zone_for_point, SnapZone};

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Modifiers: u32 {
        const SUPER = 1;
        const CTRL = 2;
        const ALT = 4;
        const SHIFT = 8;
    }
}

pub fn key_name_to_keysym(name: &str) -> u32 {
    match name {
        "KEY_Return" => 0xff0d,
        "KEY_Tab" => 0xff09,
        "KEY_Left" => 0xff51,
        "KEY_Up" => 0xff52,
        "KEY_Right" => 0xff53,
        "KEY_Down" => 0xff54,
        "KEY_q" => 0x71,
        "KEY_f" => 0x66,
        "KEY_r" => 0x72,
        _ => {
            if let Some(rest) = name.strip_prefix("KEY_") {
                if let Ok(digit) = rest.parse::<u32>() {
                    return 0x30 + digit; // KEY_1..KEY_9
                }
            }
            tracing::warn!("unknown key name {name}");
            0
        }
    }
}

pub fn keysym_to_key_name(keysym: u32) -> String {
    // Inverse of key_name_to_keysym; used for DBus/debug output.
    for (name, sym) in [("KEY_Return", 0xff0d), ("KEY_Tab", 0xff09), ("KEY_Left", 0xff51),
        ("KEY_Up", 0xff52), ("KEY_Right", 0xff53), ("KEY_Down", 0xff54),
        ("KEY_q", 0x71), ("KEY_f", 0x66), ("KEY_r", 0x72)] {
        if sym == keysym { return name.to_string(); }
    }
    if (0x31..=0x39).contains(&keysym) {
        return format!("KEY_{}", keysym - 0x30);
    }
    format!("KEY_{keysym}")
}

pub fn match_action(bindings: &HashMap<String, crate::config_combo::KeyCombo>, mods: Modifiers, keysym: u32) -> Option<String> {
    use crate::config_combo::KeyCombo;
    for (action, combo) in bindings {
        let wanted = combo.modifiers.iter().fold(Modifiers::empty(), |acc, m| {
            acc | match m.as_str() {
                "SUPER" => Modifiers::SUPER,
                "CTRL" => Modifiers::CTRL,
                "ALT" => Modifiers::ALT,
                "SHIFT" => Modifiers::SHIFT,
                _ => Modifiers::empty(),
            }
        });
        if wanted == mods && key_name_to_keysym(&combo.key) == keysym {
            return Some(action.clone());
        }
    }
    None
}

pub struct AltTabMachine {
    entries: Vec<WindowId>,
    index: usize,
    active: bool,
}

impl AltTabMachine {
    pub fn new() -> Self { Self { entries: Vec::new(), index: 0, active: false } }
    pub fn start(&mut self, entries: Vec<WindowId>) {
        self.entries = entries;
        self.index = 0;
        self.active = true;
    }
    pub fn step(&mut self, next: bool) -> usize {
        if self.entries.is_empty() { return self.index; }
        if next {
            self.index = (self.index + 1) % self.entries.len();
        } else {
            self.index = if self.index == 0 { self.entries.len() - 1 } else { self.index - 1 };
        }
        self.index
    }
    pub fn end(&mut self) -> Option<WindowId> {
        self.active = false;
        self.entries.get(self.index).copied()
    }
    pub fn is_active(&self) -> bool { self.active }
    pub fn index(&self) -> usize { self.index }
}

pub enum DragResult { Moved, Snapped(SnapZone), Restored }

pub struct DragMachine {
    window_id: Option<WindowId>,
    grab_offset: (i32, i32),
    preview_zone: Option<SnapZone>,
}

impl DragMachine {
    pub fn new() -> Self { Self { window_id: None, grab_offset: (0, 0), preview_zone: None } }
    pub fn begin(&mut self, window_id: WindowId, grab_offset: (i32, i32)) {
        self.window_id = Some(window_id);
        self.grab_offset = grab_offset;
        self.preview_zone = None;
    }
    pub fn motion(&mut self, pointer: (i32, i32), output: Rectangle, threshold: i32) {
        self.preview_zone = snap_zone_for_point(output, pointer, threshold);
    }
    pub fn preview_zone(&self) -> Option<SnapZone> { self.preview_zone }
    pub fn end(&mut self) -> DragResult {
        match (self.window_id.take(), self.preview_zone.take()) {
            (Some(_), Some(zone)) => DragResult::Snapped(zone),
            (Some(_), None) => DragResult::Moved,
            _ => DragResult::Restored,
        }
    }
}
```

The tests (appended) exercise matching, alt-tab cycling, and drag → snap:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_combo::KeyCombo;

    fn bindings() -> HashMap<String, KeyCombo> {
        let mut m = HashMap::new();
        m.insert("close".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_q".into() });
        m.insert("cycle:alt_tab".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_Tab".into() });
        m
    }

    #[test]
    fn exact_modifier_match() {
        assert_eq!(match_action(&bindings(), Modifiers::SUPER, 0x71), Some("close".into()));
        assert_eq!(match_action(&bindings(), Modifiers::SUPER | Modifiers::SHIFT, 0x71), None);
    }

    #[test]
    fn wrong_keysym_does_not_match() {
        assert_eq!(match_action(&bindings(), Modifiers::SUPER, 0x66), None);
    }

    #[test]
    fn alt_tab_wraps_around() {
        let mut m = AltTabMachine::new();
        m.start(vec![WindowId(1), WindowId(2), WindowId(3)]);
        assert_eq!(m.step(true), 1);
        assert_eq!(m.step(true), 2);
        assert_eq!(m.step(true), 0);
        assert_eq!(m.step(false), 2);
        assert_eq!(m.end(), Some(WindowId(3)));
        assert!(!m.is_active());
    }

    #[test]
    fn drag_to_edge_snaps() {
        let output = Rectangle { x: 0, y: 0, width: 1000, height: 800 };
        let mut m = DragMachine::new();
        m.begin(WindowId(1), (10, 10));
        m.motion((2, 400), output, 8);
        assert_eq!(m.preview_zone(), Some(SnapZone::Left));
        assert!(matches!(m.end(), DragResult::Snapped(SnapZone::Left)));
    }

    #[test]
    fn drag_center_moves() {
        let output = Rectangle { x: 0, y: 0, width: 1000, height: 800 };
        let mut m = DragMachine::new();
        m.begin(WindowId(1), (10, 10));
        m.motion((500, 400), output, 8);
        assert_eq!(m.preview_zone(), None);
        assert!(matches!(m.end(), DragResult::Moved));
    }
}
```

Note: the tests reference `crate::config_combo::KeyCombo` — create `compositor/src/config_combo.rs` re-exporting `icedtea_config::KeyCombo` (`pub use icedtea_config::KeyCombo;`) so `input.rs` and `compositor` don't depend on the config crate's name directly. Add `bitflags = "2"` to `compositor`'s dependencies.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib input`
Expected: fails — module not declared.

- [ ] **Step 3: Declare module + config re-export**

Add `pub mod config_combo;` and `pub mod input;` to `compositor/src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-compositor --lib input`
Expected: all 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add compositor/src/input.rs compositor/src/config_combo.rs compositor/src/main.rs compositor/Cargo.toml
git commit -m "feat(compositor): keybinding matcher and drag/alt-tab state machines"
```

---

### Task 10: SSD rendering and fullscreen behavior

**Files:**
- Modify: `compositor/src/render.rs` (SSD elements)
- Modify: `compositor/src/state.rs` (decoration hit-testing into window ops, fullscreen)
- Modify: `compositor/src/decoration.rs`

**Interfaces:**
- Consumes: `decoration::{hit_test, title_bar_rect, button_rects, DecorationAction, is_csd}` (Task 6), `render::wallpaper_color`.
- Produces:
  - `state::State::decoration_action_for(window_id, local: (i32,i32)) -> Option<DecorationAction>`
  - `state::State::toggle_fullscreen(id) -> Option<()>` — sets geometry to the output rect, flips `fullscreen`, emits `WindowUpdated`.
  - Rendering: windows whose `is_csd(app_id, client_decorations_requested)` is `false` get an SSD strip drawn from the palette (title text + min/max/close boxes at `button_rects`); fullscreen windows skip the strip and cover the output.

- [ ] **Step 1: Extend the decoration tests**

Append to `decoration.rs` tests:

```rust
#[test]
fn fullscreen_windows_have_no_bar() {
    let fs = Rectangle { x: 0, y: 0, width: 1920, height: 1080 };
    // fullscreen windows are rendered without an SSD strip; hit_test returns None
    assert_eq!(hit_test(fs, (fs.x + 5, fs.y + 5)), DecorationAction::Move);
}
```

Actually the meaningful assertion is at the state level (below). Keep this task's verification at the state-action level:

- [ ] **Step 2: Write the failing state tests**

`compositor/src/state.rs` tests append:

```rust
#[test]
fn decoration_action_round_trips_through_hit_test() {
    use crate::decoration::hit_test;
    let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
    let w = crate::window::Window {
        id: icedtea_contract::WindowId(1),
        app_id: "org.example".into(), title: "t".into(), pid: 1,
        workspace: 0, geometry: geo,
        maximized: false, minimized: false, fullscreen: false, focused: true,
    };
    // title-bar center -> Move (drag); rightmost button -> Close
    let close_pt = (geo.x + geo.width - 5, geo.y + 5);
    assert_eq!(hit_test(geo, close_pt), crate::decoration::DecorationAction::Close);
}

#[test]
fn toggle_fullscreen_flips_state_and_geometry() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1920, height: 1080 } });
    let id = state.window_manager.add_window("app", "t", 1);
    state.toggle_fullscreen(id).unwrap();
    let w = state.window_manager.get(id).unwrap();
    assert!(w.fullscreen);
    assert_eq!(w.geometry, Rectangle { x: 0, y: 0, width: 1920, height: 1080 });
}
```

Define the minimal `OutputSurface { geometry: Rectangle }` struct in `state.rs` and add `pub outputs: HashMap<u32, OutputSurface>` to `State` (anvil already tracks outputs; reuse its type where possible).

- [ ] **Step 3: Implement state helpers**

In `state.rs`:

```rust
pub fn toggle_fullscreen(&mut self, id: contract::WindowId) -> Option<()> {
    let w = self.window_manager.get(id)?;
    let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
    let target = !w.fullscreen;
    self.window_manager.set_fullscreen(id, target)?;
    if target {
        self.window_manager.set_geometry(id, output_geo)?;
    } else {
        self.window_manager.set_geometry(id, self.saved_geometry.remove(&id)?)?;
    }
    self.emit_pending();
    Some(())
}
```

Add `saved_geometry: HashMap<WindowId, Rectangle>` to `State`, populated in `set_geometry` when a window is not fullscreen (save the pre-toggle geometry). Wire `decoration_action_for` to dispatch on the focused window's `hit_test` and apply `Minimize`/`Maximize`/`Close`/`Move` (close → `remove_window`, etc.), emitting pending events. Double-click on the bar area (two `Motion`+`Button` presses within a timeout in the input handler) → `toggle_maximized`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p icedtea-compositor --lib state`
Expected: both new tests pass.

- [ ] **Step 5: Manual check (nested)**

Run: `cargo run -p icedtea-compositor -- --nested` and open a non-GTK client (e.g. `weston-terminal` if present). Expected: title bar strip with close button; clicking the close button closes the window; pressing the fullscreen key (`SUPER+F`) makes it cover the output and removes the strip.

- [ ] **Step 6: Commit**

```bash
git add compositor/src/render.rs compositor/src/state.rs compositor/src/decoration.rs
git commit -m "feat(compositor): SSD rendering and fullscreen handling"
```

---

### Task 11: Keybinding actions, workspaces from config, and input wiring

**Files:**
- Modify: `compositor/src/state.rs`
- Modify: `compositor/src/input.rs`
- Modify: `compositor/src/main.rs`

**Interfaces:**
- Consumes: `input::{match_action, AltTabMachine, DragMachine, Modifiers}`, `layout::{snapped_geometry, restored_geometry}`, `config::Config`.
- Produces:
  - `state::State::apply_action(&mut self, action: &str) -> Option<()>` — the action dispatcher (returns `None` for unrecognized actions). Actions: `close`, `fullscreen`, `reload`, `quit`, `cycle:alt_tab`, `spawn:terminal`, `workspace:N`, `move_to_workspace:N`, `snap:left|right|up|down|restore`.
  - `state::State::apply_config(&mut self, cfg: Config) -> Vec<Event>` — rebuilds workspaces from `workspace_names` and swaps keybindings/behavior; returns the events to emit (used by startup and hot-reload).
  - `state::State::snap(&mut self, id, zone: SnapZone) -> Option<()>` and `snap_restore(&mut self, id) -> Option<()>`.
  - `state::State::handle_key(&mut self, mods: Modifiers, keysym: u32) -> Option<()>` — wires `match_action` → `apply_action`.
  - `state::State::handle_pointer(&mut self, ...)` — wires `DragMachine` for move/snap and emits `AltTabState` on alt-tab changes.

- [ ] **Step 1: Write the failing dispatcher tests**

`compositor/src/state.rs` tests append:

```rust
#[test]
fn apply_action_switches_workspaces_and_snaps() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    let id = state.window_manager.add_window("app", "t", 1);
    state.apply_action("workspace:2").unwrap();
    assert_eq!(state.window_manager.active_workspace(), 1);
    state.apply_action("snap:left").unwrap();
    assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);
    state.apply_action("snap:restore").unwrap();
    assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 640);
}

#[test]
fn apply_config_rebuilds_workspaces() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    let mut cfg = icedtea_config::default_config();
    cfg.workspace_names = vec!["A".into(), "B".into()];
    let events = state.apply_config(cfg);
    assert_eq!(state.window_manager.workspace_info().len(), 2);
    assert!(events.iter().any(|e| matches!(e, Event::WorkspaceList(_))));
}

#[test]
fn close_action_removes_focused() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    let id = state.window_manager.add_window("app", "t", 1);
    state.apply_action("close").unwrap();
    assert!(state.window_manager.get(id).is_none());
}

#[test]
fn alt_tab_cycles_focus() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    let a = state.window_manager.add_window("a", "a", 1);
    let b = state.window_manager.add_window("b", "b", 2);
    state.apply_action("cycle:alt_tab").unwrap();
    assert!(state.window_manager.get(a).unwrap().focused);
    state.apply_action("cycle:alt_tab").unwrap();
    assert!(state.window_manager.get(b).unwrap().focused);
    state.apply_action("cycle:alt_tab").unwrap();
    assert!(state.window_manager.get(a).unwrap().focused);
}
```

The tests assume the window's default geometry is `640x400` placed at `(0,0)` on a `1000x800` output. To make that deterministic, `add_window` must receive a placement: change `WindowManager::add_window(app_id, title, pid, geometry)` — the compositor passes `layout::cascade_point(...)` on map. Update Task 4's `add_window` signature accordingly (4-arg → 5-arg) and update the Task 4 tests to pass a geometry. (The planner's Task 4 tests are adjusted in this task; the signature change is intentional.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib state`
Expected: fails (missing `apply_action`, `apply_config`, `snap`, `snap_restore`).

- [ ] **Step 3: Implement the dispatcher**

In `state.rs`:

```rust
pub fn apply_action(&mut self, action: &str) -> Option<()> {
    let mut parts = action.splitn(2, ':');
    let base = parts.next()?;
    let arg = parts.next().map(|s| s.to_string());
    match base {
        "close" => {
            let id = self.window_manager.focused_window()?.id;
            self.window_manager.remove_window(id);
        }
        "fullscreen" => {
            let id = self.window_manager.focused_window()?.id;
            self.toggle_fullscreen(id)?;
        }
        "quit" => self.quitting = true,
        "reload" => {
            let cfg = icedtea_config::load_or_default(&icedtea_config::default_db_path());
            let events = self.apply_config(cfg);
            self.pending_config_events.extend(events);
        }
        "cycle:alt_tab" => {
            let entries = self.window_manager.alt_tab_entries();
            if entries.is_empty() { return None; }
            if !self.alt_tab.is_active() {
                self.alt_tab.start(entries.clone());
            } else {
                self.alt_tab.step(true);
            }
            let idx = self.alt_tab.index();
            if let Some(wid) = entries.get(idx).copied() {
                self.window_manager.focus(wid);
            }
            self.emit(Event::AltTabState(AltTabState { active: true, entries, index: idx }));
        }
        "spawn" => {
            let cmd = arg?;
            std::process::Command::new("sh").arg("-c").arg(&cmd).spawn().ok();
        }
        "workspace" => {
            let n: u32 = arg?.parse().ok()?;
            self.window_manager.set_active_workspace(n - 1);
        }
        "move_to_workspace" => {
            let id = self.window_manager.focused_window()?.id;
            let n: u32 = arg?.parse().ok()?;
            self.window_manager.set_workspace(id, n - 1)?;
            self.window_manager.set_active_workspace(n - 1);
        }
        "snap" => {
            let id = self.window_manager.focused_window()?.id;
            let zone = match arg?.as_str() {
                "left" => SnapZone::Left,
                "right" => SnapZone::Right,
                "up" => SnapZone::Top,
                "down" => SnapZone::Bottom,
                _ => return None,
            };
            self.snap(id, zone)?;
        }
        "snap:restore" => {
            let id = self.window_manager.focused_window()?.id;
            self.snap_restore(id)?;
        }
        _ => return None,
    }
    self.emit_pending();
    Some(())
}

pub fn snap(&mut self, id: WindowId, zone: SnapZone) -> Option<()> {
    let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
    let gap = self.config.appearance.snap_gap;
    self.saved_geometry.entry(id).or_insert(self.window_manager.get(id)?.geometry);
    self.window_manager.set_geometry(id, snapped_geometry(output_geo, zone, gap))?;
    Some(())
}

pub fn snap_restore(&mut self, id: WindowId) -> Option<()> {
    if let Some(orig) = self.saved_geometry.remove(&id) {
        self.window_manager.set_geometry(id, orig)?;
    }
    Some(())
}

pub fn apply_config(&mut self, cfg: Config) -> Vec<Event> {
    self.config = cfg.clone();
    self.window_manager = WindowManager::new(cfg.workspace_names.clone());
    vec![Event::WorkspaceList(self.window_manager.workspace_info()),
         Event::ConfigReloaded(cfg.appearance.clone())]
}
```

Add to `State`: `pub alt_tab: input::AltTabMachine`, `pub quitting: bool`, `pub pending_config_events: Vec<Event>`. `handle_key` maps keysym+mods via `input::match_action(&self.config.keybindings, mods, keysym)` and applies. `handle_pointer` begins a `DragMachine` on `DecorationAction::Move`, updates preview on motion (rendering consumes `drag.preview_zone()`), and on release calls `snap`/`snap_restore` or moves the window to `pointer - grab_offset`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p icedtea-compositor --lib state`
Expected: all 4 dispatcher tests pass (plus earlier ones).

- [ ] **Step 5: Manual check (nested)**

Run the compositor with a client open. Expected: `SUPER+1..4` switch workspaces; `SUPER+LEFT` snaps the focused window to the left half with the gap; `SUPER+SHIFT+LEFT` restores it; `SUPER+TAB` cycles focus; `SUPER+Q` closes; dragging a window to an edge shows the translucent preview and snaps on release.

- [ ] **Step 6: Commit**

```bash
git add compositor/src/state.rs compositor/src/input.rs compositor/src/window.rs compositor/src/main.rs
git commit -m "feat(compositor): keybinding actions, workspace config, and pointer drag/snap wiring"
```

---

### Task 12: `dbus.rs` — the `org.icedtea.WM` service

**Files:**
- Create: `compositor/src/dbus.rs`
- Modify: `compositor/src/main.rs`
- Modify: `compositor/src/state.rs`

**Interfaces:**
- Consumes: `contract` types, `WindowManager` (Task 4), `config::Config`.
- Produces:
  - `dbus::spawn_service(events_rx: crossbeam_channel::Receiver<Event>, cmd_tx: crossbeam_channel::Sender<DbCommand>, quit_signal: Arc<AtomicBool>) -> zbus::blocking::Connection` — runs on a dedicated thread, returns the shared connection.
  - `dbus::DbCommand` enum: `Focus(WindowId), Close(WindowId), Minimize(WindowId, bool), Maximize(WindowId, bool), Fullscreen(WindowId, bool), SetWorkspace(u32), MoveToWorkspace(WindowId, u32), ReloadConfig, Quit, GetState(crossbeam_channel::Sender<Snapshot>)`
  - `dbus::WmInterface` — the `#[interface]` object, methods named exactly `FocusWindow`, `CloseWindow`, `MinimizeWindow`, `MaximizeWindow`, `FullscreenWindow`, `SetWorkspace`, `MoveWindowToWorkspace`, `GetState`, `ReloadConfig`, `Quit`.
  - Signal emission: `Event::WindowOpened` → `WindowOpened(info)`, `Event::WindowClosed` → `WindowClosed(id)`, `Event::WindowUpdated` → `WindowUpdated(id, update)`, `Event::WorkspaceSet` → `WorkspaceSet(id, active)`, `Event::WorkspaceList` → `WorkspaceList(workspaces)`, `Event::AltTabState` → `AltTabState(state)`, `Event::ConfigReloaded` → `ConfigReloaded(appearance)`.

- [ ] **Step 1: Write the interface type mapping tests**

`compositor/src/dbus.rs` (tests first):

```rust
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use icedtea_contract::{AltTabState, Appearance, Event, Rectangle, Snapshot, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};
use zbus::blocking::Connection;
use zbus::interface;

#[derive(Debug, Clone)]
pub enum DbCommand {
    Focus(WindowId),
    Close(WindowId),
    Minimize(WindowId, bool),
    Maximize(WindowId, bool),
    Fullscreen(WindowId, bool),
    SetWorkspace(u32),
    MoveToWorkspace(WindowId, u32),
    ReloadConfig,
    Quit,
    GetState(Sender<Snapshot>),
}

/// Map a contract::Event to its DBus signal name, so the emitter thread can
/// dispatch. Pure and unit-tested.
pub fn event_signal_name(event: &Event) -> &'static str {
    match event {
        Event::WindowOpened(_) => "WindowOpened",
        Event::WindowClosed(_) => "WindowClosed",
        Event::WindowUpdated { .. } => "WindowUpdated",
        Event::WorkspaceSet { .. } => "WorkspaceSet",
        Event::WorkspaceList(_) => "WorkspaceList",
        Event::AltTabState(_) => "AltTabState",
        Event::ConfigReloaded(_) => "ConfigReloaded",
    }
}
```

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_match_interface() {
        assert_eq!(event_signal_name(&Event::WindowOpened(sample_info())), "WindowOpened");
        assert_eq!(event_signal_name(&Event::WindowClosed(WindowId(1))), "WindowClosed");
        assert_eq!(event_signal_name(&Event::ConfigReloaded(default_appearance())), "ConfigReloaded");
    }

    fn sample_info() -> WindowInfo {
        WindowInfo { id: WindowId(1), app_id: "a".into(), title: "t".into(), pid: 1, workspace: 0,
            geometry: Rectangle { x: 0, y: 0, width: 10, height: 10 },
            maximized: false, minimized: false, fullscreen: false, focused: true }
    }
    fn default_appearance() -> Appearance {
        Appearance { bar_position: "bottom".into(), bar_height: 42, corner_radius: 8, snap_gap: 8,
            palette: icedtea_contract::Palette { background: "#000000".into(), foreground: "#ffffff".into(), accent: "#0000ff".into() },
            wallpaper: None }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib dbus`
Expected: fails — module not declared.

- [ ] **Step 3: Implement the interface and bridge**

Add `pub mod dbus;` to `main.rs`. In `dbus.rs`:

```rust
pub struct WmInterface {
    cmd_tx: Sender<DbCommand>,
    conn: Connection,
}

#[interface(name = "org.icedtea.WM")]
impl WmInterface {
    fn focus_window(&self, id: u32) {
        let _ = self.cmd_tx.send(DbCommand::Focus(WindowId(id)));
    }
    fn close_window(&self, id: u32) {
        let _ = self.cmd_tx.send(DbCommand::Close(WindowId(id)));
    }
    fn minimize_window(&self, id: u32, toggle: bool) {
        let _ = self.cmd_tx.send(DbCommand::Minimize(WindowId(id), toggle));
    }
    fn maximize_window(&self, id: u32, toggle: bool) {
        let _ = self.cmd_tx.send(DbCommand::Maximize(WindowId(id), toggle));
    }
    fn fullscreen_window(&self, id: u32, toggle: bool) {
        let _ = self.cmd_tx.send(DbCommand::Fullscreen(WindowId(id), toggle));
    }
    fn set_workspace(&self, id: u32) {
        let _ = self.cmd_tx.send(DbCommand::SetWorkspace(id));
    }
    fn move_window_to_workspace(&self, id: u32, workspace: u32) {
        let _ = self.cmd_tx.send(DbCommand::MoveToWorkspace(WindowId(id), workspace));
    }
    fn get_state(&self) -> Snapshot {
        // Synchronous round-trip: ask the compositor for a snapshot.
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        let _ = self.cmd_tx.send(DbCommand::GetState(reply_tx));
        reply_rx.recv().unwrap_or_else(|_| Snapshot { seq: 0, windows: vec![], workspaces: vec![], active_workspace: 0 })
    }
    fn reload_config(&self) {
        let _ = self.cmd_tx.send(DbCommand::ReloadConfig);
    }
    fn quit(&self) {
        let _ = self.cmd_tx.send(DbCommand::Quit);
    }
}
```

Emitter thread (in `spawn_service`):

```rust
pub fn spawn_service(events_rx: Receiver<Event>, cmd_tx: Sender<DbCommand>) -> Connection {
    let conn = zbus::blocking::Connection::session().expect("session bus available");
    let iface = WmInterface { cmd_tx: cmd_tx.clone(), conn: conn.clone() };
    conn.object_server().at(crate::WM_PATH, iface).expect("register interface");
    let _conn2 = conn.clone();
    std::thread::spawn(move || {
        while let Ok(event) = events_rx.recv() {
            let name = event_signal_name(&event);
            let payload = match &event {
                Event::WindowOpened(i) => (i.clone(),),
                Event::WindowClosed(id) => (id.0,),
                Event::WindowUpdated { id, update } => (id.0, update.clone()),
                Event::WorkspaceSet { id, active } => (*id, *active),
                Event::WorkspaceList(ws) => (ws.clone(),),
                Event::AltTabState(s) => (s.clone(),),
                Event::ConfigReloaded(a) => (a.clone(),),
            };
            let _ = _conn2.emit_signal(None, crate::WM_PATH, "org.icedtea.WM", name, &payload);
        }
    });
    conn
}
```

`GetState` synchronously round-trips through the command channel, so the compositor must handle it: in `state.rs`, on `DbCommand::GetState(tx)` send `self.window_manager.snapshot()`. Wire the command channel into the compositor's calloop loop via `calloop::channel` (anvil uses calloop channels; follow `anvil/src/input_handler.rs`'s use of `insert_channel`). Incoming `DbCommand`s map to model ops and `emit_pending()`.

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p icedtea-compositor --lib dbus`
Expected: `event_names_match_interface` passes.

- [ ] **Step 5: Manual verification with a real session bus**

With the compositor running (`--nested` or DRM) inside a session with `DBUS_SESSION_BUS_ADDRESS` set:

```bash
gdbus call --session --dest org.icedtea.WM --object-path /org/icedtea/WM --method org.icedtea.WM.GetState
```
Expected: a snapshot with the open windows (empty `windows` is fine if none open).

With a client window open, run:
```bash
gdbus call --session --dest org.icedtea.WM --object-path /org/icedtea/WM --method org.icedtea.WM.CloseWindow 1
```
Expected: the window closes. `busctl --user monitor` shows `WindowOpened`/`WindowClosed` signals.

- [ ] **Step 6: Commit**

```bash
git add compositor/src/dbus.rs compositor/src/main.rs compositor/src/state.rs
git commit -m "feat(compositor): org.icedtea.WM DBus service with channel bridge"
```

---

### Task 13: Config hot reload over `ReloadConfig`

**Files:**
- Modify: `compositor/src/dbus.rs`
- Modify: `compositor/src/state.rs`

**Interfaces:**
- Consumes: `config::{load_or_default, default_db_path, Config}` (Task 3), `state::State::apply_config` (Task 11), `DbCommand::ReloadConfig` (Task 12).
- Produces:
  - `state::State::handle_command(&mut self, cmd: dbus::DbCommand) -> Option<()>` — central dispatcher for all DBus commands.
  - On `ReloadConfig`: re-`load_or_default`, `apply_config`, append `ConfigReloaded(appearance)` to the pending event list, emit.

- [ ] **Step 1: Write the failing reload test**

`compositor/src/state.rs` tests append:

```rust
#[test]
fn reload_config_applies_new_workspaces_and_emits() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    // Persist a config with different workspace names, then reload from disk.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg.redb");
    let db = icedtea_config::open(&path).unwrap();
    let mut cfg = icedtea_config::default_config();
    cfg.workspace_names = vec!["A".into(), "B".into(), "C".into()];
    cfg.save(&db).unwrap();
    drop(db);

    state.config_path = Some(path.clone());
    let events = state.reload_config_from_disk();
    assert_eq!(state.window_manager.workspace_info().len(), 3);
    assert!(events.iter().any(|e| matches!(e, Event::ConfigReloaded(_))));
    let emitted = rx.try_iter().collect::<Vec<_>>();
    assert!(emitted.iter().any(|e| matches!(e, Event::ConfigReloaded(_))));
}
```

Add `pub config_path: Option<PathBuf>` to `State`; `reload_config_from_disk()` loads and applies, returning events.

Add `tempfile = "3"` as a dev-dependency of `compositor`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-compositor --lib state reload_config`
Expected: fails — `config_path`/`reload_config_from_disk` missing.

- [ ] **Step 3: Implement**

`state.rs`:

```rust
pub fn reload_config_from_disk(&mut self) -> Vec<Event> {
    let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
    let cfg = icedtea_config::load_or_default(&path);
    let mut events = self.apply_config(cfg);
    if let Some(appearance) = self.config.appearance.clone().into() {
        events.push(Event::ConfigReloaded(self.config.appearance.clone()));
    }
    events
}
```

Wire `handle_command`:

```rust
pub fn handle_command(&mut self, cmd: dbus::DbCommand) -> Option<()> {
    match cmd {
        DbCommand::Focus(id) => self.window_manager.focus(id)?,
        DbCommand::Close(id) => { self.window_manager.remove_window(id); }
        DbCommand::Minimize(id, v) => self.window_manager.set_minimized(id, v)?,
        DbCommand::Maximize(id, v) => self.window_manager.set_maximized(id, v)?,
        DbCommand::Fullscreen(id, v) => { self.window_manager.set_fullscreen(id, v)?; }
        DbCommand::SetWorkspace(id) => { self.window_manager.set_active_workspace(id); }
        DbCommand::MoveToWorkspace(id, ws) => { self.window_manager.set_workspace(id, ws)?; self.window_manager.set_active_workspace(ws); }
        DbCommand::GetState(tx) => { let _ = tx.send(self.window_manager.snapshot()); }
        DbCommand::ReloadConfig => { self.reload_config_from_disk(); }
        DbCommand::Quit => self.quitting = true,
    }
    self.emit_pending();
    Some(())
}
```

Simplify `reload_config_from_disk`'s event push (the `.into()` in the draft is wrong): just `events.push(Event::ConfigReloaded(self.config.appearance.clone()));`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p icedtea-compositor --lib state`
Expected: reload test passes.

- [ ] **Step 5: Manual verification**

With the compositor running, from a shell that can write the DB:
```bash
gdbus call --session --dest org.icedtea.WM --object-path /org/icedtea/WM --method org.icedtea.WM.ReloadConfig
```
Expected: `busctl --user monitor` shows a `ConfigReloaded` signal; if the on-disk config changed workspace names, the model reflects it (observable via `GetState`).

- [ ] **Step 6: Commit**

```bash
git add compositor/src/state.rs compositor/src/dbus.rs compositor/Cargo.toml
git commit -m "feat(compositor): config hot reload via ReloadConfig"
```

---

### Task 14: End-to-end verification pass

**Files:**
- Create: `compositor/src/window.rs` additions (test-only helper)
- Modify: `compositor/tests/` — create `compositor/tests/end_to_end.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–13.
- Produces: a `WaylandClientHarness` in `tests/end_to_end.rs` that drives `WindowManager` + `apply_action` + DBus `DbCommand` dispatch without a real bus, plus a documented manual session checklist.

- [ ] **Step 1: Write the model-level end-to-end test**

`compositor/tests/end_to_end.rs` (integration test, exercises the real `WindowManager`, `apply_action`, and `handle_command`):

```rust
use icedtea_contract::{Event, WindowId};
use icedtea_compositor::dbus::DbCommand;
use icedtea_compositor::state::State;
use icedtea_config::default_config;

#[test]
fn window_lifecycle_and_dbus_commands() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);

    let id = state.window_manager.add_window("org.test.App", "App", 42, icedtea_contract::Rectangle { x: 0, y: 0, width: 640, height: 400 });
    assert!(matches!(rx.try_recv(), Ok(Event::WindowOpened(_))));

    // DBus commands mutate the model.
    state.handle_command(DbCommand::Focus(id)).unwrap();
    assert!(state.window_manager.get(id).unwrap().focused);

    state.handle_command(DbCommand::SetWorkspace(1)).unwrap();
    assert_eq!(state.window_manager.active_workspace(), 1);

    state.handle_command(DbCommand::Close(id)).unwrap();
    assert!(state.window_manager.get(id).is_none());
    assert!(matches!(rx.try_recv(), Ok(Event::WindowClosed(_))));
}

#[test]
fn action_dispatch_cover_all_default_actions() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.window_manager.add_window("a", "a", 1, icedtea_contract::Rectangle { x: 0, y: 0, width: 640, height: 400 });
    for action in ["close", "fullscreen", "workspace:2", "move_to_workspace:1",
                   "snap:left", "snap:restore", "cycle:alt_tab", "reload"] {
        assert!(state.apply_action(action).is_some(), "action {action} should dispatch");
    }
}
```

Note: `State::new`, `apply_action`, and `handle_command` must be `pub`; `add_window` now takes 5 args (geometry), matching Task 11's signature change. Re-export the compositor's modules from `lib` — add `compositor/src/lib.rs` exposing `pub mod state; pub mod dbus; pub mod window; pub mod layout; pub mod decoration; pub mod input; pub mod render;`, and keep `main.rs` as a thin `fn main()` calling the library's `run()` (mirror anvil's `main.rs`/`lib.rs` split). Adjust the crate's `Cargo.toml` to have a lib target (`[lib] name = "icedtea_compositor"`).

- [ ] **Step 2: Run the end-to-end tests**

Run: `cargo test -p icedtea-compositor`
Expected: all tests pass, including `end_to_end`.

- [ ] **Step 3: Manual real-session checklist**

On a real DRM session (from a VT with the user in `seat`, or via `--nested` from an existing Wayland session):

- [ ] `RUST_LOG=info icedtea-compositor` boots on DRM and logs outputs.
- [ ] `foot` / any wayland client opens and is listed by `gdbus ... GetState`.
- [ ] Drag by a title bar moves the window; dragging to a screen edge shows the translucent preview and snaps on release; dragging back to center restores.
- [ ] `SUPER+1`..`SUPER+4` switch workspaces; windows follow their workspace.
- [ ] `SUPER+TAB` cycles focus (alt-tab); `SUPER+Q` closes.
- [ ] `SUPER+F` fullscreens (SSD strip disappears); `SUPER+F` again restores.
- [ ] `SUPER+SHIFT+R` reloads config from redb (observe `ConfigReloaded` in `busctl --user monitor`).
- [ ] `gdbus call ... GetState` returns a well-formed snapshot; `CloseWindow`/`SetWorkspace` work from the bus.
- [ ] A killed shell/client window does not take down the compositor.

- [ ] **Step 4: Commit**

```bash
git add compositor/src/lib.rs compositor/src/main.rs compositor/tests/end_to_end.rs compositor/Cargo.toml
git commit -m "test(compositor): end-to-end model and action-dispatch coverage"
```

---

## Self-review notes

- **Spec coverage:** Plan 1 delivers every compositor-core item from the spec: window model + workspaces (T4, T11), layout/snapping (T5, T11), SSD hybrid via `xdg-decoration` + `decoration.rs` (T6, T10), wallpaper + scene + snap preview (T8), input/keybindings/alt-tab (T9, T11), fullscreen (T10), config crate + redb + defaults (T3), `org.icedtea.WM` DBus service + channel bridge (T12), `ReloadConfig` hot reload (T13), and the nested/DRM bootstrap (T7). Shell, settings, logind, and systemd units are out of scope by design (Plans 2–4).
- **Type consistency:** `add_window` grows a geometry argument in T11 — Task 4 and Task 14 are adjusted to match. `DbCommand`, `Event`, `WindowId`, `SnapZone`, `DragResult`, `Modifiers`, and `KeyCombo` names are used identically across all tasks.
- **Known external-dependency caveats:** the pinned anvil source is the authority for any 0.7 API drift (notably `EventLoop` construction and `insert_channel`); where the plan sketches a call that differs, follow anvil. `Database::create/open` are wrapped in `unsafe` per redb's API.
