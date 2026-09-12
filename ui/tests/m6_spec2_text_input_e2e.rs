//! M6.1 Spec 2 end to end: an IME `commit_string` reaches a toolkit `Entry`
//! through the real compositor relay, and typing re-syncs surrounding text.
//!
//! The app runs on its own thread against the harness compositor (the
//! `app_frame_hook` shape); the test thread focuses the entry with a Tab,
//! drives composition through `InputMethodClient`, and asserts on the app's
//! model text plus the IME-side surrounding relay.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_harness::{Compositor, InputMethodClient, VirtualKeyboardClient, VirtualPointerClient};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::text_input::{ContentHint, ContentPurpose, Snapshot};
use icedtea_ui::view::builders::entry;
use icedtea_ui::view::{App, Cmd, View};
use icedtea_ui::wayland::BTN_LEFT;
use icedtea_ui::window::{Role, SurfaceSpec, Window};

/// Linux input-event keycodes (`KEY_A` for the typing half).
const KEY_A: u32 = 30;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Changed(String),
}

#[derive(Debug, Clone)]
struct Model {
    text: String,
    seen: Arc<Mutex<Vec<String>>>,
}

fn update(model: &mut Model, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::Changed(text) => {
            model.text = text.clone();
            model
                .seen
                .lock()
                .expect("seen")
                .push(text);
            Cmd::None
        }
    }
}

fn view(model: &Model) -> View<Msg> {
    entry::<Msg>(&model.text)
        .id("ime_entry")
        .on_change(|text: &str| Msg::Changed(text.to_owned()))
}

fn poll_until(deadline: Instant, mut pred: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    pred()
}

fn open_window(socket_path: &std::path::Path, title: &str) -> Window {
    let spec = SurfaceSpec {
        role: Role::Toplevel,
        size: (240, 60),
        title: title.to_string(),
        app_id: "org.icedtea.ImeE2e".to_string(),
    };
    let sheet = CompiledSheet::compile(
        "window { background-color: #ffffff; } \
         entry { min-width: 200px; min-height: 28px; }",
    );
    Window::open_at_path(socket_path, spec, sheet, FontDatabase::new())
        .expect("the toolkit window opens against the harness compositor")
}

/// The `Window`-level half: `ime_enable`/`ime_sync`/`ime_disable` drive the
/// relay with no app and no focus dance in the loop. Bisects an
/// app-level failure (Tab/focus/routing) from a plumbing one.
#[test]
fn window_ime_calls_drive_the_relay() {
    let comp = Compositor::spawn();
    let mut window = open_window(&comp.socket_path(), "ime-plumbing");
    assert!(
        window.ime_ready(),
        "the toolkit window holds no text-input: the manager/seat pair never completed"
    );
    // An unmapped surface never takes keyboard focus, and without focus the
    // relay never enters: paint the first frame so the surface maps, then
    // pump once so any `enter` lands before the enable below.
    window.render().expect("the first frame paints");
    let _ = window.pump(Some(Duration::from_millis(500)));
    let socket = comp.socket.clone();
    let mut im = InputMethodClient::spawn(&socket);

    window.ime_enable(ContentHint::NONE.wire(), ContentPurpose::Normal.wire());
    window.ime_sync(Snapshot::new(
        "hello",
        5,
        Some(5),
        ContentHint::NONE,
        ContentPurpose::Normal,
        (10, 20, 2, 16),
    ));
    let _ = window.pump(Some(Duration::from_millis(500)));

    assert!(
        im.wait_until(|client| client
            .surroundings()
            .last()
            .is_some_and(|(text, cursor, anchor)| text == "hello" && (*cursor, *anchor) == (5, 5))),
        "the relay never got surrounding text, saw {:?}",
        im.surroundings()
    );
    assert!(
        im.content_types().last() == Some(&(0, 0)),
        "the relay never got the content type, saw {:?}",
        im.content_types()
    );
    assert!(comp.input_method_active(), "oracle: IME must be active");

    window.ime_disable();
    let _ = window.pump(Some(Duration::from_millis(500)));
    assert!(
        im.wait_until(|client| client.deactivates() > 0),
        "the relay never deactivated"
    );
    assert!(!comp.input_method_active(), "oracle: IME must be idle");
}

