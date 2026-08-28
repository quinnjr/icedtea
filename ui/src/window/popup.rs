//! The `xdg_popup` role and its positioner. Task 14 fills this in.

/// A popup's identity within one window. Opaque; only [`crate::window::Window`]
/// mints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) u64);
