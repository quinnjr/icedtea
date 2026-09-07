//! M5 close-out: the dependency audit for `icedtea-shell` (contract §4.1).
//!
//! Crate scope only. The workspace-wide `Cargo.lock` assertions live once, in
//! `settings/tests/dependency_audit.rs` (contract deviation P6-D3).
//!
//! Hermetic (P6-D2): no `cargo` is invoked. §4.1's `cargo tree` invert queries
//! are run once by hand at close-out; their output is in this commit's
//! message.

use std::path::{Path, PathBuf};

/// Case-sensitive tokens that cannot appear innocently in this crate's
/// manifest or sources. Identical to settings' list, deliberately: two crates
/// audited by the same rule, each in its own file, beats a shared helper
/// module that neither crate owns.
const GTK_TOKENS: &[&str] = &[
    "gtk4",
    "glib",
    "gdk",
    "pango",
    "cairo",
    "gobject",
    "gio::",
    "use gio",
    "gio =",
    "GtkBox",
    "ApplicationWindow",
    "CssProvider",
];

/// Upper-case prose that describes a process shape this crate no longer has.
/// `compositor_client.rs` and `clip_client.rs` keep their worker bodies
/// verbatim (contract §3.5, P5 must-not-touch), so their doc comments still
/// said the worker "pushes to the GTK thread" -- there is no GTK thread any
/// more, there is the app's loop thread and its inbox. Prose that merely
/// mentions GTK historically ("the GTK app this replaced") is fine and is not
/// listed here.
const STALE_GTK_PROSE: &[&str] = &[
    "GTK thread",
    "GTK side",
    "GTK running",
    "GTK main loop",
    "the render half",
];

/// M5 defers nothing.
const DEFERRED_WORK_TOKENS: &[&str] = &["todo!(", "unimplemented!(", "TODO", "FIXME"];

/// The audit files quote the tokens they forbid.
const SELF_EXCLUDED: &[&str] = &["dependency_audit.rs", "deletion_ledger.rs"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `root`, depth-first, sorted, minus the audit files.
/// A missing `root` yields an empty vector rather than a panic.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !SELF_EXCLUDED.iter().any(|name| path.ends_with(name))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn offending_lines(paths: &[PathBuf], tokens: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (i, line) in text.lines().enumerate() {
            if tokens.iter().any(|t| line.contains(t)) {
                out.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    out
}

fn shell_sources() -> Vec<PathBuf> {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files -- it is looking in the wrong place",
        paths.len()
    );
    paths
}

/// Mutation check: add `gtk4-layer-shell = "0.8"` to `[dependencies]`; this
/// must fail naming that line. Restore.
#[test]
fn the_shell_manifest_declares_no_gtk_dependency() {
    let manifest =
        std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("read shell/Cargo.toml");
    let mut offenders = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        if GTK_TOKENS.iter().any(|t| code.contains(t)) {
            offenders.push(format!("Cargo.toml:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "shell/Cargo.toml still declares the GTK stack: {offenders:#?}"
    );
}

/// Mutation check: re-add `pub use gtk4;` to `shell/src/lib.rs`; this must
/// fail naming that line. Restore.
#[test]
fn no_shell_source_mentions_a_gtk_name() {
    let offenders = offending_lines(&shell_sources(), GTK_TOKENS);
    assert!(
        offenders.is_empty(),
        "shell/ still names the GTK stack (code or doc comment): {offenders:#?}"
    );
}

/// Mutation check: restore `//! `history_changed` and pushes it to the GTK
/// thread` at the top of `clip_client.rs`; this must fail naming that line.
/// Restore.
#[test]
fn no_shell_doc_comment_still_describes_a_gtk_process() {
    let offenders = offending_lines(&shell_sources(), STALE_GTK_PROSE);
    assert!(
        offenders.is_empty(),
        "shell/ documents a process shape it no longer has: {offenders:#?}"
    );
}

/// Mutation check: add `// TODO: reconnect` to `shell/src/panel.rs`; this must
/// fail naming that line. Restore.
#[test]
fn no_shell_source_defers_work() {
    let offenders = offending_lines(&shell_sources(), DEFERRED_WORK_TOKENS);
    assert!(
        offenders.is_empty(),
        "shell/ defers work at the M5 close-out: {offenders:#?}"
    );
}
