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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Kind, Prop, PropName, Props};

    #[derive(Debug, Clone, PartialEq)]
    #[expect(
        dead_code,
        reason = "the message type is only ever a type parameter here; \
                  Controller::on_event's default behaviour emits nothing"
    )]
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
}
