//! Path grammar and the canonicalizers for the non-alphanumeric roots.
//!
//! One grammar, defined exactly once here (design §"Paths"): an absolute,
//! slash-separated path of `[A-Za-z0-9._%-]` segments. MIME types and URI
//! schemes contain a character (`/`) that is the path separator, so callers
//! never build those paths by hand — they call [`mime_path`] / [`scheme_path`],
//! which percent-encode the slash. `%` is therefore a legal segment character
//! (and the only escape introducer).
//!
//! Segments are case-sensitive and uppercase is legal: an app id is a
//! reverse-DNS identifier like `org.gnome.Calculator` whose case is
//! significant. The canonicalizers lowercase MIME types and schemes, where the
//! case is not significant.

use crate::error::RegistryError;

/// Longest permitted path, in segments (including the leading empty one's
/// children), to bound the daemon's parse and `List` fan-out.
pub const MAX_PATH_DEPTH: usize = 32;
/// Longest permitted single segment.
pub const MAX_SEGMENT_LEN: usize = 128;

/// Validate a path against the grammar. `/` (the root) is valid and is what a
/// prefix `List`/`Reset` of everything uses.
pub fn validate_path(path: &str) -> Result<(), RegistryError> {
    if path.is_empty() {
        return Err(RegistryError::InvalidPath("path is empty".into()));
    }
    if !path.starts_with('/') {
        return Err(RegistryError::InvalidPath(format!(
            "path {path:?} is not absolute"
        )));
    }
    if path == "/" {
        return Ok(());
    }
    if path.ends_with('/') {
        return Err(RegistryError::InvalidPath(format!(
            "path {path:?} has a trailing slash"
        )));
    }
    let mut depth = 0usize;
    for segment in path.split('/').skip(1) {
        depth += 1;
        if depth > MAX_PATH_DEPTH {
            return Err(RegistryError::InvalidPath(format!(
                "path {path:?} is deeper than {MAX_PATH_DEPTH} segments"
            )));
        }
        if segment.is_empty() {
            return Err(RegistryError::InvalidPath(format!(
                "path {path:?} has an empty segment"
            )));
        }
        if segment.len() > MAX_SEGMENT_LEN {
            return Err(RegistryError::InvalidPath(format!(
                "path {path:?} has a segment longer than {MAX_SEGMENT_LEN}"
            )));
        }
        if let Some(bad) = segment.chars().find(|c| !is_segment_char(*c)) {
            return Err(RegistryError::InvalidPath(format!(
                "path {path:?} has a segment with the illegal character {bad:?}"
            )));
        }
    }
    Ok(())
}

/// Is a path segment character legal? `%` is included because the MIME
/// canonicalizer introduces it as the slash escape.
fn is_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '%')
}

/// Is `prefix` a path-prefix of `path` in the segment sense? `/a/b` is a prefix
/// of `/a/b/c` and of `/a/b`, but **not** of `/a/bc` — a plain `str::starts_with`
/// would wrongly match the last case, and every `Watch`/`List`/`Reset` decision
/// rests on this.
pub fn is_path_prefix(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return path.starts_with('/');
    }
    if !path.starts_with(prefix) {
        return false;
    }
    match path[prefix.len()..].chars().next() {
        None => true,
        Some('/') => true,
        Some(_) => false,
    }
}

/// Percent-encode a raw name into a legal path segment. Any byte outside
/// `[A-Za-z0-9._-]` — including `%` itself — becomes `%XX`. Used for the
/// dynamic names that are not already segment-safe: keybinding actions
/// (`snap:right`, `spawn:icedtea-session lock`) and MIME types
/// (`application/atom+xml`).
pub fn encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Inverse of [`encode_segment`]. A malformed escape is passed through
/// verbatim rather than dropped, so a hand-typed path is not silently mangled.
pub fn decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path for a MIME type's root: `text/html` -> `/desktop/mime/text%2Fhtml`.
pub fn mime_path(mime: &str) -> String {
    format!(
        "/desktop/mime/{}",
        encode_segment(&mime.to_ascii_lowercase())
    )
}

