//! The loop: events in, messages folded, a new view reconciled, the dirty
//! parts restyled, relaid out and repainted.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use crate::anim::{Clock, ManualClock};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ResolveEnv;
use crate::css::node::Node;
use crate::icons::IconTheme;
use crate::layout::{Container, LayoutError, LayoutTree, Measure};
use crate::paint::{ImageCache, PaintCx};
use crate::text::FontDatabase;
use crate::view::cmd::Cmd;
use crate::view::controller::{Event, EventCx, Phase};
use crate::view::reconcile::{BuildCx, Instance, containers_of, reconcile};
use crate::view::render::{
    Animations, NodeAddr, NodePainter, StyleMap, layout_tree, paint_tree, restyle_tree,
};
use crate::view::{EventKind, Handlers, Kind, View};
use crate::window::focus::{
    Binding, FocusCause, FocusDirection, FocusRing, navigate, window_binding,
};
use crate::window::pointer::{ImplicitGrab, hit_chain};
use crate::window::popup::{PopupAnchorPoint, PopupKey, Positioner};
use crate::window::selection::Clipboard;
use crate::window::{InputEvent, SurfaceError, SurfaceTarget};

/// The borrows one dispatch pass needs, minus the per-node ones.
pub struct Dispatch<'a, Msg> {
    /// Computed styles, for a controller that hit-tests inside itself.
    pub styles: &'a StyleMap,
    /// Allocations.
    pub tree: &'a LayoutTree,
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
    /// Side effects controllers requested.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
}

/// The chain of nodes from the outermost instance down to `target`,
/// outermost first — the same order `window::pointer::hit_chain` returns.
/// Empty when `target` is not in the tree.
#[must_use]
pub fn path_to<Msg: Clone + 'static>(roots: &[Instance<Msg>], target: &Node) -> Vec<Node> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        target: &Node,
        out: &mut Vec<Node>,
    ) -> bool {
        out.push(instance.node.clone());
        if instance.node.ptr_eq(target) {
            return true;
        }
        for child in &instance.children {
            if walk(child, target, out) {
                return true;
            }
        }
        out.pop();
        false
    }

    let mut out = Vec::new();
    for root in roots {
        if walk(root, target, &mut out) {
            return out;
        }
        out.clear();
    }
    Vec::new()
}

/// Deliver `event` along `path` in GTK's three phases: capture from the
/// outermost node inward, then the target, then bubble back out.
///
/// A controller that sets `cx.handled` ends **the whole dispatch**, not just
/// the phase it is in: the node that set it is the last one this event
/// reaches. A handled capture therefore suppresses both target and bubble and
/// the target never sees the event; a handled target suppresses bubble; a
/// handled bubble stops climbing. Messages come back in the order they were
/// produced.
///
/// This is deviation **D19**, which supersedes the contract §4.6 wording D5
/// quotes ("stops the phase it is in"). Per-phase stopping is unimplementable
/// as written: the phases run outermost-in then innermost-out over the same
/// path, so "capture stops descending" and "the event still reaches the
/// target" contradict each other — a capture that stopped only its own phase
/// would hand the event to the very node it just intercepted. `cx.phase` (D5)
/// still tells a controller which phase it is in; what changed is the reach of
/// `handled`.
/// M5-D5's three pointer kinds, fired from one place (P0-D1).
///
/// The contract's text puts this in `GenericC::on_event` plus a forwarding arm
/// per widget. `GenericC` is not on the dispatch path of a widget that has its
/// own controller — `ButtonC`, `DrawingAreaC` and thirty others replace it —
/// and P5 puts `on_pointer_up_with_button` on `button(..)` nodes while being
/// forbidden from touching `ui/`. So the firing lives in `deliver`, once, for
/// every kind alike, and no widget file is edited to opt in.
///
/// `button` is [`BTN_NONE`](crate::window::pointer::BTN_NONE) for a motion,
/// which carries none (P0-D2).
fn fire_pointer_handlers<Msg: Clone + 'static>(
    event: &Event,
    handlers: &Handlers<Msg>,
) -> Vec<Msg> {
    let (kind, local, button) = match event {
        Event::PointerDown { local, button, .. } => (EventKind::PointerDown, *local, *button),
        Event::PointerMotion { local } => (
            EventKind::PointerMotion,
            *local,
            crate::window::pointer::BTN_NONE,
        ),
        Event::PointerUp { local, button, .. } => (EventKind::PointerUp, *local, *button),
        _ => return Vec::new(),
    };
    handlers
        .fire_pair_button(kind, f64::from(local.0), f64::from(local.1), button)
        .into_iter()
        .collect()
}

/// The point `event` carries, in the node's own border-box space, if it
/// carries one at all.
fn event_local(event: &Event) -> Option<(f32, f32)> {
    match event {
        Event::PointerEnter { local } | Event::PointerMotion { local } => Some(*local),
        Event::PointerDown { local, .. } | Event::PointerUp { local, .. } => Some(*local),
        _ => None,
    }
}

/// `event` with its point rewritten into another node's border-box space.
fn with_local(event: &Event, local: (f32, f32)) -> Event {
    match event {
        Event::PointerEnter { .. } => Event::PointerEnter { local },
        Event::PointerMotion { .. } => Event::PointerMotion { local },
        Event::PointerDown { button, serial, .. } => Event::PointerDown {
            button: *button,
            local,
            serial: *serial,
        },
        Event::PointerUp { button, serial, .. } => Event::PointerUp {
            button: *button,
            local,
            serial: *serial,
        },
        other => other.clone(),
    }
}

pub fn deliver<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    path: &[Node],
    event: &Event,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let last = path.len() - 1;
    // P0-D8's bubble pass, and the capture pass before it, hand the *same*
    // `Event` to every node on the path — and `local` is relative to the node
    // the event was aimed at (`window::pointer::descend`). An ancestor
    // therefore read a child's coordinate frame: a release five pixels past a
    // button's right edge still fell inside the button's own `0..width` bounds
    // check, and one in its left padding arrived as a negative x. The window
    // point is recovered once, from the aimed node's own origin, and each node
    // on the path is handed the point in *its* frame.
    let tree = cx.tree;
    let origin_of = |node: &Node| {
        tree.allocation(node)
            .map(|alloc| (alloc.border_box.x, alloc.border_box.y))
    };
    let window_point = event_local(event)
        .zip(origin_of(&path[last]))
        .map(|(local, origin)| (local.0 + origin.0, local.1 + origin.1));

    'phases: for phase in Phase::ALL {
        let order: Vec<usize> = match phase {
            Phase::Capture => (0..last).collect(),
            Phase::Target => vec![last],
            Phase::Bubble => (0..last).rev().collect(),
        };
        for index in order {
            let node = path[index].clone();
            // `local` in this node's own frame; the aimed node keeps the
            // event verbatim, so a widget with no allocation is unaffected.
            let localised = if index == last {
                None
            } else {
                window_point.zip(origin_of(&node)).map(|(point, origin)| {
                    with_local(event, (point.0 - origin.0, point.1 - origin.1))
                })
            };
            let event: &Event = localised.as_ref().unwrap_or(event);
            let Some(instance) = roots.iter_mut().find_map(|root| root.find_mut(&node)) else {
                continue;
            };
            // Split the borrow: the controller is `&mut`, everything else
            // the `EventCx` needs is behind a different field or a clone.
            let Instance {
                controller,
                handlers,
                node: own,
                ..
            } = instance;
            let mut ecx = EventCx {
                node: own,
                handlers,
                tree: cx.tree,
                styles: cx.styles,
                focus: cx.focus,
                clipboard: cx.clipboard,
                icons: cx.icons,
                fonts: cx.fonts,
                clock: cx.clock,
                env: cx.env,
                cmds: cx.cmds,
                phase,
                handled: false,
            };
            // M5-D5/P0-D1: the node the event is aimed at — the innermost
            // `Instance` (P5-D33) — gets its pointer handlers fired before its
            // controller runs, so a controller that sets `cx.handled` (as
            // `GenericC` does on every left press) cannot swallow them.
            //
            // P0-D8: and again while bubbling, so a container whose press
            // landed on a child instance still sees the gesture. `Bubble`'s
            // node order excludes the target, so nothing fires twice, and
            // P4-D19 still governs — a child that marked the event handled
            // ends the dispatch and there is no bubble to fire on.
            if phase == Phase::Target || phase == Phase::Bubble {
                out.extend(fire_pointer_handlers(event, &*handlers));
            }
            out.extend(controller.on_event(event, &mut ecx));
            // `cx.handled` ends the whole dispatch (deviation D19, recorded in
            // §10 as P4-D19): the node that set it is the last one this event
            // reaches, so a handled capture never lets the event descend to
            // the target and a handled target never lets it bubble.
            if ecx.handled {
                break 'phases;
            }
        }
    }
    out
}

/// Run every controller's `tick`, deepest last, and collect what they say.
pub fn tick_all<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    let mut out = Vec::new();
    for root in roots.iter_mut() {
        tick_one(root, now, cx, &mut out);
    }
    out
}

fn tick_one<Msg: Clone + 'static>(
    instance: &mut Instance<Msg>,
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
    out: &mut Vec<Msg>,
) {
    let Instance {
        controller,
        handlers,
        node,
        children,
        ..
    } = instance;
    let mut ecx = EventCx {
        node,
        handlers,
        tree: cx.tree,
        styles: cx.styles,
        focus: cx.focus,
        clipboard: cx.clipboard,
        icons: cx.icons,
        fonts: cx.fonts,
        clock: cx.clock,
        env: cx.env,
        cmds: cx.cmds,
        phase: Phase::Target,
        handled: false,
    };
    out.extend(controller.tick(now, &mut ecx));
    for child in children {
        tick_one(child, now, cx, out);
    }
}

/// The earliest deadline any controller in the tree reports.
#[must_use]
pub fn next_controller_deadline<Msg: Clone + 'static>(
    roots: &[Instance<Msg>],
    now: Duration,
) -> Option<Duration> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        now: Duration,
        best: &mut Option<Duration>,
    ) {
        if let Some(deadline) = instance.controller.next_deadline(now) {
            *best = Some(best.map_or(deadline, |b: Duration| b.min(deadline)));
        }
        for child in &instance.children {
            walk(child, now, best);
        }
    }
    let mut best = None;
    for root in roots {
        walk(root, now, &mut best);
    }
    best
}

/// One step of an offscreen script.
pub enum ScriptStep<Msg> {
    /// Feed an input event, exactly as a `Window::pump` would have.
    Event(InputEvent),
    /// Move the `ManualClock` forward and run one frame.
    Advance(Duration),
    /// Inject a message as if a controller had produced it.
    Message(Msg),
    /// Render and keep the resulting RGBA frame.
    Capture,
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for ScriptStep<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptStep::Event(ev) => f.debug_tuple("Event").field(ev).finish(),
            ScriptStep::Advance(d) => f.debug_tuple("Advance").field(d).finish(),
            ScriptStep::Message(m) => f.debug_tuple("Message").field(m).finish(),
            ScriptStep::Capture => f.write_str("Capture"),
        }
    }
}

/// The frames a script captured.
#[derive(Debug, Default)]
pub struct Frames {
    size: (u32, u32),
    frames: Vec<Vec<u8>>,
}

impl Frames {
    /// How many frames were captured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether nothing was captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The surface size every frame shares.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The surface width every frame shares.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.size.0
    }

    /// The surface height every frame shares.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.size.1
    }

    /// One pixel, as `(r, g, b, a)` straight out of skia's N32 buffer;
    /// `None` when the frame or the point is out of range.
    #[must_use]
    pub fn pixel(&self, frame: usize, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        let (w, h) = self.size;
        if x >= w || y >= h {
            return None;
        }
        let buffer = self.frames.get(frame)?;
        let offset = ((y as usize) * (w as usize) + (x as usize)) * 4;
        let bytes = buffer.get(offset..offset + 4)?;
        Some((bytes[0], bytes[1], bytes[2], bytes[3]))
    }
}

