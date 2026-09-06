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

/// `settings/src/main.rs`'s own `APP_ID`, duplicated here rather than shared
/// (this module is self-contained by design, see the module docs above).
const SETTINGS_APP_ID: &str = "org.icedtea.Settings";

/// The server-side title bar height the harness compositor draws above every
/// window's own content, in output pixels.
///
/// `settings/tests/support/mod.rs`'s own `TITLE_BAR_HEIGHT`, duplicated for
/// the same self-containment reason. This driver's `alloc`/`point` are the
/// app's own **window-local** layout coordinates (what the probe report
/// carries); a screencopy capture is in **output** coordinates, and the two
/// differ by exactly the window's placement (`geometry.x`, `geometry.y +
/// TITLE_BAR_HEIGHT` — confirmed live: a probe-reported y of 45 painted at
/// output y 73, a 28px gap that disappears entirely once this offset is
/// added). Every coordinate this driver hands back from `alloc`/`point` is
/// pre-translated to output space so callers never have to know this.
const TITLE_BAR_HEIGHT: i32 = 28;

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

/// One `alloc <id> <x> <y> <w> <h>` line, already translated from the report's
/// window-local coordinates to output coordinates ([`SettingsDriver::alloc`]
/// adds the window's [`origin`](SettingsDriver::origin)) — ready to feed
/// straight to a screencopy pixel lookup.
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

