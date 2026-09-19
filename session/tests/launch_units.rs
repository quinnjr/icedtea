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
    assert!(
        out.status.success(),
        "systemd-analyze verify failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_target_wants_every_component() {
    let target =
        std::fs::read_to_string(units_dir().join("icedtea-session.target")).expect("target file");
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
