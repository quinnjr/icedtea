//! The `GtkText` engine every entry-family widget embeds.
//!
//! Contract §5.3: `GtkText`'s own shortcut and action tables are the single
//! source for editing behaviour, not the per-entry class pages. The shared
//! `text` subnode's tree is
//!
//! ```text
//! text[.read-only]
//! ├── placeholder
//! ├── undershoot.left
//! ├── undershoot.right
//! ├── [selection]
//! ├── [block-cursor]
//! ╰── [window.popup]
//! ```
//!
//! Shortcuts implemented here: `Ctrl+A`/`Ctrl+/` select all;
//! `Ctrl+Shift+A`/`Ctrl+\` unselect; `Ctrl+Z` undo; `Ctrl+Y`/`Ctrl+Shift+Z`
//! redo; `Ctrl+C`/`X`/`V` clipboard; `Clear` clears. `Ctrl+Shift+T` (toggle
//! direction) and `misc.insert-emoji` are out of scope (spec §Out of scope).

use std::time::Duration;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::LayoutTree;
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use crate::text_input::{
    ContentHint, ContentPurpose, Preedit, Snapshot, apply_commit_string, apply_delete_surrounding,
    caret_surface_rect,
};
use crate::view::BuildCx;
use crate::view::cmd::Cmd;
use crate::view::controller::{Event, EventCx};
use crate::window::keyboard::{KeyEvent, Mods};

/// Clamp `offset` into `s`, walking *down* to the nearest `char` boundary.
///
/// A caret is a byte offset into the buffer it was placed in. When the model
/// swaps the text underneath it, `offset.min(s.len())` keeps the caret inside
/// the new string but can leave it inside a multi-byte codepoint — and the
/// next `String::replace_range` over a range starting there panics. Untrusted
/// input never panics (part-5 global constraint), so every clamp of a caret,
/// an anchor or a hit-tested offset goes through here.
#[must_use]
pub fn clamp_to_boundary(s: &str, offset: usize) -> usize {
    let mut n = offset.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// What one key did, so the embedding controller knows what to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// The key was not ours.
    Ignored,
    /// The cursor or selection moved; the buffer did not change.
    Moved,
    /// The buffer changed; fire `EventKind::Change`.
    Changed,
    /// Enter was pressed; fire `EventKind::Activate`.
    Activated,
}

/// One recorded edit.
#[derive(Debug, Clone)]
struct Edit {
    before: String,
    after: String,
    cursor_before: usize,
    cursor_after: usize,
    at: Duration,
}

/// `GtkText`'s undo stack.
pub struct UndoStack {
    steps: Vec<Edit>,
    /// Index of the next step `redo` would replay; `steps.len()` at the top.
    cursor: usize,
    enabled: bool,
}

impl UndoStack {
    /// Consecutive single-character insertions within this window merge.
    const COALESCE: Duration = Duration::from_millis(1_000);
    /// The deepest the stack ever grows; older steps are dropped.
    pub const MAX_STEPS: usize = 512;

    /// A stack honouring `GtkEditable:enable-undo`.
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        UndoStack {
            steps: Vec::new(),
            cursor: 0,
            enabled,
        }
    }

    /// Record the transition from `before` to `after`.
    pub fn record(&mut self, before: &str, after: &str, cursor: usize, now: Duration) {
        if !self.enabled || before == after {
            return;
        }
        self.steps.truncate(self.cursor);
        let single_char_insert = after.len() > before.len()
            && after.starts_with(before)
            && after[before.len()..].chars().count() == 1;
        if let Some(last) = self.steps.last_mut().filter(|_| single_char_insert) {
            let recent = now.saturating_sub(last.at) < Self::COALESCE;
            let continues = last.after == before;
            if recent && continues {
                last.after = after.to_owned();
                last.cursor_after = cursor;
                last.at = now;
                return;
            }
        }
        self.steps.push(Edit {
            before: before.to_owned(),
            after: after.to_owned(),
            cursor_before: before.len().min(cursor),
            cursor_after: cursor,
            at: now,
        });
        if self.steps.len() > Self::MAX_STEPS {
            let excess = self.steps.len() - Self::MAX_STEPS;
            self.steps.drain(..excess);
        }
        self.cursor = self.steps.len();
    }

    /// Step back.
    pub fn undo(&mut self) -> Option<(String, usize)> {
        if !self.enabled || self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        let step = self.steps.get(self.cursor)?;
        Some((step.before.clone(), step.cursor_before))
    }

    /// Step forward.
    pub fn redo(&mut self) -> Option<(String, usize)> {
        if !self.enabled || self.cursor >= self.steps.len() {
            return None;
        }
        let step = self.steps.get(self.cursor)?.clone();
        self.cursor += 1;
        Some((step.after, step.cursor_after))
    }

    /// Whether `undo` would do anything.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.enabled && self.cursor > 0
    }

    /// Whether `redo` would do anything.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.enabled && self.cursor < self.steps.len()
    }
}

