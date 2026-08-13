pub mod clipboard;
pub mod event;
pub mod types;

pub const WM_BUS_NAME: &str = "org.icedtea.WM";
pub const WM_PATH: &str = "/org/icedtea/WM";

pub use clipboard::{ClipEntry, ClipKind, CLIP_BUS_NAME, CLIP_PATH};
pub use event::{Event, SeqEvent};
pub use types::*;
