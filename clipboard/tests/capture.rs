//! The daemon end-to-end against a real headless compositor: a client copies
//! text, the data-control manager captures it into history, and a re-paste
//! (`Activate`) makes the daemon own the selection with the stored bytes —
//! without duplicating the head (the self-capture guard).

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_clipboard::history::History;
use icedtea_clipboard::manager::{self, Command};
use icedtea_harness::{Compositor, DataControlClient, TestClient, VirtualKeyboardClient};

/// Connect a fresh Wayland client to a running harness compositor by socket.
fn daemon_connection(socket: &str) -> wayland_client::Connection {
    let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let path = std::path::Path::new(&dir).join(socket);
    let stream = UnixStream::connect(&path).unwrap_or_else(|e| panic!("connect {}: {e}", path.display()));
    wayland_client::Connection::from_socket(stream).expect("wayland connection")
}

fn wait_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    pred()
}

#[test]
fn daemon_captures_a_copy_and_repastes_it() {
    let comp = Compositor::spawn();
    // Keep a keyboard on the seat so the copying client gets a set_selection serial.
    let mut _vk = VirtualKeyboardClient::spawn(&comp.socket);

    // Run the data-control manager against the compositor on its own thread.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (chg_tx, _chg_rx) = crossbeam_channel::unbounded();
    let (wake_read, wake_write) = UnixStream::pair().expect("wake pipe");
    let snapshot = Arc::new(Mutex::new(Vec::new()));
    let dconn = daemon_connection(&comp.socket);
    let snap = snapshot.clone();
    let daemon = std::thread::spawn(move || {
        manager::run(dconn, History::new(50), cmd_rx, chg_tx, wake_read, snap);
    });

    // A client copies text.
    let mut app = TestClient::map_toplevel(&comp.socket, "app.copy", "copier");
    assert!(app.wait_until(|c| c.has_input_serial()), "no keyboard serial");
    app.set_selection_text("text/plain;charset=utf-8", b"hello-history");

    // The daemon captures it into history. `app` must keep dispatching so its
    // data source answers the daemon's `receive` (the compositor forwards the
    // daemon's request to `app`'s source, on this thread).
    let captured = {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            app.pump();
            if snapshot.lock().unwrap().iter().any(|e| e.preview == "hello-history") {
                break true;
            }
            if Instant::now() > end {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    assert!(captured, "daemon never captured the copy; history = {:?}", snapshot.lock().unwrap());
    let (id, len_before) = {
        let s = snapshot.lock().unwrap();
        (s.iter().find(|e| e.preview == "hello-history").unwrap().id, s.len())
    };

    // Re-paste it: the daemon takes over the selection with a source it serves.
    cmd_tx.send(Command::Activate(id)).unwrap();
    {
        use std::io::Write as _;
        let mut w = &wake_write;
        w.write_all(&[1]).unwrap();
    }

    // A data-control reader now sees the re-pasted bytes (served by the daemon).
    let mut reader = DataControlClient::spawn(&comp.socket);
    assert!(reader.wait_until(|c| c.has_offer()), "reader saw no data-control offer");
    assert_eq!(
        reader.read_offer_blocking("text/plain;charset=utf-8"),
        b"hello-history",
        "re-paste did not serve the stored bytes"
    );

    // The self-capture guard held: re-pasting did not duplicate the head.
    assert!(
        wait_until(Duration::from_secs(2), || snapshot.lock().unwrap().len() == len_before)
            || snapshot.lock().unwrap().len() == len_before,
        "re-paste duplicated the history head (self-capture guard failed): {:?}",
        snapshot.lock().unwrap()
    );

    // Tear the daemon down cleanly (disconnecting it before the compositor).
    drop(reader);
    cmd_tx.send(Command::Quit).unwrap();
    {
        use std::io::Write as _;
        let mut w = &wake_write;
        let _ = w.write_all(&[1]);
    }
    let _ = daemon.join();
}