/// What can go wrong running an app.
#[derive(Debug)]
pub enum AppError {
    /// The surface or its connection failed.
    Surface(SurfaceError),
    /// Layout failed.
    Layout(LayoutError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Surface(e) => write!(f, "surface: {e}"),
            AppError::Layout(e) => write!(f, "layout: {e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<SurfaceError> for AppError {
    fn from(value: SurfaceError) -> Self {
        AppError::Surface(value)
    }
}
impl From<LayoutError> for AppError {
    fn from(value: LayoutError) -> Self {
        AppError::Layout(value)
    }
}

/// A built, styled and laid-out view tree with no Wayland connection.
///
/// This is what `gallery --probe-points` reads: the same pipeline
/// [`App::run`] drives, stopped after the first layout so a tool can ask the
/// tree where its nodes ended up. Without it the only way to learn a
/// coordinate would be to hard-code it, which the M3 gate forbids.
pub struct Probe<Msg> {
    root: Node,
    instances: Vec<Instance<Msg>>,
    tree: LayoutTree,
}

impl<Msg> Probe<Msg> {
    /// The synthetic root every instance hangs from.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The top-level instances, in view order.
    #[must_use]
    pub fn instances(&self) -> &[Instance<Msg>] {
        &self.instances
    }

    /// `node`'s allocation in tree-origin coordinates, or `None` for a node
    /// that is not in this tree.
    #[must_use]
    pub fn allocation(&self, node: &Node) -> Option<crate::layout::Allocation> {
        self.tree.allocation(node)
    }
}

/// `App::update`'s field type (M5-D4). Named so `clippy::type_complexity`
/// does not fire on the inline `Box<dyn FnMut(..)>` in the struct.
type UpdateFn<M, Msg> = Box<dyn FnMut(&mut M, Msg) -> Cmd<Msg>>;
/// `App::view`'s field type (M5-D4); see [`UpdateFn`].
type ViewFn<M, Msg> = Box<dyn Fn(&M) -> View<Msg>>;

/// The Elm loop over a retained tree.
/// What happened to one of this app's popup surfaces.
///
/// Contract §6 P5-D4: `Cmd::OpenPopup` produces a key the loop keeps to
/// itself, and `InputEvent::PopupDone` is routed to the focused *controller*,
/// so an `update` that owns the open/closed state — which contract §3.4 makes
/// the panel's single source of truth — had no way to see either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PopupEvent {
    /// `Cmd::OpenPopup` succeeded and this is the key it produced.
    Opened(PopupKey),
    /// The compositor dismissed it (`xdg_popup.popup_done`) — an outside
    /// click, a grab break, or the parent going away. The surface is already
    /// gone by the time the message reaches `update`.
    Dismissed(PopupKey),
}

pub struct App<M, Msg> {
    model: M,
    /// Boxed, not a `fn` pointer (M5-D4): both M5 apps capture — settings a
    /// `WorkerHandles`, shell an `Rc<dyn CompositorCommands>` its tests swap.
    /// `FnMut`, because an `update` may own counters and senders.
    update: UpdateFn<M, Msg>,
    /// `Fn`, not `FnMut`: `view` runs during reconcile while the model is
    /// borrowed, and must not mutate.
    view: ViewFn<M, Msg>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
    /// External messages (M5-D2). At most one per app.
    inbox: Option<crate::view::inbox::Inbox<Msg>>,
    /// `FdReady(id)` → messages (M5-D2's `on_fd`).
    #[allow(clippy::type_complexity, reason = "one boxed closure per watched fd")]
    fd_handlers: Vec<(crate::window::WatchId, Box<dyn Fn() -> Vec<Msg>>)>,
    /// Turns popup lifecycle into messages (M5-D4's `on_popup`).
    popup_hook: Option<Box<dyn Fn(PopupEvent) -> Option<Msg>>>,
    /// Where `run` writes `probe`/`alloc` lines, when asked (M5-D9, P0-D4).
    probe_report: Option<std::path::PathBuf>,
    /// Observes the live window once per rendered frame (M5-D5's `on_frame`).
    #[allow(
        clippy::type_complexity,
        reason = "one boxed closure, at most one per app"
    )]
    frame_hook: Option<Box<dyn FnMut(&crate::window::Window)>>,
}

impl<M: std::fmt::Debug, Msg> std::fmt::Debug for App<M, Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// The mutable half of a running app, shared by `run` and `run_offscreen`.
struct Runtime<Msg> {
    root: Node,
    instances: Vec<Instance<Msg>>,
    styles: StyleMap,
    anims: Animations,
    containers: HashMap<NodeAddr, Container>,
    layout: LayoutTree,
    focus: FocusRing,
    grab: ImplicitGrab,
    hovered: Option<Node>,
    /// The node the last `Event::FocusIn` was delivered to.
    ///
    /// `FocusRing` records who holds the focus but not that the change has
    /// been announced; this is the announced half, and the difference between
    /// the two is exactly one `FocusOut`/`FocusIn` pair (see [`sync_focus`]).
    focused: Option<Node>,
    /// The last pointer position in window-frame space, so a button event
    /// (which carries none) can be aimed and localised.
    last_pointer: (f32, f32),
    /// Which surface the pointer is on, from the last `PointerEnter`.
    ///
    /// Contract §6 P5-D1: `route` hit-tests one retained tree, and a popup is
    /// a second surface with its own. Wayland's own rule is the rule here —
    /// an enter names the surface and everything up to the matching leave
    /// belongs to it.
    pointer_target: SurfaceTarget,
    /// Which surface has the keyboard, from the last `KeyboardEnter`.
    keyboard_target: SurfaceTarget,
    queue: VecDeque<Msg>,
    cmds: Vec<Cmd<Msg>>,
    timers: Vec<(Duration, Rc<dyn Fn() -> Msg>)>,
    images: ImageCache,
    env: ResolveEnv,
    quit: bool,
    /// The popup surfaces this app has open, in the order they were opened.
    ///
    /// `Cmd::OpenPopup` carries the popup's own view function, and this is
    /// where that payload is built and kept: without it the command opened a
    /// surface and dropped the view (contract §10 P5-D34).
    popups: Vec<PopupSurface<Msg>>,
    /// The next offscreen [`PopupKey`] to mint. A windowed run takes its keys
    /// from `Window::open_popup` instead.
    next_popup_key: u64,
}

/// One open popup surface's own retained tree.
///
/// A popup is a second surface with its own root, styles, layout and
/// instances -- the main window's tree knows nothing about it -- so it
/// carries the same five fields `Runtime` does for the window, plus where it
/// sits in the parent's frame space.
struct PopupSurface<Msg> {
    key: PopupKey,
    root: Node,
    instances: Vec<Instance<Msg>>,
    styles: StyleMap,
    anims: Animations,
    containers: HashMap<NodeAddr, Container>,
    layout: LayoutTree,
    /// Top-left in the parent window's frame space, from the positioner's
    /// anchor rectangle.
    origin: (f32, f32),
    size: (u32, u32),
    /// The payload `Cmd::OpenPopup` carried.
    ///
    /// Contract §6 P5-D2: kept, not dropped, so `rebuild_popups` can run it
    /// again on every fold — an open popover tracks the model exactly as the
    /// window's own tree does.
    view: Rc<dyn Fn() -> View<Msg>>,
}

/// Bridges `Controller::measure` into taffy.
struct ControllerMeasure<'a, Msg> {
    instances: &'a mut Vec<Instance<Msg>>,
    sheet: &'a CompiledSheet,
    fonts: &'a mut FontDatabase,
    icons: &'a mut IconTheme,
    clock: &'a Rc<dyn Clock>,
    env: &'a ResolveEnv,
}

impl<Msg: Clone + 'static> Measure for ControllerMeasure<'_, Msg> {
    fn measure(
        &mut self,
        node: &Node,
        _style: &crate::css::computed::ComputedStyle,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        let cap = |known: Option<f32>, space: taffy::AvailableSpace| match (known, space) {
            (Some(v), _) => Some(v),
            (None, taffy::AvailableSpace::Definite(v)) => Some(v),
            _ => None,
        };
        let mut cx = BuildCx {
            sheet: self.sheet,
            fonts: self.fonts,
            icons: self.icons,
            clock: self.clock,
            env: self.env,
        };
        let room = (
            cap(known.width, available.width),
            cap(known.height, available.height),
        );
        let measured = match self
            .instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
        {
            Some(instance) => instance.controller.measure(room, &mut cx),
            // No instance owns this node: a recycling view's pooled row, which
            // is a bare `Node` by design. Its text lives on the row-binding
            // side table instead (`widgets::measure_row`).
            None => crate::widgets::measure_row(node, room, &mut cx),
        };
        match measured {
            Some((w, h)) => taffy::Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(h),
            },
            None => taffy::Size {
                width: known.width.unwrap_or(0.0),
                height: known.height.unwrap_or(0.0),
            },
        }
    }
}

/// Bridges `Controller::paint` into `paint_tree`.
struct ControllerPainter<'a, Msg> {
    instances: &'a mut Vec<Instance<Msg>>,
}