/// The editing state all five entry kinds embed.
pub struct TextEditState {
    /// The plain-text buffer, as the model sees it.
    pub buffer: String,
    /// The shaped single line over [`TextEditState::display`].
    pub layout: TextLayout,
    /// Cursor byte offset into `buffer`.
    pub cursor: usize,
    /// Selection anchor; `None` means no selection.
    pub anchor: Option<usize>,
    /// Horizontal scroll of the text inside the entry, in px.
    pub scroll_offset: f32,
    /// The undo stack.
    pub undo: UndoStack,
    /// `GtkText:overwrite-mode`.
    pub overwrite: bool,
    /// `GtkText:visibility`; `false` renders the invisible character.
    pub visibility: bool,
    /// `GtkText:max-length`; `None` is unlimited.
    pub max_length: Option<usize>,
    /// The `text` subnode.
    pub text_node: Node,
    /// The `placeholder` subnode.
    pub placeholder_node: Node,
    /// The in-flight IME preedit: display-only text that never touches
    /// [`TextEditState::buffer`] until a `commit_string` arrives. Painted by
    /// the embedding controller after the buffer's own text.
    pub preedit: Option<Preedit>,
    /// The `selection` subnode, present only while a selection exists.
    pub selection_node: Option<Node>,
}

impl TextEditState {
    /// GTK's own invisible character.
    const INVISIBLE: char = '\u{2022}';

    /// Build the shared `text` subtree under `parent`.
    pub fn build(parent: &Node, text: &str, cx: &mut BuildCx<'_>) -> Self {
        let text_node = Node::new("text");
        parent.append_child(&text_node);
        let placeholder_node = Node::new("placeholder");
        text_node.append_child(&placeholder_node);
        text_node.append_child(&Node::with_classes("undershoot", &["left"]));
        text_node.append_child(&Node::with_classes("undershoot", &["right"]));
        let mut this = TextEditState {
            buffer: text.to_owned(),
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            cursor: text.len(),
            anchor: None,
            scroll_offset: 0.0,
            undo: UndoStack::new(true),
            overwrite: false,
            visibility: true,
            max_length: None,
            text_node,
            placeholder_node,
            preedit: None,
            selection_node: None,
        };
        this.reshape(cx);
        this
    }

    /// Replace the buffer from a prop, without recording an undo step — the
    /// model, not the user, made this change. A model write supersedes any
    /// in-flight composition, so a pending preedit is dropped with it.
    pub fn set_text(&mut self, text: &str, cx: &mut BuildCx<'_>) {
        if self.buffer == text {
            return;
        }
        self.buffer = text.to_owned();
        self.cursor = clamp_to_boundary(&self.buffer, self.cursor);
        self.anchor = None;
        self.preedit = None;
        self.reshape(cx);
        self.sync_selection_node();
    }

    /// Reshape the layout over the currently displayed string.
    pub fn reshape(&mut self, cx: &mut BuildCx<'_>) {
        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        let display = self.display();
        self.layout = TextLayout::build(
            &display,
            &style,
            cx.fonts,
            None,
            WrapMode::None,
            Ellipsize::None,
        );
        self.placeholder_node.set_state(
            crate::css::node::PseudoStates::DISABLED,
            !self.buffer.is_empty(),
        );
    }

    /// What is actually drawn: the buffer, or one bullet per character when
    /// `visibility` is off.
    #[must_use]
    pub fn display(&self) -> String {
        if self.visibility {
            self.buffer.clone()
        } else {
            Self::INVISIBLE
                .to_string()
                .repeat(self.buffer.chars().count())
        }
    }

