//! Shared scaffolding for `icedtea-shell`'s integration tests: one compositor,
//! one real panel on its own thread, recording mocks behind the two command
//! traits, and a pointer to click with.
//!
//! Not every test uses every helper, hence the blanket `dead_code` allow: this
//! module is compiled once per test binary that declares it.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use icedtea_contract::{
    ClipEntry, ClipKind, Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo,
};
use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};
use icedtea_shell::clip_client::ClipCommands;
use icedtea_shell::compositor_client::CompositorCommands;
use icedtea_shell::panel::{self, Msg, PanelModel};
use icedtea_shell::style;
use icedtea_ui::gallery::Theme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox, InboxSender, PopupEvent};

/// How long any wait in this module gives the compositor and the panel.
///
/// A generous complexity bound, never a timing pin: the panel has to connect,
/// take a configure, lay out and paint before its first report exists.
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// A recording `CompositorCommands`, shared with the panel's thread.
///
/// `Arc<Mutex<_>>`, unlike `panel.rs`'s unit-test mock: the panel runs on its
/// own thread here and the assertions read from the test's.
#[derive(Clone, Default)]
pub struct MockWm(pub Arc<Mutex<Vec<(String, u32)>>>);

impl CompositorCommands for MockWm {
    fn focus_window(&self, id: u32) {
        self.0.lock().expect("wm calls").push(("focus".into(), id));
    }
    fn close_window(&self, id: u32) {
        self.0.lock().expect("wm calls").push(("close".into(), id));
    }
    fn set_workspace(&self, id: u32) {
        self.0
            .lock()
            .expect("wm calls")
            .push(("workspace".into(), id));
    }
}

/// A recording `ClipCommands`, shared with the panel's thread.
#[derive(Clone, Default)]
pub struct MockClip(pub Arc<Mutex<Vec<(String, u64)>>>);

impl ClipCommands for MockClip {
    fn activate(&self, id: u64) {
        self.0
            .lock()
            .expect("clip calls")
            .push(("activate".into(), id));
    }
    fn pin(&self, id: u64, _on: bool) {
        self.0.lock().expect("clip calls").push(("pin".into(), id));
    }
    fn remove(&self, id: u64) {
        self.0
            .lock()
            .expect("clip calls")
            .push(("remove".into(), id));
    }
    fn clear(&self) {
        self.0.lock().expect("clip calls").push(("clear".into(), 0));
    }
}

/// A directory removed when the test ends, however it ends.
///
/// Hand-rolled rather than `tempfile`: contract §3.5 keeps shell's
/// dev-dependencies as they are, and this is ten lines.
pub struct TempDir(PathBuf);