impl<Msg: Clone + 'static> NodePainter for ControllerPainter<'_, Msg> {
    fn paint_content(
        &mut self,
        node: &Node,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        match self
            .instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
        {
            Some(instance) => instance.controller.paint(canvas, alloc, style, cx),
            // A pooled row, for `ControllerMeasure::measure`'s reason.
            None => crate::widgets::paint_row(node, canvas, alloc, style, cx),
        }
    }
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// A new app over `model`, folded by `update`, described by `view`.
    #[must_use]
    pub fn new(
        model: M,
        update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static,
        view: impl Fn(&M) -> View<Msg> + 'static,
    ) -> Self {
        App {
            model,
            update: Box::new(update),
            view: Box::new(view),
            sheet: None,
            fonts: None,
            icons: None,
            inbox: None,
            fd_handlers: Vec::new(),
            popup_hook: None,
            probe_report: std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from),
            frame_hook: None,
        }
    }

    /// The compiled theme `run_offscreen` styles with (deviation D11).
    /// `run` takes the window's instead.
    #[must_use]
    pub fn with_sheet(mut self, sheet: CompiledSheet) -> Self {
        self.sheet = Some(sheet);
        self
    }

    /// The font database `run_offscreen` shapes with (deviation D11).
    #[must_use]
    pub fn with_fonts(mut self, fonts: FontDatabase) -> Self {
        self.fonts = Some(fonts);
        self
    }

    /// The icon theme both loops resolve icons through (deviation D11).
    #[must_use]
    pub fn with_icons(mut self, icons: IconTheme) -> Self {
        self.icons = Some(icons);
        self
    }

    /// Feed `inbox`'s messages into this app's loop.
    ///
    /// At most one inbox per app; a second call replaces the first. `Msg: Send`
    /// is what makes a worker thread able to hold the sender, and is why an M5
    /// message carries `Arc<T>` and never `Rc<T>`.
    #[must_use]
    pub fn with_inbox(mut self, inbox: crate::view::inbox::Inbox<Msg>) -> Self
    where
        Msg: Send,
    {
        self.inbox = Some(inbox);
        self
    }

    /// Map a foreign fd's readiness to messages.
    ///
    /// `f` runs on the loop thread whenever [`InputEvent::FdReady`] names `id`,
    /// and its messages are enqueued in order — after the inbox's, before the
    /// frame's input batch. A handler for an id that has since been
    /// `unwatch`ed is simply never called again (M5-D1 §4).
    ///
    /// A second `on_fd` for the same `id` replaces the first, so this stays
    /// consistent with `with_inbox`/`on_popup`/`on_frame`.
    #[must_use]
    pub fn on_fd(mut self, id: crate::window::WatchId, f: impl Fn() -> Vec<Msg> + 'static) -> Self {
        self.fd_handlers.retain(|(w, _)| *w != id);
        self.fd_handlers.push((id, Box::new(f)));
        self
    }

    /// Turn popup lifecycle into messages. `None` drops the event.
    ///
    /// At most one hook per app; a second call replaces the first. It runs on
    /// the loop thread, and the messages it returns are queued exactly like an
    /// inbox message — never folded re-entrantly.
    #[must_use]
    pub fn on_popup(mut self, f: impl Fn(PopupEvent) -> Option<Msg> + 'static) -> Self {
        self.popup_hook = Some(Box::new(f));
        self
    }

    /// Observe the live window once per rendered frame.
    ///
    /// Runs on the loop thread after the frame is painted, with the window's
    /// layout tree already settled, so `Window::probe_points` and
    /// `Window::allocation` answer for what was just drawn. It may not
    /// mutate the model and gets no way to: this is the hook M5's apps write
    /// their `$ICEDTEA_PROBE_REPORT` lines from, and the one an app caches a
    /// widget's allocation through. Never called by `run_offscreen`, which has
    /// no window.
    ///
    /// At most one hook per app; a second call replaces the first.
    #[must_use]
    pub fn on_frame(mut self, f: impl FnMut(&crate::window::Window) + 'static) -> Self {
        self.frame_hook = Some(Box::new(f));
        self
    }

    /// Write `probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines to
    /// `path`, once per frame in which they change.
    ///
    /// `App::new` already picks `$ICEDTEA_PROBE_REPORT` up; this is the
    /// explicit form. The lines are exactly what `ui/tests/support/mod.rs`
    /// parses, and the labels are `Window::probe_points`'.
    #[must_use]
    pub fn with_probe_report(mut self, path: std::path::PathBuf) -> Self {
        self.probe_report = Some(path);
        self
    }

    /// The current model — what an offscreen test asserts on besides pixels.
    #[must_use]
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Run view → reconcile → restyle → layout → **settle to a fixpoint**
    /// (tick → restyle → layout, repeated until the allocations stop changing,
    /// capped at `PROBE_MAX_SETTLE_TICKS`), against `size`, with no surface, and
    /// hand back the result.
    ///
    /// The settling is not decoration. A controller that learns its own geometry
    /// from the frame it was just laid out in does so in
    /// [`Controller::tick`](crate::view::Controller::tick) —
    /// `ListViewC::adopt_metrics` grows its pool from one placeholder row to the
    /// viewport's worth there. A fixed-extent list reaches that in one tick, but
    /// `column_view`'s inner list has no fixed extent, so each tick grows it and
    /// it takes several to converge; [`App::run`] ticks every frame and reaches
    /// the fixpoint, so a probe must too. This settled tree is what the gallery's
    /// `--print-allocation`/`--probe-points` publish and what the gates locate
    /// every widget by. A single tick (M6-FUP1) described a tree a running app
    /// never shows: every widget below such a list came out above where it
    /// paints. The tick uses a single frozen clock instant so the fixpoint is
    /// over tree state alone, not a moving animation clock.
    ///
    /// `sheet`, `fonts` and `icons` are parameters rather than fields because
    /// [`App::run`] takes them from the `Window` it is handed; a probe has no
    /// window, so its caller supplies them.
    ///
    /// # Errors
    ///
    /// [`AppError::Layout`] if the tree cannot be laid out.
    pub fn probe(
        self,
        size: (u32, u32),
        sheet: CompiledSheet,
        mut fonts: FontDatabase,
        mut icons: IconTheme,
        clock: Rc<dyn Clock>,
    ) -> Result<Probe<Msg>, AppError> {
        // Same body as `run`'s per-frame pass, minus the surface: build the
        // view from the model, reconcile it into instances, restyle the
        // whole tree, then lay out against `size` as the available space.
        let App { model, view, .. } = self;
        let env = ResolveEnv::default();
        let root = Node::with_classes(Kind::Window.css_name(), Kind::Window.base_classes());
        let mut instances: Vec<Instance<Msg>> = Vec::new();
        let mut containers = HashMap::new();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        let mut tree = LayoutTree::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(&root, &mut instances, vec![view(&model)], &mut cx);
        }
        containers.insert(
            crate::view::render::node_addr(&root),
            Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        containers_of(&instances, &mut containers);
        restyle_and_layout(
            &root,
            &mut instances,
            &sheet,
            &env,
            &mut fonts,
            &mut icons,
            &clock,
            &containers,
            &mut styles,
            &mut anims,
            &mut tree,
            size,
        )
        .map_err(AppError::Layout)?;
        // Settle to a fixpoint, not just one tick. A controller learns its own
        // geometry from the frame it was just laid out in and adjusts in `tick`
        // (`ListViewC::adopt_metrics`, whose pool goes from one placeholder row
        // to the viewport's worth). A fixed-extent list converges in a single
        // tick, but `column_view`'s inner list has no fixed extent: each tick
        // grows its pool, the taller layout feeds the next tick's `adopt_metrics`,
        // and it takes several ticks to reach the fixpoint. `App::run` and
        // `run_offscreen` tick every frame and so converge; a `Probe` is what the
        // gallery's `--print-allocation`/`--probe-points` publish and what the
        // gates locate every widget by, so it must report that same settled tree
        // — otherwise every widget below such a list is reported above where a
        // running app paints it (M6-FUP1). Loop tick + relayout until the
        // allocations stop changing, capped so a tree that never converges still
        // terminates.
        const PROBE_MAX_SETTLE_TICKS: usize = 16;
        // Freeze the clock for the settle loop. `tick_all` and
        // `restyle_and_layout` both advance animations against the clock, but a
        // *layout* fixpoint must be over tree state alone — a widget whose
        // allocation is a function of time would otherwise never satisfy
        // `now == settled` and we would return an arbitrary animation phase (a
        // flaky probe). A running app settles its geometry the same way; only
        // the moving animation clock is excluded here.
        let frozen: Rc<dyn Clock> = {
            let c = ManualClock::new();
            c.set_ms(u64::try_from(clock.now().as_millis()).unwrap_or(u64::MAX));
            Rc::new(c)
        };
        // Snapshot every descendant's allocation in traversal order. `Option`,
        // not `filter_map`, so a node that gains or loses an allocation between
        // ticks is a detected change rather than silently dropped; two equal
        // snapshots in a row is the fixpoint.
        let snapshot = |root: &Node, tree: &LayoutTree| -> Vec<Option<crate::layout::Allocation>> {
            root.descendants().map(|n| tree.allocation(&n)).collect()
        };
        let mut settled = snapshot(&root, &tree);
        let mut converged = false;
        for _ in 0..PROBE_MAX_SETTLE_TICKS {
            {
                let mut clipboard = Clipboard::offscreen();
                let mut focus = <FocusRing as Default>::default();
                let mut cmds: Vec<Cmd<Msg>> = Vec::new();
                let mut cx = Dispatch {
                    styles: &styles,
                    tree: &tree,
                    focus: &mut focus,
                    clipboard: &mut clipboard,
                    icons: &mut icons,
                    fonts: &mut fonts,
                    clock: &frozen,
                    env: &env,
                    cmds: &mut cmds,
                };
                tick_all(&mut instances, frozen.now(), &mut cx);
            }
            restyle_and_layout(
                &root,
                &mut instances,
                &sheet,
                &env,
                &mut fonts,
                &mut icons,
                &frozen,
                &containers,
                &mut styles,
                &mut anims,
                &mut tree,
                size,
            )
            .map_err(AppError::Layout)?;
            let now = snapshot(&root, &tree);
            if now == settled {
                converged = true;
                break;
            }
            settled = now;
        }
        if !converged {
            tracing::warn!(
                ticks = PROBE_MAX_SETTLE_TICKS,
                "probe layout did not settle within the tick cap; reported \
                 allocations may not match a running app"
            );
        }
        Ok(Probe {
            root,
            instances,
            tree,
        })
    }

    /// Run the whole loop against an offscreen raster surface with no
    /// Wayland connection, driven by `script` on a [`ManualClock`].
    ///
    /// This is how the counter-app test and every controller unit test run.
    ///
    /// # Errors
    ///
    /// [`AppError::Surface`] if the raster surface cannot be created;
    /// [`AppError::Layout`] if taffy fails.
    pub fn run_offscreen(
        mut self,
        size: (u32, u32),
        clock: Rc<ManualClock>,
        script: Vec<ScriptStep<Msg>>,
    ) -> Result<Frames, AppError> {
        let sheet = self
            .sheet
            .take()
            .unwrap_or_else(|| CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT));
        let mut fonts = self.fonts.take().unwrap_or_default();
        let mut icons = self.icons.take().unwrap_or_else(IconTheme::from_env);
        let mut clipboard = Clipboard::offscreen();
        let dyn_clock: Rc<dyn Clock> = clock.clone();

        let mut rt = Runtime {
            root: Node::with_classes(Kind::Window.css_name(), Kind::Window.base_classes()),
            instances: Vec::new(),
            styles: StyleMap::new(),
            anims: Animations::new(),
            containers: HashMap::new(),
            layout: LayoutTree::new(),
            focus: <FocusRing as Default>::default(),
            grab: ImplicitGrab::default(),
            hovered: None,
            focused: None,
            last_pointer: (0.0, 0.0),
            pointer_target: SurfaceTarget::Window,
            keyboard_target: SurfaceTarget::Window,
            queue: VecDeque::new(),
            cmds: Vec::new(),
            timers: Vec::new(),
            images: ImageCache::new(),
            env: ResolveEnv::default(),
            quit: false,
            popups: Vec::new(),
            next_popup_key: 0,
        };

        let mut surface = skia_rs_safe::canvas::Surface::new_raster_n32_premul(
            i32::try_from(size.0).unwrap_or(i32::MAX),
            i32::try_from(size.1).unwrap_or(i32::MAX),
        )
        .ok_or(SurfaceError::Render("offscreen raster surface"))?;
        let mut frames = Frames {
            size,
            frames: Vec::new(),
        };
        // M5-D9, P0-D4: same report, same dedup, as `run` -- offscreen just
        // has no window to publish from.
        let mut probe_reported: Vec<String> = Vec::new();
        let mut probe_frame: u64 = 0;

        rebuild(
            &mut self, &mut rt, &sheet, &mut fonts, &mut icons, &dyn_clock,
        );
        render_once(
            &mut rt,
            &sheet,
            &mut fonts,
            &mut icons,
            &dyn_clock,
            size,
            &mut surface,
            self.probe_report
                .as_deref()
                .map(|path| (path, &mut probe_reported, &mut probe_frame)),
        )?;

        for step in script {
            // The same drain, at the same point: an ingress test needs no
            // compositor (M5-D2 §5).
            drain_inbox(&self, &mut rt);
            let mut capture = false;
            match step {
                ScriptStep::Capture => capture = true,
                ScriptStep::Message(msg) => rt.queue.push_back(msg),
                ScriptStep::Event(ev) => {
                    if let InputEvent::PopupDone(key) = &ev {
                        close_popup(&mut rt, *key);
                        if let Some(msg) = popup_msg(&self, PopupEvent::Dismissed(*key)) {
                            rt.queue.push_back(msg);
                        }
                    }
                    let produced = route(
                        &mut rt,
                        &ev,
                        &sheet,
                        &mut fonts,
                        &mut icons,
                        &mut clipboard,
                        &dyn_clock,
                    );
                    rt.queue.extend(produced);
                }
                ScriptStep::Advance(delta) => {
                    let before = clock.now();
                    clock.set_ms(u64::try_from((before + delta).as_millis()).unwrap_or(u64::MAX));
                    let now = clock.now();
                    let due: Vec<Rc<dyn Fn() -> Msg>> = {
                        let (fired, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut rt.timers)
                            .into_iter()
                            .partition(|(at, _)| *at <= now);
                        rt.timers = pending;
                        fired.into_iter().map(|(_, f)| f).collect()
                    };
                    rt.queue.extend(due.iter().map(|f| f()));
                    let ticked = {
                        let mut cx = Dispatch {
                            styles: &rt.styles,
                            tree: &rt.layout,
                            focus: &mut rt.focus,
                            clipboard: &mut clipboard,
                            icons: &mut icons,
                            fonts: &mut fonts,
                            clock: &dyn_clock,
                            env: &rt.env,
                            cmds: &mut rt.cmds,
                        };
                        tick_all(&mut rt.instances, now, &mut cx)
                    };
                    rt.queue.extend(ticked);
                }
            }

            for other in drain(
                &mut self,
                &mut rt,
                &sheet,
                &mut fonts,
                &mut icons,
                &mut clipboard,
                &dyn_clock,
                clock.now(),
            ) {
                match other {
                    // Offscreen there is no compositor to ask for a popup
                    // surface, but the command's *payload* is a view like any
                    // other: it is built into its own retained tree here and
                    // composited over the window in `render_popups`, so a
                    // pixel test can see what a menu actually paints.
                    Cmd::OpenPopup {
                        anchor,
                        positioner,
                        view,
                    } => {
                        let key = PopupKey(rt.next_popup_key);
                        rt.next_popup_key += 1;
                        let root = Node::with_classes("popup", &["background"]);
                        open_popup(
                            &mut rt,
                            key,
                            root,
                            &anchor,
                            &positioner,
                            &view,
                            &sheet,
                            &mut fonts,
                            &mut icons,
                            &dyn_clock,
                        );
                        if let Some(msg) = popup_msg(&self, PopupEvent::Opened(key)) {
                            rt.queue.push_back(msg);
                        }
                    }
                    Cmd::ClosePopup(key) => close_popup(&mut rt, key),
                    // There is still no surface to title or minimise.
                    other => tracing::debug!(?other, "command has no effect offscreen"),
                }
            }
            // `Cmd::Focus` moves the ring inside `drain`; announce it too.
            let moved = sync_focus(
                &mut rt,
                FocusCause::Programmatic,
                &mut fonts,
                &mut icons,
                &mut clipboard,
                &dyn_clock,
            );
            rt.queue.extend(moved);
            render_once(
                &mut rt,
                &sheet,
                &mut fonts,
                &mut icons,
                &dyn_clock,
                size,
                &mut surface,
                self.probe_report
                    .as_deref()
                    .map(|path| (path, &mut probe_reported, &mut probe_frame)),
            )?;
            render_popups(
                &mut rt,
                &sheet,
                &mut fonts,
                &mut icons,
                &dyn_clock,
                &mut surface,
            )?;

            if capture {
                frames.frames.push(read_rgba(&surface, size));
            }
            if rt.quit {
                break;
            }
        }
        Ok(frames)
    }
}

/// Run `view`, reconcile it into the retained tree, and refresh the
/// container map the layout walker reads.
fn rebuild<M: 'static, Msg: Clone + 'static>(
    app: &mut App<M, Msg>,
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
) {
    let described = (app.view)(&app.model);
    let mut cx = BuildCx {
        sheet,
        fonts,
        icons,
        clock,
        env: &rt.env,
    };
    reconcile(&rt.root, &mut rt.instances, vec![described], &mut cx);
    rt.containers.clear();
    rt.containers.insert(
        crate::view::render::node_addr(&rt.root),
        Container::Box {
            direction: crate::layout::BoxDirection::Column,
        },
    );
    containers_of(&rt.instances, &mut rt.containers);
}

