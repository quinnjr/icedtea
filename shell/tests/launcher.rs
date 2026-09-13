//! The launcher, offscreen: rest state per theme plus the
//! open → search → dismiss transitions, with no compositor involved.
//!
//! The live-compositor e2e (open from Start, launch reaching the
//! compositor, deletion-tested spawn path) is Task 6's scope; what this
//! file pins is that the real `view`/`update` lay out and paint something
//! addressable under both Adwaita themes, and that the transition folds
//! drive the offscreen loop to dismissal.

mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use icedtea_shell::compositor_client::CompositorCommands;
use icedtea_shell::launcher_view::{self, LAUNCHER_SIZE, LauncherModel, LauncherMsg};
use icedtea_shell::style;
use icedtea_ui::anim::{Clock, ManualClock};
use icedtea_ui::css::node::Node;
use icedtea_ui::gallery::Theme;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::app::Probe;
use icedtea_ui::view::{App, ScriptStep};
use support::TempDir;

/// A recording [`CompositorCommands`]. Single-threaded (`RefCell`, not
/// `Mutex`): the offscreen loop runs on this thread.
///
/// One of four `MockWm`s (the others: `shell/tests/support/mod.rs`,
/// `shell/src/launcher_view.rs`'s unit tests, `shell/src/panel.rs`'s unit
/// tests — threading genuinely differs, so no structural unification).
/// Each must implement every `CompositorCommands` method: `focus_window`,
/// `close_window`, `set_workspace`, `spawn_app`, `quit`.
struct MockWm {
    spawns: RefCell<Vec<String>>,
    succeed: Cell<bool>,
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
        self.succeed.get()
    }
}

/// Three fixture apps on disk: `dirs` + [`LauncherModel::open`] is the
/// hermetic seam — no real XDG dirs are ever scanned here.
fn app_dir() -> TempDir {
    let dir = TempDir::new("launcher");
    for (id, name) in [
        ("firefox", "Firefox"),
        ("firetools", "Fire Tools"),
        ("music", "Music"),
    ] {
        std::fs::write(
            dir.path().join(format!("{id}.desktop")),
            format!("[Desktop Entry]\nName={name}\nExec={id}\n"),
        )
        .expect("fixture app");
    }
    dir
}

fn seeded_model(dir: &TempDir) -> (LauncherModel, Rc<MockWm>) {
    let wm = Rc::new(MockWm {
        spawns: RefCell::new(Vec::new()),
        succeed: Cell::new(true),
    });
    let mut model = LauncherModel::new(wm.clone());
    model.dirs = vec![dir.path().to_path_buf()];
    // Seed through the real config path: one pin, one size-2 tile group.
    model.seed(&icedtea_config::LauncherConfig {
        pinned: vec!["firefox".to_string()],
        tile_groups: vec![icedtea_config::TileGroup {
            name: "Web".to_string(),
            ids: vec!["firefox".to_string()],
            size: 2,
        }],
        recency: std::collections::HashMap::new(),
    });
    model.open();
    (model, wm)
}

/// Lay the launcher out offscreen under `theme`, exactly as the running
/// `App` would before its first frame.
fn probe(theme: Theme, model: LauncherModel) -> Probe<LauncherMsg> {
    let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
    App::new(model, launcher_view::update, launcher_view::view)
        .probe(
            LAUNCHER_SIZE,
            style::sheet_for(theme),
            FontDatabase::new(),
            IconTheme::with_name_and_roots("hicolor", vec![]),
            clock,
        )
        .expect("the launcher lays out offscreen")
}

/// The node `id` names, or a panic listing what the tree does hold — a
/// renamed id reads as a failure, not a mystery.
fn node_by_id(probe: &Probe<LauncherMsg>, id: &str) -> Node {
    probe
        .root()
        .descendants()
        .find(|n| n.id().is_some_and(|got| got.as_str() == id))
        .unwrap_or_else(|| {
            let held: Vec<String> = probe
                .root()
                .descendants()
                .filter_map(|n| n.id().map(|got| got.as_str().to_string()))
                .collect();
            panic!("no node #{id}; the tree holds {held:?}");
        })
}

/// Ids the launcher must have painted something into at rest.
///
/// Deliberately the leaves a user can see and touch, not every container:
/// `launcher`, `launcher_body` and `launcher_left` own no paint of their
/// own, and asserting on them would pass for free.
const REST_IDS: &[&str] = &[
    "launcher_search",
    "launcher_pinned",
    "launcher_all",
    "launcher_tiles",
    "launcher_power",
];

