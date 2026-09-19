# DE session + VM fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Boot the VirtualBox VM into a usable icedtea desktop that runs the repo's own session definition (systemd user units), shows a wallpaper and a panel with a clock, and is verifiable by an automated screenshot check.

**Architecture:** The DE's startup moves out of the VM's ad-hoc `/usr/local/bin/icedtea` script and into the repo as systemd `--user` units plus a small `icedtea-wait` helper, with an env-publisher unit that shares the runtime-chosen Wayland socket name with the other client units. The compositor gains a default-wallpaper fallback; the shell's panel gains a `CenterBox` start/end split and a minute-ticking clock. Provisioning installs the units/asset and stamps a build revision; `vagrant/verify.sh` proves the desktop by screenshot assertions.

**Tech Stack:** Rust (workspace, edition 2024, rust 1.94), systemd `--user`, `busctl`, `systemd-analyze`, VirtualBox (`VBoxManage`), ImageMagick, `jiff` for wall-clock formatting.

**Spec:** `docs/superpowers/specs/2026-09-19-de-session-vm-design.md` — read it alongside this plan; the plan argues from it.

## Global Constraints

- Rust workspace: `edition = "2024"`, `rust-version = "1.94"`; no new dependency except `jiff` (spec D6), and only in `shell`.
- The repo's unsafe policy applies: no new `unsafe` outside the sanctioned compositor FFI; a UI/shell change carries none.
- `jiff = "0.2"` in `[workspace.dependencies]`; `shell` consumes it as `jiff.workspace = true`.
- Units use `ExecStart=/usr/bin/env <binary>` (PATH-resolved) so the same unit files work installed or in the VM's `target/release`.
- User units live under `/etc/systemd/user/` when provisioned; the target is started explicitly (`systemctl --user start icedtea-session.target`), never via `enable`.
- Wallpaper fallback order is fixed: configured path → `ICEDTEA_DEFAULT_WALLPAPER` → `/usr/share/icedtea/default-wallpaper.png` → flat background.
- VM keeps `WLR_RENDERER=pixman`, `GSK_RENDERER=cairo`, `GDK_BACKEND=wayland`.
- Gates for every Rust task: `cargo test` on the touched crate, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`.

---

### Task 1: Session units + helpers

**Files:**
- Create: `session/launch/icedtea-wait`
- Create: `session/launch/icedtea-session-start`
- Create: `session/launch/units/icedtea-compositor.service`
- Create: `session/launch/units/icedtea-wayland-env.service`
- Create: `session/launch/units/icedtea-notifications.service`
- Create: `session/launch/units/icedtea-clipboard.service`
- Create: `session/launch/units/icedtea-session.service`
- Create: `session/launch/units/icedtea-shell.service`
- Create: `session/launch/units/icedtea-session.target`
- Test: `session/tests/launch_scripts.rs`, `session/tests/launch_units.rs`

**Interfaces:**
- Produces: `icedtea-wait <wayland|wayland-env|name <bus-name>>` (exit 0 on ready, non-zero on timeout; `wayland` and `wayland-env` print the socket basename); `icedtea-session-start` (starts the target); the seven unit files. Task 4 installs these.

- [ ] **Step 1: Write the failing script tests**

Create `session/tests/launch_scripts.rs`:

```rust
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
    let dir = std::env::temp_dir().join(format!(
        "icedtea-wait-test-{tag}-{}",
        std::process::id()
    ));
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
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
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
    let recorded = dir.join("busctl.argv");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    // `busctl --user list --no-legend` prints one name per line, first column.
    fs::write(
        tools.join("busctl"),
        "#!/bin/sh\nprintf ':1.1  42  org.freedesktop.DBus\\n'\nprintf ':1.2  7   org.icedtea.WM\\n'\n",
    )
    .expect("fake busctl");
    use std::os::unix::fs::PermissionsExt as _;
    let mut perm = fs::metadata(tools.join("busctl")).expect("stat").permissions();
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
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn name_mode_times_out_when_unowned() {
    let dir = tmpdir("name-none");
    let runtime = dir.join("runtime");
    fs::create_dir_all(&runtime).expect("runtime dir");
    let tools = dir.join("bin");
    fs::create_dir_all(&tools).expect("tools dir");
    fs::write(tools.join("busctl"), "#!/bin/sh\nprintf ':1.1  42  org.freedesktop.DBus\\n'\n")
        .expect("fake busctl");
    use std::os::unix::fs::PermissionsExt as _;
    let mut perm = fs::metadata(tools.join("busctl")).expect("stat").permissions();
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
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let argv = fs::read_to_string(&recorded).expect("systemctl was called");
    let lines: Vec<&str> = argv.lines().collect();
    assert!(
        lines.contains(&"set-environment") && lines.contains(&"WAYLAND_DISPLAY=wayland-3"),
        "systemctl argv was {argv:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-session --test launch_scripts`
Expected: FAIL — `launch/icedtea-wait` does not exist (the spawn itself errors).

- [ ] **Step 3: Write `session/launch/icedtea-wait`**

```sh
#!/bin/sh
# icedtea-wait — block until a session resource exists, or time out.
#
# Usage:
#   icedtea-wait wayland          # exit 0 once a Wayland socket exists (print its name)
#   icedtea-wait wayland-env      # ...and publish WAYLAND_DISPLAY to the user manager
#   icedtea-wait name <bus-name>  # exit 0 once the session bus owns <bus-name>
#
# systemd runs this as an ExecStart/ExecStartPre, so a timeout must FAIL the
# unit (non-zero), never hang it. ICEDTEA_WAIT_TIMEOUT (seconds) bounds the wait.
set -eu

TIMEOUT="${ICEDTEA_WAIT_TIMEOUT:-15}"
RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
TRIES=$((TIMEOUT * 10))

# The first real socket under the runtime dir: skip lock files, accept only
# sockets. Mirrors the scan the old launcher script did.
wayland_socket() {
  for s in "$RUNTIME"/wayland-[0-9]*; do
    case "$s" in *.lock) continue ;; esac
    if [ -S "$s" ]; then
      basename "$s"
      return 0
    fi
  done
  return 1
}

wait_wayland() {
  i=0
  while [ "$i" -lt "$TRIES" ]; do
    if name=$(wayland_socket); then
      printf '%s\n' "$name"
      return 0
    fi
    i=$((i + 1))
    sleep 0.1
  done
  echo "icedtea-wait: no Wayland socket in $RUNTIME after ${TIMEOUT}s" >&2
  return 1
}

wait_name() {
  target="$1"
  i=0
  while [ "$i" -lt "$TRIES" ]; do
    if busctl --user list --no-legend 2>/dev/null | awk '{print $1}' | grep -qx "$target"; then
      return 0
    fi
    i=$((i + 1))
    sleep 0.1
  done
  echo "icedtea-wait: $target not owned on the session bus after ${TIMEOUT}s" >&2
  return 1
}

case "${1:-}" in
  wayland)
    wait_wayland >/dev/null
    ;;
  wayland-env)
    # The socket name is chosen at runtime (`Display::add_socket_auto`); the
    # compositor sets WAYLAND_DISPLAY only in its own environment, so client
    # units learn it through the user manager instead.
    name=$(wait_wayland)
    systemctl --user set-environment "WAYLAND_DISPLAY=$name"
    ;;
  name)
    [ $# -ge 2 ] || { echo "icedtea-wait: 'name' needs a bus name" >&2; exit 2; }
    wait_name "$2"
    ;;
  *)
    echo "usage: icedtea-wait wayland|wayland-env|name <bus-name>" >&2
    exit 2
    ;;
esac
```

Make it executable: `chmod +x session/launch/icedtea-wait`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-session --test launch_scripts`
Expected: PASS (5 tests).

- [ ] **Step 5: Write `session/launch/icedtea-session-start`**

```sh
#!/bin/sh
# Start the icedtea desktop session through the user manager.
#
# The one command a tty autologin runs here, and the command a display
# manager would run on bare metal: the session IS the systemd target.
set -eu
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
exec systemctl --user start icedtea-session.target
```

`chmod +x session/launch/icedtea-session-start`.

- [ ] **Step 6: Write the unit files**

`session/launch/units/icedtea-session.target`:

```ini
[Unit]
Description=icedtea desktop session
Documentation=https://github.com/quinnjr/icedtea
Wants=icedtea-compositor.service
Wants=icedtea-wayland-env.service
Wants=icedtea-notifications.service
Wants=icedtea-clipboard.service
Wants=icedtea-session.service
Wants=icedtea-shell.service
After=icedtea-compositor.service
```

`session/launch/units/icedtea-compositor.service`:

```ini
[Unit]
Description=icedtea compositor (Wayland/DRM)
Documentation=https://github.com/quinnjr/icedtea
# A desktop without a compositor is no session at all: its exit stops the
# session target, and its own restart policy drives recovery.
RequiredBy=icedtea-session.target
Before=icedtea-wayland-env.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStart=/usr/bin/env icedtea-compositor
Restart=on-failure
RestartSec=1
```

`session/launch/units/icedtea-wayland-env.service`:

```ini
[Unit]
Description=icedtea session: publish WAYLAND_DISPLAY
Documentation=https://github.com/quinnjr/icedtea
After=icedtea-compositor.service
Requires=icedtea-compositor.service
Before=icedtea-clipboard.service icedtea-session.service icedtea-shell.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/env icedtea-wait wayland-env
```

`session/launch/units/icedtea-notifications.service`:

```ini
[Unit]
Description=icedtea notification daemon (org.freedesktop.Notifications)
Documentation=https://github.com/quinnjr/icedtea
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStart=/usr/bin/env icedtea-notifications
Restart=on-failure
RestartSec=1
```

`session/launch/units/icedtea-clipboard.service`:

```ini
[Unit]
Description=icedtea clipboard daemon (org.icedtea.Clipboard)
Documentation=https://github.com/quinnjr/icedtea
After=icedtea-wayland-env.service
Wants=icedtea-wayland-env.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStartPre=/usr/bin/env icedtea-wait wayland
ExecStart=/usr/bin/env icedtea-clipboard
Restart=on-failure
RestartSec=1
```

`session/launch/units/icedtea-session.service`:

```ini
[Unit]
Description=icedtea session daemon (logind/idle/lock)
Documentation=https://github.com/quinnjr/icedtea
After=icedtea-wayland-env.service
Wants=icedtea-wayland-env.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
# Needs the compositor's org.icedtea.WM on the bus (its SessionLockChanged
# subscription), not the Wayland socket itself.
ExecStartPre=/usr/bin/env icedtea-wait name org.icedtea.WM
ExecStart=/usr/bin/env icedtea-session
Restart=on-failure
RestartSec=1
```

`session/launch/units/icedtea-shell.service`:

```ini
[Unit]
Description=icedtea shell (panel)
Documentation=https://github.com/quinnjr/icedtea
After=icedtea-wayland-env.service icedtea-clipboard.service
Wants=icedtea-wayland-env.service icedtea-clipboard.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStartPre=/usr/bin/env icedtea-wait name org.icedtea.Clipboard
ExecStart=/usr/bin/env icedtea-shell
Restart=on-failure
RestartSec=1
```

- [ ] **Step 7: Write the failing unit-verification test**

Create `session/tests/launch_units.rs`:

```rust
//! Every session unit is validated by systemd itself. `systemd-analyze
//! verify` catches ordering/dependency typos, unknown directives, and
//! missing `[Unit]` sections — the mistakes a hand-written unit file makes.
//! Skipped visibly where systemd-analyze is absent (non-systemd CI), the
//! same skip-don't-fail discipline the rest of the suite uses.

use std::path::{Path, PathBuf};
use std::process::Command;

fn units_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("launch/units")
}

fn unit_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(units_dir())
        .expect("units dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("service") | Some("target")
            )
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no unit files found");
    files
}

