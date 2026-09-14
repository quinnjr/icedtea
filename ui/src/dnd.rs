//! Toolkit-internal drag-and-drop: MIME payloads, the
//! press → threshold → dragging → drop/cancel session machine, per-node
//! drop-target state, and the compositor seat seam.
//!
//! This module knows nothing about widgets, nodes or the Wayland seat. The
//! reactive layer (`view::app`) owns one [`DragSession`] per window plus the
//! source/target node bookkeeping; controllers expose offers and acceptance
//! through it. Cross-surface drags are the compositor's job — see
//! [`DragSeat`] and §7 of
//! `docs/superpowers/specs/2026-09-12-m6-toolkit-dnd-design.md`.

/// `text/plain` (UTF-8): the only v1 payload flavor with a decoder.
pub const TEXT_PLAIN: &str = "text/plain";

/// `text/uri-list` (UTF-8, CR/LF-separated): offered and negotiated v1, read
/// as text through the same `Text` handler binding.
pub const TEXT_URI_LIST: &str = "text/uri-list";

/// The class a drop target carries exactly while a drag hovers it.
///
/// Theme business what it paints (Adwaita styles nothing for it); the
/// toolkit guarantees its presence, and the acceptance tests assert it.
pub const DROP_ACTIVE_CLASS: &str = "drop-target-active";

/// Press-to-drag distance, in px. GTK's `gtk_drag_check_threshold` parity:
/// a press that never moves this far is a click, never a drag.
pub const DRAG_THRESHOLD_PX: f32 = 8.0;

/// Whether `at` is far enough from the press `origin` to start a drag.
///
/// Euclidean distance strictly greater than [`DRAG_THRESHOLD_PX`]: at
/// exactly the threshold the press is still a click.
#[must_use]
pub fn beyond_threshold(origin: (f32, f32), at: (f32, f32)) -> bool {
    let (dx, dy) = (at.0 - origin.0, at.1 - origin.1);
    dx.hypot(dy) > DRAG_THRESHOLD_PX
}

/// One MIME-typed chunk of a drag payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragEntry {
    mime: String,
    data: Vec<u8>,
}

impl DragEntry {
    /// The offered MIME type.
    #[must_use]
    pub fn mime(&self) -> &str {
        &self.mime
    }

    /// The payload bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// What a drag source offers: an ordered list of MIME-typed chunks.
///
/// Order is source preference order — [`DragPayload::data_for`] picks the
/// first chunk the target accepts, mirroring `wl_data_device` offer
/// negotiation narrowed to one surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragPayload {
    entries: Vec<DragEntry>,
}

impl DragPayload {
    /// A single `text/plain` offer.
    #[must_use]
    pub fn offer_text(text: &str) -> Self {
        DragPayload {
            entries: vec![DragEntry {
                mime: TEXT_PLAIN.to_owned(),
                data: text.as_bytes().to_vec(),
            }],
        }
    }

    /// An empty offer, extended with [`DragPayload::add`].
    #[must_use]
    pub fn empty() -> Self {
        DragPayload {
            entries: Vec::new(),
        }
    }

    /// Append one more flavor; earlier (more-preferred) entries are untouched.
    pub fn add(&mut self, mime: &str, data: Vec<u8>) {
        self.entries.push(DragEntry {
            mime: mime.to_owned(),
            data,
        });
    }

    /// Every offered MIME type, in preference order.
    #[must_use]
    pub fn offered_mimes(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.mime.as_str()).collect()
    }

    /// The first offered chunk whose MIME is in `accepted`, or `None` when
    /// source and target share no flavor.
    #[must_use]
    pub fn data_for(&self, accepted: &[&str]) -> Option<(&str, &[u8])> {
        self.entries
            .iter()
            .find(|e| accepted.contains(&e.mime.as_str()))
            .map(|e| (e.mime.as_str(), e.data.as_slice()))
    }

    /// Whether anything is offered at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many flavors are offered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The target side of the contract: which flavors a node takes, and whether
/// a drag is hovering it right now.
///
/// The retained tree is the registry — one `DropTarget` lives in the target
/// controller, so there is no second map to go stale when reconcile moves
/// nodes around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropTarget {
    accepted: Vec<String>,
    highlighted: bool,
}

impl DropTarget {
    /// Accepts `text/plain` only.
    #[must_use]
    pub fn accept_plain() -> Self {
        DropTarget {
            accepted: vec![TEXT_PLAIN.to_owned()],
            highlighted: false,
        }
    }

    /// Accepts a comma-separated MIME list (`"text/plain, text/uri-list"`).
    /// Empty segments are dropped; an empty list accepts nothing.
    #[must_use]
    pub fn accept_list(mimes: &str) -> Self {
        DropTarget {
            accepted: mimes
                .split(',')
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
                .collect(),
            highlighted: false,
        }
    }

