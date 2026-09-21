//! A live D-Bus round-trip proving the settings app's write path actually
//! reaches a running compositor: edit the on-disk config, ask the real
//! `org.icedtea.Compositor` service to reload it, and observe the resulting
//! `ConfigReloaded` signal carry the *new* value. Mirrors
//! `shell/tests/live_dbus.rs` (M4.6), but exercises
//! `settings::model::apply` and `settings::compositor_reload::ReloadClient` against
//! a real compositor instead of the clipboard client against a real
//! clipboard daemon.
//!
//! ## Why the real `icedtea-compositor` binary, not `icedtea_harness::Compositor`
//!
//! `icedtea_harness::Compositor::spawn` deliberately boots the compositor's
//! event loop with the D-Bus service, the config-reload worker/wake pipe,
//! and the wallpaper worker all left out (see that module's doc) -- it
//! wires only the `SeqEvent`/`DbCommand` channels a protocol test drives
//! directly. Concretely, it never calls `State::set_config_reload_sender`,
//! so `DbCommand::ReloadConfig` (`state.rs::handle_command`) is a no-op
//! against a harness compositor: nothing to spawn the off-loop reload
//! worker onto. That's the harness's deliberate scope, not a bug, and nothing
//! this task is allowed to change (icedtea-ONLY, no compositor/harness
//! source changes).
//!
//! So this test spawns the actual `icedtea-compositor` binary as a
//! subprocess -- the same `run()` boot path production uses, config reload
//! wiring included -- against a temp `XDG_CONFIG_HOME`, headless backend,
//! and drives it purely over the real session bus, the same way a real
//! `icedtea-settings` instance would talk to a real running compositor.
//!
//! Skipped (visibly) only when the session bus is unavailable or
//! `org.icedtea.Compositor` is already owned (a real compositor instance is
//! running) -- same posture as `shell/tests/live_dbus.rs`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_contract::{Appearance, COMPOSITOR_BUS_NAME, COMPOSITOR_IFACE, COMPOSITOR_PATH};
use icedtea_registry::REGISTRY_BUS_NAME;
use icedtea_settings::compositor_reload::{ReloadClient, ReloadOutcome};
use icedtea_settings::model;

const NEW_ACCENT: &str = "#ff00aa";
/// How long to wait for the subprocess compositor to boot and claim the bus
/// name -- generous because a debug build under test-suite load can be slow
/// to get past `wlr::Runtime::init_graphics`.
const BOOT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for `ConfigReloaded` once `ReloadConfig` has returned.
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Kills the child compositor process when dropped -- including on a test
/// panic, via unwind -- so a failing assertion never leaks a headless
/// compositor holding `org.icedtea.Compositor` into the next test run.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The directory this test binary itself was built into
/// (`target/<profile>/deps/`'s parent), which is also where cargo places
/// every workspace binary -- so `icedtea-compositor` sits right next to it
/// regardless of any `CARGO_TARGET_DIR` override.
fn target_profile_dir() -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop(); // deps/
    path.pop(); // debug/ or release/
    path
}

/// Path to the `icedtea-compositor` binary, building it first if this
/// profile directory doesn't have one yet -- a checkout that only ever
/// built/tested `icedtea-settings` (per this task's gate) has no reason to
/// already have it.
fn compositor_binary() -> PathBuf {
    let bin = target_profile_dir().join("icedtea-compositor");
    if !bin.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let status = Command::new(cargo)
            .args(["build", "-p", "icedtea-compositor"])
            .status()
            .expect("failed to invoke cargo to build icedtea-compositor");
        assert!(status.success(), "cargo build -p icedtea-compositor failed");
    }
    bin
}

/// Path to the `icedtea-registryd` binary, building it first if this profile
/// directory doesn't have one yet.
fn registryd_binary() -> PathBuf {
    let bin = target_profile_dir().join("icedtea-registryd");
    if !bin.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let status = Command::new(cargo)
            .args(["build", "-p", "icedtea-registryd"])
            .status()
            .expect("failed to invoke cargo to build icedtea-registryd");
        assert!(status.success(), "cargo build -p icedtea-registryd failed");
    }
    bin
}

