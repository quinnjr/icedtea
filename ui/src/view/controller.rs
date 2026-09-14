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
use crate::view::{EventKind, Handlers, Kind, Prop, PropName, Props};
use crate::window::SurfaceStates;
use crate::window::focus::{FOCUSABLE_CLASS, FocusCause, FocusRing};
use crate::window::keyboard::KeyEvent;
use crate::window::pointer::Scroll;
use crate::window::selection::Clipboard;

#[doc(inline)]
pub use crate::paint::PaintCx;

/// Which leg of GTK's three-phase dispatch is running.
///
/// Contract deviation D5: §4.6 requires "capture → target → bubble" and says
/// a controller that sets `cx.handled = true` "stops the phase it is in" —
/// which a controller cannot honour without seeing the phase. What `handled`
/// actually stops is the whole dispatch (deviation D19, on
/// [`crate::view::app::deliver`]).
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
    /// An IME `preedit_string` landed for the focused node: display-only
    /// composing text, never written to the buffer.
    ImePreedit {
        /// The composing text.
        text: String,
        /// Byte offset of the cursor within `text`.
        cursor_begin: i32,
        /// Selection end within `text`.
        cursor_end: i32,
    },
    /// An IME `commit_string` landed for the focused node.
    ImeCommit(String),
    /// An IME `delete_surrounding_text` landed for the focused node, as
    /// UTF-8 byte lengths around the cursor.
    ImeDelete {
        /// Bytes to delete before the cursor.
        before: u32,
        /// Bytes to delete after the cursor.
        after: u32,
    },
    /// Space/Enter, or a click that completed inside — the "activate" GTK
    /// means.
    Activate,
    /// A press-move past the drag threshold began a drag on this node.
    /// The controller must disarm its click latch: the completing release
    /// is a drop/cancel, never a click.
    DragStart,
    /// An in-flight drag entered this drop target.
    DragEnter,
    /// An in-flight drag moved within this drop target.
    DragMotion {
        /// Point in the node's border-box space.
        local: (f32, f32),
    },
    /// An in-flight drag left this drop target without dropping.
    DragLeave,
    /// An in-flight drag was released over this drop target, carrying the
    /// whole offered payload — the controller negotiates against what it
    /// accepts and fires `Drop` with the winner.
    Drop {
        /// Everything the source offered, in preference order.
        payload: crate::dnd::DragPayload,
    },
    /// A drag this node started has ended.
    DragEnd {
        /// Whether it ended in a drop (`true`) or a cancel (`false`).
        dropped: bool,
    },
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
    /// Set to `true` to consume the event: dispatch ends here and no further
    /// node or phase sees it (deviation D19, on
    /// [`crate::view::app::deliver`]).
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
// `Any` as a supertrait (rather than a defaulted `as_any` method) is what
// lets [`crate::widgets::child_slot`] downcast a `&dyn Controller<Msg>` to a
// concrete controller like `PopoverC`: trait-object upcasting coerces
// `&dyn Controller<Msg>` straight to `&dyn Any` at the call site, so no
// per-implementor boilerplate is needed.
pub trait Controller<Msg>: std::any::Any {
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

    /// The `text/plain` payload this node offers when a press here arms a
    /// drag, or `None` when this node is not a drag source. The default is
    /// `None`: existing widgets are never sources until they opt in.
    fn drag_offer(&self) -> Option<crate::dnd::DragPayload> {
        None
    }

    /// Whether this node accepts a drop of anything in `offered` (MIME
    /// types in source preference order). The default is `false`: existing
    /// widgets are never targets until they opt in.
    fn drop_accepts(&self, offered: &[&str]) -> bool {
        let _ = offered;
        false
    }

