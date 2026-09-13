//! The B1 app launcher surface (Task 5): a Win7/10-style Start menu as a
//! dedicated layer-shell surface in the shell process.
//!
//! The pure core (`launcher/`) owns the index, matcher and stores; this
//! module owns the surface: [`spec()`] (corner-anchored `Layer::Top` with
//! exclusive keyboard focus), [`LauncherModel`]/[`LauncherMsg`]/[`update`]
//! (search → filter → Enter-to-launch, arrows across panes, Escape close,
//! right-click pin/unpin) and [`view()`] (search box, pinned rail, All-apps
//! list, tiles pane, power row).
//!
//! The power row shells out per `launcher::power_argv` (logout takes the
//! compositor quit path); every failure surfaces as `status`, never
//! silently.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

use icedtea_ui::view::builders::{SearchEntryExt, box_, button, label, search_entry};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::keyboard::KeyEvent;
use icedtea_ui::window::pointer::{BTN_LEFT, BTN_RIGHT};
use icedtea_ui::window::{LayerSpec, Role, SurfaceSpec};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use xkbcommon::xkb;

use crate::compositor_client::CompositorCommands;
use crate::launcher::{
    DesktopEntry, DesktopIndex, Matcher, PinStore, RecencyStore, TileStore, default_dirs,
    power_argv, power_error_message,
};

/// The launcher's initial surface size: a menu, not a bar — fixed, modest,
/// content-laid-out inside it.
pub const LAUNCHER_SIZE: (u32, u32) = (400, 500);

/// One tile grid unit in px: a size-`n` tile requests `n` units of width.
pub const TILE_PX: i32 = 96;

/// The launcher's surface: corner-anchored `Layer::Top`, keyboard-exclusive,
/// no exclusive zone.
///
/// The anchored corner follows `Appearance.bar_position` (`"bottom"`
/// default, `"top"` option — the same knob as [`crate::panel::spec`], never
/// a setting of its own): bottom anchors Left+Bottom, anything else
/// Left+Top. Unlike the bar it never spans an output edge (no Right
/// anchor). `keyboard: Exclusive` — verified against
/// `wayland-protocols-wlr`'s own `wlr-layer-shell-unstable-v1.xml`
/// (`keyboard_interactivity/exclusive`): the menu needs keyboard focus,
/// where the panel takes `None`. `exclusive_zone: 0`: the menu overlays,
/// never pushing windows around (spec §Architecture).
#[must_use]
pub fn spec(bar_position: &str) -> SurfaceSpec {
    let edge = if bar_position == "top" {
        zwlr_layer_surface_v1::Anchor::Top
    } else {
        zwlr_layer_surface_v1::Anchor::Bottom
    };
    SurfaceSpec {
        role: Role::Layer(LayerSpec {
            layer: zwlr_layer_shell_v1::Layer::Top,
            anchor: zwlr_layer_surface_v1::Anchor::Left | edge,
            margin: [0, 0, 0, 0],
            exclusive_zone: 0,
            keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        }),
        size: LAUNCHER_SIZE,
        // A layer surface has no `namespace` field: `title` is the namespace.
        title: "icedtea-launcher".to_string(),
        app_id: "org.icedtea.Launcher".to_string(),
    }
}

/// Re-exported for the power row's existing call sites: the enum itself
/// lives in the pure core (`launcher`) beside its argv contract.
pub use crate::launcher::PowerAction;

#[derive(Clone, Debug)]
pub enum LauncherMsg {
    /// The search box changed (per keystroke and debounced alike).
    SearchChanged(String),
    /// Enter inside the search box.
    SearchActivated,
    /// Arrow keys: Up/Down move within the filtered list, Left/Right jump
    /// across panes (pinned / all-apps / tiles section starts).
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    /// Enter outside the search box: launch the selection, else the top hit.
    ActivateSelected,
    /// Left-click (or pointer release with `BTN_LEFT`) on an app row.
    ActivateApp(String),
    /// Right-click on an app row.
    TogglePin(String),
    /// Any other pointer button on an app row: inert by design.
    Ignore,
    /// A single-letter jump in the All-apps list (empty query only).
    LetterJump(char),
    /// The power row: shell-outs per `launcher::power_argv`, logout via
    /// the compositor quit path.
    Power(PowerAction),
    /// Escape or the Start button toggling shut. No *keyboard* focus-loss
    /// path exists under `Exclusive` on a compliant compositor; a
    /// compositor-driven unmap/kill is covered by the supervisor's
    /// done-channel reap.
    Close,
}

/// Contract §3.1: the inbox carries `LauncherMsg` across a thread, so it
/// must be `Send` — the supervisor thread owns the sender while the App
/// owns the loop.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LauncherMsg>();
};

/// The launcher's model: the pure core's stores plus the view state.
///
/// `wm` is `Rc`, not `Arc`: like the panel's, it never crosses a thread —
/// `update` runs on the launcher loop's thread, and so does `Cmd::Task`.
pub struct LauncherModel {
    index: DesktopIndex,
    pins: PinStore,
    tiles: TileStore,
    recency: RecencyStore,
    query: String,
    selected: Option<usize>,
    open: bool,
    status: Option<String>,
    /// XDG dirs rescanned on [`LauncherModel::open`]. `pub` for the
    /// supervisor (production) and hermetic tmpdirs (tests).
    pub dirs: Vec<PathBuf>,
    scanned: Option<SystemTime>,
    wm: Rc<dyn CompositorCommands>,
    /// Fires after a close fold (successful launch, Escape): the supervisor
    /// resets the panel's `launcher_open` bit through it.
    pub on_close: Option<Rc<dyn Fn()>>,
    /// How a power action shells out. Defaults to [`run_power_command`];
    /// tests override it so no test ever execs `loginctl`/`systemctl`.
    power_run: fn(&str, &[&str]) -> std::io::Result<()>,
    /// Write-back for the pin/tile/recency stores. Called with
    /// [`LauncherModel::snapshot`] on every close (successful launch,
    /// power action, Escape); `run_surface` points it at the config DB,
    /// so a reopened menu still shows the pins and recency. `None` (unit
    /// tests) means no write-back.
    pub on_persist: Option<Rc<dyn Fn(icedtea_config::LauncherConfig)>>,
}

/// Persist one launcher snapshot through the scoped
/// [`icedtea_config::Config::save_launcher`] (launcher table only — never
/// a full [`icedtea_config::Config::save`], which would clobber a
/// concurrent settings-app edit with this process's stale in-memory
/// copy). Reads the current file first so the other sections survive;
/// every failure comes back as `Err`, never a panic.
pub fn persist_launcher_config(
    db_path: &std::path::Path,
    launcher: &icedtea_config::LauncherConfig,
) -> Result<(), String> {
    let mut full = icedtea_config::load_or_default(db_path);
    full.launcher = launcher.clone();
    let db = icedtea_config::open(db_path)
        .map_err(|err| format!("launcher write-back: cannot open config db: {err}"))?;
    full.save_launcher(&db)
        .map_err(|err| format!("launcher write-back: cannot save: {err}"))?;
    Ok(())
}

