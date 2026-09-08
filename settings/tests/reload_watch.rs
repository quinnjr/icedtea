//! Hermetic coverage for the `ConfigReloaded` signal watcher
//! (`settings/src/ipc/reload.rs`): the `MatchRule` (interface + path), the
//! `member.as_str() == "ConfigReloaded"` filter, and the
//! `ReloadRequest`-free path that turns a matching signal into
//! `Msg::ConfigReloaded` on the inbox.
//!
//! It mirrors `settings/tests/interaction.rs`'s `PrivateBus` posture: a
//! private `dbus-daemon` this test owns outright, so the developer's real
//! session bus is never touched. It skips visibly (a greppable `LEXSKIP:`
//! marker) when no `dbus-daemon` is on `PATH` — **CI must provide
//! `dbus-daemon` for these gates to actually run.**
//!
//! The watcher is reached through the `spawn_watch_on_bus` seam added for this
//! test (analogous to the existing `spawn_worker_on_bus`): production always
//! passes `None` (the session bus); here we pass the private bus address.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH};
use icedtea_settings::app::Msg;
use icedtea_settings::ipc::reload::spawn_watch_on_bus;
use icedtea_ui::view::Inbox;

/// How long a freshly spawned `dbus-daemon` gets to print its address.
const BUS_BOOT: Duration = Duration::from_secs(5);
/// How long to keep re-emitting / polling before giving up on a signal that
/// should arrive (subscription setup is asynchronous, so early emits are lost
/// and must be retried until one lands).
const REACT: Duration = Duration::from_secs(10);

/// A private session-bus instance this test owns outright, killed on drop.
struct PrivateBus {
    address: String,
    child: Child,
}

impl PrivateBus {
    /// Spawn `dbus-daemon --session --nofork --print-address` and read its
    /// first line of stdout as the bus address. `None` if the binary is
    /// missing or never prints one within [`BUS_BOOT`].
    fn spawn() -> Option<PrivateBus> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let started = Instant::now();
        if reader.read_line(&mut line).ok()? == 0 || started.elapsed() > BUS_BOOT {
            let _ = child.kill();
            return None;
        }
        let address = line.trim().to_string();
        if address.is_empty() {
            let _ = child.kill();
            return None;
        }
        Some(PrivateBus { address, child })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A connection to `address` that can emit signals with an arbitrary member
/// name on `org.icedtea.Compositor` at `COMPOSITOR_PATH`. The watcher never
/// deserialises the body, so it is always the empty tuple.
struct Emitter {
    conn: zbus::blocking::Connection,
}

impl Emitter {
    fn open(address: &str) -> Emitter {
        let conn = zbus::blocking::connection::Builder::address(address)
            .expect("address is well formed")
            .build()
            .expect("connect to the private bus");
        Emitter { conn }
    }

    /// Emit one signal named `member` on the compositor interface/path.
    fn emit(&self, member: &str) {
        let dest: Option<&str> = None;
        self.conn
            .emit_signal(dest, COMPOSITOR_PATH, COMPOSITOR_BUS_NAME, member, &())
            .expect("emit the signal");
    }
}

/// Re-emit `member` until the inbox produces a message or [`REACT`] elapses.
/// Returns the first message, or `None` on timeout.
fn emit_until_received(emitter: &Emitter, member: &str, inbox: &Inbox<Msg>) -> Option<Msg> {
    let deadline = Instant::now() + REACT;
    while Instant::now() < deadline {
        emitter.emit(member);
        std::thread::sleep(Duration::from_millis(50));
        if let Some(msg) = inbox.try_recv() {
            return Some(msg);
        }
    }
    None
}

/// A `ConfigReloaded` signal on the compositor interface/path reaches the
/// inbox as `Msg::ConfigReloaded`.
///
/// Mutation check: change `CONFIG_RELOADED` in `reload.rs` to any other string;
/// the filter no longer matches and this times out. Restore.
#[test]
fn a_config_reloaded_signal_reaches_the_inbox() {
    let Some(bus) = PrivateBus::spawn() else {
        eprintln!("LEXSKIP: a_config_reloaded_signal_reaches_the_inbox skipped — no dbus-daemon");
        return;
    };
    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let _watch = spawn_watch_on_bus(tx, Some(bus.address.clone()))
        .expect("the ConfigReloaded watcher thread starts");

    let emitter = Emitter::open(&bus.address);
    match emit_until_received(&emitter, "ConfigReloaded", &inbox) {
        Some(Msg::ConfigReloaded) => {}
        other => panic!("expected Msg::ConfigReloaded, got {other:?}"),
    }
}

/// A signal on the same interface/path but a **different member** is delivered
/// to the watcher's stream (the `MatchRule` matches on interface + path only)
/// yet is *not* forwarded — the `member.as_str() == "ConfigReloaded"` filter
/// drops it.
///
/// Mutation check: replace the member filter with an unconditional `true`; the
/// wrong-member signal then forwards and the "stays empty" assertion fails.
/// Restore.
#[test]
fn a_signal_with_a_different_member_is_not_forwarded() {
    let Some(bus) = PrivateBus::spawn() else {
        eprintln!(
            "LEXSKIP: a_signal_with_a_different_member_is_not_forwarded skipped — no dbus-daemon"
        );
        return;
    };
    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let _watch = spawn_watch_on_bus(tx, Some(bus.address.clone()))
        .expect("the ConfigReloaded watcher thread starts");

    let emitter = Emitter::open(&bus.address);

    // First prove the subscription is live and drain the confirming message,
    // so the silence below is attributable to the filter, not a watcher that
    // never subscribed.
    assert!(
        matches!(
            emit_until_received(&emitter, "ConfigReloaded", &inbox),
            Some(Msg::ConfigReloaded)
        ),
        "the subscription never came up — cannot test the filter"
    );
    while inbox.try_recv().is_some() {}

    // Now the wrong member, five times, then exactly one correct "fence"
    // signal. The bus delivers signals in emission order on a single
    // connection, so the fence arrives at the watcher strictly after all five
    // wrong-member signals: when the fence lands, every wrong-member signal
    // ahead of it has already been processed. The fence is emitted exactly
    // once, so at most one legitimate `Msg::ConfigReloaded` can exist — any
    // *extra* message in the inbox is a wrongly-forwarded wrong-member signal.
    for _ in 0..5 {
        emitter.emit("SomethingElse");
    }
    emitter.emit("ConfigReloaded");

    let deadline = Instant::now() + REACT;
    let mut first = None;
    while Instant::now() < deadline {
        if let Some(msg) = inbox.try_recv() {
            first = Some(msg);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        matches!(first, Some(Msg::ConfigReloaded)),
        "the fence signal never arrived: {first:?}"
    );
    // Give any wrongly-forwarded wrong-member signals (which would sit behind
    // the fence, having been emitted first) time to land, then assert none did.
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        inbox.try_recv().is_none(),
        "a non-ConfigReloaded member must not be forwarded"
    );
}