    /// Whether any of `offered` is acceptable.
    #[must_use]
    pub fn accepts(&self, offered: &[&str]) -> bool {
        offered.iter().any(|m| self.accepted.iter().any(|a| a == m))
    }

    /// Whether this target and `payload` share a flavor.
    #[must_use]
    pub fn accepts_payload(&self, payload: &DragPayload) -> bool {
        self.accepts(&payload.offered_mimes())
    }

    /// The accepted MIME list, in the order it was declared.
    #[must_use]
    pub fn accepted_mimes(&self) -> Vec<&str> {
        self.accepted.iter().map(String::as_str).collect()
    }

    /// Mark the target hovered (or not) by an in-flight drag.
    pub fn set_highlighted(&mut self, on: bool) {
        self.highlighted = on;
    }

    /// Whether a drag is hovering this target right now.
    #[must_use]
    pub fn is_highlighted(&self) -> bool {
        self.highlighted
    }
}

/// What [`DragSession::motion`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragTransition {
    /// No session is active; the motion is an ordinary hover.
    None,
    /// Armed, still within the threshold.
    StillArmed,
    /// This motion crossed the threshold: a drag begins here.
    BeganDragging,
    /// An in-flight drag moved.
    Moved,
}

/// What [`DragSession::release`] settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// No session was active.
    Nothing,
    /// Armed but never dragged: the normal click path proceeds.
    ClickThrough,
    /// A drag was in flight; the caller now drops or cancels by target.
    FinishedDrag,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SessionState {
    Idle,
    Armed {
        origin: (f32, f32),
        button: u32,
        serial: u32,
    },
    Dragging {
        origin: (f32, f32),
        pos: (f32, f32),
        /// The arming press serial, kept for the seat offer (§7): the
        /// compositor validates it when the drag leaves the surface.
        serial: u32,
    },
}

/// The press → threshold → dragging machine, without any node knowledge.
///
/// The window owns one and keeps the source/target nodes beside it: this
/// stays unit-testable over bare positions. Node bookkeeping (which node is
/// the source, which is hovered) lives in `view::app`, which is the only
/// place with both the session and the hit-test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragSession {
    state: SessionState,
}

impl Default for DragSession {
    fn default() -> Self {
        DragSession::new()
    }
}

impl DragSession {
    /// An idle session.
    #[must_use]
    pub fn new() -> Self {
        DragSession {
            state: SessionState::Idle,
        }
    }

    /// Whether no press is tracked.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.state == SessionState::Idle
    }

    /// Whether a press is tracked but the threshold not yet crossed.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        matches!(self.state, SessionState::Armed { .. })
    }

    /// Whether a drag is in flight.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        matches!(self.state, SessionState::Dragging { .. })
    }

    /// Track a press at `at`. Returns `false` — and keeps the old state —
    /// when a press or drag is already active: a second button never re-arms.
    pub fn begin_press(&mut self, at: (f32, f32), button: u32, serial: u32) -> bool {
        if self.state != SessionState::Idle {
            return false;
        }
        self.state = SessionState::Armed {
            origin: at,
            button,
            serial,
        };
        true
    }

    /// The arming press serial, for the future seat offer (§7 of the design).
    /// `None` when idle; kept through the whole drag so `DragStart` can
    /// still hand it to the seat.
    #[must_use]
    pub fn serial(&self) -> Option<u32> {
        match self.state {
            SessionState::Armed { serial, .. } | SessionState::Dragging { serial, .. } => {
                Some(serial)
            }
            SessionState::Idle => None,
        }
    }

    /// Feed a pointer position. Crossing the threshold reports
    /// [`DragTransition::BeganDragging`] exactly once per press.
    pub fn motion(&mut self, at: (f32, f32)) -> DragTransition {
        match self.state {
            SessionState::Idle => DragTransition::None,
            SessionState::Armed { origin, serial, .. } => {
                if beyond_threshold(origin, at) {
                    self.state = SessionState::Dragging {
                        origin,
                        pos: at,
                        serial,
                    };
                    DragTransition::BeganDragging
                } else {
                    DragTransition::StillArmed
                }
            }
            SessionState::Dragging { origin, serial, .. } => {
                self.state = SessionState::Dragging {
                    origin,
                    pos: at,
                    serial,
                };
                DragTransition::Moved
            }
        }
    }

    /// Feed a release of the arming button. Always settles to idle: there is
    /// no path that keeps a session alive past its button.
    pub fn release(&mut self) -> DragOutcome {
        let out = match self.state {
            SessionState::Idle => DragOutcome::Nothing,
            SessionState::Armed { .. } => DragOutcome::ClickThrough,
            SessionState::Dragging { .. } => DragOutcome::FinishedDrag,
        };
        self.state = SessionState::Idle;
        out
    }

    /// Abandon the session (`Escape`, or a grab break). Returns whether
    /// anything was active — `false` on idle means the caller owes no
    /// `DragLeave`/`DragEnd` notifications.
    pub fn cancel(&mut self) -> bool {
        let active = self.state != SessionState::Idle;
        self.state = SessionState::Idle;
        active
    }
}

