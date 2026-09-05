//! Harness helpers shared by the settings integration tests.
//!
//! Each integration test file compiles this module separately (`mod
//! support;`), and no single test file uses every item here — the same
//! reason `ui/tests/support/mod.rs` carries this same allow.
//!
//! P2-D2 (M5 part2 contract deviations) rules that this module is shared
//! with P1 and extended, not replaced: Task 9 (P2) added everything from
//! [`Reaper`] down, whose signatures are normative for P3/P4. Four names
//! P1 had already taken this shape for (`spawn_settings`, `click`,
//! `paints_something`, and `pixel_at`'s argument type) collided outright
//! with Task 9's normative signatures — Rust has no overloading — so the
//! three that could not simply be widened were renamed in place
//! (`spawn_settings_process`, `click_fixed`, `paints_something_anywhere`)
//! and their two P1 call sites (`skeleton.rs`, `outputs_pump.rs`) updated to
//! match; `pixel_at` itself is widened to Task 9's `i32` signature since its
//! one existing call site (`skeleton.rs`, literal `1, 1`) is unaffected.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// A 480x420 toplevel window on `comp`'s socket, with the bundled Adwaita
/// sheet — the same surface `icedtea-settings` opens.
pub fn open_test_window(comp: &icedtea_harness::Compositor) -> icedtea_ui::window::Window {
    // SAFETY-free: the harness owns the socket for the life of `comp`, and
    // `Window::open` reads `$WAYLAND_DISPLAY` once, here, before any thread
    // that could race it exists.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &comp.socket) };
    icedtea_ui::window::Window::open(
        icedtea_ui::window::SurfaceSpec {
            role: icedtea_ui::window::Role::Toplevel,
            size: (480, 420),
            title: "icedtea Settings".to_string(),
            app_id: "org.icedtea.Settings".to_string(),
        },
        icedtea_ui::app::compile_theme(&icedtea_ui::app::ThemeSource::Bundled),
        icedtea_ui::text::FontDatabase::new(),
    )
    .expect("open a toplevel on the harness")
}

/// A running `icedtea-settings` against a harness compositor, with its probe
/// report on disk.
pub struct SettingsProc {
    child: Child,
    report: std::path::PathBuf,
    #[allow(dead_code, reason = "kept alive so the temp dir outlives the child")]
    dir: tempfile::TempDir,
}

impl Drop for SettingsProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The directory cargo put this test binary in, which is also where it put
/// `icedtea-settings`.
fn target_profile_dir() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop(); // deps/
    path.pop(); // debug/ or release/
    path
}

/// Spawn the real settings binary on `comp`'s socket, with a private config db
/// and a probe report.
///
/// Renamed from P1's original `spawn_settings` (P2-D2): Task 9 needs that
/// name for a differently-shaped helper (below) that a caller controls with
/// an already-seeded config dir, an explicit socket string, a theme and a
/// starting page — this one keeps P1's simpler shape, spawning against
/// nothing but a `Compositor`.
pub fn spawn_settings_process(comp: &icedtea_harness::Compositor) -> SettingsProc {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("report");
    let child = Command::new(target_profile_dir().join("icedtea-settings"))
        .env("WAYLAND_DISPLAY", &comp.socket)
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .env("ICEDTEA_PROBE_REPORT", &report)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn icedtea-settings");
    SettingsProc { child, report, dir }
}

impl SettingsProc {
    /// Every line reported so far.
    pub fn lines(&self) -> Vec<String> {
        let mut text = String::new();
        if let Ok(mut file) = std::fs::File::open(&self.report) {
            let _ = file.read_to_string(&mut text);
        }
        text.lines().map(str::to_owned).collect()
    }

