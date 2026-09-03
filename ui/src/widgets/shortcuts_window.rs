//! `GtkShortcutsWindow` — `Kind::ShortcutsWindow`, CSS node `window`, class
//! `.shortcuts` (`Kind::base_classes`).
//!
//! ```text
//! window.shortcuts
//! ```
//!
//! One `WindowC` embedded directly on this controller's own root node — the
//! composition [`crate::widgets::popover_menu::PopoverMenuC`] uses for its
//! embedded `PopoverC` — plus `sections`, this widget's own real application
//! children: [`shortcuts_window`] takes real [`View`]s (contract §5.7,
//! unlike [`crate::widgets::alert_dialog::alert_dialog`]'s string-and-props
//! shape), so — the same timing gap [`crate::widgets::stack::StackC`]'s own
//! module doc explains for `StackC::place` — `Controller::build` runs
//! before the reconciler attaches those children onto `node`, and
//! `sections` can only be re-derived from `node.children()` once
//! [`Controller::reserved_total`] runs on a real reconcile. That forces
//! `sections` into a `RefCell` rather than the plain field the interface
//! sketch shows, for the identical reason `WindowC::titlebar`'s own module
//! doc gives.
//!
//! `search` is a plain field: nothing outside [`Controller::on_event`]
//! (`&mut self`) ever writes it.

use std::cell::RefCell;
use std::rc::Rc;

use crate::css::node::Node;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::window::WindowC;

/// A `GtkShortcutsWindow` over `sections`, each a real child view.
#[must_use]
pub fn shortcuts_window<Msg: Clone + 'static>(
    sections: impl IntoIterator<Item = View<Msg>>,
) -> View<Msg> {
    View::new(Kind::ShortcutsWindow).children(sections)
}

/// A [`shortcuts_window`] section's own setters, chained on the child
/// [`View`] passed to it — the same shape
/// [`crate::widgets::popover_menu::PopoverMenuItemExt::section`] uses for a
/// menu item's section.
pub trait ShortcutsSectionExt<Msg>: Sized {
    /// This section's name, `GtkShortcutsWindow`'s own model identity for
    /// it.
    fn section_name(self, name: &str) -> Self;
    /// The view this section belongs to (`GtkShortcutsWindow:view-name`'s
    /// filter, for a window with more than one).
    fn view_name(self, name: &str) -> Self;
}

impl<Msg: Clone + 'static> ShortcutsSectionExt<Msg> for View<Msg> {
    fn section_name(self, name: &str) -> Self {
        self.prop(PropName::SectionName, Prop::Str(Rc::from(name)))
    }
    fn view_name(self, name: &str) -> Self {
        self.prop(PropName::ViewName, Prop::Str(Rc::from(name)))
    }
}

/// `Kind::ShortcutsWindow`'s controller.
pub struct ShortcutsWindowC {
    /// The embedded `Window` chrome — see the module doc.
    pub window: WindowC,
    /// One node per real section child, re-derived on every reconcile — see
    /// the module doc for why this is a `RefCell`.
    pub sections: RefCell<Vec<Node>>,
    /// The current search text (`GtkShortcutsWindow`'s built-in search
    /// entry). Nothing types into it yet — see
    /// [`ShortcutsWindowC::on_event`] — so it starts and stays empty until a
    /// caller sets it directly.
    pub search: String,
    /// This controller's own root node, so [`ShortcutsWindowC::place`] (a
    /// `&self` method, run from `reserved_total`) can re-read its real
    /// children — the same need `WindowC`'s own private `node` field meets.
    node: Node,
}

impl ShortcutsWindowC {
    /// Re-derive `sections` from the real children the reconciler has by
    /// now attached onto `self.node`.
    fn place(&self) {
        *self.sections.borrow_mut() = self.node.children();
    }

    /// Test hook: `sections`, through a `&dyn Controller<Msg>` -- the shape
    /// this crate's other test hooks (`AlertDialogC::default_of`) use.
    #[must_use]
    pub fn sections_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Vec<Node> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .map(|m| m.sections.borrow().clone())
            .unwrap_or_default()
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ShortcutsWindowC {
    fn kind(&self) -> Kind {
        Kind::ShortcutsWindow
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let window = <WindowC as Controller<Msg>>::build(node, props, cx);
        ShortcutsWindowC {
            window,
            sections: RefCell::new(Vec::new()),
            search: String::new(),
            node: node.clone(),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        <WindowC as Controller<Msg>>::set_prop(&mut self.window, node, name, value, cx);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut msgs = <WindowC as Controller<Msg>>::on_event(&mut self.window, ev, cx);
        if let Event::Key(key) = ev
            && key.pressed
        {
            use crate::window::keyboard::Mods;
            use xkbcommon::xkb::keysyms;
            let sym = u32::from(key.keysym);
            if sym == keysyms::KEY_Escape {
                cx.handled = true;
                if let Some(m) = cx.handlers.fire_unit(EventKind::Close) {
                    msgs.push(m);
                }
            } else if sym == keysyms::KEY_f && key.effective_mods() == Mods::CTRL {
                cx.handled = true;
                if let Some(m) = cx.handlers.fire_text(EventKind::Search, &self.search) {
                    msgs.push(m);
                }
            }
        }
        msgs
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        <WindowC as Controller<Msg>>::reserved_total(&self.window, view_count);
        self.place();
        view_count
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::shortcuts_window::ShortcutsWindowC;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::Title, Prop::Str("Keyboard Shortcuts".into()));
        p
    }

    #[test]
    fn the_window_is_window_shortcuts() {
        // Mutation check: dropping `.shortcuts` from `Kind::base_classes`
        // leaves this indistinguishable from a plain `Window` in CSS.
        let built = build_widget::<()>(Kind::ShortcutsWindow, &props());
        matches_fixture(&built.node, "window.shortcuts\n").expect("shortcuts_window fixture");
    }

    #[test]
    fn escape_closes_and_ctrl_f_fires_search_with_the_current_text() {
        // Interaction test. Mutation check: firing `Search` without gating
        // on `Ctrl` would fire on every `f` a user types anywhere in the
        // window.
        let built = build_widget::<String>(Kind::ShortcutsWindow, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Close, Handler::Unit("closed".to_string()));
            h.set(
                EventKind::Search,
                Handler::Text(std::rc::Rc::new(|s: &str| s.to_string())),
            );
        });
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("Escape")), &mut cx),
            vec!["closed".to_string()]
        );
        assert_eq!(
            c.on_event(
                &Event::Key(Headless::key_with_mods("f", &["Control"])),
                &mut cx
            ),
            vec![String::new()]
        );
    }

    #[test]
    fn sections_are_re_derived_from_real_children_once_attached() {
        // `sections` is empty until the reconciler attaches this window's
        // real view children -- exactly `WindowC::titlebar`'s own timing
        // gap. Attaching two nodes directly and calling `reserved_total`
        // (what `Controller::reserved_total` is for) simulates that.
        let built = build_widget::<()>(Kind::ShortcutsWindow, &props());
        built.node.append_child(&crate::css::node::Node::new("box"));
        built.node.append_child(&crate::css::node::Node::new("box"));
        let total = built.controller.reserved_total(2);
        assert_eq!(total, 2);
        let sections = ShortcutsWindowC::sections_of::<()>(built.controller.as_ref());
        assert_eq!(sections.len(), 2);
    }
}
