//! `window-probe` -- the client `ui/tests/window_events.rs` drives.
//!
//! Builds a GTK-shaped node tree by hand (P5/P6 own the widgets that will
//! produce the same trees), maps it as an `xdg_toplevel`, and appends one line
//! per interesting event to `$ICEDTEA_PROBE_REPORT` so a test can assert on
//! what the client actually saw, not only on what it painted.
//!
//! Environment:
//! - `ICEDTEA_UI_THEME`   -- a path to the complete theme to use.
//! - `ICEDTEA_PROBE_MODE` -- `entry` (default) or `menu`.
//! - `ICEDTEA_PROBE_REPORT` -- the report file to append to.

use std::io::Write;
use std::path::PathBuf;

use icedtea_ui::app::{ThemeSource, compile_theme};
use icedtea_ui::css::node::Node;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::window::focus::{
    Binding, FOCUSABLE_CLASS, FocusCause, FocusRing, navigate, window_binding,
};
use icedtea_ui::window::popup::{PopupAnchorPoint, Positioner};
use icedtea_ui::window::{InputEvent, Interest, Role, SurfaceSpec, Window};

fn report(line: &str) {
    let Ok(path) = std::env::var("ICEDTEA_PROBE_REPORT") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let theme = std::env::var("ICEDTEA_UI_THEME").map_or(ThemeSource::Bundled, |v| {
        ThemeSource::File(PathBuf::from(v))
    });
    let mode = std::env::var("ICEDTEA_PROBE_MODE").unwrap_or_else(|_| "entry".into());
    let sheet = compile_theme(&theme);
    let fonts = FontDatabase::new();

    let window = Window::open(
        SurfaceSpec {
            role: Role::Toplevel,
            size: (400, 200),
            title: "window probe".into(),
            app_id: "org.icedtea.WindowProbe".into(),
        },
        sheet,
        fonts,
    )
    .expect("the probe could not open its window");

    let two_watches = std::env::args().any(|a| a == "--two-watches");
    let silent_watch = std::env::args().any(|a| a == "--silent-watch");
    if mode == "ingress" {
        run_ingress(window, two_watches, silent_watch);
        return;
    }

    let mut window = window;
    // window > box > (entry > text), (menubutton > label)
    let root = window.root().clone();
    let container = Node::new("box");
    root.append_child(&container);
    let entry = Node::with_classes("entry", &[FOCUSABLE_CLASS]);
    entry.set_id(Some("entry"));
    let text = Node::new("text");
    entry.append_child(&text);
    container.append_child(&entry);
    let menubutton = Node::with_classes("menubutton", &[FOCUSABLE_CLASS]);
    menubutton.set_id(Some("menubutton"));
    let label = Node::new("label");
    menubutton.append_child(&label);
    container.append_child(&menubutton);
    window.set_node_text(&text, "");
    window.set_node_text(&label, "Menu");

    // `FocusRing` also has an inherent `default(&self) -> Option<Node>`
    // method (the window's *default-activated* node), which shadows
    // `Default::default()` under `Type::method()` call syntax -- the trait
    // method needs the fully qualified form to be reached at all.
    let mut focus = <FocusRing as Default>::default();
    let mut typed = String::new();
    let mut popup = None;
    // The first few `wl_surface.frame` callbacks only. A commit requests one
    // callback and the probe only commits when it is dirty, so the stream is
    // bounded anyway; the cap keeps the report file bounded regardless.
    let mut frames = 0_u32;
    loop {
        if window.render().is_err() {
            break;
        }
        let timeout = window.next_deadline();
        let Ok(events) = window.pump(timeout) else {
            break;
        };
        for event in events {
            match event {
                InputEvent::Configure { size, states } => {
                    report(&format!("configure {} {} {states:?}", size.0, size.1));
                }
                InputEvent::Close => return,
                InputEvent::KeyboardEnter { .. } => {
                    if focus.focus().is_none() {
                        focus.set_focus(Some(&entry), FocusCause::Programmatic);
                    }
                    report("keyboard-enter");
                }
                InputEvent::Key(key) => {
                    focus.note_key(&key);
                    match window_binding(&key) {
                        Some(Binding::Move(dir)) => {
                            let next =
                                navigate(&root, window.layout(), focus.focus().as_ref(), dir)
                                    .or_else(|| navigate(&root, window.layout(), None, dir));
                            focus.set_focus(next.as_ref(), FocusCause::Keyboard);
                            report(&format!(
                                "focus {} visible={}",
                                next.and_then(|n| n.id()).map_or_else(
                                    || "none".to_string(),
                                    |id| id.as_str().to_string()
                                ),
                                focus.focus_visible()
                            ));
                        }
                        Some(Binding::Dismiss) => {
                            if let Some(key) = popup.take() {
                                window.close_popup(key);
                            }
                        }
                        Some(_) | None => {
                            if key.pressed
                                && let Some(utf8) = &key.utf8
                                && focus.focus().is_some_and(|n| n.ptr_eq(&entry))
                            {
                                typed.push_str(utf8);
                                window.set_node_text(&text, &typed);
                                report(&format!("typed {typed}"));
                            }
                        }
                    }
                    window.mark_dirty(&root);
                }
                InputEvent::PointerButton { pressed: true, .. } if mode == "menu" => {
                    match window.open_popup(
                        PopupAnchorPoint::Node(menubutton.clone()),
                        Positioner::menu(icedtea_ui::layout::Rect::zero(), (160, 120)),
                    ) {
                        Ok(key) => {
                            popup = Some(key);
                            report("popup-open");
                        }
                        Err(err) => report(&format!("popup-error {err}")),
                    }
                }
                InputEvent::Frame { now } => {
                    if frames < 3 {
                        frames += 1;
                        report(&format!("frame {}", now.as_micros()));
                    }
                }
                InputEvent::PopupDone(key) => {
                    window.close_popup(key);
                    popup = None;
                    report("popup-done");
                }
                _ => {}
            }
        }
        if window.is_closed() {
            return;
        }
    }
}

