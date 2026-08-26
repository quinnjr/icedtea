pub mod clipboard;
pub mod event;
pub mod notifications;
pub mod types;

pub const COMPOSITOR_BUS_NAME: &str = "org.icedtea.Compositor";
pub const COMPOSITOR_PATH: &str = "/org/icedtea/Compositor";

/// The `org.icedtea.Compositor` wire-contract revision, exposed on the bus as
/// the interface's read-only `Version` property and compiled into every
/// client that links this crate.
///
/// Review finding F7: the shell and the compositor marshal these types
/// independently, so a compositor upgrade that changes a *signature* --
/// `WindowInfo` gaining the `attention` bit turned `GetState`'s reply from
/// `...bbbb` into `...bbbbb` -- makes an older shell's `GetState` fail with
/// `SignatureMismatch` and nothing else. Bumping this on every such change
/// gives the mismatch a name: the shell reads the property at connect and
/// says which side is stale, instead of only reporting a deserialization
/// error from one call.
///
/// A property, not a method argument, so adding it changes no method
/// signature and cannot itself become the next incompatibility.
///
/// * `1` -- the pre-A2 contract.
/// * `2` -- A2 batch 2: `WindowInfo.attention` / `WindowUpdate.attention`.
pub const COMPOSITOR_CONTRACT_VERSION: u32 = 2;

pub use clipboard::{ClipEntry, ClipKind, CLIP_BUS_NAME, CLIP_PATH};
pub use event::{Event, SeqEvent};
pub use notifications::{
    CloseReason, IconSource, Notification, NotificationAction, Urgency, NOTIF_BUS_NAME,
    NOTIF_PATH,
};
pub use types::*;
