//! The `org.freedesktop.portal.FileChooser` wallpaper picker (spec D5).
//!
//! One worker thread, blocking zbus, answering on P0's inbox. The `Entry`
//! beside the Browse button is always live and always authoritative
//! (contract §2.6) — this worker is a convenience, and every one of its
//! failure modes is a single `Msg::WallpaperPickerFailed` the status line
//! shows, never a panic and never a hang — and a *cancel* is neither of those
//! (amendment P2-D16): it answers `Msg::WallpaperPickerCancelled`, which says
//! so in the status line and leaves the Browse button live.

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

/// What one `OpenFile` exchange ended as.
///
/// Three states, not two (fix wave, contract amendment P2-D16). Contract
/// §2.6's `portal_available` latch was written for "the portal is
/// unavailable", and pressing Escape in a file chooser that works perfectly
/// is not that: folding a cancel into the failure path greyed Browse out for
/// the rest of the session the first time a user changed their mind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalOutcome {
    /// The portal returned a local path this app can open.
    Chosen(PathBuf),
    /// The user dismissed the chooser. Nothing is wrong with the portal.
    Cancelled,
    /// The portal is absent, wedged, or answered with something unusable.
    /// The message is what the status line shows.
    Failed(String),
}

/// Turn one `org.freedesktop.portal.Request.Response` body into an outcome.
///
/// `response` is `0` for success, `1` for "the user cancelled" and `2` for
/// "ended some other way" (which is also what our own `Request.Close` on a
/// timeout produces — a wedged portal, so a failure).
#[must_use]
pub fn response_to_outcome(
    response: u32,
    results: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> PortalOutcome {
    match response {
        0 => {}
        1 => return PortalOutcome::Cancelled,
        other => {
            return PortalOutcome::Failed(format!(
                "The file portal ended the request (code {other})"
            ));
        }
    }
    let uris = results
        .get("uris")
        .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
        .unwrap_or_default();
    uris.iter()
        .find_map(|uri| decode_file_uri(uri))
        .map_or_else(
            || {
                PortalOutcome::Failed(
                    "The file portal returned no file this app can open".to_string(),
                )
            },
            PortalOutcome::Chosen,
        )
}

/// What the loop asks the portal worker for.
pub enum PortalRequest {
    /// Open the file chooser, starting at `current`'s directory if it has one.
    OpenFile { current: Option<PathBuf> },
    /// Stop after the requests already queued. Sent by `WorkerHandles::
    /// shutdown` so the app can wait for an exchange in flight instead of
    /// exiting under it.
    Shutdown,
}

/// How long the worker waits for a `Response` before closing the request.
///
/// A file dialog is user-driven, so this is a ceiling on a wedged portal, not
/// a UX budget: on expiry the worker calls `Request.Close`, which makes the
/// portal answer with a non-zero response, and the user sees a status line
/// instead of a Browse button that never comes back.
pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the worker waits for a session bus to hand it a connection.
///
/// `Connection::session()` is blocking with no deadline of its own: a bus
/// socket that accepts and then never authenticates parked the sole worker
/// thread forever, and every later Browse click vanished with no `Msg` at
/// all — exactly the hang `await_response`'s own doc claims was fixed.
/// The connect runs on a helper thread and this is how long its answer is
/// waited for.
pub const PORTAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The deadline zbus itself puts on every method call this worker makes —
/// `OpenFile` and the post-timeout `Request.Close`, both of which were
/// unbounded (zbus 5's `method_timeout` is `None` unless asked for).
pub const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a `Request.Close` is given to make a merely slow portal answer.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// One worker thread serving `rx` until every sender is dropped.
///
/// Per request: run the whole `OpenFile` exchange, then send exactly one of
/// [`Msg::WallpaperChosen`], [`Msg::WallpaperPickerCancelled`] (amendment
/// P2-D16) or [`Msg::WallpaperPickerFailed`]. A send error means the app
/// exited, and the thread returns.
///
/// **Superseding.** A request found waiting behind another is the one the
/// user meant: the worker drains everything already queued and serves only
/// the last, answering once.
pub fn spawn(
    rx: crossbeam_channel::Receiver<PortalRequest>,
    tx: InboxSender<Msg>,
) -> Option<std::thread::JoinHandle<()>> {
    spawn_on_bus(rx, tx, None)
}

/// [`spawn`], against one named bus address rather than `$DBUS_SESSION_BUS_
/// ADDRESS`.
///
/// The environment is process-global: a test that pointed the worker at a
/// dead bus by `unsafe set_var` doctored it for every other test in the same
/// binary, and on edition 2024 does so through a call that is undefined
/// behaviour the moment another thread reads the environment. The address
/// travels per connection instead.
pub fn spawn_on_bus(
    rx: crossbeam_channel::Receiver<PortalRequest>,
    tx: InboxSender<Msg>,
    address: Option<String>,
) -> Option<std::thread::JoinHandle<()>> {
    // Named, and `Builder::spawn` rather than `std::thread::spawn`: under
    // thread exhaustion the latter panics, and §2.4 wants this to degrade.
    // The caller keeps a live `Sender` either way; with no worker behind it a
    // Browse click simply never answers, which the status line already covers,
    // and the app boots.
    std::thread::Builder::new()
        .name("settings-portal".to_string())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut request = first;
                while let Ok(newer) = rx.try_recv() {
                    request = newer;
                }
                let current = match request {
                    PortalRequest::Shutdown => return,
                    PortalRequest::OpenFile { current } => current,
                };
                let msg = match open_file(current.as_deref(), address.as_deref()) {
                    PortalOutcome::Chosen(path) => Msg::WallpaperChosen(path),
                    PortalOutcome::Cancelled => Msg::WallpaperPickerCancelled,
                    PortalOutcome::Failed(reason) => Msg::WallpaperPickerFailed(reason),
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
        })
        .map_err(|err| tracing::warn!(%err, "no file-portal worker thread"))
        .ok()
}