/// The one real power shell-out (spec §Spawn+power): `program` + `args` is
/// exactly [`power_argv`]'s output. Both an io failure (binary missing) and
/// a non-zero exit (polkit-denied, the spec's own risk) are failures, so a
/// refusal surfaces as a launcher status line, never silently.
// A3-logind: replace with logind client.
fn run_power_command(program: &str, args: &[&str]) -> std::io::Result<()> {
    // A3-logind: replace with logind client.
    let status = std::process::Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with {status}"
        )))
    }
}

impl LauncherModel {
    #[must_use]
    pub fn new(wm: Rc<dyn CompositorCommands>) -> Self {
        Self {
            index: DesktopIndex::default(),
            pins: PinStore::default(),
            tiles: TileStore::default(),
            recency: RecencyStore::default(),
            query: String::new(),
            selected: None,
            open: false,
            status: None,
            dirs: default_dirs(),
            scanned: None,
            wm,
            on_close: None,
            power_run: run_power_command,
            on_persist: None,
        }
    }

    /// Seed the pin/tile/recency stores from the persisted config (read
    /// path; the write-back is [`LauncherModel::close`] → `on_persist` →
    /// [`persist_launcher_config`], wired in `run_surface`).
    pub fn seed(&mut self, config: &icedtea_config::LauncherConfig) {
        for id in &config.pinned {
            self.pins.pin(id);
        }
        for group in &config.tile_groups {
            self.tiles.ingest(&group.name, &group.ids, group.size);
        }
        self.recency.restore(&config.recency);
    }

    /// Open (or re-open) the menu: mtime-gated rescan, then a blank query,
    /// no selection, no status. Steady-state opens do no I/O: the rescan
    /// runs only when [`fingerprint`] moved (spec §Index).
    pub fn open(&mut self) {
        let current = fingerprint(&self.dirs);
        if self.scanned != current {
            self.index = DesktopIndex::from_entries(DesktopIndex::scan(&self.dirs));
            self.scanned = current;
        }
        self.query.clear();
        self.selected = None;
        self.status = None;
        self.open = true;
    }

    /// The current filtered list: the matcher over name + keywords + exec
    /// basename, recency as tiebreak. Empty query returns every app in
    /// index order.
    fn filtered(&self) -> Vec<&DesktopEntry> {
        Matcher::new(&self.index)
            .with_recency(&self.recency)
            .rank(&self.query)
    }

    /// Which pane an entry renders in: pinned rail (0), All-apps (1), tiles
    /// (2). Pinned wins over tiled — a pinned tile still reads as pinned.
    fn section_of(&self, entry: &DesktopEntry) -> u8 {
        if self.pins.is_pinned(&entry.id) {
            0
        } else if self
            .tiles
            .groups()
            .iter()
            .any(|group| group.ids.iter().any(|id| id == &entry.id))
        {
            2
        } else {
            1
        }
    }

    /// Launch one app id: `spawn_app` on success records recency and closes;
    /// on failure (or an unknown id) a status line explains, and the menu
    /// stays open.
    fn launch(&mut self, id: &str) -> Cmd<LauncherMsg> {
        let Some(entry) = self.index.find(id) else {
            self.status = Some(format!("Unknown application: {id}"));
            return Cmd::None;
        };
        if self.wm.spawn_app(id) {
            self.recency.record(id);
            self.close()
        } else {
            self.status = Some(format!("Could not launch {}", entry.name));
            Cmd::None
        }
    }

    /// The pin/tile/recency stores as one persistable config section: what
    /// [`LauncherModel::close`] hands to `on_persist`, and what
    /// [`LauncherModel::seed`] reads back on the next open.
    pub fn snapshot(&self) -> icedtea_config::LauncherConfig {
        icedtea_config::LauncherConfig {
            pinned: self.pins.pinned().to_vec(),
            tile_groups: self
                .tiles
                .groups()
                .iter()
                .map(|group| icedtea_config::TileGroup {
                    name: group.name.clone(),
                    ids: group.ids.clone(),
                    size: group.size,
                })
                .collect(),
            recency: self.recency.snapshot(),
        }
    }

    /// Dismiss the menu and leave the loop; the panel learns through
    /// `on_close` (a `Cmd::Task`: the loop runs it after the fold, before
    /// `Quit` takes effect — `Cmd::flatten` preserves the order).
    ///
    /// The close also writes the stores back through `on_persist` first,
    /// so pin/unpin + recency mutations reach `Config::save` even when the
    /// menu shuts without a launch (Escape, power action).
    fn close(&mut self) -> Cmd<LauncherMsg> {
        self.open = false;
        if let Some(persist) = self.on_persist.clone() {
            persist(self.snapshot());
        }
        match self.on_close.clone() {
            Some(notify) => Cmd::Batch(vec![Cmd::Task(Rc::new(move || notify())), Cmd::Quit]),
            None => Cmd::Quit,
        }
    }

    /// Launch the selection, else the top hit; nothing when the list is
    /// empty. A stale selection (list shrank under it) falls back to the
    /// top hit rather than launching nothing.
    fn launch_selected(&mut self) -> Cmd<LauncherMsg> {
        let ids: Vec<String> = self.filtered().iter().map(|e| e.id.clone()).collect();
        let Some(id) = self
            .selected
            .and_then(|i| ids.get(i))
            .or_else(|| ids.first())
        else {
            return Cmd::None;
        };
        self.launch(id)
    }
}

