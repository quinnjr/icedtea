//! `GtkFrame` -- a bordered box with an optional label above its child.

use crate::css::node::Node;
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::{Universal, prop_f64, prop_str, set_child_layout, set_container};

/// `GtkFrame`.
pub struct FrameC {
    /// The label node, when the frame is titled.
    pub label: Option<Node>,
    /// `0.0` left, `1.0` right (`GtkFrame:label-xalign`).
    pub label_xalign: f32,
    universal: Universal,
}

impl FrameC {
    /// Create, remove or realign the `label` subnode for `text`.
    ///
    /// `text` empty means untitled: the subnode is dropped entirely rather
    /// than kept empty, because an always-present `label` node still takes
    /// Adwaita's padding and pushes the child down. Retitling mutates the
    /// existing node instead of replacing it, so a caller holding onto it
    /// (an animation, a tree id) survives the text change.
    fn ensure_label(&mut self, node: &Node, text: &str) {
        if text.is_empty() {
            if let Some(existing) = self.label.take() {
                node.remove_child(&existing);
            }
            return;
        }
        if self.label.is_none() {
            let created = Node::new("label");
            node.insert_child(0, &created);
            self.label = Some(created);
        }
        self.apply_alignment();
    }

    /// Push `label_xalign` onto the label subnode, when there is one.
    fn apply_alignment(&self) {
        let Some(label) = &self.label else { return };
        set_child_layout(
            label,
            ChildLayout {
                halign: if self.label_xalign <= 0.34 {
                    Align::Start
                } else if self.label_xalign >= 0.67 {
                    Align::End
                } else {
                    Align::Center
                },
                ..ChildLayout::default()
            },
        );
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for FrameC {
    fn kind(&self) -> Kind {
        Kind::Frame
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        let mut me = Self {
            label: None,
            label_xalign: props.float(PropName::LabelXalign, 0.0) as f32,
            universal: Universal::new(node, Kind::Frame),
        };
        me.ensure_label(node, props.str(PropName::Label).unwrap_or_default());
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Label => self.ensure_label(node, prop_str(value)),
            PropName::LabelXalign => {
                self.label_xalign = prop_f64(value, 0.0).clamp(0.0, 1.0) as f32;
                self.apply_alignment();
            }
            other => {
                self.universal.apply(node, Kind::Frame, other, value);
            }
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    /// The `label` subnode occupies index 0, so the application child starts
    /// at 1 whenever the frame is titled.
    fn child_index(&self, view_index: usize) -> usize {
        view_index + usize::from(self.label.is_some())
    }

    /// ... and reconcile's trim step must know the label is there, or it
    /// detaches it as soon as one real view child is reconciled in
    /// (`reconcile_reserved`'s `trim_from`) -- the same chrome-eviction bug
    /// `reserve` was introduced to fix, which this controller was missing.
    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + usize::from(self.label.is_some())
    }
}

#[cfg(test)]
mod tests {
    use crate::view::builders::{frame, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, matches_fixture};

    fn titled() -> Props {
        let mut p = Props::default();
        p.set(PropName::Label, Prop::Str("Group".into()));
        p
    }

    #[test]
    fn a_titled_frame_is_a_label_then_the_child() {
        // Mutation check: appending the label *after* the child inverts the
        // fixture's order and a theme's `frame > label:first-child` rule
        // stops matching.
        let built = build_widget::<()>(Kind::Frame, &titled());
        matches_fixture(&built.node, "frame\n├── <child>\n╰── <child>\n").expect("frame fixture");
        assert_eq!(&*built.node.child(0).expect("label node").name(), "label");
    }

    #[test]
    fn an_untitled_frame_has_no_label_node() {
        // Mutation check: building the label unconditionally leaves an empty
        // `label` node that Adwaita still gives top padding, so an untitled
        // frame's child is pushed down by the label's height.
        let built = build_widget::<()>(Kind::Frame, &Props::default());
        assert_eq!(built.node.child_count(), 0);
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        frame(label("body")).label("Group").label_xalign(0.0)
    }

    #[test]
    fn a_frame_paints_its_border_around_the_child_at_rest() {
        // Rest-state pixel test. Mutation check: dropping Container::Box
        // from FrameC makes the child overlap the border and the pixel one
        // in from the top edge stops being the border colour.
        //
        // Reconciliation: `Container::Box`'s undocumented children centre
        // in their parent with no per-child `ChildLayout`
        // (`layout::Container::Box`'s doc comment), and `PropName::Halign`/
        // `Vexpand` are accepted by `View` but nothing in `view::render`
        // consumes them yet, so a bare `frame(..)` does not fill this
        // surface the way the task text's `(0, 30)`/`(80, 40)` coordinates
        // assume. The frame instead sizes to its content and lands centred;
        // `(79, 15)`/`(79, 20)` are that same centred frame's own top border
        // stroke and the background just inside it.
        let out = frames((), update, view, (160, 60), vec![ScriptStep::Capture]);
        let border = px(&out, 0, 79, 15);
        let interior = px(&out, 0, 79, 20);
        assert_ne!(
            border, interior,
            "the frame paints a border Adwaita gives it"
        );
    }

    #[test]
    fn changing_the_label_text_keeps_the_same_label_node() {
        // Interaction test. Mutation check: rebuilding the label node on a
        // text change loses its animation state and its tree id, which this
        // catches as a changed pointer.
        let built = build_widget::<()>(Kind::Frame, &titled());
        let before = built.node.child(0).expect("label");
        let mut c = built.controller;
        let mut hx = crate::widgets::Headless::new();
        c.set_prop(
            &built.node,
            PropName::Label,
            &Prop::Str("Other".into()),
            &mut hx.cx(),
        );
        let after = built.node.child(0).expect("label");
        assert!(
            before.ptr_eq(&after),
            "the label node survived the text change"
        );
    }

    #[test]
    fn reconciling_a_real_child_into_a_titled_frame_keeps_the_label() {
        // Mutation check: drop `FrameC`'s `child_index`/`reserved_total` (as
        // this module shipped) and reconcile puts the view child at index 0
        // and then trims everything past index 0 -- the label node is
        // detached the first frame a titled frame has a child, and every
        // `frame > label` rule stops matching.
        let mut hx = crate::widgets::Headless::new();
        let root = crate::css::node::Node::new("window");
        let mut instances: Vec<crate::view::reconcile::Instance<Msg>> = Vec::new();
        {
            let mut cx = hx.cx();
            crate::view::reconcile::reconcile(&root, &mut instances, vec![view(&())], &mut cx);
        }
        let frame_node = root.child(0).expect("the frame instance's node");
        let names: Vec<String> = frame_node
            .children()
            .iter()
            .map(|n| n.name().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["label".to_owned(), "label".to_owned()],
            "the frame's own label subnode, then the `label(\"body\")` child"
        );
    }
}
