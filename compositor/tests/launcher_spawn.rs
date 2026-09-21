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
//!
//! Gap (still open): no live D-Bus round-trip exists for `SpawnApp` —
//! `compositor/tests/` has no `live_dbus.rs` harness (unlike
//! `shell/tests/live_dbus.rs` for the clipboard path), so nothing here
//! proves the `SpawnApp` member is exposed on the real bus with the
//! `(string) -> bool` signature the shell's `CompositorProxy` calls. Only
//! the model-level contract (`DbCommand::SpawnApp` -> bool reply) is
//! pinned. Building that harness is out of scope here; this comment is the
//! marker until one exists.

mod support;

use icedtea_compositor::state::State;
use icedtea_registry_schema::default_config;
use support::{assert_quiescent, seed_touch_entry, spawn_app, tmpdir, wait_for};

fn state_with_app_dir(dir: &std::path::Path) -> State {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.set_app_dirs(vec![dir.to_path_buf()]);
    state
}

#[test]
fn spawn_app_unknown_id_returns_false_and_spawns_nothing() {
    let dir = tmpdir("unknown");
    let sentinel = dir.join("should-not-exist");
    let mut state = state_with_app_dir(&dir);

    let ok = spawn_app(&mut state, "no-such-app");
    assert!(!ok, "unknown app id must reply false");

    assert_quiescent("unknown app id must spawn nothing", || !sentinel.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spawn_app_known_id_spawns_entry_exec() {
    let dir = tmpdir("known");
    let sentinel = dir.join("launched.ok");
    seed_touch_entry(&dir, "probe", &sentinel);
    let mut state = state_with_app_dir(&dir);

    let ok = spawn_app(&mut state, "probe");
    assert!(ok, "known app id must reply true");

    wait_for(&sentinel);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 6: one `State` serves the launcher's back-to-back commands, so a
/// failed lookup must leave later launches working — the e2e's launch half
/// runs on exactly this shared-state path.
#[test]
fn spawn_app_failure_leaves_later_launches_working() {
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

    wait_for(&sentinel);
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

    assert_quiescent(
        "unparseable Exec must spawn nothing (only broken.desktop may exist)",
        || std::fs::read_dir(&dir).expect("read app dir").count() == 1,
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
    // that pins the allowlist boundary.
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

    assert_quiescent("traversal ids must spawn nothing", || !sentinel.exists());
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
