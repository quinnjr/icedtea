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
//! The power row invokes the `icedtea-session` CLI per `launcher::power_argv`
//! (logout takes the compositor quit path); every failure surfaces as
//! `status`, never silently.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
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
    desktop_paths, power_argv, power_error_message, run_power_command as core_run_power_command,
};

/// The launcher's initial surface size: a menu, not a bar — fixed, modest,
/// content-laid-out inside it. Fixed so the layer surface never resizes
/// under the compositor mid-session.
pub const LAUNCHER_SIZE: (u32, u32) = (400, 500);

/// One tile grid unit in px: a size-`n` tile requests `n` units of width.
/// 96px keeps a 1x1 tile finger-sized without overflowing the 400px menu.
pub const TILE_PX: i32 = 96;

/// Ceiling for a tile group's span in [`TILE_PX`] units. `size` is
/// config-controlled (hand-editable redb), so the width product must clamp
/// instead of wrapping (debug) or running off the surface (release):
/// 1024 units is already an absurd menu, and `1024 * TILE_PX` sits far
/// below `i32::MAX`, so the clamped product cannot overflow.
pub const MAX_TILE_SPAN: u32 = 1024;

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
    /// The power row: `icedtea-session` CLI calls per `launcher::power_argv`,
    /// logout via the compositor quit path.
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
    /// Logout needs a confirm press: the first `Power(Logout)` arms (with
    /// an arming status), the second quits. Any other message or a fresh
    /// [`LauncherModel::open`] disarms, so a stale arming never surprises.
    logout_armed: bool,
    /// XDG dirs rescanned on [`LauncherModel::open`]. `pub` for the
    /// supervisor (production) and hermetic tmpdirs (tests).
    pub dirs: Vec<PathBuf>,
    scanned: (usize, Option<SystemTime>),
    wm: Rc<dyn CompositorCommands>,
    /// Fires after a close fold (successful launch, Escape): the supervisor
    /// resets the panel's `launcher_open` bit through it.
    pub on_close: Option<Rc<dyn Fn()>>,
    /// How a power action is invoked. Defaults to [`run_power_command`];
    /// tests override it so no test ever execs `icedtea-session`.
    power_run: fn(&str, &[&str]) -> std::io::Result<()>,
    /// Write-back for the pin/tile/recency stores. Called with
    /// [`LauncherModel::snapshot`] on every close (successful launch,
    /// power action, Escape); `run_surface` points it at the config DB,
    /// so a reopened menu still shows the pins and recency. `None` (unit
    /// tests) means no write-back.
    pub on_persist: Option<Rc<dyn Fn(icedtea_config::LauncherConfig)>>,
}

/// Consecutive launcher write-back failures, sticky across closes: the
/// final fallback only warns and drops, so the streak rides along in the
/// message instead of vanishing with the dismissed menu.
static WRITE_BACK_FAILURES: AtomicU32 = AtomicU32::new(0);

/// True when a write-back failure message looks like lock/busy contention
/// (another writer holds the config DB) rather than corruption or I/O:
/// only contention is worth a retry with backoff.
fn is_lock_busy(message: &str) -> bool {
    message.contains("Cannot acquire lock") || message.contains("still in progress")
}

/// Persist one launcher snapshot through the scoped
/// [`icedtea_config::Config::save_launcher`] (launcher table only — never
/// a full [`icedtea_config::Config::save`], which would clobber a
/// concurrent settings-app edit with this process's stale in-memory
/// copy). Reads the current file first so the other sections survive;
/// every failure comes back as `Err`, never a panic.
///
/// A lock/busy failure (the settings app or compositor holds the DB)
/// retries with a short backoff before giving up; the streak of
/// consecutive failures rides in the returned message, and the caller
/// keeps warn-and-drop as the final fallback.
pub fn persist_launcher_config(
    db_path: &std::path::Path,
    launcher: &icedtea_config::LauncherConfig,
) -> Result<(), String> {
    const ATTEMPTS: u32 = 3;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match persist_launcher_config_once(db_path, launcher) {
            Ok(()) => {
                WRITE_BACK_FAILURES.store(0, Ordering::Relaxed);
                return Ok(());
            }
            Err(err) if attempt < ATTEMPTS && is_lock_busy(&err) => {
                std::thread::sleep(std::time::Duration::from_millis(25 * u64::from(attempt)));
            }
            Err(err) => {
                let streak = WRITE_BACK_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
                return Err(format!("{err} (write-back failure #{streak})"));
            }
        }
    }
}

