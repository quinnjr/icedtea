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

/// Whether the `touch` binary is on `PATH`, mirroring `xwayland.rs`'s
/// `xwayland_on_path` skip pattern: the spawn-positive tests exec `touch`
/// as the entry's program, so without it they skip visibly instead of
/// failing. Set `REQUIRE_TOUCH=1` (in CI, where coreutils IS installed)
/// to turn that skip into a loud failure, so a broken provisioning step
/// can never masquerade as a passing spawn suite.
fn touch_on_path() -> bool {
    let present = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join("touch").is_file()))
        .unwrap_or(false);
    if !present && std::env::var_os("REQUIRE_TOUCH").is_some() {
        panic!(
            "REQUIRE_TOUCH is set but the `touch` binary is not on PATH; \
             the SpawnApp spawn-positive tests cannot run and must not be reported as passing"
        );
    }
    present
}

/// Seed `dir/name.desktop` with `Exec=touch "<sentinel>"`. The quotes keep
/// the sentinel one word under `parse_spawn_argv`'s shlex-style split even
/// when `temp_dir` carries spaces, so the test is hermetic w.r.t. the
/// runner's temp path.
fn seed_touch_entry(dir: &std::path::Path, name: &str, sentinel: &std::path::Path) {
    std::fs::write(
        dir.join(format!("{name}.desktop")),
        format!(
            "[Desktop Entry]\nName={name}\nExec=touch \"{}\"\n",
            sentinel.display()
        ),
    )
    .unwrap_or_else(|_| panic!("seed {name}.desktop"));
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
    if !touch_on_path() {
        eprintln!("SKIP: `touch` is not on PATH; the SpawnApp spawn-positive test cannot run");
        return;
    }
    let dir = tmpdir("known");
    let sentinel = dir.join("launched.ok");
    seed_touch_entry(&dir, "probe", &sentinel);
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

/// Task 6: one `State` serves the launcher's back-to-back commands, so a
/// failed lookup must leave later launches working — the e2e's launch half
/// runs on exactly this shared-state path.
#[test]
fn spawn_app_failure_leaves_later_launches_working() {
    if !touch_on_path() {
        eprintln!("SKIP: `touch` is not on PATH; the SpawnApp spawn-positive test cannot run");
        return;
    }
    let dir = tmpdir("back-to-back");
    let sentinel = dir.join("launched.ok");
    seed_touch_entry(&dir, "probe", &sentinel);
    let mut state = state_with_app_dir(&dir);

    assert!(
        !spawn_app(&mut state, "no-such-app"),
        "unknown app id must reply false"
    );
    assert!(
        spawn_app(&mut state, "probe"),
        "a later known app id must still reply true"
    );

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

#[test]
fn spawn_app_rejects_traversal_and_non_stem_ids_without_spawning() {
    let dir = tmpdir("traversal");
    let sentinel = dir.join("escaped.ok");
    // A live entry the traversal ids below must NOT be able to reach:
    // `sub/../evil` resolves through the filesystem to this file, so it
    // replies `true` (and touches the sentinel) the moment the file-stem
    // charset guard in `lookup_app_exec` is loosened -- the loud failure
    // that pins the allowlist boundary. (That loudness needs `touch` on
    // PATH; without it the replies below still pin the guard, but the
    // sentinel check is vacuous -- hence no skip here, unlike the
    // spawn-positive test above.)
    seed_touch_entry(&dir, "evil", &sentinel);
    // `sub/` exists so `sub/../evil` resolves through the filesystem to
    // the entry above -- without the guard it would reply `true`.
    std::fs::create_dir_all(dir.join("sub")).expect("seed sub dir");
    let mut state = state_with_app_dir(&dir);

    for bad_id in [
        "",
        ".",
        "..",
        "../evil",
        "sub/../evil",
        "/usr/share/applications/probe",
        "a/b",
        "a\\b",
    ] {
        assert!(
            !spawn_app(&mut state, bad_id),
            "traversal/non-stem id {bad_id:?} must reply false"
        );
    }

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !sentinel.exists(),
        "traversal ids must spawn nothing, yet {sentinel:?} exists"
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read app dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["evil.desktop".to_string(), "sub".to_string()],
        "traversal ids must create no new files"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
