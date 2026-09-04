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
