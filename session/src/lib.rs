//! `icedtea-session` internals, exposed as a library so the pure policy and the
//! logind seam can be unit-tested without a system bus or a Wayland compositor.
//! The binary (`main.rs`) is a thin wiring layer over these.

pub mod logind;
pub mod policy;
pub mod service;
pub mod wm_client;
