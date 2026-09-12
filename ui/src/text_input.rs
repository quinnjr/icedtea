//! The `zwp_text_input_v3` client: IME composition for entry-family widgets.
//!
//! Pure protocol logic lives here so libtest covers it without a compositor:
//! [`apply_commit_string`]/[`apply_delete_surrounding`] splice IME-delivered
//! text into a buffer, [`Preedit`] is display-only state that never touches
//! the buffer, [`PendingState`] diffs the double-buffered client state so
//! unchanged state is never re-sent, and [`ContentPurpose`]/[`ContentHint`]
//! map to the wire `content_type` values. The live proxy wrapper
//! ([`TextInputConn`]) and the `Window` wiring sit on top of this.

use crate::css::node::Node;
use crate::layout::{LayoutTree, Rect};
use crate::widgets::edit::clamp_to_boundary;

/// Apply an IME `commit_string` to `buffer`: replace the selection
/// (`cursor`..`anchor`, `None` anchor means no selection) with `text`.
///
/// Returns the new buffer and the new cursor (byte offset just past the
/// inserted text). Untrusted input never panics: offsets are clamped to
/// `char` boundaries first.
#[must_use]
pub fn apply_commit_string(
    buffer: &str,
    cursor: usize,
    anchor: Option<usize>,
    text: &str,
) -> (String, usize) {
    let cursor = clamp_to_boundary(buffer, cursor);
    let anchor = clamp_to_boundary(buffer, anchor.unwrap_or(cursor));
    let start = anchor.min(cursor);
    let end = anchor.max(cursor);
    let mut out = String::with_capacity(buffer.len() + text.len());
    out.push_str(&buffer[..start]);
    out.push_str(text);
    out.push_str(&buffer[end..]);
    (out, start + text.len())
}

/// Apply an IME `delete_surrounding_text` to `buffer`: delete `before_len`
/// bytes before the cursor and `after_len` bytes after it.
///
/// Lengths are UTF-8 bytes per the protocol. Both ends are walked down to
/// the nearest `char` boundary, so only whole characters are ever removed
/// and this never panics, even when the IME names a range that splits a
/// codepoint. Returns the new buffer and the new cursor.
#[must_use]
pub fn apply_delete_surrounding(
    buffer: &str,
    cursor: usize,
    before_len: u32,
    after_len: u32,
) -> (String, usize) {
    let cursor = clamp_to_boundary(buffer, cursor);
    let before = (before_len as usize).min(cursor);
    let mut start = cursor - before;
    while !buffer.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (cursor.saturating_add(after_len as usize)).min(buffer.len());
    while !buffer.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(buffer.len().saturating_sub(end - start));
    out.push_str(&buffer[..start]);
    out.push_str(&buffer[end..]);
    (out, start)
}

/// An in-flight IME preedit: display-only text that never touches the buffer
/// until a `commit_string` arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preedit {
    /// The composing text.
    pub text: String,
    /// Byte offset of the cursor within [`Preedit::text`].
    pub cursor_begin: i32,
    /// Selection end within [`Preedit::text`].
    pub cursor_end: i32,
}

/// The toolkit-side content purpose, mapped 1:1 to the wire
/// `content_purpose` enum (`text-input-unstable-v3.xml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentPurpose {
    Normal,
    Password,
    Terminal,
}

impl ContentPurpose {
    /// The wire value.
    #[must_use]
    pub fn wire(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Password => 8,
            Self::Terminal => 13,
        }
    }
}

/// The toolkit-side content hint bitfield, mapped 1:1 to the wire
/// `content_hint` bitfield.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHint(u32);

impl ContentHint {
    /// No special behavior.
    pub const NONE: Self = Self(0);
    /// Characters should be hidden.
    pub const HIDDEN_TEXT: Self = Self(0x40);
    /// Typed text should not be stored.
    pub const SENSITIVE_DATA: Self = Self(0x80);
    /// The text input is multiline.
    pub const MULTILINE: Self = Self(0x200);