    /// Poll the report until a line starting with `prefix` appears.
    pub fn wait_line(&self, prefix: &str, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(line) = self.lines().into_iter().find(|l| l.starts_with(prefix)) {
                return Some(line);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }

    /// The most recent `probe <id> <x> <y>` line's coordinates.
    pub fn point(&self, id: &str) -> Option<(i32, i32)> {
        let prefix = format!("probe {id} ");
        let line = self
            .lines()
            .into_iter()
            .rev()
            .find(|l| l.starts_with(&prefix))?;
        let mut fields = line.split_whitespace().skip(2);
        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    }

    /// The most recent `alloc <id> <x> <y> <w> <h>` line's box.
    pub fn allocation(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        let prefix = format!("alloc {id} ");
        let line = self
            .lines()
            .into_iter()
            .rev()
            .find(|l| l.starts_with(&prefix))?;
        let mut f = line.split_whitespace().skip(2);
        Some((
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
        ))
    }
}

/// The harness's headless output size (`wlr`'s default headless mode, with
/// no `WLR_HEADLESS_OUTPUTS` size override — see `harness::Compositor::spawn`).
/// `wlr_virtual_pointer_v1.motion_absolute`'s `x`/`y` are normalised against
/// an `x_extent`/`y_extent` the caller supplies, not raw output pixels;
/// passing the real output size here is what makes the `x`, `y` this module
/// takes behave as plain output-logical pixels. `ui/tests/window_events.rs`
/// gets this from `Compositor::output_size()` at each call site instead of a
/// constant — reconciled the same way here would mean threading `&Compositor`
/// through every `click` call the plan gives this module a fixed signature
/// for, so it is hard-coded to the one size this harness ever boots.
const OUTPUT_SIZE: (u32, u32) = (1280, 720);

/// Move, press and release at `(x, y)` in output coordinates, with the settle
/// loop `ui/tests/support`'s `Driver::click` uses (surface focus is assigned
/// asynchronously; a press that races it lands nowhere).
///
/// Renamed from P1's original `click` (P2-D2): Task 9's `click` (below) also
/// pumps a `ScreencopyClient` to size its motion against the real captured
/// frame, which this one — fixed against [`OUTPUT_SIZE`] — has no parameter
/// for.
pub fn click_fixed(pointer: &mut icedtea_harness::VirtualPointerClient, x: i32, y: i32) {
    for _ in 0..8 {
        pointer.motion_absolute(f64::from(x), f64::from(y), OUTPUT_SIZE.0, OUTPUT_SIZE.1);
        pointer.frame();
        pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
    }
    pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
    pointer.frame();
    pointer.pump();
    std::thread::sleep(Duration::from_millis(25));
    pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
}

/// Whether `frame` contains any pixel that is not the compositor's wallpaper.
///
/// Sampled on a 4px grid: even a modest screencopy frame is well over a
/// million pixels, and the question is only "did anything paint at all".
///
/// Renamed from P1's original `paints_something` (P2-D2): Task 9's
/// `paints_something` (below) checks one caller-given rectangle exactly,
/// rather than the whole frame on a stride, which this one keeps for its one
/// existing call site.
pub fn paints_something_anywhere(
    frame: &icedtea_harness::CapturedFrame,
    background: (u8, u8, u8),
) -> bool {
    (0..frame.height).step_by(4).any(|y| {
        (0..frame.width)
            .step_by(4)
            .any(|x| pixel_at(frame, x as i32, y as i32).is_some_and(|px| px != background))
    })
}

// ---------------------------------------------------------------------------
// Task 9 (P2): scaffolding for the Appearance/Behavior harness gates.
// ---------------------------------------------------------------------------

/// The compositor's server-side title bar (`compositor/src/decoration.rs`),
/// mirrored from `ui/tests/window_events.rs`.
pub const TITLE_BAR_HEIGHT: i32 = 28;

/// The app id `settings/src/main.rs` registers.
pub const SETTINGS_APP_ID: &str = "org.icedtea.Settings";

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture, matching `ui/tests/support/mod.rs`'s constant and
/// its reasoning: the output goes through a format conversion the offscreen
/// gate does not.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// How long a capture loop sleeps between frames.
const POLL: Duration = Duration::from_millis(25);

/// Kill the child on the way out however the test ends.
pub struct Reaper(pub Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One `alloc <id> <x> <y> <w> <h>` line.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryAllocation {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One `probe <label> <x> <y>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// A temp `XDG_CONFIG_HOME` holding a config `edit` has had its way with.
///
/// `icedtea_config::default_db_path` reads `XDG_CONFIG_HOME`, so pointing the
/// child at this directory is all the isolation a gate needs — no new env knob.
#[must_use]
pub fn seeded_config_dir(edit: impl FnOnce(&mut icedtea_config::Config)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("config dir");
    let db = dir.path().join("icedtea").join("config.redb");
    std::fs::create_dir_all(db.parent().expect("parent")).expect("create the config dir");
    let mut cfg = icedtea_config::default_config();
    edit(&mut cfg);
    icedtea_settings::model::apply(&cfg, &db).expect("seed the config db");
    dir
}

/// The bundled sheet content a `theme` name (`light`/`dark`/`hc`) selects.
///
/// Reconciliation (Task 9): the M5 part0 contract's E3 ruling — binding on
/// both P2-D7 and P3-D6 — settles a contradiction between this plan's own
/// deviation note and its illustrative code: `$ICEDTEA_UI_THEME` names a
/// *path* to a complete base theme file (`ui/src/bin/window-probe.rs:41`'s
/// convention), not the literal string `spawn_settings`'s `theme` parameter
/// takes. `theme` keeps the plan's exact three-way spelling; this function
/// resolves it to bundled CSS content that [`spawn_settings`] writes to a
/// file inside `config_home` before pointing the child's environment at
/// that path.
fn bundled_theme_content(theme: &str) -> &'static str {
    match theme {
        "dark" => icedtea_ui::BUNDLED_ADWAITA_DARK,
        "hc" => icedtea_ui::BUNDLED_ADWAITA_HC,
        _ => icedtea_ui::BUNDLED_ADWAITA_LIGHT,
    }
}

/// Spawn `icedtea-settings` against `socket`, reaped when the guard drops.
///
/// `theme` is `light`/`dark`/`hc`; `page` is a `PageId::name()` value. See
/// [`bundled_theme_content`] for why `theme` does not travel to the child
/// verbatim.
#[must_use]
pub fn spawn_settings(
    socket: &str,
    config_home: &Path,
    theme: &str,
    page: &str,
    report: &Path,
) -> Reaper {
    let theme_path = config_home.join("theme.css");
    std::fs::write(&theme_path, bundled_theme_content(theme)).expect("write the theme file");
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
            .env("WAYLAND_DISPLAY", socket)
            .env("XDG_CONFIG_HOME", config_home)
            .env("ICEDTEA_UI_THEME", &theme_path)
            .env("ICEDTEA_SETTINGS_PAGE", page)
            .env("ICEDTEA_PROBE_REPORT", report)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn icedtea-settings"),
    )
}

/// Poll the compositor's model until the settings window is in it.
///
/// Reconciliation (Task 9): the plan's own literal code waited twenty
/// seconds, but `skeleton.rs`'s Task 7 reconciliation already recorded that
/// the real Appearance page (drop-down, two spin buttons, three colour
/// swatches, a wallpaper entry, a status row) is heavy enough under this
/// harness's headless backend that its wlr `mapped` signal — which trails
/// the client's own layout, and so its first probe report, by a visible
/// amount — was measured landing at ~22-23s there; empirically here it lands
/// at ~28.5s. Widened to match `skeleton.rs`'s `SETTLE` (45s) for the same
/// reason and the same margin, not because the property being waited for
/// changed.
///
/// # Panics
/// If the window never appears within the budget — a mapped window is the
/// precondition of every gate here, not something to skip over.
#[must_use]
pub fn wait_for_window(compositor: &Compositor) -> icedtea_contract::Rectangle {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(45) {
        if let Some(window) = compositor
            .snapshot()
            .windows
            .into_iter()
            .find(|w| w.app_id == SETTINGS_APP_ID)
        {
            return window.geometry;
        }
        std::thread::sleep(POLL);
    }
    panic!("the settings window never entered the compositor's model");
}

/// Where the client area starts, in output coordinates: the frame's origin
/// plus the server-side title bar the compositor draws above it.
#[must_use]
pub fn client_origin(geometry: icedtea_contract::Rectangle) -> (i32, i32) {
    (geometry.x, geometry.y + TITLE_BAR_HEIGHT)
}

/// Every line the app has reported so far.
#[must_use]
pub fn report_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Poll the report until a line starting with `prefix` appears.
#[must_use]
pub fn wait_for_prefix(path: &Path, prefix: &str, timeout: Duration) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(line) = report_lines(path)
            .into_iter()
            .find(|l| l.starts_with(prefix))
        {
            return Some(line);
        }
        std::thread::sleep(POLL);
    }
    None
}

/// The most recent allocation reported for each id.
///
/// The app appends a fresh block every time the tree changes, so the last
/// line for an id is the only one that describes what is on screen now.
#[must_use]
pub fn latest_allocations(path: &Path) -> HashMap<String, EntryAllocation> {
    let mut out = HashMap::new();
    for line in report_lines(path) {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("alloc") {
            continue;
        }
        let Some(id) = fields.next() else { continue };
        let numbers: Vec<f32> = fields.filter_map(|f| f.parse().ok()).collect();
        if numbers.len() != 4 {
            continue;
        }
        out.insert(
            id.to_string(),
            EntryAllocation {
                id: id.to_string(),
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            },
        );
    }
    out
}

/// The most recent probe point reported for each label.
#[must_use]
pub fn latest_probe_points(path: &Path) -> Vec<ProbePoint> {
    let mut out: HashMap<String, ProbePoint> = HashMap::new();
    for line in report_lines(path) {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("probe") {
            continue;
        }
        let Some(label) = fields.next() else { continue };
        let numbers: Vec<i32> = fields.filter_map(|f| f.parse().ok()).collect();
        if numbers.len() != 2 {
            continue;
        }
        out.insert(
            label.to_string(),
            ProbePoint {
                label: label.to_string(),
                x: numbers[0],
                y: numbers[1],
            },
        );
    }
    let mut points: Vec<ProbePoint> = out.into_values().collect();
    points.sort_by(|a, b| a.label.cmp(&b.label));
    points
}

/// The RGB triple at `(x, y)`, or `None` outside the frame.
///
/// Widened from P1's original `u32`-only `pixel_at` (P2-D2): Task 9 needs
/// negative coordinates to be a plain `None` rather than an overflow panic
/// from casting, and its one existing caller (`skeleton.rs`, literal `1, 1`)
/// is unaffected by widening the parameter type.
#[must_use]
pub fn pixel_at(frame: &CapturedFrame, x: i32, y: i32) -> Option<(u8, u8, u8)> {
    if x < 0 || y < 0 {
        return None;
    }
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x as u32, y as u32)
}

/// Whether two RGB triples match on every channel within the tolerance.
#[must_use]
pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let close =
        |x: u8, y: u8| i32::from(x).abs_diff(i32::from(y)) <= u32::from(SCREENCOPY_TOLERANCE);
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

/// The most common colour inside `rect`.
///
/// The page background, derived rather than sampled at a corner: a settings
/// page has chrome in every corner (a switcher above, a footer below), so
/// there is no fixed pixel that is reliably bare. The window background is by
/// construction the colour most of the client area is.
#[must_use]
pub fn dominant_colour(frame: &CapturedFrame, rect: (i32, i32, i32, i32)) -> (u8, u8, u8) {
    let (x, y, w, h) = rect;
    let mut counts: HashMap<(u8, u8, u8), usize> = HashMap::new();
    for py in y..y + h {
        for px in x..x + w {
            if let Some(colour) = pixel_at(frame, px, py) {
                *counts.entry(colour).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(colour, _)| colour)
        .expect("a non-empty client area")
}

/// Whether anything inside `rect` differs from `background`.
#[must_use]
pub fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h).any(|py| {
        (x..x + w).any(|px| pixel_at(frame, px, py).is_some_and(|got| !matches(got, background)))
    })
}

/// Move, press and release the left button at `(x, y)`, letting the
/// compositor settle between phases.
///
/// The settle loop is `ui/tests/support/mod.rs`'s `move_to`: surface focus is
/// assigned asynchronously and a press sent in the same breath as the motion
/// races it.
pub fn click(
    pointer: &mut VirtualPointerClient,
    screencopy: &mut ScreencopyClient,
    x: i32,
    y: i32,
) {
    let (w, h) = (screencopy.capture().width, screencopy.capture().height);
    pointer.motion_absolute(f64::from(x), f64::from(y), w, h);
    pointer.frame();
    pointer.pump();
    for _ in 0..8 {
        std::thread::sleep(POLL);
        pointer.pump();
    }
    pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
    pointer.frame();
    pointer.pump();
    std::thread::sleep(POLL);
    pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
}

/// Capture until the pixel at `(x, y)` differs from `before`, or two seconds
/// pass. A generous complexity bound, not a timing pin.
pub fn wait_pixel_change(
    screencopy: &mut ScreencopyClient,
    x: i32,
    y: i32,
    before: (u8, u8, u8),
) -> (u8, u8, u8) {
    let started = Instant::now();
    let mut px = before;
    while started.elapsed() < Duration::from_secs(2) {
        let frame = screencopy.capture();
        px = pixel_at(&frame, x, y).unwrap_or(before);
        if !matches(px, before) {
            return px;
        }
        std::thread::sleep(POLL);
    }
    px
}
