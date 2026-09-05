//! The `org.freedesktop.portal.FileChooser` wallpaper picker (spec D5).
//!
//! One worker thread, blocking zbus, answering on P0's inbox. The `Entry`
//! beside the Browse button is always live and always authoritative
//! (contract §2.6) — this worker is a convenience, and every one of its
//! failure modes is a single `Msg::WallpaperPickerFailed` the status line
//! shows, never a panic and never a hang.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use icedtea_ui::view::InboxSender;

use crate::app::Msg;

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

/// What the loop asks the portal worker for.
pub enum PortalRequest {
    /// Open the file chooser, starting at `current`'s directory if it has one.
    OpenFile { current: Option<PathBuf> },
}

/// How long the worker waits for a `Response` before closing the request.
///
/// A file dialog is user-driven, so this is a ceiling on a wedged portal, not
/// a UX budget: on expiry the worker calls `Request.Close`, which makes the
/// portal answer with a non-zero response, and the user sees a status line
/// instead of a Browse button that never comes back.
pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// One worker thread serving `rx` until every sender is dropped.
///
/// Per request: run the whole `OpenFile` exchange, then send exactly one of
/// [`Msg::WallpaperChosen`] or [`Msg::WallpaperPickerFailed`]. A send error
/// means the app exited, and the thread returns.
///
/// **Superseding.** A request found waiting behind another is the one the
/// user meant: the worker drains everything already queued and serves only
/// the last, answering once.
pub fn spawn(
    rx: crossbeam_channel::Receiver<PortalRequest>,
    tx: InboxSender<Msg>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut request = first;
            while let Ok(newer) = rx.try_recv() {
                request = newer;
            }
            let PortalRequest::OpenFile { current } = request;
            let msg = match open_file(current.as_deref()) {
                Ok(path) => Msg::WallpaperChosen(path),
                Err(reason) => Msg::WallpaperPickerFailed(reason),
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
    })
}

/// The whole exchange, blocking, on the worker thread.
///
/// Subscribe first (the request path is derived, not read off the reply, so a
/// fast answer cannot be missed), then call `OpenFile`, then wait for the
/// `Response` signal on that path.
fn open_file(current: Option<&std::path::Path>) -> Result<PathBuf, String> {
    let conn = Connection::session()
        .map_err(|err| format!("No session bus for the file portal: {err}"))?;
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_string())
        .ok_or_else(|| "The session bus gave this app no unique name".to_string())?;
    let token = format!("icedtea_{}", std::process::id());
    let path = request_object_path(&unique, &token);

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .member("Response")
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .path(path.as_str())
        .map_err(|err| format!("Bad portal request path: {err}"))?
        .build();
    let mut signals = MessageIterator::for_match_rule(rule, &conn, Some(4))
        .map_err(|err| format!("Cannot listen for the portal's answer: {err}"))?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert("multiple", Value::from(false));
    options.insert(
        "filters",
        Value::new(image_filters())
            .try_clone()
            .map_err(|err| format!("Cannot build the portal's image filter: {err}"))?,
    );
    if let Some(folder) = current.and_then(std::path::Path::parent) {
        let mut bytes = folder.as_os_str().as_encoded_bytes().to_vec();
        bytes.push(0);
        options.insert("current_folder", Value::from(bytes));
    }

    let reply = conn
        .call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some(FILE_CHOOSER_IFACE),
            "OpenFile",
            &("", "Choose wallpaper", &options),
        )
        .map_err(|err| format!("The file portal is unavailable: {err}"))?;
    let handle: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|err| format!("The file portal answered with something else: {err}"))?;
    if handle.as_str() != path {
        tracing::warn!(
            expected = %path,
            got = %handle.as_str(),
            "the portal ignored handle_token; watching its own path instead"
        );
        return await_response_on(&conn, handle.as_str());
    }

    await_response(&mut signals, &conn, &path)
}

/// Wait on an already-open subscription.
fn await_response(
    signals: &mut MessageIterator,
    conn: &Connection,
    path: &str,
) -> Result<PathBuf, String> {
    let (done_tx, done_rx) = crossbeam_channel::bounded::<Result<PathBuf, String>>(1);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for message in signals.by_ref() {
                let Ok(message) = message else { continue };
                let body = message.body();
                let outcome = match body.deserialize::<(u32, HashMap<String, OwnedValue>)>() {
                    Ok((response, results)) => response_to_outcome(response, &results),
                    Err(err) => Err(format!("Unreadable portal answer: {err}")),
                };
                let _ = done_tx.send(outcome);
                return;
            }
            let _ = done_tx.send(Err("The file portal closed its connection".to_string()));
        });
        match done_rx.recv_timeout(PORTAL_TIMEOUT) {
            Ok(outcome) => outcome,
            Err(_) => {
                // Close the request so the portal answers and the reader
                // thread this scope is waiting on can finish.
                let _ = conn.call_method(Some(PORTAL_BUS), path, Some(REQUEST_IFACE), "Close", &());
                done_rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or_else(|_| Err("The file portal did not answer".to_string()))
            }
        }
    })
}

/// The same wait, but subscribing after the fact — only reached when a portal
/// ignores `handle_token` and hands back a path of its own choosing.
fn await_response_on(conn: &Connection, path: &str) -> Result<PathBuf, String> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .member("Response")
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .path(path)
        .map_err(|err| format!("Bad portal request path: {err}"))?
        .build();
    let mut signals = MessageIterator::for_match_rule(rule, conn, Some(4))
        .map_err(|err| format!("Cannot listen for the portal's answer: {err}"))?;
    await_response(&mut signals, conn, path)
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