impl TempDir {
    #[must_use]
    pub fn new(tag: &str) -> TempDir {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("icedtea-shell-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One compositor, one running panel, one pointer and one screencopy client.
pub struct Panel {
    pub wm: MockWm,
    pub clip: MockClip,
    tx: InboxSender<Msg>,
    pointer: VirtualPointerClient,
    screencopy: ScreencopyClient,
    report: PathBuf,
    output: (u32, u32),
    background: (u8, u8, u8),
    _dir: TempDir,
    _panel: JoinHandle<()>,
    // Dropped last: killing the compositor first would take the panel's
    // connection out from under it mid-assertion.
    _compositor: Compositor,
}

impl Panel {
    /// Boot a compositor and run the real panel against it under `theme`.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output or screencopy, or if the panel
    /// never publishes its first report.
    #[must_use]
    pub fn spawn(theme: Theme) -> Panel {
        let compositor = Compositor::spawn();
        // The panel connects by the compositor's *absolute* socket path, not
        // by a bare `$WAYLAND_DISPLAY` name resolved against the process
        // environment. That path already lives under the harness's private
        // runtime dir, so the panel joins this compositor and never the
        // developer's live session — the effect the settings harness gets by
        // setting `XDG_RUNTIME_DIR` on the child it spawns, achieved here
        // without touching the process-global environment the way an
        // `unsafe set_var` would (P5 Task 12 reconciliation).
        let socket = compositor.socket.clone();
        let socket_path = compositor.socket_path();
        let (ow, oh) = compositor.output_size();
        let output = (ow as u32, oh as u32);

        let dir = TempDir::new(theme.name());
        let report = dir.path().join("report");

        let wm = MockWm::default();
        let clip = MockClip::default();
        let (handshake_tx, handshake_rx) = mpsc::channel::<InboxSender<Msg>>();

        let thread_wm = wm.clone();
        let thread_clip = clip.clone();
        let thread_report = report.clone();
        let panel_thread = std::thread::spawn(move || {
            let window = match icedtea_ui::window::Window::open_at_path(
                &socket_path,
                panel::spec(),
                style::sheet_for(theme),
                FontDatabase::new(),
            ) {
                Ok(window) => window,
                Err(err) => panic!("the panel could not open its layer surface: {err}"),
            };
            let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
            let width_tx = tx.clone();
            if handshake_tx.send(tx).is_err() {
                return;
            }
            let model = PanelModel::new(
                Rc::new(thread_wm) as Rc<dyn CompositorCommands>,
                Rc::new(thread_clip) as Rc<dyn ClipCommands>,
            );
            // The `App::on_frame` hook the binary installs, inlined: publish
            // `#clip`'s border box into the cell `update` reads as the
            // popover's anchor. The probe report itself is written by
            // `App::run` (P5 Task 11 moved it there from a panel-owned hook),
            // so the harness names its path with `with_probe_report` rather
            // than through a `panel::frame_hook`.
            let clip_rect = model.clip_rect.clone();
            let last_width = std::cell::Cell::new(model.bar_width);
            let _ = App::new(model, panel::update, panel::view)
                .with_inbox(inbox)
                .on_popup(|ev| match ev {
                    PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
                    PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
                    _ => None,
                })
                .on_frame(move |w| {
                    clip_rect.set(w.allocation("clip").map(|a| a.border_box));
                    // M5 Task 13: mirrors `shell/src/main.rs`'s own
                    // `on_frame` wiring for `panel::PanelModel::bar_width` --
                    // a real `Msg`, diffed so an unchanging surface does not
                    // refold every frame.
                    #[allow(
                        clippy::cast_possible_wrap,
                        reason = "a layer surface's width is well within i32"
                    )]
                    let width = w.size().0 as i32;
                    if width != last_width.get() {
                        last_width.set(width);
                        let _ = width_tx.send(Msg::SurfaceWidth(width));
                    }
                })
                .with_probe_report(thread_report)
                .run(window);
        });

        let tx = handshake_rx
            .recv_timeout(TIMEOUT)
            .expect("the panel never opened its window");
        let mut screencopy = ScreencopyClient::spawn(&socket);
        let empty = screencopy.capture();
        // The bar is anchored to the top, so the bottom-right corner is
        // always wallpaper: that is the background every "did it paint?"
        // assertion compares against.
        let background = pixel(&empty, output.0 - 3, output.1 - 3).expect("background probe");
        let pointer = VirtualPointerClient::spawn(&socket);

        let panel = Panel {
            wm,
            clip,
            tx,
            pointer,
            screencopy,
            report,
            output,
            background,
            _dir: dir,
            _panel: panel_thread,
            _compositor: compositor,
        };
        let _ = panel.wait_for("alloc bar ");
        panel
    }

    /// Push a message onto the panel's inbox — what a D-Bus worker does.
    pub fn send(&self, msg: Msg) {
        self.tx.send(msg).expect("the panel is still running");
    }

    #[must_use]
    pub fn wm_calls(&self) -> Vec<(String, u32)> {
        self.wm.0.lock().expect("wm calls").clone()
    }

    #[must_use]
    pub fn clip_calls(&self) -> Vec<(String, u64)> {
        self.clip.0.lock().expect("clip calls").clone()
    }

    /// Every line of the panel's current report.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Poll the report until a line starting with `prefix` appears, and return
    /// the **most recent** such line.
    ///
    /// The report is append-only across frames (`ui/src/view/app.rs` writes a
    /// fresh `frame N` block each time the geometry changes), so a coordinate
    /// evolves down the file as the surface is configured and clicked. The
    /// last matching line is the one that describes what is on screen now —
    /// the same "latest wins" `settings/tests/support/mod.rs` reads with
    /// `.rev().find`.
    ///
    /// # Panics
    ///
    /// If no such line appears within [`TIMEOUT`]; the message lists what the
    /// report does hold, which is what makes a renamed id a readable failure.
    pub fn wait_for(&self, prefix: &str) -> String {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(line) = self
                .report()
                .into_iter()
                .rev()
                .find(|l| l.starts_with(prefix))
            {
                return line;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "no report line starting with {prefix:?} within {TIMEOUT:?}; report holds {:?}",
            self.report()
        );
    }

    /// Poll the report until `want` accepts it.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_until(&self, want: impl Fn(&[String]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.report()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; report holds {:?}",
            self.report()
        );
    }

    /// Poll the recorded commands until `want` accepts them.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_for_calls(&self, want: impl Fn(&[(String, u32)]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.wm_calls()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; wm calls were {:?}",
            self.wm_calls()
        );
    }

    /// The `ClipCommands` counterpart.
    ///
    /// # Panics
    ///
    /// If it does not within [`TIMEOUT`].
    pub fn wait_for_clip_calls(&self, want: impl Fn(&[(String, u64)]) -> bool, what: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if want(&self.clip_calls()) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "{what} did not happen within {TIMEOUT:?}; clip calls were {:?}",
            self.clip_calls()
        );
    }

    /// The output-space centre of the probe point `label`.
    ///
    /// The layer surface is anchored left, right and top with zero margins, so
    /// its own (0, 0) is the output's: a window coordinate *is* an output
    /// coordinate here, and no offset arithmetic is needed.
    ///
    /// # Panics
    ///
    /// If the panel exposes no such probe point.
    #[must_use]
    pub fn point(&self, label: &str) -> (i32, i32) {
        let line = self.wait_for(&format!("probe {label} "));
        let fields: Vec<&str> = line.split_whitespace().collect();
        (
            fields[2].parse().expect("probe x"),
            fields[3].parse().expect("probe y"),
        )
    }

    /// `id`'s border box in output coordinates, as `(x, y, width, height)`.
    ///
    /// # Panics
    ///
    /// If the panel reports no allocation for `id`.
    #[must_use]
    pub fn allocation(&self, id: &str) -> (i32, i32, i32, i32) {
        let line = self.wait_for(&format!("alloc {id} "));
        let f: Vec<&str> = line.split_whitespace().collect();
        (
            parse_coord(f[2]),
            parse_coord(f[3]),
            parse_coord(f[4]),
            parse_coord(f[5]),
        )
    }

    /// The ids the report holds for direct children of `container`, in
    /// left-to-right order — the replacement for `shell_gtk.rs`'s `labels()`
    /// walk of a GTK widget tree.
    ///
    /// Membership is by prefix and geometry: `window_*`/`ws_*`/`history_*`
    /// ids are unique per entity, and an id whose box sits inside
    /// `container`'s box is one of its children. Ordered by `x` for a
    /// horizontal container and by `y` for a vertical one, which is decided by
    /// which of the container's dimensions is the larger.
    #[must_use]
    pub fn labels_under(&self, container: &str, prefix: &str) -> Vec<String> {
        let (cx, cy, cw, ch) = self.allocation(container);
        // Latest wins per id: the append-only report carries an id once per
        // frame it changed in, so a plain scan would list it as many times as
        // it moved. The last line for an id is the box it holds now.
        let mut latest: std::collections::HashMap<String, (i32, i32)> =
            std::collections::HashMap::new();
        for line in self.report() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() != 6 || f[0] != "alloc" || !f[1].starts_with(prefix) {
                continue;
            }
            latest.insert(f[1].to_string(), (parse_coord(f[2]), parse_coord(f[3])));
        }
        let mut found: Vec<(i32, i32, String)> = latest
            .into_iter()
            .filter(|(_, (x, y))| *x >= cx && *x < cx + cw && *y >= cy && *y < cy + ch)
            .map(|(id, (x, y))| (x, y, id))
            .collect();
        if cw >= ch {
            found.sort_by_key(|(x, _, _)| *x);
        } else {
            found.sort_by_key(|(_, y, _)| *y);
        }
        found.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Left-click at `(x, y)`.
    pub fn click(&mut self, x: i32, y: i32) {
        self.click_button(x, y, icedtea_ui::window::pointer::BTN_LEFT);
    }

    /// Press and release `button` at `(x, y)`.
    ///
    /// The settle between the motion and the button repeats
    /// `ui/tests/support/mod.rs`'s `Driver::move_to` rationale verbatim: the
    /// compositor's assignment of pointer focus to the surface under the
    /// cursor is not synchronous with the `motion_absolute` that triggers it,
    /// and `wlr_seat_pointer_notify_button` drops a button with no focused
    /// surface silently.
    pub fn click_button(&mut self, x: i32, y: i32, button: u32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(25));
            self.pointer.pump();
        }
        self.pointer.button(button, true);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.button(button, false);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.pump();
    }

    /// One screencopy frame of the whole output.
    pub fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }

    #[must_use]
    pub fn background(&self) -> (u8, u8, u8) {
        self.background
    }

    #[must_use]
    pub fn output(&self) -> (u32, u32) {
        self.output
    }
}

