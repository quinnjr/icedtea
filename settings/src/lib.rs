//! `icedtea-settings`: GTK config editor for icedtea-wm.
//!
//! [`model`] is the GTK-free core -- working-copy state, keybinding-capture
//! translation, and validation -- consumed by the (GTK) view in [`pages`]
//! and `main.rs`.

pub mod app;
pub mod compositor_reload;
pub mod ipc;
pub mod model;
pub mod outputs;
pub mod pages;
