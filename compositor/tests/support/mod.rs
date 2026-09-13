//! Shared helpers for the compositor's launcher integration binaries
//! (`launcher_spawn.rs`, `launcher_parity.rs`): temp dirs, `.desktop`
//! seeding, the `SpawnApp` round-trip, and absence polling.
//!
//! Each binary stands alone, so both declare `mod support;` (the same
//! `tests/support/mod.rs` precedent `shell/tests` uses). Not every binary
//! uses every helper, hence the blanket `dead_code` allow: this module is
//! compiled once per test binary that declares it.

#![allow(
    dead_code,
    reason = "shared test-support module compiled per test binary; not every binary uses every helper"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use icedtea_compositor::dbus::DbCommand;
use icedtea_compositor::state::State;

/// A temp dir unique per test: pid + tag + nonce, so parallel tests (and
/// reruns that outlive a previous cleanup) never share a directory.
pub fn tmpdir(tag: &str) -> PathBuf {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("icedtea-launcher-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("launcher temp dir");
    dir
}

/// Seed `dir/name.desktop` with `text`.
pub fn write_entry(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(format!("{name}.desktop")), text)
        .unwrap_or_else(|_| panic!("seed {name}.desktop"));
}

/// Seed `dir/name.desktop` with `Exec=touch "<sentinel>"`. The quotes keep
/// the sentinel one word under `parse_spawn_argv`'s shlex-style split even
/// when `temp_dir` carries spaces, so the test is hermetic w.r.t. the
/// runner's temp path.
///
/// No `touch`-absent skip: if coreutils is missing these tests FAIL loudly
/// (fail loud over green-but-empty).
pub fn seed_touch_entry(dir: &Path, name: &str, sentinel: &Path) {
    write_entry(
        dir,
        name,
        &format!(
            "[Desktop Entry]\nName={name}\nExec=touch \"{}\"\n",
            sentinel.display()
        ),
    );
}

/// Send `SpawnApp` and block for the round-trip reply, exactly as
/// `CompositorInterface::spawn_app` does (bounded channel, `GetState` shape).
pub fn spawn_app(state: &mut State, app_id: &str) -> bool {
    let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
    state.handle_command(DbCommand::SpawnApp {
        app_id: app_id.to_string(),
        reply: reply_tx,
    });
    reply_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("SpawnApp replies")
}

/// Block until `path` exists (a spawn side effect landing).
pub fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if Instant::now() >= deadline {
            panic!("expected spawn side effect at {path:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Poll-then-quiesce for a negative assertion: `cond` must hold on every
/// poll until the deadline, so a late side effect fails loudly instead of
/// slipping past a single fixed sleep.
pub fn assert_quiescent(desc: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        assert!(cond(), "{desc}");
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
