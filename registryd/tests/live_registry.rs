//! A live D-Bus round-trip against the real `icedtea-registryd` binary spawned
//! as a subprocess — the same posture as `notifications/tests/live_notify.rs`.
//! This is the coverage no unit test can give: the `#[interface]` member
//! names, the `(y tag, ay payload)` wire shape, the `Changed` signal, and the
//! bus-name-conflict exit are otherwise only checked by hand.
//!
//! The child is given a temp `XDG_CONFIG_HOME`, so it owns its own
//! `registry.redb`. The test **skips** when the session bus is unavailable or
//! when another registry daemon already owns the name (in which case talking to
//! it could mutate a real user's settings).

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use icedtea_registry::{Registry, Value};

const BOOT_TIMEOUT: Duration = Duration::from_secs(10);
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn target_profile_dir() -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop();
    path.pop();
    path
}

fn daemon_binary() -> PathBuf {
    target_profile_dir().join("icedtea-registryd")
}

/// True when a registry daemon already answers on the session bus. If so, this
/// test must not run: its writes could land in a real user's store.
fn daemon_already_serving() -> bool {
    match Registry::connect() {
        Ok(registry) => registry.seq().is_ok(),
        Err(_) => false,
    }
}

fn wait_until_ready() -> Option<Registry> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(registry) = Registry::connect()
            && registry.seq().is_ok()
        {
            return Some(registry);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[test]
fn live_set_get_watch_and_spec() {
    // Skip visibly when there is no session bus at all.
    if Registry::connect().is_err() {
        eprintln!("skipping live_registry: no session bus available");
        return;
    }
    if daemon_already_serving() {
        eprintln!("skipping live_registry: a registry daemon already owns the name");
        return;
    }

    let config_home = tempfile::tempdir().expect("tempdir");
    let child = Command::new(daemon_binary())
        .env("XDG_CONFIG_HOME", config_home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn icedtea-registryd");
    let _guard = ChildGuard(child);

    let registry = wait_until_ready().expect("registry daemon became ready");

    // A registered key reads its schema default before anything is written.
    let (value, is_default) = registry
        .get("/org/icedtea/appearance/bar_height")
        .expect("get default");
    assert_eq!(value, Value::Uint(42));
    assert!(is_default);

    // Set it, read it back as stored.
    let seq = registry
        .set("/org/icedtea/appearance/bar_height", &Value::Uint(50))
        .expect("set");
    assert!(seq >= 1);
    let (value, is_default) = registry
        .get("/org/icedtea/appearance/bar_height")
        .expect("get stored");
    assert_eq!(value, Value::Uint(50));
    assert!(!is_default);

    // Unset reverts to the default.
    registry
        .unset("/org/icedtea/appearance/bar_height")
        .expect("unset");
    let (value, is_default) = registry
        .get("/org/icedtea/appearance/bar_height")
        .expect("get after unset");
    assert_eq!(value, Value::Uint(42));
    assert!(is_default);

    // A wrong-typed write is refused by the daemon.
    assert!(
        registry
            .set(
                "/org/icedtea/appearance/bar_height",
                &Value::Str("tall".into())
            )
            .is_err(),
        "daemon must reject a type-mismatched registered write"
    );

    // Unregistered keys are accepted and listed.
    registry
        .set("/apps/icedtea-registry-test/thing", &Value::Str("x".into()))
        .expect("set unregistered");
    let listed = registry
        .list("/apps/icedtea-registry-test", true)
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "/apps/icedtea-registry-test/thing");

    // Spec exists for a registered key, not for an unregistered one.
    assert!(
        registry
            .spec_json("/org/icedtea/appearance/bar_height")
            .unwrap()
            .is_some()
    );
    assert!(
        registry
            .spec_json("/apps/icedtea-registry-test/thing")
            .unwrap()
            .is_none()
    );

    // The Changed signal reaches a prefix watcher.
    let (tx, rx) = mpsc::channel();
    registry
        .watch(
            "/apps/icedtea-registry-test".to_string(),
            move |changes, seq| {
                let _ = tx.send((changes.to_vec(), seq));
            },
        )
        .expect("watch");
    // Give the watcher's thread a moment to install the signal match.
    std::thread::sleep(Duration::from_millis(200));
    registry
        .set("/apps/icedtea-registry-test/other", &Value::Uint(1))
        .expect("set to trigger watch");
    let (changes, _seq) = rx
        .recv_timeout(SIGNAL_TIMEOUT)
        .expect("a Changed signal for the watched prefix");
    assert!(
        changes
            .iter()
            .any(|(path, present)| path == "/apps/icedtea-registry-test/other" && *present),
        "the signal names the changed path: {changes:?}"
    );

    // Clean up the test's own keys.
    let _ = registry.reset("/apps/icedtea-registry-test");
}
