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
use crate::view::{Kind, View};
use crate::window::focus::{FocusCause, FocusRing};
use crate::window::pointer::{ImplicitGrab, hit_chain};
use crate::window::popup::{PopupAnchorPoint, PopupKey, Positioner};
use crate::window::selection::Clipboard;
use crate::window::{InputEvent, SurfaceError};

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

    'phases: for phase in Phase::ALL {
        let order: Vec<usize> = match phase {
            Phase::Capture => (0..last).collect(),
            Phase::Target => vec![last],
            Phase::Bubble => (0..last).rev().collect(),
        };
        for index in order {
            let node = path[index].clone();
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

/// The Elm loop over a retained tree.
pub struct App<M, Msg> {
    model: M,
    update: fn(&mut M, Msg) -> Cmd<Msg>,
    view: fn(&M) -> View<Msg>,
    sheet: Option<CompiledSheet>,
    fonts: Option<FontDatabase>,
    icons: Option<IconTheme>,
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
        let measured = self
            .instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
            .and_then(|instance| {
                instance.controller.measure(
                    (
                        cap(known.width, available.width),
                        cap(known.height, available.height),
                    ),
                    &mut cx,
                )
            });
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
        self.instances
            .iter_mut()
            .find_map(|root| root.find_mut(node))
            .is_some_and(|instance| instance.controller.paint(canvas, alloc, style, cx))
    }
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// A new app over `model`, folded by `update`, described by `view`.
    #[must_use]
    pub fn new(model: M, update: fn(&mut M, Msg) -> Cmd<Msg>, view: fn(&M) -> View<Msg>) -> Self {
        App {
            model,
            update,
            view,
            sheet: None,
            fonts: None,
            icons: None,
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

    /// The current model — what an offscreen test asserts on besides pixels.
    #[must_use]
    pub fn model(&self) -> &M {
        &self.model
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
        )?;

        for step in script {
            let mut capture = false;
            match step {
                ScriptStep::Capture => capture = true,
                ScriptStep::Message(msg) => rt.queue.push_back(msg),
                ScriptStep::Event(ev) => {
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

/// Restyle, relayout and repaint into `surface`.
fn render_once<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clock: &Rc<dyn Clock>,
    size: (u32, u32),
    surface: &mut skia_rs_safe::canvas::Surface,
) -> Result<(), AppError> {
    let now = clock.now();
    restyle_tree(&rt.root, sheet, &rt.env, &mut rt.styles, &mut rt.anims, now);
    {
        let mut measure = ControllerMeasure {
            instances: &mut rt.instances,
            sheet,
            fonts,
            icons,
            clock,
            env: &rt.env,
        };
        layout_tree(
            &rt.root,
            &rt.styles,
            &rt.containers,
            &mut rt.layout,
            &rt.env,
            (Some(size.0 as f32), Some(size.1 as f32)),
            &mut measure,
        )?;
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
            restyle_tree(
                &popup.root,
                sheet,
                &rt.env,
                &mut popup.styles,
                &mut popup.anims,
                now,
            );
            let mut measure = ControllerMeasure {
                instances: &mut popup.instances,
                sheet,
                fonts,
                icons,
                clock,
                env: &rt.env,
            };
            layout_tree(
                &popup.root,
                &popup.styles,
                &popup.containers,
                &mut popup.layout,
                &rt.env,
                (Some(width as f32), Some(height as f32)),
                &mut measure,
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

/// Turn one `InputEvent` into controller events and return the messages.
#[allow(
    clippy::too_many_arguments,
    reason = "one routing pass threading the whole per-frame context"
)]
fn route<Msg: Clone + 'static>(
    rt: &mut Runtime<Msg>,
    event: &InputEvent,
    _sheet: &CompiledSheet,
    fonts: &mut FontDatabase,
    icons: &mut IconTheme,
    clipboard: &mut Clipboard,
    clock: &Rc<dyn Clock>,
) -> Vec<Msg> {
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
            if let Some(node) = rt.focus.focus() {
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
            // The window-bound commands are `run`'s (Task 17): only it has a
            // surface to title, minimise or open a popup on. They are handed
            // back to the caller rather than dropped here.
            other => unhandled.push(other),
        }
    }
    if folded {
        rebuild(app, rt, sheet, fonts, icons, clock);
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

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Run the loop against a live `Window`.
    ///
    /// Contract deviation D1: §4.7 writes `run(self, surface: Surface)`, but
    /// `Window` -- not `Surface` -- owns `pump`, `next_deadline`,
    /// `open_popup`, `clipboard` and the root node the loop needs, and there
    /// is no public way to get one from the other.
    ///
    /// Each iteration: wait up to [`frame_deadline`] for events, route them
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
        let mut icons = self.icons.take().unwrap_or_else(IconTheme::from_env);
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

        while !rt.quit && !window.is_closed() {
            let now = clock.now();
            let wait = frame_deadline(
                window.next_deadline(),
                rt.anims.next_deadline(now),
                &rt.timers,
                next_controller_deadline(&rt.instances, now),
                now,
            );
            let events = window.pump(wait)?;

            for event in &events {
                if matches!(event, InputEvent::Close) {
                    rt.quit = true;
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
                        }
                        Err(error) => tracing::warn!(?error, "opening a popup failed"),
                    },
                    Cmd::ClosePopup(key) => {
                        close_popup(&mut rt, key);
                        window.close_popup(key);
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
            restyle_tree(
                &rt.root,
                &sheet,
                &rt.env,
                &mut rt.styles,
                &mut rt.anims,
                now,
            );
            {
                let mut measure = ControllerMeasure {
                    instances: &mut rt.instances,
                    sheet: &sheet,
                    fonts: &mut fonts,
                    icons: &mut icons,
                    clock: &clock,
                    env: &rt.env,
                };
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a surface is never 2^24 px on a side"
                )]
                layout_tree(
                    &rt.root,
                    &rt.styles,
                    &rt.containers,
                    &mut rt.layout,
                    &rt.env,
                    (Some(w as f32), Some(h as f32)),
                    &mut measure,
                )?;
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
}
