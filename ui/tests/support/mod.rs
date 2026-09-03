//! Shared scaffolding for `icedtea-ui`'s integration tests.
//!
//! Not every test uses every helper, hence the blanket `dead_code` allow:
//! this module is compiled once per test binary that declares it.

#![allow(dead_code)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{
    CapturedFrame, Compositor, ScreencopyClient, VirtualKeyboardClient, VirtualPointerClient,
};

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

// ---------------------------------------------------------------------------
// M3: the gallery
// ---------------------------------------------------------------------------

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};

/// One `<widget> <label> <x> <y>` line from `gallery --probe-points`.
///
/// A local struct with owned fields, not `icedtea_ui::gallery::ProbePoint`:
/// what comes back over a pipe is text, and the library type's `widget` is a
/// `&'static str` that no parse can produce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub widget: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// One `<widget> <x> <y> <width> <height>` line from
/// `gallery --print-allocation`: an entry's border box in page coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryAllocation {
    pub widget: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Parse one probe-point line. `None` for anything malformed — a child that
/// crashed mid-line must fail an assertion, never panic the test harness.
#[must_use]
pub fn parse_probe_line(line: &str) -> Option<ProbePoint> {
    let mut fields = line.split_whitespace();
    let widget = fields.next()?.to_string();
    let label = fields.next()?.to_string();
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(ProbePoint {
        widget,
        label,
        x,
        y,
    })
}

/// Parse one allocation line, with the same total-or-`None` rule.
#[must_use]
pub fn parse_allocation_line(line: &str) -> Option<EntryAllocation> {
    let mut fields = line.split_whitespace();
    let widget = fields.next()?.to_string();
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    let width = fields.next()?.parse().ok()?;
    let height = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(EntryAllocation {
        widget,
        x,
        y,
        width,
        height,
    })
}

/// A `gallery` command with the hermetic environment every test wants.
fn gallery(theme: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gallery"));
    command.arg("--theme").arg(theme);
    command
}

/// Run `gallery --probe-points` (headless) and parse every line.
///
/// # Panics
///
/// If the binary cannot be run, exits non-zero, or prints a line the parser
/// rejects — all three mean the gate cannot know where to sample.
#[must_use]
pub fn probe_points(theme: &str, widget: Option<&str>) -> Vec<ProbePoint> {
    let mut command = gallery(theme);
    if let Some(widget) = widget {
        command.arg("--widget").arg(widget);
    }
    let output = command
        .arg("--probe-points")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --probe-points");
    assert!(
        output.status.success(),
        "gallery --probe-points exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("probe points are not UTF-8");
    stdout
        .lines()
        .map(|line| parse_probe_line(line).unwrap_or_else(|| panic!("bad probe line {line:?}")))
        .collect()
}

/// Run `gallery --print-allocation` (headless) and parse every line.
///
/// # Panics
///
/// As [`probe_points`].
#[must_use]
pub fn entry_allocations(theme: &str) -> Vec<EntryAllocation> {
    let output = gallery(theme)
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --print-allocation");
    assert!(
        output.status.success(),
        "gallery --print-allocation exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocations are not UTF-8");
    stdout
        .lines()
        .map(|line| {
            parse_allocation_line(line).unwrap_or_else(|| panic!("bad allocation line {line:?}"))
        })
        .collect()
}

/// Two captured spans, compared pixel for pixel with [`matches`]'s tolerance.
#[must_use]
pub fn row_matches(a: &[(u8, u8, u8)], b: &[(u8, u8, u8)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| matches(*x, *y))
}

/// `entry_allocations`, but for one widget and with `--size` pinned to `size`.
///
/// The interaction gate needs a widget's real border box, not only its probe
/// centres: a click that must land *past* the end of an entry's text (where
/// GTK puts the caret at the end of the buffer) is a fraction of a box, not
/// the centre of a subnode, and deriving it from the box the binary reports
/// keeps it correct when an intrinsic size moves. See
/// [`spawn_gallery_widget_sized`] for why `--size` must be pinned on every
/// query.
///
/// # Panics
///
/// As [`probe_points`], plus if `widget` prints no allocation at all.
#[must_use]
pub fn allocation_sized(theme: &str, widget: &str, size: (u32, u32)) -> EntryAllocation {
    let output = gallery(theme)
        .arg("--widget")
        .arg(widget)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --print-allocation");
    assert!(
        output.status.success(),
        "gallery --print-allocation exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocations are not UTF-8");
    stdout
        .lines()
        .filter_map(parse_allocation_line)
        .find(|alloc| alloc.widget == widget)
        .unwrap_or_else(|| panic!("gallery --widget {widget} printed no allocation"))
}

/// `allocation_sized`, but with the gallery's `--open` flag set: the border
/// box a disclosure widget has once it is showing its body.
///
/// The counterpart to [`probe_points_open`] for a band that has to span a
/// *box* rather than sit on a subnode's centre. A collapsed `Expander` gives
/// its `content` no space and a closed `MenuButton` hides its popover, so the
/// closed box says nothing about where the disclosed body lands; the test
/// still drives the real disclosure by clicking.
///
/// # Panics
///
/// As [`allocation_sized`].
#[must_use]
pub fn allocation_open(theme: &str, widget: &str, size: (u32, u32)) -> EntryAllocation {
    let output = gallery(theme)
        .arg("--widget")
        .arg(widget)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--open")
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --print-allocation --open");
    assert!(
        output.status.success(),
        "gallery --print-allocation --open exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocations are not UTF-8");
    stdout
        .lines()
        .filter_map(parse_allocation_line)
        .find(|alloc| alloc.widget == widget)
        .unwrap_or_else(|| panic!("gallery --open --widget {widget} printed no allocation"))
}

/// A running `gallery`, killed on drop, with its stdout captured.
///
/// The captured stdout is what makes §7's "screencopy **or model** assertions"
/// possible: the app prints one flushed `msg <line>` per folded message, so
/// the interaction gate can assert on the model without sharing memory with
/// it.
pub struct GalleryProc {
    child: Child,
    messages: Arc<Mutex<Vec<String>>>,
}

impl GalleryProc {
    /// Every `msg` line seen so far, in order, without the `msg ` prefix.
    ///
    /// # Panics
    ///
    /// If the reader thread poisoned the lock, which only a panic there can do.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().expect("message log").clone()
    }

    /// Wait until some message line equals `needle`, or `timeout` passes.
    #[must_use]
    pub fn wait_msg(&self, needle: &str, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.messages().iter().any(|line| line == needle) {
                return true;
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(CAPTURE_POLL);
        }
    }
}

