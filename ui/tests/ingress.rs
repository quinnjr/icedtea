//! M5 P0's ingress surface: `Window::watch_fd`, the inbox, `App::on_fd`,
//! `Cmd::Task`, the view-layer pointer events and `Keymap::base_keysym`.
//!
//! Live-`Window` tests drive `window-probe` under the harness compositor,
//! because `Window::open` reads `WAYLAND_DISPLAY` from the process
//! environment and a test process cannot set that per-test (edition 2024
//! makes `set_var` unsafe, and the tests in this binary run in parallel).
//! Everything that does not need a surface runs offscreen, in-process.

mod support;

use std::time::Duration;

use icedtea_harness::Compositor;
use support::{probe_report, probe_theme, spawn_window_probe_with, wait_for_report_line};

/// How long a probe gets to boot, map and report. The gallery gate's own
/// `GALLERY_MAP_TIMEOUT` documents why this is seconds rather than
/// milliseconds: a first anti-aliased paint is slow in a debug build.
const REPORT: Duration = Duration::from_secs(20);

#[test]
fn a_watched_pipe_wakes_pump_with_fd_ready() {
    // mutation: drop the `for (index, watch) in watches.entries()` loop from
    // `wait_bounded`'s `Ok(_)` arm; nothing ever pushes `FdReady` and this
    // test times out on `fd-ready`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    let registered = wait_for_report_line(report.path(), "watch-registered ", REPORT)
        .expect("the probe never registered its pipe");
    let id = registered
        .split_whitespace()
        .nth(1)
        .expect("the line carries the watch id")
        .to_string();
    let ready = wait_for_report_line(report.path(), "fd-ready ", REPORT)
        .expect("a byte on the watched pipe never woke pump");
    assert_eq!(
        ready.split_whitespace().nth(1),
        Some(id.as_str()),
        "the FdReady names a different watch: {ready}"
    );
}

#[test]
fn an_unwatched_fd_stops_waking_the_loop() {
    // mutation: make `Watches::remove` a no-op; the probe keeps reporting
    // `fd-ready` after the unwatch and never reports `fd-quiet`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-quiet", REPORT).is_some(),
        "the probe still saw readiness after unwatching: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let unwatched = lines
        .iter()
        .position(|l| l.starts_with("unwatched "))
        .expect("the probe unwatched");
    let quiet = lines
        .iter()
        .position(|l| l == "fd-quiet")
        .expect("the probe reported quiet");
    assert!(
        quiet > unwatched,
        "the quiet window must follow the unwatch"
    );
    assert!(
        !lines[unwatched..]
            .iter()
            .any(|l| l.starts_with("fd-ready ")),
        "an FdReady arrived after the unwatch: {lines:?}"
    );
}

#[test]
fn two_watched_fds_report_in_registration_order() {
    // mutation: reverse `Watches::entries()`; the ids arrive swapped.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--two-watches"],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-pair ", REPORT).is_some(),
        "the probe never saw both fds ready in one batch: {:?}",
        probe_report(report.path())
    );
    let line = probe_report(report.path())
        .into_iter()
        .find(|l| l.starts_with("fd-pair "))
        .expect("the pair line");
    let ids: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    assert_eq!(ids.len(), 2, "the pair line carries two ids: {line}");
    assert!(
        ids[0] < ids[1],
        "FdReady must arrive in registration order, got {ids:?}"
    );
}

#[test]
fn a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout() {
    // A window with a registered but silent watch must still configure, paint
    // and report frames exactly as `window_events.rs` expects of one without.
    // mutation: return `SurfaceError::Timeout` whenever any watch is
    // registered; `frame ` never appears.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    assert!(
        wait_for_report_line(report.path(), "configure ", REPORT).is_some(),
        "a window with a silent watch never configured"
    );
    assert!(
        wait_for_report_line(report.path(), "frame ", REPORT).is_some(),
        "a window with a silent watch never painted a frame"
    );
}

#[test]
fn unwatching_an_unknown_id_is_a_no_op() {
    // mutation: `panic!` in `Watches::remove` for an id it does not hold; the
    // probe dies before reporting `unwatch-unknown-ok`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "unwatch-unknown-ok", REPORT).is_some(),
        "unwatching an unknown id was not survivable: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn an_idle_watch_costs_no_extra_wakeups() {
    // A complexity bound, not a wall-clock pin (contract §5 cross-cutting
    // rule): over a one-second idle window with a registered, silent watch,
    // `pump` must return far fewer times than a spin would produce. A spin
    // returns thousands; the frame clock alone returns tens.
    // mutation: add `Duration::ZERO` to `frame_deadline` whenever a watch is
    // registered; `wakes` climbs past the bound and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    let line = wait_for_report_line(report.path(), "wakes ", REPORT)
        .expect("the probe never closed its idle window");
    let wakes: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|f| f.parse().ok())
        .expect("the wakes line carries a count");
    assert!(
        wakes < 500,
        "an idle window with one silent watch woke {wakes} times in a second; \
         that is a spin, not a poll"
    );
}