    /// The framework is about to deliver [`Event::DragStart`] to this node.
    /// A controller with a press latch clears it here so the completing
    /// release is a drop/cancel, never a click. The default is a no-op: a
    /// controller with no press gesture owes nothing.
    fn cancel_press(&mut self, node: &Node) {
        let _ = node;
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

    /// Where the view's child at `view_index` (0-based, counting only this
    /// level's visible reconciled children) belongs among this
    /// controller's own node's actual children, for a controller that
    /// attaches chrome subnodes of its own directly onto that same node
    /// alongside the view's children -- a [`Frame`](crate::widgets::frame)'s
    /// label, a [`Paned`](crate::widgets::paned)'s separator. Reconcile
    /// (`view::reconcile::reconcile`) calls this to place and to keep
    /// track of each reused or newly-built child, instead of assuming the
    /// node's children are exactly the view's children in order.
    ///
    /// The default is the identity: a controller with no such interleaved
    /// chrome.
    fn child_index(&self, view_index: usize) -> usize {
        view_index
    }

    /// The total number of node children expected once `view_count` view
    /// children have been reconciled at this level, chrome subnodes
    /// included. Reconcile's trim step -- the one that detaches whatever a
    /// previous frame left past the current child count -- never detaches
    /// a node before this bound, so a controller that reserves chrome the
    /// last [`Controller::child_index`] call can't see (there being no
    /// view child to index chrome placed *after*, e.g. with zero view
    /// children) must say so here.
    ///
    /// The default is `view_count`: no reserved chrome.
    fn reserved_total(&self, view_count: usize) -> usize {
        view_count
    }
}

/// The drag-and-drop bookkeeping every controller shares once it opts in.
///
/// v1 wired this state directly into [`GenericC`] only (deviation M6-D2), so
/// a dedicated controller (`ButtonC`, `EntryC`, …) answered `DragSource` /
/// `DropAccept` with the trait defaults and could be neither a source nor a
/// target. Lifting the three fields and the six `Event` arms here lets a
/// dedicated controller opt in with one field plus a two-line delegation in
/// `set_prop`/`on_event`, while `cancel_press` clears whatever press latch
/// that controller owns.
///
/// **Adoption recipe** — a dedicated controller becomes a drag source/target
/// with exactly these edits; the first five are mechanical, the last is a
/// convention the trait cannot enforce:
///
/// 1. add a `dnd: DndState` field, initialised `DndState::default()`;
/// 2. `fn drag_offer(&self) { self.dnd.offer() }`;
/// 3. `fn drop_accepts(&self, offered: &[&str]) { self.dnd.accepts(offered) }`;
/// 4. early-return from `set_prop` on `self.dnd.set_prop(node, name, value)`;
/// 5. early-return from `on_event` on `self.dnd.on_event(ev, cx)`;
/// 6. override `cancel_press` to clear the controller's own press latch and
///    its `:active` state — the framework calls it just before `DragStart`,
///    and a controller that skips it would fire a click from the drag's
///    completing release.
///
/// Step 6 is convention-only because the default is a no-op and the trait has
/// no way to know which field is the latch; every opt-in controller in this
/// crate (see `ButtonC`/`LabelC`/`EntryC`/`ListBoxC`) overrides it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DndState {
    /// The `DragSource` prop's payload text. `Some` makes this node a source.
    drag_text: Option<String>,
    /// The `DropAccept` prop's target state, plus the live hover flag.
    target: Option<crate::dnd::DropTarget>,
    /// Set by `DragStart`, cleared by `DragEnd`. While set the completing
    /// release is a drop/cancel (never a click) and a second press is ignored.
    dragging: bool,
}