impl Drop for GalleryProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `command` against `socket` with its stdout drained by a reader
/// thread, so a full pipe can never block the child.
fn spawn_gallery_command(mut command: Command, socket: &str) -> GalleryProc {
    let mut child = command
        .env("WAYLAND_DISPLAY", socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn gallery");
    let stdout = child.stdout.take().expect("piped stdout");
    let messages = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&messages);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(msg) = line.strip_prefix("msg ") {
                sink.lock().expect("message log").push(msg.to_string());
            }
        }
    });
    GalleryProc { child, messages }
}

/// The whole page, scrolled to `scroll`.
#[must_use]
pub fn spawn_gallery(socket: &str, theme: &str, scroll: i32) -> GalleryProc {
    let mut command = gallery(theme);
    command.arg("--scroll").arg(scroll.to_string());
    spawn_gallery_command(command, socket)
}

/// One widget, alone, at the origin.
#[must_use]
pub fn spawn_gallery_widget(socket: &str, theme: &str, widget: &str) -> GalleryProc {
    let mut command = gallery(theme);
    command.arg("--widget").arg(widget);
    spawn_gallery_command(command, socket)
}

/// `spawn_gallery_widget`, but with `--size` pinned to `size` instead of
/// [`Options::DEFAULT_SIZE`].
///
/// Reconciliation: [`Driver`] needs this and [`probe_points_sized`] below,
/// which [`spawn_gallery_widget`]/[`probe_points`] cannot be repurposed into
/// without changing a signature every other test file in this crate already
/// calls positionally. A layer surface requesting the default 1280x800 is
/// silently clamped by the compositor to fit its real output when that
/// output is shorter -- the harness's own is 1280x720 -- so the *live*
/// gallery relayouts to the clamped size while a headless `--probe-points`
/// run with no `--size` still measures the unclamped 1280x800 layout. A
/// widget centred on the window's full height then lands at a different
/// point in each (e.g. y=400 in the assumed 800px height versus y=360 in the
/// real 720px one, well outside a 34px-tall button), so every click coded
/// against the mismatched probe coordinate silently misses the widget.
/// Pinning `--size` to the compositor's own `output_size()` on both the live
/// process and every probe query keeps them laying out identically.
#[must_use]
pub fn spawn_gallery_widget_sized(
    socket: &str,
    theme: &str,
    widget: &str,
    size: (u32, u32),
) -> GalleryProc {
    let mut command = gallery(theme);
    command
        .arg("--widget")
        .arg(widget)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1));
    spawn_gallery_command(command, socket)
}

