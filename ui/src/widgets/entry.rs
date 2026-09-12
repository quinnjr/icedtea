//! `GtkEntry` — `Kind::Entry`, CSS node `entry`.
//!
//! ```text
//! entry[.flat][.warning][.error]
//! ├── text[.read-only]
//! ├── image.left
//! ├── image.right
//! ╰── [progress[.pulse]]
//! ```
//!
//! The `text` node's own subtree comes from [`TextEditState`], which every
//! entry-family widget shares; GTK's own block defers to `GtkText` for it.

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::layout::Rect;
use crate::text_input::{ContentHint, ContentPurpose};
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState, UndoStack};
use crate::widgets::{PointerState, content_rect_local, local_rect, shift_event};
use crate::window::focus::FocusCause;
use crate::window::keyboard::Mods;
use crate::window::popup::PopupKey;

/// Plan reconciliation: `TextLayout::size` alone measures an empty buffer to
/// zero width, and a zero-area node is one `window/pointer.rs::descend`
/// skips outright — an empty `Entry` could never be clicked into focus. GTK
/// reserves the caret's own width even with nothing typed yet, matching
/// `TextView`'s own `CARET_WIDTH_PX` floor (the plan's `measure` omitted
/// this).
const CARET_WIDTH_PX: f32 = 8.0;