/// Fold one message. Outbound commands leave as `Cmd` (the loop runs them
/// after the fold); the one exception is [`CompositorCommands::spawn_app`],
/// called synchronously because the fold needs its bool to decide between
/// recency-plus-close and the status line (Task 6's e2e pins exactly this:
/// `Enter → spawn_app(entry.id) → on true, record recency + close`). The
/// proxy maps every bus failure to `false`, never a panic, so the fold
/// never blocks longer than a local D-Bus round trip.
pub fn update(m: &mut LauncherModel, msg: LauncherMsg) -> Cmd<LauncherMsg> {
    match msg {
        LauncherMsg::SearchChanged(query) => {
            m.query = query;
            m.selected = None;
            Cmd::None
        }
        LauncherMsg::SearchActivated | LauncherMsg::ActivateSelected => m.launch_selected(),
        LauncherMsg::MoveUp | LauncherMsg::MoveDown => {
            let up = matches!(msg, LauncherMsg::MoveUp);
            let len = m.filtered().len();
            m.selected = match (m.selected, len) {
                (_, 0) => None,
                (None, _) => {
                    if up {
                        Some(len - 1)
                    } else {
                        Some(0)
                    }
                }
                (Some(i), _) => {
                    // Defensive clamp: every list-shrinking path resets the
                    // selection today, but an index must never escape it.
                    let current = i.min(len - 1);
                    if up {
                        Some(current.saturating_sub(1))
                    } else {
                        Some((current + 1).min(len - 1))
                    }
                }
            };
            Cmd::None
        }
        LauncherMsg::MoveLeft | LauncherMsg::MoveRight => {
            let right = matches!(msg, LauncherMsg::MoveRight);
            let entries = m.filtered();
            if entries.is_empty() {
                m.selected = None;
                return Cmd::None;
            }
            // Pane runs over the filtered list: consecutive entries in the
            // same section (pinned rail / All-apps / tiles) form one run.
            // Left/Right jump between run starts — Up/Down own the
            // within-pane steps.
            let sections: Vec<u8> = entries.iter().map(|e| m.section_of(e)).collect();
            let mut starts = vec![0usize];
            for i in 1..sections.len() {
                if sections[i] != sections[i - 1] {
                    starts.push(i);
                }
            }
            let current = m.selected.unwrap_or(0).min(entries.len() - 1);
            m.selected = if right {
                starts
                    .iter()
                    .find(|&&s| s > current)
                    .copied()
                    .or(Some(entries.len() - 1))
            } else {
                starts.iter().rfind(|&&s| s < current).copied().or(Some(0))
            };
            Cmd::None
        }
        LauncherMsg::ActivateApp(id) => m.launch(&id),
        LauncherMsg::TogglePin(id) => {
            if m.pins.is_pinned(&id) {
                m.pins.unpin(&id);
            } else {
                m.pins.pin(&id);
            }
            Cmd::None
        }
        LauncherMsg::Ignore => Cmd::None,
        LauncherMsg::LetterJump(letter) => {
            if !m.query.trim().is_empty() {
                return Cmd::None;
            }
            let needle = letter.to_lowercase().to_string();
            m.selected = m
                .filtered()
                .iter()
                .position(|e| e.name.to_lowercase().starts_with(&needle));
            Cmd::None
        }
        // Power backends (spec §Spawn+power): each action shells out per
        // [`power_argv`], except logout, which takes the compositor quit
        // path. A shell-out failure surfaces as `m.status`, never silently;
        // a success dismisses the menu (the session is going away, or the
        // locker covers it).
        LauncherMsg::Power(action) => match power_argv(action) {
            // A3-logind: replace with logind client (the exec lives in
            // `run_power_command`; this arm only routes its outcome).
            Some((program, args)) => match (m.power_run)(program, args) {
                Ok(()) => m.close(),
                Err(err) => {
                    m.status = Some(power_error_message(action, &err.to_string()));
                    Cmd::None
                }
            },
            // Logout: the compositor quit path, never a shell-out.
            None => {
                m.wm.quit();
                m.close()
            }
        },
        LauncherMsg::Close => m.close(),
    }
}

/// The `.desktop` world's mtime fingerprint: the newest modification time
/// over `dirs`' `*.desktop` children (metadata only — no file reads, so a
/// steady-state [`LauncherModel::open`] does no I/O). `None` when no dir
/// holds a `.desktop` file; any install, removal or edit moves it.
fn fingerprint(dirs: &[PathBuf]) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = None;
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            let modified = entry.metadata().and_then(|md| md.modified()).ok();
            latest = latest.max(modified);
        }
    }
    latest
}

/// The whole menu. `#launcher` is what `style.css`'s launcher rule names.
///
/// Layout is the spec's Win7 left rail + Win10 right pane: search box on
/// top, pinned rail over the alphabetical All-apps list on the left, the
/// tile groups on the right, the power row at the bottom. The status line
/// renders only while some failure owns one. The top hit carries
/// `.suggested-action` (existing toolkit vocabulary — no new theme
/// machinery); the keyboard selection carries `active`, the same class the
/// bar uses for the same "this is the one" meaning.
pub fn view(m: &LauncherModel) -> View<LauncherMsg> {
    let filtered = m.filtered();
    let top = filtered.first().map(|e| e.id.clone());
    let mut children = vec![search_box(m), body(m, &filtered, top.as_deref())];
    if let Some(status) = &m.status {
        children.push(label(status).id("launcher_status"));
    }
    children.push(power_row());
    box_(Orientation::Vertical, children)
        .id("launcher")
        .on_key(root_key)
}

/// The IME-capable search box (M6.1 Spec 2, the same `SearchEntry` the
/// clipboard popover uses): per-keystroke `on_change` filtering — IME
/// commits land as buffer writes — plus the debounced `on_search`, both
/// folding [`LauncherMsg::SearchChanged`]; Enter launches the top hit.
fn search_box(m: &LauncherModel) -> View<LauncherMsg> {
    search_entry(&m.query)
        .id("launcher_search")
        .placeholder("Search applications")
        .on_change(|text: &str| LauncherMsg::SearchChanged(text.to_owned()))
        .on_search(|text: &str| LauncherMsg::SearchChanged(text.to_owned()))
        .on_activate(LauncherMsg::SearchActivated)
}

/// The menu body: left rail (pinned, then All-apps) beside the tiles pane.
fn body(m: &LauncherModel, filtered: &[&DesktopEntry], top: Option<&str>) -> View<LauncherMsg> {
    let ids: Vec<&str> = filtered.iter().map(|e| e.id.as_str()).collect();
    box_(
        Orientation::Horizontal,
        [left_rail(m, filtered, top), tiles_pane(m, &ids)],
    )
    .id("launcher_body")
}

/// Pinned rail over the scrollable alphabetical All-apps list.
fn left_rail(
    m: &LauncherModel,
    filtered: &[&DesktopEntry],
    top: Option<&str>,
) -> View<LauncherMsg> {
    let pinned: Vec<&DesktopEntry> = filtered
        .iter()
        .filter(|e| m.pins.is_pinned(&e.id))
        .copied()
        .collect();
    let pinned_rows: Vec<View<LauncherMsg>> = pinned
        .iter()
        .map(|e| app_row(e, RowKind::Pinned, m.selected_for(&e.id, filtered), top))
        .collect();
    let all_rows: Vec<View<LauncherMsg>> = filtered
        .iter()
        .map(|e| app_row(e, RowKind::All, m.selected_for(&e.id, filtered), top))
        .collect();
    // Letter jump lives on the All-apps list: with an empty query a
    // single-letter key selects the first app starting with it; while
    // filtering, letters belong to the search box instead.
    let query_empty = m.query.trim().is_empty();
    let all = box_(Orientation::Vertical, all_rows)
        .id("launcher_all")
        .on_key(move |ev: &KeyEvent| {
            if !query_empty {
                return None;
            }
            let text = ev.utf8.as_deref().unwrap_or("");
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(LauncherMsg::LetterJump(c)),
                _ => None,
            }
        });
    box_(
        Orientation::Vertical,
        [
            box_(Orientation::Vertical, pinned_rows).id("launcher_pinned"),
            all,
        ],
    )
    .id("launcher_left")
}

