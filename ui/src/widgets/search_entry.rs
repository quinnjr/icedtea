//! `GtkSearchEntry` — `Kind::SearchEntry`, CSS node `entry`, class `.search`.
//!
//! ```text
//! entry.search
//! ╰── text
//! ```
//!
//! `search-changed` is debounced: `tick` fires `EventKind::Search` once
//! `search_delay` has elapsed with no further edit, which is what
//! `GtkSearchEntry:search-delay` means.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{PointerState, content_rect_local, shift_event};
use crate::window::focus::FocusCause;

/// GTK's own default `search-delay`.
const DEFAULT_DELAY_MS: u32 = 150;

/// Plan reconciliation: see [`crate::widgets::entry`]'s own `CARET_WIDTH_PX`
/// — an empty buffer measures to zero width, and a zero-area node is one
/// `window/pointer.rs::descend` skips outright, so an empty `SearchEntry`
/// could never be clicked into focus without this floor.
const CARET_WIDTH_PX: f32 = 8.0;

/// A `GtkSearchEntry` holding `text`.
#[must_use]
pub fn search_entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::SearchEntry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkSearchEntry`'s own setters and signals.
pub trait SearchEntryExt<Msg>: Sized {
    /// `GtkSearchEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkSearchEntry:search-delay`, in milliseconds.
    fn search_delay(self, ms: u32) -> Self;
    /// `GtkSearchEntry::search-changed`, debounced.
    fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkEditable::changed`, on every keystroke.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkSearchEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> SearchEntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn search_delay(self, ms: u32) -> Self {
        self.prop(PropName::TransitionDuration, Prop::Int(i64::from(ms)))
    }
    fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Search, Handler::Text(Rc::new(f)))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// `Kind::SearchEntry`'s controller.
pub struct SearchEntryC {
    /// The shared editing state.
    pub edit: TextEditState,
    /// The debounce window.
    pub delay_ms: u32,
    /// When the pending search became due, if one is pending.
    pub pending_since: Option<Duration>,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for SearchEntryC {
    fn kind(&self) -> Kind {
        Kind::SearchEntry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("search");
        SearchEntryC {
            edit: TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx),
            delay_ms: u32::try_from(
                props.int(PropName::TransitionDuration, i64::from(DEFAULT_DELAY_MS)),
            )
            .unwrap_or(DEFAULT_DELAY_MS),
            pending_since: None,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => self.edit.set_text(text, cx),
            (PropName::TransitionDuration, Prop::Int(ms)) => {
                self.delay_ms = u32::try_from(*ms).unwrap_or(DEFAULT_DELAY_MS);
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if matches!(ev, Event::PointerDown { .. }) {
            // Plan reconciliation: the plan's `on_event` never granted focus
            // on click, the same gap `Entry` and `TextView` had (see those
            // controllers' own notes) — without this a `SearchEntry` could
            // never receive keyboard focus by any means.
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        // Plan reconciliation: the plan hit-tested the text with
        // `local_rect(cx.tree, cx.node, &self.edit.text_node)`, as `Entry`
        // does — but `text` is a subnode `TextEditState::build` appends
        // itself, never a `View` child, so it has no taffy node and
        // `tree.allocation` (hence `local_rect`) is `None` for it, always,
        // leaving the whole block dead. `content_rect_local` derives the same
        // region from this controller's own allocation instead, which
        // `window/pointer.rs::descend` does stop at.
        if let Some(rect) = content_rect_local(cx.tree, cx.node) {
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.edit.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
            if let Event::PointerDown { local, .. } = shifted {
                // Plan reconciliation: the plan's `on_event` does not place
                // the caret on click either; `Entry`'s own controller does,
                // and both siblings built here now do the same, through the
                // shared `buffer_offset_at` (identity here — a search entry is
                // never masked — and mask-aware in `PasswordEntry`).
                self.edit.cursor = self.edit.buffer_offset_at(local);
                self.edit.anchor = None;
                cx.handled = true;
            }
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        match self.edit.key(key, cx) {
            EditOutcome::Ignored => Vec::new(),
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Changed => {
                cx.handled = true;
                // Every edit restarts the debounce window.
                self.pending_since =
                    Some(cx.clock.now() + Duration::from_millis(u64::from(self.delay_ms)));
                let text = self.edit.buffer.clone();
                cx.handlers
                    .fire_text(EventKind::Change, &text)
                    .map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                self.pending_since = None;
                cx.handlers
                    .fire_unit(EventKind::Activate)
                    .map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.pending_since.is_some_and(|due| now >= due) {
            self.pending_since = None;
            let text = self.edit.buffer.clone();
            return cx
                .handlers
                .fire_text(EventKind::Search, &text)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.pending_since
    }

    // Plan reconciliation: the plan's `SearchEntryC` left `measure`/`paint`
    // at the `Controller` defaults (no intrinsic size, nothing drawn), which
    // — as `Entry`'s own `CARET_WIDTH_PX` note explains — would leave a
    // `SearchEntry` zero-sized, unclickable, and blank. Mirrors `EntryC`'s
    // pair, minus the icon nodes it has none of.
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.edit.reshape(cx);
        let (w, h) = self.edit.layout.size();
        Some((w.max(CARET_WIDTH_PX), h))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        for rect in self.edit.layout.selection_rects(self.edit.selection()) {
            let placed = Rect::new(
                content.x + rect.x,
                content.y + rect.y,
                rect.width,
                rect.height,
            );
            canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        }
        self.edit.layout.draw(
            canvas,
            (content.x - self.edit.scroll_offset, content.y),
            style.color(),
        );
        let caret = self.edit.layout.caret_rect(self.edit.cursor);
        let placed = Rect::new(content.x + caret.x, content.y + caret.y, 1.0, caret.height);
        canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
