//! P5's `App::on_frame` (contract §6 P5-D5): the per-frame window hook M5's
//! apps write their probe report from.
//!
//! Harness-driven because the hook's whole point is a *live* `Window`: the
//! app runs on its own thread against `icedtea_harness::Compositor`, and the
//! test thread watches a counter and a probe label the hook publishes.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_harness::Compositor;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::{box_, button};
use icedtea_ui::view::{App, Cmd, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::{Role, SurfaceSpec, Window};

#[derive(Clone, Debug, PartialEq)]
#[allow(
    dead_code,
    reason = "never constructed; only names the App's message type"
)]
enum Msg {
    Never,
}

#[test]
fn on_frame_sees_the_live_windows_probe_points() {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();

    let frames = Arc::new(AtomicUsize::new(0));
    let labels: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let frames_in = frames.clone();
    let labels_in = labels.clone();

    std::thread::spawn(move || {
        // SAFETY: this thread is the only one that touches the environment,
        // and it does so before opening any Wayland connection.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket) };
        let spec = SurfaceSpec {
            role: Role::Toplevel,
            size: (200, 60),
            title: "on-frame".to_string(),
            app_id: "org.icedtea.OnFrame".to_string(),
        };
        let sheet = CompiledSheet::compile(
            "window { background-color: #ffffff; } \
             button { min-width: 40px; min-height: 20px; background-color: #808080; }",
        );
        let Ok(window) = Window::open(spec, sheet, FontDatabase::new()) else {
            return;
        };
        let _ = App::new(
            (),
            |_m: &mut (), _msg: Msg| Cmd::None,
            |_m: &()| -> View<Msg> {
                box_(
                    Orientation::Horizontal,
                    [button::<Msg>("hello").id("greeting")],
                )
            },
        )
        .on_frame(move |w| {
            frames_in.fetch_add(1, Ordering::SeqCst);
            let mut seen = labels_in.lock().expect("labels");
            for point in w.probe_points() {
                if !seen.contains(&point.label) {
                    seen.push(point.label);
                }
            }
        })
        .run(window);
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if frames.load(Ordering::SeqCst) > 0 && !labels.lock().expect("labels").is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        frames.load(Ordering::SeqCst) > 0,
        "on_frame never ran against a live window"
    );
    let seen = labels.lock().expect("labels").clone();
    assert!(
        seen.iter().any(|l| l == "greeting"),
        "the hook must see the live window's probe points; saw {seen:?}"
    );
}
