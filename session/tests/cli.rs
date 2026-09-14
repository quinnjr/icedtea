//! `icedtea-session` CLI: every subcommand calls exactly one
//! `org.icedtea.Session` method and exits. With no daemon on the session bus
//! the call must fail fast and non-zero, never panic.

mod support;

use std::process::{Command, Output};
use std::sync::Arc;

use icedtea_session::logind::{LogindCall, RecordingLogind};
use icedtea_session::service::{self, SESSION_BUS_NAME, SESSION_IFACE, SESSION_PATH};
use icedtea_session::wm_client::{RecordingWm, WmCall};

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

/// Every subcommand reaches exactly one service method. The service runs
/// in-process with recording seams on a private bus, and a client on that bus
/// invokes each member in turn; the recorded call sequences must be exactly
/// one call per subcommand (`LogOut` → `Quit`).
#[test]
fn each_subcommand_reaches_exactly_one_service_call() {
    let Some(bus) = support::spawn() else {
        eprintln!(
            "{} each_subcommand_reaches_exactly_one_service_call — no dbus-daemon",
            support::SKIP_MARKER
        );
        return;
    };
    let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
    let wm = Arc::new(RecordingWm::new(true));
    let _service = service::spawn_on_bus(logind.clone(), wm.clone(), &bus.address)
        .expect("register the service on the private bus");

    let client = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .expect("private bus address")
        .build()
        .expect("client connects to the private bus");
    let call = |member: &str| {
        client
            .call_method(
                Some(SESSION_BUS_NAME),
                SESSION_PATH,
                Some(SESSION_IFACE),
                member,
                &(),
            )
            .unwrap_or_else(|err| panic!("call {member} failed: {err}"));
    };

    call("Lock");
    call("Suspend");
    call("Hibernate");
    call("PowerOff");
    call("Reboot");
    call("LogOut");

    assert_eq!(
        logind.calls(),
        vec![
            LogindCall::Lock,
            LogindCall::Suspend,
            LogindCall::Hibernate,
            LogindCall::PowerOff,
            LogindCall::Reboot,
        ],
        "each power/lock subcommand reached logind exactly once"
    );
    assert_eq!(
        wm.calls(),
        vec![WmCall::IsLocked, WmCall::Quit],
        "Lock confirmed against the compositor; LogOut forwarded to Quit"
    );
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