/// Layout only, by name: the offscreen `Probe` carries allocations but no
/// pixels (and no known background to sample against), so per-id paint
/// sampling like the panel gate is infeasible here — the pixel oracle lives
/// in `typing_at_open_reaches_the_search_box_without_a_prior_tab` instead.
#[test]
fn launcher_rest_state_lays_out_in_light_and_dark() {
    for theme in [Theme::Light, Theme::Dark] {
        let dir = app_dir();
        let (model, _) = seeded_model(&dir);
        let probe = probe(theme, model);
        for id in REST_IDS {
            let node = node_by_id(&probe, id);
            let alloc = probe
                .allocation(&node)
                .unwrap_or_else(|| panic!("{theme:?}: #{id} has no allocation"));
            assert!(
                alloc.border_box.width > 0.0 && alloc.border_box.height > 0.0,
                "{theme:?}: #{id} lays out empty"
            );
        }
    }
}

#[test]
fn search_narrows_the_all_apps_list_offscreen() {
    let dir = app_dir();
    let (mut model, _) = seeded_model(&dir);
    let _ = launcher_view::update(&mut model, LauncherMsg::SearchChanged("fire".into()));
    let probe = probe(Theme::Dark, model);
    let mut rows: Vec<String> = probe
        .root()
        .descendants()
        .filter_map(|n| n.id())
        .map(|id| id.as_str().to_string())
        .filter(|id| id.starts_with("app_"))
        .collect();
    rows.sort();
    assert_eq!(rows, vec!["app_firefox", "app_firetools"]);
    // Exactly one row carries `.suggested-action` — the rank top hit
    // ("Fire Tools" first: both names prefix-match, tiebreak is name
    // order, as the unit suite pins).
    let suggested: Vec<String> = rows
        .iter()
        .filter(|id| {
            node_by_id(&probe, id)
                .classes()
                .iter()
                .any(|c| c.as_str() == "suggested-action")
        })
        .cloned()
        .collect();
    assert_eq!(suggested, vec!["app_firetools".to_string()]);
}

#[test]
fn dismissal_quits_the_offscreen_loop() {
    let dir = app_dir();
    let (model, _) = seeded_model(&dir);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(model, launcher_view::update, launcher_view::view)
        .with_sheet(style::sheet_for(Theme::Dark))
        .with_fonts(FontDatabase::new())
        .with_icons(IconTheme::with_name_and_roots("hicolor", vec![]))
        .run_offscreen(
            LAUNCHER_SIZE,
            clock,
            vec![
                ScriptStep::Message(LauncherMsg::SearchChanged("mus".into())),
                ScriptStep::Capture,
                ScriptStep::Message(LauncherMsg::Close),
            ],
        )
        .expect("the open → search → dismiss script runs offscreen");
    assert_eq!(
        frames.len(),
        1,
        "the single Capture survives to dismissal; Quit ends the loop after it"
    );
}

/// Remainder (b): pin/unpin + recency mutations reach `Config::save` on
/// close, so a reopened menu still shows them. Session 1 pins one app and
/// launches another (the launch's close writes back); session 2 is a fresh
/// model seeded from disk, and the pin renders in its pinned rail.
#[test]
fn pin_and_recency_survive_close_and_reopen() {
    use icedtea_shell::launcher_view::persist_launcher_config;

    let dir = app_dir();
    let db_dir = TempDir::new("launcher-persist");
    let db_path = db_dir.path().join("cfg.redb");

    // Session 1: pin first (no close yet), then launch — the successful
    // launch closes the menu, and the close writes pins + recency back.
    {
        let wm = Rc::new(MockWm {
            spawns: RefCell::new(Vec::new()),
            succeed: Cell::new(true),
        });
        let mut model = LauncherModel::new(wm);
        model.dirs = vec![dir.path().to_path_buf()];
        model.open();
        let path = db_path.clone();
        model.on_persist = Some(Rc::new(move |cfg| {
            persist_launcher_config(&path, &cfg).expect("test write-back")
        }));
        let _ = launcher_view::update(&mut model, LauncherMsg::TogglePin("music".into()));
        let _ = launcher_view::update(&mut model, LauncherMsg::ActivateApp("firefox".into()));
    }

    let stored = icedtea_config::load_or_default(&db_path).launcher;
    assert!(
        stored.pinned.contains(&"music".to_string()),
        "the pin survived the close: {:?}",
        stored.pinned
    );
    assert_eq!(
        stored.recency.get("firefox").map(|(count, _)| *count),
        Some(1),
        "the launch survived the close: {:?}",
        stored.recency
    );

    // Session 2: a fresh model seeded from disk renders the pin.
    let wm = Rc::new(MockWm {
        spawns: RefCell::new(Vec::new()),
        succeed: Cell::new(true),
    });
    let mut reopened = LauncherModel::new(wm);
    reopened.dirs = vec![dir.path().to_path_buf()];
    reopened.seed(&stored);
    reopened.open();
    let probe = probe(Theme::Dark, reopened);
    node_by_id(&probe, "pinned_music");
}

