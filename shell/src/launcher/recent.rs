//! Parsing for the recent-files (`recently-used.xbel`) document.
//!
//! Split from [`super::provider`] so the URI/escape rules stay separate from
//! the provider seam: this module turns one XBEL document into candidate
//! local paths and knows nothing about scanning, ranking, or confinement
//! (the provider applies the security checks).

use std::path::PathBuf;

/// Extract up to `limit` local paths from a `recently-used.xbel`, in
/// document order (newest first in the real file). Non-`file://` schemes,
/// malformed escapes and duplicates are dropped.
pub fn parse_recent(xbel: &str, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in xbel.lines() {
        if out.len() >= limit {
            break;
        }
        let Some(start) = line.find("href=\"") else {
            continue;
        };
        let rest = &line[start + "href=\"".len()..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let Some(path) = decode_file_uri(&rest[..end]) else {
            continue;
        };
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Decode a `file://` URI to a local path. `None` for any other scheme, a
/// non-localhost authority, a malformed escape, non-UTF-8 bytes, or a NUL.
fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = match rest.find('/') {
        Some(0) => rest,
        Some(slash) if &rest[..slash] == "localhost" => &rest[slash..],
        _ => return None,
    };
    let bytes = percent_decode(path_part)?;
    if bytes.contains(&0) {
        return None;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

/// `%XX` decoding. `None` on a truncated or non-hex escape.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recent_decodes_local_uris_and_skips_other_schemes() {
        let xbel = r#"<?xml version="1.0"?>
<bookmarks>
  <bookmark href="file:///home/u/My%20Docs/notes.txt" modified="2026-01-01T00:00:00Z"/>
  <bookmark href="https://example.com/page"/>
  <bookmark href="file:///home/u/a.txt"/>
  <bookmark href="file:///home/u/%ZZ.txt"/>
  <bookmark href="file:///home/u/a.txt"/>
</bookmarks>"#;
        let paths = parse_recent(xbel, 10);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/My Docs/notes.txt"),
                PathBuf::from("/home/u/a.txt"),
            ],
            "decoded, deduped, non-file schemes and bad escapes dropped"
        );
    }

    #[test]
    fn parse_recent_caps_at_the_limit() {
        let mut xbel = String::new();
        for i in 0..10 {
            xbel.push_str(&format!("<bookmark href=\"file:///tmp/f{i}\"/>\n"));
        }
        assert_eq!(parse_recent(&xbel, 3).len(), 3);
    }
}
