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
    if Command::new("systemd-analyze")
        .arg("--version")
        .output()
        .is_err()
    {
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "systemd-analyze verify failed:\n{stderr}"
    );
    // Unknown keys are warnings, not errors: verify still exits 0. Fail
    // explicitly so a misplaced directive (e.g. RequiredBy= in [Unit]) cannot
    // slip through a green `status.success()`.
    assert!(
        !stderr.contains("Unknown key") && !stderr.contains("ignoring"),
        "systemd-analyze verify ignored a directive:\n{stderr}"
    );
}

#[test]
fn the_target_pulls_in_every_component() {
    let target =
        std::fs::read_to_string(units_dir().join("icedtea-session.target")).expect("target file");
    // The compositor is the one hard dependency: a desktop without a
    // compositor is no session, so its exit stops the target.
    assert!(
        target.contains("Requires=icedtea-compositor.service"),
        "the session target must hard-require the compositor"
    );
    for unit in [
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

#[test]
fn the_wayland_env_publisher_refreshes_on_compositor_restart() {
    let unit = std::fs::read_to_string(units_dir().join("icedtea-wayland-env.service"))
        .expect("wayland-env unit");
    // A compositor restart must propagate to the publisher (PartOf=), which
    // re-waits for the socket and re-publishes; its ExecStop clears the old
    // value first so a stale WAYLAND_DISPLAY cannot survive.
    assert!(
        unit.contains("PartOf=icedtea-compositor.service"),
        "a compositor restart must propagate to the publisher"
    );
    assert!(
        unit.contains("ExecStop=") && unit.contains("unset-environment WAYLAND_DISPLAY"),
        "the publisher must clear the stale WAYLAND_DISPLAY on stop"
    );
}
