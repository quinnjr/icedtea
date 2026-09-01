//! `GtkPaned` -- two children and a draggable separator between them.
//!
//! ```text
//! paned
//! ├── <child>
//! ├── separator[.wide]
//! ╰── <child>
//! ```
//! (`gtk/gtkpaned.c:92`)

use crate::css::node::{Node, PseudoStates};
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::Orientation;
use crate::widgets::{
    Universal, WidgetEnum as _, prop_bool, prop_i64, set_child_layout, set_container,
};
use crate::window::keyboard::Mods;

/// `GtkPaned`.
pub struct PanedC {
    /// Divider offset from the leading edge, px.
    pub position: f32,
    /// Grab offset while dragging: `Some(pointer - position)`.
    pub drag: Option<f32>,
    /// `.wide` on the separator.
    pub wide: bool,
    /// The separator node.
    pub separator: Node,
    /// Main axis.
    pub orientation: Orientation,
    /// Whether the position was set explicitly (`GtkPaned:position-set`).
    pub position_set: bool,
    /// `resize-start-child` / `resize-end-child`.
    pub resize: (bool, bool),
    /// `shrink-start-child` / `shrink-end-child`.
    pub shrink: (bool, bool),
    /// The separator has keyboard focus, so arrows move it.
    pub handle_focused: bool,
    universal: Universal,
}

impl PanedC {
    /// The current divider offset -- the test hook.
    ///
    /// `Controller<Msg>: std::any::Any` (`view::controller`'s own doc
    /// comment) is what lets a `&dyn Controller<Msg>` coerce straight to
    /// `&dyn Any` at this call site -- trait-object upcasting, stable since
    /// the workspace's pinned Rust, needs no `as_any` method on the trait
    /// itself (`widgets::child_slot` already relies on the same coercion).
    #[must_use]
    pub fn position_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> f32 {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>().map_or(f32::NAN, |c| c.position)
    }

    fn axis_of(&self, local: (f32, f32)) -> f32 {
        match self.orientation {
            Orientation::Horizontal => local.0,
            Orientation::Vertical => local.1,
        }
    }