#[test]
fn on_fd_maps_a_foreign_fd_to_messages() {
    // mutation: drop the `for (watch, handler) in &self.fd_handlers` loop from
    // `App::run`; the probe never reports `folded fd`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "app-inbox",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "folded fd", REPORT).is_some(),
        "an `on_fd` closure never produced a message: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn an_inbox_message_wakes_a_live_app() {
    // The end-to-end claim P1 and P5 rest on: a worker thread's `send` reaches
    // `update` on a running app with no polling and no timer.
    // mutation: never call `window.watch_fd` for the inbox in `App::run`; the
    // app only notices on some *other* wakeup, and with nothing else moving
    // the report line never appears inside the timeout.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "app-inbox",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "folded inbox", REPORT).is_some(),
        "a worker's message never reached update: {:?}",
        probe_report(report.path())
    );
}

use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::view::builders::button;
use icedtea_ui::view::{App, Cmd, Inbox, ScriptStep, View};
use icedtea_ui::window::InputEvent;
use std::rc::Rc;

/// What the ordering tests fold: every message appends its own tag, so the
/// model *is* the delivery order.
#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Inbox(u32),
    Clicked,
}

fn tag_update(model: &mut Vec<String>, msg: Tag) -> Cmd<Tag> {
    match msg {
        Tag::Inbox(n) => model.push(format!("inbox{n}")),
        Tag::Clicked => model.push("click".to_owned()),
    }
    Cmd::None
}

// `&Vec<String>`, not `&[String]`: `App::new`'s `view` parameter is
// `impl Fn(&M) -> View<Msg>` with `M = Vec<String>` here, so the parameter
// type is fixed by the model type, not a free choice this function makes.
#[allow(
    clippy::ptr_arg,
    reason = "the model type App::new fixes is Vec<String>"
)]
fn tag_view(model: &Vec<String>) -> View<Tag> {
    // A `button`, not a `label`: `LabelC::on_event` handles only link
    // activation and never fires the generic `Click` event, so a
    // `label(..).on_click(..)` handler is unreachable no matter where the
    // pointer lands (reconciliation — the plan's literal test used `label`).
    button(&model.join(",")).on_click(Tag::Clicked).id("target")
}

#[test]
fn inbox_messages_are_applied_in_send_order() {
    // mutation: drain the channel with `rx.recv().into_iter().rev()`; the
    // order flips and this fails.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    for n in 0..3 {
        tx.send(Tag::Inbox(n)).expect("the inbox is alive");
    }
    let (frames, model) = run_tagged(inbox, vec![ScriptStep::Capture]);
    assert_eq!(frames.len(), 1);
    assert_eq!(model, vec!["inbox0", "inbox1", "inbox2"]);
}

#[test]
fn inbox_messages_are_applied_before_the_frames_input_batch() {
    // The property P1's reload worker and P5's compositor client depend on:
    // state that arrived from a worker is folded before the click that the
    // same frame delivers, so `update` never sees a click against a stale
    // model (M5-D2 §3).
    //
    // mutation: move the inbox drain *after* the `for event in &events`
    // routing loop in `App::run`/`run_offscreen`; the order becomes
    // ["click", "inbox9"] and this fails.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    tx.send(Tag::Inbox(9)).expect("the inbox is alive");
    // (80.0, 20.0) is the window's center: an unstyled `label` lays out
    // centered in its parent, so this point stays inside its border box
    // whether the label is empty (frame 1) or reads "inbox9" (frame 2+),
    // unlike a point picked near the left edge.
    let (_, model) = run_tagged(
        inbox,
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(80.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(
        model,
        vec!["inbox9", "click"],
        "the inbox message must be folded before the frame's input batch"
    );
}

#[test]
fn an_inbox_message_reaches_update_on_the_next_frame() {
    // A message sent *after* the app started still arrives: the pipe is what
    // makes the loop look.
    // mutation: never register the inbox fd in `App::run`; the live probe
    // test `an_app_inbox_message_reaches_update` (Task 6) fails, and here the
    // offscreen drain still runs, so this test's value is the send-then-step
    // sequencing rather than the wake.
    let (inbox, tx) = Inbox::<Tag>::new().expect("a pipe");
    let sender = tx.clone();
    let (_, model) = run_tagged(
        inbox,
        vec![ScriptStep::Capture, {
            sender.send(Tag::Inbox(1)).expect("alive");
            ScriptStep::Capture
        }],
    );
    assert_eq!(model, vec!["inbox1"]);
}

/// Build an app whose `update` mirrors the fold order into `seen`, run it
/// offscreen, and hand back the frames plus that order.
fn run_tagged(
    inbox: Inbox<Tag>,
    script: Vec<ScriptStep<Tag>>,
) -> (icedtea_ui::view::Frames, Vec<String>) {
    let seen: Rc<std::cell::RefCell<Vec<String>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = Rc::clone(&seen);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        Vec::<String>::new(),
        move |model: &mut Vec<String>, msg: Tag| {
            let cmd = tag_update(model, msg);
            recorder
                .borrow_mut()
                .push(model.last().cloned().unwrap_or_default());
            cmd
        },
        tag_view,
    )
    .with_inbox(inbox)
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen((160, 40), clock, script)
    .expect("the offscreen app runs");
    let order = seen.borrow().clone();
    (frames, order)
}