    /// The wire value.
    #[must_use]
    pub fn wire(self) -> u32 {
        self.0
    }
}

/// One snapshot of the double-buffered client state, as one `commit` sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The full buffer text.
    pub surrounding: String,
    /// Cursor byte offset, as the wire's `i32`.
    pub cursor: i32,
    /// Anchor byte offset (`== cursor` when there is no selection).
    pub anchor: i32,
    /// Wire `content_hint` bits.
    pub hint: u32,
    /// Wire `content_purpose` value.
    pub purpose: u32,
    /// Cursor rectangle in surface-local coordinates (x, y, w, h).
    pub cursor_rect: (i32, i32, i32, i32),
}

impl Snapshot {
    /// Build a snapshot from live widget state. Offsets are clamped to
    /// `char` boundaries and saturated to the wire's `i32`: controllers
    /// always hold valid offsets, but this is the trust boundary, so it
    /// does not assume that.
    #[must_use]
    pub fn new(
        buffer: &str,
        cursor: usize,
        anchor: Option<usize>,
        hint: ContentHint,
        purpose: ContentPurpose,
        cursor_rect: (i32, i32, i32, i32),
    ) -> Self {
        fn wire(offset: usize) -> i32 {
            i32::try_from(offset).unwrap_or(i32::MAX)
        }
        let cursor = clamp_to_boundary(buffer, cursor);
        let anchor = clamp_to_boundary(buffer, anchor.unwrap_or(cursor));
        Snapshot {
            surrounding: buffer.to_owned(),
            cursor: wire(cursor),
            anchor: wire(anchor),
            hint: hint.wire(),
            purpose: purpose.wire(),
            cursor_rect,
        }
    }
}

/// The caret's surface-local rectangle for `set_cursor_rectangle`.
///
/// `tree.allocation(node)` is absolute (surface space); the `caret` rect the
/// layout reports is relative to the shaped line's origin, which paints at
/// the content-box origin minus the scroll offset. Falls back to an empty
/// rect at the origin when nothing is laid out yet — the next sync after
/// layout repairs it.
#[must_use]
pub fn caret_surface_rect(
    tree: &LayoutTree,
    node: &Node,
    caret: &Rect,
    scroll: (f32, f32),
) -> (i32, i32, i32, i32) {
    let Some(alloc) = tree.allocation(node) else {
        return (0, 0, 0, 0);
    };
    let x = alloc.content_box.x - scroll.0 + caret.x;
    let y = alloc.content_box.y - scroll.1 + caret.y;
    (
        x as i32,
        y as i32,
        caret.width.max(1.0) as i32,
        caret.height as i32,
    )
}

/// The double-buffered client state: staged values are diffed against the
/// last committed ones so unchanged state is never re-sent.
#[derive(Debug, Clone, Default)]
pub struct PendingState {
    last: Option<Snapshot>,
    next: Option<Snapshot>,
}

impl PendingState {
    /// An empty state: nothing staged, nothing committed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage a snapshot; overwrites whatever was staged but uncommitted.
    pub fn stage(&mut self, snapshot: Snapshot) {
        self.next = Some(snapshot);
    }

    /// Take the staged snapshot if it differs from the last commit.
    ///
    /// Returns `None` when nothing is staged or the staged snapshot is
    /// identical to the last commit. A taken snapshot becomes the last
    /// commit.
    pub fn take_commit(&mut self) -> Option<Snapshot> {
        let next = self.next.take()?;
        if self.last.as_ref() == Some(&next) {
            return None;
        }
        self.last = Some(next.clone());
        Some(next)
    }

    /// Forget the last commit, so the next staged snapshot is always sent.
    /// Used on `enable`: the relay treats enable as fresh state.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentHint, ContentPurpose, PendingState, Snapshot, apply_commit_string};

