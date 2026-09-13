//! Task 4: `SpawnApp` allowlist D-Bus method.
//!
//! Model-level coverage (the `end_to_end.rs` pattern): drives the real
//! `State::handle_command` exactly as the D-Bus service would, without a
//! real Wayland display, event loop, or session bus.
//!
//! - Unknown app id replies `false` and spawns nothing.
//! - A seeded `.desktop` dir resolves id -> entry -> exec, replies `true`,
//!   and the entry's program actually runs (proven by a sentinel file).
//! - An entry whose `Exec` does not word-split replies `false` and spawns
//!   nothing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use icedtea_compositor::dbus::DbCommand;
use icedtea_compositor::state::State;
use icedtea_config::default_config;

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("icedtea-spawnapp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("spawnapp temp dir");
    dir
}

fn state_with_app_dir(dir: &std::path::Path) -> State {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.set_app_dirs(vec![dir.to_path_buf()]);
    state
}

/// Send `SpawnApp` and block for the round-trip reply, exactly as
/// `CompositorInterface::spawn_app` does (bounded channel, `GetState` shape).
fn spawn_app(state: &mut State, app_id: &str) -> bool {
    let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
    state.handle_command(DbCommand::SpawnApp {
        app_id: app_id.to_string(),
        reply: reply_tx,
    });
    reply_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("SpawnApp replies")
}

#[test]
fn spawn_app_unknown_id_returns_false_and_spawns_nothing() {
    let dir = tmpdir("unknown");
    let sentinel = dir.join("should-not-exist");
    let mut state = state_with_app_dir(&dir);

    let ok = spawn_app(&mut state, "no-such-app");
    assert!(!ok, "unknown app id must reply false");

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !sentinel.exists(),
        "unknown app id must spawn nothing, yet {sentinel:?} exists"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spawn_app_known_id_spawns_entry_exec() {
    let dir = tmpdir("known");
    let sentinel = dir.join("launched.ok");
    // Direct argv exec, no shell: `touch <sentinel>` word-splits cleanly.
    // (`sentinel` comes from `temp_dir`, which carries no spaces here, so
    // the shlex-style split keeps it one word.)
    std::fs::write(
        dir.join("probe.desktop"),
        format!(
            "[Desktop Entry]\nName=Probe\nExec=touch {}\n",
            sentinel.display()
        ),
    )
    .expect("seed probe.desktop");
    let mut state = state_with_app_dir(&dir);

    let ok = spawn_app(&mut state, "probe");
    assert!(ok, "known app id must reply true");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !sentinel.exists() {
        if Instant::now() >= deadline {
            panic!("SpawnApp replied true but the entry's program never ran ({sentinel:?} absent)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spawn_app_unparseable_exec_returns_false_and_spawns_nothing() {
    let dir = tmpdir("unparseable");
    // `""` is a non-empty `Exec` (so the entry parses) that word-splits to
    // zero words (so `parse_spawn_argv` returns `None`).
    std::fs::write(
        dir.join("broken.desktop"),
        "[Desktop Entry]\nName=Broken\nExec=\"\"\n",
    )
    .expect("seed broken.desktop");
    let mut state = state_with_app_dir(&dir);

    let ok = spawn_app(&mut state, "broken");
    assert!(!ok, "unparseable Exec must reply false");

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        std::fs::read_dir(&dir).expect("read app dir").count() == 1,
        "unparseable Exec must spawn nothing (only broken.desktop may exist)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
