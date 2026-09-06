//! The app's own lines in `$ICEDTEA_PROBE_REPORT`.
//!
//! `App::with_probe_report` publishes geometry; this publishes state — `page
//! <name>`, `status <text>`, `msg <variant>` — into the same append-only file,
//! so a harness gate can wait for a state and then click a widget it located
//! by id in the same report.

/// The report path, when the environment names one.
#[must_use]
pub fn report_path() -> Option<std::path::PathBuf> {
    std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from)
}

/// Append one line. A missing variable or an I/O failure is a no-op: this is
/// diagnostics, and a settings build with no report must behave identically.
pub fn report(line: &str) {
    let Some(path) = report_path() else {
        return;
    };
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}
