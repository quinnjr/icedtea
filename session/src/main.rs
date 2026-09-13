//! `icedtea-session` — placeholder boot for the logind session/seat daemon.
//!
//! Task 3 lands only the crate skeleton, the logind seam, and the pure policy;
//! the `org.icedtea.Session` service, idle notifier, and sleep-inhibitor wiring
//! arrive in later tasks. `main` therefore reads config, resolves this login
//! session (the discovery path, at boot), and connects both buses before
//! exiting with success, so the skeleton is exercisable end to end.

use icedtea_session::logind::Logind as _;

fn main() -> std::process::ExitCode {
    let config = icedtea_config::load_or_default(&icedtea_config::default_db_path());
    let locker = config.power.locker_command.as_deref().unwrap_or("(none)");
    tracing::info!(locker, "icedtea-session starting");

    let xdg_session_id = std::env::var("XDG_SESSION_ID").ok();
    let logind = match icedtea_session::logind::ZbusLogind::connect(
        xdg_session_id.as_deref(),
        std::process::id(),
    ) {
        Ok(logind) => logind,
        Err(err) => {
            tracing::error!(%err, "could not resolve this logind session; exiting");
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing::info!(session = ?logind.session_path(), "resolved logind session");

    match zbus::blocking::Connection::session() {
        Ok(_session) => {}
        Err(err) => {
            tracing::error!(%err, "session bus unavailable; exiting");
            return std::process::ExitCode::FAILURE;
        }
    }

    tracing::info!("icedtea-session skeleton up; service/idle/sleep wiring lands in later tasks");
    std::process::ExitCode::SUCCESS
}