/// The bundled sheet content a `theme` name (`light`/`dark`/`hc`) selects.
///
/// Reconciliation (Task 11): `open`'s `theme` parameter used to reach
/// `$ICEDTEA_UI_THEME` unmodified, but `settings/src/main.rs`'s `sheet()`
/// treats that variable as a *path* to a complete theme file (never a
/// keyword) — so "light"/"dark"/"hc" each failed to read as a file and fell
/// back to the same bundled light sheet, making the three rest-state gates
/// indistinguishable. This mirrors `settings/tests/support/mod.rs`'s own
/// `bundled_theme_content`: resolve the keyword to bundled CSS and write it
/// to a file inside the isolated `XDG_CONFIG_HOME`.
fn bundled_theme_content(theme: &str) -> &'static str {
    match theme {
        "dark" => icedtea_ui::BUNDLED_ADWAITA_DARK,
        "hc" => icedtea_ui::BUNDLED_ADWAITA_HC,
        _ => icedtea_ui::BUNDLED_ADWAITA_LIGHT,
    }
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
    /// The desktop's own colour, sampled *before* the window ever opened —
    /// used only to detect the settings process's first paint
    /// ([`wait_for_first_frame`](Self::wait_for_first_frame)). Never a stand-in
    /// for the *window's* background: the window does not cover the whole
    /// output (`output_size` is bigger than the fixed-content window), so a
    /// point outside it is desktop wallpaper, not page chrome — see
    /// [`background`](Self::background) for the one gates actually compare
    /// widget pixels against.
    boot_background: (u8, u8, u8),
    /// The id of the page's own root box (`page_from_env`/every page module's
    /// convention: `.id("displays")`, `.id("appearance")`, … match the page
    /// name exactly), used to locate a guaranteed-blank point for
    /// [`background`](Self::background).
    page_root: String,
    /// The window's own top-left corner in output coordinates — its
    /// compositor-assigned geometry, plus [`TITLE_BAR_HEIGHT`] for the
    /// decoration drawn above the client area. Added to every window-local
    /// coordinate `alloc`/`point` hands back before it reaches a caller.
    origin: (i32, i32),
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
        Self::open_on_bus(theme, page, None)
    }

    /// [`SettingsDriver::open`], with the spawned process's session-bus
    /// address overridden to `bus_address` when given.
    ///
    /// `icedtea_settings::compositor_reload::ReloadClient::new` (used by the
    /// production reload worker, `ipc::reload::spawn_worker_on_bus(.., None)`)
    /// resolves `$DBUS_SESSION_BUS_ADDRESS` through
    /// `zbus::blocking::Connection::session()` — that env var is the *only*
    /// knob it reads, so pointing the child's own copy of it at an isolated
    /// bus (never the developer's live session bus) is what lets a gate
    /// observe the real binary's `ReloadConfig` call land on a test-owned
    /// mock instead of whatever really owns `org.icedtea.Compositor` on the
    /// developer's desktop.
    #[must_use]
    pub fn open_on_bus(theme: &str, page: &str, bus_address: Option<&str>) -> SettingsDriver {
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
        // Desktop wallpaper, only ever used to notice the first paint.
        let empty = screencopy.capture();
        let boot_background = pixel_at(&empty, w as u32 - 3, h as u32 / 2)
            .expect("the background probe is inside the frame");

        let home = tempfile::tempdir().expect("temp XDG_CONFIG_HOME");
        let report = home.path().join("probe.txt");
        let theme_path = home.path().join("theme.css");
        std::fs::write(&theme_path, bundled_theme_content(theme)).expect("write the theme file");
        let mut command = Command::new(env!("CARGO_BIN_EXE_icedtea-settings"));
        command
            .env("WAYLAND_DISPLAY", &socket)
            .env("XDG_RUNTIME_DIR", icedtea_harness::runtime_dir())
            .env("XDG_CONFIG_HOME", home.path())
            .env("ICEDTEA_UI_THEME", &theme_path)
            .env("ICEDTEA_SETTINGS_PAGE", page)
            .env("ICEDTEA_PROBE_REPORT", &report)
            .stderr(Stdio::null());
        if let Some(address) = bus_address {
            command.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
        let child = command.spawn().expect("failed to spawn icedtea-settings");

        let pointer = VirtualPointerClient::spawn(&socket);
        let mut driver = SettingsDriver {
            _compositor: compositor,
            _child: Reaper(child),
            _home: home,
            report,
            screencopy,
            pointer,
            output: (w as u32, h as u32),
            boot_background,
            page_root: page.to_string(),
            origin: (0, 0),
        };
        driver.wait_for_first_frame();
        driver.origin = driver.wait_for_window_origin();
        driver
    }

    /// Poll the compositor's own model for this settings window's placement.
    ///
    /// # Panics
    ///
    /// If the settings window never enters the compositor's model within
    /// [`BOOT`].
    fn wait_for_window_origin(&self) -> (i32, i32) {
        let started = Instant::now();
        while started.elapsed() < BOOT {
            if let Some(window) = self
                ._compositor
                .snapshot()
                .windows
                .into_iter()
                .find(|w| w.app_id == SETTINGS_APP_ID)
            {
                let g = window.geometry;
                return (g.x, g.y + TITLE_BAR_HEIGHT);
            }
            std::thread::sleep(POLL);
        }
        panic!("the settings window never entered the compositor's model within {BOOT:?}");
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
                (0..frame.width).step_by(4).any(|x| {
                    pixel_at(&frame, x, y).is_some_and(|px| !matches(px, self.boot_background))
                })
            });
            if painted {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!("icedtea-settings never painted within {BOOT:?}");
    }

    /// The page's own background colour — sampled from inside the *window*,
    /// never the desktop.
    ///
    /// Reconciliation (Task 11): the driver's own [`boot_background`]
    /// (the far right edge of the *output*) is desktop wallpaper — `output_size`
    /// is bigger than the window, which the harness places at the output's
    /// origin rather than filling it (confirmed live: a 640x400 window on a
    /// larger output). Comparing a widget's pixels against wallpaper is no
    /// gate at all: the window itself already differs from the wallpaper
    /// everywhere, so a widget that painted nothing would still read as
    /// "painted something" by inheriting the plain page background.
    ///
    /// This instead samples a point just *below* the page's own root box, in
    /// the window's own remaining space beneath it — the app footer's chrome,
    /// not the page's `.margin(12, 12, 12, 12)` band itself (`root.h` already
    /// includes that margin, so `root.y + root.h` is the margin's *outer*
    /// edge, not a point inside it). A point just *above* the root box lands
    /// in the nav switcher's own chrome instead (confirmed live: a beige/tan
    /// sample, not the plain page background every widget rect is measured
    /// against), so only the space below is used.
    ///
    /// [`boot_background`]: Self::boot_background
    ///
    /// # Panics
    ///
    /// If the page's root id never reported an allocation, or the sampled
    /// point falls outside the captured frame.
    pub fn background(&mut self) -> (u8, u8, u8) {
        let root = self.alloc(&self.page_root.clone());
        let frame = self.capture();
        let x = (root.x + 10).max(0) as u32;
        let y = (root.y + root.h + 10).max(0) as u32;
        pixel_at(&frame, x, y).expect("a point below the page's own root box is inside the frame")
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

    /// The most recent centre reported for `label`, translated to output
    /// coordinates (see [`origin`](Self::origin)).
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
            .map(|p| (p.x + self.origin.0, p.y + self.origin.1))
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
                    x: x + self.origin.0,
                    y: y + self.origin.1,
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

    /// The most recent `frame <n>` marker — `ui/src/view/app.rs`'s
    /// `write_probe_report` writes one before each batch of `probe`/`alloc`
    /// lines that differ from the last paint, so this counts distinct
    /// painted states, not folds. `0` before the first one.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.lines()
            .iter()
            .rev()
            .find_map(|line| line.strip_prefix("frame ")?.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Poll until [`frame_count`](Self::frame_count) is past `after`.
    #[must_use]
    pub fn wait_for_frame_after(&self, after: u64, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if self.frame_count() > after {
                return true;
            }
            std::thread::sleep(POLL);
        }
        false
    }

    /// Whether a `msg <prefix>` line has been reported — `update`'s own
    /// `crate::probe::report(&format!("msg {msg:?}"))`, once per fold.
    ///
    /// A click on an insensitive widget (`Prop::sensitive(false)`) never
    /// reaches `update` at all, so this is what tells a gate that a retried
    /// click landed as the real action rather than on a still-disabled
    /// button — as opposed to polling geometry, which does not change with
    /// sensitivity. `prefix` matches on a word boundary (`Apply` does not
    /// also match `Applied { .. }`).
    #[must_use]
    pub fn has_message(&self, prefix: &str) -> bool {
        let needle = format!("msg {prefix}");
        self.lines().iter().any(|line| {
            line.strip_prefix(needle.as_str())
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
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
