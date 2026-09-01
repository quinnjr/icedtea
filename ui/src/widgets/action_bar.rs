//! `GtkActionBar` -- `Kind::ActionBar`, CSS node `actionbar`.
//!
//! ```text
//! actionbar
//! ╰── revealer
//!     ╰── box
//!         ├── box.start
//!         │   ╰── <child>
//!         ├── box.center
//!         │   ╰── <child>
//!         ╰── box.end
//!             ╰── <child>
//! ```
//!
//! `box.start`/`box.center`/`box.end` are built once, in [`ActionBarC::build`],
//! and never removed: they are what `ActionBarC::apply` centres the middle
//! one against. [`crate::view::builders::pack_start`]/`pack_end`/`center`
//! tag a child with [`PropName::Section`] rather than routing it to a
//! different [`crate::widgets::child_slot`] (there is only one attach point
//! a reconciled batch of children can land on), so this controller does not
//! use `child_slot` at all: the view's real children land flat, as direct
//! siblings of `revealer` on the widget's own root node -- the same
//! "chrome alongside the view's children" arrangement `Frame`'s label and
//! `Paned`'s separator use, just with the chrome (the revealer subtree)
//! *before* every view child instead of interleaved. `ActionBarC::place`
//! then walks that flat list and moves each real child into its recorded
//! section, reading it back with `crate::widgets::props_of` the same way
//! [`crate::widgets::grid::GridC`] re-derives a child's grid cell -- from the
//! child's own node, not from an `Instance` this controller does not own.
//!
//! `ActionBarC::place` runs from [`Controller::reserved_total`]: the
//! reconciler calls it exactly once per reconcile, after every new or moved
//! child has already been attached flat to the root (`view::reconcile`'s
//! step 4), and before it trims anything past the returned bound -- the one
//! hook in the trait that both runs on every pass and hands this controller
//! a moment to react after the physical tree is in a known state.

use crate::css::node::Node;
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{Revealer, Universal, prop_bool, set_child_layout, set_container};

/// `GtkActionBar`'s own setters, chained after
/// [`crate::view::builders::action_bar`].
///
/// `.revealed` is scoped to this trait rather than a plain inherent
/// `View<Msg>` method for the same reason `SearchBarC`'s own setters are
/// (`search_bar`'s module doc): `GtkInfoBar`'s `InfoBarExt::revealed` already
/// exists over a different prop (`Reveal`), and an inherent method would
/// silently win over it at every call site, `InfoBar`'s included.
pub trait ActionBarExt<Msg>: Sized {
    /// `GtkActionBar:revealed`.
    fn revealed(self, v: impl Into<Prop>) -> Self;
}

impl<Msg: Clone + 'static> ActionBarExt<Msg> for View<Msg> {
    fn revealed(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Revealed, v)
    }
}

/// `GtkActionBar`.
pub struct ActionBarC {
    revealer: Revealer,
    start: Node,
    center: Node,
    end: Node,
    universal: Universal,
}

impl ActionBarC {
    fn apply(&self) {
        for (index, child) in [&self.start, &self.center, &self.end]
            .into_iter()
            .enumerate()
        {
            let outer = index != 1;
            set_container(
                child,
                Container::Box {
                    direction: BoxDirection::Row,
                },
            );
            set_child_layout(
                child,
                ChildLayout {
                    hexpand: outer,
                    halign: if outer { Align::Fill } else { Align::Center },
                    valign: Align::Center,
                    ..ChildLayout::default()
                },
            );
        }
    }