/// Full launch path (Task 6 Step 4): open → type a real query → launch.
/// The typed keys travel the live route (compositor `KeyboardEnter`, then
/// key events into the focused search box); the launch folds through the
/// real `update` into the `MockWm`-recorded `spawn_app`, and the success
/// closes the menu, whose write-back persists the recency. Deletion-tested
/// during development: gutting the `spawn_app` call in `LauncherModel::launch`
/// fails this test (no recorded spawn), restoring it passes again.
#[test]
fn open_type_launch_reaches_the_compositor_and_records_recency() {
    use icedtea_shell::launcher_view::persist_launcher_config;
    use icedtea_ui::window::InputEvent;
    use icedtea_ui::window::keyboard::{KeyEvent, Mods};

    fn key(keysym: u32, text: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            keycode: 0,
            keysym: xkbcommon::xkb::Keysym::from(keysym),
            base: xkbcommon::xkb::Keysym::from(keysym),
            utf8: Some(text.to_string()),
            mods: Mods::empty(),
            consumed: Mods::empty(),
            pressed: true,
            repeat: false,
            serial: 0,
            time_ms: 0,
        })
    }

    let dir = app_dir();
    let db_dir = TempDir::new("launcher-launch-e2e");
    let db_path = db_dir.path().join("cfg.redb");

    let wm = Rc::new(MockWm {
        spawns: RefCell::new(Vec::new()),
        succeed: Cell::new(true),
    });
    let mut model = LauncherModel::new(wm.clone());
    model.dirs = vec![dir.path().to_path_buf()];
    model.open();
    let path = db_path.clone();
    model.on_persist = Some(Rc::new(move |cfg| {
        persist_launcher_config(&path, &cfg).expect("test write-back")
    }));

    let clock = Rc::new(ManualClock::new());
    App::new(model, launcher_view::update, launcher_view::view)
        .with_autofocus_first(true)
        .with_sheet(style::sheet_for(Theme::Dark))
        .with_fonts(FontDatabase::new())
        .with_icons(IconTheme::with_name_and_roots("hicolor", vec![]))
        .run_offscreen(
            LAUNCHER_SIZE,
            clock,
            vec![
                // The live open path: the compositor grants the Exclusive
                // surface the keyboard, the user types, then the top hit
                // launches.
                ScriptStep::Event(InputEvent::keyboard_enter(1)),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_m, "m")),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_u, "u")),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_s, "s")),
                // A real Enter key through `route`: the focused search box
                // fires `SearchActivated`, which launches the top hit.
                ScriptStep::Event(InputEvent::Key(KeyEvent {
                    keycode: 0,
                    keysym: xkbcommon::xkb::Keysym::from(xkbcommon::xkb::keysyms::KEY_Return),
                    base: xkbcommon::xkb::Keysym::from(xkbcommon::xkb::keysyms::KEY_Return),
                    utf8: None,
                    mods: Mods::empty(),
                    consumed: Mods::empty(),
                    pressed: true,
                    repeat: false,
                    serial: 0,
                    time_ms: 0,
                })),
            ],
        )
        .expect("the open → type → launch script runs offscreen");

    // The spawn reached the compositor side (the MockWm-recorded call).
    assert_eq!(
        wm.spawns.borrow().as_slice(),
        ["music".to_string()],
        "typing 'mus' then Enter launches Music"
    );
    // The success closed the menu, and the close persisted the recency.
    let stored = icedtea_config::load_or_default(&db_path).launcher;
    assert_eq!(
        stored.recency.get("music").map(|(count, _)| *count),
        Some(1),
        "the launch recorded recency: {:?}",
        stored.recency
    );
}

