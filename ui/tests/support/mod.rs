//! Shared scaffolding for `icedtea-ui`'s integration tests.
//!
//! Not every test uses every helper, hence the blanket `dead_code` allow:
//! this module is compiled once per test binary that declares it.

#![allow(dead_code)]

use std::process::{Child, Command, Stdio};

/// The theme every test pins its expected colours against. Never the
/// developer's own `gtk.css`.
pub const TEST_THEME: &str = "bundled";

/// Kill the child on the way out however the test ends.
pub struct Reaper(pub Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `themed-button` command with the hermetic environment every test wants.
fn themed_button(label: &str, classes: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_themed-button"));
    command
        .env("ICEDTEA_UI_THEME", TEST_THEME)
        .env("ICEDTEA_UI_CLASSES", classes)
        .env("ICEDTEA_UI_LABEL", label);
    command
}

/// The four numbers `themed-button --print-allocation` prints.
///
/// A local struct, not `icedtea_ui::layout::Allocation`: M2 reshaped that
/// type, and `tests/layer_shell_screencopy.rs` is a gated file whose only
/// permitted edit is the `use` line that names this one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintedAllocation {
    pub width: f32,
    pub height: f32,
    pub label_x: f32,
    pub label_y: f32,
}

/// The allocation the binary itself computes for `label`/`classes`.
///
/// Asking the binary rather than recomputing it here is what keeps the
/// screencopy test's sample coordinates honest: they are derived from the
/// same code path that sizes the layer surface, so a layout change moves the
/// samples instead of silently invalidating them.
///
/// # Panics
///
/// If the binary cannot be run, exits non-zero, or prints something other
/// than the four numbers `--print-allocation` documents.
#[must_use]
pub fn allocation_of(label: &str, classes: &str) -> PrintedAllocation {
    let output = themed_button(label, classes)
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run themed-button --print-allocation");
    assert!(
        output.status.success(),
        "themed-button --print-allocation exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocation is not UTF-8");
    let numbers: Vec<f32> = stdout
        .split_whitespace()
        .map(|field| {
            field
                .parse()
                .unwrap_or_else(|_| panic!("not a number in {stdout:?}"))
        })
        .collect();
    assert_eq!(
        numbers.len(),
        4,
        "expected `width height label_x label_y`, got {stdout:?}"
    );
    PrintedAllocation {
        width: numbers[0],
        height: numbers[1],
        label_x: numbers[2],
        label_y: numbers[3],
    }
}

/// Spawn `themed-button` against `socket`, reaped when the guard drops.
///
/// # Panics
///
/// If the binary cannot be spawned.
#[must_use]
pub fn spawn_themed_button(socket: &str, label: &str, classes: &str) -> Reaper {
    Reaper(
        themed_button(label, classes)
            .env("WAYLAND_DISPLAY", socket)
            .spawn()
            .expect("failed to spawn themed-button"),
    )
}
