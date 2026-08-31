//! Per-kind behaviour, and the state the model must not own.

use std::rc::Rc;
use std::time::Duration;

use skia_rs_safe::canvas::Canvas;

use crate::anim::Clock;
use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::icons::IconTheme;
use crate::layout::{Allocation, LayoutTree};
use crate::text::FontDatabase;
use crate::view::cmd::Cmd;
use crate::view::reconcile::BuildCx;
use crate::view::render::StyleMap;
use crate::view::{Handlers, Kind, Prop, PropName, Props};
use crate::window::SurfaceStates;
use crate::window::focus::{FocusCause, FocusRing};
use crate::window::keyboard::KeyEvent;
use crate::window::pointer::Scroll;
use crate::window::selection::Clipboard;

#[doc(inline)]
pub use crate::paint::PaintCx;

/// Which leg of GTK's three-phase dispatch is running.
///
/// Contract deviation D5: §4.6 requires "capture → target → bubble" and says
/// a controller that sets `cx.handled = true` "stops the phase it is in" —
/// which a controller cannot honour without seeing the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Root → target, before the target sees the event.
    Capture,
    /// The node the event landed on.
    Target,
    /// Target → root, after the target has had it.
    Bubble,
}

impl Phase {
    /// The three phases, in dispatch order.
    pub const ALL: [Phase; 3] = [Phase::Capture, Phase::Target, Phase::Bubble];
}

/// What a controller sees: P3's `InputEvent` narrowed to this node, plus the
/// lifecycle events the framework synthesises.
#[derive(Debug, Clone)]
pub enum Event {
    /// The pointer entered this node.
    PointerEnter {
        /// Point in the node's border-box space.
        local: (f32, f32),
    },
    /// The pointer moved within this node (or within its implicit grab).
    PointerMotion {
        /// Point in the node's border-box space.
        local: (f32, f32),
    },
    /// The pointer left this node.
    PointerLeave,
    /// A button went down on this node.
    PointerDown {
        /// evdev button code; `window::BTN_LEFT` is `0x110`.
        button: u32,
        /// Point in the node's border-box space.
        local: (f32, f32),
        /// The input serial, for `xdg_popup.grab` and clipboard requests.
        serial: u32,
    },
    /// A button was released.
    PointerUp {
        /// evdev button code.
        button: u32,
        /// Point in the node's border-box space; may be outside the node
        /// when an implicit grab is in force.
        local: (f32, f32),
        /// The input serial.
        serial: u32,
    },
    /// An axis event reached this node.
    Scroll(Scroll),
    /// A key reached this node because it holds the focus.
    Key(KeyEvent),
    /// This node took the focus.
    FocusIn {
        /// What moved the focus here.
        cause: FocusCause,
    },
    /// This node lost the focus.
    FocusOut,
    /// Space/Enter, or a click that completed inside — the "activate" GTK
    /// means.
    Activate,
    /// A popup this controller opened was dismissed by the compositor.
    PopupDone,
    /// The window's size or states changed.
    Configure {
        /// The new surface size, in px.
        size: (u32, u32),
        /// The new `xdg_toplevel` states.
        states: SurfaceStates,
    },
}

/// Everything a controller may reach while handling an event.
pub struct EventCx<'a, Msg> {
    /// The controller's own retained node.
    pub node: &'a Node,
    /// This frame's handler set — the controller's only way to emit.
    pub handlers: &'a Handlers<Msg>,
    /// Allocations, for hit-tests inside the widget.
    pub tree: &'a LayoutTree,
    /// Computed styles, keyed by [`crate::view::NodeAddr`].
    pub styles: &'a StyleMap,
    /// The window's focus owner.
    pub focus: &'a mut FocusRing,
    /// Copy/paste and the primary selection.
    pub clipboard: &'a mut Clipboard,
    /// Icon lookup.
    pub icons: &'a mut IconTheme,
    /// Font matching and shaping.
    pub fonts: &'a mut FontDatabase,
    /// The animation clock.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
    /// Side effects requested without going through the model.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
    /// Which dispatch leg is running (deviation D5).
    pub phase: Phase,
    /// Set to `true` to stop this phase.
    pub handled: bool,
}

impl<Msg> EventCx<'_, Msg> {
    /// A [`BuildCx`] over the same resources, for a controller that has to
    /// rebuild a subnode while handling an event.
    pub fn build_cx<'b>(&'b mut self, sheet: &'b CompiledSheet) -> BuildCx<'b> {
        BuildCx {
            sheet,
            fonts: self.fonts,
            icons: self.icons,
            clock: self.clock,
            env: self.env,
        }
    }
}

