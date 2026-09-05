//! Harness helpers shared by the settings integration tests.
//!
//! Each integration test file compiles this module separately (`mod
//! support;`), and no single test file uses every item here — the same
//! reason `ui/tests/support/mod.rs` carries this same allow.
#![allow(dead_code)]

use std::io::Read as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
pub fn spawn_settings(comp: &icedtea_harness::Compositor) -> SettingsProc {
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
pub fn click(pointer: &mut icedtea_harness::VirtualPointerClient, x: i32, y: i32) {
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

/// One pixel of a screencopy frame, as `ui/tests/support::pixel_at` reads it.
pub fn pixel_at(frame: &icedtea_harness::CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// Whether `frame` contains any pixel that is not the compositor's wallpaper.
///
/// Sampled on a 4px grid: even a modest screencopy frame is well over a
/// million pixels, and the question is only "did anything paint at all".
pub fn paints_something(frame: &icedtea_harness::CapturedFrame, background: (u8, u8, u8)) -> bool {
    (0..frame.height).step_by(4).any(|y| {
        (0..frame.width)
            .step_by(4)
            .any(|x| pixel_at(frame, x, y).is_some_and(|px| px != background))
    })
}