#[test]
fn every_unit_verifies_under_systemd() {
    if Command::new("systemd-analyze").arg("--version").output().is_err() {
        eprintln!("SKIP: systemd-analyze not available");
        return;
    }
    let files = unit_files();
    let mut cmd = Command::new("systemd-analyze");
    cmd.arg("--user").arg("verify");
    for f in &files {
        cmd.arg(f);
    }
    let out = cmd.output().expect("run systemd-analyze");
    assert!(
        out.status.success(),
        "systemd-analyze verify failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_target_wants_every_component() {
    let target = std::fs::read_to_string(units_dir().join("icedtea-session.target"))
        .expect("target file");
    for unit in [
        "icedtea-compositor.service",
        "icedtea-wayland-env.service",
        "icedtea-notifications.service",
        "icedtea-clipboard.service",
        "icedtea-session.service",
        "icedtea-shell.service",
    ] {
        assert!(
            target.contains(&format!("Wants={unit}")),
            "the session target must want {unit}"
        );
    }
}
```

- [ ] **Step 8: Run it to verify it passes**

Run: `cargo test -p icedtea-session --test launch_units`
Expected: PASS (2 tests; verify runs if systemd-analyze is present).

- [ ] **Step 9: Gates + commit**

```bash
cargo clippy -p icedtea-session --all-targets -- -D warnings
cargo fmt --all --check
git add session/launch session/tests/launch_scripts.rs session/tests/launch_units.rs
git commit -m "feat(session): repo-owned DE startup — user units + wait helpers"
```

---

### Task 2: Default wallpaper

**Files:**
- Modify: `compositor/src/lib.rs` (add `DEFAULT_WALLPAPER_PATH`, `resolve_wallpaper`, and use it at the `spawn_wallpaper` call site)
- Create: `session/assets/default-wallpaper.png` (generated asset)
- Test: inline `#[cfg(test)]` in `compositor/src/lib.rs`

**Interfaces:**
- Produces: `pub const DEFAULT_WALLPAPER_PATH: &str`; `fn resolve_wallpaper(configured: Option<&str>, env_override: Option<&std::ffi::OsStr>, installed: &std::path::Path) -> Option<String>`. Task 4 installs the asset to `DEFAULT_WALLPAPER_PATH`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` at the end of `compositor/src/lib.rs` (create the module if absent, following the file's existing test style):

```rust
    #[test]
    fn wallpaper_resolution_prefers_config_then_env_then_installed() {
        let dir = std::env::temp_dir().join(format!("icedtea-wall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let installed = dir.join("default.png");
        std::fs::write(&installed, b"x").expect("asset");

        assert_eq!(
            super::resolve_wallpaper(Some("/custom.png"), Some(std::ffi::OsStr::new("/env.png")), &installed),
            Some("/custom.png".to_string()),
            "a configured wallpaper always wins"
        );
        assert_eq!(
            super::resolve_wallpaper(None, Some(std::ffi::OsStr::new("/env.png")), &installed),
            Some("/env.png".to_string()),
            "the env override beats the installed default"
        );
        assert_eq!(
            super::resolve_wallpaper(None, None, &installed),
            Some(installed.to_string_lossy().into_owned()),
            "an existing installed default is used when config and env are unset"
        );
        assert_eq!(
            super::resolve_wallpaper(None, None, &dir.join("missing.png")),
            None,
            "no installed default means no wallpaper (the flat background)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p icedtea-compositor --lib wallpaper_resolution`
Expected: FAIL — `resolve_wallpaper` not found.

- [ ] **Step 3: Implement the resolver and wire it**

In `compositor/src/lib.rs`, near the other module-level constants, add:

```rust
/// Where the session's default wallpaper is installed. Provisioning copies
/// `session/assets/default-wallpaper.png` here; tests and unusual installs
/// override it with `ICEDTEA_DEFAULT_WALLPAPER`.
pub const DEFAULT_WALLPAPER_PATH: &str = "/usr/share/icedtea/default-wallpaper.png";

/// Resolve which wallpaper to show: the configured path if set, else the
/// `ICEDTEA_DEFAULT_WALLPAPER` override, else the installed default when it
/// exists, else none (today's flat background).
///
/// A fresh install has `Appearance.wallpaper == None`; without this the
/// desktop is a bare colour until someone opens settings and picks an image.
fn resolve_wallpaper(
    configured: Option<&str>,
    env_override: Option<&std::ffi::OsStr>,
    installed: &std::path::Path,
) -> Option<String> {
    if let Some(path) = configured {
        return Some(path.to_string());
    }
    if let Some(path) = env_override {
        return Some(path.to_string_lossy().into_owned());
    }
    installed
        .exists()
        .then(|| installed.to_string_lossy().into_owned())
}
```

Replace the wallpaper path read at `compositor/src/lib.rs:270`:

```rust
    let wallpaper_path = state.config.appearance.wallpaper.clone();
```

with:

```rust
    let wallpaper_path = resolve_wallpaper(
        state.config.appearance.wallpaper.as_deref(),
        std::env::var_os("ICEDTEA_DEFAULT_WALLPAPER").as_deref(),
        std::path::Path::new(DEFAULT_WALLPAPER_PATH),
    );
```

(The `state.spawn_wallpaper(wallpaper_path);` call below it is unchanged.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p icedtea-compositor --lib wallpaper_resolution`
Expected: PASS.

- [ ] **Step 5: Generate the wallpaper asset**

```bash
mkdir -p session/assets
magick -size 1920x1080 \
  gradient:'#1e1e2e-#45475a' \
  -fill '#89b4fa' -draw 'circle 960,560 960,300' \
  -blur 0x120 \
  -resize 1920x1080 \
  session/assets/default-wallpaper.png
```

Generated from the repo's own palette, so it carries no third-party licence. Verify it is a valid PNG: `magick identify session/assets/default-wallpaper.png`.

- [ ] **Step 6: Gates + commit**

```bash
cargo test -p icedtea-compositor --lib
cargo clippy -p icedtea-compositor --all-targets -- -D warnings
cargo fmt --all --check
git add compositor/src/lib.rs session/assets/default-wallpaper.png
git commit -m "feat(compositor): default wallpaper fallback + session asset"
```

---

### Task 3: Panel layout + clock

**Files:**
- Modify: `Cargo.toml` (workspace `jiff` dependency)
- Modify: `shell/Cargo.toml` (`jiff`)
- Modify: `shell/src/panel.rs` (view split, `Msg::Tick`, clock state, `format_clock`)
- Modify: `shell/src/main.rs` (tick thread, `Msg::Tick` handling)
- Modify: `shell/style.css` (`#bar` group rules)
- Test: inline `#[cfg(test)]` in `shell/src/panel.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–2.
- Produces: `panel::Msg::Tick`; `panel::PanelModel.clock: Option<String>`; `fn panel::format_clock(now: &jiff::Zoned) -> String`.

- [ ] **Step 1: Add the dependency**

In the workspace `Cargo.toml` `[workspace.dependencies]`, add:

```toml
jiff = "0.2"
```

In `shell/Cargo.toml` `[dependencies]`, add:

```toml
jiff.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Add to `shell/src/panel.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn the_clock_formats_as_zero_padded_hh_mm() {
        let zoned: jiff::Zoned = "2026-09-19T09:05:00[UTC]".parse().expect("parse");
        assert_eq!(super::format_clock(&zoned), "09:05");
        let afternoon: jiff::Zoned = "2026-09-19T23:59:00[UTC]".parse().expect("parse");
        assert_eq!(super::format_clock(&afternoon), "23:59");
    }

    /// The bar is a `CenterBox` (start/centre/end), not the old single
    /// centred row: the clock lives in the end group, so the left and right
    /// groups are pushed to the bar's edges (`Container::Center` maps to
    /// `JustifyContent::SPACE_BETWEEN`). `View`'s fields are public, so the
    /// structure is asserted directly.
    #[test]
    fn the_bar_splits_into_start_and_end_groups() {
        let (model, _wm, _clip) = panel();
        let view = super::view(&model);
        assert_eq!(
            view.kind,
            icedtea_ui::view::Kind::CenterBox,
            "the bar must be a CenterBox so its groups sit at the edges"
        );
        assert_eq!(
            view.children.len(),
            3,
            "a CenterBox carries exactly start/centre/end"
        );
    }

    #[test]
    fn a_tick_records_the_clock_label() {
        let (mut model, _wm, _clip) = panel();
        assert_eq!(model.clock, None);
        let _ = super::update(&mut model, Msg::Tick);
        assert!(
            model.clock.as_deref().is_some_and(|c| c.len() == 5 && c.as_bytes()[2] == b':'),
            "a tick must record an HH:MM label, got {:?}",
            model.clock
        );
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p icedtea-shell --lib panel::tests::the_clock_formats_as_zero_padded_hh_mm panel::tests::the_bar_splits_into_start_and_end_groups panel::tests::a_tick_records_the_clock_label`
Expected: FAIL — `format_clock` missing, `Msg::Tick` unknown, view kind unchanged.

- [ ] **Step 4: Implement the clock and the split in `panel.rs`**

Add to `PanelModel` (beside the other fields, e.g. after `keyboard_layout`):

```rust
    /// The last formatted wall-clock label (`"HH:MM"`), or `None` before the
    /// first [`Msg::Tick`]. The value is the formatted string, not the instant:
    /// `view` renders what `update` stored, so nothing re-reads the clock
    /// during layout (a formatting failure would panic mid-frame).
    pub clock: Option<String>,
```

Initialize it in `PanelModel::new`: add `clock: None,`.

Add the pure formatter next to `clamp_label`:

```rust
/// Format a wall-clock instant as the panel's `"HH:MM"` label.
///
/// Pure so it is unit-testable without a compositor or a real clock; the
/// timer thread supplies a live `Zoned` and stores the result.
fn format_clock(now: &jiff::Zoned) -> String {
    now.strftime("%H:%M").to_string()
}
```

Add the `Msg` variant (to the `pub enum Msg`):

```rust
    /// One minute elapsed, from the clock thread. Carries nothing: the
    /// handler reads the wall clock, so a delayed tick still shows the
    /// current time rather than the time the tick was scheduled for.
    Tick,
```

Add the clock view (beside `clip_button`):

```rust
/// The right-end wall clock. Renders nothing before the first tick, so a
/// shell that has not yet woken its clock thread shows no stale time.
fn clock_label(m: &PanelModel) -> Option<View<Msg>> {
    m.clock
        .as_deref()
        .map(|label| button(label).id("clock").class("clock"))
}
```

Rewrite `view` to a `CenterBox` (start / centre / end):

```rust
pub fn view(m: &PanelModel) -> View<Msg> {
    // The bar is a CenterBox: the start group (launcher, workspaces, window
    // tiles) sits at the left edge, the indicators and clock at the right,
    // with an empty centre. `Container::Center` maps to
    // `JustifyContent::SPACE_BETWEEN`, so the two groups are pushed apart —
    // a plain `Box` centres its child, which is what made the old bar read
    // as a floating cluster. See the spec's panel section.
    let mut start = vec![start_button(m), workspaces(m), windows(m)];
    if let Some(indicator) = ime_indicator(m) {
        start.push(indicator);
    }
    if let Some(indicator) = layout_indicator(m) {
        start.push(indicator);
    }
    if let Some(indicator) = inhibit_indicator(m) {
        start.push(indicator);
    }
    if let Some(indicator) = touch_indicator(m) {
        start.push(indicator);
    }

    let mut end = Vec::new();
    if let Some(clock) = clock_label(m) {
        end.push(clock);
    }
    end.push(clip_button(m));

    center_box(
        box_(Orientation::Horizontal, start),
        box_(Orientation::Horizontal, Vec::new()),
        box_(Orientation::Horizontal, end),
    )
    .id("bar")
    // See `bar_width`'s doc comment: a pixel width request is the one
    // mechanism that reaches taffy for a plain (non-`ChildLayout`) bar.
    .width_request(m.bar_width)
}
```

Ensure `center_box` is imported (`use icedtea_ui::view::builders::center_box;` or the path the file already uses for `box_`).

Add the `Tick` arm to `update` (beside the other arms):

```rust
        Msg::Tick => {
            m.clock = Some(format_clock(&jiff::Zoned::now()));
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p icedtea-shell --lib panel`
Expected: PASS (including the three new tests).

- [ ] **Step 6: Spawn the tick thread in `main.rs`**

Add a function beside `forward`:

```rust
/// Wake the panel on every minute boundary so the clock advances.
///
/// The toolkit deliberately does not repaint an idle surface ("the whole
/// difference between this and a busy loop"), so a clock cannot simply be
/// read during layout: something has to ask for the redraw. This mirrors
/// `forward`'s worker + inbox shape — a message into the same inbox the
/// D-Bus forwards use — rather than adding a toolkit scheduling API (spec D5).
/// A failed send means the app dropped its inbox (shutdown), so the thread
/// ends the same quiet way `forward` does.
fn clock_tick(tx: InboxSender<Msg>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            // Sleep to the next minute boundary, then tick. `60 - second` is 0
            // exactly on the boundary, so clamp that to a full minute —
            // otherwise the loop busy-spins for the rest of that second.
            let wait = match 60 - u64::from(jiff::Zoned::now().second()) {
                0 => 60,
                n => n,
            };
            std::thread::sleep(std::time::Duration::from_secs(wait));
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    })
}
```

Call it where the other forwards are started in `main`'s `run` (beside `forward(comp_rx, ...)` / `forward(clip_rx, ...)`), sending one immediate tick so the clock appears before the first minute elapses:

```rust
            let _clock = clock_tick(tx.clone());
            let _ = tx.send(Msg::Tick);
```

(If `run` does not already have `tx` in scope at that point, clone the app's `InboxSender` where the other forwards clone it.)

- [ ] **Step 7: Style the groups**

In `shell/style.css`, extend the `#bar` rule and add the clock rule. The bar's own background/padding comment stays; add after the existing `#bar button` rule:

```css
/* The clock is information, not a control: it keeps a button's metrics so
   the end group stays optically even, but never lights up. */
#bar button.clock {
    background-image: none;
    font-variant-numeric: tabular-nums;
}
```

- [ ] **Step 8: Gates + commit**

```bash
cargo test -p icedtea-shell --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
git add Cargo.toml Cargo.lock shell/Cargo.toml shell/src/panel.rs shell/src/main.rs shell/style.css
git commit -m "feat(shell): bar CenterBox split + minute-ticking clock"
```

---

### Task 4: Provisioning, freshness, verification

**Files:**
- Modify: `Vagrantfile`
- Modify: `vagrant/provision-system.sh`
- Modify: `vagrant/provision-build.sh`
- Create: `vagrant/verify.sh`

**Interfaces:**
- Consumes: the units and helpers from Task 1, the asset from Task 2, all binaries from the build.

- [ ] **Step 1: Install units, helpers, and asset in `vagrant/provision-system.sh`**

Replace the launcher block (the `cat > /usr/local/bin/icedtea … EOF` heredoc and the `icedtea-hint.sh` heredoc, lines ~36–97) with:

```sh
# The DE's own session definition, installed where systemd finds it. The
# units and helpers come from the repo (session/launch/), so the VM runs the
# same session artifacts bare metal would.
mkdir -p /etc/systemd/user /usr/share/icedtea
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/*.service /etc/systemd/user/
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/icedtea-session.target /etc/systemd/user/
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-wait /usr/local/bin/icedtea-wait
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-session-start /usr/local/bin/icedtea-session-start
install -m 0644 /home/vagrant/icedtea-wm/session/assets/default-wallpaper.png /usr/share/icedtea/default-wallpaper.png

# Guest-only rendering choices: wlroots on VirtualBox's vmwgfx needs the
# software paths, and these must NOT be baked into the units (a bare-metal
# session would otherwise be forced onto software rendering).
install -d -m 0755 /home/vagrant/.config/environment.d
cat > /home/vagrant/.config/environment.d/99-icedtea-vm.conf <<'EOF'
WLR_RENDERER=pixman
GSK_RENDERER=cairo
GDK_BACKEND=wayland
EOF
chown -R vagrant:vagrant /home/vagrant/.config

# tty1 autologin starts the session target, the one command a display
# manager would run on bare metal. Guarded by the tty so ssh logins do not
# start a second session.
cat > /etc/profile.d/icedtea-session.sh <<'EOF'
[ "$(tty)" = /dev/tty1 ] && [ -z "${WAYLAND_DISPLAY:-}" ] && exec icedtea-session-start
EOF
```

This also removes the old `sleep 1`/`sleep 0.7` ordering hacks: `icedtea-wait` replaces them.

- [ ] **Step 2: Stamp the build and link the binaries in `vagrant/provision-build.sh`**

Replace the file's body with:

```sh
#!/usr/bin/env bash
# User-phase provisioning: rust toolchain, release build, and the pieces the
# session units exec (binaries on PATH + a build-revision stamp).
set -euo pipefail

rustup default stable
cd ~/icedtea-wm
cargo build --release

# The units resolve binaries through PATH (`/usr/bin/env icedtea-…`), so link
# every session binary the same way an installed system would.
for bin in icedtea-compositor icedtea-clipboard icedtea-notifications icedtea-session icedtea-shell; do
  ln -sf ~/icedtea-wm/target/release/$bin /usr/local/bin/$bin
done

# rsync excludes .git/, so the guest cannot ask git what it is running. Stamp
# it at build time; `vagrant/verify.sh` prints this, so "the VM runs stale
# code" is visible rather than guessed.
{
  echo "rev=$(git -C /vagrant rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "built=$(date -u +%FT%TZ)"
} > ~/icedtea-wm/BUILD_INFO
cat ~/icedtea-wm/BUILD_INFO
```

- [ ] **Step 3: Force a rebuild on rsync in `Vagrantfile`**

The rsync of `BUILD_INFO` is host-missing on first push, so provision-build still owns it. No change needed beyond confirming `.git/` stays excluded and the workspace `Cargo.toml`/`session/` are synced (they are: the folder syncs `.`). Leave the provider block as-is (VMSVGA, gui, 4 GB, 4 CPUs). Add a comment on the sync block:

```ruby
  # rsync (one-way, host -> guest). `session/`, the unit files and the
  # wallpaper asset ride this same tree: provisioning installs them from the
  # guest's copy, so the VM's session is always the checked-in one.
```

- [ ] **Step 4: Write `vagrant/verify.sh`**

```sh
#!/usr/bin/env bash
# Prove the VM is showing a usable icedtea desktop, or fail loudly.
#
# Run against a booted VM (or let this boot it). Every assertion is against
# pixels or unit state, so a black screen, a missing panel, or a missing
# wallpaper fails here instead of looking "fine" from a shell — the failure
# mode the design exists to remove.
#
# Hosts need: vagrant, VBoxManage, ImageMagick (`magick`). Usage:
#   vagrant/verify.sh            # resume/boot, wait, assert
set -euo pipefail

VM="icedtea-wm"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${VERIFY_OUT:-/tmp/icedtea-verify}"
mkdir -p "$OUT"

fail() { echo "FAIL: $*" >&2; exit 1; }

command -v VBoxManage >/dev/null || fail "VBoxManage not found"
command -v magick >/dev/null || fail "ImageMagick (magick) not found"

# 1. Boot if needed and wait for the session units.
state=$(VBoxManage showvminfo "$VM" --machinereadable | sed -n 's/^VMState="\(.*\)"$/\1/p')
case "$state" in
  running) ;;
  saved) VBoxManage startvm "$VM" --type headless >/dev/null ;;
  *) fail "VM '$VM' is $state; provision it first (vagrant up)" ;;
esac

echo "== BUILD_INFO =="
vagrant ssh -c 'cat ~/icedtea-wm/BUILD_INFO' || fail "cannot read BUILD_INFO"

# 2. The session is actually up. Poll, because systemd start is async.
active() { vagrant ssh -c "systemctl --user is-active $1" 2>/dev/null | tr -d '\r'; }
i=0
until [ "$(active icedtea-session.target)" = "active" ] && [ "$(active icedtea-shell.service)" = "active" ] && [ "$(active icedtea-compositor.service)" = "active" ]; do
  i=$((i + 1)); [ "$i" -lt 60 ] || {
    vagrant ssh -c 'systemctl --user list-units "icedtea-*" --no-pager; journalctl --user -u icedtea-compositor -n 20 --no-pager' || true
    fail "session units did not become active within 60s"
  }
  sleep 1
done

# 3. Capture and assert the desktop is there.
VBoxManage controlvm "$VM" screenshotpng "$OUT/desktop.png" >/dev/null
mean() { magick "$1" -colorspace Gray -format '%[fx:mean]' info:; }

desktop_mean=$(mean "$OUT/desktop.png")
awk "BEGIN { exit !($desktop_mean > 0.005) }" || fail "screen is (near) black (mean $desktop_mean)"

# The panel bar is a distinct dark band across the top row.
magick "$OUT/desktop.png" -crop x4+0+0 +repage -format '%[fx:mean]' info: > "$OUT/bar.txt"
bar_mean=$(cat "$OUT/bar.txt")
# The wallpaper must differ from the flat default background (the flat navy
# reads ~0.119 in grey; a wallpaper shifts it). Assert the centre region is
# not the flat colour.
centre=$(magick "$OUT/desktop.png" -crop 400x300+440+300 +repage -format '%[fx:standard_deviation]' info:)
awk "BEGIN { exit !($centre > 0.01) }" || fail "desktop is a flat colour — no wallpaper (sd $centre), bar mean $bar_mean"

echo "desktop mean=$desktop_mean bar mean=$bar_mean centre sd=$centre"

# 4. An app window opens and changes the frame.
vagrant ssh -c 'export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 GDK_BACKEND=wayland; nohup foot >/tmp/foot.log 2>&1 & sleep 3; pgrep -x foot >/dev/null' \
  || fail "foot did not start"
VBoxManage controlvm "$VM" screenshotpng "$OUT/desktop-foot.png" >/dev/null
delta=$(magick compare -metric AE "$OUT/desktop.png" "$OUT/desktop-foot.png" null: 2>&1 || true)
awk "BEGIN { exit !($delta > 10000) }" || fail "no window appeared after launching foot (delta $delta px)"

echo "PASS: icedtea desktop verified. Screenshots in $OUT"
```

`chmod +x vagrant/verify.sh`.

- [ ] **Step 5: Re-provision and run the verification end to end**

```bash
vagrant rsync
vagrant provision
vagrant/verify.sh
```

Expected: `BUILD_INFO` prints the current `develop` revision, all three units become `active`, and the script prints `PASS` with screenshots. Inspect `$OUT/desktop.png`: a wallpaper is visible and the bar shows a start button on the left and an `HH:MM` clock on the right.

- [ ] **Step 6: Commit**

```bash
git add Vagrantfile vagrant/provision-system.sh vagrant/provision-build.sh vagrant/verify.sh
git commit -m "feat(vagrant): install the DE session, stamp the build, verify by screenshot"
```

---

## Self-Review

**Spec coverage:**

- §1 session composition (units, graph, compositor `RequiredBy`/others `Wants`, readiness via `icedtea-wait` + env publisher, trigger, environment drop-in) → Task 1 (units, helpers, tests) and Task 4 Steps 1–3 (install, trigger, drop-in). Covered.
- §2 wallpaper default (env → installed → flat, asset, provisioning install) → Task 2 (resolver + asset) and Task 4 Step 1 (install). Covered.
- §3 panel (CenterBox split, clock, tick thread, time crate) → Task 3. Covered. (The design said "`justify-content: space-between`"; the toolkit hardcodes `JustifyContent::CENTER` for a plain `Box` and exposes `Container::Center`/`center_box` for `SPACE_BETWEEN` — the plan uses `center_box`, which is the mechanism that actually yields the split while keeping `#bar`'s background painted. This is a like-for-like implementation of the design's intent, noted here for the executor.)
- §4 provisioning, freshness (`BUILD_INFO`), verification (`verify.sh` pixel asserts) → Task 4. Covered.
- Failure handling (bounded restart, unit-active checks, fail-closed pixel asserts) → Task 1 units (`StartLimitBurst`), Task 4 Step 4. Covered.
- Testing (hermetic script tests, `systemd-analyze verify`, panel/clock unit tests, resolver test, VM verification) → Tasks 1–4. Covered.

**Placeholder scan:** no "TBD"/"implement later"/"similar to Task N"/"add error handling" — every code step carries the code. The one judgement left to the executor is an import path (`center_box`) and the exact insertion point in `main.rs`'s `run`, both stated as "where the other forwards are started".

**Type consistency:** `resolve_wallpaper(Option<&str>, Option<&OsStr>, &Path) -> Option<String>` used identically in test and call site; `format_clock(&jiff::Zoned) -> String` and `Msg::Tick`/`PanelModel.clock: Option<String>` used identically in Task 3's tests and implementation; `icedtea-wait`'s modes (`wayland`, `wayland-env`, `name`) match between Task 1's script, its tests, and the units that call them.
