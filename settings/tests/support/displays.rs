//! Harness scaffolding for the Displays gates.
//!
//! Self-contained on purpose (plan P4-D1): the contract's module map names no
//! shared settings test-support module, so this one carries everything P4's two
//! test binaries need and shares nothing with any other part's.
//!
//! The shape is `ui/tests/support/mod.rs`'s `Driver`, with `icedtea-settings`
//! in place of the gallery: a harness compositor, one settings process against
//! an isolated `XDG_CONFIG_HOME`, a screencopy client and a virtual pointer,
//! and coordinates read out of `$ICEDTEA_PROBE_REPORT` rather than hard-coded.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// The theme every gate pins its colours against. Never the developer's own
/// `gtk.css`.
pub const TEST_THEME: &str = "bundled";

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture — the output goes through a format conversion an
/// offscreen sample does not. `ui/tests/support/mod.rs`'s ceiling, verbatim.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// How long an interaction gets to reach the model and the screen.
///
/// The same generous bound `ui/tests/interaction_gate.rs` documents at length:
/// one anti-aliased `clip_path` in `skia-rs-canvas` 0.4.0 rasterises its mask
/// over the whole canvas, so a single repaint costs seconds on a debug build.
/// A complexity bound, not a wall-clock pin.
pub const REACT: Duration = Duration::from_secs(60);

/// How long the settings process gets to map its first frame.
pub const BOOT: Duration = Duration::from_secs(60);

/// How long a polling loop sleeps between samples.
const POLL: Duration = Duration::from_millis(25);

/// One `probe <label> <x> <y>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// One `alloc <id> <x> <y> <w> <h>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alloc {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Whether two channel values are the same within [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= u32::from(SCREENCOPY_TOLERANCE)
}

/// Whether an RGB triple matches `expected` on every channel.
#[must_use]
pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

/// The RGB at `(x, y)` in a captured frame, or `None` outside it.
#[must_use]
pub fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// Whether any pixel of `rect` differs from `background`.
///
/// A full scan, not a 5x5 inset grid: the whole page is a few tens of
/// rectangles and one screencopy round trip dwarfs the scan.
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
        (x..x + w).any(|px| {
            px >= 0
                && py >= 0
                && pixel_at(frame, px as u32, py as u32)
                    .is_some_and(|got| !matches(got, background))
        })
    })
}

/// Kill the child however the test ends.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A harness compositor plus one `icedtea-settings` process, its probe report
/// and its injectors.
pub struct SettingsDriver {
    _compositor: Compositor,
    _child: Reaper,
    /// Kept alive: dropping it deletes the isolated config store and report.
    _home: tempfile::TempDir,
    report: PathBuf,
    screencopy: ScreencopyClient,
    pointer: VirtualPointerClient,
    output: (u32, u32),
    background: (u8, u8, u8),
    background_probe: (u32, u32),
}

impl SettingsDriver {
    /// Boot a compositor and open `icedtea-settings` on `page` under `theme`.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output, screencopy or virtual pointer,
    /// if the binary cannot be spawned, or if it never paints within [`BOOT`].
    #[must_use]
    pub fn open(theme: &str, page: &str) -> SettingsDriver {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let (w, h) = compositor.output_size();
        let mut screencopy = ScreencopyClient::spawn(&socket);
        // A column the settings window never covers: the far right edge.
        let background_probe = (w as u32 - 3, h as u32 / 2);
        let empty = screencopy.capture();
        let background = pixel_at(&empty, background_probe.0, background_probe.1)
            .expect("the background probe is inside the frame");

        let home = tempfile::tempdir().expect("temp XDG_CONFIG_HOME");
        let report = home.path().join("probe.txt");
        let child = Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
            .env("WAYLAND_DISPLAY", &socket)
            .env("XDG_RUNTIME_DIR", icedtea_harness::runtime_dir())
            .env("XDG_CONFIG_HOME", home.path())
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_SETTINGS_PAGE", page)
            .env("ICEDTEA_PROBE_REPORT", &report)
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn icedtea-settings");

        let pointer = VirtualPointerClient::spawn(&socket);
        let mut driver = SettingsDriver {
            _compositor: compositor,
            _child: Reaper(child),
            _home: home,
            report,
            screencopy,
            pointer,
            output: (w as u32, h as u32),
            background,
            background_probe,
        };
        driver.wait_for_first_frame();
        driver
    }

