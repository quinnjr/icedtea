//! `GtkSearchBar` -- `Kind::SearchBar`, CSS node `searchbar`.
//!
//! ```text
//! searchbar
//! ╰── revealer
//!     ╰── box
//!          ├── <child>
//!          ╰── [button.close]
//! ```
//!
//! The single application child (an entry, typically) lives inside the inner
//! `box`, alongside an optional `button.close` this controller builds itself
//! ([`crate::widgets::child_slot`] routes it there, the same way `Expander`'s
//! `content` node works). `key_capture` is GTK's "type to search": a
//! printable key reaching this bar while it is closed opens it and is *also*
//! forwarded to the child entry, unlike Escape, which this controller
//! consumes outright.

use std::time::Duration;

use crate::css::node::Node;
use crate::view::{
    BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props, View,
};
use crate::widgets::{Revealer, Universal, prop_bool};

/// `GtkSearchBar`'s own setters, chained after
/// [`crate::view::builders::search_bar`].
///
/// `.show_close_button`/`.revealed` are scoped to this trait rather than
/// plain inherent `View<Msg>` methods: `GtkInfoBar` (P5, `InfoBarExt`)
/// already has same-named methods over *different* props (`Buttons`,
/// `Reveal`), and an inherent method always wins over a trait one -- adding
/// these as inherent methods would silently repoint every existing
/// `InfoBar` call site at the wrong prop.
pub trait SearchBarExt<Msg>: Sized {
    /// `GtkSearchBar:search-mode-enabled`.
    fn search_mode(self, v: impl Into<Prop>) -> Self;
    /// `GtkSearchBar:show-close-button`.
    fn show_close_button(self, v: impl Into<Prop>) -> Self;
    /// `GtkSearchBar:key-capture-widget` (as a bool: whether this bar
    /// captures key events at all, rather than the widget it captures from).
    fn key_capture(self, v: impl Into<Prop>) -> Self;
}

impl<Msg: Clone + 'static> SearchBarExt<Msg> for View<Msg> {
    fn search_mode(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::SearchMode, v)
    }
    fn show_close_button(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShowCloseButton, v)
    }
    fn key_capture(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::KeyCapture, v)
    }
}

/// Whether `key` carries `Escape`.
fn is_escape(key: &crate::window::keyboard::KeyEvent) -> bool {
    u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_Escape
}

/// Whether `key` would insert text -- xkbcommon's own notion of "printable",
/// not this crate's `KeyEvent::utf8` (`None` for a synthesised test event
/// that never went through a real compose sequence, `Some` only for a key
/// that already produced text elsewhere).
fn is_printable(key: &crate::window::keyboard::KeyEvent) -> bool {
    xkbcommon::xkb::keysym_to_utf32(key.keysym) != 0
}

/// `GtkSearchBar`.
pub struct SearchBarC {
    revealer: Revealer,
    /// The inner `box` node; [`crate::widgets::child_slot`] routes the
    /// view's own child here, ahead of [`SearchBarC::close`].
    pub(crate) contents: Node,
    /// The `button.close` subnode, when `show-close-button`.
    close: Option<Node>,
    key_capture: bool,
    universal: Universal,
}

impl SearchBarC {
    /// Test hook: whether the bar is currently revealed.
    #[must_use]
    pub fn revealed_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> bool {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .is_some_and(|c| c.revealer.revealed)
    }

    /// Test hook: the `button.close` node, when built.
    #[must_use]
    pub fn close_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> Option<Node> {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>().and_then(|c| c.close.clone())
    }

    fn ensure_close(&mut self, want: bool) {
        match (want, self.close.take()) {
            (true, Some(existing)) => self.close = Some(existing),
            (true, None) => {
                let button = Node::with_classes("button", &["close"]);
                self.contents.append_child(&button);
                self.close = Some(button);
            }
            (false, Some(existing)) => existing.detach(),
            (false, None) => {}
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SearchBarC {
    fn kind(&self) -> Kind {
        Kind::SearchBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let revealer = Revealer::build(node, props.bool(PropName::SearchMode, false));
        let contents = Node::new("box");
        revealer.node.append_child(&contents);
        let mut me = Self {
            revealer,
            contents,
            close: None,
            key_capture: props.bool(PropName::KeyCapture, false),
            universal: Universal::new(node, Kind::SearchBar),
        };
        me.ensure_close(props.bool(PropName::ShowCloseButton, false));
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::SearchMode => {
                self.revealer
                    .set_revealed(prop_bool(value, false), cx.clock.now());
            }
            PropName::ShowCloseButton => self.ensure_close(prop_bool(value, false)),
            PropName::KeyCapture => self.key_capture = prop_bool(value, false),
            other => {
                self.universal.apply(node, Kind::SearchBar, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        match ev {
            Event::Key(key) if key.pressed && is_escape(key) && self.revealer.revealed => {
                self.revealer.set_revealed(false, cx.clock.now());
                cx.handled = true;
                cx.handlers
                    .fire_bool(EventKind::Toggle, false)
                    .into_iter()
                    .collect()
            }
            Event::Key(key)
                if key.pressed
                    && self.key_capture
                    && !self.revealer.revealed
                    && is_printable(key) =>
            {
                self.revealer.set_revealed(true, cx.clock.now());
                // The key that opened the bar is *also* delivered to the
                // child entry -- GTK's capture forwards it -- so this does
                // not set `cx.handled`.
                cx.handlers
                    .fire_bool(EventKind::Toggle, true)
                    .into_iter()
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        self.revealer.tick(now);
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.revealer.next_deadline(now)
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + usize::from(self.close.is_some())
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::search_bar::SearchBarC;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    const FIXTURE: &str =
        "searchbar\n╰── revealer\n    ╰── box\n        ├── <child>\n        ╰── [button.close]\n";

    fn props(close: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::ShowCloseButton, Prop::Bool(close));
        p.set(PropName::KeyCapture, Prop::Bool(true));
        p
    }

    #[test]
    fn the_bar_wraps_its_child_in_a_revealer_and_a_box() {
        // Mutation check: hanging the child straight off `searchbar` skips
        // the revealer, so the reveal animation has nothing to animate and
        // Adwaita's `searchbar > revealer > box` padding never applies.
        let built = build_widget::<()>(Kind::SearchBar, &props(true));
        matches_fixture(&built.node, FIXTURE).expect("search_bar fixture");
    }

    #[test]
    fn escape_closes_and_a_printable_key_opens_when_key_capture_is_on() {
        // Interaction test. Mutation check: forwarding Escape to the child
        // as well as closing makes an entry clear its text *and* the bar
        // close, which is two actions for one key.
        let built = build_widget::<bool>(Kind::SearchBar, &props(true));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Toggle, Handler::Bool(std::rc::Rc::new(|on| on)));
        });
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("a")), &mut cx),
            vec![true]
        );
        assert!(SearchBarC::revealed_of(&*c));
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("Escape")), &mut cx),
            vec![false]
        );
        assert!(!SearchBarC::revealed_of(&*c));
        assert!(cx.handled, "Escape is consumed here, not forwarded");
    }

    #[test]
    fn the_close_button_exists_only_while_show_close_button_is_set() {
        // Rest-state test. Mutation check: always building the button adds
        // Adwaita's 34px control to a bar that asked for none.
        let with = build_widget::<()>(Kind::SearchBar, &props(true));
        let without = build_widget::<()>(Kind::SearchBar, &props(false));
        assert!(SearchBarC::close_of(&*with.controller).is_some());
        assert!(SearchBarC::close_of(&*without.controller).is_none());
    }
}
