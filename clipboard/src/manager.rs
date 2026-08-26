//! The `wlr-data-control` client: watches the seat selection, captures text/uri
//! payloads into [`History`], and re-pastes a chosen entry by setting the
//! selection to a source it serves.
//!
//! Single-threaded: the Wayland connection and every protocol object live on
//! the loop thread. The zbus service reaches in only through the [`Command`]
//! channel, whose arrival is signalled on a wake pipe polled alongside the
//! Wayland fd — the same self-pipe idiom the compositor uses for its command
//! loop.

use std::io::Read as _;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use icedtea_contract::{ClipEntry, ClipKind};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as dev, zwlr_data_control_manager_v1 as mgr,
    zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as src,
};

use crate::history::{Change, History};

/// A request from the zbus service, delivered over the wake pipe.
#[derive(Debug)]
pub enum Command {
    Activate(u64),
    Pin(u64, bool),
    Remove(u64),
    Clear,
    /// Stop the loop and disconnect cleanly (used by tests to tear the daemon
    /// down before the compositor).
    Quit,
}

/// Mimes we capture, most-preferred first.
const MIMES: &[&str] = &["text/plain;charset=utf-8", "text/plain", "text/uri-list"];

fn kind_of(mime: &str) -> ClipKind {
    if mime == "text/uri-list" {
        ClipKind::Uris
    } else if mime.starts_with("image/") {
        ClipKind::Image
    } else {
        ClipKind::Text
    }
}

fn pick_mime(offered: &[String]) -> Option<String> {
    MIMES
        .iter()
        .find(|m| offered.iter().any(|o| o == *m))
        .map(|m| m.to_string())
}

struct App {
    manager: Option<mgr::ZwlrDataControlManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    device: Option<dev::ZwlrDataControlDeviceV1>,

    /// The offer currently advertised as the selection, plus its mimes.
    offer: Option<offer::ZwlrDataControlOfferV1>,
    offer_mimes: Vec<String>,
    /// Set when a new selection offer arrives; drained by the loop to capture.
    pending_capture: bool,

    /// The source we set for a re-paste, and the payload it answers `send` with.
    our_source: Option<src::ZwlrDataControlSourceV1>,
    our_serve: Option<(String, Vec<u8>)>,

    history: History,
    changes: Sender<Change>,
    /// The current history, mirrored for the zbus `get_history` reader.
    snapshot: Arc<Mutex<Vec<ClipEntry>>>,
}

impl App {
    /// Refresh the shared snapshot and signal that the history changed.
    fn publish(&self) {
        *self.snapshot.lock().unwrap() = self.history.snapshot();
        let _ = self.changes.send(Change::Changed);
    }

    fn ensure_device(&mut self, qh: &QueueHandle<App>) {
        if self.device.is_none()
            && let (Some(m), Some(s)) = (self.manager.as_ref(), self.seat.as_ref())
        {
            self.device = Some(m.get_data_device(s, qh, ()));
        }
    }

    /// Receive the current offer in the best supported mime and push it.
    ///
    /// Skips while we own the selection (a re-paste we set): the data is already
    /// the history head, and `receive()`-ing our own offer would ask this very
    /// thread to both write and read the same pipe — a self-deadlock. Ownership
    /// is dropped on the source's `Cancelled` (another client took over).
    fn capture(&mut self, conn: &Connection) {
        if self.our_source.is_some() {
            return;
        }
        let Some(off) = self.offer.clone() else {
            return;
        };
        let Some(mime) = pick_mime(&self.offer_mimes) else {
            return;
        };

        let (read_end, write_end) = match std::io::pipe() {
            Ok(p) => p,
            Err(_) => return,
        };
        off.receive(mime.clone(), write_end.as_fd());
        let _ = conn.flush();
        drop(write_end); // the compositor dups the write end to the owner

        // Read on a helper thread with a deadline, so a hung or malicious
        // selection owner that never writes/closes cannot wedge the loop (and
        // with it every queued command) forever.
        let (btx, brx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let mut read_end = read_end;
            let mut buf = Vec::new();
            let _ = read_end.read_to_end(&mut buf);
            let _ = btx.send(buf);
        });
        let bytes = match brx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => return, // timed out or empty; the read thread ends on its own
        };
        if self.history.push(kind_of(&mime), mime, &bytes, None) == Change::Changed {
            self.publish();
        }
    }

    fn handle(&mut self, cmd: Command, qh: &QueueHandle<App>, conn: &Connection) {
        let change = match cmd {
            Command::Activate(id) => {
                self.activate(id, qh, conn);
                // `activate` sets the selection; the resulting history state is
                // unchanged (no new entry), so no signal here.
                Change::Unchanged
            }
            Command::Pin(id, on) => self.history.pin(id, on),
            Command::Remove(id) => self.history.remove(id),
            Command::Clear => self.history.clear(),
            // Intercepted by the run loop before reaching here.
            Command::Quit => Change::Unchanged,
        };
        if change == Change::Changed {
            self.publish();
        }
    }

    /// Re-paste: own the selection with a source serving `id`'s stored bytes.
    fn activate(&mut self, id: u64, qh: &QueueHandle<App>, conn: &Connection) {
        let Some((mime, bytes)) = self.history.bytes_for(id) else {
            return;
        };
        let Some(manager) = self.manager.as_ref() else {
            return;
        };
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let source = manager.create_data_source(qh, ());
        source.offer(mime.clone());
        device.set_selection(Some(&source));
        self.our_serve = Some((mime, bytes));
        self.our_source = Some(source);
        // While `our_source` is set, `capture()` skips — so the compositor's
        // echo of this selection is not re-captured, and we never `receive()`
        // our own offer. Ownership is dropped on the source's `Cancelled`.
        let _ = conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "zwlr_data_control_manager_v1" => {
                    // Bind v1 deliberately: v2 also delivers primary-selection
                    // offers, which would race the shared `offer_mimes` against
                    // clipboard `Selection`s. The daemon only tracks the
                    // clipboard, so v1 is exactly the surface it needs.
                    state.manager = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                _ => {}
            }
            state.ensure_device(qh);
        }
    }
}