/// The tiles pane: one group container per tile group, holding the member
/// rows that match the filter. Each tile's width follows its group's
/// [`TileGroup`](crate::launcher::TileGroup) `size` in [`TILE_PX`] units.
fn tiles_pane(m: &LauncherModel, filtered_ids: &[&str]) -> View<LauncherMsg> {
    let groups: Vec<View<LauncherMsg>> = m
        .tiles
        .groups()
        .iter()
        .filter_map(|group| {
            let members: Vec<View<LauncherMsg>> = group
                .ids
                .iter()
                .filter(|id| filtered_ids.contains(&id.as_str()))
                .filter_map(|id| m.index.find(id))
                .map(|e| tile_row(e, group.size))
                .collect();
            if members.is_empty() {
                return None;
            }
            Some(
                box_(
                    Orientation::Vertical,
                    [label(&group.name), box_(Orientation::Vertical, members)],
                )
                .id(&format!("tile_group_{}", group.name)),
            )
        })
        .collect();
    box_(Orientation::Vertical, groups).id("launcher_tiles")
}

/// Where a row renders: the id prefix keeps probe ids unique across panes.
#[derive(Clone, Copy)]
enum RowKind {
    Pinned,
    All,
}

/// One app row: click launches, right-click pins/unpins, any other button
/// is inert. Exactly one handler (`on_pointer_up_with_button`) so a left
/// release can never double-fire through a second `on_click` (M5-D5 §5).
fn app_row(
    entry: &DesktopEntry,
    kind: RowKind,
    selected: bool,
    top: Option<&str>,
) -> View<LauncherMsg> {
    let prefix = match kind {
        RowKind::Pinned => "pinned",
        RowKind::All => "app",
    };
    let id = entry.id.clone();
    let launch_id = id.clone();
    let pin_id = id.clone();
    let mut row = button(&entry.name)
        .key(stable_key(&id))
        .id(&format!("{prefix}_{id}"))
        .on_pointer_up_with_button(move |_, _, pressed| {
            if pressed == BTN_RIGHT {
                LauncherMsg::TogglePin(pin_id.clone())
            } else if pressed == BTN_LEFT {
                LauncherMsg::ActivateApp(launch_id.clone())
            } else {
                LauncherMsg::Ignore
            }
        });
    if top == Some(id.as_str()) {
        row = row.class("suggested-action");
    }
    if selected {
        row = row.class("active");
    }
    row
}

/// One tile: same launch/right-click model as an app row, `id`-prefixed
/// `tile_`, width in group-size units.
fn tile_row(entry: &DesktopEntry, size: u32) -> View<LauncherMsg> {
    let id = entry.id.clone();
    let launch_id = id.clone();
    let pin_id = id.clone();
    // Saturating width math: `size` is config-controlled (hand-editable
    // redb), so a huge span must clamp, never wrap (debug) or truncate the
    // surface (release). 1024 units is already an absurd menu; the product
    // below cannot overflow `i32`.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the span is capped at 1024 units, far below i32::MAX / TILE_PX"
    )]
    let width = size.min(1024) as i32 * TILE_PX;
    button(&entry.name)
        .key(stable_key(&format!("tile:{id}")))
        .id(&format!("tile_{id}"))
        .width_request(width)
        .on_pointer_up_with_button(move |_, _, pressed| {
            if pressed == BTN_RIGHT {
                LauncherMsg::TogglePin(pin_id.clone())
            } else if pressed == BTN_LEFT {
                LauncherMsg::ActivateApp(launch_id.clone())
            } else {
                LauncherMsg::Ignore
            }
        })
}

/// The bottom row: lock / log out / suspend / restart / shut down. UI shell
/// only — every handler is a Task-6 no-op (see `update`).
fn power_row() -> View<LauncherMsg> {
    box_(
        Orientation::Horizontal,
        [
            button("Lock")
                .id("power_lock")
                .on_click(LauncherMsg::Power(PowerAction::Lock)),
            button("Log out")
                .id("power_logout")
                .on_click(LauncherMsg::Power(PowerAction::Logout)),
            button("Suspend")
                .id("power_suspend")
                .on_click(LauncherMsg::Power(PowerAction::Suspend)),
            button("Restart")
                .id("power_reboot")
                .on_click(LauncherMsg::Power(PowerAction::Reboot)),
            button("Shut down")
                .id("power_off")
                .on_click(LauncherMsg::Power(PowerAction::PowerOff)),
        ],
    )
    .id("launcher_power")
}

/// The root key map: typing filters, arrows move within and across panes,
/// Enter launches, Escape closes.
///
/// `on_key` sits on the root so keys bubble here from anywhere the
/// `SearchEntry` does not consume them itself: it eats text and caret keys,
/// while arrows-up/down, Enter (as `Activate`) and Escape reach this map.
///
/// Precondition: some node must hold the keyboard focus — with an empty
/// focus ring `App` drops keys before any view sees them. `Exclusive`
/// gives the *surface* the seat's focus on open, but no *open-time* focus
/// path is wired yet: focusing needs a post-layout `Node` handle after
/// `KeyboardEnter` on the new surface, which is non-trivial and owned by
/// Task 6. The focus mechanism itself exists — `Cmd::Focus(node)` moves
/// the ring (`ui/src/view/app.rs`, with the `KeyboardEnter`-when-empty
/// precedent in `ui/src/bin/window-probe.rs`) — it is the open-time
/// wiring, not the mechanism, that is missing. Until that lands (pinned
/// by the ignored
/// `typing_at_open_reaches_the_search_box_without_a_prior_tab` gate) one
/// Tab — first focusable in reading order — or one click focuses search,
/// and everything below holds from there.
fn root_key(ev: &KeyEvent) -> Option<LauncherMsg> {
    match ev.keysym {
        xkb::Keysym::Escape => Some(LauncherMsg::Close),
        xkb::Keysym::Return | xkb::Keysym::KP_Enter | xkb::Keysym::ISO_Enter => {
            Some(LauncherMsg::ActivateSelected)
        }
        xkb::Keysym::Up | xkb::Keysym::KP_Up => Some(LauncherMsg::MoveUp),
        xkb::Keysym::Down | xkb::Keysym::KP_Down => Some(LauncherMsg::MoveDown),
        xkb::Keysym::Left | xkb::Keysym::KP_Left => Some(LauncherMsg::MoveLeft),
        xkb::Keysym::Right | xkb::Keysym::KP_Right => Some(LauncherMsg::MoveRight),
        _ => None,
    }
}

impl LauncherModel {
    /// True when `id` is the keyboard-selected row of `filtered`.
    fn selected_for(&self, id: &str, filtered: &[&DesktopEntry]) -> bool {
        self.selected
            .and_then(|i| filtered.get(i))
            .is_some_and(|e| e.id == id)
    }
}