/// Re-run every open popup's payload and reconcile it into its own tree.
///
/// The window's counterpart is [`rebuild`]; this is the same three steps --
/// describe, reconcile, refresh containers -- for each popup surface, in the
/// order they were opened. Cheap when nothing changed: `reconcile` diffs, and
/// a popup whose view returns the same tree produces no ops.
fn rebuild_popups<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
) {
    for index in 0..rt.popups.len() {
        let described = (Rc::clone(&rt.popups[index].view))();
        let mut cx = BuildCx {
            sheet,
            fonts,
            icons,
            clock,
            env: &rt.env,
        };
        let popup = &mut rt.popups[index];
        reconcile(&popup.root, &mut popup.instances, vec![described], &mut cx);
        popup.containers.clear();
        popup.containers.insert(
            crate::view::render::node_addr(&popup.root),
            Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        containers_of(&popup.instances, &mut popup.containers);
    }
}

/// Restyle the tree under `root`, then lay it out against `size`.
///
/// Shared by [`render_once`] (called every frame `run`/`run_offscreen` draw)
/// and [`App::probe`] (called once, with no surface to paint into), so the
/// two never drift into computing two different trees.
#[allow(clippy::too_many_arguments, reason = "one call site plus probe's")]
#[allow(
    clippy::cast_precision_loss,
    reason = "a surface is never 2^24 px on a side"
)]
fn restyle_and_layout<Msg: Clone + 'static>(
    root: &Node,
    instances: &mut Vec<Instance<Msg>>,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
    containers: &HashMap<NodeAddr, Container>,
    styles: &mut StyleMap,
    anims: &mut Animations,
    tree: &mut LayoutTree,
    size: (u32, u32),
) -> Result<(), LayoutError> {
    let now = clock.now();
    restyle_tree(root, sheet, env, styles, anims, now);
    let mut measure = ControllerMeasure {
        instances,
        sheet,
        fonts,
        icons,
        clock,
        env,
    };
    layout_tree(
        root,
        styles,
        containers,
        tree,
        env,
        (Some(size.0 as f32), Some(size.1 as f32)),
        &mut measure,
    )
}

/// Restyle, relayout and repaint into `surface`.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site's worth of loop state plus the offscreen probe report"
)]
fn render_once<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
    size: (u32, u32),
    surface: &mut skia_rs_safe::canvas::Surface,
    probe: Option<(&std::path::Path, &mut Vec<String>, &mut u64)>,
) -> Result<(), AppError> {
    let now = clock.now();
    restyle_and_layout(
        &rt.root,
        &mut rt.instances,
        sheet,
        &rt.env,
        fonts,
        icons,
        clock,
        &rt.containers,
        &mut rt.styles,
        &mut rt.anims,
        &mut rt.layout,
        size,
    )?;
    // M5-D9, P0-D4: `run_offscreen` never hands its window over, but its
    // laid-out tree still lives only in this `Runtime` -- the same reason
    // `run` publishes from inside its own loop.
    if let Some((path, last, frame)) = probe {
        let lines = probe_report_lines(rt);
        write_probe_report(path, &lines, last, frame);
    }
    surface
        .canvas()
        .clear(skia_rs_safe::core::Color::TRANSPARENT);
    let mut cx = PaintCx {
        env: &rt.env,
        colors: &sheet.colors,
        fonts,
        images: &mut rt.images,
        icons,
        text: None,
    };
    let mut painter = ControllerPainter {
        instances: &mut rt.instances,
    };
    let mut canvas = surface.canvas();
    paint_tree(
        &mut canvas,
        &rt.root,
        &rt.styles,
        &rt.layout,
        &mut rt.anims,
        now,
        (0.0, 0.0),
        &mut cx,
        &mut painter,
    );
    Ok(())
}

/// Build `view`'s tree into a new popup surface and push it onto `rt`.
///
/// Ten arguments because a popup build needs everything a window build does
/// (`sheet`/`fonts`/`icons`/`clock` are `BuildCx`'s own fields, borrowed
/// separately from `rt` at both call sites) plus the surface's identity and
/// placement.
///
/// This is the payload half of `Cmd::OpenPopup`: the command carries the
/// popup's own view function, and until the P6 fix wave the loop dropped it
/// and opened an empty surface (contract §10 P5-D34). `key` comes from
/// `Window::open_popup` in a windowed run and from `rt.next_popup_key`
/// offscreen; `root` is the popup surface's own root node, the window's own
/// when there is a window.
#[allow(
    clippy::too_many_arguments,
    reason = "a popup build takes the window build's five borrows plus the surface's identity, placement and payload"
)]
fn open_popup<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    key: PopupKey,
    root: Node,
    anchor: &PopupAnchorPoint,
    positioner: &Positioner,
    view: &Rc<dyn Fn() -> View<Msg>>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
) {
    let anchor_rect = match anchor {
        crate::window::popup::PopupAnchorPoint::Rect(rect) => *rect,
        crate::window::popup::PopupAnchorPoint::Node(node) => rt
            .layout
            .allocation(node)
            .map_or(positioner.anchor_rect, |a| a.border_box),
    };
    let mut surface = PopupSurface {
        key,
        root,
        instances: Vec::new(),
        styles: StyleMap::new(),
        anims: Animations::new(),
        containers: HashMap::new(),
        layout: LayoutTree::new(),
        // The compositor places the real surface from the positioner's
        // gravity; offscreen there is no compositor, so this is the one
        // placement a menu positioner always means -- under the anchor's
        // bottom-left corner.
        origin: (anchor_rect.x, anchor_rect.y + anchor_rect.height),
        size: positioner.size,
        view: Rc::clone(view),
    };
    let described = view();
    let mut cx = BuildCx {
        sheet,
        fonts,
        icons,
        clock,
        env: &rt.env,
    };
    reconcile(
        &surface.root,
        &mut surface.instances,
        vec![described],
        &mut cx,
    );
    surface.containers.insert(
        crate::view::render::node_addr(&surface.root),
        Container::Box {
            direction: crate::layout::BoxDirection::Column,
        },
    );
    containers_of(&surface.instances, &mut surface.containers);
    rt.popups.push(surface);
}

/// Drop the popup `key` names, and every popup opened after it.
///
/// xdg-shell destroys popups topmost-first, and a menu chain dismissed at
/// its root takes its submenus with it; the retained trees follow the same
/// rule so a `ClosePopup` on the root never leaves an orphan.
fn close_popup<Msg>(rt: &mut Runtime<Msg>, key: PopupKey) {
    if let Some(index) = rt.popups.iter().position(|p| p.key == key) {
        rt.popups.truncate(index);
    }
}

/// Ask the app's popup hook for a message, if it has one.
fn popup_msg<M, Msg>(app: &App<M, Msg>, ev: PopupEvent) -> Option<Msg> {
    app.popup_hook.as_ref().and_then(|f| f(ev))
}

/// Restyle, relayout and paint every open popup into `surface`, at its own
/// origin in the parent's frame space.
///
/// Offscreen there is one raster surface, so a popup composites over the
/// window the way the compositor stacks the real surfaces.
fn render_popups<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
    surface: &mut skia_rs_safe::canvas::Surface,
) -> Result<(), AppError> {
    let now = clock.now();
    for index in 0..rt.popups.len() {
        let (width, height) = rt.popups[index].size;
        let origin = rt.popups[index].origin;
        {
            let popup = &mut rt.popups[index];
            restyle_and_layout(
                &popup.root,
                &mut popup.instances,
                sheet,
                &rt.env,
                fonts,
                icons,
                clock,
                &popup.containers,
                &mut popup.styles,
                &mut popup.anims,
                &mut popup.layout,
                (width, height),
            )?;
        }
        let images = &mut rt.images;
        let env = &rt.env;
        let popup = &mut rt.popups[index];
        let mut cx = PaintCx {
            env,
            colors: &sheet.colors,
            fonts,
            images,
            icons,
            text: None,
        };
        let mut painter = ControllerPainter {
            instances: &mut popup.instances,
        };
        let mut canvas = surface.canvas();
        paint_tree(
            &mut canvas,
            &popup.root,
            &popup.styles,
            &popup.layout,
            &mut popup.anims,
            now,
            origin,
            &mut cx,
            &mut painter,
        );
    }
    Ok(())
}

/// Announce a focus move as `Event::FocusOut` then `Event::FocusIn`.
///
/// Nothing else constructs those two events: the focus can move from a
/// controller (`EventCx::set_focus`), from `Cmd::Focus` or from a keyboard
/// binding, and all three go through `FocusRing`, which changes
/// `PseudoStates::FOCUS` but sends no message. This compares the ring's owner
/// against the last one announced and dispatches the pair when they differ, so
/// `View::on_focus_in`/`on_focus_out` fire once per real move whatever moved
/// it.
///
/// `cause` is the input that moved it, which `FocusRing` does not retain.
fn sync_focus<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    cause: FocusCause,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
    let current = rt.focus.focus();
    let same = match (rt.focused.as_ref(), current.as_ref()) {
        (Some(old), Some(new)) => old.ptr_eq(new),
        (None, None) => true,
        _ => false,
    };
    if same {
        return Vec::new();
    }
    let mut pending: Vec<(Node, Event)> = Vec::new();
    if let Some(old) = rt.focused.take() {
        pending.push((old, Event::FocusOut));
    }
    if let Some(new) = current {
        pending.push((new.clone(), Event::FocusIn { cause }));
        rt.focused = Some(new);
    }

    let mut out = Vec::new();
    for (node, ev) in pending {
        let path = path_to(&rt.instances, &node);
        let mut cx = Dispatch {
            styles: &rt.styles,
            tree: &rt.layout,
            focus: &mut rt.focus,
            clipboard,
            icons,
            fonts,
            clock,
            env: &rt.env,
            cmds: &mut rt.cmds,
        };
        out.extend(deliver(&mut rt.instances, &path, &ev, &mut cx));
    }
    out
}

/// Whether `node` or any ancestor of it is recorded `display: none`.
///
/// `LayoutTree::is_displayed` answers for the node it was asked about only,
/// and only the subtree *root* a widget hides is ever recorded (a `Stack`
/// marks the page, never the page's contents), so the walk to the root is what
/// makes a buried descendant answer honestly. A node the tree has never seen
/// counts as displayed, which keeps a widget the reconciler has just added
/// (and not yet laid out) a focus candidate.
#[must_use]
pub fn is_hidden(tree: &crate::layout::LayoutTree, node: &Node) -> bool {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        if !tree.is_displayed(&node) {
            return true;
        }
        current = node.parent();
    }
    false
}

/// Drop the focus when its owner has been hidden.
fn prune_hidden_focus<Msg: Clone + 'static>(rt: &mut Runtime<Msg>) {
    if let Some(node) = rt.focus.focus()
        && is_hidden(&rt.layout, &node)
    {
        rt.focus.set_focus(None, FocusCause::Programmatic);
    }
}

/// Swap popup `index`'s retained tree into the runtime's window slots.
///
/// Called twice around a routing pass, so the popup's tree is what `route_surface`
/// hit-tests, dispatches into and mutates. Destructured rather than indexed so
/// the four swaps borrow disjoint fields of `rt`.
fn swap_popup_tree<Msg>(rt: &mut Runtime<Msg>, index: usize) {
    let Runtime {
        root,
        instances,
        styles,
        layout,
        popups,
        ..
    } = rt;
    let popup = &mut popups[index];
    std::mem::swap(root, &mut popup.root);
    std::mem::swap(instances, &mut popup.instances);
    std::mem::swap(styles, &mut popup.styles);
    std::mem::swap(layout, &mut popup.layout);
}

/// Lend every open popup's laid-out tree to the `Window`, or take it back.
///
/// The frame hook's mirror of [`swap_popup_tree`]: each popup's tree lives on
/// `rt.popups` (for taffy's incremental dirty tracking), not on the `Window`
/// popup whose `popup_probe_points` the hook reads, so it is swapped in for the
/// span of the hook and swapped straight back. The swap is its own inverse, so
/// one function serves both the swap-in and the swap-out call.
fn swap_popup_layouts<Msg>(rt: &mut Runtime<Msg>, window: &mut crate::window::Window) {
    for popup in &mut rt.popups {
        if let Some(win_layout) = window.popup_layout(popup.key) {
            std::mem::swap(&mut popup.layout, win_layout);
        }
    }
}

/// Route one input event to the surface it arrived on.
///
/// Contract §6 P5-D1. `InputEvent`'s two `Enter` variants carry a
/// [`SurfaceTarget`]; everything after an enter belongs to that surface until
/// the matching leave, which is how Wayland itself defines focus and is what
/// `SurfaceTarget`'s own doc comment says. Coordinates on a popup event are
/// already popup-surface-local, so no offset arithmetic is needed: the popup's
/// tree starts at its own (0, 0), exactly as `paint_tree` paints it.
#[allow(
    clippy::too_many_arguments,
    reason = "one routing pass threading the whole per-frame context"
)]
fn route<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    event: &InputEvent,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
    let target = match event {
        InputEvent::PointerEnter { target, .. } => {
            rt.pointer_target = *target;
            *target
        }
        InputEvent::PointerLeave => {
            let was = rt.pointer_target;
            rt.pointer_target = SurfaceTarget::Window;
            was
        }
        InputEvent::KeyboardEnter { target, .. } => {
            rt.keyboard_target = *target;
            *target
        }
        InputEvent::KeyboardLeave => {
            let was = rt.keyboard_target;
            rt.keyboard_target = SurfaceTarget::Window;
            was
        }
        InputEvent::Key(_) => rt.keyboard_target,
        _ => rt.pointer_target,
    };
    let index = match target {
        SurfaceTarget::Window => None,
        // A key for a popup this app no longer retains routes nowhere rather
        // than falling back to the window: the event belonged to a surface
        // that is gone, and replaying it on the window would fire the wrong
        // handler at the wrong coordinates.
        SurfaceTarget::Popup(key) => match rt.popups.iter().position(|p| p.key == key) {
            Some(index) => Some(index),
            None => return Vec::new(),
        },
    };
    let Some(index) = index else {
        return route_surface(rt, event, sheet, fonts, icons, clipboard, clock);
    };
    swap_popup_tree(rt, index);
    let out = route_surface(rt, event, sheet, fonts, icons, clipboard, clock);
    // Unconditionally, with nothing fallible between the two swaps: leaving
    // the popup's tree in the window's slots would corrupt every later frame.
    swap_popup_tree(rt, index);
    out
}