    /// Clamp and record a new divider offset, and realign the leading
    /// child.
    fn set_position(&mut self, value: f32) {
        self.position = if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        };
        if let Some(child) = self.separator.parent().and_then(|p| p.child(0)) {
            set_child_layout(
                &child,
                ChildLayout {
                    hexpand: false,
                    vexpand: false,
                    halign: Align::Fill,
                    valign: Align::Fill,
                    margin: [0.0; 4],
                    ..ChildLayout::default()
                },
            );
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PanedC {
    fn kind(&self) -> Kind {
        Kind::Paned
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let orientation =
            Orientation::from_prop(props.get(PropName::Orientation), Orientation::Horizontal);
        set_container(
            node,
            Container::Box {
                direction: match orientation {
                    Orientation::Horizontal => BoxDirection::Row,
                    Orientation::Vertical => BoxDirection::Column,
                },
            },
        );
        // `build` runs before the reconciler attaches this view's own
        // children (`view::reconcile::build_instance`'s own order: controller
        // first, `reconcile` over `view.children` after), so `node` has none
        // yet here whatever the final child count will be. The separator
        // therefore always lands at index 0 -- `insert_child` clamps to
        // `child_count()`, which is zero -- and stays the widget's sole
        // subnode until real children are attached.
        let separator = Node::new("separator");
        node.insert_child(0, &separator);
        let mut me = Self {
            position: 0.0,
            drag: None,
            wide: props.bool(PropName::WideHandle, false),
            separator,
            orientation,
            position_set: props.get(PropName::Position).is_some(),
            resize: (
                props.bool(PropName::ResizeStart, true),
                props.bool(PropName::ResizeEnd, true),
            ),
            shrink: (
                props.bool(PropName::ShrinkStart, true),
                props.bool(PropName::ShrinkEnd, true),
            ),
            handle_focused: false,
            universal: Universal::new(node, Kind::Paned),
        };
        if me.wide {
            me.separator.add_class("wide");
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "widget positions never reach 2^53"
        )]
        me.set_position(props.int(PropName::Position, 0).max(0) as f32);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Position => {
                self.position_set = true;
                self.set_position(prop_i64(value, 0).max(0) as f32);
            }
            PropName::PositionSet => self.position_set = prop_bool(value, false),
            PropName::WideHandle => {
                self.wide = prop_bool(value, false);
                if self.wide {
                    self.separator.add_class("wide");
                } else {
                    self.separator.remove_class("wide");
                }
            }
            PropName::Orientation => {
                self.orientation = Orientation::from_prop(Some(value), Orientation::Horizontal);
            }
            PropName::ResizeStart => self.resize.0 = prop_bool(value, true),
            PropName::ResizeEnd => self.resize.1 = prop_bool(value, true),
            PropName::ShrinkStart => self.shrink.0 = prop_bool(value, true),
            PropName::ShrinkEnd => self.shrink.1 = prop_bool(value, true),
            other => {
                self.universal.apply(node, Kind::Paned, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut out = Vec::new();
        match ev {
            Event::PointerDown { local, .. } => {
                self.drag = Some(self.axis_of(*local) - self.position);
                self.separator.set_state(PseudoStates::ACTIVE, true);
                cx.handled = true;
            }
            Event::PointerMotion { local } => {
                if let Some(offset) = self.drag {
                    self.set_position(self.axis_of(*local) - offset);
                    if let Some(msg) = cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, f64::from(self.position))
                    {
                        out.push(msg);
                    }
                    cx.handled = true;
                }
            }
            Event::PointerUp { .. } => {
                self.drag = None;
                self.separator.set_state(PseudoStates::ACTIVE, false);
            }
            Event::FocusIn { .. } => self.handle_focused = true,
            Event::FocusOut => self.handle_focused = false,
            Event::Key(key) if self.handle_focused && key.pressed => {
                use xkbcommon::xkb::keysyms;
                let step = if key.mods.contains(Mods::CTRL) {
                    10.0
                } else {
                    1.0
                };
                let moved = match u32::from(key.keysym) {
                    keysyms::KEY_Left | keysyms::KEY_Up => Some(self.position - step),
                    keysyms::KEY_Right | keysyms::KEY_Down => Some(self.position + step),
                    keysyms::KEY_Home => Some(0.0),
                    keysyms::KEY_End => Some(f32::MAX / 4.0),
                    _ => None,
                };
                if let Some(value) = moved {
                    self.set_position(value);
                    if let Some(msg) = cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, f64::from(self.position))
                    {
                        out.push(msg);
                    }
                    cx.handled = true;
                }
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::types::Orientation;
    use crate::widgets::{WidgetEnum as _, build_widget, matches_fixture};

    fn props(position: i64, wide: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Position, Prop::Int(position));
        p.set(PropName::WideHandle, Prop::Bool(wide));
        p.set(
            PropName::Orientation,
            Prop::Enum(Orientation::Horizontal.to_u16()),
        );
        p
    }

    #[test]
    fn a_paned_is_child_separator_child() {
        // Mutation check: appending the separator last (the natural
        // "add children then the handle" order) fails the fixture, and the
        // handle then sits outside both panes.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        matches_fixture(
            &built.node,
            "paned\n├── <child>\n├── separator[.wide]\n╰── <child>\n",
        )
        .expect("paned fixture");
    }

    #[test]
    fn the_wide_handle_prop_toggles_the_separators_class() {
        // Mutation check: setting `.wide` on the paned node instead of the
        // separator means Adwaita's `paned > separator.wide` never matches
        // and the handle keeps its 1px width.
        //
        // Reconciliation: the task text's own reference `PanedC::build`
        // inserts the separator at `1.min(node.child_count())`, and
        // `build_widget` (like the real reconciler -- `build_instance` runs
        // the controller before attaching any view children) always calls
        // `build` on an empty node, so that index is always
        // `1.min(0) == 0`, not `1`; the separator is `built.node.child(0)`
        // here, not `child(1)` as the task text's test literally reads.
        let built = build_widget::<()>(Kind::Paned, &props(100, true));
        let separator = built.node.child(0).expect("separator");
        assert!(separator.classes().iter().any(|c| c.as_str() == "wide"));
    }

    #[test]
    fn dragging_the_separator_moves_the_position_and_reports_it() {
        // Interaction test. Mutation check: applying the pointer's absolute
        // x instead of the delta from the grab point makes the position jump
        // to the cursor on the first motion.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &crate::view::Event::PointerDown {
                button: 0x110,
                local: (100.0, 10.0),
                serial: 1,
            },
            &mut cx,
        );
        c.on_event(
            &crate::view::Event::PointerMotion {
                local: (140.0, 10.0),
            },
            &mut cx,
        );
        assert_eq!(
            crate::widgets::paned::PanedC::position_of(c.as_ref()),
            140.0
        );
        c.on_event(
            &crate::view::Event::PointerUp {
                button: 0x110,
                local: (140.0, 10.0),
                serial: 2,
            },
            &mut cx,
        );
        c.on_event(
            &crate::view::Event::PointerMotion {
                local: (200.0, 10.0),
            },
            &mut cx,
        );
        assert_eq!(
            crate::widgets::paned::PanedC::position_of(c.as_ref()),
            140.0,
            "motion after the release is not a drag"
        );
    }

    #[test]
    fn arrow_keys_move_the_focused_separator_and_home_end_go_to_the_extremes() {
        // Mutation check: handling arrows on the paned node rather than the
        // focused separator makes a paned steal arrow keys from a focused
        // child list.
        let built = build_widget::<()>(Kind::Paned, &props(100, false));
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &crate::view::Event::FocusIn {
                cause: crate::window::focus::FocusCause::Keyboard,
            },
            &mut cx,
        );
        c.on_event(
            &crate::view::Event::Key(crate::widgets::Headless::key("Right")),
            &mut cx,
        );
        assert_eq!(
            crate::widgets::paned::PanedC::position_of(c.as_ref()),
            101.0
        );
        c.on_event(
            &crate::view::Event::Key(crate::widgets::Headless::key("Home")),
            &mut cx,
        );
        assert_eq!(crate::widgets::paned::PanedC::position_of(c.as_ref()), 0.0);
    }

    #[test]
    fn a_hostile_position_never_panics_and_never_leaves_the_pane_negative() {
        for v in [i64::MIN, -1, 0, i64::MAX] {
            let built = build_widget::<()>(Kind::Paned, &props(v, false));
            assert!(crate::widgets::paned::PanedC::position_of(built.controller.as_ref()) >= 0.0);
        }
    }
}