/// A stable reconciler key for one app id: the rows reorder on every
/// keystroke, so positional identity would smear hover and focus across
/// rows as the list filters.
fn stable_key(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// The launcher supervisor (B1 Task 5): the second surface's whole
/// lifecycle on its own thread, beside the panel's loop.
///
/// The panel's `Msg::StartClicked` arm sends the new `launcher_open` bit
/// down `rx`; this thread opens one surface per `true` (a child thread
/// running [`run_surface`]) and dismisses it per `false`. The child's own
/// close paths (Escape, successful launch) report back through the panel
/// inbox as `panel::Msg::LauncherClosed`, which clears the bit — so a
/// `false` that arrives with no live menu is simply late news, never an
/// error. Dropping the sender (panel teardown) ends this thread.
///
/// Every failure here degrades to "the menu never opens", never a panic
/// and never the panel: this runs on a foreign thread, so no
/// `unwrap`/`expect` anywhere below — errors log and notify the panel shut.
pub fn supervise(
    rx: std::sync::mpsc::Receiver<bool>,
    panel_tx: icedtea_ui::view::InboxSender<crate::panel::Msg>,
    bar_position: String,
) {
    use std::sync::mpsc::{Receiver, TryRecvError};
    use std::thread::JoinHandle;

    /// A live menu: how to ask it to dismiss itself, the thread running
    /// it, and its completion signal.
    struct LiveMenu {
        stop: icedtea_ui::view::InboxSender<LauncherMsg>,
        handle: JoinHandle<()>,
        done: Receiver<()>,
    }

    // Reap a menu that dismissed itself (or died): a finished handle joins
    // immediately; a received completion joins after thread teardown only
    // (microseconds — the child sent it past `App::run`). A disconnected
    // channel means the child died without completing — free the slot all
    // the same, or Start would brick forever on one dead menu.
    let mut running: Option<LiveMenu> = None;
    let reap = |running: &mut Option<LiveMenu>| {
        let finished = running.as_ref().is_some_and(|menu| {
            menu.handle.is_finished() || !matches!(menu.done.try_recv(), Err(TryRecvError::Empty))
        });
        if finished && let Some(menu) = running.take() {
            let _ = menu.handle.join();
        }
    };

    while let Ok(want_open) = rx.recv() {
        reap(&mut running);
        if want_open {
            if running.is_some() {
                continue;
            }
            let (inbox, stop) = match icedtea_ui::view::Inbox::<LauncherMsg>::new() {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(%err, "the launcher inbox failed; Start does nothing");
                    let _ = panel_tx.send(crate::panel::Msg::LauncherClosed);
                    continue;
                }
            };
            let position = bar_position.clone();
            let closed = panel_tx.clone();
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            match std::thread::Builder::new()
                .name("launcher-surface".into())
                .spawn(move || run_surface(&position, closed, inbox, done_tx))
            {
                Ok(handle) => {
                    running = Some(LiveMenu {
                        stop,
                        handle,
                        done: done_rx,
                    });
                }
                Err(err) => {
                    tracing::warn!(%err, "the launcher thread failed to spawn");
                    let _ = panel_tx.send(crate::panel::Msg::LauncherClosed);
                }
            }
        } else if let Some(menu) = &running {
            // Ask the live menu to dismiss itself through its own close
            // path: it notifies the panel and quits its loop. A failed send
            // means it already left; the next reap collects it.
            let _ = menu.stop.send(LauncherMsg::Close);
        }
    }
    // The panel dropped its end (process teardown): ask a live menu to
    // dismiss itself and end. No join — the process is going away, and
    // joining a loop that may never fold again would hang teardown.
    if let Some(menu) = running.take() {
        let _ = menu.stop.send(LauncherMsg::Close);
    }
}

/// One launcher surface, start to dismissal (blocking, on its own thread).
///
/// Builds the model with its own D-Bus proxy (a blocking connection per
/// thread is the established `CompositorProxy` pattern), seeds the stores
/// from the persisted config, opens the [`spec()`] surface and runs the
/// `App` until a close path quits it. Every exit notifies the panel shut —
/// the notification is idempotent, so the error exits and the post-`run`
/// one can safely overlap.
fn run_surface(
    bar_position: &str,
    panel_tx: icedtea_ui::view::InboxSender<crate::panel::Msg>,
    inbox: icedtea_ui::view::Inbox<LauncherMsg>,
    done: std::sync::mpsc::Sender<()>,
) {
    use crate::compositor_client::CompositorProxy;
    use crate::panel::Offline;

    let notify = {
        let panel_tx = panel_tx.clone();
        Rc::new(move || {
            let _ = panel_tx.send(crate::panel::Msg::LauncherClosed);
        })
    };
    let wm: Rc<dyn CompositorCommands> = match CompositorProxy::new() {
        Ok(proxy) => Rc::new(proxy),
        Err(err) => {
            tracing::error!(%err, "no session bus; launcher launch commands disabled");
            Rc::new(Offline)
        }
    };
    let mut model = LauncherModel::new(wm);
    let db_path = icedtea_config::default_db_path();
    model.seed(&icedtea_config::load_or_default(&db_path).launcher);
    model.on_close = Some(notify.clone());
    // Write-back for the next open: pin/unpin + recency mutations reach
    // the launcher table on every close. Failures warn and drop — a dead
    // or locked DB must degrade to "the menu never persists", never a
    // panic on this foreign thread, and never the panel.
    model.on_persist = Some(Rc::new(move |launcher| {
        if let Err(err) = persist_launcher_config(&db_path, &launcher) {
            tracing::warn!(%err, "launcher write-back failed; reopen may show stale pins/recency");
        }
    }));
    model.open();
    let window = match icedtea_ui::window::Window::open(
        spec(bar_position),
        crate::style::sheet(),
        icedtea_ui::text::FontDatabase::new(),
    ) {
        Ok(window) => window,
        Err(err) => {
            tracing::error!(%err, "the launcher could not open its layer surface");
            let _ = done.send(());
            notify();
            return;
        }
    };
    // The open menu's own probe lines, beside `$ICEDTEA_PROBE_REPORT` (the
    // panel's own pattern in `main.rs`). Only a harness sets the env var,
    // so this is inert in a real session.
    let probe_report = std::env::var_os("ICEDTEA_PROBE_REPORT").map(|value| {
        let mut path = PathBuf::from(value);
        path.set_extension("launchers");
        path
    });
    let mut app = icedtea_ui::view::App::new(model, update, view).with_inbox(inbox);
    if let Some(path) = probe_report {
        app = app.with_probe_report(path);
    }
    if let Err(err) = app.run(window) {
        tracing::error!(%err, "the launcher event loop failed");
    }
    // Completion first: the supervisor may open the next menu off this,
    // and it must never see a stale slot. Then the panel notification.
    let _ = done.send(());
    notify();
}

#[cfg(test)]
mod tests {
    use icedtea_ui::view::PropName;

    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// A recording [`CompositorCommands`]: `RefCell`, not `Mutex` — the unit
    /// tests and `update` are single-threaded. The cross-thread mock the
    /// harness needs lives in `shell/tests/support/mod.rs`.
    struct MockWm {
        spawns: RefCell<Vec<String>>,
        succeed: Cell<bool>,
        quits: Cell<u32>,
    }

    impl MockWm {
        fn ok() -> Self {
            Self {
                spawns: RefCell::new(Vec::new()),
                succeed: Cell::new(true),
                quits: Cell::new(0),
            }
        }

        fn failing() -> Self {
            Self {
                spawns: RefCell::new(Vec::new()),
                succeed: Cell::new(false),
                quits: Cell::new(0),
            }
        }
    }