/// Turn one `InputEvent` into controller events and return the messages.
#[allow(
    clippy::too_many_arguments,
    reason = "one routing pass threading the whole per-frame context"
)]
fn route_surface<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    event: &InputEvent,
    _sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
    // A subtree that has become `display: none` — a `Stack` page the user
    // left, a collapsed `Expander`, a closed `Popover` — must not keep the
    // keyboard focus: `FocusRing` records an owner and nothing else checks
    // that the owner is still on screen, so every keystroke went on landing in
    // an invisible `Entry` on a page nobody can see. Pruned centrally, here,
    // once per routed event rather than in each of the widgets that hide a
    // subtree; the `sync_focus` at the end of this function announces the
    // resulting `FocusOut` like any other move.
    prune_hidden_focus(rt);

    // The node an event is aimed at, and the point in its own space.
    let aim = |rt: &Runtime<Msg>, x: f32, y: f32| -> Option<(Node, (f32, f32))> {
        if let Some(target) = rt.grab.target() {
            let local = rt
                .layout
                .allocation(&target)
                .map_or((x, y), |a| (x - a.border_box.x, y - a.border_box.y));
            return Some((target, local));
        }
        let chain = hit_chain(&rt.root, &rt.layout, &rt.styles, (x, y));
        // The geometrically deepest hit is often a *controller-owned* subnode
        // -- a `DropDown`'s `button.toggle`, a `ColorDialogButton`'s
        // `button.color` -- which no `Instance` owns. `path_to` finds no path
        // to such a node, so `deliver` below silently drops the event and the
        // widget never sees its own click. Flat widgets hid this: their
        // content nodes measure to (0, 0), so their deepest hit *was* their
        // own `Instance` root. Aim instead at the innermost node the instance
        // tree actually knows, which for a flat widget is the same node as
        // before and for nested chrome is the controller that built it.
        chain
            .iter()
            .rev()
            .find(|hit| !path_to(&rt.instances, &hit.node).is_empty())
            .map(|hit| (hit.node.clone(), hit.local))
    };

    let mut pending: Vec<(Node, Event)> = Vec::new();
    match event {
        InputEvent::PointerEnter { x, y, .. } | InputEvent::PointerMotion { x, y, .. } => {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "surface coordinates are within f32 exactly"
            )]
            let point = (*x as f32, *y as f32);
            rt.last_pointer = point;
            let target = aim(rt, point.0, point.1);
            let same = match (rt.hovered.as_ref(), target.as_ref()) {
                (Some(old), Some((new, _))) => old.ptr_eq(new),
                (None, None) => true,
                _ => false,
            };
            if !same {
                if let Some(old) = rt.hovered.take() {
                    pending.push((old, Event::PointerLeave));
                }
                if let Some((node, local)) = target.clone() {
                    pending.push((node.clone(), Event::PointerEnter { local }));
                    rt.hovered = Some(node);
                }
            }
            if let Some((node, local)) = target {
                pending.push((node, Event::PointerMotion { local }));
            }
        }
        InputEvent::PointerLeave => {
            if let Some(old) = rt.hovered.take() {
                pending.push((old, Event::PointerLeave));
            }
        }
        InputEvent::PointerButton {
            button,
            pressed,
            serial,
            ..
        } => {
            // `wl_pointer.button` carries no coordinates: the position is
            // the last motion's, and the target is the grab's while one is
            // held, exactly as the compositor's implicit grab decides it.
            let (x, y) = rt.last_pointer;
            if let Some((node, local)) = aim(rt, x, y) {
                if *pressed {
                    rt.grab.press(*button, &node);
                    pending.push((
                        node,
                        Event::PointerDown {
                            button: *button,
                            local,
                            serial: *serial,
                        },
                    ));
                } else {
                    rt.grab.release(*button);
                    pending.push((
                        node,
                        Event::PointerUp {
                            button: *button,
                            local,
                            serial: *serial,
                        },
                    ));
                }
            }
        }
        InputEvent::Scroll(scroll) => {
            if let Some(node) = rt.hovered.clone() {
                pending.push((node, Event::Scroll(*scroll)));
            }
        }
        InputEvent::Key(key) => {
            // GTK's focus-visible rule (`_gtk_window_update_focus_visible`),
            // fed every key both ways *before* the key is acted on, so the
            // press records the focus this key found and the release can ask
            // whether it moved: a Tab that moves the focus shows the ring,
            // typing a letter hides it again.
            rt.focus.note_key(key);
            // Tab is the window's, not the focused widget's. P3 shipped
            // `window_binding`/`navigate` and P8-D72's close-out is where
            // `App::run` finally calls them: until then nothing in the app
            // ever moved the focus by keyboard at all -- Tab was delivered to
            // the focused controller, no controller in the crate reads it, and
            // it did nothing. Only the two Tab directions are taken here; the
            // arrows, Space, Return and Escape `window_binding` also names
            // belong to widgets that already read them for themselves (a
            // `ListView`'s row cursor, a `SpinButton`'s step), and stealing
            // them at the window would break those.
            let tab = match window_binding(key) {
                Some(Binding::Move(
                    dir @ (FocusDirection::TabForward | FocusDirection::TabBackward),
                )) => Some(dir),
                _ => None,
            };
            if let Some(dir) = tab {
                // Wrapping at the end of the ring, as `window-probe` does:
                // the second `navigate` starts over from no focus at all.
                let next = navigate(&rt.root, &rt.layout, rt.focus.focus().as_ref(), dir)
                    .or_else(|| navigate(&rt.root, &rt.layout, None, dir));
                if next.is_some() {
                    rt.focus.set_focus(next.as_ref(), FocusCause::Keyboard);
                }
            } else if let Some(node) = rt.focus.focus() {
                pending.push((node, Event::Key(key.clone())));
            }
        }
        InputEvent::Configure { size, states } => {
            for instance in &rt.instances {
                pending.push((
                    instance.node.clone(),
                    Event::Configure {
                        size: *size,
                        states: *states,
                    },
                ));
            }
        }
        InputEvent::PopupDone(_) => {
            if let Some(node) = rt.focus.focus() {
                pending.push((node, Event::PopupDone));
            }
        }
        _ => {}
    }

    let mut out = Vec::new();
    for (node, ev) in pending {
        let path = path_to(&rt.instances, &node);
        let mut cx = Dispatch {
            styles: &rt.styles,
            tree: &rt.layout,
            focus: &mut rt.focus,
            clipboard,
            icons,
            fonts,
            clock,
            env: &rt.env,
            cmds: &mut rt.cmds,
        };
        out.extend(deliver(&mut rt.instances, &path, &ev, &mut cx));
    }

    // A controller may have moved the focus while handling the event above
    // (`GenericC` does, on every left press); announce it in the same pass so
    // `on_focus_in`/`on_focus_out` land in this frame's message batch.
    let cause = match event {
        InputEvent::Key(_) => FocusCause::Keyboard,
        InputEvent::PointerButton { .. }
        | InputEvent::PointerEnter { .. }
        | InputEvent::PointerMotion { .. }
        | InputEvent::PointerLeave => FocusCause::Pointer,
        _ => FocusCause::Programmatic,
    };
    out.extend(sync_focus(rt, cause, fonts, icons, clipboard, clock));
    out
}

/// M5-D2 §3 steps (a) and (b): read the wake pipe empty, then move every
/// queued message onto the fold queue **in send order**.
///
/// Called once per loop iteration, before any input is routed, so a message a
/// worker produced is folded against the same model the frame's clicks are.
fn drain_inbox<M, Msg>(app: &App<M, Msg>, rt: &mut Runtime<Msg>) {
    let Some(inbox) = app.inbox.as_ref() else {
        return;
    };
    inbox.drain_pipe();
    inbox.drain_into(&mut rt.queue);
}

/// Fold every queued message, run the commands they produced, and rebuild
/// the view **once** per drained batch (contract §4.7).
///
/// Returns the flattened commands `drain` itself cannot execute -- the
/// window-bound ones (`SetTitle`, `Minimize`, `ToggleMaximized`, `OpenPopup`,
/// `ClosePopup`). [`App::run`] applies them to its `Window`; `run_offscreen`
/// has no surface and logs them. They must be *returned* rather than dropped
/// here: `drain` is what calls `update`, so a `Cmd` an `update` returns first
/// exists inside this function, and a filter run before it never sees one.
#[allow(
    clippy::too_many_arguments,
    reason = "one fold pass threading the whole per-frame context"
)]
fn drain<M: 'static, Msg: Clone + 'static>(
    app: &mut App<M, Msg>,
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
    now: Duration,
) -> Vec<Cmd<Msg>> {
    if rt.queue.is_empty() && rt.cmds.is_empty() {
        return Vec::new();
    }
    let mut folded = false;
    let mut unhandled = Vec::new();
    // A bound, so an `update` that enqueues on every message cannot wedge
    // the loop: the rest is carried to the next frame.
    for _ in 0..1024 {
        let Some(msg) = rt.queue.pop_front() else {
            break;
        };
        let cmd = (app.update)(&mut app.model, msg);
        rt.cmds.push(cmd);
        folded = true;
    }
    for cmd in std::mem::take(&mut rt.cmds)
        .into_iter()
        .flat_map(Cmd::flatten)
    {
        match cmd {
            Cmd::After(delay, f) => rt.timers.push((now + delay, f)),
            Cmd::Copy(text) => clipboard.copy(&text, 0),
            Cmd::Paste(f) => {
                let value = clipboard.paste(Duration::from_millis(50));
                rt.queue.push_back(f(value));
            }
            Cmd::SetPrimary(text) => clipboard.set_primary(&text, 0),
            Cmd::Primary(f) => {
                let value = clipboard.primary(Duration::from_millis(50));
                rt.queue.push_back(f(value));
            }
            Cmd::Focus(node) => rt
                .focus
                .set_focus(Some(&node), crate::window::focus::FocusCause::Programmatic),
            Cmd::Quit | Cmd::CloseWindow => rt.quit = true,
            // Outside `update`, after every queued message has been folded
            // (M5-D3). Not window-bound, so it is never returned to `run`.
            // Guarded: a panic in the closure must drop the task, not unwind
            // through and kill the whole event loop.
            Cmd::Task(f) => {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f())).is_err() {
                    tracing::error!("a Cmd::Task closure panicked; dropping it");
                }
            }
            // The window-bound commands are `run`'s (Task 17): only it has a
            // surface to title, minimise or open a popup on. They are handed
            // back to the caller rather than dropped here.
            other => unhandled.push(other),
        }
    }
    if folded {
        rebuild(app, rt, sheet, fonts, icons, clock);
        rebuild_popups(rt, sheet, fonts, icons, clock);
    }
    unhandled
}

/// The shortest of the four things that could want the next frame.
///
/// `Duration::ZERO` is a legitimate "now" -- an interpolating transition or
/// an already-due timer -- and must never be read as "spin".
fn frame_deadline<T>(
    window: Option<Duration>,
    animation: Option<Duration>,
    timers: &[(Duration, T)],
    controllers: Option<Duration>,
    now: Duration,
) -> Option<Duration> {
    let soonest_timer = timers.iter().map(|(at, _)| at.saturating_sub(now)).min();
    [window, animation, soonest_timer, controllers]
        .into_iter()
        .flatten()
        .min()
}

/// Append `lines` to `path`, under a `frame <n>` marker, when they differ
/// from `last`.
///
/// Deduplicated by content: a settled app writes nothing, so the file stays
/// bounded no matter how long the app runs, and a test that greps for a line
/// still finds it. `frame` only advances on an actual write, so a reader can
/// count it as "how many distinct states this app has published" (M5-D9,
/// deviation P1-D5).
fn write_probe_report(
    path: &std::path::Path,
    lines: &[String],
    last: &mut Vec<String>,
    frame: &mut u64,
) {
    use std::io::Write;

    if lines.is_empty() || lines == last.as_slice() {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        tracing::warn!(
            ?path,
            "could not open the probe report; skipping this frame's lines"
        );
        return;
    };
    let _ = writeln!(file, "frame {frame}");
    for line in lines {
        let _ = writeln!(file, "{line}");
    }
    // The dedup cache is committed only once the lines are really on disk: it
    // used to be updated before the fallible open, so a report that could not
    // be opened (a directory removed under it, a full disk) silently dropped
    // that state forever — the next differing batch was compared against
    // lines nobody ever wrote.
    if file.flush().is_ok() {
        last.clear();
        last.extend_from_slice(lines);
        *frame += 1;
    }
}