    /// Block until anything paints over the background.
    ///
    /// Reconciliation (Task 10): the brief's literal code samples the
    /// output's centre pixel, assuming the compositor places the toplevel
    /// there. It does not — `settings/tests/skeleton.rs` reads the window's
    /// actual frame geometry from `Compositor::snapshot` before translating
    /// any coordinate — so a 480x420 window placed away from centre (as this
    /// harness's placement policy does) would make the centre sample spin
    /// for the full [`BOOT`] budget and panic on a perfectly healthy paint.
    /// Scanning the whole frame on a 4px stride (`support/mod.rs`'s
    /// `paints_something_anywhere`, same budget) is placement-independent.
    fn wait_for_first_frame(&mut self) {
        let started = Instant::now();
        while started.elapsed() < BOOT {
            let frame = self.screencopy.capture();
            let painted = (0..frame.height).step_by(4).any(|y| {
                (0..frame.width)
                    .step_by(4)
                    .any(|x| pixel_at(&frame, x, y).is_some_and(|px| !matches(px, self.background)))
            });
            if painted {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!("icedtea-settings never painted within {BOOT:?}");
    }

    /// The output's empty background colour.
    #[must_use]
    pub fn background(&self) -> (u8, u8, u8) {
        self.background
    }

    /// Every line the settings process has reported so far.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Every `probe` line, most recent first — the report appends per frame, so
    /// a later line supersedes an earlier one with the same label.
    #[must_use]
    pub fn probe_points(&self) -> Vec<ProbePoint> {
        let mut out: Vec<ProbePoint> = Vec::new();
        for line in self.lines() {
            let mut f = line.split_whitespace();
            if f.next() != Some("probe") {
                continue;
            }
            let (Some(label), Some(x), Some(y)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x.parse(), y.parse()) else {
                continue;
            };
            let label = label.to_string();
            out.retain(|p| p.label != label);
            out.push(ProbePoint { label, x, y });
        }
        out
    }

    /// The most recent centre reported for `label`.
    ///
    /// # Panics
    ///
    /// If no probe line ever named `label`; the message lists what was there.
    #[must_use]
    pub fn point(&self, label: &str) -> (i32, i32) {
        let points = self.probe_points();
        points
            .iter()
            .find(|p| p.label == label)
            .map(|p| (p.x, p.y))
            .unwrap_or_else(|| {
                let labels: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
                panic!("no probe point {label:?}; got {labels:?}")
            })
    }

    /// The most recent allocation reported for `id`.
    ///
    /// # Panics
    ///
    /// If no `alloc` line ever named `id`.
    #[must_use]
    pub fn alloc(&self, id: &str) -> Alloc {
        let mut found = None;
        for line in self.lines() {
            let mut f = line.split_whitespace();
            if f.next() != Some("alloc") || f.next() != Some(id) {
                continue;
            }
            let nums: Vec<i32> = f
                .filter_map(|n| n.parse::<f32>().ok().map(|v| v as i32))
                .collect();
            if let [x, y, w, h] = nums[..] {
                found = Some(Alloc {
                    id: id.to_string(),
                    x,
                    y,
                    w,
                    h,
                });
            }
        }
        found.unwrap_or_else(|| panic!("no allocation line for {id:?}"))
    }

    /// The most recent `state <key> <value…>` value, if any.
    #[must_use]
    pub fn state(&self, key: &str) -> Option<String> {
        let mut found = None;
        for line in self.lines() {
            let Some(rest) = line.strip_prefix("state ") else {
                continue;
            };
            let Some(value) = rest.strip_prefix(key) else {
                continue;
            };
            let Some(value) = value.strip_prefix(' ') else {
                continue;
            };
            found = Some(value.to_string());
        }
        found
    }

    /// Poll until `key` reads `value`.
    #[must_use]
    pub fn wait_state(&self, key: &str, value: &str, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if self.state(key).as_deref() == Some(value) {
                return true;
            }
            std::thread::sleep(POLL);
        }
        false
    }

    /// Poll until `key` reads something other than `before`, and return it.
    #[must_use]
    pub fn wait_state_change(
        &self,
        key: &str,
        before: Option<String>,
        timeout: Duration,
    ) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            let now = self.state(key);
            if now != before && now.is_some() {
                return now;
            }
            std::thread::sleep(POLL);
        }
        None
    }

    /// One screencopy frame.
    pub fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }

    /// The RGB at `(x, y)` on the output.
    ///
    /// # Panics
    ///
    /// If the point is outside the captured frame.
    pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let frame = self.capture();
        pixel_at(&frame, x as u32, y as u32).expect("sample inside the frame")
    }

    /// Move the pointer and settle.
    ///
    /// The settle is `ui/tests/support/mod.rs`'s: a button sent back to back
    /// with the motion that first entered a surface can reach the seat before
    /// focus is assigned, and `wlr_seat_pointer_notify_button` drops a button
    /// with no focused surface silently.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(POLL);
            self.pointer.pump();
        }
    }

    /// Press the left button at `(x, y)`.
    pub fn press(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Release the left button at `(x, y)`.
    pub fn release(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release at `(x, y)`.
    pub fn click(&mut self, x: i32, y: i32) {
        self.press(x, y);
        self.release(x, y);
    }

    /// Press at `from`, move through the midpoint, release at `to`.
    ///
    /// The intermediate motion is what makes it a drag; a teleport looks like a
    /// click somewhere else.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.press(from.0, from.1);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release(to.0, to.1);
    }
}
