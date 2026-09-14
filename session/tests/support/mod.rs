//! Shared test support for the `icedtea-session` integration tests.
//!
//! [`PrivateBus`] is a private `dbus-daemon` the calling test owns outright,
//! killed on drop. It mirrors the `settings` crate's hermetic-bus posture:
//! never the developer's live bus.
//!
//! Tests skip visibly when no `dbus-daemon` binary exists: callers print
//! [`SKIP_MARKER`] and return. **CI must provide `dbus-daemon` for these to
//! run.**

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long a freshly spawned `dbus-daemon` gets to print its address before
/// the caller gives up and treats the bus as unavailable.
const BUS_BOOT: Duration = Duration::from_secs(5);

/// The greppable visible-skip marker shared by the hermetic-bus tests.
///
/// `#[allow(dead_code)]` because each integration-test binary compiles its own
/// copy of this module: a test that skips with a literal marker instead of this
/// constant must not fail `-D warnings` on the version it does not use.
#[allow(dead_code)]
pub(crate) const SKIP_MARKER: &str = "LEXSKIP:";

/// A private session-bus instance the calling test owns, killed on drop.
pub(crate) struct PrivateBus {
    /// The bus address to hand to `zbus`.
    pub(crate) address: String,
    child: Child,
}

/// Spawn `dbus-daemon --session --nofork --print-address` and return the bus
/// once it prints an address. `None` if the binary is missing, exits, or does
/// not print within [`BUS_BOOT`].
///
/// The address read runs on a worker thread and is bounded by
/// [`mpsc::Receiver::recv_timeout`]: `BufRead::read_line` on the daemon's
/// stdout blocks forever if the daemon hangs, so a same-thread `elapsed()`
/// check would never be reached. The timeout is the only thing that actually
/// bounds startup.
pub(crate) fn spawn() -> Option<PrivateBus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let (tx, rx) = mpsc::channel();
    // Detached: if this times out we kill the child below, which closes its
    // stdout and makes the blocked `read_line` return EOF so the thread
    // finishes instead of leaking.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = tx.send(reader.read_line(&mut line).map(|n| (n, line)));
    });
    let address = match rx.recv_timeout(BUS_BOOT) {
        Ok(Ok((n, line))) if n > 0 => line.trim().to_string(),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    if address.is_empty() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(PrivateBus { address, child })
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
