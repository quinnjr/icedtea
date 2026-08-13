//! `icedtea-clipboard` internals, exposed as a library so integration tests can
//! drive the data-control [`manager`] and inspect [`history`] directly. The
//! binary (`main.rs`) is a thin wiring layer over these.

pub mod history;
pub mod manager;
pub mod service;
