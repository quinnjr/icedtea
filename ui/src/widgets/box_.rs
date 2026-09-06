//! `GtkBox` -- a single `box` node whose children are laid out on one axis.

use crate::css::node::Node;
use crate::layout::{BoxDirection, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::{BaselinePosition, Orientation};
use crate::widgets::{Universal, WidgetEnum as _, prop_bool, prop_i64, prop_u16};

/// `GtkBox`.
pub struct BoxC {
    /// The main axis.
    pub orientation: Orientation,
    /// Gap between children, px.
    pub spacing: f32,
    /// Every child gets the same main-axis extent.
    pub homogeneous: bool,
    /// Where extra space goes when children have baselines.
    pub baseline_position: BaselinePosition,
    universal: Universal,
}

impl BoxC {
    /// This node's recorded container -- the test hook, and how a parent
    /// controller asks what axis a child box is on.
    #[must_use]
    pub fn container_of(node: &Node) -> Container {
        crate::widgets::container_of(node)
    }

    fn axis(orientation: Orientation) -> BoxDirection {
        match orientation {
            Orientation::Horizontal => BoxDirection::Row,
            Orientation::Vertical => BoxDirection::Column,
        }
    }

    fn apply(&self, node: &Node) {
        crate::widgets::set_container(
            node,
            Container::Box {
                direction: Self::axis(self.orientation),
            },
        );
        // `spacing` is GTK's own gap; the CSS `border-spacing` still applies
        // and the larger of the two wins, exactly as GtkBox does.
        crate::widgets::set_gap(node, self.spacing, self.orientation);
        crate::widgets::set_homogeneous(node, self.homogeneous, self.orientation);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for BoxC {
    fn kind(&self) -> Kind {
        Kind::Box
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let me = Self {
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            spacing: props.int(PropName::Spacing, 0).clamp(0, 1_000_000) as f32,
            homogeneous: props.bool(PropName::Homogeneous, false),
            baseline_position: BaselinePosition::from_u16(
                u16::try_from(props.int(PropName::BaselinePosition, 1)).unwrap_or(1),
            ),
            universal: Universal::new(node, Kind::Box),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Orientation => {
                self.orientation = Orientation::from_prop(Some(value), Orientation::Horizontal);
            }
            PropName::Spacing => {
                self.spacing = prop_i64(value, 0).clamp(0, 1_000_000) as f32;
            }
            PropName::Homogeneous => {
                self.homogeneous = prop_bool(value, false);
            }
            PropName::BaselinePosition => {
                self.baseline_position = BaselinePosition::from_u16(prop_u16(value, 1));
            }
            other => {
                self.universal.apply(node, Kind::Box, other, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Reconciliation (Task 8 fix-round, recorded as P3-D9): matches
        // `GenericC::on_event`'s own `Event::Key` arm. `Kind::Box` (this
        // controller) is not the catch-all `GenericC` — `build_controller`
        // gives every `box_(...)` node this dedicated controller — so a
        // `.on_key` handler on a `box_(...)` root (as `settings/src/app.rs`
        // wires its window-root capture handler, P3-D1) previously had no
        // path to ever fire: `BoxC::on_event` ignored every event
        // unconditionally, `deliver`'s only Key-firing call site was this
        // arm inside `GenericC`, and D19 keys off `cx.handled`, which
        // nothing here ever set for a `Key` event either way.
        match ev {
            Event::Key(key) if key.pressed => cx
                .handlers
                .fire_key(EventKind::KeyPressed, key)
                .into_iter()
                .inspect(|_| cx.handled = true)
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BoxC;
    use crate::layout::{BoxDirection, Container};
    use crate::view::builders::{BoxExt, box_, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::types::Orientation;
    use crate::widgets::{build_widget, matches_fixture};

    fn props(spacing: i64, horizontal: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Spacing, Prop::Int(spacing));
        p.set(
            PropName::Orientation,
            Prop::Enum(if horizontal { 0 } else { 1 }),
        );
        p
    }

    #[test]
    fn a_box_is_one_node_named_box() {
        // Mutation check: giving BoxC a wrapper subnode (the "I need
        // somewhere to hang spacing" mistake) adds a child and fails the
        // fixture, which is a single line.
        let built = build_widget::<()>(Kind::Box, &props(6, true));
        matches_fixture(&built.node, "box\n").expect("box fixture");
    }

    #[test]
    fn the_orientation_prop_picks_the_taffy_axis() {
        // Mutation check: ignoring PropName::Orientation in set_prop leaves
        // every box a column and the whole toolkit lays out vertically.
        let built = build_widget::<()>(Kind::Box, &props(0, true));
        let c = built.controller;
        assert_eq!(c.kind(), Kind::Box);
        let horizontal = build_widget::<()>(Kind::Box, &props(0, true));
        let vertical = build_widget::<()>(Kind::Box, &props(0, false));
        assert_eq!(
            BoxC::container_of(&horizontal.node),
            Container::Box {
                direction: BoxDirection::Row
            }
        );
        assert_eq!(
            BoxC::container_of(&vertical.node),
            Container::Box {
                direction: BoxDirection::Column
            }
        );
    }

    #[derive(Clone)]
    enum Msg {}

    fn view(_: &()) -> View<Msg> {
        box_(
            Orientation::Horizontal,
            [label("a").class("first"), label("b").class("second")],
        )
        .spacing(12)
        .class("frame")
    }

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    #[test]
    fn a_box_paints_its_children_twelve_pixels_apart_at_rest() {
        // Rest-state pixel test. Mutation check: dropping `spacing` from
        // set_prop puts the two labels adjacent and the gap column carries
        // glyph pixels instead of the container's background.
        let out = frames((), update, view, (200, 40), vec![ScriptStep::Capture]);
        assert_eq!(out.len(), 1);
        let background = px(&out, 0, 199, 39);
        // The 12px gap is background all the way down its middle column.
        let gap = crate::widgets::offscreen::gap_column(&out);
        assert_eq!(
            px(&out, 0, gap, 20),
            background,
            "the spacing column is empty"
        );
    }

    #[test]
    fn re_running_the_view_with_the_same_model_repaints_identically() {
        // Interaction test at the container level: reconciliation must not
        // move anything. Mutation check: rebuilding the child nodes in
        // set_prop (instead of mutating them) shifts the second frame.
        let out = frames(
            (),
            update,
            view,
            (200, 40),
            vec![
                ScriptStep::Capture,
                ScriptStep::Advance(std::time::Duration::from_millis(16)),
                ScriptStep::Capture,
            ],
        );
        for x in [0_u32, 50, 199] {
            assert_eq!(px(&out, 0, x, 20), px(&out, 1, x, 20));
        }
    }

    #[test]
    fn hostile_spacing_and_orientation_values_never_panic() {
        // Mutation check: forwarding a NaN spacing to taffy makes the layout
        // NaN and every later allocation unusable; `finite` clamps it.
        for v in [i64::MIN, i64::MAX, -1, 0] {
            let mut p = Props::default();
            p.set(PropName::Spacing, Prop::Int(v));
            p.set(PropName::Orientation, Prop::Enum(u16::MAX));
            p.set(PropName::Homogeneous, Prop::Str("yes".into()));
            let _ = build_widget::<()>(Kind::Box, &p);
        }
    }
}
