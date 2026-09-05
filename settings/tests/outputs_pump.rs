//! The outputs pump against the harness compositor: the second Wayland
//! connection's fd is registered with the window, and readiness turns into
//! `Msg::Outputs`.

mod support;

use icedtea_settings::app::Msg;
use icedtea_settings::outputs::pump::OutputsPump;

/// Mutation check: make `OutputsPump::drain` return `Vec::new()`
/// unconditionally; this test fails. Restore.
#[test]
fn the_pump_registers_its_fd_and_drains_protocol_messages() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut window = support::open_test_window(&comp);
    let before = window.watches().len();

    let pump = OutputsPump::attach(&mut window)
        .expect("attach never fails once the connection is up")
        .expect("the harness advertises zwlr_output_manager_v1");

    assert_eq!(
        window.watches().len(),
        before + 1,
        "the pump's queue fd must be in the window's poll set"
    );
    assert!(window.watches().contains(&pump.watch()));

    // The connection roundtripped during `attach`, so an initial
    // HeadsChanged is already queued.
    let msgs = pump.drain();
    assert!(
        msgs.iter().any(|m| matches!(m, Msg::Outputs(u) if matches!(
            **u,
            icedtea_settings::outputs::OutputsMsg::HeadsChanged(_)
        ))),
        "expected an initial HeadsChanged, got {msgs:?}"
    );
}

/// The initial enumeration happens inside `attach`'s two roundtrips, before
/// any loop exists, and after it the socket is quiet — so `App::on_fd` would
/// never fire to deliver it. `main` seeds the loop with one `drain()` for
/// exactly this reason; here that seed is folded through `update` and must
/// reach the model.
///
/// This pins the fold half (a drained message reaching the model) and
/// `with_outputs`' `manager_present()`; the companion test below pins the
/// seeding itself, in the real binary.
#[test]
fn the_initial_enumeration_reaches_the_model_when_the_loop_is_seeded() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut window = support::open_test_window(&comp);
    let pump = OutputsPump::attach(&mut window)
        .expect("attach")
        .expect("the harness advertises zwlr_output_manager_v1");

    let dir = tempfile::tempdir().expect("tempdir");
    let (workers, _rx) = icedtea_settings::ipc::handles_for_test();
    let mut model =
        icedtea_settings::app::SettingsModel::new(dir.path().join("config.redb"), workers)
            .with_outputs(Some(pump.clone()));

    // `manager_present()`, not `is_some()` — the harness has the global, so
    // this is true; without it the page must start unavailable.
    assert!(pump.manager_present());
    assert!(model.outputs_available);

    for msg in pump.drain() {
        icedtea_settings::app::update(&mut model, msg);
    }
    assert!(
        !model.displays.heads.is_empty(),
        "the seeded enumeration must populate the Displays model"
    );
}

/// A pump whose compositor is gone: the fd stays permanently readable, so the
/// `Disconnected` fold must retire the watch, and the pump must latch so the
/// interval before that lands is silent instead of a warn-spamming spin.
///
/// Mutation check: drop the `dead` latch from `OutputsPump::drain`; the
/// second-drain assertion fails. Return `Cmd::None` from the `Disconnected`
/// arm; the `Cmd::Unwatch` assertion fails. Restore.
#[test]
fn a_dead_connection_latches_the_pump_and_unwatches_the_fd() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut window = support::open_test_window(&comp);
    let pump = OutputsPump::attach(&mut window)
        .expect("attach")
        .expect("the harness advertises zwlr_output_manager_v1");
    let _ = pump.drain(); // the initial enumeration

    drop(comp);

    // The socket closes asynchronously; poll until the read errors.
    let mut disconnected = None;
    for _ in 0..200 {
        let msgs = pump.drain();
        if let Some(msg) = msgs.into_iter().find(|m| {
            matches!(m, Msg::Outputs(u) if matches!(
                **u,
                icedtea_settings::outputs::OutputsMsg::Disconnected
            ))
        }) {
            disconnected = Some(msg);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let disconnected = disconnected.expect("a dead socket reports Disconnected");
    assert!(pump.is_dead(), "the first error latches the pump shut");
    assert!(
        pump.drain().is_empty(),
        "a latched pump never dispatches, warns or re-emits Disconnected again"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let (workers, _rx) = icedtea_settings::ipc::handles_for_test();
    let mut model =
        icedtea_settings::app::SettingsModel::new(dir.path().join("config.redb"), workers)
            .with_outputs(Some(pump.clone()));
    let cmd = icedtea_settings::app::update(&mut model, disconnected);
    assert!(
        matches!(cmd, icedtea_ui::view::Cmd::Unwatch(id) if id == pump.watch()),
        "Disconnected must retire the watch, or the loop spins on the dead fd forever"
    );
    assert!(!model.outputs_available);
}

/// The seeding in `main.rs`, in the shipped binary: the running app folds an
/// `Outputs` message with no input at all, purely from `attach`'s initial
/// enumeration.
///
/// Mutation check: delete the seeding block in `settings/src/main.rs`; no
/// `msg Outputs` line is ever reported and this test fails. Restore.
#[test]
fn the_real_binary_seeds_its_loop_with_the_initial_enumeration() {
    let comp = icedtea_harness::Compositor::spawn();
    let settings = support::spawn_settings(&comp);
    let line = settings.wait_line("msg Outputs", std::time::Duration::from_secs(20));
    assert!(
        line.is_some_and(|l| l.contains("HeadsChanged")),
        "the app must fold the initial enumeration without waiting for an fd wake; got {:?}",
        settings.lines()
    );
}