/// `probe_points`, but with `--size` pinned to `size`. See
/// [`spawn_gallery_widget_sized`] for why.
///
/// # Panics
///
/// As [`probe_points`].
#[must_use]
pub fn probe_points_sized(theme: &str, widget: &str, size: (u32, u32)) -> Vec<ProbePoint> {
    let output = gallery(theme)
        .arg("--widget")
        .arg(widget)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--probe-points")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --probe-points");
    assert!(
        output.status.success(),
        "gallery --probe-points exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("probe points are not UTF-8");
    stdout
        .lines()
        .map(|line| parse_probe_line(line).unwrap_or_else(|| panic!("bad probe line {line:?}")))
        .collect()
}

/// `probe_points_sized`, but with the gallery's `--open` flag set: every
/// popover-bearing sample is built with its popover already shown.
///
/// A `DropDown`'s popover is hidden while closed — laid out at zero size — so
/// a closed-state probe reports its rows collapsed onto one point and cannot
/// say where any of them lands once a click opens it. This asks the same
/// binary for the *open* tree's coordinates; the test still drives the real
/// open by clicking the button.
///
/// # Panics
///
/// As [`probe_points`].
#[must_use]
pub fn probe_points_open(theme: &str, widget: &str, size: (u32, u32)) -> Vec<ProbePoint> {
    let output = gallery(theme)
        .arg("--widget")
        .arg(widget)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--open")
        .arg("--probe-points")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --probe-points --open");
    assert!(
        output.status.success(),
        "gallery --probe-points --open exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("probe points are not UTF-8");
    stdout
        .lines()
        .map(|line| parse_probe_line(line).unwrap_or_else(|| panic!("bad probe line {line:?}")))
        .collect()
}

/// How long a gallery gets to map and paint its first frame.
///
/// A generous complexity bound, not a wall-clock pin: it covers compiling a
/// ~1,900-line Adwaita sheet, matching fonts and painting 64 widgets on a
/// loaded CI box.
///
/// Reconciliation: the task text puts this at 20s. The gallery's first frame
/// takes about two minutes, and the reason is now measured rather than
/// guessed. Instrumenting `paint_node_with_children` (temporarily; the
/// instrumentation is not committed) over a live first paint gives ~275ms per
/// node in `paint_box_shadows` and ~195ms in `paint_backgrounds`. Drilling
/// into `paint_outset` puts effectively all of that in the single
/// `canvas.clip_path(.., ClipOp::Difference, true)` call, at **~4 seconds for
/// one call**; `paint_backgrounds`' anti-aliased `clip_path(.., Intersect,
/// true)` is the same cost in smaller form. `skia-rs-canvas` 0.4.0's
/// `ClipStack::clip_path` builds `ClipMask::from_path_aa(path,
/// device_bounds)` with `device_bounds = IRect::new(0, 0, canvas.width,
/// canvas.height)` — the mask is rasterised over the *whole canvas* however
/// small the path is — so the cost is O(surface area) per anti-aliased clip
/// and no call-site change short of dropping anti-aliasing (which would
/// change every pinned pixel in `themed_button_offscreen.rs` and
/// `widget_pixels.rs`) can avoid it. Setting `opt-level = 2` for
/// `icedtea-ui` and `skia-rs-safe` in the dev profile changed nothing,
/// confirming it is algorithmic and not codegen.
///
/// So this is a real, reproducible cost and not a hang — every widget does
/// eventually paint — and it belongs to a paint-pipeline task (or an upstream
/// fix bounding that mask by the path), not to wiring `gallery::run`. The
/// bound is set wide enough for a cold page on a machine that is also running
/// the rest of `cargo test`, rather than narrowed to hide it.
pub const GALLERY_MAP_TIMEOUT: Duration = Duration::from_secs(480);

/// Capture until `probe` stops showing `before` — the gallery's first frame.
///
/// # Panics
///
/// If nothing changes within [`GALLERY_MAP_TIMEOUT`], which means the gallery
/// never mapped.
pub fn wait_for_gallery(
    screencopy: &mut ScreencopyClient,
    probe: (u32, u32),
    before: (u8, u8, u8),
) -> CapturedFrame {
    let started = Instant::now();
    loop {
        let frame = screencopy.capture();
        let px = pixel_at(&frame, probe.0, probe.1).expect("the probe is inside the frame");
        if !matches(px, before) {
            return frame;
        }
        assert!(
            started.elapsed() < GALLERY_MAP_TIMEOUT,
            "the gallery never painted: ({}, {}) is still {before:?}",
            probe.0,
            probe.1
        );
        std::thread::sleep(CAPTURE_POLL);
    }
}

/// Whether anything inside `rect` differs from `background`.
///
/// Reconciliation: the task text samples a 5x5 grid inset one pixel from the
/// border box, "dense enough to catch a widget that drew only a border". It is
/// not: a 1px border is caught only when a sample column or row happens to
/// land on it, which depends on `(w - 2) * col / 4` hitting `0` or `w - 3`.
/// Measured against the real page, `action_bar` (124x47, 460 non-background
/// pixels — a child button's border and its flat bar edge) is a false
/// negative, while `toggle_button` (36x34, 140 non-background pixels of
/// exactly the same kind) passes purely because its grid lands on the bottom
/// and right border rows. A gate that reports "painted nothing" for something
/// that plainly painted is worse than a slower one, so this scans every pixel
/// of the border box instead. The whole page is ~59 rectangles totalling
/// ~130k pixels per capture, which is noise next to one screencopy round trip.
///
/// The `w <= 2 || h <= 2` early return of the task text goes with it, for the
/// same reason: it was there only because a 5x5 inset grid has nothing to
/// sample in a 2px box. A full scan does, and `separator` (1x1, one
/// `#d8d4d0` pixel) does paint (`calendar` used to be the other 2px case,
/// before `CalendarC::measure` gave it a real month-grid allocation).
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
            pixel_at(frame, px as u32, py as u32).is_some_and(|got| !matches(got, background))
        })
    })
}

