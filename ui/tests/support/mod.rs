//! Shared scaffolding for `icedtea-ui`'s integration tests.
//!
//! Not every test uses every helper, hence the blanket `dead_code` allow:
//! this module is compiled once per test binary that declares it.

#![allow(dead_code)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, ScreencopyClient, VirtualPointerClient};

/// The theme every test pins its expected colours against. Never the
/// developer's own `gtk.css`.
pub const TEST_THEME: &str = "bundled";

/// Kill the child on the way out however the test ends.
pub struct Reaper(pub Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `themed-button` command with the hermetic environment every test wants.
fn themed_button(label: &str, classes: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_themed-button"));
    command
        .env("ICEDTEA_UI_THEME", TEST_THEME)
        .env("ICEDTEA_UI_CLASSES", classes)
        .env("ICEDTEA_UI_LABEL", label);
    command
}

/// The four numbers `themed-button --print-allocation` prints.
///
/// A local struct, not `icedtea_ui::layout::Allocation`: M2 reshaped that
/// type, and `tests/layer_shell_screencopy.rs` is a gated file whose only
/// permitted edit is the `use` line that names this one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintedAllocation {
    pub width: f32,
    pub height: f32,
    pub label_x: f32,
    pub label_y: f32,
}

/// The allocation the binary itself computes for `label`/`classes`.
///
/// Asking the binary rather than recomputing it here is what keeps the
/// screencopy test's sample coordinates honest: they are derived from the
/// same code path that sizes the layer surface, so a layout change moves the
/// samples instead of silently invalidating them.
///
/// # Panics
///
/// If the binary cannot be run, exits non-zero, or prints something other
/// than the four numbers `--print-allocation` documents.
#[must_use]
pub fn allocation_of(label: &str, classes: &str) -> PrintedAllocation {
    let output = themed_button(label, classes)
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run themed-button --print-allocation");
    assert!(
        output.status.success(),
        "themed-button --print-allocation exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocation is not UTF-8");
    let numbers: Vec<f32> = stdout
        .split_whitespace()
        .map(|field| {
            field
                .parse()
                .unwrap_or_else(|_| panic!("not a number in {stdout:?}"))
        })
        .collect();
    assert_eq!(
        numbers.len(),
        4,
        "expected `width height label_x label_y`, got {stdout:?}"
    );
    PrintedAllocation {
        width: numbers[0],
        height: numbers[1],
        label_x: numbers[2],
        label_y: numbers[3],
    }
}

/// Spawn `themed-button` against `socket`, reaped when the guard drops.
///
/// # Panics
///
/// If the binary cannot be spawned.
#[must_use]
pub fn spawn_themed_button(socket: &str, label: &str, classes: &str) -> Reaper {
    Reaper(
        themed_button(label, classes)
            .env("WAYLAND_DISPLAY", socket)
            .spawn()
            .expect("failed to spawn themed-button"),
    )
}

/// Spawn `themed-button` against `socket` with `theme` as its *whole* theme.
///
/// `ICEDTEA_UI_THEME` set to a path makes the file a complete theme rather
/// than an override, which is what a test declaring its own colours wants.
///
/// # Panics
///
/// If the binary cannot be spawned.
#[must_use]
pub fn spawn_themed_button_with_theme(
    socket: &str,
    theme: &std::path::Path,
    label: &str,
    classes: &str,
) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_themed-button"))
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_UI_LABEL", label)
            .env("ICEDTEA_UI_CLASSES", classes)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn themed-button"),
    )
}

/// The RGB of `(x, y)` in a captured frame, or `None` outside it.
#[must_use]
pub fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// How far apart two channel bytes may be and still count as the same
/// colour on a *screencopy* capture.
///
/// A compositor's output goes through a format conversion the offscreen
/// pixel gate does not, so an on-screen sample is never exact; the offscreen
/// gate keeps its own, tighter constant. This is a ceiling, not a comfortable
/// margin -- `layer_shell_screencopy.rs` documents at length why Adwaita's
/// accent and hover bands stop being distinguishable above it.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// Whether `a` and `b` are the same channel value within
/// [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= u32::from(SCREENCOPY_TOLERANCE)
}