    /// The buffer byte offset a point inside the shaped line names.
    ///
    /// [`TextLayout::byte_at`] answers in offsets into
    /// [`TextEditState::display`], which is the buffer itself only while
    /// `visibility` is on; with it off the layout is one three-byte bullet per
    /// *character*, so a display offset has to come back through the character
    /// index to be a valid `buffer` offset (and `cursor` is documented as a
    /// `buffer` offset).
    #[must_use]
    pub fn buffer_offset_at(&self, local: (f32, f32)) -> usize {
        let display_offset = self.layout.byte_at(local);
        if self.visibility {
            return clamp_to_boundary(&self.buffer, display_offset);
        }
        let display = self.display();
        let chars = display
            .get(..display_offset)
            .map_or_else(|| display.chars().count(), |head| head.chars().count());
        self.buffer
            .char_indices()
            .nth(chars)
            .map_or(self.buffer.len(), |(byte, _)| byte)
    }

    /// The ordered, clamped selection range.
    #[must_use]
    pub fn selection(&self) -> std::ops::Range<usize> {
        let anchor = clamp_to_boundary(&self.buffer, self.anchor.unwrap_or(self.cursor));
        let cursor = clamp_to_boundary(&self.buffer, self.cursor);
        anchor.min(cursor)..anchor.max(cursor)
    }

    fn sync_selection_node(&mut self) {
        let has = !self.selection().is_empty();
        match (has, self.selection_node.take()) {
            (true, Some(node)) => self.selection_node = Some(node),
            (true, None) => {
                let node = Node::new("selection");
                self.text_node.append_child(&node);
                self.selection_node = Some(node);
            }
            (false, Some(node)) => node.detach(),
            (false, None) => {}
        }
    }

    /// Insert `text` at the cursor, replacing any selection and honouring
    /// `max_length` (counted in characters, as GTK does).
    fn insert(&mut self, text: &str, now: Duration) -> bool {
        let before = self.buffer.clone();
        let range = self.selection();
        let remaining = self.max_length.map(|max| {
            let kept = before.chars().count() - before[range.clone()].chars().count();
            max.saturating_sub(kept)
        });
        let text = match remaining {
            Some(room) => {
                let end = text
                    .char_indices()
                    .nth(room)
                    .map_or(text.len(), |(offset, _)| offset);
                &text[..end]
            }
            None => text,
        };
        if text.is_empty() && range.is_empty() {
            return false;
        }
        self.buffer.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        true
    }

    /// Stage an IME `preedit_string`: display-only, never touches the buffer.
    /// `None` clears a pending preedit (the protocol's null preedit string).
    pub fn apply_ime_preedit(&mut self, preedit: Option<Preedit>) {
        self.preedit = preedit;
    }

    /// The `ImeEnable` + `ImeSync` pair a controller pushes on `FocusIn`:
    /// enable first (the relay treats enable as fresh state), then the
    /// buffer's current surrounding text, content type and cursor rectangle.
    pub fn ime_enable_cmds<Msg>(
        &self,
        hint: ContentHint,
        purpose: ContentPurpose,
        tree: &LayoutTree,
        node: &Node,
    ) -> [Cmd<Msg>; 2] {
        [
            Cmd::ImeEnable {
                hint: hint.wire(),
                purpose: purpose.wire(),
            },
            self.ime_sync_cmd(hint, purpose, tree, node),
        ]
    }

    /// The `ImeSync` a controller pushes whenever the buffer or the caret
    /// moves while focused.
    pub fn ime_sync_cmd<Msg>(
        &self,
        hint: ContentHint,
        purpose: ContentPurpose,
        tree: &LayoutTree,
        node: &Node,
    ) -> Cmd<Msg> {
        let caret = self.layout.caret_rect(self.cursor);
        Cmd::ImeSync(Snapshot::new(
            &self.buffer,
            self.cursor,
            self.anchor,
            hint,
            purpose,
            caret_surface_rect(tree, node, &caret, self.scroll_offset),
        ))
    }

    /// Push the `ImeSync` for a buffer or caret move while focused.
    pub fn ime_resync<Msg>(
        &self,
        cx: &mut EventCx<'_, Msg>,
        hint: ContentHint,
        purpose: ContentPurpose,
    ) {
        let tree = cx.tree;
        let node = cx.node;
        cx.cmds.push(self.ime_sync_cmd(hint, purpose, tree, node));
    }