// ---------------------------------------------------------------------------
// M3: the interaction driver
// ---------------------------------------------------------------------------

/// Linux input-event keycodes the interaction gate sends.
pub const KEY_ESC: u32 = 1;
pub const KEY_A: u32 = 30;
pub const KEY_H: u32 = 35;
pub const KEY_I: u32 = 23;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_SPACE: u32 = 57;
pub const KEY_UP: u32 = 103;
pub const KEY_DOWN: u32 = 108;

/// One compositor, one gallery, one pointer and one keyboard: everything an
/// interaction test needs, with the coordinates read from the binary.
pub struct Driver {
    compositor: Compositor,
    socket: String,
    screencopy: ScreencopyClient,
    pointer: VirtualPointerClient,
    keyboard: VirtualKeyboardClient,
    output: (u32, u32),
    theme: String,
    widget: String,
    background: (u8, u8, u8),
    background_probe: (u32, u32),
}

impl Driver {
    /// Boot a compositor and its injectors.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output, screencopy or injector.
    #[must_use]
    pub fn new() -> Driver {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let (w, h) = compositor.output_size();
        let mut screencopy = ScreencopyClient::spawn(&socket);
        let background_probe = (w as u32 - 3, h as u32 / 2);
        let empty = screencopy.capture();
        let background = pixel_at(&empty, background_probe.0, background_probe.1)
            .expect("the background probe is inside the frame");
        let pointer = VirtualPointerClient::spawn(&socket);
        let keyboard = VirtualKeyboardClient::spawn(&socket);
        Driver {
            compositor,
            socket,
            screencopy,
            pointer,
            keyboard,
            output: (w as u32, h as u32),
            theme: String::new(),
            widget: String::new(),
            background,
            background_probe,
        }
    }

    /// The compositor's socket name.
    #[must_use]
    pub fn socket(&self) -> String {
        self.socket.clone()
    }