impl DndState {
    /// Write one M6 drag-and-drop prop onto the controller. Returns whether
    /// `name` was a DnD prop and the caller need do nothing more.
    pub fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop) -> bool {
        match name {
            PropName::DragSource => {
                self.drag_text = match value {
                    Prop::Str(s) => Some((**s).to_owned()),
                    _ => None,
                };
                true
            }
            PropName::DropAccept => {
                self.target = match value {
                    Prop::Bool(true) => Some(crate::dnd::DropTarget::accept_plain()),
                    Prop::Str(mimes) => Some(crate::dnd::DropTarget::accept_list(mimes)),
                    _ => None,
                };
                // A fresh target starts unhighlighted; resync so removing the
                // prop mid-hover also drops the class.
                self.sync_highlight(node);
                true
            }
            _ => false,
        }
    }

    /// The payload this node offers when a press here arms a drag.
    #[must_use]
    pub fn offer(&self) -> Option<crate::dnd::DragPayload> {
        self.drag_text
            .as_deref()
            .map(crate::dnd::DragPayload::offer_text)
    }

    /// Whether this node accepts a drop of anything in `offered`.
    #[must_use]
    pub fn accepts(&self, offered: &[&str]) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| target.accepts(offered))
    }

    /// Whether a drag started on this node is in flight.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Add or remove [`crate::dnd::DROP_ACTIVE_CLASS`] to match the current
    /// hover state. Idempotent, so a controller that rewrites its whole class
    /// list (as [`GenericC`] does) can call it again without duplicating.
    pub fn sync_highlight(&self, node: &Node) {
        if self
            .target
            .as_ref()
            .is_some_and(crate::dnd::DropTarget::is_highlighted)
        {
            node.add_class(crate::dnd::DROP_ACTIVE_CLASS);
        } else {
            node.remove_class(crate::dnd::DROP_ACTIVE_CLASS);
        }
    }

    /// Handle one of the six drag events. `None` when `ev` is not a drag
    /// event this node opted into, so the caller can carry on; `Some(msgs)`
    /// when it was handled (and `cx.handled` set).
    pub fn on_event<Msg: Clone + 'static>(
        &mut self,
        ev: &Event,
        cx: &mut EventCx<'_, Msg>,
    ) -> Option<Vec<Msg>> {
        match ev {
            Event::DragStart => {
                self.drag_text.as_ref()?;
                // The latch clear itself belongs to the controller's own
                // press state (`Controller::cancel_press`, called by
                // `deliver`); this only records that a drag is in flight.
                self.dragging = true;
                cx.handled = true;
                Some(
                    cx.handlers
                        .fire_unit(EventKind::DragStart)
                        .into_iter()
                        .collect(),
                )
            }
            Event::DragEnter => {
                self.target.as_ref()?;
                if let Some(target) = self.target.as_mut() {
                    target.set_highlighted(true);
                }
                self.sync_highlight(cx.node);
                cx.handled = true;
                Some(
                    cx.handlers
                        .fire_unit(EventKind::DragEnter)
                        .into_iter()
                        .collect(),
                )
            }
            Event::DragMotion { local } => {
                self.target.as_ref()?;
                cx.handled = true;
                Some(
                    cx.handlers
                        .fire_pair(
                            EventKind::DragMotion,
                            f64::from(local.0),
                            f64::from(local.1),
                        )
                        .into_iter()
                        .collect(),
                )
            }
            Event::DragLeave => {
                self.target.as_ref()?;
                if let Some(target) = self.target.as_mut() {
                    target.set_highlighted(false);
                }
                self.sync_highlight(cx.node);
                cx.handled = true;
                Some(
                    cx.handlers
                        .fire_unit(EventKind::DragLeave)
                        .into_iter()
                        .collect(),
                )
            }
            Event::Drop { payload } => {
                self.target.as_ref()?;
                if let Some(target) = self.target.as_mut() {
                    target.set_highlighted(false);
                }
                self.sync_highlight(cx.node);
                cx.handled = true;
                // Re-negotiated here so a stale route can never push bytes
                // somewhere the current props reject.
                let accepted = self
                    .target
                    .as_ref()
                    .map(crate::dnd::DropTarget::accepted_mimes)
                    .unwrap_or_default();
                let msgs = payload
                    .data_for(&accepted)
                    .and_then(|(_, data)| {
                        cx.handlers
                            .fire_text(EventKind::Drop, &String::from_utf8_lossy(data))
                    })
                    .map_or_else(Vec::new, |m| vec![m]);
                Some(msgs)
            }
            Event::DragEnd { dropped } => {
                self.drag_text.as_ref()?;
                self.dragging = false;
                cx.handled = true;
                Some(
                    cx.handlers
                        .fire_bool(EventKind::DragEnd, *dropped)
                        .into_iter()
                        .collect(),
                )
            }
            _ => None,
        }
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
    /// Whether the node carries [`crate::window::focus::FOCUSABLE_CLASS`].
    focusable: bool,
    /// The `Classes` prop's extra classes, kept so the class list can be
    /// rewritten whenever `focusable` changes.
    extra_classes: Vec<Rc<str>>,
    /// The M6 drag-and-drop state (deviation M6-D2's shared helper; dedicated
    /// controllers opt in by holding one of these too).
    dnd: DndState,
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

    /// Whether a drag started on this node is in flight.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.dnd.is_dragging()
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

    /// Rewrite the node's whole class list: the kind's base classes, then the
    /// `Classes` prop's, then [`FOCUSABLE_CLASS`] when the node takes focus,
    /// then [`crate::dnd::DROP_ACTIVE_CLASS`] while a drag hovers a target.
    ///
    /// One function writes all four because `Node::set_classes` replaces the
    /// list wholesale, so `Classes` and `Focusable` arriving in either prop
    /// order must produce the same result. The DnD highlight is re-asserted
    /// after the wholesale rewrite from [`DndState::sync_highlight`].
    fn sync_classes(&self, node: &Node) {
        let mut list: Vec<&str> = self.kind.base_classes().to_vec();
        list.extend(self.extra_classes.iter().map(|c| &**c));
        if self.focusable && !list.contains(&FOCUSABLE_CLASS) {
            list.push(FOCUSABLE_CLASS);
        }
        node.set_classes(&list);
        self.dnd.sync_highlight(node);
    }

    /// Write one universal prop onto the node.
    fn apply_universal(&mut self, node: &Node, name: PropName, value: &Prop) -> bool {
        match name {
            PropName::Classes => {
                self.extra_classes = match value {
                    Prop::Classes(extra) => extra.to_vec(),
                    _ => Vec::new(),
                };
                self.sync_classes(node);
                true
            }
            PropName::Focusable => {
                // Absent restores the kind's GTK default.
                self.focusable = match value {
                    Prop::Bool(on) => *on,
                    _ => self.kind.is_focusable_by_default(),
                };
                self.sync_classes(node);
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

    fn drag_offer(&self) -> Option<crate::dnd::DragPayload> {
        self.dnd.offer()
    }

    fn drop_accepts(&self, offered: &[&str]) -> bool {
        self.dnd.accepts(offered)
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
            // `build_controller` sets the real default once it has set `kind`.
            focusable: false,
            extra_classes: Vec::new(),
            dnd: DndState::default(),
        };
        me.reshape(node, cx);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if self.apply_universal(node, name, value) {
            return;
        }
        if self.dnd.set_prop(node, name, value) {
            self.sync_classes(node);
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

    fn cancel_press(&mut self, node: &Node) {
        self.pressed = false;
        node.set_state(crate::css::node::PseudoStates::ACTIVE, false);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use crate::css::node::PseudoStates;

        // Capture never acts here: a generic controller has no reason to
        // intercept an event on its way down (deviation D5).
        if cx.phase == Phase::Capture {
            return Vec::new();
        }

        // M6: the six drag arms live in the shared `DndState` (deviation
        // M6-D2's follow-up; the same helper a dedicated controller holds).
        if let Some(out) = self.dnd.on_event(ev, cx) {
            return out;
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
                // A second press while this node's drag is in flight is not
                // a new press: it must not arm a click the drag's release
                // would then fire.
                if self.dnd.is_dragging() {
                    return Vec::new();
                }
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
            // Drag arms live in `DndState::on_event`, above; a node that
            // never set `DragSource`/`DropAccept` lets the event bubble to an
            // ancestor that did, and the innermost opt-in node consumes it so
            // exactly one target highlights and exactly one source observes
            // each drag.
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
/// One definition, one fallback (contract §11 E1): the dispatch table lives
/// beside the widgets in [`crate::widgets::build_controller`], and this is the
/// name [`crate::view::reconcile::reconcile`]'s `Insert` arm calls. A kind no part has
/// written a controller for still lands on [`GenericC`], never on an inert
/// stub.
#[must_use]
pub fn build_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    crate::widgets::build_controller(kind, node, props, cx)
}

/// [`GenericC`], configured for `kind` — the catch-all
/// [`crate::widgets::build_controller`] falls back to.
///
/// The caller has already written `kind.base_classes()` onto `node` and is
/// responsible for the `set_prop` pass over `props`; this only builds the
/// controller and gives it its identity.
#[must_use]
pub fn generic_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    let mut generic = <GenericC as Controller<Msg>>::build(node, props, cx);
    generic.kind = kind;
    // GTK's focusability default reaches P3's focus ring as a class
    // (`window::focus::FOCUSABLE_CLASS`); without this line no reconciled node
    // is ever a Tab stop. A `PropName::Focusable` in the caller's prop loop
    // overrides it, in either prop order.
    generic.focusable = kind.is_focusable_by_default();
    generic.sync_classes(node);
    Box::new(generic)
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
        // Base classes, then P3's focus-ring marker (`ToggleButtonC`'s
        // `Universal::new` adds it at build time, ahead of the props loop —
        // unlike `GenericC`, which rewrites the whole class list on every
        // change and so always reports base/extra/focusable in that order),
        // then the `Classes` prop's.
        assert_eq!(
            classes,
            vec![
                "toggle".to_owned(),
                FOCUSABLE_CLASS.to_owned(),
                "flat".to_owned(),
            ]
        );
        assert_eq!(
            node.id().map(|i| i.as_str().to_owned()),
            Some("go".to_owned())
        );
        assert!(node.states().contains(PseudoStates::DISABLED));
    }

    /// P3's focus ring only walks nodes carrying `FOCUSABLE_CLASS`, so
    /// without this every reconciled tree has zero Tab stops.
    #[test]
    fn focusability_reaches_the_focus_ring_as_a_class_in_either_prop_order() {
        let (sheet, mut fonts, mut icons, clock, env) = build_cx_fixture();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let has = |node: &Node| node.classes().iter().any(|c| c.as_str() == FOCUSABLE_CLASS);

        // The kind's GTK default, with no prop at all.
        let button = Node::new("button");
        let mut c: Box<dyn Controller<Msg>> =
            build_controller(Kind::Button, &button, &Props::default(), &mut cx);
        assert!(has(&button));
        let label = Node::new("label");
        let _: Box<dyn Controller<Msg>> =
            build_controller(Kind::Label, &label, &Props::default(), &mut cx);
        assert!(!has(&label));

        // `View::focusable(false)` overrides it, and `Prop::None` (a removed
        // prop) restores the default.
        c.set_prop(&button, PropName::Focusable, &Prop::Bool(false), &mut cx);
        assert!(!has(&button));
        c.set_prop(&button, PropName::Focusable, &Prop::None, &mut cx);
        assert!(has(&button));

        // A later `Classes` prop must not wipe the marker, and an earlier one
        // must not be wiped by it: `set_classes` replaces the whole list.
        c.set_prop(
            &button,
            PropName::Classes,
            &Prop::Classes(std::rc::Rc::from([std::rc::Rc::from("flat")])),
            &mut cx,
        );
        assert!(has(&button));
        let classes: Vec<String> = button
            .classes()
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect();
        // `ButtonC` (unlike `GenericC`) applies `Universal` incrementally, so
        // the marker already on the node stays ahead of a class a later
        // `Classes` prop appends.
        assert_eq!(classes, vec![FOCUSABLE_CLASS.to_owned(), "flat".to_owned()]);

        // A non-focusable kind given `focusable(true)` joins the ring.
        c.set_prop(&button, PropName::Focusable, &Prop::Bool(false), &mut cx);
        assert!(!has(&button));
        let mut lc: Box<dyn Controller<Msg>> =
            build_controller(Kind::Label, &label, &Props::default(), &mut cx);
        lc.set_prop(&label, PropName::Focusable, &Prop::Bool(true), &mut cx);
        assert!(has(&label));
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
        // `Kind::MenuButton` (still `GenericC`, unlike `Kind::Button` since
        // Task 19): this exercises GenericC's own hover-flag click tracking,
        // which — unlike `PointerState`'s allocation-based one P5/P6 widgets
        // share — has no allocation to consult in this test's bare
        // `LayoutTree::new()`.
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
            build_controller::<Msg>(Kind::MenuButton, &node, &Props::default(), &mut cx)
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