impl Dispatch<dev::ZwlrDataControlDeviceV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &dev::ZwlrDataControlDeviceV1,
        event: dev::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            dev::Event::DataOffer { .. } => state.offer_mimes.clear(),
            dev::Event::Selection { id } => {
                // `pending_capture` stays false on a cleared selection.
                state.offer = id;
                state.pending_capture = state.offer.is_some();
            }
            _ => {}
        }
    }

    event_created_child!(App, dev::ZwlrDataControlDeviceV1, [
        dev::EVT_DATA_OFFER_OPCODE => (offer::ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<offer::ZwlrDataControlOfferV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &offer::ZwlrDataControlOfferV1,
        event: offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event {
            state.offer_mimes.push(mime_type);
        }
    }
}

impl Dispatch<src::ZwlrDataControlSourceV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &src::ZwlrDataControlSourceV1,
        event: src::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            src::Event::Send { mime_type, fd } => {
                if let Some((mime, bytes)) = state.our_serve.as_ref()
                    && &mime_type == mime
                {
                    use std::io::Write as _;
                    let mut f = std::fs::File::from(fd);
                    let _ = f.write_all(bytes);
                }
            }
            src::Event::Cancelled => {
                state.our_source = None;
                state.our_serve = None;
            }
            _ => {}
        }
    }
}

// wayland-client needs a `Dispatch` for every bound interface; the manager and
// seat carry nothing we act on beyond binding.
wayland_client::delegate_noop!(App: ignore mgr::ZwlrDataControlManagerV1);
wayland_client::delegate_noop!(App: ignore wl_seat::WlSeat);

/// Drive the data-control loop: dispatch Wayland events and drain `commands`
/// (woken via `wake_read`) until the connection dies. Owns `history`.
pub fn run(
    conn: Connection,
    history: History,
    commands: Receiver<Command>,
    changes: Sender<Change>,
    wake_read: UnixStream,
    snapshot: Arc<Mutex<Vec<ClipEntry>>>,
) {
    let mut queue = conn.new_event_queue::<App>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut app = App {
        manager: None,
        seat: None,
        device: None,
        offer: None,
        offer_mimes: Vec::new(),
        pending_capture: false,
        our_source: None,
        our_serve: None,
        history,
        changes,
        snapshot,
    };

    // Settle the registry + device.
    let _ = queue.roundtrip(&mut app);
    app.ensure_device(&qh);
    let _ = queue.roundtrip(&mut app);

    loop {
        // Flush and run any already-queued events first.
        let _ = queue.flush();
        if queue.dispatch_pending(&mut app).is_err() {
            break;
        }
        if app.pending_capture {
            app.pending_capture = false;
            app.capture(&conn);
        }

        // Block until either fd is readable.
        let mut fds = [
            rustix::event::PollFd::new(&conn, rustix::event::PollFlags::IN),
            rustix::event::PollFd::new(&wake_read, rustix::event::PollFlags::IN),
        ];
        if rustix::event::poll(&mut fds, None).is_err() {
            break;
        }

        if fds[0].revents().contains(rustix::event::PollFlags::IN) {
            // Read Wayland events into the queue.
            if let Some(guard) = queue.prepare_read() {
                let _ = guard.read();
            }
            if queue.dispatch_pending(&mut app).is_err() {
                break;
            }
            if app.pending_capture {
                app.pending_capture = false;
                app.capture(&conn);
            }
        }

        if fds[1].revents().contains(rustix::event::PollFlags::IN) {
            // Drain the wake byte(s) and every queued command.
            drain_wake(&wake_read);
            while let Ok(cmd) = commands.try_recv() {
                if matches!(cmd, Command::Quit) {
                    let _ = conn.flush();
                    return;
                }
                app.handle(cmd, &qh, &conn);
            }
        }
    }
}

fn drain_wake(sock: &UnixStream) {
    use std::io::Read as _;
    let mut buf = [0u8; 64];
    let mut sock = sock;
    // Best-effort drain; one read clears the readable state for our purposes.
    let _ = sock.read(&mut buf);
}