    /// Open one widget, alone, and wait for its first frame.
    ///
    /// # Panics
    ///
    /// If the gallery never paints.
    pub fn open(&mut self, theme: &str, widget: &str) -> GalleryProc {
        self.theme = theme.to_string();
        self.widget = widget.to_string();
        let gallery = spawn_gallery_widget_sized(&self.socket, theme, widget, self.output);
        let _ = wait_for_gallery(&mut self.screencopy, self.background_probe, self.background);
        gallery
    }

    /// The output-space centre of `widget`'s `label` subnode.
    ///
    /// # Panics
    ///
    /// If the widget exposes no such probe point — the message lists the ones
    /// it does expose, which is what makes a renamed subnode a readable
    /// failure rather than a mystery.
    #[must_use]
    pub fn point(&self, widget: &str, label: &str) -> (i32, i32) {
        let points = probe_points_sized(&self.theme, widget, self.output);
        points
            .iter()
            .find(|p| p.label == label)
            .map(|p| (p.x, p.y))
            .unwrap_or_else(|| {
                let have: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
                panic!("{widget} has no {label:?} probe point; it has {have:?}")
            })
    }

    /// The output-space centre of `widget`'s `label` subnode, as it will be
    /// once every popover in `widget` is open — see [`probe_points_open`].
    /// The counterpart to [`Driver::point`] for a target that only has real
    /// geometry while something is open.
    ///
    /// # Panics
    ///
    /// As [`Driver::point`].
    #[must_use]
    pub fn point_open(&self, widget: &str, label: &str) -> (i32, i32) {
        let points = probe_points_open(&self.theme, widget, self.output);
        points
            .iter()
            .find(|p| p.label == label)
            .map(|p| (p.x, p.y))
            .unwrap_or_else(|| {
                let have: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
                panic!("open {widget} has no {label:?} probe point; it has {have:?}")
            })
    }

    /// `widget`'s own border box, in output coordinates.
    ///
    /// The counterpart to [`Driver::point`] for a click that has to be placed
    /// *relative to* a box rather than at a subnode's centre.
    #[must_use]
    pub fn allocation(&self, widget: &str) -> EntryAllocation {
        allocation_sized(&self.theme, widget, self.output)
    }

    /// `widget`'s border box as it will be once its body is disclosed — see
    /// [`allocation_open`]. The counterpart to [`Driver::point_open`] for a
    /// band that has to span a box.
    #[must_use]
    pub fn allocation_open(&self, widget: &str) -> EntryAllocation {
        allocation_open(&self.theme, widget, self.output)
    }

    /// Move the pointer, in output coordinates.
    ///
    /// Reconciliation: the compositor's own assignment of pointer focus to
    /// the surface under the cursor is not synchronous with the
    /// `motion_absolute` request that triggers it -- `window_events.rs`
    /// documents the analogous "spawn the pointer before the client connects"
    /// race for `wl_pointer` object capability, and this is the same race one
    /// level up, for *surface* focus: a `button()` sent immediately after a
    /// `motion_absolute` that has just moved the pointer onto a surface for
    /// the first time can reach the seat before it has finished assigning
    /// focus, and `wlr_seat_pointer_notify_button` drops a button with no
    /// focused surface silently rather than erroring. A short settle with a
    /// few extra roundtrips closes that window; confirmed against a debug
    /// build of the gallery binary that a `button()` sent back-to-back with
    /// `motion_absolute` (no settle) is dropped, while the same click after
    /// this settle reaches `ButtonC::on_event`.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(25));
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

