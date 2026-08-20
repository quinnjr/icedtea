//! `icedtea-notifications` internals, exposed as a library so integration
//! tests can drive the D-Bus service and inspect [`store`] directly. The
//! binary (`main.rs`) is a thin wiring layer over these.

pub mod expiry;
pub mod service;
pub mod store;
