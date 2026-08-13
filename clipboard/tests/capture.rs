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

    // A client copies two distinct payloads in turn. `app` must keep
    // dispatching so its data source answers the daemon's `receive` (the
    // compositor forwards the daemon's request to `app`'s source, on this
    // thread), so the wait loop pumps it.
    let mut app = TestClient::map_toplevel(&comp.socket, "app.copy", "copier");
    assert!(app.wait_until(|c| c.has_input_serial()), "no keyboard serial");

    let copy_and_wait = |app: &mut TestClient, text: &[u8]| {
        app.set_selection_text("text/plain;charset=utf-8", text);
        let preview = String::from_utf8_lossy(text).to_string();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            app.pump();
            if snapshot.lock().unwrap().iter().any(|e| e.preview == preview) {
                return;
            }
            assert!(Instant::now() < end, "daemon never captured {preview:?}: {:?}", snapshot.lock().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    copy_and_wait(&mut app, b"first");
    copy_and_wait(&mut app, b"second");

    // History is now [second, first]; the older entry is the re-paste target,
    // so a broken self-capture guard would insert a *new* head (or self-
    // deadlock) rather than leave the two-entry set intact.
    let (first_id, len_before) = {
        let s = snapshot.lock().unwrap();
        assert_eq!(s.len(), 2, "expected two distinct entries, got {s:?}");
        (s.iter().find(|e| e.preview == "first").unwrap().id, s.len())
    };

    // Re-paste the OLDER entry: the daemon owns the selection with a source it serves.
    cmd_tx.send(Command::Activate(first_id)).unwrap();
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
        b"first",
        "re-paste did not serve the older entry's bytes"
    );

    // The self-capture guard held: re-pasting neither duplicated an entry nor
    // grew the history. Give it a moment in case a stray capture were queued.
    std::thread::sleep(Duration::from_millis(200));
    {
        let s = snapshot.lock().unwrap();
        assert_eq!(s.len(), len_before, "re-paste changed the history (self-capture guard failed): {s:?}");
        assert_eq!(
            s.iter().filter(|e| e.preview == "first").count(),
            1,
            "re-paste duplicated the older entry: {s:?}"
        );
    }

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