    /// Press at `from`, move through the midpoint, release at `to` — the
    /// motion in the middle is what a drag needs; a teleport looks like a
    /// click somewhere else.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.press(from.0, from.1);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release(to.0, to.1);
    }

    /// Scroll at `(x, y)`; positive `vertical` scrolls down.
    pub fn scroll(&mut self, x: i32, y: i32, vertical: f64) {
        self.move_to(x, y);
        self.pointer.axis(0.0, vertical);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release one key.
    pub fn key(&mut self, keycode: u32) {
        self.keyboard.key_press(keycode);
        self.keyboard.pump();
    }

    /// Press and release each key in order.
    pub fn keys(&mut self, keycodes: &[u32]) {
        for &code in keycodes {
            self.key(code);
        }
    }

    /// The current colour at `(x, y)`.
    ///
    /// # Panics
    ///
    /// If the point is outside the output.
    pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let frame = self.screencopy.capture();
        pixel_at(&frame, x as u32, y as u32)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the {:?} output", self.output))
    }

    /// Every colour along row `y` from `x0` to `x1` inclusive, from one
    /// capture.
    ///
    /// A single pixel is the wrong probe for "did this text repaint": a glyph
    /// row is mostly background between the stems, so which exact pixel a
    /// probe centre lands on is an accident of the current metrics. A whole
    /// band across the widget changes if *anything* in it did, and stays
    /// meaningful when a glyph moves by a pixel.
    ///
    /// # Panics
    ///
    /// If any point in the span is outside the output.
    pub fn row(&mut self, x0: i32, x1: i32, y: i32) -> Vec<(u8, u8, u8)> {
        let frame = self.screencopy.capture();
        (x0..=x1)
            .map(|x| {
                pixel_at(&frame, x as u32, y as u32)
                    .unwrap_or_else(|| panic!("({x}, {y}) is outside the {:?} output", self.output))
            })
            .collect()
    }

    /// Capture until row `y` over `x0..=x1` stops matching `before`, or
    /// [`REACT_TIMEOUT`] passes; returns whatever it ended on, so the caller
    /// writes the assertion.
    pub fn wait_row_change(
        &mut self,
        x0: i32,
        x1: i32,
        y: i32,
        before: &[(u8, u8, u8)],
    ) -> Vec<(u8, u8, u8)> {
        let started = Instant::now();
        loop {
            let row = self.row(x0, x1, y);
            let same = row.len() == before.len()
                && row.iter().zip(before).all(|(now, was)| matches(*now, *was));
            if !same || started.elapsed() >= REACT_TIMEOUT {
                return row;
            }
            self.pointer.pump();
            std::thread::sleep(CAPTURE_POLL);
        }
    }

    /// Capture until `(x, y)` stops being `before`, or [`REACT_TIMEOUT`]
    /// passes; returns whatever it ended on so the caller writes the
    /// assertion.
    pub fn wait_pixel_change(&mut self, x: i32, y: i32, before: (u8, u8, u8)) -> (u8, u8, u8) {
        let started = Instant::now();
        loop {
            let px = self.pixel(x, y);
            if !matches(px, before) || started.elapsed() >= REACT_TIMEOUT {
                return px;
            }
            self.pointer.pump();
            std::thread::sleep(CAPTURE_POLL);
        }
    }

    /// Capture until `(x, y)` holds the same colour across several
    /// consecutive polls — "the animation is over", without pinning how long
    /// it took.
    ///
    /// Two agreeing polls are not enough: both can land before the repaint
    /// they are waiting on has even started, so "unchanged" would just be the
    /// pre-animation colour reported twice. Settlement therefore needs
    /// [`SETTLED_SAMPLES`] agreeing reads *and* [`SETTLED_MIN`] of wall clock
    /// to have passed, which puts the window past the compositor round trip
    /// that starts the repaint.
    pub fn wait_pixel_settled(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let started = Instant::now();
        let mut last = self.pixel(x, y);
        let mut agreeing = 1usize;
        loop {
            std::thread::sleep(CAPTURE_POLL);
            let now = self.pixel(x, y);
            agreeing = if matches(now, last) { agreeing + 1 } else { 1 };
            last = now;
            let settled = agreeing >= SETTLED_SAMPLES && started.elapsed() >= SETTLED_MIN;
            if settled || started.elapsed() >= REACT_TIMEOUT {
                return now;
            }
        }
    }
}

/// The ceiling every `wait_*` here shares: a generous complexity bound on one
/// round trip through the compositor, the app loop, a restyle and a repaint.
///
/// Reconciliation: see `interaction_gate.rs`'s `REACT` for why the task
/// text's 5s is not enough -- the same `clip_path` cost `GALLERY_MAP_TIMEOUT`
/// documents applies to every repaint, not only the first.
pub const REACT_TIMEOUT: Duration = Duration::from_secs(60);

/// Consecutive agreeing captures [`Driver::wait_pixel_settled`] needs before
/// it calls a pixel settled.
pub const SETTLED_SAMPLES: usize = 3;

/// Floor on how long [`Driver::wait_pixel_settled`] polls before it will
/// accept settlement at all, so it cannot report the pre-animation colour.
pub const SETTLED_MIN: Duration = Duration::from_millis(100);