/// A bus connection with a deadline on the connect *and* on every method call
/// made through it.
///
/// The connect runs on its own short-lived thread because zbus offers no
/// deadline for it; on expiry this returns and the helper is abandoned to
/// finish or fail on its own, holding nothing the worker loop needs.
fn connect(address: Option<&str>) -> Result<Connection, String> {
    let address = address.map(str::to_owned);
    let (tx, rx) = crossbeam_channel::bounded::<Result<Connection, String>>(1);
    let spawned = std::thread::Builder::new()
        .name("settings-portal-connect".to_string())
        .spawn(move || {
            let built = match address {
                Some(address) => zbus::blocking::connection::Builder::address(address.as_str()),
                None => zbus::blocking::connection::Builder::session(),
            }
            .and_then(|builder| builder.method_timeout(PORTAL_CALL_TIMEOUT).build())
            .map_err(|err| format!("No session bus for the file portal: {err}"));
            let _ = tx.send(built);
        });
    if let Err(err) = spawned {
        return Err(format!("Cannot reach the file portal: {err}"));
    }
    rx.recv_timeout(PORTAL_CONNECT_TIMEOUT)
        .unwrap_or_else(|_| Err("The session bus did not answer".to_string()))
}

/// The whole exchange, blocking, on the worker thread.
///
/// Subscribe first (the request path is derived, not read off the reply, so a
/// fast answer cannot be missed), then call `OpenFile`, then wait for the
/// `Response` signal on that path.
fn open_file(current: Option<&std::path::Path>, address: Option<&str>) -> PortalOutcome {
    match open_file_inner(current, address) {
        Ok(outcome) => outcome,
        Err(reason) => PortalOutcome::Failed(reason),
    }
}

/// The body of [`open_file`], written with `?` over the failure strings.
fn open_file_inner(
    current: Option<&std::path::Path>,
    address: Option<&str>,
) -> Result<PortalOutcome, String> {
    let conn = connect(address)?;
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_string())
        .ok_or_else(|| "The session bus gave this app no unique name".to_string())?;
    let token = format!("icedtea_{}", std::process::id());
    let path = request_object_path(&unique, &token);

    // Deliberately *not* filtered on the request path: a portal that ignores
    // `handle_token` answers on a path of its own choosing, and re-subscribing
    // after reading that path back off the reply is a race the portal can win
    // (the `Response` can arrive between the two). One subscription, opened
    // before `OpenFile` is even called, covers both paths; which one is ours
    // is decided when the reply names it, and `await_response` filters.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .member("Response")
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, Some(16))
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
    }

    Ok(await_response(signals, &conn, handle.as_str()))
}