/// Behaviour, and the state the model should not own: press state, entry
/// cursor / selection / undo stack, scroll offset, expander progress,
/// dropdown open state, spin repeat timer.
pub trait Controller<Msg>: 'static {
    /// Which widget this controller implements.
    fn kind(&self) -> Kind;

    /// Build the kind's subnodes under `node` and seed state from `props`.
    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self
    where
        Self: Sized;

    /// One prop changed. Must update `node` — classes, pseudo-states,
    /// subnode text. This is the only place a controller touches the
    /// retained tree outside [`Controller::on_event`] and
    /// [`Controller::tick`].
    ///
    /// `value` is [`Prop::None`] when the property was removed.
    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>);

    /// The whole behaviour. Returns the messages this event produced, in
    /// order.
    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg>;

    /// Clock-driven behaviour: spin repeat, kinetic scroll, expander
    /// animation, spinner rotation.
    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let _ = (now, cx);
        Vec::new()
    }

    /// Lower bound on the next [`Controller::tick`] that would do something.
    /// `Duration::ZERO` is a legitimate "now", never "spin".
    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let _ = now;
        None
    }

    /// Intrinsic size for a leaf the layout tree cannot measure itself
    /// (text, icon, drawing area). `None` lets taffy measure the box.
    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let _ = (available, cx);
        None
    }

    /// Extra painting inside the node's own effect layer, after M2's box
    /// painting and before children. `true` if anything was drawn.
    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let _ = (canvas, alloc, style, cx);
        false
    }
}

/// The controller every [`Kind`] gets until P5 or P6 writes a specific one.
///
/// It is not a placeholder: it implements the behaviour every GTK widget
/// shares — the universal props, `:hover`/`:active`/`:focus`/`:disabled`/
/// `:checked` bookkeeping, click and activate, and single-line text for the
/// kinds that carry a `Label`/`Text` prop. P5/P6 replace it kind by kind in
/// [`build_controller`]; anything they do not replace still renders and still
/// responds.
pub struct GenericC {
    kind: Kind,
    text: Option<String>,
    style: Option<crate::text::TextStyle>,
    shaped: Option<Rc<crate::text::ShapedText>>,
    pressed: bool,
    hovered: bool,
}

impl std::fmt::Debug for GenericC {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericC")
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("pressed", &self.pressed)
            .field("hovered", &self.hovered)
            .finish_non_exhaustive()
    }
}

impl GenericC {
    /// The label or text this controller renders, if any.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Whether a button is currently held on this node.
    #[must_use]
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// Whether the pointer is over this node.
    #[must_use]
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Reshape after the text or the style changed.
    fn reshape(&mut self, node: &Node, cx: &mut BuildCx<'_>) {
        let Some(text) = self.text.as_deref().filter(|t| !t.is_empty()) else {
            self.shaped = None;
            return;
        };
        let mut match_cx = crate::css::select::MatchCx::new();
        let computed = ComputedStyle::resolve_chain(cx.sheet, node, cx.env, &mut match_cx);
        let style = crate::text::TextStyle::from_computed(&computed);
        let Some(face) = cx.fonts.match_face(&style.query()) else {
            // No usable typeface: a stripped container, not an error.
            self.shaped = None;
            self.style = Some(style);
            return;
        };
        self.shaped = Some(cx.fonts.shape(&style.shape_key(text, &face)));
        self.style = Some(style);
    }

