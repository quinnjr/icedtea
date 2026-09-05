//! The `org.freedesktop.portal.FileChooser` wallpaper picker (spec D5).
//!
//! One worker thread, blocking zbus, answering on P0's inbox. The `Entry`
//! beside the Browse button is always live and always authoritative
//! (contract §2.6) — this worker is a convenience, and every one of its
//! failure modes is a single `Msg::WallpaperPickerFailed` the status line
//! shows, never a panic and never a hang.

use std::collections::HashMap;
use std::path::PathBuf;

/// The portal's well-known bus name.
pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
/// The portal's object path.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The file-chooser interface.
pub const FILE_CHOOSER_IFACE: &str = "org.freedesktop.portal.FileChooser";
/// The interface the asynchronous answer arrives on.
pub const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// The object path the portal will emit `Response` on, derived the documented
/// way from our unique bus name and the `handle_token` we send.
///
/// Deriving it lets the worker subscribe *before* it calls `OpenFile`, which
/// is the only way to be sure a fast answer is not missed.
#[must_use]
pub fn request_object_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Decode a `file://` URI into a local path.
///
/// `None` for anything that is not a local file: another scheme, a non-empty
/// authority that is not `localhost`, a malformed percent escape, a byte
/// sequence that is not UTF-8, or an interior NUL. The URI comes from another
/// process, so none of those is a panic.
#[must_use]
pub fn decode_file_uri(uri: &str) -> Option<PathBuf> {
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
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
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

/// The `filters` option: one group, one glob per accepted extension.
///
/// The `a(sa(us))` signature is `(name, [(kind, pattern)])`; kind `0` is a
/// shell glob and kind `1` is a MIME type. Globs, not MIME types, because the
/// same extension list is what [`crate::pages::appearance::validate_wallpaper`]
/// enforces on the way back in — offering a MIME filter the validator does not
/// share would let the portal return a file the field then rejects.
#[must_use]
pub fn image_filters() -> Vec<(String, Vec<(u32, String)>)> {
    let globs = crate::pages::appearance::WALLPAPER_EXTENSIONS
        .iter()
        .map(|ext| (0u32, format!("*.{ext}")))
        .collect();
    vec![("Images".to_string(), globs)]
}

/// Turn one `org.freedesktop.portal.Request.Response` body into an outcome.
///
/// `response` is `0` for success, `1` for "the user cancelled" and `2` for
/// "ended some other way" (which is also what our own `Request.Close` on a
/// timeout produces).
pub fn response_to_outcome(
    response: u32,
    results: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<PathBuf, String> {
    match response {
        0 => {}
        1 => return Err("Wallpaper selection cancelled".to_string()),
        other => return Err(format!("The file portal ended the request (code {other})")),
    }
    let uris = results
        .get("uris")
        .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
        .unwrap_or_default();
    uris.iter()
        .find_map(|uri| decode_file_uri(uri))
        .ok_or_else(|| "The file portal returned no file this app can open".to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use zbus::zvariant::{OwnedValue, Value};

    use super::*;

    fn results(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        pairs
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    OwnedValue::try_from(v).expect("value converts"),
                )
            })
            .collect()
    }

    #[test]
    fn portal_uri_decoding_handles_percent_escapes() {
        assert_eq!(
            decode_file_uri("file:///home/me/My%20Pictures/wall%2Bpaper.png"),
            Some(PathBuf::from("/home/me/My Pictures/wall+paper.png"))
        );
        assert_eq!(
            decode_file_uri("file:///tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png"))
        );
        assert_eq!(
            decode_file_uri("file://localhost/tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png")),
            "an explicit localhost authority is the same local file"
        );
    }

    #[test]
    fn portal_uri_decoding_rejects_what_is_not_a_local_file() {
        assert_eq!(decode_file_uri("https://example.invalid/a.png"), None);
        assert_eq!(decode_file_uri("file://remote-host/tmp/a.png"), None);
        assert_eq!(decode_file_uri(""), None);
        assert_eq!(decode_file_uri("file:///tmp/%zz.png"), None, "bad escape");
        assert_eq!(
            decode_file_uri("file:///tmp/%00.png"),
            None,
            "an interior NUL never becomes a path"
        );
    }

    #[test]
    fn a_cancelled_response_is_a_failure_message_not_a_path() {
        let err = response_to_outcome(1, &results(vec![])).expect_err("cancelled");
        assert!(err.contains("cancelled"), "{err:?}");
        let err = response_to_outcome(2, &results(vec![])).expect_err("ended");
        assert!(!err.is_empty());
    }

    #[test]
    fn an_empty_or_missing_uri_list_is_a_failure_message() {
        let err = response_to_outcome(0, &results(vec![])).expect_err("no uris key");
        assert!(err.contains("no file"), "{err:?}");
        let empty: Vec<String> = Vec::new();
        let err = response_to_outcome(0, &results(vec![("uris", Value::from(empty))]))
            .expect_err("empty uris");
        assert!(err.contains("no file"), "{err:?}");
    }

    #[test]
    fn the_first_local_uri_wins() {
        let uris = vec!["file:///tmp/one.png".to_string()];
        assert_eq!(
            response_to_outcome(0, &results(vec![("uris", Value::from(uris))])).expect("a path"),
            PathBuf::from("/tmp/one.png")
        );
    }

    #[test]
    fn a_request_path_is_the_documented_sender_derived_one() {
        assert_eq!(
            request_object_path(":1.42", "icedtea_0"),
            "/org/freedesktop/portal/desktop/request/1_42/icedtea_0"
        );
        assert_eq!(
            request_object_path("1.42", "t"),
            "/org/freedesktop/portal/desktop/request/1_42/t",
            "a name that already lost its colon is handled the same"
        );
    }

    #[test]
    fn the_image_filter_covers_every_accepted_extension() {
        let filters = image_filters();
        let patterns: Vec<String> = filters
            .iter()
            .flat_map(|(_, globs)| globs.iter().map(|(_, g)| g.clone()))
            .collect();
        for ext in crate::pages::appearance::WALLPAPER_EXTENSIONS {
            assert!(
                patterns.iter().any(|p| p == &format!("*.{ext}")),
                "the portal filter offers .{ext}"
            );
        }
        assert!(
            filters
                .iter()
                .all(|(_, globs)| globs.iter().all(|(k, _)| *k == 0)),
            "0 is the glob-pattern filter kind; 1 would be a MIME type"
        );
    }
}
