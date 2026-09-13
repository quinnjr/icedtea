//! `icedtea-session` — the logind session/seat daemon and its CLI.
//!
//! With no argument it boots the daemon: it reads config, resolves this login
//! session, opens the session bus, and registers `org.icedtea.Session`, arms
//! the logind signal/inhibitor flow, and (when `lock_idle_timeout_ms` is set)
//! starts the `ext_idle_notifier_v1` idle→lock client. With a subcommand
//! (`lock`, `suspend`, `hibernate`, `poweroff`, `reboot`, `logout`) it is a
//! thin CLI that calls the matching `org.icedtea.Session` method over the
//! session bus and exits — non-zero when no daemon is there, never a panic.

use std::process::ExitCode;
use std::sync::Arc;

use icedtea_session::flow::LockFlow;
use icedtea_session::idle;
use icedtea_session::logind::{self, Logind, ZbusLogind};
use icedtea_session::service::{
    self, SESSION_BUS_NAME, SESSION_IFACE, SESSION_PATH, SharedLogind, SharedWm,
};
use icedtea_session::wm_client::ZbusWmClient;

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => run_daemon(),
        Some(subcommand) => match parse_subcommand(subcommand) {
            Some(member) => call_session(member),
            None => {
                eprintln!("icedtea-session: unknown subcommand `{subcommand}`");
                ExitCode::FAILURE
            }
        },
    }
}

/// Map a CLI subcommand to its `org.icedtea.Session` wire member. `None` for
/// anything the daemon's surface does not name.
fn parse_subcommand(subcommand: &str) -> Option<&'static str> {
    Some(match subcommand {
        "lock" => "Lock",
        "suspend" => "Suspend",
        "hibernate" => "Hibernate",
        "poweroff" => "PowerOff",
        "reboot" => "Reboot",
        "logout" => "LogOut",
        _ => return None,
    })
}

/// The CLI: one method call, then exit. Every failure — no bus, no daemon, a
/// rejected call — is a non-zero exit and a log line, never a panic. The
/// method's reply (if any) is ignored; the side effect is the point.
fn call_session(member: &str) -> ExitCode {
    let conn = match zbus::blocking::Connection::session() {
        Ok(conn) => conn,
        Err(err) => {
            tracing::error!(%err, "session bus unavailable; cannot reach org.icedtea.Session");
            return ExitCode::FAILURE;
        }
    };
    call_outcome(
        conn.call_method(
            Some(SESSION_BUS_NAME),
            SESSION_PATH,
            Some(SESSION_IFACE),
            member,
            &(),
        )
        .map(|_| ()),
    )
}

/// Map a session method's result to an exit code: a successful call exits `0`,
/// and any error — including the `org.icedtea.Session` service replying with a
/// D-Bus error such as `Failed` (a logind operation that did not go through) —
/// exits non-zero.
fn call_outcome(result: zbus::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "org.icedtea.Session call failed");
            ExitCode::FAILURE
        }
    }
}

/// Boot the daemon: resolve this login session, open the buses, and register
/// `org.icedtea.Session`. Parks once registered so the connection and process
/// stay alive until the signal/idle/sleep wiring lands in a later task.
fn run_daemon() -> ExitCode {
    let config = icedtea_config::load_or_default(&icedtea_config::default_db_path());
    let locker = config.power.locker_command.as_deref().unwrap_or("(none)");
    tracing::info!(locker, "icedtea-session starting");

    let xdg_session_id = std::env::var("XDG_SESSION_ID").ok();
    let logind = match ZbusLogind::connect(xdg_session_id.as_deref(), std::process::id()) {
        Ok(logind) => logind,
        Err(err) => {
            tracing::error!(%err, "could not resolve this logind session; exiting");
            return ExitCode::FAILURE;
        }
    };
    let session_path = logind.session_path();

    let wm = match ZbusWmClient::new() {
        Ok(wm) => wm,
        Err(err) => {
            tracing::error!(%err, "session bus unavailable; exiting");
            return ExitCode::FAILURE;
        }
    };

    let logind: SharedLogind = Arc::new(logind);
    let wm: SharedWm = Arc::new(wm);

    // Arm the power-key block inhibitor and the sleep delay inhibitor, then
    // drive them (and the locker funnel) from logind's signals on their own
    // thread.
    let flow = Arc::new(LockFlow::new(
        Arc::clone(&logind),
        Arc::clone(&wm),
        &config.power,
    ));
    let events: logind::SharedLogindEvents = flow.clone();
    logind::spawn_signal_loop(session_path, events);

    // Idle→lock rides the same `LockFlow` funnel as lock-before-sleep, so an
    // idle timeout and an imminent suspend cannot spawn two lockers. `None`
    // leaves idle-lock disabled and starts no client (spec Decision 7). The
    // idle thread is daemon-lifetime, so its handle is deliberately dropped
    // (detached) here.
    match idle::spawn(Arc::clone(&flow), config.power.lock_idle_timeout_ms) {
        Ok(Some(_idle)) => tracing::info!("idle-lock client armed"),
        Ok(None) => {}
        Err(err) => tracing::warn!(
            %err,
            "could not start the idle-lock client; idle-lock is off this session"
        ),
    }

    let _service = match service::spawn(logind, wm) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::error!(
                %err,
                "could not register org.icedtea.Session (another daemon running?); exiting"
            );
            return ExitCode::FAILURE;
        }
    };

    tracing::info!("org.icedtea.Session registered; watching logind sleep/lock signals");
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{call_outcome, parse_subcommand};

    #[test]
    fn subcommands_map_to_their_session_wire_members() {
        assert_eq!(parse_subcommand("lock"), Some("Lock"));
        assert_eq!(parse_subcommand("suspend"), Some("Suspend"));
        assert_eq!(parse_subcommand("hibernate"), Some("Hibernate"));
        assert_eq!(parse_subcommand("poweroff"), Some("PowerOff"));
        assert_eq!(parse_subcommand("reboot"), Some("Reboot"));
        assert_eq!(parse_subcommand("logout"), Some("LogOut"));
    }

    #[test]
    fn an_unknown_subcommand_has_no_wire_member() {
        assert_eq!(parse_subcommand("frobnicate"), None);
        assert_eq!(parse_subcommand(""), None);
    }

    /// A success reply exits `0`; any error reply — e.g. the service's
    /// `fdo::Error::Failed` after logind rejected a power action — exits
    /// non-zero instead of silently reporting success.
    #[test]
    fn a_failed_method_call_exits_non_zero() {
        assert_eq!(call_outcome(Ok(())), ExitCode::SUCCESS);
        assert_eq!(
            call_outcome(Err(zbus::Error::Failure("suspend failed".to_string()))),
            ExitCode::FAILURE
        );
    }
}
