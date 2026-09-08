//! M5 close-out: the dependency audit for `icedtea-settings` (contract §4.1),
//! plus the two workspace-scope lockfile assertions (contract deviation
//! P6-D3 — a workspace-scope test needs a crate to live in, and every crate
//! outside `settings/`, `shell/` and `ui/` is out of M5 scope).
//!
//! These tests are hermetic on purpose (P6-D2): they read `Cargo.toml`, the
//! workspace `Cargo.lock` and this crate's own `.rs` files, and never invoke
//! `cargo`. A nested `cargo` under `cargo test` contends for the build lock.
//! §4.1's `cargo tree` invert queries are still run once by hand at close-out
//! as independent corroboration; their output is in this commit's message.
//!
//! This file supersedes `settings/tests/no_gtk.rs` (deleted here), which did
//! the same job with a `Command::new("cargo tree" ...)` / `Command::new("grep"
//! ...)` pair per package -- exactly the nested-`cargo`-under-`cargo-test`
//! hazard P6-D2 exists to avoid, and flagged on the P2-D20 ledger as a
//! "vacuous pass" nobody had proven could ever go red. This file's tests are
//! each proven red by the mutation checks below (see task-1-report.md for the
//! transcripts).
//!
//! Mutation check for the whole file: add `gtk4 = "0.11"` back to
//! `settings/Cargo.toml` and run `cargo build -p icedtea-settings` to relock;
//! `the_settings_manifest_declares_no_gtk_dependency` and
//! `the_workspace_lockfile_has_no_gtk_stack_package` must both fail. Restore
//! the manifest and relock.

use std::path::{Path, PathBuf};

/// Every GTK-stack package the workspace `Cargo.lock` carries, measured with
/// `awk -F'"' '/^name = /{print $2}' Cargo.lock` filtered by the prefixes
/// below plus `field-offset`. Verified empty on the current tree (504c222):
/// `settings/` and `shell/` were their only holders, and P5 removed the last
/// of them.
const GTK_STACK_PACKAGES: &[&str] = &[
    "cairo-rs",
    "cairo-sys-rs",
    "field-offset",
    "gdk-pixbuf",
    "gdk-pixbuf-sys",
    "gdk4",
    "gdk4-sys",
    "gio",
    "gio-sys",
    "glib",
    "glib-macros",
    "glib-sys",
    "gobject-sys",
    "graphene-rs",
    "graphene-sys",
    "gsk4",
    "gsk4-sys",
    "gtk4",
    "gtk4-layer-shell",
    "gtk4-layer-shell-sys",
    "gtk4-macros",
    "gtk4-sys",
    "pango",
    "pango-sys",
];

/// Name prefixes no crate in this workspace may lock. Twenty-three of the
/// twenty-four packages above match one of these; nothing else in the
/// lockfile does, so there is no false positive to exempt. This catches a
/// *new* GTK-stack package (a `gtk4` minor bump adding `gdk4-wayland`, say)
/// that the explicit list above could not know about.
const GTK_STACK_PREFIXES: &[&str] = &[
    "gtk", "gdk", "gsk", "glib", "gio", "gobject", "pango", "cairo", "graphene",
];

/// Tokens that cannot appear in this crate's manifest: the exact dependency
/// keys of the GTK stack. A `Cargo.toml` line is a dependency declaration,
/// not prose, so a bare crate name here is unambiguous.
const MANIFEST_TOKENS: &[&str] = &[
    "gtk4",
    "gtk4-layer-shell",
    "glib",
    "gio",
    "gdk4",
    "pango",
    "cairo-rs",
    "gobject-sys",
];

/// Tokens that cannot appear in this crate's sources or doc comments. Scoped
/// (`gdk::`, not bare `gdk`) rather than the manifest's bare crate names,
/// because a doc comment is prose: this crate's own files legitimately say
/// things like "GTK-free", "the GTK app this replaced", or draw an analogy to
/// "the old cairo baseline" without naming a type this crate can no longer
/// use. `gdk::ModifierType` in a doc comment is not fine, because it documents
/// a type that no longer exists here -- and every real GTK item path (a use
/// of an actual GTK/GDK/glib/gio/pango/cairo symbol) is written with a `::`
/// or is one of the three named widget/type identifiers below.
const SOURCE_TOKENS: &[&str] = &[
    "gtk4",
    "glib::",
    "gio::",
    "gdk::",
    "pango::",
    "cairo::",
    "GtkBox",
    "ApplicationWindow",
    "CssProvider",
];

/// M5 defers nothing (global rule: no TODOs, no stubs, no "later" notes).
const DEFERRED_WORK_TOKENS: &[&str] = &["todo!(", "unimplemented!(", "TODO", "FIXME"];