/// The report lines for one laid-out tree: every probe point, then one
/// `alloc` line per node that has an id.
fn probe_report_lines<Msg>(rt: &Runtime<Msg>) -> Vec<String> {
    let mut lines: Vec<String> = crate::window::probe_points_of(&rt.root, &rt.layout)
        .into_iter()
        .map(|p| format!("probe {} {} {}", p.label, p.x, p.y))
        .collect();
    for node in rt.root.descendants() {
        let Some(id) = node.id() else { continue };
        // A `display: none` subtree is not on screen, and a report that lists
        // a hidden page's widgets beside a visible one's is a report a gate
        // cannot aim a pointer from.
        if is_hidden(&rt.layout, &node) {
            continue;
        }
        let Some(alloc) = rt.layout.allocation(&node) else {
            continue;
        };
        let r = alloc.border_box;
        lines.push(format!(
            "alloc {} {} {} {} {}",
            id.as_str(),
            r.x,
            r.y,
            r.width,
            r.height
        ));
    }
    lines
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Run the loop against a live `Window`.
    ///
    /// Contract deviation D1: §4.7 writes `run(self, surface: Surface)`, but
    /// `Window` -- not `Surface` -- owns `pump`, `next_deadline`,
    /// `open_popup`, `clipboard` and the root node the loop needs, and there
    /// is no public way to get one from the other.
    ///
    /// Each iteration: wait up to `frame_deadline` for events, route them
    /// to controllers, fold the messages one at a time, run their commands,
    /// rebuild the view once, then restyle, relayout and repaint the dirty
    /// tree into the window's next buffer.
    ///
    /// # Errors
    ///
    /// [`AppError::Surface`] on a protocol or buffer failure,
    /// [`AppError::Layout`] if taffy fails.
    pub fn run(mut self, mut window: crate::window::Window) -> Result<(), AppError> {
        let sheet = self.sheet.take().unwrap_or_else(|| window.sheet().clone());
        let mut fonts = self
            .fonts
            .take()
            .unwrap_or_else(|| std::mem::take(window.fonts()));
        // The window already selected a theme from the environment; take it
        // rather than building a second one, so the two never disagree.
        let mut icons = self
            .icons
            .take()
            .unwrap_or_else(|| window.take_icon_theme());
        let clock: Rc<dyn Clock> = Rc::clone(window.clock());

        let mut rt = Runtime {
            root: window.root().clone(),
            instances: Vec::new(),
            styles: StyleMap::new(),
            anims: Animations::new(),
            containers: HashMap::new(),
            layout: LayoutTree::new(),
            focus: <FocusRing as Default>::default(),
            grab: ImplicitGrab::default(),
            hovered: None,
            focused: None,
            last_pointer: (0.0, 0.0),
            pointer_target: SurfaceTarget::Window,
            keyboard_target: SurfaceTarget::Window,
            queue: VecDeque::new(),
            cmds: Vec::new(),
            timers: Vec::new(),
            images: ImageCache::new(),
            env: ResolveEnv::default(),
            quit: false,
            popups: Vec::new(),
            next_popup_key: 0,
        };

        rebuild(&mut self, &mut rt, &sheet, &mut fonts, &mut icons, &clock);

        // M5-D9, P0-D4: lines written by `write_probe_report` only when they
        // differ from what is already here.
        let mut reported: Vec<String> = Vec::new();
        let mut probe_frame: u64 = 0;

        // M5-D2 §2: the inbox's wake pipe joins the window's poll set, and
        // leaves it on the way out.
        let inbox_watch = match self.inbox.as_ref().map(crate::view::inbox::Inbox::watch_fd) {
            Some(Ok(fd)) => Some(window.watch_fd(fd, crate::window::Interest::Read)),
            Some(Err(err)) => {
                tracing::warn!(%err, "the inbox pipe could not be registered; \
                                      its messages will only be seen on other wakeups");
                None
            }
            None => None,
        };

        // Bug fix (found while wiring `gallery::run`, the first real caller
        // of this loop against a live compositor): a page with no running
        // animation or key-repeat has `frame_deadline` return `None` on the
        // very first iteration, since nothing has painted yet to arm a
        // `wl_surface.frame` callback -- `window.next_deadline()` only knows
        // about deadlines a *previous* frame already scheduled. `pump(None)`
        // then blocks forever on a `configure` that already arrived inside
        // `Window::open`, so the window never paints its first frame. A
        // zero-duration wait on the first iteration only turns that block
        // into an immediate, empty poll -- the rest of the iteration runs
        // exactly as it would after any other real wakeup, restyles and
        // paints once, and `paint_with`'s own `wl_surface.frame` request
        // arms every iteration after this one.
        let mut first_iteration = true;
        while !rt.quit && !window.is_closed() {
            let now = clock.now();
            let mut wait = frame_deadline(
                window.next_deadline(),
                rt.anims.next_deadline(now),
                &rt.timers,
                next_controller_deadline(&rt.instances, now),
                now,
            );
            if std::mem::take(&mut first_iteration) {
                wait = Some(wait.unwrap_or(Duration::ZERO));
            }
            let events = window.pump(wait)?;

            // M5-D2 §3: inbox first, then `on_fd`, then the input batch.
            drain_inbox(&self, &mut rt);
            for event in &events {
                if let InputEvent::FdReady(id) = event {
                    for (watch, handler) in &self.fd_handlers {
                        if watch == id {
                            rt.queue.extend(handler());
                        }
                    }
                }
            }

            for event in &events {
                if matches!(event, InputEvent::Close) {
                    rt.quit = true;
                }
                // HiDPI: the output scale is what icons rasterise at.
                if let InputEvent::ScaleChanged(factor) = event {
                    icons.set_scale(u32::try_from(*factor).unwrap_or(1));
                }
                if let InputEvent::PopupDone(key) = event {
                    // The compositor has already destroyed the surface; drop
                    // our retained copy (and every popup opened after it, as
                    // xdg-shell requires) before telling the model.
                    close_popup(&mut rt, *key);
                    window.close_popup(*key);
                    if let Some(msg) = popup_msg(&self, PopupEvent::Dismissed(*key)) {
                        rt.queue.push_back(msg);
                    }
                }
                let produced = {
                    let clipboard = window.clipboard();
                    route(
                        &mut rt, event, &sheet, &mut fonts, &mut icons, clipboard, &clock,
                    )
                };
                rt.queue.extend(produced);
            }

            let now = clock.now();
            let due: Vec<Msg> = {
                let (fired, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut rt.timers)
                    .into_iter()
                    .partition(|(at, _)| *at <= now);
                rt.timers = pending;
                fired.into_iter().map(|(_, f)| f()).collect()
            };
            rt.queue.extend(due);

            let ticked = {
                let clipboard = window.clipboard();
                let mut cx = Dispatch {
                    styles: &rt.styles,
                    tree: &rt.layout,
                    focus: &mut rt.focus,
                    clipboard,
                    icons: &mut icons,
                    fonts: &mut fonts,
                    clock: &clock,
                    env: &rt.env,
                    cmds: &mut rt.cmds,
                };
                tick_all(&mut rt.instances, now, &mut cx)
            };
            rt.queue.extend(ticked);

            // The window-bound pass runs *after* `drain`, not before it:
            // `drain` is what calls `update`, so a `Cmd::SetTitle` an `update`
            // returns comes into existence inside `drain` and a filter placed
            // ahead of it would never see one. Controller-emitted commands
            // (`EventCx::cmds`) are already in `rt.cmds` and `drain` returns
            // them by the same path.
            let window_cmds = {
                let clipboard = window.clipboard();
                drain(
                    &mut self, &mut rt, &sheet, &mut fonts, &mut icons, clipboard, &clock, now,
                )
            };
            for cmd in window_cmds {
                match cmd {
                    Cmd::SetTitle(title) => window.set_title(&title),
                    Cmd::Minimize => window.minimize(),
                    Cmd::ToggleMaximized => window.toggle_maximized(),
                    Cmd::OpenPopup {
                        anchor,
                        positioner,
                        view,
                    } => match window.open_popup(anchor.clone(), positioner) {
                        Ok(key) => {
                            // The payload builds into the popup surface's own
                            // root, which `Window` created and owns.
                            if let Some(root) = window.popup_root(key) {
                                open_popup(
                                    &mut rt,
                                    key,
                                    root,
                                    &anchor,
                                    &positioner,
                                    &view,
                                    &sheet,
                                    &mut fonts,
                                    &mut icons,
                                    &clock,
                                );
                                window.popup_mark_dirty(key);
                            }
                            if let Some(msg) = popup_msg(&self, PopupEvent::Opened(key)) {
                                rt.queue.push_back(msg);
                            }
                        }
                        Err(error) => tracing::warn!(?error, "opening a popup failed"),
                    },
                    Cmd::ClosePopup(key) => {
                        close_popup(&mut rt, key);
                        window.close_popup(key);
                    }
                    // P0-D7: the only way out of a watch once `run` owns the
                    // window. An id `run` minted for the inbox is not one an
                    // app can name, and an unknown id is a no-op, so this can
                    // only retire a watch the app registered itself.
                    Cmd::Unwatch(id) => {
                        window.unwatch(id);
                        // Drop the handler too: leaving it in `fd_handlers`
                        // would keep firing a closure for a watch that no
                        // longer exists (and leak it for the app's lifetime).
                        self.fd_handlers.retain(|(w, _)| *w != id);
                    }
                    other => tracing::debug!(?other, "command not applicable to a window"),
                }
            }
            {
                // `Cmd::Focus` moves the ring inside `drain`; announce it too.
                let clipboard = window.clipboard();
                let moved = sync_focus(
                    &mut rt,
                    FocusCause::Programmatic,
                    &mut fonts,
                    &mut icons,
                    clipboard,
                    &clock,
                );
                rt.queue.extend(moved);
            }

            let (w, h) = window.size();
            restyle_and_layout(
                &rt.root,
                &mut rt.instances,
                &sheet,
                &rt.env,
                &mut fonts,
                &mut icons,
                &clock,
                &rt.containers,
                &mut rt.styles,
                &mut rt.anims,
                &mut rt.layout,
                (w, h),
            )?;

            if let Some(path) = self.probe_report.as_deref() {
                let lines = probe_report_lines(&rt);
                write_probe_report(path, &lines, &mut reported, &mut probe_frame);
            }

            let styles = &rt.styles;
            let layout = &rt.layout;
            let anims = &mut rt.anims;
            let images = &mut rt.images;
            let instances = &mut rt.instances;
            let root = &rt.root;
            let env = &rt.env;
            let fonts_ref = &mut fonts;
            let icons_ref = &mut icons;
            window.paint_with(|surface| {
                let mut cx = PaintCx {
                    env,
                    colors: &sheet.colors,
                    fonts: fonts_ref,
                    images,
                    icons: icons_ref,
                    text: None,
                };
                let mut painter = ControllerPainter { instances };
                let mut canvas = surface.canvas();
                paint_tree(
                    &mut canvas,
                    root,
                    styles,
                    layout,
                    anims,
                    now,
                    (0.0, 0.0),
                    &mut cx,
                    &mut painter,
                );
            })?;

            // Every open popup is its own surface with its own tree; the
            // window gives one buffer per popup through `paint_popup_with`
            // (contract §10 P6-D39's second limit, closed by P7).
            for index in 0..rt.popups.len() {
                let key = rt.popups[index].key;
                let Some((pw, ph)) = window.popup_size(key) else {
                    continue;
                };
                {
                    let popup = &mut rt.popups[index];
                    popup.size = (pw, ph);
                    restyle_and_layout(
                        &popup.root,
                        &mut popup.instances,
                        &sheet,
                        &rt.env,
                        &mut fonts,
                        &mut icons,
                        &clock,
                        &popup.containers,
                        &mut popup.styles,
                        &mut popup.anims,
                        &mut popup.layout,
                        (pw, ph),
                    )?;
                }
                let images = &mut rt.images;
                let env = &rt.env;
                let popup = &mut rt.popups[index];
                let fonts_ref = &mut fonts;
                let icons_ref = &mut icons;
                window.paint_popup_with(key, |surface| {
                    let mut cx = PaintCx {
                        env,
                        colors: &sheet.colors,
                        fonts: fonts_ref,
                        images,
                        icons: icons_ref,
                        text: None,
                    };
                    let mut painter = ControllerPainter {
                        instances: &mut popup.instances,
                    };
                    let mut canvas = surface.canvas();
                    paint_tree(
                        &mut canvas,
                        &popup.root,
                        &popup.styles,
                        &popup.layout,
                        &mut popup.anims,
                        now,
                        // A popup surface's own origin: the compositor placed
                        // the surface, so the tree starts at its top-left.
                        (0.0, 0.0),
                        &mut cx,
                        &mut painter,
                    );
                })?;
            }

            if let Some(hook) = self.frame_hook.as_mut() {
                // `Window::probe_points`/`allocation` read `Window`'s own
                // layout tree, which `App::run` otherwise never touches (its
                // reconcile loop keeps its own on `Runtime`, alive across
                // frames for taffy's incremental dirty tracking). Swap it in
                // for the span of the hook, then take it back.
                window.set_layout(std::mem::take(&mut rt.layout));
                // The same swap for every open popup, so a popover's own probe
                // points are not always empty under a windowed run (P5-D8,
                // authorised T15 extension). SWAP-SAFE: no early exit between
                // swap-in, hook and swap-out, so the loop's trees are always
                // restored. See [`swap_popup_layouts`].
                swap_popup_layouts(&mut rt, &mut window);
                hook(&window);
                swap_popup_layouts(&mut rt, &mut window);
                rt.layout = window.take_layout();
            }
        }
        if let Some(id) = inbox_watch {
            window.unwatch(id);
        }
        Ok(())
    }
}

