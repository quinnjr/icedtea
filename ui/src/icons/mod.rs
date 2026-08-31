//! Freedesktop icon themes: lookup, rendering, symbolic recolouring and
//! GTK's builtin shapes.
//!
//! P4 (contract deviation D9) ships only [`IconTheme`]'s construction and
//! inheritance chain, because `view::BuildCx`/`view::EventCx` carry an
//! `&mut IconTheme` and P4 executes before P7. P7 owns everything else in
//! this module: `lookup`, `render`, `IconFile`, `DirKind`, `Palette`, the
//! symbolic recolour path, the builtin shapes and the caches.

pub mod builtin;
pub mod theme;

pub use builtin::Builtin;
pub use theme::{IconTheme, theme_name_from_settings_ini};