/// Whether an RGB triple matches `expected` on every channel, within
/// [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn matches(px: (u8, u8, u8), expected: (u8, u8, u8)) -> bool {
    close(px.0, expected.0) && close(px.1, expected.1) && close(px.2, expected.2)
}

/// How long a capture loop sleeps between frames.
///
/// One 40 Hz-ish tick: short enough that the elapsed time a transition test
/// measures is dominated by the transition, long enough not to spin.
pub const CAPTURE_POLL: Duration = Duration::from_millis(25);

/// The background the probe's theme paints, and the colour every window
/// e2e samples for "the client is on screen here".
pub const PROBE_BG: (u8, u8, u8) = (0x33, 0x77, 0x22);
/// The entry's own background, distinct from the window's.
pub const PROBE_ENTRY_BG: (u8, u8, u8) = (0xEE, 0xEE, 0xEE);

/// A complete theme with flat, unmistakable colours.
///
/// Not Adwaita: these tests assert "the client painted here", and a gradient
/// with a 1px border is a bad probe for that.
#[must_use]
pub fn probe_theme() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("theme file");
    std::io::Write::write_all(
        &mut file,
        b"window { background-color: #337722; }
          box { background-color: #337722; min-width: 380px; min-height: 60px; }
          entry { background-color: #eeeeee; color: #101010; min-width: 200px; min-height: 34px; }
          entry:focus-visible { background-color: #ffcc00; }
          text { color: #101010; }
          menubutton { background-color: #cccccc; min-width: 100px; min-height: 34px; }
          label { color: #101010; }
        ",
    )
    .expect("write the theme");
    file
}

/// Spawn `window-probe` against `socket`, reaped when the guard drops.
#[must_use]
pub fn spawn_window_probe(
    socket: &str,
    mode: &str,
    theme: &std::path::Path,
    report: &std::path::Path,
) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_window-probe"))
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_PROBE_MODE", mode)
            .env("ICEDTEA_PROBE_REPORT", report)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn window-probe"),
    )
}

/// Every line the probe has reported so far.
#[must_use]
pub fn probe_report(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Poll the report until a line starting with `prefix` appears.
///
/// Polling a file, not a pipe: the probe is a separate process whose stdout
/// buffering is not ours to control, and a test that reads a pipe has to keep
/// draining it or deadlock the child.
#[must_use]
pub fn wait_for_report_line(
    path: &std::path::Path,
    prefix: &str,
    timeout: Duration,
) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(line) = probe_report(path)
            .into_iter()
            .find(|l| l.starts_with(prefix))
        {
            return Some(line);
        }
        std::thread::sleep(CAPTURE_POLL);
    }
    None
}

/// Capture until `ready` accepts the pixel at `sample`, pumping `pointer` so
/// the compositor keeps delivering, or until `timeout` passes.
///
/// Returns the last sampled pixel and how long the loop ran, so a caller can
/// assert on the elapsed time as well as the colour.
///
/// # Panics
///
/// If `sample` falls outside the captured frame.
pub fn capture_until(
    screencopy: &mut ScreencopyClient,
    mut pointer: Option<&mut VirtualPointerClient>,
    sample: (u32, u32),
    timeout: Duration,
    ready: impl Fn((u8, u8, u8)) -> bool,
) -> ((u8, u8, u8), Duration) {
    let sample_pixel = |frame: &CapturedFrame| {
        pixel_at(frame, sample.0, sample.1).expect("sample pixel inside the captured frame")
    };
    let started = Instant::now();
    let mut px = sample_pixel(&screencopy.capture());
    while !ready(px) && started.elapsed() < timeout {
        std::thread::sleep(CAPTURE_POLL);
        if let Some(pointer) = pointer.as_deref_mut() {
            pointer.pump();
        }
        px = sample_pixel(&screencopy.capture());
    }
    (px, started.elapsed())
}
