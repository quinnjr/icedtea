//! The dependency-swap gate: the settings crate builds and runs with no GTK
//! stack anywhere in its normal dependency tree or its source.
//!
//! Mutation check: put `gtk4 = "0.11"` back in settings/Cargo.toml; this test
//! fails. Restore.

use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

#[test]
fn no_gtk_stack_in_the_settings_tree_or_sources() {
    for package in [
        "gtk4",
        "gtk4-layer-shell",
        "glib",
        "gio",
        "gdk4",
        "pango",
        "cairo-rs",
    ] {
        let out = Command::new(env!("CARGO"))
            .current_dir(workspace_root())
            .args([
                "tree",
                "-p",
                "icedtea-settings",
                "-e",
                "normal",
                "-i",
                package,
            ])
            .output()
            .expect("cargo tree runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.status.success() || text.contains("nothing to print"),
            "{package} is still in icedtea-settings' normal tree:\n{text}"
        );
    }

    for dir in ["settings/src", "settings/tests"] {
        let out = Command::new("grep")
            .current_dir(workspace_root())
            .args([
                "-rn",
                "--exclude=no_gtk.rs",
                "gtk4\\|glib::\\|gio::\\|gdk::\\|pango::\\|cairo::",
                dir,
            ])
            .output()
            .expect("grep runs");
        let hits = String::from_utf8_lossy(&out.stdout);
        assert!(hits.trim().is_empty(), "GTK paths remain in {dir}:\n{hits}");
    }
}