    fn snapshot(surrounding: &str) -> Snapshot {
        Snapshot {
            surrounding: surrounding.to_owned(),
            cursor: surrounding.len() as i32,
            anchor: surrounding.len() as i32,
            hint: 0,
            purpose: 0,
            cursor_rect: (0, 0, 0, 0),
        }
    }

    #[test]
    fn a_commit_string_replaces_the_selection_and_advances_the_cursor() {
        // mutation: splice at the cursor instead of the selection and the
        // selected text survives alongside the insertion.
        let (buffer, cursor) = apply_commit_string("hello world", 5, Some(11), "there");
        assert_eq!(buffer, "hellothere");
        assert_eq!(cursor, 10);
    }

    #[test]
    fn a_commit_string_with_no_selection_inserts_at_a_mid_cjk_cursor() {
        // mutation: use `cursor + 1` and the insertion lands inside the next
        // codepoint's bytes.
        let buffer = "あいう";
        let (out, cursor) = apply_commit_string(buffer, 6, None, "え");
        assert_eq!(out, "あいえう");
        assert_eq!(cursor, 9);
    }

    #[test]
    fn a_commit_string_clamps_a_mid_codepoint_cursor_to_a_boundary() {
        // mutation: skip `clamp_to_boundary` and the splice panics on
        // `&buffer[..start]` with a non-boundary index.
        let buffer = "a\u{1F600}c";
        let (out, cursor) = apply_commit_string(buffer, 3, None, "z");
        assert_eq!(out, "az\u{1F600}c");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn delete_surrounding_removes_whole_characters_only() {
        // mutation: slice at the raw byte offsets and this panics on the
        // emoji; mutation: walk the end *up* and this eats the trailing 'c'.
        let buffer = "a\u{1F600}c";
        let (out, cursor) = super::apply_delete_surrounding(buffer, 5, 4, 0);
        assert_eq!(out, "ac");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn delete_surrounding_clamps_past_both_ends() {
        // mutation: drop the `.min` clamps and the subtraction underflows /
        // the end slice reads past the buffer.
        let (out, cursor) = super::apply_delete_surrounding("ab", 1, 99, 99);
        assert_eq!(out, "");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn an_unchanged_snapshot_is_never_recommitted() {
        // mutation: always return the staged snapshot and the second commit
        // is `Some`, re-sending identical surrounding text on every keystroke.
        let mut pending = PendingState::new();
        pending.stage(snapshot("hi"));
        assert!(pending.take_commit().is_some());
        pending.stage(snapshot("hi"));
        assert_eq!(
            pending.take_commit(),
            None,
            "identical state must not recommit"
        );
        pending.stage(snapshot("hi!"));
        assert!(pending.take_commit().is_some(), "changed state must commit");
    }

    #[test]
    fn reset_forces_the_next_snapshot_through() {
        // mutation: drop `reset`'s body and enable carries a stale diff.
        let mut pending = PendingState::new();
        pending.stage(snapshot("hi"));
        assert!(pending.take_commit().is_some());
        pending.reset();
        pending.stage(snapshot("hi"));
        assert!(
            pending.take_commit().is_some(),
            "post-enable state always sends"
        );
    }

    #[test]
    fn content_enums_carry_the_protocol_wire_values() {
        // mutation: renumber any variant and the IME offers the wrong layout.
        assert_eq!(ContentPurpose::Normal.wire(), 0);
        assert_eq!(ContentPurpose::Password.wire(), 8);
        assert_eq!(ContentPurpose::Terminal.wire(), 13);
        assert_eq!(ContentHint::NONE.wire(), 0);
        assert_eq!(ContentHint::HIDDEN_TEXT.wire(), 0x40);
        assert_eq!(ContentHint::SENSITIVE_DATA.wire(), 0x80);
        assert_eq!(ContentHint::MULTILINE.wire(), 0x200);
    }
}
