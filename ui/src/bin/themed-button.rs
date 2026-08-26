//! `themed-button` — the M1 proving slice, runnable.
//!
//! Shows one GTK-themed button as a `zwlr_layer_shell_v1` overlay, anchored
//! top-left. Environment:
//!
//! - `ICEDTEA_UI_THEME`  — `bundled`, or a path to a `gtk.css`. Default:
//!   the user's installed GTK4 theme, falling back to the bundled Adwaita.
//! - `ICEDTEA_UI_LABEL`   — the button's label. Default: `Click me`.
//! - `ICEDTEA_UI_CLASSES` — comma-separated style classes, e.g.
//!   `suggested-action`. Default: none.

use std::path::PathBuf;

use icedtea_ui::app::{ThemeSource, run_themed_button};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let theme = match std::env::var("ICEDTEA_UI_THEME") {
        Ok(value) if value == "bundled" => ThemeSource::Bundled,
        Ok(value) if !value.is_empty() => ThemeSource::File(PathBuf::from(value)),
        _ => ThemeSource::UserPreferred,
    };
    let label = std::env::var("ICEDTEA_UI_LABEL").unwrap_or_else(|_| "Click me".to_string());
    let classes_raw = std::env::var("ICEDTEA_UI_CLASSES").unwrap_or_default();
    let classes: Vec<&str> = classes_raw
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();

    if let Err(err) = run_themed_button(&label, &classes, &theme) {
        tracing::error!(%err, "themed-button failed");
        std::process::exit(1);
    }
}
