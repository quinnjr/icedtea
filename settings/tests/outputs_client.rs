//! End-to-end test of the GTK-free `outputs/` protocol client against the
//! `icedtea-harness` compositor (which advertises `zwlr_output_manager_v1`).
//!
//! No GTK, gio, or glib here: the [`OutputsConnection`] is driven with explicit
//! roundtrips, proving the protocol layer stands on its own. The glib source in
//! `outputs::client` is for the real app.

use icedtea_harness::Compositor;
use icedtea_settings::outputs::{HeadEdit, ModeRequest, OutputsConnection, OutputsMsg};

/// Drain every message currently queued on the receiver.
fn drain(rx: &async_channel::Receiver<OutputsMsg>) -> Vec<OutputsMsg> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

#[test]
fn enumerates_heads_and_applies_a_configuration() {
    let comp = Compositor::spawn();
    let (tx, rx) = async_channel::unbounded::<OutputsMsg>();

    let mut client = OutputsConnection::connect_to_path(comp.socket_path(), tx)
        .expect("connect outputs client to harness socket");

    // The harness compositor advertises the manager global.
    assert!(
        client.manager_present(),
        "harness should advertise zwlr_output_manager_v1"
    );

    // A `done` fired during construction, so a HeadsChanged is already queued.
    let initial = drain(&rx);
    assert!(
        initial
            .iter()
            .any(|m| matches!(m, OutputsMsg::HeadsChanged(_))),
        "expected an initial HeadsChanged, got {initial:?}"
    );

    let heads = client.heads();
    assert!(
        !heads.is_empty(),
        "harness (WLR_HEADLESS_OUTPUTS=1) should enumerate at least one head"
    );
    let head = &heads[0];
    assert!(
        !head.modes.is_empty(),
        "head {} should advertise at least one mode, got {head:?}",
        head.name
    );

    // Pick a mode to (re-)assert: the current one if the head reports it, else
    // the first advertised mode.
    let target = head.current_mode.unwrap_or(head.modes[0]);
    let edits = vec![HeadEdit {
        name: head.name.clone(),
        enabled: true,
        mode: Some(ModeRequest {
            width: target.width,
            height: target.height,
            refresh_mhz: target.refresh_mhz,
        }),
        position: Some((0, 0)),
        scale: None,
        transform: None,
    }];

    client
        .build_and_send_configuration(&edits)
        .expect("build/send configuration");

    // Pump until the apply result comes back (bounded — a hung compositor
    // should fail the test, not spin forever).
    let mut result = None;
    for _ in 0..50 {
        client.roundtrip().expect("roundtrip");
        for msg in drain(&rx) {
            match msg {
                OutputsMsg::ApplySucceeded { is_test } => {
                    assert!(!is_test, "build_and_send_configuration is an apply, not a test");
                    result = Some(Ok(()));
                }
                OutputsMsg::ApplyFailed { .. } => result = Some(Err("failed")),
                OutputsMsg::ApplyCancelled => result = Some(Err("cancelled")),
                _ => {}
            }
        }
        if result.is_some() {
            break;
        }
    }

    match result {
        Some(Ok(())) => {}
        Some(Err(kind)) => panic!("apply did not succeed: {kind}"),
        None => panic!("no apply result arrived from the compositor"),
    }
}