/// One write-back attempt: what [`persist_launcher_config`] retries.
fn persist_launcher_config_once(
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

/// The one real power invocation: `program` + `args` is exactly
/// [`power_argv`]'s output (the `icedtea-session` CLI). Both an io failure
/// (binary missing) and a non-zero exit (no session daemon, or logind
/// refusing) are failures, so a refusal surfaces as a launcher status line,
/// never silently.
///
/// A thin adapter over the core's bounded runner (internal ~30s bound, so
/// a hung helper can never wedge the fold); kept under this name so the
/// `power_run` call sites and tests never exec through another path.
// The power row routes through the `icedtea-session` CLI to
// `org.icedtea.Session`; this adapter maps its exit status to a launcher
// result.
fn run_power_command(program: &str, args: &[&str]) -> std::io::Result<()> {
    let status = core_run_power_command(program, args)?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with {status}"
        )))
    }
}

/// Which visual pane a row belongs to: the pinned rail, the All-apps list,
/// or the tiles pane. One index space orders them Pinned → All → Tiled,
/// which is what the Left/Right pane jumps navigate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Pinned,
    All,
    Tiled,
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
            logout_armed: false,
            dirs: default_dirs(),
            scanned: (0, None),
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
            // Clamp at the boundary: `size` is hand-editable redb, so a 0
            // (or absurd) span must never persist into the store — the tile
            // width math assumes at least one unit.
            self.tiles
                .ingest(&group.name, &group.ids, group.size.clamp(1, MAX_TILE_SPAN));
        }
        self.recency.restore(&config.recency);
    }

    /// Open (or re-open) the menu: mtime-gated rescan, then a blank query,
    /// no selection, no status. Steady-state opens do no I/O: the rescan
    /// runs only when [`fingerprint`] moved (spec §Index).
    pub fn open(&mut self) {
        let current = fingerprint(&self.dirs);
        if self.scanned != current {
            let entries = DesktopIndex::scan(&self.dirs);
            if entries.is_empty() {
                // Nothing parseable came back: say how many `.desktop`
                // files the scan read, so an empty menu points at its
                // cause (no files vs. all files skipped) instead of
                // silence. Rare path only — no extra walk otherwise.
                let scanned = desktop_paths(&self.dirs).len();
                tracing::debug!(
                    scanned,
                    "launcher scan yielded 0 apps; all files skipped or no .desktop files present"
                );
            }
            self.index = DesktopIndex::from_entries(entries);
            self.scanned = current;
        }
        self.query.clear();
        self.selected = None;
        self.status = None;
        self.logout_armed = false;
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

    /// Which pane an entry renders in. Pinned wins over tiled — a pinned
    /// tile still reads as pinned.
    fn section_of(&self, entry: &DesktopEntry) -> Section {
        if self.pins.is_pinned(&entry.id) {
            Section::Pinned
        } else if self
            .tiles
            .groups()
            .iter()
            .any(|group| group.ids.iter().any(|id| id == &entry.id))
        {
            Section::Tiled
        } else {
            Section::All
        }
    }

    /// The keyboard's one index space, in visual order: the pinned rail's
    /// rows, then every All-apps row, then the tiles pane's rows (group and
    /// member order — the exact order [`tiles_pane`] renders). Entries can
    /// render in several panes at once (a pinned tile appears three times),
    /// so one filtered index may own several visual rows; each row carries
    /// its pane and its filtered index.
    fn visual_rows(&self, filtered: &[&DesktopEntry]) -> Vec<(Section, usize)> {
        let mut rows = Vec::new();
        rows.extend(
            filtered
                .iter()
                .enumerate()
                .filter(|(_, entry)| self.section_of(entry) == Section::Pinned)
                .map(|(i, _)| (Section::Pinned, i)),
        );
        rows.extend((0..filtered.len()).map(|i| (Section::All, i)));
        for group in self.tiles.groups() {
            for id in &group.ids {
                if let Some(i) = filtered.iter().position(|entry| entry.id == *id) {
                    rows.push((Section::Tiled, i));
                }
            }
        }
        rows
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
    // A logout press arms; anything else disarms, so a stale arming can
    // never surprise a later menu.
    if !matches!(&msg, LauncherMsg::Power(PowerAction::Logout)) {
        m.logout_armed = false;
    }
    match msg {
        LauncherMsg::SearchChanged(query) => {
            m.query = query;
            m.selected = None;
            Cmd::None
        }
        LauncherMsg::SearchActivated | LauncherMsg::ActivateSelected => m.launch_selected(),
        LauncherMsg::MoveUp | LauncherMsg::MoveDown => {
            move_vertical(m, matches!(msg, LauncherMsg::MoveUp))
        }
        LauncherMsg::MoveLeft | LauncherMsg::MoveRight => {
            move_pane(m, matches!(msg, LauncherMsg::MoveRight))
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
        // Power actions: each calls the `icedtea-session` CLI per
        // [`power_argv`], except logout, which takes the compositor quit
        // path. A failed call surfaces as `m.status`, never silently;
        // a success dismisses the menu (the session is going away, or the
        // locker covers it).
        LauncherMsg::Power(action) => handle_power(m, action),
        LauncherMsg::Close => m.close(),
    }
}

/// One power action (extracted from [`update`]): the `icedtea-session` CLI
/// route goes through [`power_argv`] and `power_run`, logout takes the
/// compositor quit path.
/// Logout needs a confirm press — the first arms with a status, the second
/// quits — and a refused quit (`false`, a dead bus) is a logout-failed
/// status, never a silent dismissal.
fn handle_power(m: &mut LauncherModel, action: PowerAction) -> Cmd<LauncherMsg> {
    match power_argv(action) {
        // The `icedtea-session` CLI owns the org.icedtea.Session call;
        // `power_run` stays as the injection seam for tests.
        Some((program, args)) => match (m.power_run)(program, args) {
            Ok(()) => m.close(),
            Err(err) => {
                m.status = Some(power_error_message(action, &err.to_string()));
                Cmd::None
            }
        },
        // Logout: the compositor quit path, never a shell-out.
        None => {
            if !m.logout_armed {
                m.logout_armed = true;
                m.status = Some("Press Log out again to confirm".to_string());
                return Cmd::None;
            }
            if m.wm.quit() {
                m.close()
            } else {
                m.status = Some(power_error_message(
                    action,
                    "the compositor did not respond",
                ));
                Cmd::None
            }
        }
    }
}

/// Left/Right across panes (extracted from [`update`]): jump between the
/// section starts of [`LauncherModel::visual_rows`] — pinned rail, then
/// All-apps, then tiles — so the tiles pane is reachable even when every
/// tile is also pinned. The selection stays a filtered index (what
/// Up/Down, launch and highlight all address); a duplicated entry resolves
/// to its first visual row, and every occurrence shares one id, so all of
/// them light together.
fn move_pane(m: &mut LauncherModel, right: bool) -> Cmd<LauncherMsg> {
    let entries = m.filtered();
    let rows = m.visual_rows(&entries);
    if rows.is_empty() {
        m.selected = None;
        return Cmd::None;
    }
    let mut starts = vec![0usize];
    for i in 1..rows.len() {
        if rows[i].0 != rows[i - 1].0 {
            starts.push(i);
        }
    }
    let current = m
        .selected
        .and_then(|selected| rows.iter().position(|&(_, i)| i == selected))
        .unwrap_or(0);
    let landing = if right {
        starts
            .iter()
            .find(|&&s| s > current)
            .copied()
            .unwrap_or(rows.len() - 1)
    } else {
        starts.iter().rfind(|&&s| s < current).copied().unwrap_or(0)
    };
    m.selected = Some(rows[landing].1);
    Cmd::None
}

/// Up/Down within the filtered list (extracted from [`update`]: pure
/// selection fold, no behavior of its own).
fn move_vertical(m: &mut LauncherModel, up: bool) -> Cmd<LauncherMsg> {
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

/// The `.desktop` world's fingerprint: the file count paired with the
/// newest modification time over exactly the file set a scan would read
/// ([`desktop_paths`], metadata only — no file reads, so a steady-state
/// [`LauncherModel::open`] does no I/O). The count catches what mtime
/// alone misses: a removal, or an add carrying an old mtime, leaves the
/// newest stamp untouched while the set changed. `(0, None)` when no dir
/// holds a `.desktop` file; any install, removal or edit moves the pair.
fn fingerprint(dirs: &[PathBuf]) -> (usize, Option<SystemTime>) {
    let paths = desktop_paths(dirs);
    let mut skipped = 0usize;
    let mut latest: Option<SystemTime> = None;
    for path in &paths {
        match std::fs::metadata(path).and_then(|md| md.modified()) {
            Ok(modified) => {
                latest = latest.max(Some(modified));
            }
            Err(_) => {
                skipped += 1;
                tracing::debug!(path = %path.display(), "launcher fingerprint: skipping entry without mtime");
            }
        }
    }
    tracing::debug!(
        count = paths.len(),
        skipped,
        "launcher fingerprint scanned the desktop file set"
    );
    (paths.len(), latest)
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
        [left_rail(m, filtered, top), tiles_pane(m, filtered, &ids)],
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
/// [`TileGroup`](icedtea_config::TileGroup) `size` in [`TILE_PX`] units.
fn tiles_pane(
    m: &LauncherModel,
    filtered: &[&DesktopEntry],
    filtered_ids: &[&str],
) -> View<LauncherMsg> {
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
                .map(|e| tile_row(e, group.size, m.selected_for(&e.id, filtered)))
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

/// One row button: click launches, right-click pins/unpins, any other
/// button is inert. Exactly one handler (`on_pointer_up_with_button`) so a
/// left release can never double-fire through a second `on_click`
/// (M5-D5 §5). `width` is `Some` for tiles (group-size units), `None` for
/// plain app rows.
fn row_button(
    prefix: &str,
    key: u64,
    name: &str,
    id: &str,
    width: Option<i32>,
) -> View<LauncherMsg> {
    let launch_id = id.to_string();
    let pin_id = id.to_string();
    let mut row = button(name)
        .key(key)
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
    if let Some(width) = width {
        row = row.width_request(width);
    }
    row
}

/// One app row: a [`row_button`] with the pane's id prefix, plus the
/// top-hit and keyboard-selection classes.
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
    let mut row = row_button(prefix, stable_key(&entry.id), &entry.name, &entry.id, None);
    if top == Some(entry.id.as_str()) {
        row = row.class("suggested-action");
    }
    if selected {
        row = row.class("active");
    }
    row
}

/// One tile: a [`row_button`] with the `tile_` prefix and the group-size
/// width, plus the keyboard-selection class so a pane jump into the tiles
/// pane lights the tile, not just its All-apps twin.
fn tile_row(entry: &DesktopEntry, size: u32, selected: bool) -> View<LauncherMsg> {
    // Clamped width math: `size` is config-controlled (hand-editable
    // redb), so a 0 span must still paint one unit and a huge span must
    // clamp, never wrap (debug) or truncate the surface (release). The
    // product below cannot overflow `i32` (see [`MAX_TILE_SPAN`]).
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the span is capped at MAX_TILE_SPAN units, far below i32::MAX / TILE_PX"
    )]
    let width = size.clamp(1, MAX_TILE_SPAN) as i32 * TILE_PX;
    let mut row = row_button(
        "tile",
        stable_key(&format!("tile:{}", entry.id)),
        &entry.name,
        &entry.id,
        Some(width),
    );
    if selected {
        row = row.class("active");
    }
    row
}

/// The bottom row: lock / log out / suspend / restart / shut down.
/// View-only: each button folds `LauncherMsg::Power`, which [`update`]
/// routes through [`power_argv`] (the `icedtea-session` CLI via `power_run`,
/// logout via the compositor quit path with a confirm press); every failure
/// surfaces as `status`, never silently.
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

/// A live menu: how to ask it to dismiss itself, the thread running
/// it, and its completion signal.
struct LiveMenu {
    stop: icedtea_ui::view::InboxSender<LauncherMsg>,
    handle: std::thread::JoinHandle<()>,
    done: std::sync::mpsc::Receiver<()>,
}

/// Reap a menu that dismissed itself (or died): a finished handle joins
/// immediately; a received completion joins after thread teardown only
/// (microseconds — the child sent it past `App::run`). A disconnected
/// channel means the child died without completing — free the slot all
/// the same, or Start would brick forever on one dead menu. A panicked
/// child logs its payload as an error; the slot frees either way.
fn reap(running: &mut Option<LiveMenu>) {
    use std::sync::mpsc::TryRecvError;

    let finished = running.as_ref().is_some_and(|menu| {
        menu.handle.is_finished() || !matches!(menu.done.try_recv(), Err(TryRecvError::Empty))
    });
    if !finished {
        return;
    }
    let Some(menu) = running.take() else {
        return;
    };
    if let Err(payload) = menu.handle.join() {
        // The payload is `Box<dyn Any>`: report a string one, note any
        // other, and free the slot either way.
        if let Some(msg) = payload.downcast_ref::<String>() {
            tracing::error!(%msg, "the launcher surface thread panicked; its slot is free");
        } else if let Some(msg) = payload.downcast_ref::<&str>() {
            tracing::error!(%msg, "the launcher surface thread panicked; its slot is free");
        } else {
            tracing::error!(
                "the launcher surface thread panicked with a non-string payload; its slot is free"
            );
        }
    }
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
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    // The reap tick: a reopen that arrives while the old menu is still
    // tearing down (a `true` during shutdown) cannot spawn yet — the slot
    // is held — so the intent waits in `pending_open` until a tick's reap
    // frees the slot, then spawns. A `false` clears it: an explicit close
    // cancels the reopen. Without the tick the intent could only fire on
    // the next message, and a reopen sent during shutdown would drop.
    const REAP_TICK: Duration = Duration::from_millis(50);
    let mut running: Option<LiveMenu> = None;
    let mut pending_open = false;

    loop {
        match rx.recv_timeout(REAP_TICK) {
            Ok(want_open) => {
                reap(&mut running);
                if !want_open {
                    pending_open = false;
                    if let Some(menu) = &running {
                        // Ask the live menu to dismiss itself through its
                        // own close path: it notifies the panel and quits
                        // its loop. A failed send means it already left;
                        // the next reap collects it.
                        let _ = menu.stop.send(LauncherMsg::Close);
                    }
                    continue;
                }
                if running.is_some() {
                    pending_open = true;
                    continue;
                }
                // This spawn consumes a fresh `true` or a waiting reopen.
                pending_open = false;
                spawn_menu(&panel_tx, &bar_position, &mut running);
            }
            Err(RecvTimeoutError::Timeout) => {
                reap(&mut running);
                if pending_open && running.is_none() {
                    pending_open = false;
                    spawn_menu(&panel_tx, &bar_position, &mut running);
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // The panel dropped its end (process teardown): ask a live menu to
    // dismiss itself and end. No join — the process is going away, and
    // joining a loop that may never fold again would hang teardown.
    if let Some(menu) = running.take() {
        let _ = menu.stop.send(LauncherMsg::Close);
    }
}

/// Spawn one launcher surface into `running`: the `supervise` loop's spawn
/// step, shared by fresh `true`s and waiting reopens. Every failure here
/// degrades to "the menu never opens", never a panic and never the panel:
///
/// this runs on a foreign thread, so no `unwrap`/`expect` below — errors
/// log and notify the panel shut.
fn spawn_menu(
    panel_tx: &icedtea_ui::view::InboxSender<crate::panel::Msg>,
    bar_position: &str,
    running: &mut Option<LiveMenu>,
) {
    let (inbox, stop) = match icedtea_ui::view::Inbox::<LauncherMsg>::new() {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(%err, "the launcher inbox failed; Start does nothing");
            let _ = panel_tx.send(crate::panel::Msg::LauncherClosed);
            return;
        }
    };
    let position = bar_position.to_string();
    let closed = panel_tx.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    match std::thread::Builder::new()
        .name("launcher-surface".into())
        .spawn(move || run_surface(&position, closed, inbox, done_tx))
    {
        Ok(handle) => {
            *running = Some(LiveMenu {
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
    let mut app = icedtea_ui::view::App::new(model, update, view)
        .with_inbox(inbox)
        .with_autofocus_first(true);
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
    ///
    /// One of four `MockWm`s (the others: `shell/tests/support/mod.rs`,
    /// `shell/tests/launcher.rs`, `shell/src/panel.rs`'s unit tests —
    /// threading genuinely differs, so no structural unification). Each
    /// must implement every [`CompositorCommands`] method: `focus_window`,
    /// `close_window`, `set_workspace`, `spawn_app`, `quit`.
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
        fn quit(&self) -> bool {
            self.quits.set(self.quits.get() + 1);
            self.succeed.get()
        }
    }

    /// A power runner that always succeeds, standing in for the real
    /// `icedtea-session` CLI (which no test may execute).
    fn ok_power(_program: &str, _args: &[&str]) -> std::io::Result<()> {
        Ok(())
    }

    /// A power runner that always fails, standing in for a denied or
    /// missing `icedtea-session`.
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
        // Visual order mirrors the layout: the pinned rail's rows, then
        // every All-apps row, then the tiles pane's rows. firefox is pinned
        // AND tiled, so it owns a row in each pane: the visual rows are
        // [(Pinned,0), (All,0..3), (Tiled,0)] with section starts {0, 1, 5}.
        let (mut m, _) = seeded();
        let _ = update(&mut m, LauncherMsg::MoveDown);
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(1));
        let _ = update(&mut m, LauncherMsg::MoveLeft);
        assert_eq!(
            m.selected,
            Some(0),
            "Left jumps to the pinned section's app"
        );
        let _ = update(&mut m, LauncherMsg::MoveRight);
        assert_eq!(
            m.selected,
            Some(0),
            "Right steps from the rail into the All pane on the same app"
        );
        // Reach the tiles pane: from Music, Right jumps to the tile
        // section, and the tile itself carries the selection.
        let _ = update(&mut m, LauncherMsg::MoveDown);
        let _ = update(&mut m, LauncherMsg::MoveDown);
        assert_eq!(m.selected, Some(2));
        let _ = update(&mut m, LauncherMsg::MoveRight);
        assert_eq!(m.selected, Some(0), "Right jumps to the tiles section");
        assert!(
            classes(&view(&m), "tile_firefox").contains(&"active".to_string()),
            "the tile lights with the selection, not just its All-apps twin"
        );
        // No rows, no selection — in either direction.
        let _ = update(&mut m, LauncherMsg::SearchChanged("zzzz".into()));
        let _ = update(&mut m, LauncherMsg::MoveLeft);
        assert_eq!(m.selected, None);
        let _ = update(&mut m, LauncherMsg::MoveRight);
        assert_eq!(m.selected, None);
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
    fn power_cli_success_closes_the_menu() {
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
    fn power_cli_failure_reports_a_status_line_and_stays_open() {
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

    /// The real [`run_power_command`] against stub executables: hermetic
    /// exit-code mapping with no external deps. The stubs are data files
    /// (temp-dir `#!/bin/sh` scripts, `exit 0` / `exit 3`) executed through
    /// the real exec path; the missing-binary case is a nonexistent path.
    /// Success maps to `Ok`, any non-zero exit or io failure to `Err`.
    #[test]
    fn power_real_runner_maps_exit_codes_hermetically() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "icedtea-power-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("power temp dir");

        fn stub(dir: &std::path::Path, name: &str, code: u8) -> String {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\nexit {code}\n")).expect("stub script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("stub executable");
            path.to_string_lossy().into_owned()
        }

        let ok = stub(&dir, "ok.sh", 0);
        let denied = stub(&dir, "denied.sh", 3);
        assert!(run_power_command(&ok, &[]).is_ok(), "exit 0 is success");
        assert!(
            run_power_command(&denied, &[]).is_err(),
            "exit 3 is a refusal"
        );
        assert!(
            run_power_command(&dir.join("no-such-binary").to_string_lossy(), &[]).is_err(),
            "a missing binary is an io failure, never a panic"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
    fn power_logout_needs_a_confirm_press_then_quits_and_closes() {
        let (mut m, wm) = seeded();
        m.open = true;
        // First press arms: no quit, menu stays open, arming status shows.
        let cmd = update(&mut m, LauncherMsg::Power(PowerAction::Logout));
        assert_eq!(wm.quits.get(), 0, "the first press only arms");
        assert!(m.open, "arming keeps the menu open");
        assert!(!quits(&cmd));
        let status = m.status.clone().expect("an arming status");
        assert!(
            status.contains("again to confirm"),
            "arming status: {status}"
        );
        assert!(by_id(&view(&m), "launcher_status").is_some());
        // An intervening message disarms: the next press arms anew.
        let _ = update(&mut m, LauncherMsg::SearchChanged("x".into()));
        let _ = update(&mut m, LauncherMsg::Power(PowerAction::Logout));
        assert_eq!(wm.quits.get(), 0, "disarmed by the typing; arms again");
        // Second consecutive press quits through the compositor path.
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
    fn power_logout_refused_quit_reports_a_status_line_and_stays_open() {
        let (mut m, _) = seeded();
        let failing = Rc::new(MockWm::failing());
        m.wm = failing.clone();
        m.open = true;
        let _ = update(&mut m, LauncherMsg::Power(PowerAction::Logout));
        let cmd = update(&mut m, LauncherMsg::Power(PowerAction::Logout));
        assert_eq!(failing.quits.get(), 1, "the quit was attempted");
        assert!(m.open, "a refused logout keeps the menu open");
        assert!(!quits(&cmd));
        let status = m.status.clone().expect("a status line");
        assert!(status.contains("Log out"), "names the action: {status}");
        assert!(by_id(&view(&m), "launcher_status").is_some());
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
    fn tile_size_zero_clamps_to_one_unit_instead_of_vanishing() {
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm);
        m.index = DesktopIndex::from_entries(vec![entry("music", "Music")]);
        // A 0 span in hand-editable redb: the seed clamps it to one unit,
        // so the tile still paints instead of collapsing to zero width.
        m.seed(&icedtea_config::LauncherConfig {
            pinned: Vec::new(),
            tile_groups: vec![icedtea_config::TileGroup {
                name: "Media".to_string(),
                ids: vec!["music".to_string()],
                size: 0,
            }],
            recency: std::collections::HashMap::new(),
        });
        assert_eq!(
            m.tiles.groups()[0].size,
            1,
            "a 0 span cannot persist into the store"
        );
        let width = by_id(&view(&m), "tile_music")
            .expect("tile")
            .props
            .int(PropName::WidthRequest, 0);
        assert_eq!(width, i64::from(TILE_PX), "a clamped tile is one unit wide");
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
    fn fingerprint_moves_on_removal_even_when_the_newest_mtime_survives() {
        // The stale-index bug: mtime alone misses a removal (or an add
        // carrying an old mtime) whenever the newest stamp survives the
        // change. The count half of the fingerprint catches it.
        let dir =
            std::env::temp_dir().join(format!("icedtea-launcher-fprem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("a.desktop"),
            "[Desktop Entry]\nName=App A\nExec=a\n",
        )
        .expect("fixture");
        std::fs::write(
            dir.join("b.desktop"),
            "[Desktop Entry]\nName=App B\nExec=b\n",
        )
        .expect("fixture");
        let before = fingerprint(std::slice::from_ref(&dir));
        assert_eq!(before.0, 2, "two files fingerprinted");
        // Remove the older file: the newest mtime (b's) is untouched, so
        // only the count moves the fingerprint.
        std::fs::remove_file(dir.join("a.desktop")).expect("fixture");
        let after = fingerprint(std::slice::from_ref(&dir));
        assert_eq!(after.0, 1, "one file left");
        assert_eq!(
            after.1, before.1,
            "the newest mtime survived, pinning the stale-index scenario"
        );
        assert_ne!(after, before, "the count moves the fingerprint on removal");
        // And the open path rescans on it: the removed app is gone.
        let wm = Rc::new(MockWm::ok());
        let mut m = LauncherModel::new(wm);
        m.dirs = vec![dir.clone()];
        m.open();
        assert_eq!(m.index.apps().len(), 1);
        assert!(m.index.find("b").is_some());
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
