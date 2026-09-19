//! Hermetic tests for the session launcher helpers. No systemd, no D-Bus,
//! no Wayland: `busctl`/`systemctl` are faked on `PATH`, and the Wayland
//! socket is a real `UnixListener` in a temp runtime dir.

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;

fn wait_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("launch/icedtea-wait")
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("icedtea-wait-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A fake executable that records its argv (one arg per line) to `out`.
fn fake_tool(dir: &Path, name: &str, out: &Path, exit: i32) {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\nexit {exit}\n",
            out.display()
        ),
    )
    .expect("write fake tool");
    let mut perm = fs::metadata(&path).expect("stat").permissions();
    use std::os::unix::fs::PermissionsExt as _;
    perm.set_mode(0o755);
    fs::set_permissions(&path, perm).expect("chmod");
}

#[test]
fn wayland_mode_succeeds_once_a_socket_exists() {
    let runtime = tmpdir("wayland-ok");
    let _listener = UnixListener::bind(runtime.join("wayland-9")).expect("bind socket");
    let out = Command::new(wait_script())
        .arg("wayland")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("ICEDTEA_WAIT_TIMEOUT", "2")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "wayland-9");
    let _ = fs::remove_dir_all(&runtime);
}

#[test]
fn wayland_mode_times_out_with_a_message() {
    let runtime = tmpdir("wayland-none");
    let out = Command::new(wait_script())
        .arg("wayland")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("ICEDTEA_WAIT_TIMEOUT", "1")
        .output()
        .expect("run");
    assert!(!out.status.success(), "an absent socket must fail the unit");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no Wayland socket"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&runtime);
}

#[test]
fn name_mode_matches_a_bus_owner() {
    let dir = tmpdir("name-ok");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    // `busctl --user list --no-legend` prints one name per line, first column.
    fs::write(
        tools.join("busctl"),
        "#!/bin/sh\nprintf 'org.freedesktop.DBus  42  :1.1\\n'\nprintf 'org.icedtea.WM  7  :1.2\\n'\n",
    )
    .expect("fake busctl");
    use std::os::unix::fs::PermissionsExt as _;
    let mut perm = fs::metadata(tools.join("busctl"))
        .expect("stat")
        .permissions();
    perm.set_mode(0o755);
    fs::set_permissions(tools.join("busctl"), perm).expect("chmod");
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(wait_script())
        .arg("name")
        .arg("org.icedtea.WM")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", path)
        .env("ICEDTEA_WAIT_TIMEOUT", "2")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn name_mode_times_out_when_unowned() {
    let dir = tmpdir("name-none");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    fs::write(
        tools.join("busctl"),
        "#!/bin/sh\nprintf 'org.freedesktop.DBus  42  :1.1\\n'\n",
    )
    .expect("fake busctl");
    use std::os::unix::fs::PermissionsExt as _;
    let mut perm = fs::metadata(tools.join("busctl"))
        .expect("stat")
        .permissions();
    perm.set_mode(0o755);
    fs::set_permissions(tools.join("busctl"), perm).expect("chmod");
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(wait_script())
        .arg("name")
        .arg("org.icedtea.Absent")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", path)
        .env("ICEDTEA_WAIT_TIMEOUT", "1")
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not owned"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn wayland_env_mode_publishes_the_socket_name() {
    let dir = tmpdir("env");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let _listener = UnixListener::bind(runtime.join("wayland-3")).expect("bind socket");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    let recorded = dir.join("systemctl.argv");
    fake_tool(&tools, "systemctl", &recorded, 0);
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(wait_script())
        .arg("wayland-env")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", path)
        .env("ICEDTEA_WAIT_TIMEOUT", "2")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let argv = fs::read_to_string(&recorded).expect("systemctl was called");
    let lines: Vec<&str> = argv.lines().collect();
    assert!(
        lines.contains(&"--user")
            && lines.contains(&"set-environment")
            && lines.contains(&"WAYLAND_DISPLAY=wayland-3"),
        "systemctl argv was {argv:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn name_mode_ignores_activatable_names() {
    let dir = tmpdir("name-activatable");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    // An activatable service appears in column 1 but owns no connection: the
    // connection column reads `(activatable)`. It must not count as ready.
    fs::write(
        tools.join("busctl"),
        "#!/bin/sh\nprintf 'org.freedesktop.DBus  42  :1.1\\n'\nprintf 'org.icedtea.Absent  -  -  -  (activatable) -  -  -\\n'\n",
    )
    .expect("fake busctl");
    use std::os::unix::fs::PermissionsExt as _;
    let mut perm = fs::metadata(tools.join("busctl"))
        .expect("stat")
        .permissions();
    perm.set_mode(0o755);
    fs::set_permissions(tools.join("busctl"), perm).expect("chmod");
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(wait_script())
        .arg("name")
        .arg("org.icedtea.Absent")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", path)
        .env("ICEDTEA_WAIT_TIMEOUT", "1")
        .output()
        .expect("run");
    assert!(
        !out.status.success(),
        "an activatable-but-unowned name must not count as ready"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("not owned"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn session_start_imports_environment_then_starts_target() {
    let dir = tmpdir("start");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    let recorded = dir.join("systemctl.argv");
    fake_tool(&tools, "systemctl", &recorded, 0);
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("launch/icedtea-session-start");
    let out = Command::new(script)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", path)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let argv = fs::read_to_string(&recorded).expect("systemctl was called");
    let lines: Vec<&str> = argv.lines().collect();
    assert!(
        lines.contains(&"import-environment")
            && lines.contains(&"XDG_SESSION_TYPE")
            && lines.contains(&"start")
            && lines.contains(&"--wait")
            && lines.contains(&"icedtea-session.target"),
        "systemctl argv was {argv:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}
