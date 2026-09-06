//! `icedtea-settings`: a config editor for icedtea, built on `icedtea-ui`.
//!
//! [`model`] is the toolkit-free core -- working-copy state, keybinding-capture
//! translation, and validation -- consumed by [`app`]'s `update`/`view` and
//! each page module in [`pages`].

pub mod app;
pub mod compositor_reload;
pub mod ipc;
pub mod model;
pub mod outputs;
pub mod pages;
pub mod probe;