/// Typing at open reaches the search box with no prior Tab or click.
///
/// The spec's keyboard model binds this ("open focuses search"). The
/// open-time wiring lives in `App`'s `KeyboardEnter` arm
/// (`ui/src/view/app.rs::route`): when the compositor hands a window
/// surface the keyboard while its focus ring is empty, the first
/// focusable node — the search box, first in reading order — takes the
/// focus, so the typed keys land in it.
///
/// Why not `Cmd::Focus` from the launcher's `update`: that command
/// carries a retained `Node` handle, and only the runtime (or a
/// controller's `cx.node`) can mint one — `update` has no access to the
/// tree, so no app-level code can construct it for its own search box.
/// The route arm performs the same move one level down
/// (`FocusRing::set_focus` with `Programmatic`, exactly what the
/// `Cmd::Focus` arm in `drain` does), following the
/// `KeyboardEnter`-when-empty precedent `window-probe` runs in its own
/// loop. See the arm's own comment for the full reasoning.
///
/// The script models the live open path: the compositor maps the
/// `Exclusive` launcher surface and sends `KeyboardEnter`, then the user
/// types. No Tab, no click.
#[test]
fn typing_at_open_reaches_the_search_box_without_a_prior_tab() {
    use icedtea_ui::window::InputEvent;
    use icedtea_ui::window::keyboard::{KeyEvent, Mods};

    fn key(keysym: u32, text: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            keycode: 0,
            keysym: xkbcommon::xkb::Keysym::from(keysym),
            base: xkbcommon::xkb::Keysym::from(keysym),
            utf8: Some(text.to_string()),
            mods: Mods::empty(),
            consumed: Mods::empty(),
            pressed: true,
            repeat: false,
            serial: 0,
            time_ms: 0,
        })
    }

    let dir = app_dir();
    let (model, _) = seeded_model(&dir);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(model, launcher_view::update, launcher_view::view)
        .with_autofocus_first(true)
        .with_sheet(style::sheet_for(Theme::Dark))
        .with_fonts(FontDatabase::new())
        .with_icons(IconTheme::with_name_and_roots("hicolor", vec![]))
        .run_offscreen(
            LAUNCHER_SIZE,
            clock,
            vec![
                ScriptStep::Capture,
                // What the compositor sends when the Exclusive launcher
                // surface opens: the keyboard now belongs to this surface.
                ScriptStep::Event(icedtea_ui::window::InputEvent::keyboard_enter(1)),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_m, "m")),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_u, "u")),
                ScriptStep::Event(key(xkbcommon::xkb::keysyms::KEY_s, "s")),
                ScriptStep::Capture,
            ],
        )
        .expect("the typing script runs offscreen");
    assert_eq!(frames.len(), 2);
    // Any painted difference between the rest frame and the typed frame:
    // "mus" narrows the lists, so the pixels must move.
    let (w, h) = frames.size();
    let mut same = true;
    let mut x = 0;
    while x < w {
        let mut y = 0;
        while y < h {
            if frames.pixel(0, x, y) != frames.pixel(1, x, y) {
                same = false;
                break;
            }
            y += 7;
        }
        if !same {
            break;
        }
        x += 7;
    }
    assert!(
        !same,
        "typing narrowed nothing: the keys never reached search"
    );
    // Model oracle first, pixels second: the same query through the real
    // `update` fold must hold "mus" in search and leave Music the top hit.
    let dir = app_dir();
    let (mut model, _) = seeded_model(&dir);
    let _ = launcher_view::update(&mut model, LauncherMsg::SearchChanged("mus".into()));
    let probe = probe(Theme::Dark, model);
    let mut rows: Vec<String> = probe
        .root()
        .descendants()
        .filter_map(|n| n.id())
        .map(|id| id.as_str().to_string())
        .filter(|id| id.starts_with("app_"))
        .collect();
    rows.sort();
    assert_eq!(rows, vec!["app_music"], "search holds the query");
    assert!(
        node_by_id(&probe, "app_music")
            .classes()
            .iter()
            .any(|c| c.as_str() == "suggested-action"),
        "Music is the top hit"
    );
}