#[test]
fn an_ime_commit_reaches_a_toolkit_entry_through_the_relay() {
    let compositor = Compositor::spawn();
    let socket_path = compositor.socket_path().to_path_buf();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_in = seen.clone();
    let frames: Arc<std::sync::atomic::AtomicUsize> = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let frames_in = frames.clone();
    // The live window's probe points, republished every frame: the click
    // below needs the entry's window-local centre.
    let points: Arc<Mutex<Vec<(String, i32, i32)>>> = Arc::new(Mutex::new(Vec::new()));
    let points_in = points.clone();

    std::thread::spawn(move || {
        let spec = SurfaceSpec {
            role: Role::Toplevel,
            size: (240, 60),
            title: "ime-e2e".to_string(),
            app_id: "org.icedtea.ImeE2e".to_string(),
        };
        let sheet = CompiledSheet::compile(
            "window { background-color: #ffffff; } \
             entry { min-width: 200px; min-height: 28px; }",
        );
        let Ok(window) = Window::open_at_path(&socket_path, spec, sheet, FontDatabase::new())
        else {
            return;
        };
        let _ = App::new(
            Model { text: String::new(), seen: seen_in },
            update,
            view,
        )
        .on_frame(move |window| {
            frames_in.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut published = points_in.lock().expect("points");
            published.clear();
            published.extend(
                window
                    .probe_points()
                    .into_iter()
                    .map(|point| (point.label, point.x, point.y)),
            );
        })
        .run(window);
    });

    // The app thread maps and frames before any click can mean anything.
    assert!(
        poll_until(Instant::now() + Duration::from_secs(20), || {
            frames.load(std::sync::atomic::Ordering::SeqCst) > 0
        }),
        "the app never framed: its thread died before mapping"
    );

    // The surface position, from the compositor's own model: probe points
    // are window-local, the pointer speaks output-global.
    let (output_w, output_h) = compositor.output_size();
    let deadline = Instant::now() + Duration::from_secs(20);
    let surface = loop {
        let hit = compositor
            .snapshot()
            .windows
            .into_iter()
            .find(|window| window.title == "ime-e2e");
        if let Some(surface) = hit {
            break surface;
        }
        assert!(Instant::now() < deadline, "the app surface never mapped");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(surface.focused, "the app surface never took keyboard focus");

    let deadline = Instant::now() + Duration::from_secs(20);
    let entry = loop {
        let hit = points
            .lock()
            .expect("points")
            .iter()
            .find(|(label, _, _)| label == "ime_entry")
            .cloned();
        if let Some(entry) = hit {
            break entry;
        }
        assert!(Instant::now() < deadline, "the entry never laid out");
        std::thread::sleep(Duration::from_millis(50));
    };

    // Focus the entry with a click, the pointer user's path into a field.
    // Probe points are client-surface-local; the compositor's snapshot
    // geometry starts at the server-side title bar, so the click adds its
    // height (the `window_events` gate's own `TITLE_BAR_HEIGHT`).
    const TITLE_BAR_HEIGHT: i32 = 28;
    let mut pointer = VirtualPointerClient::spawn(&compositor.socket);
    let click = (
        (surface.geometry.x + entry.1) as f64,
        (surface.geometry.y + TITLE_BAR_HEIGHT + entry.2) as f64,
    );
    pointer.motion_absolute(click.0, click.1, output_w as u32, output_h as u32);
    pointer.frame();
    pointer.pump();
    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.pump();
    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();

    let socket = compositor.socket.clone();
    let mut vk = VirtualKeyboardClient::spawn(&socket);
    let mut im = InputMethodClient::spawn(&socket);

    // The click focuses the entry, which enables IME: the IME-side
    // surrounding text is the oracle.
    assert!(
        im.wait_until(|client| !client.surroundings().is_empty()),
        "the IME never saw surrounding text after the entry was clicked"
    );

    // The empty entry syncs empty surrounding with the cursor at zero.
    assert_eq!(
        im.surroundings().last(),
        Some(&("".to_string(), 0, 0)),
        "initial surrounding must echo the empty buffer, saw {:?}",
        im.surroundings()
    );

    // A preedit alone is display-only: no commit reaches the app.
    im.send_commit(Some("に"), None, None);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        seen.lock().expect("seen").is_empty(),
        "a preedit must not change the model, saw {:?}",
        seen.lock().expect("seen")
    );

    // The commit lands in the buffer and fires Change.
    im.send_commit(None, Some("にち"), None);
    let deadline = Instant::now() + Duration::from_secs(20);
    assert!(
        poll_until(deadline, || seen.lock().expect("seen").last().is_some()),
        "the IME commit never reached the entry"
    );
    assert_eq!(
        seen.lock().expect("seen").last(),
        Some(&"にち".to_string()),
        "the entry must hold exactly the committed string"
    );

    // Typing re-syncs surrounding text back through the relay.
    vk.key_press(KEY_A);
    let deadline = Instant::now() + Duration::from_secs(20);
    assert!(
        poll_until(deadline, || seen
            .lock()
            .expect("seen")
            .last()
            .is_some_and(|text| text == "にちa")),
        "typing `a` never changed the entry, saw {:?}",
        seen.lock().expect("seen")
    );
    assert!(
        im.wait_until(|client| client
            .surroundings()
            .last()
            .is_some_and(|(text, _, _)| text == "にちa")),
        "the relay never re-forwarded the typed surrounding, saw {:?}",
        im.surroundings()
    );
}