/// What the toolkit would hand the compositor seat to start a cross-surface
/// drag: the arming press serial (M4.2's grab-serial validation input), the
/// offered MIME list, and the bytes the seat must serve on `receive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatDragRequest {
    /// The press serial that armed this drag.
    pub serial: u32,
    /// Offered flavors, in source preference order.
    pub mimes: Vec<String>,
    /// The payload bytes, keyed by MIME, for the seat's `wl_data_source.send`.
    ///
    /// Kept beside `mimes` (rather than derivable from it) so a stub seat can
    /// still assert the request's *shape* without reading the payload, while a
    /// real seat has the bytes it must write to the destination's pipe.
    pub payload: DragPayload,
}

impl SeatDragRequest {
    /// The request a toolkit drag start produces for `serial` over `payload`.
    #[must_use]
    pub fn new(serial: u32, payload: DragPayload) -> Self {
        let mimes = payload
            .offered_mimes()
            .iter()
            .map(|m| (*m).to_owned())
            .collect();
        SeatDragRequest {
            serial,
            mimes,
            payload,
        }
    }
}

/// The seat's answer to [`DragSeat::offer_drag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatDragResult {
    /// The serial validated and the drag is live on the seat.
    Accepted,
    /// Bad serial or no seat: the drag stays toolkit-internal (or dies).
    Refused,
}

/// The compositor-seat seam (§7 of the design).
///
/// The windowed runtime holds the real implementation
/// (`window::selection::ClientSeat`), which validates `request.serial`
/// against the compositor's grant table (wlroots' M4.2 decision-3 gate) and
/// calls `wl_data_device.start_drag`; an offscreen run or a unit test holds
/// none or a [`StubSeat`], and the drag stays toolkit-internal.
pub trait DragSeat {
    /// Offer a drag to the seat.
    fn offer_drag(&mut self, request: &SeatDragRequest) -> SeatDragResult;
}

/// A scripted [`DragSeat`]: records every request, answers accept/refuse on
/// command. What the unit tests — and later the compositor wiring tests —
/// drive instead of a live seat.
#[derive(Debug, Default)]
pub struct StubSeat {
    accept: bool,
    log: Vec<SeatDragRequest>,
}

impl StubSeat {
    /// A stub that accepts every offer.
    #[must_use]
    pub fn accepting() -> Self {
        StubSeat {
            accept: true,
            log: Vec::new(),
        }
    }

    /// A stub that refuses every offer (bad serial, seatless surface).
    #[must_use]
    pub fn refusing() -> Self {
        StubSeat {
            accept: false,
            log: Vec::new(),
        }
    }

    /// Every request offered so far, in order.
    #[must_use]
    pub fn log(&self) -> &[SeatDragRequest] {
        &self.log
    }
}