/// Parse one `alloc`/`probe` coordinate field.
///
/// The report writes each box as a laid-out `f32` (`ui/src/view/app.rs`'s
/// `probe_report_lines`), so a whole number arrives as `28` and a fractional
/// one as `27.5`; both must round to the same integer output pixel a pointer
/// can be aimed at. A bare `str::parse::<i32>` would reject the second form.
#[must_use]
fn parse_coord(field: &str) -> i32 {
    if let Ok(n) = field.parse::<i32>() {
        return n;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a laid-out coordinate is within i32 by construction"
    )]
    field
        .parse::<f32>()
        .map(|v| v.round() as i32)
        .unwrap_or_else(|_| panic!("unparseable report coordinate {field:?}"))
}

/// The RGB of `(x, y)` in a captured frame, or `None` outside it.
#[must_use]
pub fn pixel(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture — `ui/tests/support/mod.rs`'s constant, for the
/// same format-conversion reason.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// Whether `a` and `b` are the same colour within [`SCREENCOPY_TOLERANCE`].
#[must_use]
pub fn same(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let close =
        |p: u8, q: u8| i32::from(p).abs_diff(i32::from(q)) <= u32::from(SCREENCOPY_TOLERANCE);
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
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
        (x..x + w)
            .any(|px| pixel(frame, px as u32, py as u32).is_some_and(|got| !same(got, background)))
    })
}

// --- fixtures, verbatim from `tests/shell_gtk.rs` -------------------------

#[must_use]
pub fn win(id: u32, title: &str) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: "app".into(),
        title: title.into(),
        pid: 0,
        workspace: 0,
        geometry: Rectangle {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        maximized: false,
        minimized: false,
        fullscreen: false,
        focused: false,
        attention: false,
    }
}

#[must_use]
pub fn snapshot(windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Snapshot {
    Snapshot {
        seq: 1,
        windows,
        workspaces,
        active_workspace: 0,
    }
}

#[must_use]
pub fn clip_entry(id: u64, preview: &str) -> ClipEntry {
    ClipEntry {
        id,
        kind: ClipKind::Text,
        preview: preview.into(),
        mime: "text/plain".into(),
        pinned: false,
        source_app: None,
    }
}
