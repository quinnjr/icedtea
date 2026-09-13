//! `icedtea-session` CLI: every subcommand calls exactly one
//! `org.icedtea.Session` method and exits. With no daemon on the session bus
//! the call must fail fast and non-zero, never panic.

use std::process::{Command, Output};

/// Run the binary with the given argument against an unreachable session bus,
/// so the test exercises the "no daemon" path deterministically even on a
/// machine that is running a real desktop session.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_icedtea-session"))
        .args(args)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/icedtea-session-test-bus",
        )
        .output()
        .expect("icedtea-session must launch")
}

#[test]
fn poweroff_without_a_daemon_exits_non_zero_without_panicking() {
    let out = run(&["poweroff"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected a non-zero exit; stderr={stderr}"
    );
    assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
}

#[test]
fn every_subcommand_fails_cleanly_without_a_bus() {
    for arg in [
        "lock",
        "suspend",
        "hibernate",
        "poweroff",
        "reboot",
        "logout",
    ] {
        let out = run(&[arg]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{arg}: expected a non-zero exit");
        assert!(
            !stderr.contains("panicked"),
            "{arg}: must not panic: {stderr}"
        );
    }
}

#[test]
fn an_unknown_subcommand_exits_non_zero_without_panicking() {
    let out = run(&["frobnicate"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected a non-zero exit");
    assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
}
