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
use crate::text_input::{ContentHint, ContentPurpose};
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
    // `GtkEditable::changed`, on every keystroke, is the inherent
    // `View::on_change`, which shadows any same-named trait method at every
    // call site anyway.
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
        // P3's focus ring only walks nodes carrying `FOCUSABLE_CLASS`, and
        // this controller predates the `Universal` helper that seeds it
        // from `Kind::is_focusable_by_default` (true for `SearchEntry`) —
        // so without this line no search entry is ever a Tab stop and the
        // B1 launcher's open-time focus landing (`App::route`'s
        // `KeyboardEnter`-when-empty arm) walks straight past the search
        // box to the first button. The sibling text-entry controllers
        // (`EntryC`, `PasswordEntryC`, `TextViewC`) have the same gap;
        // they are left untouched on purpose (out of scope — widening Tab
        // order toolkit-wide is a separate change with its own gates).
        node.add_class(crate::window::focus::FOCUSABLE_CLASS);
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
        // An IME session follows the focus; composition applies to the
        // shared engine. See `TextEditState::ime_event`.
        if let Some(msgs) = self
            .edit
            .ime_event(ev, cx, ContentHint::NONE, ContentPurpose::Normal)
        {
            return msgs;
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
                self.edit
                    .ime_resync(cx, ContentHint::NONE, ContentPurpose::Normal);
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
        cx: &mut crate::view::controller::PaintCx<'_>,
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
        // An in-flight composition paints at the caret, ahead of it.
        self.edit.paint_preedit(canvas, &content, style.color(), cx);
        let caret = self.edit.layout.caret_rect(self.edit.cursor);
        let placed = Rect::new(content.x + caret.x, content.y + caret.y, 1.0, caret.height);
        canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::view::cmd::Cmd;
    use crate::view::controller::Event;
    use crate::view::{EventKind, Handler, Handlers, Kind, Prop, PropName, Props};
    use crate::widgets::{Headless, build_widget};
    use crate::window::focus::FocusCause;

    fn search_hi() -> (Headless, crate::widgets::BuiltWidget<String>) {
        let mut props = Props::default();
        props.set(PropName::Text, Prop::Str("hi".into()));
        (
            Headless::new(),
            build_widget::<String>(Kind::SearchEntry, &props),
        )
    }

    #[test]
    fn all_entry_family_widgets_are_tab_stops() {
        // The focus ring only walks nodes carrying `FOCUSABLE_CLASS`: every
        // text-entry kind must seed it at build, or Tab (and the C4
        // open-time focus landing) walks straight past it.
        for kind in [
            Kind::Entry,
            Kind::SearchEntry,
            Kind::PasswordEntry,
            Kind::TextView,
        ] {
            let built = build_widget::<String>(kind, &Props::default());
            assert!(
                built
                    .node
                    .classes()
                    .iter()
                    .any(|c| c.as_str() == crate::window::focus::FOCUSABLE_CLASS),
                "{kind:?} must carry FOCUSABLE_CLASS to be a Tab stop"
            );
        }
    }

    #[test]
    fn focus_in_enables_ime_and_a_commit_fires_change() {
        // mutation: skip the `ime_event` call and FocusIn emits nothing
        // while the commit never reaches the buffer.
        let (mut hx, built) = search_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(
            &Event::FocusIn {
                cause: FocusCause::Pointer,
            },
            &mut cx,
        );
        let enables = cx
            .cmds
            .iter()
            .filter(|cmd| matches!(cmd, Cmd::ImeEnable { purpose: 0, .. }))
            .count();
        assert_eq!(
            enables, 1,
            "a search entry enables purpose normal, saw {:?}",
            cx.cmds
        );

        let mut cx = hx.event_cx_with_handlers(&built.node, |handlers: &mut Handlers<String>| {
            handlers.set(
                EventKind::Change,
                Handler::Text(Rc::new(|text: &str| text.to_owned())),
            );
        });
        let msgs = c.on_event(&Event::ImeCommit("に".to_owned()), &mut cx);
        assert_eq!(msgs, vec!["hiに".to_owned()]);
    }
}