    /// Write one universal prop onto the node.
    fn apply_universal(node: &Node, kind: Kind, name: PropName, value: &Prop) -> bool {
        match name {
            PropName::Classes => {
                let mut list: Vec<&str> = kind.base_classes().to_vec();
                if let Prop::Classes(extra) = value {
                    list.extend(extra.iter().map(|c| &**c));
                }
                node.set_classes(&list);
                true
            }
            PropName::Id => {
                node.set_id(match value {
                    Prop::Str(s) => Some(s),
                    _ => None,
                });
                true
            }
            PropName::Sensitive => {
                // Absent means GTK's default, which is sensitive.
                let sensitive = matches!(value, Prop::Bool(true) | Prop::None);
                node.set_state(crate::css::node::PseudoStates::DISABLED, !sensitive);
                true
            }
            PropName::Checked => {
                node.set_state(
                    crate::css::node::PseudoStates::CHECKED,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            PropName::Indeterminate => {
                node.set_state(
                    crate::css::node::PseudoStates::INDETERMINATE,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            PropName::Selected => {
                node.set_state(
                    crate::css::node::PseudoStates::SELECTED,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            _ => false,
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for GenericC {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        // The kind is not in `props`; `build_controller` sets it right after.
        // `Kind::Box` is the neutral placeholder for the two lines between.
        let mut me = GenericC {
            kind: Kind::Box,
            text: props
                .str(PropName::Label)
                .or_else(|| props.str(PropName::Text))
                .map(str::to_owned),
            style: None,
            shaped: None,
            pressed: false,
            hovered: false,
        };
        me.reshape(node, cx);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if GenericC::apply_universal(node, self.kind, name, value) {
            return;
        }
        if matches!(name, PropName::Label | PropName::Text) {
            self.text = match value {
                Prop::Str(s) => Some((**s).to_owned()),
                _ => None,
            };
            self.reshape(node, cx);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use crate::css::node::PseudoStates;
        use crate::view::EventKind;

        // Capture never acts here: a generic controller has no reason to
        // intercept an event on its way down (deviation D5).
        if cx.phase == Phase::Capture {
            return Vec::new();
        }

        match ev {
            Event::PointerEnter { .. } => {
                self.hovered = true;
                cx.node.set_state(PseudoStates::HOVER, true);
                Vec::new()
            }
            Event::PointerLeave => {
                self.hovered = false;
                cx.node.set_state(PseudoStates::HOVER, false);
                // GTK drops `:active` when the pointer leaves while held and
                // re-arms it on re-entry; `pressed` stays true so the
                // implicit grab can still complete the click if it comes back.
                cx.node.set_state(PseudoStates::ACTIVE, false);
                Vec::new()
            }
            Event::PointerMotion { .. } => {
                if self.pressed && self.hovered {
                    cx.node.set_state(PseudoStates::ACTIVE, true);
                }
                Vec::new()
            }
            Event::PointerDown { button, .. } if *button == crate::window::layer::BTN_LEFT => {
                self.pressed = true;
                self.hovered = true;
                cx.node.set_state(PseudoStates::ACTIVE, true);
                cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
                cx.handled = true;
                Vec::new()
            }
            Event::PointerUp { button, .. } if *button == crate::window::layer::BTN_LEFT => {
                let was_pressed = std::mem::replace(&mut self.pressed, false);
                cx.node.set_state(PseudoStates::ACTIVE, false);
                if !was_pressed || !self.hovered {
                    return Vec::new();
                }
                cx.handled = true;
                let mut out = Vec::new();
                out.extend(cx.handlers.fire_unit(EventKind::Click));
                out.extend(cx.handlers.fire_unit(EventKind::Activate));
                out
            }
            Event::Activate => {
                let mut out = Vec::new();
                out.extend(cx.handlers.fire_unit(EventKind::Activate));
                out.extend(cx.handlers.fire_unit(EventKind::Click));
                if !out.is_empty() {
                    cx.handled = true;
                }
                out
            }
            Event::FocusIn { .. } => cx
                .handlers
                .fire_unit(EventKind::FocusIn)
                .into_iter()
                .collect(),
            Event::FocusOut => cx
                .handlers
                .fire_unit(EventKind::FocusOut)
                .into_iter()
                .collect(),
            Event::Key(key) if key.pressed => cx
                .handlers
                .fire_key(EventKind::KeyPressed, key)
                .into_iter()
                .inspect(|_| cx.handled = true)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let shaped = self.shaped.as_ref()?;
        let style = self.style.as_ref()?;
        let width = shaped.metrics.width;
        let height = style.line_height_px(&shaped.metrics);
        Some((
            available.0.map_or(width, |cap| width.min(cap)),
            available.1.map_or(height, |cap| height.min(cap)),
        ))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(shaped) = self.shaped.as_ref() else {
            return false;
        };
        crate::paint::text::paint_text(
            canvas,
            shaped,
            alloc.content_box,
            style,
            &cx.base_length_ctx(),
        );
        true
    }
}

/// Build the controller for `kind`.
///
/// This single `match` is the seam P5 and P6 extend: each arm they add
/// replaces a [`GenericC`] with the kind's real controller, and nothing else
/// in the framework changes.
#[must_use]
pub fn build_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    // Identity first, so a `Classes` prop lands on top of the base classes.
    node.set_classes(kind.base_classes());
    let mut generic = <GenericC as Controller<Msg>>::build(node, props, cx);
    generic.kind = kind;

    let mut boxed: Box<dyn Controller<Msg>> = Box::new(generic);
    for (name, value) in props.iter() {
        boxed.set_prop(node, name, value, cx);
    }
    boxed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::node::PseudoStates;
    use crate::view::{Kind, Prop, PropName, Props};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Pressed,
    }

    /// The smallest possible controller: it proves the trait is object
    /// safe, that every defaulted method has a working default, and that a
    /// `Box<dyn Controller<Msg>>` can be driven without knowing its type.
    #[derive(Debug, Default)]
    struct Counting {
        events: usize,
        label: Option<String>,
    }

    impl<M: Clone + 'static> Controller<M> for Counting {
        fn kind(&self) -> Kind {
            Kind::Button
        }

        fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
            Counting {
                events: 0,
                label: props.str(PropName::Label).map(str::to_owned),
            }
        }

        fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
            if name == PropName::Label {
                self.label = match value {
                    Prop::Str(s) => Some((**s).to_owned()),
                    _ => None,
                };
            }
        }

        fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, M>) -> Vec<M> {
            self.events += 1;
            Vec::new()
        }
    }

    #[test]
    fn the_trait_is_object_safe_and_its_defaults_are_inert() {
        let node = Node::new("button");
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));

        let sheet = CompiledSheet::compile("button { color: #000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };

        let mut boxed: Box<dyn Controller<Msg>> =
            Box::new(<Counting as Controller<Msg>>::build(&node, &props, &mut cx));
        assert_eq!(boxed.kind(), Kind::Button);
        assert_eq!(boxed.next_deadline(Duration::ZERO), None);
        assert_eq!(boxed.measure((None, None), &mut cx), None);
        boxed.set_prop(&node, PropName::Label, &Prop::Str("Cancel".into()), &mut cx);
    }

    #[test]
    fn phases_are_ordered_capture_then_target_then_bubble() {
        assert_eq!(Phase::ALL, [Phase::Capture, Phase::Target, Phase::Bubble]);
        assert!(Phase::Capture < Phase::Target);
        assert!(Phase::Target < Phase::Bubble);
    }

    #[test]
    fn the_offscreen_clipboard_round_trips_without_a_compositor() {
        // Deviation D10: `run_offscreen` has no Wayland connection, and
        // every `EventCx` needs a `&mut Clipboard`.
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        assert!(!clipboard.has_selection());
        assert_eq!(clipboard.paste(Duration::from_millis(1)), None);
        clipboard.copy("hello", 1);
        assert!(clipboard.has_selection());
        assert_eq!(
            clipboard.paste(Duration::from_millis(1)).as_deref(),
            Some("hello")
        );
        assert!(!clipboard.has_primary());
        clipboard.set_primary("mid", 2);
        assert!(clipboard.has_primary());
        assert_eq!(
            clipboard.primary(Duration::from_millis(1)).as_deref(),
            Some("mid")
        );
    }

    #[test]
    fn ops_compare_by_value_so_a_test_can_pin_a_whole_op_list() {
        use crate::view::reconcile::Op;
        assert_eq!(Op::Insert { index: 0 }, Op::Insert { index: 0 });
        assert_ne!(Op::Insert { index: 0 }, Op::Remove { index: 0 });
        assert_ne!(
            Op::SetProp {
                index: 1,
                name: PropName::Label
            },
            Op::SetProp {
                index: 1,
                name: PropName::Text
            }
        );
        assert_eq!(Op::Move { from: 2, to: 0 }, Op::Move { from: 2, to: 0 });
    }

    fn build_cx_fixture() -> (
        CompiledSheet,
        crate::text::FontDatabase,
        crate::icons::IconTheme,
        Rc<dyn Clock>,
        ResolveEnv,
    ) {
        (
            CompiledSheet::compile("button { color: #000000; } button:hover { color: #ff0000; }"),
            crate::text::FontDatabase::probe_only(),
            crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]),
            Rc::new(crate::anim::ManualClock::new()),
            ResolveEnv::default(),
        )
    }

    #[test]
    fn build_controller_applies_the_kind_s_identity_and_the_universal_props() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let node = Node::new(Kind::ToggleButton.css_name());
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));
        props.set(PropName::Sensitive, Prop::Bool(false));
        props.set(
            PropName::Classes,
            Prop::Classes(std::rc::Rc::from([std::rc::Rc::from("flat")])),
        );
        props.set(PropName::Id, Prop::Str("go".into()));

        let controller: Box<dyn Controller<Msg>> =
            build_controller(Kind::ToggleButton, &node, &props, &mut cx);
        assert_eq!(controller.kind(), Kind::ToggleButton);

        let classes: Vec<String> = node
            .classes()
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect();
        assert_eq!(classes, vec!["toggle".to_owned(), "flat".to_owned()]);
        assert_eq!(
            node.id().map(|i| i.as_str().to_owned()),
            Some("go".to_owned())
        );
        assert!(node.states().contains(PseudoStates::DISABLED));
    }

    #[test]
    fn set_prop_moves_the_node_state_and_removing_a_prop_restores_the_default() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let node = Node::new("button");
        let mut controller: Box<dyn Controller<Msg>> =
            build_controller(Kind::Button, &node, &Props::default(), &mut cx);

        controller.set_prop(&node, PropName::Sensitive, &Prop::Bool(false), &mut cx);
        assert!(node.states().contains(PseudoStates::DISABLED));
        // A removed prop is `Prop::None` and must restore GTK's default.
        controller.set_prop(&node, PropName::Sensitive, &Prop::None, &mut cx);
        assert!(!node.states().contains(PseudoStates::DISABLED));

        controller.set_prop(&node, PropName::Checked, &Prop::Bool(true), &mut cx);
        assert!(node.states().contains(PseudoStates::CHECKED));
    }

    #[test]
    fn a_press_and_release_inside_fires_click_then_activate_once() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let node = Node::new("button");
        let mut handlers: Handlers<Msg> = Handlers::default();
        handlers.set(
            crate::view::EventKind::Click,
            crate::view::Handler::Unit(Msg::Pressed),
        );

        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            build_controller::<Msg>(Kind::Button, &node, &Props::default(), &mut cx)
        };

        let tree = LayoutTree::new();
        let styles = StyleMap::new();
        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();

        let mut ecx = EventCx {
            node: &node,
            handlers: &handlers,
            tree: &tree,
            styles: &styles,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
            phase: Phase::Target,
            handled: false,
        };

        let down = controller.on_event(
            &Event::PointerDown {
                button: 0x110,
                local: (2.0, 2.0),
                serial: 1,
            },
            &mut ecx,
        );
        assert!(down.is_empty(), "a press alone is not a click");
        assert!(node.states().contains(PseudoStates::ACTIVE));

        let up = controller.on_event(
            &Event::PointerUp {
                button: 0x110,
                local: (2.0, 2.0),
                serial: 2,
            },
            &mut ecx,
        );
        assert_eq!(up, vec![Msg::Pressed]);
        assert!(!node.states().contains(PseudoStates::ACTIVE));

        // A second release with no press in between fires nothing.
        let again = controller.on_event(
            &Event::PointerUp {
                button: 0x110,
                local: (2.0, 2.0),
                serial: 3,
            },
            &mut ecx,
        );
        assert!(again.is_empty());
    }

    #[test]
    fn a_press_released_outside_the_node_does_not_click() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let node = Node::new("button");
        let mut handlers: Handlers<Msg> = Handlers::default();
        handlers.set(
            crate::view::EventKind::Click,
            crate::view::Handler::Unit(Msg::Pressed),
        );
        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            build_controller::<Msg>(Kind::Button, &node, &Props::default(), &mut cx)
        };
        let tree = LayoutTree::new();
        let styles = StyleMap::new();
        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut ecx = EventCx {
            node: &node,
            handlers: &handlers,
            tree: &tree,
            styles: &styles,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
            phase: Phase::Target,
            handled: false,
        };

        controller.on_event(
            &Event::PointerDown {
                button: 0x110,
                local: (2.0, 2.0),
                serial: 1,
            },
            &mut ecx,
        );
        controller.on_event(&Event::PointerLeave, &mut ecx);
        // GTK keeps `:active` off while the pointer is away but the button
        // is still held; the release outside is not a click.
        let up = controller.on_event(
            &Event::PointerUp {
                button: 0x110,
                local: (-5.0, -5.0),
                serial: 2,
            },
            &mut ecx,
        );
        assert!(up.is_empty(), "releasing outside the node clicked anyway");
    }

    #[test]
    fn labelled_kinds_measure_their_text_and_unlabelled_ones_do_not() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let node = Node::new("label");
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Hello".into()));
        let mut labelled = build_controller::<Msg>(Kind::Label, &node, &props, &mut cx);
        let measured = labelled.measure((None, None), &mut cx);
        // A machine with no usable face measures nothing; that is not a
        // failure, it is `FontDatabase::match_face` returning `None`.
        if let Some((w, h)) = measured {
            assert!(w > 0.0 && h > 0.0, "a shaped label measured {w}x{h}");
        }

        let empty = Node::new("box");
        let mut plain = build_controller::<Msg>(Kind::Box, &empty, &Props::default(), &mut cx);
        assert_eq!(plain.measure((None, None), &mut cx), None);
    }
}