impl DragSeat for StubSeat {
    fn offer_drag(&mut self, request: &SeatDragRequest) -> SeatDragResult {
        self.log.push(request.clone());
        if self.accept {
            SeatDragResult::Accepted
        } else {
            SeatDragResult::Refused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_threshold_is_strictly_greater_than_eight_px() {
        let origin = (10.0f32, 10.0f32);
        assert!(!beyond_threshold(origin, origin));
        assert!(!beyond_threshold(origin, (18.0, 10.0)));
        assert!(!beyond_threshold(origin, (10.0 + 8.0_f32.hypot(0.0), 10.0)));
        assert!(beyond_threshold(origin, (18.1, 10.0)));
        assert!(beyond_threshold(origin, (10.0, 18.1)));
        // Diagonal: 5/5 stays under, 6/6 crosses.
        assert!(!beyond_threshold(origin, (15.0, 15.0)));
        assert!(beyond_threshold(origin, (16.0, 16.0)));
    }

    #[test]
    fn negotiation_picks_the_first_offer_the_target_takes() {
        let mut payload = DragPayload::empty();
        payload.add(TEXT_URI_LIST, b"file:///a".to_vec());
        payload.add(TEXT_PLAIN, b"hello".to_vec());
        // Source preference order wins, not target order.
        assert_eq!(
            payload.data_for(&[TEXT_PLAIN, TEXT_URI_LIST]),
            Some((TEXT_URI_LIST, b"file:///a".as_slice()))
        );
        assert_eq!(
            payload.data_for(&[TEXT_PLAIN]),
            Some((TEXT_PLAIN, b"hello".as_slice()))
        );
        assert_eq!(payload.data_for(&["application/x-nothing"]), None);
        assert_eq!(DragPayload::empty().data_for(&[TEXT_PLAIN]), None);
    }

    #[test]
    fn offer_text_is_a_single_plain_chunk() {
        let payload = DragPayload::offer_text("hi");
        assert_eq!(payload.offered_mimes(), vec![TEXT_PLAIN]);
        assert_eq!(
            payload.data_for(&[TEXT_PLAIN]),
            Some((TEXT_PLAIN, b"hi".as_slice()))
        );
        assert!(!payload.is_empty());
        assert_eq!(payload.len(), 1);
    }

    #[test]
    fn a_drop_target_parses_its_mime_list_and_tracks_highlight() {
        let target = DropTarget::accept_list("text/plain, text/uri-list ,");
        assert_eq!(target.accepted_mimes(), vec![TEXT_PLAIN, TEXT_URI_LIST]);
        assert!(target.accepts(&[TEXT_URI_LIST]));
        assert!(!target.accepts(&["application/x-nothing"]));
        assert!(target.accepts_payload(&DragPayload::offer_text("x")));
        assert!(!DropTarget::accept_list("").accepts(&[TEXT_PLAIN]));
        assert!(DropTarget::accept_plain().accepts(&[TEXT_PLAIN]));

        let mut target = DropTarget::accept_plain();
        assert!(!target.is_highlighted());
        target.set_highlighted(true);
        assert!(target.is_highlighted());
        target.set_highlighted(false);
        assert!(!target.is_highlighted());
    }

    #[test]
    fn the_session_runs_press_threshold_drag_release() {
        let mut session = DragSession::new();
        assert!(session.is_idle());
        assert_eq!(session.motion((0.0, 0.0)), DragTransition::None);
        assert_eq!(session.release(), DragOutcome::Nothing);

        assert!(session.begin_press((0.0, 0.0), 0x110, 7));
        assert!(session.is_armed());
        assert_eq!(session.serial(), Some(7));
        assert_eq!(session.motion((4.0, 4.0)), DragTransition::StillArmed);
        assert!(session.is_armed());
        // The threshold crossing reports exactly once.
        assert_eq!(session.motion((0.0, 9.0)), DragTransition::BeganDragging);
        assert!(session.is_dragging());
        // The arming serial survives into the drag: it is what the future
        // seat offer validates (§7 of the design).
        assert_eq!(session.serial(), Some(7));
        assert_eq!(session.motion((0.0, 20.0)), DragTransition::Moved);
        assert_eq!(session.release(), DragOutcome::FinishedDrag);
        assert!(session.is_idle());
        assert_eq!(session.serial(), None);
    }

    #[test]
    fn release_while_armed_is_a_click_not_a_drag() {
        let mut session = DragSession::new();
        assert!(session.begin_press((5.0, 5.0), 0x110, 1));
        assert_eq!(session.motion((6.0, 6.0)), DragTransition::StillArmed);
        assert_eq!(session.release(), DragOutcome::ClickThrough);
        assert!(session.is_idle());
    }

    #[test]
    fn a_second_press_never_re_arms_and_cancel_reports_activity() {
        let mut session = DragSession::new();
        assert!(session.begin_press((0.0, 0.0), 0x110, 1));
        assert!(!session.begin_press((50.0, 50.0), 0x110, 2));
        // Still the first press: the far-away second origin did not take.
        assert_eq!(session.motion((0.0, 9.0)), DragTransition::BeganDragging);
        assert!(!session.begin_press((0.0, 0.0), 0x110, 3));
        assert!(session.cancel());
        assert!(session.is_idle());
        assert!(!session.cancel());
    }

    #[test]
    fn the_stub_seat_logs_requests_and_answers_on_command() {
        let request = SeatDragRequest::new(41, DragPayload::offer_text("payload"));
        assert_eq!(request.mimes, vec![TEXT_PLAIN.to_owned()]);
        assert_eq!(
            request.payload.data_for(&[TEXT_PLAIN]),
            Some((TEXT_PLAIN, b"payload".as_slice()))
        );
        let mut accepting = StubSeat::accepting();
        assert_eq!(accepting.offer_drag(&request), SeatDragResult::Accepted);
        assert_eq!(accepting.log(), std::slice::from_ref(&request));

        let mut refusing = StubSeat::refusing();
        assert_eq!(refusing.offer_drag(&request), SeatDragResult::Refused);
        assert_eq!(refusing.log(), &[request]);
    }
}
