pub mod clipboard;
pub mod event;
pub mod notifications;
pub mod types;

pub const COMPOSITOR_BUS_NAME: &str = "org.icedtea.Compositor";
pub const COMPOSITOR_PATH: &str = "/org/icedtea/Compositor";

pub use clipboard::{ClipEntry, ClipKind, CLIP_BUS_NAME, CLIP_PATH};
pub use event::{Event, SeqEvent};
pub use notifications::{
    CloseReason, IconSource, Notification, NotificationAction, Urgency, NOTIF_BUS_NAME,
    NOTIF_PATH,
};
pub use types::*;
