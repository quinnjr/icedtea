//! `icedtea-shell` internals as a library, so integration tests can build the
//! panel and drive the pure models directly.

pub mod bridge;
pub mod clip_client;
pub mod clipboard;
pub mod taskbar;
pub mod compositor_client;

// Re-exported so integration tests can drive real widgets without a second
// gtk4 dependency edge.
pub use gtk4;
