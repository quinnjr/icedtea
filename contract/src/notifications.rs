//! The `org.freedesktop.Notifications` / `org.icedtea.Notifications` IPC
//! vocabulary shared by the `icedtea-notifications` daemon (which serves both
//! interfaces) and the future shell popup UI (which renders the icedtea one).

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use zvariant::{Signature, Type};

/// Well-known bus name the notification daemon claims — the standard
/// freedesktop name, required verbatim since every sender looks it up.
pub const NOTIF_BUS_NAME: &str = "org.freedesktop.Notifications";
/// Object path both interfaces are served at.
pub const NOTIF_PATH: &str = "/org/freedesktop/Notifications";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum CloseReason {
    Expired,
    Dismissed,
    ClosedByRequest,
    Undefined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    None,
    /// A themed icon name or an absolute/`file://` path — the spec's
    /// `app_icon` argument and the deprecated `image-path` hint collapse to
    /// this; the popup UI resolves it through the icon theme or the
    /// filesystem, the daemon never interprets it.
    Named(String),
    /// The spec's raw `image-data`/`icon_data` hint: width, height,
    /// rowstride, has-alpha, bits-per-sample, channels, and the raw
    /// row-major pixel bytes, passed through unmodified.
    Pixels {
        width: i32,
        height: i32,
        rowstride: i32,
        has_alpha: bool,
        bits_per_sample: i32,
        channels: i32,
        data: Vec<u8>,
    },
}

/// `IconSource`'s variants have different field counts, which D-Bus's static
/// type system can't express as one enum signature (zvariant's `Type` derive
/// requires every variant to share a shape). This fixed-shape wire struct is
/// the actual on-the-wire encoding: `tag` selects the variant, and every
/// field is always present (zero/empty when the active variant doesn't use
/// it). `IconSource` converts to/from it by hand below so the public Rust
/// type keeps the exact variant shapes the model calls for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
struct IconSourceWire {
    tag: u32,
    name: String,
    width: i32,
    height: i32,
    rowstride: i32,
    has_alpha: bool,
    bits_per_sample: i32,
    channels: i32,
    data: Vec<u8>,
}

const ICON_TAG_NONE: u32 = 0;
const ICON_TAG_NAMED: u32 = 1;
const ICON_TAG_PIXELS: u32 = 2;

impl From<&IconSource> for IconSourceWire {
    fn from(src: &IconSource) -> Self {
        match src {
            IconSource::None => IconSourceWire { tag: ICON_TAG_NONE, ..Default::default() },
            IconSource::Named(name) => {
                IconSourceWire { tag: ICON_TAG_NAMED, name: name.clone(), ..Default::default() }
            }
            IconSource::Pixels { width, height, rowstride, has_alpha, bits_per_sample, channels, data } => {
                IconSourceWire {
                    tag: ICON_TAG_PIXELS,
                    width: *width,
                    height: *height,
                    rowstride: *rowstride,
                    has_alpha: *has_alpha,
                    bits_per_sample: *bits_per_sample,
                    channels: *channels,
                    data: data.clone(),
                    ..Default::default()
                }
            }
        }
    }
}

impl TryFrom<IconSourceWire> for IconSource {
    type Error = String;

    fn try_from(w: IconSourceWire) -> Result<Self, Self::Error> {
        match w.tag {
            ICON_TAG_NONE => Ok(IconSource::None),
            ICON_TAG_NAMED => Ok(IconSource::Named(w.name)),
            ICON_TAG_PIXELS => Ok(IconSource::Pixels {
                width: w.width,
                height: w.height,
                rowstride: w.rowstride,
                has_alpha: w.has_alpha,
                bits_per_sample: w.bits_per_sample,
                channels: w.channels,
                data: w.data,
            }),
            other => Err(format!("unknown IconSource wire tag {other}")),
        }
    }
}

impl Serialize for IconSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        IconSourceWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IconSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        IconSourceWire::deserialize(deserializer)
            .and_then(|w| IconSource::try_from(w).map_err(D::Error::custom))
    }
}

impl Type for IconSource {
    const SIGNATURE: &'static Signature = IconSourceWire::SIGNATURE;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

/// A stored notification as it crosses `org.icedtea.Notifications`. `id` is
/// the same id returned by the standard `Notify` call and used in
/// `NotificationClosed`/`ActionInvoked`/`CloseNotification` — one id space,
/// shared across both interfaces, so the popup UI's `id` always matches the
/// sender's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub icon: IconSource,
    pub summary: String,
    pub body: String,
    pub actions: Vec<NotificationAction>,
    pub urgency: Urgency,
    pub category: Option<String>,
    pub resident: bool,
    pub transient: bool,
    pub created_at_ms: u64,
    pub expire_at_ms: Option<u64>,
    pub suppressed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zvariant::Type;

    #[test]
    fn notification_wire_signature_is_locked() {
        // Pin whatever the derive/hand-rolled impls emit so a field reorder
        // or a type change breaks the daemon<->shell ABI here rather than
        // silently at runtime. (`option-as-array` renders `Option<T>` as
        // `aT`.) u(id) s(app_name) (usiiibiiay)(icon, hand-rolled fixed-shape
        // wire form of IconSource) s(summary) s(body) a(ss)(actions)
        // u(urgency) as(category) b(resident) b(transient) t(created_at_ms)
        // at(expire_at_ms) b(suppressed)
        assert_eq!(
            Notification::SIGNATURE.to_string(),
            "(us(usiiibiiay)ssa(ss)uasbbtatb)"
        );
    }
}