    /// Move every real child the reconciler just attached flat to `node`'s
    /// parent into its recorded [`PropName::Section`] -- `"start"`
    /// (the default), `"center"` or `"end"`.
    fn place(&self) {
        let Some(root) = self.revealer.node.parent() else {
            return;
        };
        for child in root.children() {
            if child.ptr_eq(&self.revealer.node) {
                continue;
            }
            let props = crate::widgets::props_of(&child);
            match props.str(PropName::Section).unwrap_or("start") {
                "end" => self.end.append_child(&child),
                "center" => self.center.append_child(&child),
                _ => self.start.append_child(&child),
            }
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ActionBarC {
    fn kind(&self) -> Kind {
        Kind::ActionBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let revealer = Revealer::build(node, props.bool(PropName::Revealed, false));
        let contents = Node::new("box");
        revealer.node.append_child(&contents);
        let start = Node::with_classes("box", &["start"]);
        let center = Node::with_classes("box", &["center"]);
        let end = Node::with_classes("box", &["end"]);
        for child in [&start, &center, &end] {
            contents.append_child(child);
        }
        let me = Self {
            revealer,
            start,
            center,
            end,
            universal: Universal::new(node, Kind::ActionBar),
        };
        me.apply();
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::Revealed => {
                self.revealer
                    .set_revealed(prop_bool(value, false), cx.clock.now());
            }
            other => {
                self.universal.apply(node, Kind::ActionBar, other, value);
            }
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn tick(&mut self, now: std::time::Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        self.revealer.tick(now);
        Vec::new()
    }

    fn next_deadline(&self, now: std::time::Duration) -> Option<std::time::Duration> {
        self.revealer.next_deadline(now)
    }

    fn child_index(&self, view_index: usize) -> usize {
        // Chrome (the revealer subtree) sits at index 0 on the root; every
        // real child lands flat, after it, until `place` sorts them out.
        view_index + 1
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        let _ = view_count;
        self.place();
        // `place` has just emptied the root back down to the revealer alone.
        1
    }
}

#[cfg(test)]
mod tests {
    use crate::view::builders::{action_bar, button, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::action_bar::ActionBarExt as _;
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, matches_fixture};

    const FIXTURE: &str = "actionbar\n\
        ╰── revealer\n\
        \x20   ╰── box\n\
        \x20       ├── box.start\n\
        \x20       │   ╰── <child>\n\
        \x20       ├── box.center\n\
        \x20       │   ╰── <child>\n\
        \x20       ╰── box.end\n\
        \x20           ╰── <child>\n";

    #[test]
    fn the_bar_has_a_start_box_an_optional_centre_and_an_end_box() {
        // Mutation check: packing end children into the start box (an easy
        // slot mix-up) puts them on the wrong side and fails the fixture's
        // `box.end` requirement.
        let built = build_widget::<()>(Kind::ActionBar, &Props::default());
        matches_fixture(&built.node, FIXTURE).expect("action_bar fixture");
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        action_bar()
            .pack_start(button("L"))
            .pack_end(button("R"))
            .center(label("C"))
            .revealed(true)
    }

    #[test]
    fn packed_children_land_on_their_own_side_at_rest() {
        // Rest-state pixel test. Mutation check: dropping the slot read puts
        // all three children in tree order at the left and the right third
        // of the surface stays empty.
        let out = frames((), update, view, (300, 48), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 150, 2);
        let ink_in = |from: u32, to: u32| (from..to).any(|x| px(&out, 0, x, 24) != background);
        assert!(ink_in(0, 100), "start pack painted");
        assert!(ink_in(120, 180), "centre widget painted");
        assert!(ink_in(200, 300), "end pack painted");
    }

    #[test]
    fn collapsing_the_bar_animates_and_then_stops_asking_for_frames() {
        // Interaction test + the "ZERO means now, never spin" rule.
        let built = build_widget::<()>(Kind::ActionBar, &Props::default());
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        // Reconciliation: the task text builds `cx` before this `set_prop`
        // call and then borrows `hx` a second time (`&mut hx.cx()`) while
        // `cx` is still live -- `Headless::event_cx`'s own doc comment says
        // its `EventCx` holds `Headless` mutably borrowed for as long as a
        // test keeps using it, so a second `&mut self` borrow through
        // `hx.cx()` in the same scope does not borrow-check. `set_prop`
        // takes its own `BuildCx` first, ahead of the `EventCx` `tick` needs.
        c.set_prop(
            &built.node,
            PropName::Revealed,
            &Prop::Bool(true),
            &mut hx.cx(),
        );
        assert!(c.next_deadline(std::time::Duration::ZERO).is_some());
        let mut cx = hx.event_cx(&built.node);
        c.tick(std::time::Duration::from_millis(500), &mut cx);
        assert_eq!(c.next_deadline(std::time::Duration::from_millis(500)), None);
    }
}
