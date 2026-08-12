//! Task 17: a second headless output must land in `state.outputs` with a
//! real, disjoint layout-box geometry -- not the (0,0)-origin fallback
//! `OutputHandler::new_output` used before `Runtime::output_layout_box`
//! existed.
//!
//! A separate integration-test binary, not an addition to `headless_boot.rs`:
//! `WLR_HEADLESS_OUTPUTS` is read once by `Backend::autocreate` and every
//! other test in that binary assumes the default single-output headless
//! backend. Each `tests/*.rs` file is its own process, so the env var set
//! here can never leak into `headless_boot.rs`'s tests (or vice versa).

use icedtea_compositor::state::State;

/// Set the headless-backend environment (two outputs, this file's own
/// concern) exactly once, no matter which of this binary's `#[test]`s
/// reaches it first. See `headless_boot.rs`'s `ensure_headless_env` for the
/// torn-environment hazard this guards against -- identical reasoning, this
/// file's own copy because each integration-test binary has its own statics.
fn ensure_headless_env() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // SAFETY (icedtea unsafe exception (c)): `Once::call_once` above
        // guarantees this closure runs on exactly one thread and that every
        // other thread calling `ensure_headless_env` blocks until it
        // finishes -- so nothing can observe a torn read and nothing races
        // this write.
        unsafe {
            std::env::set_var("WLR_BACKENDS", "headless");
            std::env::set_var("WLR_HEADLESS_OUTPUTS", "2");
        }
    });
}

/// Serializes compositor *creation* across this binary's test threads. See
/// `headless_boot.rs`'s `BOOT_LOCK` for the full argument (the process-global,
/// unsynchronized `wl_array` of buffer-resource interfaces `init_graphics`
/// grows). Duplicated rather than shared because each integration-test file
/// is its own binary with its own statics.
static BOOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`BOOT_LOCK`] (and set the headless environment) for the duration of
/// one compositor's creation. `drop` the returned guard once the
/// display/backend/runtime triple exists.
fn boot_lock() -> std::sync::MutexGuard<'static, ()> {
    ensure_headless_env();
    BOOT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_second_headless_output_is_tracked_with_a_layout_box() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    // `state` is declared (and so, by ordinary end-of-scope drop order,
    // dropped) after `display`/`backend`/`runtime`, matching every
    // `headless_boot.rs` test: `attach` below gives `state.wayland` a
    // `Runtime` clone, and `wlr` documents that a `Runtime` must not outlive
    // the `Display` it was initialized against.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(1, 1, icedtea_compositor::render::wallpaper_color(&state.config.appearance))
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // The command channel and its wake pipe, used only as this test's
    // bounded backstop -- same role as `headless_boot.rs`'s `run_all` tests:
    // it gives both headless outputs time to arrive (and so
    // `OutputHandler::new_output` time to run twice) before asking the loop
    // to stop.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) = icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(state.quitting, "the backstop Quit command must have stopped the loop");

    assert_eq!(state.outputs.len(), 2, "both headless outputs must have reached the model");

    let geometries: Vec<_> = state.outputs.values().map(|o| o.geometry).collect();
    let a = geometries[0];
    let b = geometries[1];
    assert!(
        a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0,
        "both outputs report a real size, got {a:?} and {b:?}"
    );
    // Disjoint boxes are the proof the layout-box path (not the
    // (0,0)-at-origin fallback, which would stack both outputs on top of
    // each other) produced these geometries.
    let disjoint = a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y;
    assert!(disjoint, "the two outputs' layout boxes must not overlap, got {a:?} and {b:?}");
}