/// The handler-list path for a MIME type.
pub fn mime_handlers_path(mime: &str) -> String {
    format!("{}/handlers", mime_path(mime))
}

/// The suppressed-handlers path for a MIME type.
pub fn mime_removed_path(mime: &str) -> String {
    format!("{}/removed", mime_path(mime))
}

/// The path for a URI scheme's root: `http` -> `/desktop/scheme/http`.
pub fn scheme_path(scheme: &str) -> String {
    format!(
        "/desktop/scheme/{}",
        encode_segment(&scheme.to_ascii_lowercase())
    )
}

/// The handler-list path for a URI scheme.
pub fn scheme_handlers_path(scheme: &str) -> String {
    format!("{}/handlers", scheme_path(scheme))
}

/// The path for a normalized third-party app record.
pub fn app_path(app_id: &str) -> String {
    format!("/apps/{app_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_simple_paths_validate() {
        assert!(validate_path("/").is_ok());
        assert!(validate_path("/org/icedtea/appearance/bar_height").is_ok());
        assert!(validate_path("/desktop/mime/text%2Fhtml/handlers").is_ok());
        assert!(validate_path("/apps/org.gnome.Calculator").is_ok());
    }

    #[test]
    fn malformed_paths_are_rejected() {
        assert!(matches!(
            validate_path(""),
            Err(RegistryError::InvalidPath(_))
        ));
        assert!(matches!(
            validate_path("org/icedtea"),
            Err(RegistryError::InvalidPath(_))
        ));
        assert!(matches!(
            validate_path("/org/"),
            Err(RegistryError::InvalidPath(_))
        ));
        assert!(matches!(
            validate_path("/org//icedtea"),
            Err(RegistryError::InvalidPath(_))
        ));
        assert!(matches!(
            validate_path("/org/iced tea"),
            Err(RegistryError::InvalidPath(_))
        ));
    }

    #[test]
    fn depth_and_segment_length_are_bounded() {
        let deep = format!("/{}", vec!["a"; MAX_PATH_DEPTH + 1].join("/"));
        assert!(matches!(
            validate_path(&deep),
            Err(RegistryError::InvalidPath(_))
        ));
        let long = format!("/{}", "a".repeat(MAX_SEGMENT_LEN + 1));
        assert!(matches!(
            validate_path(&long),
            Err(RegistryError::InvalidPath(_))
        ));
    }

    #[test]
    fn mime_slash_is_folded_and_lowercased() {
        assert_eq!(mime_path("text/html"), "/desktop/mime/text%2Fhtml");
        assert_eq!(mime_path("TEXT/HTML"), "/desktop/mime/text%2Fhtml");
        assert_eq!(
            mime_handlers_path("application/atom+xml"),
            "/desktop/mime/application%2Fatom%2Bxml/handlers"
        );
        // Every folded path is itself a legal path.
        assert!(validate_path(&mime_handlers_path("text/html")).is_ok());
        assert!(validate_path(&mime_handlers_path("application/atom+xml")).is_ok());
    }

    #[test]
    fn segment_encoding_round_trips_awkward_names() {
        for raw in [
            "snap:right",
            "spawn:icedtea-session lock",
            "workspace:9",
            "application/atom+xml",
            "100%",
            "plain",
        ] {
            let encoded = encode_segment(raw);
            assert!(validate_path(&format!("/x/{encoded}")).is_ok(), "{raw}");
            assert_eq!(decode_segment(&encoded), raw, "{raw}");
        }
    }

    #[test]
    fn scheme_path_is_lowercased() {
        assert_eq!(scheme_path("HTTP"), "/desktop/scheme/http");
        assert_eq!(
            scheme_handlers_path("mailto"),
            "/desktop/scheme/mailto/handlers"
        );
    }

    #[test]
    fn prefix_matching_is_segment_aware() {
        assert!(is_path_prefix("/a/b", "/a/b"));
        assert!(is_path_prefix("/a/b", "/a/b/c"));
        assert!(is_path_prefix("/", "/a/b"));
        assert!(!is_path_prefix("/a/b", "/a/bc"));
        assert!(!is_path_prefix("/a/b", "/a"));
        assert!(!is_path_prefix("/a/b", "/x/b"));
    }
}
