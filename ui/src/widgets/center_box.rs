//! `GtkCenterBox` -- three children, the middle one centred in the whole
//! allocation, the *first* one placed by text direction.

use crate::css::node::{Direction, Node};
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::types::Orientation;
use crate::widgets::{Universal, WidgetEnum as _, prop_bool};

/// `GtkCenterBox`.
pub struct CenterBoxC {
    /// The main axis.
    pub orientation: Orientation,
    /// The centre child shrinks last, not first.
    pub shrink_center_last: bool,
    universal: Universal,
}

impl CenterBoxC {
    /// Which edge the *first* child sits against, for the current direction.
    ///
    /// `gtk/gtkcenterbox.c:45`: "The first child of the GtkCenterBox will be
    /// allocated depending on the text direction."
    #[must_use]
    pub fn first_child_edge(node: &Node) -> Align {
        match node.direction() {
            Direction::Ltr => Align::Start,
            Direction::Rtl => Align::End,
        }
    }

    fn apply(&self, node: &Node) {
        let direction = match self.orientation {
            Orientation::Horizontal => BoxDirection::Row,
            Orientation::Vertical => BoxDirection::Column,
        };
        crate::widgets::set_container(
            node,
            Container::Center {
                direction,
                shrink_center_last: self.shrink_center_last,
            },
        );
        // The outer two grow from zero so the leftover space splits evenly
        // and the middle child lands on the container's centre whatever the
        // ends measure; the middle one refuses to grow.
        let children = node.children();
        for (i, child) in children.iter().enumerate() {
            let outer = i != 1;
            crate::widgets::set_child_layout(
                child,
                ChildLayout {
                    hexpand: outer && self.orientation == Orientation::Horizontal,
                    vexpand: outer && self.orientation == Orientation::Vertical,
                    halign: if outer { Align::Fill } else { Align::Center },
                    valign: Align::Center,
                    ..ChildLayout::default()
                },
            );
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CenterBoxC {
    fn kind(&self) -> Kind {
        Kind::CenterBox
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let me = Self {
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            shrink_center_last: props.bool(PropName::ShrinkCenterLast, true),
            universal: Universal::new(node, Kind::CenterBox),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Orientation => {
                self.orientation = Orientation::from_prop(Some(value), self.orientation);
            }
            PropName::ShrinkCenterLast => self.shrink_center_last = prop_bool(value, true),
            other => {
                self.universal.apply(node, Kind::CenterBox, other, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::CenterBoxC;
    use crate::css::node::Direction;
    use crate::layout::{BoxDirection, Container};
    use crate::view::builders::{center_box, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, container_of, matches_fixture};

    #[test]
    fn a_centre_box_is_one_node_named_box() {
        // Mutation check: `Kind::CenterBox` reusing `centerbox` as its CSS
        // name (the obvious guess) fails the fixture, whose node is `box`
        // (gtk/gtkcenterbox.c:341).
        let built = build_widget::<()>(Kind::CenterBox, &Props::default());
        matches_fixture(&built.node, "box\n").expect("center_box fixture");
        assert_eq!(&*built.node.name(), "box");
    }

    #[test]
    fn it_records_a_centre_container_carrying_shrink_center_last() {
        // Mutation check: dropping shrink_center_last from the Container
        // makes the flag unreadable by `container_of` and the centre child
        // shrinks first, hiding the title before the buttons.
        let mut props = Props::default();
        props.set(PropName::ShrinkCenterLast, Prop::Bool(true));
        let built = build_widget::<()>(Kind::CenterBox, &props);
        assert_eq!(
            container_of(&built.node),
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: true
            }
        );
        assert_eq!(built.controller.kind(), Kind::CenterBox);
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        center_box(
            label("start").class("s"),
            label("mid").class("m"),
            label("end-of-it").class("e"),
        )
    }

    #[test]
    fn the_centre_child_is_centred_although_the_ends_differ_in_width() {
        // Rest-state pixel test. Mutation check: laying the three children
        // out as a plain box makes the middle child's ink centre land left
        // of the surface centre, because "end-of-it" is wider than "start".
        let out = frames((), update, view, (300, 40), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 0, 0);
        let row = 20;
        let ink: Vec<u32> = (0..300)
            .filter(|x| px(&out, 0, *x, row) != background)
            .collect();
        let mid_ink: Vec<u32> = ink
            .iter()
            .copied()
            .filter(|x| (100..200).contains(x))
            .collect();
        let centre = (mid_ink.first().copied().unwrap() + mid_ink.last().copied().unwrap()) / 2;
        assert!(
            centre.abs_diff(150) <= 2,
            "the centre child's ink centre is the surface centre, got {centre}"
        );
    }

    #[test]
    fn under_rtl_the_first_child_is_allocated_on_the_right() {
        // Interaction test (direction is a live property, not a build-time
        // one). Mutation check: ignoring `Direction::Rtl` in the container
        // write puts "start"'s ink in the left third under both directions.
        let built = build_widget::<()>(Kind::CenterBox, &Props::default());
        built.node.set_direction(Some(Direction::Rtl));
        assert_eq!(
            CenterBoxC::first_child_edge(&built.node),
            crate::layout::Align::End
        );
    }

    #[test]
    fn hostile_orientation_and_slot_values_never_panic() {
        for raw in [0_u16, 1, 7, u16::MAX] {
            let mut props = Props::default();
            props.set(PropName::Orientation, Prop::Enum(raw));
            props.set(PropName::ShrinkCenterLast, Prop::Str("maybe".into()));
            props.set(PropName::Section, Prop::Int(3));
            let _ = build_widget::<()>(Kind::CenterBox, &props);
        }
    }
}