/// A `GtkEntry` holding `text`.
#[must_use]
pub fn entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::Entry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkEntry`'s own setters and signals.
pub trait EntryExt<Msg>: Sized {
    /// `GtkEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkEditable:editable`.
    fn editable(self, on: bool) -> Self;
    /// `GtkEntry:max-length`, in characters.
    fn max_length(self, n: i32) -> Self;
    /// `GtkEntry:visibility`.
    fn visibility(self, on: bool) -> Self;
    /// `GtkEntry:primary-icon-name`, as an `IconRef`.
    fn icon_left(self, icon: IconRef) -> Self;
    /// `GtkEntry:secondary-icon-name`, as an `IconRef`.
    fn icon_right(self, icon: IconRef) -> Self;
    /// `GtkEntry:progress-fraction`.
    fn progress_fraction(self, v: f64) -> Self;
    /// `GtkEntry:progress-pulse-step`.
    fn progress_pulse_step(self, v: f64) -> Self;
    /// `GtkEntry:activates-default`.
    fn activates_default(self, on: bool) -> Self;
    /// `GtkEditable:enable-undo`.
    fn enable_undo(self, on: bool) -> Self;
    /// `GtkEditable:xalign`.
    fn xalign(self, a: f32) -> Self;
    /// `GtkEditable:width-chars`.
    fn width_chars(self, n: i32) -> Self;
    // `GtkEditable::changed` is the inherent `View::on_change`, which shadows
    // any same-named trait method at every call site anyway.
    /// `GtkEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
    /// `GtkEntry::icon-press`, carrying 0 for the left icon and 1 for the right.
    fn on_icon_press(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> EntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn editable(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn max_length(self, n: i32) -> Self {
        self.prop(PropName::MaxLength, Prop::Int(i64::from(n)))
    }
    fn visibility(self, on: bool) -> Self {
        self.prop(PropName::Visibility, Prop::Bool(on))
    }
    fn icon_left(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn icon_right(self, icon: IconRef) -> Self {
        self.prop(PropName::Paintable, Prop::Icon(icon))
    }
    fn progress_fraction(self, v: f64) -> Self {
        self.prop(PropName::Fraction, Prop::Float(v))
    }
    fn progress_pulse_step(self, v: f64) -> Self {
        self.prop(PropName::Pulse, Prop::Float(v))
    }
    fn activates_default(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn enable_undo(self, on: bool) -> Self {
        self.prop(PropName::EnableUndo, Prop::Bool(on))
    }
    fn xalign(self, a: f32) -> Self {
        self.prop(PropName::Xalign, Prop::Float(f64::from(a)))
    }
    fn width_chars(self, n: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(n)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
    fn on_icon_press(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::Entry`'s controller.
pub struct EntryC {
    /// The shared editing state.
    pub edit: TextEditState,
    /// `[left, right]` `image` subnodes.
    pub icons: [Option<Node>; 2],
    /// The `progress` subnode, when a fraction is set.
    pub progress: Option<Node>,
    /// The context menu's popup, while open.
    pub menu: Option<PopupKey>,
    editable: bool,
    pointer: PointerState,
}

impl EntryC {
    fn apply(&self, _node: &Node) {
        if self.editable {
            self.edit.text_node.remove_class("read-only");
        } else {
            self.edit.text_node.add_class("read-only");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for EntryC {
    fn kind(&self) -> Kind {
        Kind::Entry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let mut edit = TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx);
        edit.visibility = props.bool(PropName::Visibility, true);
        edit.max_length = match props.int(PropName::MaxLength, 0) {
            0 => None,
            n => usize::try_from(n).ok(),
        };
        edit.undo = UndoStack::new(props.bool(PropName::EnableUndo, true));
        // Plan reconciliation: the plan created `image.left`/`image.right`
        // only when an icon prop was set. GTK's own entry always creates
        // both icon-area widgets — empty ones just paint nothing — and the
        // vendored fixture (Step 1) lists them unconditionally, unlike the
        // bracketed, genuinely-optional `progress` node below. Gating their
        // creation on the icon props made a bare `Entry` fail its own node-
        // tree fixture.
        let left = {
            let image = Node::with_classes("image", &["left"]);
            node.append_child(&image);
            Some(image)
        };
        let right = {
            let image = Node::with_classes("image", &["right"]);
            node.append_child(&image);
            Some(image)
        };
        let progress = props.get(PropName::Fraction).is_some().then(|| {
            let progress = Node::new("progress");
            node.append_child(&progress);
            progress
        });
        let this = EntryC {
            edit,
            icons: [left, right],
            progress,
            menu: None,
            editable: props.bool(PropName::Editable, true),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => self.edit.set_text(text, cx),
            (PropName::Editable, Prop::Bool(on)) => self.editable = *on,
            (PropName::Visibility, Prop::Bool(on)) => {
                self.edit.visibility = *on;
                self.edit.reshape(cx);
            }
            (PropName::MaxLength, Prop::Int(n)) => {
                self.edit.max_length = if *n <= 0 {
                    None
                } else {
                    usize::try_from(*n).ok()
                };
            }
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // An icon press reports its index and consumes the event.
        for (index, icon) in self.icons.iter().enumerate() {
            let Some(icon) = icon.as_ref() else {
                continue;
            };
            let Some(rect) = local_rect(cx.tree, cx.node, icon) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let mut probe = PointerState::default();
            if probe.observe(
                icon,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) {
                cx.handled = true;
                return cx
                    .handlers
                    .fire_index(EventKind::Selected, index)
                    .map_or_else(Vec::new, |m| vec![m]);
            }
        }
        if matches!(ev, Event::PointerDown { .. }) {
            // Plan reconciliation: the plan's `on_event` never granted focus
            // on click, matching the same gap `TextView` had (see that
            // controller's own note) — without this an `Entry` could never
            // receive keyboard focus by any means.
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        // The caret follows the click.
        //
        // This hit-tested `local_rect(cx.tree, cx.node, &self.edit.text_node)`,
        // which is `None` for `text` in every frame: `TextEditState::build`
        // appends that subnode itself rather than returning it as a `View`
        // child, so `reconcile`'s trim step detaches it (this controller
        // reserves nothing) and `tree.allocation` never answers for it. The
        // whole block was therefore dead and an `Entry`'s caret could not be
        // placed by clicking at all -- it stayed where `TextEditState::build`
        // put it (`cursor: text.len()`), so a click anywhere in the entry,
        // followed by typing, appended at the end of the buffer.
        // `SearchEntryC` and `PasswordEntryC` both already carry the fix and
        // say so in their own notes; `content_rect_local` derives the same
        // region from this controller's own allocation, which
        // `window/pointer.rs::descend` does stop at.
        //
        // `buffer_offset_at`, not `layout.byte_at`, for the same reason
        // `PasswordEntryC` gives: with `GtkEntry:visibility` off the layout is
        // over the invisible character, whose byte offsets are not the
        // buffer's -- a bare `byte_at` can land the cursor past the end of a
        // shorter buffer, or inside one of its codepoints, which
        // `TextEditState::insert`'s own `String::replace_range` then panics
        // on.
        if let Some(rect) = content_rect_local(cx.tree, cx.node) {
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.edit.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
            if let Event::PointerDown { local, .. } = shifted {
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
        // Ctrl+V needs a message to carry the pasted text back, which only this
        // controller can build, so it is handled here rather than in the shared
        // engine.
        if key.pressed
            && key.effective_mods().contains(Mods::CTRL)
            && u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_v
        {
            cx.handled = true;
            return Vec::new();
        }
        if !self.editable
            && !matches!(
                u32::from(key.keysym),
                xkbcommon::xkb::keysyms::KEY_Left | xkbcommon::xkb::keysyms::KEY_Right
            )
        {
            return Vec::new();
        }
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
                let text = self.edit.buffer.clone();
                cx.handlers
                    .fire_text(EventKind::Change, &text)
                    .map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                cx.handlers
                    .fire_unit(EventKind::Activate)
                    .map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

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
        // The caret.
        let caret = self.edit.layout.caret_rect(self.edit.cursor);
        let placed = Rect::new(content.x + caret.x, content.y + caret.y, 1.0, caret.height);
        canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        let _ = &self.menu;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::view::cmd::Cmd;
    use crate::view::controller::Event;
    use crate::view::{EventKind, Handler, Handlers, Kind, Prop, PropName, Props};
    use crate::widgets::{BuiltWidget, Headless, build_widget};
    use crate::window::focus::FocusCause;

    fn entry_hi() -> (Headless, BuiltWidget<String>) {
        let mut props = Props::default();
        props.set(PropName::Text, Prop::Str("hi".into()));
        (Headless::new(), build_widget::<String>(Kind::Entry, &props))
    }

    fn ime_cmds(cmds: &[Cmd<String>]) -> (bool, bool, bool) {
        let mut enable = false;
        let mut sync = false;
        let mut disable = false;
        for cmd in cmds {
            match cmd {
                Cmd::ImeEnable { .. } => enable = true,
                Cmd::ImeSync(sync_snapshot) if sync_snapshot.surrounding == "hi" => sync = true,
                Cmd::ImeSync(_) => {}
                Cmd::ImeDisable => disable = true,
                _ => {}
            }
        }
        (enable, sync, disable)
    }

    #[test]
    fn focus_in_enables_ime_and_syncs_the_buffer() {
        // mutation: push only `ImeEnable` and the IME activates with no
        // surrounding text; mutation: push neither and this sees no cmds.
        let (mut hx, built) = entry_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(
            &Event::FocusIn {
                cause: FocusCause::Pointer,
            },
            &mut cx,
        );
        let (enable, sync, _) = ime_cmds(cx.cmds);
        assert!(enable, "FocusIn must enable IME, saw {:?}", cx.cmds);
        assert!(sync, "FocusIn must sync surrounding, saw {:?}", cx.cmds);
    }

    #[test]
    fn focus_out_disables_ime() {
        // mutation: drop the `FocusOut` arm and the IME session leaks across
        // focus moves.
        let (mut hx, built) = entry_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(&Event::FocusOut, &mut cx);
        let (_, _, disable) = ime_cmds(cx.cmds);
        assert!(disable, "FocusOut must disable IME, saw {:?}", cx.cmds);
    }

    #[test]
    fn an_ime_commit_applies_and_fires_change() {
        // mutation: apply the commit without firing `Change` and the model
        // never learns the composed text.
        let (mut hx, built) = entry_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx_with_handlers(&built.node, |handlers: &mut Handlers<String>| {
            handlers.set(
                EventKind::Change,
                Handler::Text(Rc::new(|text: &str| text.to_owned())),
            );
        });
        let msgs = c.on_event(&Event::ImeCommit("に".to_owned()), &mut cx);
        assert_eq!(msgs, vec!["hiに".to_owned()]);
    }

    #[test]
    fn an_ime_preedit_is_display_only() {
        // mutation: fire `Change` for a preedit and the model learns text
        // the user has not committed.
        let (mut hx, built) = entry_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx_with_handlers(&built.node, |handlers: &mut Handlers<String>| {
            handlers.set(
                EventKind::Change,
                Handler::Text(Rc::new(|text: &str| text.to_owned())),
            );
        });
        let msgs = c.on_event(
            &Event::ImePreedit {
                text: "に".to_owned(),
                cursor_begin: 3,
                cursor_end: 3,
            },
            &mut cx,
        );
        assert!(
            msgs.is_empty(),
            "a preedit must not fire Change, saw {msgs:?}"
        );
        assert!(
            cx.cmds.is_empty(),
            "a preedit sends nothing, saw {:?}",
            cx.cmds
        );
    }

    #[test]
    fn a_key_edit_resyncs_the_ime() {
        // mutation: skip the sync on `Changed` and the IME keeps composing
        // against stale surrounding text.
        let (mut hx, built) = entry_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        let mut key = Headless::key("a");
        key.utf8 = Some("!".to_owned());
        c.on_event(&Event::Key(key), &mut cx);
        let syncs: Vec<_> = cx
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                Cmd::ImeSync(snapshot) => Some(snapshot.surrounding.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(syncs, vec!["hi!".to_owned()], "saw {:?}", cx.cmds);
    }
}