/// Copy the surface out as tightly packed RGBA.
fn read_rgba(surface: &skia_rs_safe::canvas::Surface, size: (u32, u32)) -> Vec<u8> {
    let buffer = surface.pixel_buffer();
    let mut out = Vec::with_capacity((size.0 as usize) * (size.1 as usize) * 4);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let skia_rs_safe::core::Color(argb) = buffer
                .get_pixel(i32::try_from(x).unwrap_or(0), i32::try_from(y).unwrap_or(0))
                .unwrap_or(skia_rs_safe::core::Color(0));
            out.push(((argb >> 16) & 0xFF) as u8);
            out.push(((argb >> 8) & 0xFF) as u8);
            out.push((argb & 0xFF) as u8);
            out.push(((argb >> 24) & 0xFF) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::cascade::CompiledSheet;
    use crate::css::parse::parse_stylesheet_with_base;
    use crate::view::builders::widget;
    use crate::view::reconcile::reconcile;
    use crate::view::{BuildCx, Handler, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Outer,
        Inner,
    }

    #[allow(
        clippy::type_complexity,
        reason = "a test fixture's tuple return, not a public API"
    )]
    fn tree() -> (
        CompiledSheet,
        crate::text::FontDatabase,
        crate::icons::IconTheme,
        Rc<dyn Clock>,
        ResolveEnv,
        Node,
        Vec<Instance<Msg>>,
    ) {
        let sheet = CompiledSheet::compile("box { color: #000; } button { color: #111; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<Msg>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<Msg>(Kind::Box)
                        .key("outer")
                        .on(crate::view::EventKind::Click, Handler::Unit(Msg::Outer))
                        .child(
                            widget::<Msg>(Kind::Button)
                                .key("inner")
                                .prop(PropName::Label, "Go")
                                .on(crate::view::EventKind::Click, Handler::Unit(Msg::Inner)),
                        ),
                ],
                &mut cx,
            );
        }
        (sheet, fonts, icons, clock, env, root, instances)
    }

    #[test]
    fn path_to_is_outermost_first_and_ends_at_the_target() {
        let (_s, _f, _i, _c, _e, _root, instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        assert_eq!(path.len(), 2);
        assert!(path[0].ptr_eq(&instances[0].node));
        assert!(path[1].ptr_eq(&inner));
        assert!(path_to(&instances, &Node::new("stranger")).is_empty());
    }

    #[test]
    fn a_click_on_the_inner_node_bubbles_to_the_outer_one() {
        let (sheet, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);

        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        let _ = &sheet;

        deliver(
            &mut instances,
            &path,
            &Event::PointerDown {
                button: crate::window::layer::BTN_LEFT,
                local: (1.0, 1.0),
                serial: 1,
            },
            &mut cx,
        );
        let out = deliver(
            &mut instances,
            &path,
            &Event::PointerUp {
                button: crate::window::layer::BTN_LEFT,
                local: (1.0, 1.0),
                serial: 2,
            },
            &mut cx,
        );
        // The inner button handled it and stopped the target phase; the
        // outer box never saw a press, so it produces nothing on bubble.
        assert_eq!(out, vec![Msg::Inner]);
    }

    #[test]
    fn an_empty_path_delivers_nothing_and_does_not_panic() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        assert!(deliver(&mut instances, &[], &Event::PointerLeave, &mut cx).is_empty());
    }

    #[test]
    fn messages_come_back_in_dispatch_order_target_before_bubble() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        // `Activate` is not handled-stopping unless a handler fires, and both
        // nodes bind Click, so the target answers first and the ancestor
        // second.
        let out = deliver(&mut instances, &path, &Event::Activate, &mut cx);
        assert_eq!(out.first(), Some(&Msg::Inner));
    }

    /// An ancestor's pointer handlers see the point in the *ancestor's* own
    /// frame, not the frame of whatever child the event was aimed at.
    ///
    /// Mutation check: hand the same `Event` to every node on the path again
    /// (drop `deliver`'s `localised` rewrite) and the outer node reports the
    /// inner node's `x`, ten pixels off -- which is how a release five pixels
    /// past a button's right edge still passed the button's own `0..width`
    /// bounds check, and one in its left padding arrived negative.
    #[test]
    fn bubbling_gives_each_ancestor_the_point_in_its_own_frame() {
        #[derive(Debug, Clone, PartialEq)]
        enum P {
            Outer(i64),
            Inner(i64),
        }

        // 10px of padding on the outer box is the offset between the two
        // frames; without it the test could not tell them apart.
        let sheet = CompiledSheet::compile("box { padding: 10px; } button { padding: 0; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<P>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<P>(Kind::Box)
                        .key("outer")
                        .on_pointer_up(|x, _y| P::Outer(x as i64))
                        .child(
                            widget::<P>(Kind::Box)
                                .key("inner")
                                .on_pointer_up(|x, _y| P::Inner(x as i64)),
                        ),
                ],
                &mut cx,
            );
        }
        let outer = instances[0].node.clone();
        let inner = instances[0].children[0].node.clone();

        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        crate::view::render::restyle_tree(
            &root,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        let mut containers: HashMap<crate::view::NodeAddr, crate::layout::Container> =
            HashMap::new();
        for addr in styles.keys() {
            containers.insert(
                *addr,
                crate::layout::Container::Box {
                    direction: crate::layout::BoxDirection::Column,
                },
            );
        }
        let mut layout = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 60.0,
            height: 32.0,
        });
        crate::view::render::layout_tree(
            &root,
            &styles,
            &containers,
            &mut layout,
            &env,
            (Some(200.0), Some(200.0)),
            &mut measure,
        )
        .expect("the tree lays out");

        let outer_box = layout.allocation(&outer).expect("outer").border_box;
        let inner_box = layout.allocation(&inner).expect("inner").border_box;
        let shift = inner_box.x - outer_box.x;
        assert!(shift > 0.0, "the two frames really are offset");

        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<P>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        let path = path_to(&instances, &inner);
        let out = deliver(
            &mut instances,
            &path,
            &Event::PointerUp {
                button: crate::window::layer::BTN_LEFT,
                local: (5.0, 5.0),
                serial: 1,
            },
            &mut cx,
        );
        assert_eq!(
            out,
            vec![P::Inner(5), P::Outer(5 + shift as i64)],
            "the target keeps its own point; the ancestor is handed its own"
        );
    }

    /// A `Runtime` over `root`/`instances` with everything else empty.
    fn runtime<Msg>(root: Node, instances: Vec<Instance<Msg>>) -> Runtime<Msg> {
        Runtime {
            root,
            instances,
            styles: StyleMap::new(),
            anims: Animations::new(),
            containers: HashMap::new(),
            layout: LayoutTree::new(),
            focus: <FocusRing as Default>::default(),
            grab: ImplicitGrab::default(),
            hovered: None,
            focused: None,
            last_pointer: (0.0, 0.0),
            pointer_target: SurfaceTarget::Window,
            keyboard_target: SurfaceTarget::Window,
            queue: VecDeque::new(),
            cmds: Vec::new(),
            timers: Vec::new(),
            images: ImageCache::new(),
            env: ResolveEnv::default(),
            quit: false,
            popups: Vec::new(),
            next_popup_key: 0,
        }
    }

    /// Nothing but [`sync_focus`] constructs `Event::FocusIn`/`FocusOut`, so
    /// without it `View::on_focus_in`/`on_focus_out` can never fire.
    #[test]
    fn moving_the_focus_fires_focus_out_then_focus_in() {
        #[derive(Debug, Clone, PartialEq)]
        enum F {
            In,
            Out,
        }

        let sheet = CompiledSheet::compile("button { color: #000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<F>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            // `Kind::MenuButton`: still `GenericC` (unlike `Kind::Button`
            // since Task 19), whose fallback focus handling this test means
            // to exercise — no P5/P6 widget controller forwards
            // `FocusIn`/`FocusOut` on its own.
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<F>(Kind::MenuButton).key("a").on_focus_out(F::Out),
                    widget::<F>(Kind::MenuButton).key("b").on_focus_in(F::In),
                ],
                &mut cx,
            );
        }
        let a = instances[0].node.clone();
        let b = instances[1].node.clone();
        let mut rt = runtime(root, instances);
        let mut clipboard = crate::window::selection::Clipboard::offscreen();

        // No move yet: nothing is announced.
        assert!(
            sync_focus(
                &mut rt,
                FocusCause::Programmatic,
                &mut fonts,
                &mut icons,
                &mut clipboard,
                &clock,
            )
            .is_empty()
        );

        rt.focus.set_focus(Some(&a), FocusCause::Keyboard);
        let first = sync_focus(
            &mut rt,
            FocusCause::Keyboard,
            &mut fonts,
            &mut icons,
            &mut clipboard,
            &clock,
        );
        // `a` binds only `on_focus_out`, so entering it says nothing.
        assert!(first.is_empty());

        rt.focus.set_focus(Some(&b), FocusCause::Keyboard);
        let moved = sync_focus(
            &mut rt,
            FocusCause::Keyboard,
            &mut fonts,
            &mut icons,
            &mut clipboard,
            &clock,
        );
        // Out of the old node first, then into the new one.
        assert_eq!(moved, vec![F::Out, F::In]);

        // Announced once, not once per call.
        assert!(
            sync_focus(
                &mut rt,
                FocusCause::Keyboard,
                &mut fonts,
                &mut icons,
                &mut clipboard,
                &clock,
            )
            .is_empty()
        );
    }

    /// A report that could not be opened must not poison the dedup cache: the
    /// lines it failed to write are still owed.
    ///
    /// Mutation check: commit `last` before the open again and the second
    /// call writes nothing, because the failed first call already claimed
    /// those lines. Restore.
    #[test]
    fn a_probe_report_that_cannot_be_opened_keeps_owing_its_lines() {
        let dir = std::env::temp_dir().join(format!("icedtea-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let lines = vec!["probe root 1 2".to_string()];
        let mut last: Vec<String> = Vec::new();
        let mut frame = 0;

        // A directory is never openable for append.
        write_probe_report(&dir, &lines, &mut last, &mut frame);
        assert!(last.is_empty(), "nothing was written, so nothing is cached");
        assert_eq!(frame, 0);

        let file = dir.join("report");
        write_probe_report(&file, &lines, &mut last, &mut frame);
        assert_eq!(frame, 1);
        let text = std::fs::read_to_string(&file).expect("the report exists");
        assert!(text.contains("probe root 1 2"), "{text:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A widget hidden with `display: none` — a `Stack` page the user left —
    /// must not keep the keyboard focus, or every keystroke goes on landing in
    /// an invisible `Entry` on a page nobody can see.
    ///
    /// Mutation check: drop `route`'s `prune_hidden_focus` call and the key is
    /// delivered to the hidden node (`vec![K::Typed]`) with the focus still on
    /// it. Restore.
    #[test]
    fn a_key_never_reaches_a_widget_that_has_been_hidden() {
        #[derive(Debug, Clone, PartialEq)]
        enum K {
            Typed,
        }

        let sheet = CompiledSheet::compile("box { color: #000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<K>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<K>(Kind::Box).key("page").child(
                        widget::<K>(Kind::Box)
                            .key("field")
                            .on_key(|_| Some(K::Typed)),
                    ),
                ],
                &mut cx,
            );
        }
        let page = instances[0].node.clone();
        let field = instances[0].children[0].node.clone();

        let mut rt = runtime(root, instances);
        // What `StackC::set_visible` records when a page stops being visible.
        crate::widgets::set_displayed(&page, false);
        rt.layout.sync(&rt.root).expect("sync");
        crate::widgets::flush_layout(&mut rt.layout);
        assert!(is_hidden(&rt.layout, &field), "the field is buried");

        rt.focus.set_focus(Some(&field), FocusCause::Pointer);
        let key = crate::window::keyboard::Keymap::from_string(include_str!(
            "../../tests/fixtures/keymaps/us.xkb"
        ))
        .expect("the vendored us keymap compiles")
        .translate(38, true, 1, 0); // evdev 38 == `a`

        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let out = route(
            &mut rt,
            &InputEvent::Key(key),
            &sheet,
            &mut fonts,
            &mut icons,
            &mut clipboard,
            &clock,
        );
        assert_eq!(out, Vec::new(), "the hidden field never saw the key");
        assert!(
            rt.focus.focus().is_none(),
            "and it does not still own the focus"
        );
    }

    /// `drain` is what calls `update`, so a window command an `update` returns
    /// only exists once `drain` is running: it must come back out, not be
    /// dropped with a "no effect offscreen" log.
    #[test]
    fn a_window_command_returned_by_update_survives_drain() {
        #[derive(Debug, Clone, PartialEq)]
        struct Title;

        fn update(_model: &mut (), _msg: Title) -> Cmd<Title> {
            Cmd::Batch(vec![Cmd::SetTitle("counted".into()), Cmd::Minimize])
        }
        fn view(_model: &()) -> crate::view::View<Title> {
            widget::<Title>(Kind::Box).key("root")
        }

        let mut app = App::new((), update, view);
        let sheet = CompiledSheet::compile("box { color: #000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut rt = runtime(Node::new("window"), Vec::new());
        rt.queue.push_back(Title);

        let unhandled = drain(
            &mut app,
            &mut rt,
            &sheet,
            &mut fonts,
            &mut icons,
            &mut clipboard,
            &clock,
            Duration::ZERO,
        );
        assert!(
            matches!(unhandled.as_slice(), [Cmd::SetTitle(t), Cmd::Minimize] if &**t == "counted")
        );
        // And the ones `drain` can run are still consumed there.
        assert!(rt.cmds.is_empty());
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Count {
        Inc,
        #[expect(
            dead_code,
            reason = "the plan's third message: `update` folds it, no test sends it"
        )]
        Dec,
        Set(i32),
    }

    #[derive(Debug)]
    struct Model {
        n: i32,
    }

    fn update(model: &mut Model, msg: Count) -> Cmd<Count> {
        match msg {
            Count::Inc => model.n += 1,
            Count::Dec => model.n -= 1,
            Count::Set(v) => model.n = v,
        }
        Cmd::None
    }

    fn counter_view(model: &Model) -> crate::view::View<Count> {
        widget::<Count>(Kind::Box)
            .key("root")
            .child(
                widget::<Count>(Kind::Button)
                    .key("plus")
                    .prop(PropName::Label, "+")
                    .on_click(Count::Inc),
            )
            .child(
                widget::<Count>(Kind::Label)
                    .key("value")
                    .prop(PropName::Label, model.n.to_string()),
            )
    }

    fn counter_app() -> App<Model, Count> {
        App::new(Model { n: 0 }, update, counter_view)
            .with_sheet(CompiledSheet::compile(
                "window { background-color: #ffffff; }
                 box { background-color: #ffffff; }
                 button { background-color: #3584e4; min-width: 20px; min-height: 20px; }
                 label { color: #000000; }",
            ))
            .with_fonts(crate::text::FontDatabase::probe_only())
            .with_icons(crate::icons::IconTheme::with_name_and_roots(
                "hicolor",
                vec![],
            ))
    }

    #[test]
    fn a_scripted_message_folds_through_update_and_reaches_the_view() {
        let clock = Rc::new(crate::anim::ManualClock::new());
        let frames = counter_app()
            .run_offscreen(
                (80, 40),
                Rc::clone(&clock),
                vec![
                    ScriptStep::Capture,
                    ScriptStep::Message(Count::Set(7)),
                    ScriptStep::Capture,
                ],
            )
            .expect("the offscreen loop runs");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames.size(), (80, 40));
        // Two different models must not paint identical frames.
        let a: Vec<Option<(u8, u8, u8, u8)>> = (0..80).map(|x| frames.pixel(0, x, 20)).collect();
        let b: Vec<Option<(u8, u8, u8, u8)>> = (0..80).map(|x| frames.pixel(1, x, 20)).collect();
        assert_ne!(a, b, "the label did not repaint after the model changed");
    }

    #[test]
    fn identical_models_paint_identical_pixels() {
        let run = || {
            counter_app()
                .run_offscreen(
                    (60, 30),
                    Rc::new(crate::anim::ManualClock::new()),
                    vec![ScriptStep::Message(Count::Set(3)), ScriptStep::Capture],
                )
                .expect("runs")
        };
        let first = run();
        let second = run();
        for y in 0..30 {
            for x in 0..60 {
                assert_eq!(
                    first.pixel(0, x, y),
                    second.pixel(0, x, y),
                    "pixel ({x}, {y}) differed between two runs of one model"
                );
            }
        }
    }

    #[test]
    fn a_message_produced_by_update_is_queued_never_folded_re_entrantly() {
        // `Cmd::After(ZERO)` fires on the next clock advance, not inside
        // `update` — the contract's "queued, never nested".
        fn update_after(model: &mut Model, msg: Count) -> Cmd<Count> {
            match msg {
                Count::Inc => {
                    model.n += 1;
                    if model.n < 3 {
                        return Cmd::After(Duration::ZERO, Rc::new(|| Count::Inc));
                    }
                    Cmd::None
                }
                Count::Dec => {
                    model.n -= 1;
                    Cmd::None
                }
                Count::Set(v) => {
                    model.n = v;
                    Cmd::None
                }
            }
        }

        let app = App::new(Model { n: 0 }, update_after, counter_view)
            .with_sheet(CompiledSheet::compile("box { color: #000; }"))
            .with_fonts(crate::text::FontDatabase::probe_only())
            .with_icons(crate::icons::IconTheme::with_name_and_roots(
                "hicolor",
                vec![],
            ));
        let frames = app
            .run_offscreen(
                (40, 20),
                Rc::new(crate::anim::ManualClock::new()),
                // Reconciliation: the plan captured once at the end, and a
                // frame count of 1 is the same whether the three `Inc`s were
                // folded one per clock step or all three re-entrantly inside
                // the first `update` — it cannot kill the mutation the plan
                // asks for. Capturing after every step can: queued folding
                // paints 1, then 2, then 3, so the three frames differ;
                // re-entrant folding paints 3 three times.
                vec![
                    ScriptStep::Message(Count::Inc),
                    ScriptStep::Capture,
                    ScriptStep::Advance(Duration::from_millis(1)),
                    ScriptStep::Capture,
                    ScriptStep::Advance(Duration::from_millis(1)),
                    ScriptStep::Capture,
                ],
            )
            .expect("runs");
        assert_eq!(frames.len(), 3);
        let row = |frame: usize| -> Vec<Option<(u8, u8, u8, u8)>> {
            (0..20)
                .flat_map(|y| (0..40).map(move |x| (x, y)))
                .map(|(x, y)| frames.pixel(frame, x, y))
                .collect()
        };
        assert_ne!(row(0), row(1), "the second Inc did not wait for the clock");
        assert_ne!(row(1), row(2), "the third Inc did not wait for the clock");
    }

    #[test]
    fn the_frame_deadline_folds_animations_timers_and_controllers() {
        // `App::run`'s wait is `min(window deadline, animation deadline,
        // timer deadline, controller deadline)`. The pure computation is
        // exercised here; the socket half is Task 18's e2e.
        let now = Duration::from_millis(100);
        assert_eq!(
            frame_deadline(
                Some(Duration::from_millis(16)),
                Some(Duration::from_millis(8)),
                &[(Duration::from_millis(150), ())],
                Some(Duration::from_millis(30)),
                now,
            ),
            Some(Duration::from_millis(8))
        );
        // A timer already due is "now", not "spin": ZERO is a legitimate
        // answer and the loop must not treat it as an error.
        assert_eq!(
            frame_deadline(None, None, &[(Duration::from_millis(50), ())], None, now),
            Some(Duration::ZERO)
        );
        // Nothing pending: block until an event arrives.
        assert_eq!(frame_deadline::<()>(None, None, &[], None, now), None);
    }

    #[test]
    fn a_synthetic_click_reaches_the_button_s_controller() {
        let frames = counter_app()
            .run_offscreen(
                (80, 40),
                Rc::new(crate::anim::ManualClock::new()),
                vec![
                    ScriptStep::Capture,
                    // Reconciliation: the plan's (6, 10) is outside every
                    // widget. M2's box centres its children, so in an 80x40
                    // surface the button's border box is (26, 10)-(46, 30);
                    // (30, 20) is inside it.
                    ScriptStep::Event(crate::window::InputEvent::PointerEnter {
                        x: 30.0,
                        y: 20.0,
                        serial: 1,
                        target: crate::window::SurfaceTarget::Window,
                    }),
                    ScriptStep::Event(crate::window::InputEvent::PointerButton {
                        button: crate::window::layer::BTN_LEFT,
                        pressed: true,
                        serial: 2,
                        time_ms: 0,
                    }),
                    ScriptStep::Event(crate::window::InputEvent::PointerButton {
                        button: crate::window::layer::BTN_LEFT,
                        pressed: false,
                        serial: 3,
                        time_ms: 10,
                    }),
                    ScriptStep::Capture,
                ],
            )
            .expect("runs");
        assert_eq!(frames.len(), 2);
        // Reconciliation: the plan sampled row y = 30, which this layout
        // leaves blank (the button owns y < 20 and the label's glyphs sit
        // just under it). The whole frame is compared instead, which is a
        // strictly stronger form of the same assertion.
        let whole = |frame: usize| -> Vec<Option<(u8, u8, u8, u8)>> {
            (0..40)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .map(|(x, y)| frames.pixel(frame, x, y))
                .collect()
        };
        assert_ne!(
            whole(0),
            whole(1),
            "the click never incremented the counter"
        );
    }

    /// `probe` must run the *same* pipeline `run` does — view, reconcile,
    /// restyle, layout — or the gate's coordinates would describe a tree
    /// nobody renders.
    ///
    /// Mutation check: make `probe` skip its `compute` call; every allocation
    /// comes back 0x0 and this test fails. Restore.
    #[test]
    fn probe_lays_the_tree_out_and_reports_allocations() {
        let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let app = App::new(
            0u32,
            |_m: &mut u32, _msg: ()| Cmd::None,
            |_m: &u32| crate::view::builders::label("probe me"),
        );
        let sheet = CompiledSheet::from_stylesheet(parse_stylesheet_with_base(
            crate::BUNDLED_ADWAITA_LIGHT,
            None,
        ));
        let probe = app
            .probe(
                (200, 100),
                sheet,
                FontDatabase::new(),
                IconTheme::with_name_and_roots("hicolor", Vec::new()),
                clock,
            )
            .expect("probe lays out");
        assert_eq!(probe.instances().len(), 1);
        let alloc = probe
            .allocation(&probe.instances()[0].node)
            .expect("the root has an allocation");
        assert!(
            alloc.border_box.width > 0.0 && alloc.border_box.height > 0.0,
            "a label with text must have a non-empty allocation, got {:?}",
            alloc.border_box
        );
    }

    // M6-FUP1: `App::probe` must settle to the same fixpoint a running app
    // converges to, not stop after one tick. The gallery page's `column_view`
    // grows its inner list's pool over several ticks (`ListViewC::adopt_metrics`),
    // so a single-tick probe reports every widget below it (here `popover_menu_bar`,
    // css name `menubar`) ~100px above where `run_offscreen` (which ticks every
    // frame) settles it. Compare the probe's floored-centre for that widget against
    // the settled `run_offscreen` probe report — in process, ~seconds, versus the
    // minutes-long full gallery_gate walk.
    //
    // Mutation check: revert `App::probe` to a single `tick_all` + one relayout
    // and this fails -- the probe centre is ~100px above the settled centre.
    #[test]
    fn probe_settles_the_gallery_page_like_a_running_app() {
        use crate::gallery::{GalleryModel, GalleryMsg, Theme, scrolled_page, update};

        let size = (1280, 720);
        // popover_menu_bar's root css name — a uniquely-named widget below the
        // gallery's column_view, so its probe label is just "menubar".
        let target = "menubar";

        // Settled reference: run_offscreen ticks every frame. Over-drive it and
        // read the last `probe <target> <x> <y>` (floored centre) line it wrote.
        let report = std::env::temp_dir().join(format!(
            "icedtea-m6fup1-{}-{}.probe",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let clock = Rc::new(ManualClock::new());
        let script: Vec<ScriptStep<GalleryMsg>> = (0..16)
            .map(|_| ScriptStep::Advance(std::time::Duration::from_millis(16)))
            .chain(std::iter::once(ScriptStep::Capture))
            .collect();
        App::new(GalleryModel::new(Theme::Light, None), update, scrolled_page)
            .with_probe_report(report.clone())
            .run_offscreen(size, clock, script)
            .expect("run_offscreen lays out");
        let text = std::fs::read_to_string(&report).expect("probe report written");
        let _ = std::fs::remove_file(&report);
        let settled_y: i32 = text
            .lines()
            .rev()
            .find_map(|l| {
                let mut f = l.split_whitespace();
                (f.next() == Some("probe") && f.next() == Some(target))
                    .then(|| f.nth(1).and_then(|s| s.parse::<i32>().ok()))
                    .flatten()
            })
            .expect("run_offscreen reported a probe point for the target");

        // Probe the same page, matching run_offscreen's default sheet/fonts/icons,
        // and compute the target's floored centre the same way `probe_points_of` does.
        let dyn_clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let probe = App::new(GalleryModel::new(Theme::Light, None), update, scrolled_page)
            .probe(
                size,
                CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT),
                FontDatabase::default(),
                IconTheme::from_env(),
                dyn_clock,
            )
            .expect("probe lays out");
        let node = probe
            .root()
            .descendants()
            .find(|n| n.name().as_ref() == target)
            .expect("the page has the target widget");
        let r = probe
            .allocation(&node)
            .expect("the target has an allocation")
            .border_box;
        let probe_y = (r.y + r.height / 2.0).floor() as i32;

        assert_eq!(
            probe_y, settled_y,
            "App::probe put {target}'s centre at y={probe_y}, but a settled run has \
             it at y={settled_y} -- the probe did not settle to the fixpoint"
        );
    }
}
