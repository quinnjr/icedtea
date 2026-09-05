//! The portal worker's degradation proof.
//!
//! The bus address travels *per connection* (`portal::spawn_on_bus`), never
//! through `DBUS_SESSION_BUS_ADDRESS`: the environment is process-global, so
//! an `unsafe set_var` here doctored the value for every other test running
//! beside it in this binary — and on edition 2024 is undefined behaviour the
//! moment another thread reads the environment.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use icedtea_settings::app::Msg;
use icedtea_settings::ipc::portal::{self, PortalRequest};
use icedtea_ui::view::Inbox;

/// The hard ceiling: `PORTAL_CONNECT_TIMEOUT` (10s) plus room for a loaded
/// machine. It is only the *ceiling* — [`BUDGET`] is what these tests really
/// assert against, so "answers rather than hangs" stays a claim about speed
/// and not merely about eventually returning.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// What a refused connection to a socket that does not exist may take. The
/// address never resolves to a listener, so this is dominated by process
/// scheduling, not by any timeout in the worker.
const BUDGET: Duration = Duration::from_secs(15);

/// A bus address nothing is listening on.
const NO_SUCH_BUS: &str = "unix:path=/nonexistent/icedtea-no-such-bus";

fn wait_for_answer(inbox: &Inbox<Msg>) -> Msg {
    let started = Instant::now();
    while started.elapsed() < ANSWER_TIMEOUT {
        if let Some(msg) = inbox.try_recv() {
            let took = started.elapsed();
            assert!(
                took < BUDGET,
                "the worker answered, but only after {took:?} — a wedged                  portal must fail fast, not merely before the ceiling"
            );
            return msg;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the portal worker never answered within {ANSWER_TIMEOUT:?}");
}

#[test]
fn a_missing_session_bus_answers_with_a_picker_failure() {
    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let (req_tx, req_rx) = crossbeam_channel::unbounded();
    let worker = portal::spawn_on_bus(req_rx, tx, Some(NO_SUCH_BUS.to_string()))
        .expect("the portal worker thread starts");

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
    let worker = portal::spawn_on_bus(req_rx, tx, Some(NO_SUCH_BUS.to_string()))
        .expect("the portal worker thread starts");

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