    impl CompositorCommands for MockWm {
        fn focus_window(&self, _id: u32) {}
        fn close_window(&self, _id: u32) {}
        fn set_workspace(&self, _id: u32) {}
        fn spawn_app(&self, app_id: &str) -> bool {
            self.spawns.borrow_mut().push(app_id.to_string());
            self.succeed.get()
        }
        fn quit(&self) {
            self.quits.set(self.quits.get() + 1);
        }
    }

    /// A power runner that always succeeds, standing in for the real
    /// `loginctl`/`systemctl` shell-out (which no test may execute).
    fn ok_power(_program: &str, _args: &[&str]) -> std::io::Result<()> {
        Ok(())
    }

    /// A power runner that always fails, standing in for a denied or
    /// missing `loginctl`/`systemctl`.
    fn denied_power(_program: &str, _args: &[&str]) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ))
    }

    fn entry(id: &str, name: &str) -> DesktopEntry {
        DesktopEntry {
            id: id.into(),
            name: name.into(),
            exec: id.into(),
            icon: None,
            categories: Vec::new(),
            keywords: Vec::new(),
            nodisplay: false,
            only_show_in: Vec::new(),
            not_show_in: Vec::new(),
        }
    }

    /// Four apps, one pinned (`firefox`), one tiled (`firefox` in `Web`,
    /// size 2): the pinned/tiled/plain section split the Left/Right tests
    /// pin.
    fn seeded() -> (LauncherModel, Rc<MockWm>) {
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm.clone());
        m.index = DesktopIndex::from_entries(vec![
            entry("firefox", "Firefox"),
            entry("firetools", "Fire Tools"),
            entry("music", "Music"),
            entry("files", "Files"),
        ]);
        m.pins.pin("firefox");
        m.tiles.ingest("Web", &["firefox".to_string()], 2);
        (m, wm)
    }

    /// Run a `Cmd`'s side effects the way the loop does: `Cmd::Task` bodies,
    /// in order, after the fold. Anything else is inert here.
    fn run_tasks(cmd: Cmd<LauncherMsg>) {
        match cmd {
            Cmd::Task(f) => f(),
            Cmd::Batch(list) => {
                for c in list {
                    run_tasks(c);
                }
            }
            _ => {}
        }
    }

    /// True when the command leaves the loop (`Quit`, alone or batched with
    /// the close notification).
    fn quits(cmd: &Cmd<LauncherMsg>) -> bool {
        match cmd {
            Cmd::Quit => true,
            Cmd::Batch(list) => list.iter().any(|c| matches!(c, Cmd::Quit)),
            _ => false,
        }
    }

    /// Find the first descendant of `v` whose `id` prop is `id`.
    fn by_id<'a>(v: &'a View<LauncherMsg>, id: &str) -> Option<&'a View<LauncherMsg>> {
        if v.props.str(PropName::Id) == Some(id) {
            return Some(v);
        }
        v.children.iter().find_map(|c| by_id(c, id))
    }

    /// The `classes` of the node `id` names, as owned strings.
    fn classes(v: &View<LauncherMsg>, id: &str) -> Vec<String> {
        match by_id(v, id).expect("node").props.get(PropName::Classes) {
            Some(icedtea_ui::view::Prop::Classes(list)) => {
                list.iter().map(|c| c.to_string()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The ids of the app rows under the `launcher_all` container.
    fn all_rows(v: &View<LauncherMsg>) -> Vec<String> {
        by_id(v, "launcher_all")
            .expect("all-apps list")
            .children
            .iter()
            .filter_map(|c| c.props.str(PropName::Id))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn launcher_spec_anchors_bottom_left_by_default() {
        for position in ["bottom", "anything-else"] {
            let spec = spec(position);
            let Role::Layer(layer) = spec.role else {
                panic!("the launcher is a layer surface");
            };
            assert!(
                layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Left),
                "{position}: anchored left"
            );
            assert!(
                layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Bottom),
                "{position}: anchored bottom"
            );
            assert!(
                !layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Top),
                "{position}: not top"
            );
            assert!(
                !layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Right),
                "{position}: a menu spans no output edge"
            );
        }
    }

    #[test]
    fn launcher_spec_anchors_top_left_for_top() {
        let spec = spec("top");
        let Role::Layer(layer) = spec.role else {
            panic!("the launcher is a layer surface");
        };
        assert!(layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Left));
        assert!(layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Top));
        assert!(!layer.anchor.contains(zwlr_layer_surface_v1::Anchor::Bottom));
    }

    #[test]
    fn launcher_spec_takes_exclusive_keyboard_focus_without_an_exclusive_zone() {
        // `Exclusive` is verified against `wayland-protocols-wlr`'s own
        // `wlr-layer-shell-unstable-v1.xml` (`keyboard_interactivity/exclusive`
        // value 1): the launcher needs keyboard focus, where the panel
        // (`panel::spec`) takes `None`. Zero exclusive zone: the menu
        // overlays (`Layer::Top`), never pushing windows around (spec
        // §Architecture).
        let spec = spec("bottom");
        let Role::Layer(layer) = spec.role else {
            panic!("the launcher is a layer surface");
        };
        assert_eq!(
            layer.layer,
            zwlr_layer_shell_v1::Layer::Top,
            "a menu above the shell surface layer"
        );
        assert_eq!(
            layer.keyboard,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive
        );
        assert_eq!(layer.exclusive_zone, 0);
        assert_eq!(spec.size, LAUNCHER_SIZE);
    }

    #[test]
    fn a_fresh_launcher_renders_search_pinned_all_tiles_and_power() {
        let (m, _) = seeded();
        let v = view(&m);
        assert_eq!(v.props.str(PropName::Id), Some("launcher"));
        for id in [
            "launcher_search",
            "launcher_pinned",
            "launcher_all",
            "launcher_tiles",
            "launcher_power",
        ] {
            assert!(by_id(&v, id).is_some(), "#{id} renders at rest");
        }
        assert!(
            by_id(&v, "launcher_status").is_none(),
            "no status line until something reports one"
        );
    }

    #[test]
    fn typing_filters_all_lists_and_marks_the_top_hit_suggested() {
        let (mut m, _) = seeded();
        let _ = update(&mut m, LauncherMsg::SearchChanged("fire".into()));
        let v = view(&m);
        let rows = all_rows(&v);
        // Both names prefix-match "fire" (200 each); the tiebreak is
        // case-insensitive name order ("fire tools" < "firefox"), per the
        // Task-1 matcher contract — the test pins the order, not a guess.
        assert_eq!(rows, vec!["app_firetools", "app_firefox"]);
        assert!(
            classes(&v, "app_firetools").contains(&"suggested-action".to_string()),
            "the top hit carries .suggested-action"
        );
        assert!(
            !classes(&v, "app_firefox").contains(&"suggested-action".to_string()),
            "only the top hit does"
        );
    }

    #[test]
    fn arrow_down_then_up_moves_the_active_selection() {
        let (mut m, _) = seeded();
        assert_eq!(m.selected, None);
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(0));
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(1));
        let v = view(&m);
        assert!(classes(&v, "app_firetools").contains(&"active".to_string()));
        let _ = update(&mut m, LauncherMsg::MoveUp);
        assert_eq!(m.selected, Some(0));
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let (mut m, _) = seeded();
        let _ = update(&mut m, LauncherMsg::MoveUp);
        assert_eq!(m.selected, Some(3), "Up from nothing takes the last row");
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(3), "Down past the end clamps");
        let _ = update(&mut m, LauncherMsg::SearchChanged("zzzz".into()));
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, None, "no rows, no selection");
    }

    #[test]
    fn left_and_right_jump_across_panes() {
        // Filtered order is index order: firefox(pinned) 0, firetools 1,
        // music 2, files 3. firefox is also tiled, but pinned wins the
        // section split, so sections are Pinned{0} All{1,2,3}.
        let (mut m, _) = seeded();
        let _ = update(&mut m, LauncherMsg::MoveDown);
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(1));
        let _ = update(&mut m, LauncherMsg::MoveLeft);
        assert_eq!(m.selected, Some(0), "Left jumps to the pinned section");
        let _ = update(&mut m, LauncherMsg::MoveRight);
        assert_eq!(m.selected, Some(1), "Right jumps to the next section");
        let _ = update(&mut m, LauncherMsg::MoveRight);
        assert_eq!(m.selected, Some(3), "Right past the last section clamps");
    }

    #[test]
    fn enter_with_no_selection_launches_the_top_hit_records_recency_and_quits() {
        let (mut m, wm) = seeded();
        m.open = true;
        let closed = Rc::new(Cell::new(false));
        let flag = closed.clone();
        m.on_close = Some(Rc::new(move || flag.set(true)));
        let cmd = update(&mut m, LauncherMsg::ActivateSelected);
        assert_eq!(wm.spawns.borrow().as_slice(), ["firefox".to_string()]);
        assert_eq!(m.recency.count("firefox"), 1);
        assert!(!m.open, "a successful launch dismisses the launcher");
        assert!(quits(&cmd), "dismissal leaves the loop");
        run_tasks(cmd);
        assert!(closed.get(), "the panel is notified through on_close");
    }

    #[test]
    fn a_failed_launch_reports_a_status_line_and_stays_open() {
        let (mut m, wm) = seeded();
        m.wm = Rc::new(MockWm::failing());
        m.open = true;
        let cmd = update(&mut m, LauncherMsg::ActivateApp("music".into()));
        assert_eq!(wm.spawns.borrow().as_slice(), [] as [String; 0]);
        assert_eq!(m.recency.count("music"), 0, "no recency without success");
        assert!(m.open, "a failed launch keeps the menu open");
        assert!(!quits(&cmd));
        let status = m.status.clone().expect("a status line");
        assert!(status.contains("Music"), "names the app: {status}");
        assert!(by_id(&view(&m), "launcher_status").is_some());
    }

    #[test]
    fn activating_an_unknown_id_reports_a_status_line_and_spawns_nothing() {
        let (mut m, wm) = seeded();
        m.open = true;
        let cmd = update(&mut m, LauncherMsg::ActivateApp("no-such-app".into()));
        assert!(wm.spawns.borrow().is_empty());
        assert!(m.open);
        assert!(!quits(&cmd));
        assert!(m.status.is_some());
    }

    #[test]
    fn escape_closes_the_launcher() {
        let (mut m, _) = seeded();
        m.open = true;
        let cmd = update(&mut m, LauncherMsg::Close);
        assert!(!m.open);
        assert!(quits(&cmd));
    }

    #[test]
    fn right_click_toggles_the_pin_left_click_launches() {
        use icedtea_ui::view::EventKind;
        use icedtea_ui::window::pointer::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

        let (m, wm) = seeded();
        let v = view(&m);
        // Right-click on an unpinned row pins it.
        let msg = by_id(&v, "app_music")
            .expect("music row")
            .handlers
            .fire_pair_button(EventKind::PointerUp, 4.0, 4.0, BTN_RIGHT)
            .expect("a pointer-up handler");
        assert!(matches!(msg, LauncherMsg::TogglePin(_)));
        let mut m = m;
        let _ = update(&mut m, msg);
        assert!(m.pins.is_pinned("music"));
        // Right-click again unpins.
        let _ = update(&mut m, LauncherMsg::TogglePin("music".into()));
        assert!(!m.pins.is_pinned("music"));
        // Left release launches.
        let v = view(&m);
        let msg = by_id(&v, "app_music")
            .expect("music row")
            .handlers
            .fire_pair_button(EventKind::PointerUp, 4.0, 4.0, BTN_LEFT)
            .expect("a pointer-up handler");
        assert!(matches!(msg, LauncherMsg::ActivateApp(_)));
        m.open = true;
        let _ = update(&mut m, msg);
        assert_eq!(wm.spawns.borrow().as_slice(), ["music".to_string()]);
        // The launch above dismissed the menu; reopen for the inert-button
        // probe below.
        m.open = true;
        // Any other button is inert: no launch, no pin flip, menu stays open.
        let v = view(&m);
        let msg = by_id(&v, "app_files")
            .expect("files row")
            .handlers
            .fire_pair_button(EventKind::PointerUp, 4.0, 4.0, BTN_MIDDLE)
            .expect("a pointer-up handler");
        assert!(matches!(msg, LauncherMsg::Ignore));
        let cmd = update(&mut m, msg);
        assert!(matches!(cmd, Cmd::None));
        assert_eq!(wm.spawns.borrow().len(), 1);
        assert!(!m.pins.is_pinned("files"));
        assert!(m.open);
    }

    #[test]
    fn letter_jump_selects_on_an_empty_query_and_is_ignored_while_filtering() {
        let (mut m, _) = seeded();
        let _ = update(&mut m, LauncherMsg::LetterJump('m'));
        assert_eq!(m.selected, Some(2), "first app starting with m");
        let _ = update(&mut m, LauncherMsg::SearchChanged("fire".into()));
        let _ = update(&mut m, LauncherMsg::LetterJump('m'));
        assert_eq!(m.selected, None, "typing filters instead; jump stays out");
        let _ = update(&mut m, LauncherMsg::SearchChanged(String::new()));
        let _ = update(&mut m, LauncherMsg::LetterJump('z'));
        assert_eq!(m.selected, None, "no match, no selection");
    }

    #[test]
    fn power_row_renders_all_five_buttons() {
        let (m, _) = seeded();
        let v = view(&m);
        for id in [
            "power_lock",
            "power_logout",
            "power_suspend",
            "power_reboot",
            "power_off",
        ] {
            assert!(by_id(&v, id).is_some(), "#{id} renders at rest");
        }
    }

    #[test]
    fn power_shell_out_success_closes_the_menu() {
        let (mut m, wm) = seeded();
        m.open = true;
        m.power_run = ok_power;
        let cmd = update(&mut m, LauncherMsg::Power(PowerAction::Suspend));
        assert!(!m.open, "a successful power action dismisses the menu");
        assert!(quits(&cmd), "dismissal leaves the loop");
        assert!(m.status.is_none(), "no status line on success");
        assert!(
            wm.spawns.borrow().is_empty(),
            "no launch from the power row"
        );
    }

    #[test]
    fn power_shell_out_failure_reports_a_status_line_and_stays_open() {
        let (mut m, _) = seeded();
        m.open = true;
        m.power_run = denied_power;
        let cmd = update(&mut m, LauncherMsg::Power(PowerAction::Suspend));
        assert!(m.open, "a failed power action keeps the menu open");
        assert!(!quits(&cmd));
        let status = m.status.clone().expect("a status line");
        assert!(
            status.contains("Suspend") && status.contains("permission denied"),
            "names the action and the cause: {status}"
        );
        assert!(by_id(&view(&m), "launcher_status").is_some());
    }

    #[test]
    fn close_persists_pins_tiles_and_recency_for_reopen() {
        let (mut m, _) = seeded();
        let saved: Rc<RefCell<Vec<icedtea_config::LauncherConfig>>> =
            Rc::new(RefCell::new(Vec::new()));
        let probe = saved.clone();
        m.on_persist = Some(Rc::new(move |cfg| probe.borrow_mut().push(cfg)));
        // One pin mutation, then one recorded launch whose success closes
        // the menu — the close is what writes back.
        let _ = update(&mut m, LauncherMsg::TogglePin("music".into()));
        m.open = true;
        let _ = update(&mut m, LauncherMsg::ActivateApp("firefox".into()));
        let snapshots = saved.borrow();
        assert_eq!(snapshots.len(), 1, "one close, one write-back");
        let cfg = &snapshots[0];
        assert!(
            cfg.pinned.contains(&"music".to_string()),
            "pins persist: {:?}",
            cfg.pinned
        );
        assert!(
            cfg.tile_groups.iter().any(|g| g.name == "Web"),
            "tiles round-trip: {:?}",
            cfg.tile_groups
        );
        assert_eq!(
            cfg.recency.get("firefox").map(|(count, _)| *count),
            Some(1),
            "recency persists: {:?}",
            cfg.recency
        );
    }

    #[test]
    fn power_logout_quits_the_session_and_closes_the_menu() {
        let (mut m, wm) = seeded();
        m.open = true;
        let cmd = update(&mut m, LauncherMsg::Power(PowerAction::Logout));
        assert_eq!(wm.quits.get(), 1, "logout takes the compositor quit path");
        assert!(
            wm.spawns.borrow().is_empty(),
            "no launch from the power row"
        );
        assert!(!m.open, "logout dismisses the menu");
        assert!(quits(&cmd), "dismissal leaves the loop");
    }

    #[test]
    fn seed_loads_pins_tiles_and_recency_from_config() {
        use std::collections::HashMap;
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm);
        let mut recency = HashMap::new();
        recency.insert("music".to_string(), (3u64, 7u64));
        m.seed(&icedtea_config::LauncherConfig {
            pinned: vec!["music".to_string()],
            tile_groups: vec![icedtea_config::TileGroup {
                name: "Media".to_string(),
                ids: vec!["music".to_string()],
                size: 2,
            }],
            recency,
        });
        assert!(m.pins.is_pinned("music"));
        let groups = m.tiles.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Media");
        assert_eq!(groups[0].size, 2);
        assert_eq!(m.recency.count("music"), 3);
    }

    #[test]
    fn tile_buttons_scale_with_their_group_size() {
        let (m, _) = seeded();
        let v = view(&m);
        assert!(by_id(&v, "tile_group_Web").is_some());
        // firefox sits in a size-2 group: its tile is two units wide.
        let width = by_id(&v, "tile_firefox")
            .expect("tile")
            .props
            .int(PropName::WidthRequest, 0);
        assert_eq!(
            width,
            i64::from(2 * TILE_PX),
            "tile width follows TileGroup.size"
        );
    }

    #[test]
    fn opening_rescans_only_when_the_app_dirs_changed() {
        // The spec's mtime-gated rescan: steady-state opens do no I/O.
        let dir = std::env::temp_dir().join(format!("icedtea-launcher-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("a.desktop"),
            "[Desktop Entry]\nName=App A\nExec=a\n",
        )
        .expect("fixture");
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm);
        m.dirs = vec![dir.clone()];
        m.open();
        assert!(m.open);
        assert_eq!(m.index.apps().len(), 1);
        let first = fingerprint(&m.dirs);
        // Steady state: the fingerprint is stable, so the next open reuses
        // the index it already holds.
        assert_eq!(fingerprint(&m.dirs), first);
        m.open();
        assert_eq!(m.index.apps().len(), 1);
        // A new file moves the fingerprint and the next open picks it up.
        std::fs::write(
            dir.join("b.desktop"),
            "[Desktop Entry]\nName=App B\nExec=b\n",
        )
        .expect("fixture");
        assert_ne!(fingerprint(&m.dirs), first);
        m.open();
        assert_eq!(m.index.apps().len(), 2);
        std::fs::remove_dir_all(&dir).expect("temp dir");
    }

    #[test]
    fn opening_with_a_missing_dir_yields_an_empty_index_without_panicking() {
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm);
        m.dirs = vec![std::path::PathBuf::from("/nonexistent-icedtea-dir")];
        m.open();
        assert!(m.open);
        assert!(m.index.apps().is_empty());
    }

    #[test]
    fn launcher_msg_is_send_for_the_cross_thread_inbox() {
        fn assert_send<T: Send>() {}
        assert_send::<LauncherMsg>();
    }

    /// The supervisor thread ends when the panel drops its control channel
    /// — the same shape as `main.rs`'s forward-thread test. No surface is
    /// ever opened here (no `Open` is sent), so this stays hermetic with or
    /// without a compositor.
    #[test]
    fn the_supervisor_ends_when_the_panel_drops_its_channel() {
        use std::time::{Duration, Instant};

        let (_inbox, panel_tx) =
            icedtea_ui::view::Inbox::<crate::panel::Msg>::new().expect("panel inbox");
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let handle = std::thread::spawn(move || supervise(rx, panel_tx, "bottom".to_string()));
        drop(tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "the supervisor must end once told to");
        handle.join().expect("supervisor thread");
    }

    /// A `Close` with no live menu is a no-op: the supervisor stays up for
    /// the next `Open` instead of exiting or panicking.
    #[test]
    fn close_with_no_live_menu_is_a_noop() {
        use std::time::{Duration, Instant};

        let (_inbox, panel_tx) =
            icedtea_ui::view::Inbox::<crate::panel::Msg>::new().expect("panel inbox");
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let handle = std::thread::spawn(move || supervise(rx, panel_tx, "bottom".to_string()));
        tx.send(false).expect("control channel");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !handle.is_finished(),
            "a stray Close must not kill the supervisor"
        );
        drop(tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().expect("supervisor thread");
    }
}
