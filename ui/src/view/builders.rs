//! Free-function widget builders and the event setters every one of them
//! chains.
//!
//! # The builder naming rule (normative — contract §4.4)
//!
//! P4 ships this frame; P5 and P6 fill it, and must follow the rule exactly.
//!
//! * A widget `Foo` gets **one** free constructor `fn foo(..) -> View<Msg>`
//!   in this module, named in `snake_case` from the GTK class name with the
//!   `Gtk` prefix dropped: `GtkCheckButton` → `check_button`,
//!   `GtkColorDialogButton` → `color_dialog_button`.
//! * Its required content is a **positional** argument: `button("Ok")`,
//!   `label("Hi")`, `scale(0.0, 100.0)`.
//! * Everything else is a chained `self`-consuming setter named after the
//!   **GTK property**, in `snake_case`, with no `set_` prefix:
//!   `.wrap(true)`, `.show_text(true)`, `.max_length(32)`.
//! * Handlers are `on_<eventkind snake_case>`, taking a `Msg` for
//!   [`Handler::Unit`] and a closure otherwise. All eighteen live in this
//!   file, on `View<Msg>` itself, so every builder inherits them.
//! * Every builder returns [`View<Msg>`], so §4.1's `.class()`, `.margin()`
//!   and `.key()` chain after it.

use crate::view::{EventKind, Handler, Kind, View};
use crate::window::keyboard::KeyEvent;
use std::rc::Rc;

/// A bare [`View`] of `kind`, with no props and no handlers.
///
/// The generic constructor every named builder in P5/P6 starts from, and the
/// only one P4 itself needs.
#[must_use]
pub fn widget<Msg: Clone + 'static>(kind: Kind) -> View<Msg> {
    View::new(kind)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// The widget was clicked.
    #[must_use]
    pub fn on_click(self, msg: Msg) -> Self {
        self.on(EventKind::Click, Handler::Unit(msg))
    }

    /// The widget was activated — Space/Enter, or a click that completed
    /// inside it.
    #[must_use]
    pub fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }

    /// A popover, dialog or window was closed.
    #[must_use]
    pub fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }

    /// The widget took the focus.
    #[must_use]
    pub fn on_focus_in(self, msg: Msg) -> Self {
        self.on(EventKind::FocusIn, Handler::Unit(msg))
    }

    /// The widget lost the focus.
    #[must_use]
    pub fn on_focus_out(self, msg: Msg) -> Self {
        self.on(EventKind::FocusOut, Handler::Unit(msg))
    }

    /// A toggle/check/switch changed state.
    #[must_use]
    pub fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }

    /// An expander opened or closed.
    #[must_use]
    pub fn on_expanded(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Expanded, Handler::Bool(Rc::new(f)))
    }

    /// Editable text changed.
    #[must_use]
    pub fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }

    /// A search entry's debounced text changed.
    #[must_use]
    pub fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Search, Handler::Text(Rc::new(f)))
    }

    /// A link button's URI was activated.
    #[must_use]
    pub fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }

    /// A calendar date was picked.
    ///
    /// Contract deviation D13: the payload is an ISO-8601 `YYYY-MM-DD`
    /// string, because §4.4's `Handler` has no `(i32, u32, u32)` variant.
    #[must_use]
    pub fn on_date_selected(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::DateSelected, Handler::Text(Rc::new(f)))
    }

    /// A row, item or dropdown entry was selected.
    #[must_use]
    pub fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }

    /// A notebook or stack switched page.
    #[must_use]
    pub fn on_page_changed(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::PageChanged, Handler::Index(Rc::new(f)))
    }

    /// A dialog button was chosen; the index is into the dialog's button list.
    #[must_use]
    pub fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }

    /// A reorderable child was dropped.
    ///
    /// Contract deviation D13: the payload is the **destination** index,
    /// because §4.4's `Handler` has no `(usize, usize)` variant.
    #[must_use]
    pub fn on_reordered(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Reordered, Handler::Index(Rc::new(f)))
    }

    /// A scale, scrollbar or spin button's value changed.
    #[must_use]
    pub fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }

    /// A scrolled window's offset changed; the payload is the vertical
    /// offset in px.
    #[must_use]
    pub fn on_scrolled(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::Scrolled, Handler::Float(Rc::new(f)))
    }

    /// A key reached this widget. Return `None` to let it keep bubbling to
    /// the window's focus navigation (contract §3.5).
    #[must_use]
    pub fn on_key(self, f: impl Fn(&KeyEvent) -> Option<Msg> + 'static) -> Self {
        self.on(EventKind::KeyPressed, Handler::Key(Rc::new(f)))
    }
}