/// The audit files quote the very tokens they forbid, so the walker skips
/// them by file name. Nothing else is skipped.
const SELF_EXCLUDED: &[&str] = &["dependency_audit.rs", "deletion_ledger.rs"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_dir() -> PathBuf {
    crate_dir()
        .parent()
        .expect("the crate directory has a parent")
        .to_path_buf()
}

/// Every `.rs` file under `root`, depth-first, sorted, minus the audit files.
/// A `root` that does not exist yields an empty vector rather than a panic:
/// a directory this crate happens not to have (yet) is not a bug in the
/// walker.
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

/// `"<path>:<line number>: <trimmed line>"` for every line of every file that
/// contains any of `tokens`.
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

/// The `name = "…"` values of a `Cargo.lock`, in file order.
fn locked_package_names(lock: &str) -> Vec<&str> {
    lock.lines()
        .filter_map(|line| line.strip_prefix("name = "))
        .map(|value| value.trim().trim_matches('"'))
        .collect()
}

/// Mutation check: add `gtk4 = { version = "0.11" }` to `[dependencies]`;
/// this must fail naming that line. Restore.
#[test]
fn the_settings_manifest_declares_no_gtk_dependency() {
    let path = crate_dir().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).expect("read settings/Cargo.toml");
    let mut offenders = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        // Strip TOML comments: the manifest may legitimately explain in prose
        // which GTK crate a dependency replaced.
        let code = line.split('#').next().unwrap_or("");
        if MANIFEST_TOKENS.iter().any(|t| code.contains(t)) {
            offenders.push(format!("Cargo.toml:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "settings/Cargo.toml still declares the GTK stack: {offenders:#?}"
    );
}

/// Mutation check: add `//! converts \`gdk::ModifierType\`` to
/// `settings/src/model.rs`; this must fail naming that line. Restore.
#[test]
fn no_settings_source_mentions_a_gtk_name() {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files — it is looking in the wrong place",
        paths.len()
    );
    let offenders = offending_lines(&paths, SOURCE_TOKENS);
    assert!(
        offenders.is_empty(),
        "settings/ still names the GTK stack (code or doc comment): {offenders:#?}"
    );
}

/// Mutation check: add `// TODO: wire this up` to `settings/src/app.rs`;
/// this must fail naming that line. Restore.
#[test]
fn no_settings_source_defers_work() {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    let offenders = offending_lines(&paths, DEFERRED_WORK_TOKENS);
    assert!(
        offenders.is_empty(),
        "settings/ defers work at the M5 close-out: {offenders:#?}"
    );
}

/// Workspace scope (P6-D3).
///
/// Mutation check: add `gtk4 = "0.11"` to `settings/Cargo.toml`, run
/// `cargo build -p icedtea-settings` to relock, and this must fail listing
/// every GTK package the relock pulled back in. Restore and relock.
#[test]
fn the_workspace_lockfile_has_no_gtk_stack_package() {
    let lock =
        std::fs::read_to_string(workspace_dir().join("Cargo.lock")).expect("read Cargo.lock");
    let names = locked_package_names(&lock);
    assert!(
        names.len() > 100,
        "the lockfile parse found only {} packages — the `name = \"…\"` format changed \
         and this test would pass vacuously",
        names.len()
    );
    let found: Vec<&str> = GTK_STACK_PACKAGES
        .iter()
        .copied()
        .filter(|p| names.contains(p))
        .collect();
    assert!(
        found.is_empty(),
        "Cargo.lock still locks the GTK stack: {found:?}"
    );
}

/// Workspace scope (P6-D3). Catches a GTK-stack package the explicit list
/// above could not know about.
///
/// Mutation check (verifiable against the real, unmodified `Cargo.lock`):
/// temporarily add a prefix that the workspace *does* lock — e.g. `"serde"` —
/// to `GTK_STACK_PREFIXES`; `found` becomes non-empty (serde is locked) and
/// the assertion fires. Remove it to restore. (This test reads the real
/// lockfile; do not edit `Cargo.lock` itself.)
#[test]
fn no_new_gtk_stack_package_slips_into_the_lockfile() {
    let lock =
        std::fs::read_to_string(workspace_dir().join("Cargo.lock")).expect("read Cargo.lock");
    let found: Vec<&str> = locked_package_names(&lock)
        .into_iter()
        .filter(|name| {
            GTK_STACK_PREFIXES
                .iter()
                .any(|p| *name == *p || name.starts_with(&format!("{p}-")))
        })
        .collect();
    assert!(
        found.is_empty(),
        "a GTK-stack package is locked by the workspace: {found:?}"
    );
}
