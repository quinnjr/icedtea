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
//!
//! With `--print-allocation` it prints the border-box allocation it *would*
//! map, as `width height label_x label_y` on one line, and exits without
//! touching Wayland. `label_x`/`label_y` are the *label node's* border-box
//! origin relative to the button's — not the button's own border + padding
//! inset, which coincides on `x` for Adwaita but is short on `y` by the
//! label's centring offset within the content box. That is how `tests/layer_shell_screencopy.rs` derives
//! the on-screen geometry it samples instead of hardcoding it.

use std::path::PathBuf;

use icedtea_ui::app::{ThemeSource, run_themed_button, themed_button_allocations};

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

    if std::env::args().any(|arg| arg == "--print-allocation") {
        match themed_button_allocations(&label, &classes, &theme) {
            Ok((allocation, label_allocation)) => println!(
                "{} {} {} {}",
                allocation.border_box.width,
                allocation.border_box.height,
                label_allocation.border_box.x - allocation.border_box.x,
                label_allocation.border_box.y - allocation.border_box.y
            ),
            Err(err) => {
                tracing::error!(%err, "themed-button failed");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Err(err) = run_themed_button(&label, &classes, &theme) {
        tracing::error!(%err, "themed-button failed");
        std::process::exit(1);
    }
}
