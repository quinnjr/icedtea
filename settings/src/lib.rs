//! `icedtea-settings`: GTK config editor for icedtea-wm.
//!
//! [`model`] is the GTK-free core -- working-copy state, keybinding-capture
//! translation, and validation -- consumed by the (GTK) view built on top of
//! it in later tasks.

pub mod model;
