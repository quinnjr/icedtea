//! The `org.icedtea.Clipboard` zbus service.
//!
//! Mirrors the compositor's `dbus.rs`: methods answer from a shared snapshot or
//! push a [`Command`] to the Wayland thread (nudging its wake pipe), and a
//! dedicated emitter thread turns each history [`Change`] into a
//! `history_changed` signal via the low-level `emit_signal`.

use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use icedtea_contract::{CLIP_BUS_NAME, CLIP_PATH, ClipEntry};
use zbus::blocking::Connection;
use zbus::interface;

use crate::history::Change;
use crate::manager::Command;

pub struct ClipboardService {
    snapshot: Arc<Mutex<Vec<ClipEntry>>>,
    commands: Sender<Command>,
    wake: UnixStream,
}

impl ClipboardService {
    fn push(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
        let mut wake = &self.wake;
        let _ = wake.write_all(&[1]);
    }
}

#[interface(name = "org.icedtea.Clipboard")]
impl ClipboardService {
    fn get_history(&self) -> Vec<ClipEntry> {
        self.snapshot.lock().unwrap().clone()
    }
    fn activate(&self, id: u64) {
        self.push(Command::Activate(id));
    }
    fn pin(&self, id: u64, on: bool) {
        self.push(Command::Pin(id, on));
    }
    fn remove(&self, id: u64) {
        self.push(Command::Remove(id));
    }
    fn clear(&self) {
        self.push(Command::Clear);
    }
}

/// Register `org.icedtea.Clipboard` and start the `history_changed` emitter.
/// Returns the connection, which the caller keeps alive for the service's life.
/// `Err` if the session bus is unavailable or the name is already owned (another
/// daemon running) — the binary treats that as fatal; a test can skip.
pub fn spawn(
    snapshot: Arc<Mutex<Vec<ClipEntry>>>,
    commands: Sender<Command>,
    wake: UnixStream,
    changes: Receiver<Change>,
) -> zbus::Result<Connection> {
    let conn = Connection::session()?;
    let service = ClipboardService {
        snapshot,
        commands,
        wake,
    };
    conn.object_server().at(CLIP_PATH, service)?;
    conn.request_name(CLIP_BUS_NAME)?;

    let emitter = conn.clone();
    std::thread::spawn(move || {
        let dest: Option<&str> = None;
        while changes.recv().is_ok() {
            let _ = emitter.emit_signal(dest, CLIP_PATH, CLIP_BUS_NAME, "history_changed", &());
        }
    });

    Ok(conn)
}
