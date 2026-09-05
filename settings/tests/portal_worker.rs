//! The portal worker's degradation proof.
//!
//! Owns its own test binary because it edits `DBUS_SESSION_BUS_ADDRESS`,
//! which is process-global: a second test running concurrently in the same
//! binary would see the doctored value. Same reasoning the deleted
//! `appearance_gtk.rs` gave for GTK's one-init-per-process rule.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use icedtea_settings::app::Msg;
use icedtea_settings::ipc::portal::{self, PortalRequest};
use icedtea_ui::view::Inbox;

/// Generous: this asserts "the worker answers rather than hanging", not how
/// fast a failed bus connection is refused.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

fn wait_for_answer(inbox: &Inbox<Msg>) -> Msg {
    let started = Instant::now();
    while started.elapsed() < ANSWER_TIMEOUT {
        if let Some(msg) = inbox.try_recv() {
            return msg;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the portal worker never answered within {ANSWER_TIMEOUT:?}");
}

#[test]
fn a_missing_session_bus_answers_with_a_picker_failure() {
    // SAFETY: one test per binary, set before the worker thread starts, and
    // never read by anything else in this process.
    unsafe {
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/icedtea-no-such-bus",
        );
    }

    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let (req_tx, req_rx) = crossbeam_channel::unbounded();
    let worker = portal::spawn(req_rx, tx);

    req_tx
        .send(PortalRequest::OpenFile { current: None })
        .expect("queue the request");

    match wait_for_answer(&inbox) {
        Msg::WallpaperPickerFailed(reason) => {
            assert!(
                !reason.is_empty(),
                "the failure carries a message for the status line"
            );
        }
        other => panic!("expected a picker failure, got {other:?}"),
    }

    drop(req_tx);
    worker
        .join()
        .expect("the worker exits when its queue closes");
}

#[test]
fn a_request_queued_behind_another_supersedes_it() {
    // Two requests queued before the worker can pick either up: the worker
    // must answer once, for the *last* one, not twice.
    unsafe {
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/icedtea-no-such-bus",
        );
    }
    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let (req_tx, req_rx) = crossbeam_channel::unbounded();
    req_tx
        .send(PortalRequest::OpenFile { current: None })
        .expect("first");
    req_tx
        .send(PortalRequest::OpenFile {
            current: Some(PathBuf::from("/tmp")),
        })
        .expect("second");
    let worker = portal::spawn(req_rx, tx);

    let first = wait_for_answer(&inbox);
    assert!(matches!(first, Msg::WallpaperPickerFailed(_)));

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        inbox.try_recv().is_none(),
        "the superseded request produced no second answer"
    );

    drop(req_tx);
    worker.join().expect("the worker exits");
}