    /// Handle `FocusIn`/`FocusOut` and the IME event family for the embedding
    /// controller. Returns `Some(messages)` when handled — pushing any
    /// `Cmd::Ime*` into `cx.cmds` — or `None` when `ev` is none of those and
    /// the controller should keep going (pointer, key, …).
    ///
    /// An IME session follows the focus: `FocusIn` enables (with the
    /// buffer's current state), `FocusOut` disables. Composition applies to
    /// the engine; only a real buffer change fires `Change` (and re-syncs
    /// the new surrounding text back to the IME, which is what lets the
    /// relay re-forward it).
    pub fn ime_event<Msg: Clone + 'static>(
        &mut self,
        ev: &Event,
        cx: &mut EventCx<'_, Msg>,
        hint: ContentHint,
        purpose: ContentPurpose,
    ) -> Option<Vec<Msg>> {
        match ev {
            Event::FocusIn { .. } => {
                let tree = cx.tree;
                let node = cx.node;
                cx.cmds
                    .extend(self.ime_enable_cmds(hint, purpose, tree, node));
                Some(Vec::new())
            }
            Event::FocusOut => {
                cx.cmds.push(Cmd::ImeDisable);
                Some(Vec::new())
            }
            Event::ImePreedit { text, cursor_begin, cursor_end } => {
                self.apply_ime_preedit(Some(Preedit {
                    text: text.clone(),
                    cursor_begin: *cursor_begin,
                    cursor_end: *cursor_end,
                }));
                Some(Vec::new())
            }
            Event::ImeCommit(text) => {
                let now = cx.clock.now();
                if self.apply_ime_commit(text, now) {
                    cx.handled = true;
                    self.ime_resync(cx, hint, purpose);
                    let buffer = self.buffer.clone();
                    Some(
                        cx.handlers
                            .fire_text(crate::view::EventKind::Change, &buffer)
                            .map_or_else(Vec::new, |m| vec![m]),
                    )
                } else {
                    Some(Vec::new())
                }
            }
            Event::ImeDelete { before, after } => {
                let now = cx.clock.now();
                if self.apply_ime_delete(*before, *after, now) {
                    cx.handled = true;
                    self.ime_resync(cx, hint, purpose);
                    let buffer = self.buffer.clone();
                    Some(
                        cx.handlers
                            .fire_text(crate::view::EventKind::Change, &buffer)
                            .map_or_else(Vec::new, |m| vec![m]),
                    )
                } else {
                    Some(Vec::new())
                }
            }
            _ => None,
        }
    }

    /// What the pending preedit paints as, or `None` when no composition is
    /// staged. Masked (one bullet per character) while `visibility` is off,
    /// so a password entry never paints raw composing text.
    #[must_use]
    pub fn preedit_display(&self) -> Option<String> {
        self.preedit.as_ref().map(|preedit| {
            if self.visibility {
                preedit.text.clone()
            } else {
                Self::INVISIBLE
                    .to_string()
                    .repeat(preedit.text.chars().count())
            }
        })
    }

    /// Paint the pending preedit at the caret, if one is staged: the display
    /// string over a throwaway single-line layout at the caret origin, then
    /// the caret itself after it. A no-op without a preedit.
    pub fn paint_preedit(
        &self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        content: &crate::layout::Rect,
        color: crate::css::value::Rgba,
        cx: &mut crate::paint::PaintCx<'_>,
    ) {
        let Some(text) = self.preedit_display() else {
            return;
        };
        let caret = self.layout.caret_rect(self.cursor);
        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        let preedit = TextLayout::build(
            &text,
            &style,
            cx.fonts,
            None,
            WrapMode::None,
            Ellipsize::None,
        );
        preedit.draw(
            canvas,
            (content.x - self.scroll_offset + caret.x, content.y + caret.y),
            color,
        );
    }

    /// Apply an IME `commit_string`: splice `text` over the selection, record
    /// one undo step, and consume any pending preedit. Returns whether the
    /// buffer changed (an empty commit over no selection is a no-op, so the
    /// controller fires `Change` — and re-syncs surrounding text — only on
    /// `true`).
    pub fn apply_ime_commit(&mut self, text: &str, now: Duration) -> bool {
        let before = self.buffer.clone();
        let (buffer, cursor) = apply_commit_string(&self.buffer, self.cursor, self.anchor, text);
        self.preedit = None;
        if buffer == before {
            return false;
        }
        self.buffer = buffer;
        self.cursor = cursor;
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        self.sync_selection_node();
        true
    }

    /// Apply an IME `delete_surrounding_text`: delete `before_len` bytes
    /// before the cursor and `after_len` bytes after it (UTF-8 bytes per the
    /// protocol, clamped to whole characters). Records one undo step.
    /// Returns whether the buffer changed.
    pub fn apply_ime_delete(&mut self, before_len: u32, after_len: u32, now: Duration) -> bool {
        let before = self.buffer.clone();
        let (buffer, cursor) =
            apply_delete_surrounding(&self.buffer, self.cursor, before_len, after_len);
        if buffer == before {
            return false;
        }
        self.buffer = buffer;
        self.cursor = cursor;
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        self.sync_selection_node();
        true
    }

    /// Apply one key from `GtkText`'s table.
    pub fn key<Msg: Clone + 'static>(
        &mut self,
        key: &KeyEvent,
        cx: &mut EventCx<'_, Msg>,
    ) -> EditOutcome {
        use xkbcommon::xkb::keysyms;
        if !key.pressed {
            return EditOutcome::Ignored;
        }
        let now = cx.clock.now();
        let mods = key.effective_mods();
        let ctrl = mods.contains(Mods::CTRL);
        let shift = mods.contains(Mods::SHIFT);
        let sym = u32::from(key.keysym);

        let move_to = |this: &mut Self, to: usize| {
            if shift {
                this.anchor.get_or_insert(this.cursor);
            } else {
                this.anchor = None;
            }
            this.cursor = to;
        };

        let outcome = match sym {
            keysyms::KEY_Left => {
                let to = if ctrl {
                    self.layout.prev_word(self.cursor)
                } else {
                    self.layout.prev_grapheme(self.cursor)
                };
                move_to(self, to);
                EditOutcome::Moved
            }
            keysyms::KEY_Right => {
                let to = if ctrl {
                    self.layout.next_word(self.cursor)
                } else {
                    self.layout.next_grapheme(self.cursor)
                };
                move_to(self, to);
                EditOutcome::Moved
            }
            keysyms::KEY_Home => {
                move_to(self, 0);
                EditOutcome::Moved
            }
            keysyms::KEY_End => {
                let end = self.buffer.len();
                move_to(self, end);
                EditOutcome::Moved
            }
            keysyms::KEY_a | keysyms::KEY_slash if ctrl && !shift => {
                self.anchor = Some(0);
                self.cursor = self.buffer.len();
                EditOutcome::Moved
            }
            keysyms::KEY_A | keysyms::KEY_backslash if ctrl => {
                self.anchor = None;
                EditOutcome::Moved
            }
            keysyms::KEY_z if ctrl && !shift => match self.undo.undo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = clamp_to_boundary(&self.buffer, cursor);
                    self.anchor = None;
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            keysyms::KEY_y | keysyms::KEY_Z if ctrl => match self.undo.redo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = clamp_to_boundary(&self.buffer, cursor);
                    self.anchor = None;
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            keysyms::KEY_c if ctrl => {
                let range = self.selection();
                if !range.is_empty() {
                    cx.cmds.push(Cmd::Copy(self.buffer[range].to_owned()));
                }
                EditOutcome::Ignored
            }
            keysyms::KEY_x if ctrl => {
                let range = self.selection();
                if range.is_empty() {
                    EditOutcome::Ignored
                } else {
                    cx.cmds.push(Cmd::Copy(self.buffer[range].to_owned()));
                    let before = self.buffer.clone();
                    let range = self.selection();
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                    self.anchor = None;
                    self.undo.record(&before, &self.buffer, self.cursor, now);
                    EditOutcome::Changed
                }
            }
            keysyms::KEY_Clear => {
                let before = self.buffer.clone();
                self.buffer.clear();
                self.cursor = 0;
                self.anchor = None;
                self.undo.record(&before, &self.buffer, 0, now);
                EditOutcome::Changed
            }
            keysyms::KEY_BackSpace => {
                let before = self.buffer.clone();
                let range = self.selection();
                if range.is_empty() {
                    let from = self.layout.prev_grapheme(self.cursor);
                    if from == self.cursor {
                        return EditOutcome::Ignored;
                    }
                    self.buffer.replace_range(from..self.cursor, "");
                    self.cursor = from;
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                EditOutcome::Changed
            }
            keysyms::KEY_Delete => {
                let before = self.buffer.clone();
                let range = self.selection();
                if range.is_empty() {
                    let to = self.layout.next_grapheme(self.cursor);
                    if to == self.cursor {
                        return EditOutcome::Ignored;
                    }
                    self.buffer.replace_range(self.cursor..to, "");
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                EditOutcome::Changed
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_ISO_Enter => {
                EditOutcome::Activated
            }
            keysyms::KEY_Insert => {
                self.overwrite = !self.overwrite;
                EditOutcome::Moved
            }
            _ => {
                let Some(text) = key.utf8.as_deref().filter(|_| !ctrl) else {
                    return EditOutcome::Ignored;
                };
                if text.chars().all(char::is_control) {
                    return EditOutcome::Ignored;
                }
                if self.insert(text, now) {
                    EditOutcome::Changed
                } else {
                    EditOutcome::Ignored
                }
            }
        };
        self.sync_selection_node();
        // `Cmd::Paste` needs a message to deliver the text, which only the
        // embedding controller can build, so Ctrl+V is handled there.
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::{TextEditState, UndoStack, clamp_to_boundary};
    use crate::css::node::Node;
    use crate::text_input::Preedit;
    use crate::widgets::Headless;
    use std::time::Duration;

    fn edit_with(text: &str) -> (Headless, TextEditState) {
        let mut hx = Headless::new();
        let parent = Node::new("entry");
        let state = TextEditState::build(&parent, text, &mut hx.cx());
        (hx, state)
    }

    #[test]
    fn a_caret_landing_inside_a_codepoint_walks_back_to_its_start() {
        // mutation: return `offset.min(s.len())` from clamp_to_boundary and
        // the three multi-byte cases below all report a mid-codepoint offset,
        // which is exactly the offset `String::replace_range` panics on.
        let s = "a\u{1F600}c"; // 'a', a four-byte emoji at 1..5, 'c'
        assert_eq!(clamp_to_boundary(s, 3), 1, "mid-emoji walks back to 1");
        assert_eq!(clamp_to_boundary(s, 4), 1);
        assert_eq!(clamp_to_boundary(s, 5), 5, "a real boundary stands");
        assert_eq!(clamp_to_boundary(s, 99), s.len(), "past the end clamps");
        assert_eq!(clamp_to_boundary("", 7), 0, "an empty buffer has only 0");
        for n in 0..=s.len() + 4 {
            let at = clamp_to_boundary(s, n);
            assert!(s.is_char_boundary(at), "{n} -> {at} must be splittable");
            let mut owned = s.to_owned();
            owned.replace_range(at..at, "z"); // must never panic
        }
    }

    #[test]
    fn consecutive_single_character_insertions_coalesce_into_one_undo_step() {
        // mutation: drop the COALESCE window check in `record` and undo walks
        // back one character at a time, so the first assertion sees "ab".
        let mut undo = UndoStack::new(true);
        undo.record("", "a", 1, Duration::from_millis(0));
        undo.record("a", "ab", 2, Duration::from_millis(100));
        undo.record("ab", "abc", 3, Duration::from_millis(200));
        assert_eq!(
            undo.undo(),
            Some((String::new(), 0)),
            "one step back to empty"
        );
        assert_eq!(undo.undo(), None, "and nothing below it");
        assert_eq!(undo.redo(), Some(("abc".to_owned(), 3)));
    }

    #[test]
    fn a_pause_breaks_the_coalescing_run() {
        // mutation: widen COALESCE to an hour and this collapses to one step.
        let mut undo = UndoStack::new(true);
        undo.record("", "a", 1, Duration::from_millis(0));
        undo.record("a", "ab", 2, Duration::from_millis(5_000));
        assert_eq!(
            undo.undo(),
            Some(("a".to_owned(), 1)),
            "the pause split the run"
        );
        assert_eq!(undo.undo(), Some((String::new(), 0)));
    }

    #[test]
    fn a_disabled_undo_stack_records_nothing_and_never_panics() {
        // mutation: ignore `enabled` in `record` and can_undo becomes true.
        let mut undo = UndoStack::new(false);
        undo.record("", "a", 1, Duration::ZERO);
        assert!(!undo.can_undo() && !undo.can_redo());
        assert_eq!(undo.undo(), None);
        assert_eq!(undo.redo(), None);
    }

    #[test]
    fn the_stack_is_bounded_so_a_held_key_cannot_grow_it_without_limit() {
        // mutation: remove the MAX_STEPS truncation and the depth grows to 5000.
        let mut undo = UndoStack::new(true);
        for step in 0..5_000u64 {
            let before = "x".repeat(step as usize);
            let after = "x".repeat(step as usize + 1);
            // Steps a full second apart never coalesce.
            undo.record(&before, &after, after.len(), Duration::from_secs(step));
        }
        let mut depth = 0usize;
        while undo.undo().is_some() {
            depth += 1;
            assert!(depth <= UndoStack::MAX_STEPS, "the stack must be bounded");
        }
        assert_eq!(depth, UndoStack::MAX_STEPS);
    }

    #[test]
    fn an_ime_commit_replaces_the_selection_and_records_one_undo_step() {
        // mutation: splice at the cursor instead of the selection and
        // " world" survives; mutation: skip `undo.record` and the undo below
        // returns `None`.
        let (_hx, mut edit) = edit_with("hello world");
        edit.cursor = 5;
        edit.anchor = Some(11);
        assert!(edit.apply_ime_commit("there", Duration::ZERO));
        assert_eq!(edit.buffer, "hellothere");
        assert_eq!(edit.cursor, 10);
        assert_eq!(edit.anchor, None);
        assert_eq!(
            edit.undo.undo(),
            Some(("hello world".to_owned(), 10)),
            "one step back to the pre-commit buffer"
        );
    }

    #[test]
    fn an_ime_commit_with_no_change_reports_unchanged() {
        // mutation: always return `true` and the controller fires a spurious
        // `Change` (and an `ime_sync`) for an empty commit.
        let (_hx, mut edit) = edit_with("hi");
        assert!(!edit.apply_ime_commit("", Duration::ZERO));
        assert_eq!(edit.buffer, "hi");
    }

    #[test]
    fn a_preedit_is_display_only_until_the_commit_lands() {
        // mutation: write the preedit text into `buffer` and the assertion
        // below sees it early; mutation: leave `preedit` set after the commit
        // and a stale composition keeps painting.
        let (_hx, mut edit) = edit_with("hi");
        edit.apply_ime_preedit(Some(Preedit {
            text: "に".to_owned(),
            cursor_begin: 3,
            cursor_end: 3,
        }));
        assert_eq!(edit.buffer, "hi", "preedit must not touch the buffer");
        assert!(edit.preedit.is_some());
        assert!(edit.apply_ime_commit("に", Duration::ZERO));
        assert_eq!(edit.buffer, "hiに");
        assert_eq!(edit.preedit, None, "the commit consumes the preedit");
    }

    #[test]
    fn an_ime_delete_clamps_to_char_boundaries() {
        // mutation: slice at the raw byte offsets and this panics inside the
        // emoji; mutation: ignore the undo record and Ctrl+Z cannot restore.
        let (_hx, mut edit) = edit_with("a\u{1F600}c");
        edit.cursor = 5;
        assert!(edit.apply_ime_delete(4, 0, Duration::ZERO));
        assert_eq!(edit.buffer, "ac");
        assert_eq!(edit.cursor, 1);
        assert_eq!(
            edit.undo.undo().map(|(buffer, _)| buffer),
            Some("a\u{1F600}c".to_owned())
        );
    }

    #[test]
    fn a_model_text_write_clears_a_pending_preedit() {
        // mutation: drop the clear in `set_text` and a model-driven buffer
        // swap leaves a stale composition painting over the new text.
        let (mut hx, mut edit) = edit_with("hi");
        edit.apply_ime_preedit(Some(Preedit {
            text: "に".to_owned(),
            cursor_begin: 3,
            cursor_end: 3,
        }));
        edit.set_text("bye", &mut hx.cx());
        assert_eq!(edit.preedit, None);
    }
}