// The per-widget builders themselves live beside their controllers under
// `crate::widgets` (contract §11 E5); this module re-exports them so every
// builder is reachable as `view::builders::<name>`.
pub use crate::widgets::button::{ButtonExt, button, button_from};
pub use crate::widgets::calendar::{CalendarExt, calendar};
pub use crate::widgets::check_button::{CheckButtonExt, check_button};
pub use crate::widgets::color_dialog::{
    ColorDialogButtonExt, ColorDialogExt, color_dialog, color_dialog_button,
};
pub use crate::widgets::drawing_area::{DrawingAreaExt, drawing_area};
pub use crate::widgets::drop_down::{DropDownExt, drop_down, drop_down_from};
pub use crate::widgets::entry::{EntryExt, entry};
pub use crate::widgets::font_dialog::{
    FontDialogButtonExt, FontDialogExt, font_dialog, font_dialog_button,
};
pub use crate::widgets::image::{ImageExt, image, image_named};
pub use crate::widgets::info_bar::{InfoBarExt, info_bar};
pub use crate::widgets::label::{LabelExt, label};
pub use crate::widgets::level_bar::{LevelBarExt, level_bar};
pub use crate::widgets::link_button::{LinkButtonExt, link_button};
pub use crate::widgets::menu_button::{MenuButtonExt, menu_button};
pub use crate::widgets::picture::{PictureExt, picture, picture_from_bytes};
pub use crate::widgets::popover::{PopoverExt, popover};
pub use crate::widgets::progress_bar::{ProgressBarExt, progress_bar};
pub use crate::widgets::scale::{ScaleExt, scale};
pub use crate::widgets::scrollbar::{ScrollbarExt, scrollbar};
pub use crate::widgets::separator::separator;
pub use crate::widgets::spinner::{SpinnerExt, spinner};
pub use crate::widgets::statusbar::{StatusbarExt, statusbar};
pub use crate::widgets::switch::{SwitchExt, switch};
pub use crate::widgets::text_view::{TextViewExt, text_view};
pub use crate::widgets::toggle_button::{ToggleButtonExt, toggle_button};
pub use crate::widgets::window_controls::{WindowControlsExt, window_controls};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{EventKind, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Clicked,
        Text(String),
        Index(usize),
        Value(u64),
        Flag(bool),
    }

    #[test]
    fn widget_builds_a_bare_view_of_the_kind() {
        let v: View<Msg> = widget(Kind::Spinner);
        assert_eq!(v.kind, Kind::Spinner);
        assert!(v.props.is_empty());
    }

    #[test]
    fn every_event_kind_has_exactly_one_on_setter() {
        // The eighteen setters, each binding its own EventKind and nothing
        // else. A new EventKind without a setter fails this test.
        let bound: Vec<EventKind> = vec![
            widget::<Msg>(Kind::Button).on_click(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_activate(Msg::Clicked),
            widget::<Msg>(Kind::Popover).on_close(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_in(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_out(Msg::Clicked),
            widget::<Msg>(Kind::ToggleButton).on_toggle(Msg::Flag),
            widget::<Msg>(Kind::Expander).on_expanded(Msg::Flag),
            widget::<Msg>(Kind::Entry).on_change(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::SearchEntry).on_search(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::LinkButton).on_activate_link(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::Calendar).on_date_selected(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::DropDown).on_selected(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_page_changed(Msg::Index),
            widget::<Msg>(Kind::AlertDialog).on_response(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_reordered(Msg::Index),
            widget::<Msg>(Kind::Scale).on_value_changed(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::ScrolledWindow).on_scrolled(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::Entry).on_key(|_| Some(Msg::Clicked)),
        ]
        .into_iter()
        .map(|v| {
            assert_eq!(v.handlers.len(), 1, "a setter bound more than one event");
            *EventKind::ALL
                .iter()
                .find(|k| v.handlers.has(**k))
                .expect("the setter bound no event")
        })
        .collect();

        let mut sorted = bound.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            18,
            "two setters bound the same EventKind: {bound:?}"
        );
        assert_eq!(sorted, EventKind::ALL.to_vec());
    }

    #[test]
    fn the_typed_setters_carry_their_payload_through() {
        let v: View<Msg> = widget(Kind::Entry).on_change(|s| Msg::Text(s.to_owned()));
        assert_eq!(
            v.handlers.fire_text(EventKind::Change, "abc"),
            Some(Msg::Text("abc".into()))
        );

        let v: View<Msg> = widget(Kind::DropDown).on_selected(Msg::Index);
        assert_eq!(
            v.handlers.fire_index(EventKind::Selected, 4),
            Some(Msg::Index(4))
        );

        let v: View<Msg> = widget(Kind::Scale).on_value_changed(|x| Msg::Value(x as u64));
        assert_eq!(
            v.handlers.fire_float(EventKind::ValueChanged, 12.0),
            Some(Msg::Value(12))
        );
    }

    #[test]
    fn a_key_handler_that_declines_produces_nothing() {
        let v: View<Msg> = widget(Kind::Entry).on_key(|_| None);
        let ev = crate::window::keyboard::Keymap::from_string(include_str!(
            "../../tests/fixtures/keymaps/us.xkb"
        ))
        .expect("the vendored us keymap compiles")
        .translate(38, true, 1, 0); // evdev 38 == `a`
        assert_eq!(v.handlers.fire_key(EventKind::KeyPressed, &ev), None);
    }

    #[test]
    fn builders_chain_with_the_universal_setters() {
        let v: View<Msg> = widget::<Msg>(Kind::Button)
            .class("suggested-action")
            .margin(0, 6, 0, 6)
            .on_click(Msg::Clicked)
            .key("ok");
        assert_eq!(v.kind, Kind::Button);
        assert_eq!(
            v.props.get(PropName::Margin),
            Some(&crate::view::Prop::Edges([0, 6, 0, 6]))
        );
        assert_eq!(v.handlers.fire_unit(EventKind::Click), Some(Msg::Clicked));
        assert_eq!(v.key, Some(crate::view::Key::Name(std::rc::Rc::from("ok"))));
    }
}
