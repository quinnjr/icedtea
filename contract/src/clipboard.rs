//! The `org.icedtea.Clipboard` IPC vocabulary shared by the `icedtea-clipboard`
//! daemon (which serves the interface) and `icedtea-shell` (which renders it).

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// Well-known bus name the clipboard daemon claims.
pub const CLIP_BUS_NAME: &str = "org.icedtea.Clipboard";
/// Object path the clipboard interface is served at.
pub const CLIP_PATH: &str = "/org/icedtea/Clipboard";

/// What a history entry holds, coarsely — enough for the shell to pick an icon
/// and a preview without carrying the payload over the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ClipKind {
    Text,
    Uris,
    Image,
}

/// One clipboard history entry as it crosses D-Bus. `id` is monotonic and
/// stable for the entry's lifetime; `preview` is a short label (truncated
/// text, or a uri/image summary); `mime` is the primary mime it was captured
/// under; `source_app` is best-effort and may be `None`. The payload bytes
/// stay in the daemon — the shell asks for a re-paste by `id`, it never
/// carries the content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ClipEntry {
    pub id: u64,
    pub kind: ClipKind,
    pub preview: String,
    pub mime: String,
    pub pinned: bool,
    pub source_app: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zvariant::Type;

    #[test]
    fn clip_entry_wire_signature_is_locked() {
        // Pin whatever the derive emits so a field reorder or a type change
        // breaks the daemon<->shell ABI here rather than silently at runtime.
        // (`option-as-array` renders `Option<String>` as `as`.)
        // t (id, u64) u (kind enum) s (preview) s (mime) b (pinned) as (source_app)
        assert_eq!(ClipEntry::SIGNATURE.to_string(), "(tussbas)");
    }
}