/// Poll `org.freedesktop.DBus`'s `NameHasOwner` until `name` is owned or
/// `timeout` elapses.
fn wait_for_name_owner(conn: &zbus::blocking::Connection, name: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let owned = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(name,),
            )
            .ok()
            .and_then(|reply| reply.body().deserialize::<bool>().ok())
            .unwrap_or(false);
        if owned {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Editing the config and asking the compositor to reload applies it live:
/// a changed accent round-trips through save -> ReloadConfig -> ConfigReloaded.
#[test]
fn apply_reloads_the_running_compositor() {
    let probe = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("SKIP: no session bus available ({err})");
            return;
        }
    };
    // Non-blocking check (1ms budget): skip visibly rather than fight a
    // real compositor (or a leaked prior run) for the bus name.
    if wait_for_name_owner(&probe, COMPOSITOR_BUS_NAME, Duration::from_millis(1)) {
        eprintln!("SKIP: {COMPOSITOR_BUS_NAME} is already owned -- a real compositor is running");
        return;
    }
    if wait_for_name_owner(&probe, REGISTRY_BUS_NAME, Duration::from_millis(1)) {
        eprintln!(
            "SKIP: {REGISTRY_BUS_NAME} is already owned -- a real registry daemon is running"
        );
        return;
    }

    // A temp XDG_CONFIG_HOME, and a real registry daemon owning the store so
    // both the compositor child and this test process reach it over the bus.
    // (Opening the file directly from both sides would collide on redb's
    // exclusive lock -- which is exactly the bug the daemon exists to prevent.)
    let xdg_config_home = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(xdg_config_home.path().join("icedtea")).expect("create the config dir");

    let registryd = Command::new(registryd_binary())
        .env("XDG_CONFIG_HOME", xdg_config_home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn icedtea-registryd");
    let _registryd_guard = ChildGuard(registryd);
    assert!(
        wait_for_name_owner(&probe, REGISTRY_BUS_NAME, BOOT_TIMEOUT),
        "icedtea-registryd never registered {REGISTRY_BUS_NAME} within {BOOT_TIMEOUT:?}"
    );

    // spawn (the real) compositor; it finds the registry daemon on the bus.
    let child = Command::new(compositor_binary())
        .env("XDG_CONFIG_HOME", xdg_config_home.path())
        .env("WLR_BACKENDS", "headless")
        .env("WLR_HEADLESS_OUTPUTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn icedtea-compositor");
    let _guard = ChildGuard(child);

    assert!(
        wait_for_name_owner(&probe, COMPOSITOR_BUS_NAME, BOOT_TIMEOUT),
        "icedtea-compositor never registered {COMPOSITOR_BUS_NAME} within {BOOT_TIMEOUT:?}"
    );

    // subscribe to org.icedtea.Compositor ConfigReloaded (mirrors
    // shell/src/compositor_client.rs's signal subscription, blocking-style) before
    // triggering the reload, so the signal can't race ahead of us.
    let sub_conn = zbus::blocking::Connection::session().expect("session bus");
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(COMPOSITOR_IFACE)
        .expect("interface")
        .path(COMPOSITOR_PATH)
        .expect("path")
        .member("ConfigReloaded")
        .expect("member")
        .build();
    let mut iter = zbus::blocking::MessageIterator::for_match_rule(rule, &sub_conn, Some(4))
        .expect("subscribe to ConfigReloaded");
    let (sig_tx, sig_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Some(Ok(msg)) = iter.next() {
            let _ = sig_tx.send(msg.body().deserialize::<(u64, Appearance)>());
        }
    });

    // load -> set accent -> apply -> ReloadConfig, all through the daemon.
    let registry = icedtea_registry_schema::connect().expect("connect to the registry daemon");
    let mut cfg = icedtea_registry_schema::Config::load_or_default(&registry);
    assert_ne!(
        cfg.appearance.palette.accent, NEW_ACCENT,
        "test is only non-vacuous if the accent actually changes from the default"
    );
    cfg.appearance.palette.accent = NEW_ACCENT.to_string();
    model::apply(&cfg, &registry).expect("save the edited config to the temp registry");

    let client = ReloadClient::new();
    assert_eq!(
        client.reload(),
        ReloadOutcome::Reloaded,
        "ReloadConfig must reach the compositor subprocess"
    );

    // assert a ConfigReloaded arrives with appearance.palette.accent == new
    let (seq, appearance) = sig_rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .unwrap_or_else(|_| panic!("no ConfigReloaded signal arrived within {SIGNAL_TIMEOUT:?}"))
        .expect("ConfigReloaded body did not deserialize as (u64, Appearance)");
    assert!(
        seq > 0,
        "seq should be a real post-boot sequence number, got {seq}"
    );
    assert_eq!(
        appearance.palette.accent, NEW_ACCENT,
        "ConfigReloaded must carry the NEW accent, not the default -- a no-op reload or a \
         dropped write would leave this at the default #89b4fa instead"
    );
}