/// Wait on an already-open subscription.
///
/// The signal reader runs on a **detached** thread, not inside a
/// `std::thread::scope` (fix wave). A scope's implicit join is what made
/// [`PORTAL_TIMEOUT`] decorative: on expiry this function called
/// `Request.Close` and waited five more seconds, but then the scope still
/// blocked until the reader returned — and the reader is parked in
/// `MessageIterator::next()`. A portal wedged badly enough not to answer its
/// own `Close` (exactly the case the timeout exists for) therefore hung
/// `open_file` for the process's lifetime, so [`spawn`]'s `rx.recv()` loop
/// never ran again and every later Browse click vanished with no `Msg` at
/// all: no status line, and a Browse button still sensitive because only a
/// `WallpaperPickerFailed` clears that latch and none could be sent.
///
/// Detached, this function always returns within `PORTAL_TIMEOUT + 5s`. The
/// reader may outlive it; the channel is `bounded(1)`, so its send never
/// blocks and it exits the moment the connection produces anything at all,
/// and it holds nothing the loop thread needs.
///
/// `signals` is taken by value because that is what detaching requires.
fn await_response(signals: MessageIterator, conn: &Connection, path: &str) -> PortalOutcome {
    let (done_tx, done_rx) = crossbeam_channel::bounded::<PortalOutcome>(1);
    let wanted = path.to_string();
    let reader = std::thread::Builder::new()
        .name("settings-portal-reply".to_string())
        .spawn(move || {
            let mut signals = signals;
            for message in signals.by_ref() {
                let Ok(message) = message else { continue };
                // The subscription is by interface and member, not by path
                // (see `open_file_inner`), so somebody else's `Response` is
                // skipped here rather than answered.
                let ours = message
                    .header()
                    .path()
                    .is_some_and(|p| p.as_str() == wanted);
                if !ours {
                    continue;
                }
                let body = message.body();
                let outcome = match body.deserialize::<(u32, HashMap<String, OwnedValue>)>() {
                    Ok((response, results)) => response_to_outcome(response, &results),
                    Err(err) => PortalOutcome::Failed(format!("Unreadable portal answer: {err}")),
                };
                let _ = done_tx.send(outcome);
                return;
            }
            let _ = done_tx.send(PortalOutcome::Failed(
                "The file portal closed its connection".to_string(),
            ));
        });
    // Same degradation as `spawn`'s own: no reader thread is a failed pick,
    // never a panic.
    if let Err(err) = reader {
        return PortalOutcome::Failed(format!("Cannot wait for the file portal's answer: {err}"));
    }
    match done_rx.recv_timeout(PORTAL_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(_) => {
            // Close the request so a merely slow portal still answers. Bounded
            // by the connection's own `method_timeout`, not by hope.
            let _ = conn.call_method(Some(PORTAL_BUS), path, Some(REQUEST_IFACE), "Close", &());
            let answer = done_rx.recv_timeout(CLOSE_GRACE);
            // Whatever happened, this exchange is over: closing the
            // connection is what releases the reader thread (parked in
            // `MessageIterator::next`), the connection itself and the match
            // rule on the bus. Left open, every timed-out pick leaked all
            // three for the life of the process. Each request builds its own
            // connection, so nothing else is closed with it.
            let _ = conn.clone().close();
            answer.unwrap_or(PortalOutcome::Failed(
                "The file portal did not answer".to_string(),
            ))
        }
    }
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

    fn failure(outcome: PortalOutcome) -> String {
        match outcome {
            PortalOutcome::Failed(reason) => reason,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// A cancel is its own outcome, not a failure.
    ///
    /// Mutation check: map `1` to `PortalOutcome::Failed` (what this code did
    /// before amendment P2-D16) and the first assertion fails. Restore.
    #[test]
    fn a_cancelled_response_is_its_own_outcome_and_a_closed_request_is_a_failure() {
        assert_eq!(
            response_to_outcome(1, &results(vec![])),
            PortalOutcome::Cancelled,
            "pressing Escape in a working chooser must not latch Browse off"
        );
        let err = failure(response_to_outcome(2, &results(vec![])));
        assert!(!err.is_empty());
    }

    #[test]
    fn an_empty_or_missing_uri_list_is_a_failure_message() {
        let err = failure(response_to_outcome(0, &results(vec![])));
        assert!(err.contains("no file"), "{err:?}");
        let empty: Vec<String> = Vec::new();
        let err = failure(response_to_outcome(
            0,
            &results(vec![("uris", Value::from(empty))]),
        ));
        assert!(err.contains("no file"), "{err:?}");
    }

    #[test]
    fn the_first_local_uri_wins() {
        let uris = vec!["file:///tmp/one.png".to_string()];
        assert_eq!(
            response_to_outcome(0, &results(vec![("uris", Value::from(uris))])),
            PortalOutcome::Chosen(PathBuf::from("/tmp/one.png"))
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