/// `ICEDTEA_PROBE_MODE=ingress`: exercise `Window::watch_fd`/`unwatch` against
/// a real compositor and report what the loop saw.
///
/// The pipes are written from a helper thread rather than from the loop, which
/// is the shape a real worker has: the loop must learn about them only through
/// `poll`.
fn run_ingress(mut window: Window, two_watches: bool, silent_watch: bool) {
    use rustix::pipe::{PipeFlags, pipe_with};
    use std::time::{Duration, Instant};

    // An id the window cannot have live: registered, then immediately
    // retired. `WatchId` is opaque and ids are never reused, so it stays dead
    // and `unwatch`ing it twice is the "unknown id" case M5-D1 §4 names.
    let (scratch_read, _scratch_write) =
        pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("scratch pipe");
    let stale = window.watch_fd(scratch_read, Interest::Read);
    window.unwatch(stale);
    let before = window.watches().len();
    window.unwatch(stale);
    if window.watches().len() == before {
        report("unwatch-unknown-ok");
    }

    let (read_a, write_a) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("pipe a");
    let id_a = window.watch_fd(read_a, Interest::Read);
    report(&format!("watch-registered {}", id_index(id_a)));
    let mut id_b = None;
    let mut write_b = None;
    if two_watches {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("pipe b");
        let id = window.watch_fd(read, Interest::Read);
        report(&format!("watch-registered {}", id_index(id)));
        id_b = Some(id);
        write_b = Some(write);
    }

    // The writer: one byte on each pipe, a little after the loop starts, so
    // the wake is a real poll wake and not a leftover from before it.
    if !silent_watch {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            let _ = rustix::io::write(&write_a, b"x");
            if let Some(write_b) = write_b {
                let _ = rustix::io::write(&write_b, b"x");
            }
        });
    }

    let started = Instant::now();
    let mut wakes: u64 = 0;
    let mut seen_ready = false;
    let mut unwatched = false;
    let mut quiet_from: Option<Instant> = None;
    let mut frames = 0_u32;
    loop {
        if window.render().is_err() {
            return;
        }
        // A bounded wait even when nothing is scheduled, so the idle-wake
        // count below is a count of *this* loop's polls, not of a block.
        let timeout = Some(
            window
                .next_deadline()
                .unwrap_or(Duration::from_millis(50))
                .min(Duration::from_millis(50)),
        );
        let Ok(events) = window.pump(timeout) else {
            return;
        };
        wakes += 1;
        let mut batch_ready: Vec<u64> = Vec::new();
        for event in events {
            match event {
                InputEvent::Configure { size, states } => {
                    report(&format!("configure {} {} {states:?}", size.0, size.1));
                }
                InputEvent::Frame { now } => {
                    if frames < 3 {
                        frames += 1;
                        report(&format!("frame {}", now.as_micros()));
                    }
                }
                InputEvent::FdReady(id) => {
                    batch_ready.push(id_index(id));
                    report(&format!("fd-ready {}", id_index(id)));
                }
                InputEvent::Close => return,
                _ => {}
            }
        }
        if batch_ready.len() == 2 {
            report(&format!("fd-pair {} {}", batch_ready[0], batch_ready[1]));
        }
        if !batch_ready.is_empty() {
            seen_ready = true;
        }
        // Once the readiness has been observed, unwatch *without* draining the
        // pipe: a still-readable fd that no longer wakes the loop is the whole
        // point of the assertion.
        if seen_ready && !unwatched {
            window.unwatch(id_a);
            if let Some(id) = id_b {
                window.unwatch(id);
            }
            unwatched = true;
            report(&format!("unwatched {}", id_index(id_a)));
            quiet_from = Some(Instant::now());
        }
        if let Some(from) = quiet_from
            && from.elapsed() > Duration::from_millis(500)
        {
            report("fd-quiet");
            quiet_from = None;
        }
        if silent_watch && started.elapsed() > Duration::from_secs(1) {
            report(&format!("wakes {wakes}"));
            return;
        }
        if unwatched && quiet_from.is_none() && !silent_watch {
            report(&format!("wakes {wakes}"));
            return;
        }
        if window.is_closed() {
            return;
        }
    }
}

/// A `WatchId`'s report form. `WatchId` is opaque, so the probe prints its
/// `Debug` payload — the only stable, orderable thing a consumer can quote.
fn id_index(id: icedtea_ui::window::WatchId) -> u64 {
    format!("{id:?}")
        .trim_start_matches("WatchId(")
        .trim_end_matches(')')
        .parse()
        .unwrap_or(u64::MAX)
}
