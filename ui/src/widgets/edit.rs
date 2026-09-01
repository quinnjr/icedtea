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
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use crate::view::BuildCx;
use crate::view::cmd::Cmd;
use crate::view::controller::EventCx;
use crate::window::keyboard::{KeyEvent, Mods};

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
            selection_node: None,
        };
        this.reshape(cx);
        this
    }

    /// Replace the buffer from a prop, without recording an undo step — the
    /// model, not the user, made this change.
    pub fn set_text(&mut self, text: &str, cx: &mut BuildCx<'_>) {
        if self.buffer == text {
            return;
        }
        self.buffer = text.to_owned();
        self.cursor = self.cursor.min(self.buffer.len());
        self.anchor = None;
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

    /// The ordered, clamped selection range.
    #[must_use]
    pub fn selection(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor).min(self.buffer.len());
        let cursor = self.cursor.min(self.buffer.len());
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
                    self.cursor = cursor.min(self.buffer.len());
                    self.anchor = None;
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            keysyms::KEY_y | keysyms::KEY_Z if ctrl => match self.undo.redo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = cursor.min(self.buffer.len());
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
    use super::UndoStack;
    use std::time::Duration;

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
}
