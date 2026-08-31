//! The GTK 4.22 widget set: one file per widget, each a builder, a controller
//! and a vendored CSS-node-tree fixture.
//!
//! A widget is not an object with setters. It is a [`Kind`] in a
//! [`crate::view::View`] tree (`view::builders`), an
//! [`crate::view::Instance`] the reconciler keeps alive, and a
//! [`Controller`] that owns the behaviour state the application model must
//! not: press state, a text cursor, a scroll offset, an open popover. The
//! controller builds its own CSS subnodes under the root [`Node`] the
//! reconciler hands it and mutates them in `set_prop`, `on_event` and `tick`;
//! painting is M2's `paint_node_with_children` unless the widget draws
//! something CSS cannot express.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event};
use crate::view::{BuildCx, Kind, Prop, PropName, Props};
use crate::window::focus::FOCUSABLE_CLASS;

#[doc(inline)]
pub use crate::view::ListItem;

pub mod label;
pub mod level_bar;
pub mod progress_bar;
pub mod separator;
pub mod spinner;
pub mod statusbar;

/// A widget-local enum carried through `Prop::Enum(u16)`.
///
/// [`Prop`] cannot hold an arbitrary type, and the contract's
/// `Prop::Enum(u16)` is deliberately opaque; this trait is the only sanctioned
/// way in or out of it, so a mis-cast enum is a compile error rather than a
/// silent 0.
pub trait WidgetEnum: Copy + Sized {
    /// The discriminant.
    fn to_u16(self) -> u16;
    /// The variant, or `None` for a discriminant this enum does not have.
    fn from_u16(raw: u16) -> Option<Self>;

    /// Read from a prop slot, falling back to `default` for a missing prop or
    /// an out-of-range discriminant.
    fn from_prop(prop: Option<&Prop>, default: Self) -> Self {
        match prop {
            Some(Prop::Enum(raw)) => Self::from_u16(*raw).unwrap_or(default),
            _ => default,
        }
    }

    /// Wrap for storage in a [`Props`].
    fn to_prop(self) -> Prop {
        Prop::Enum(self.to_u16())
    }
}

/// Declare a widget-local enum plus its [`WidgetEnum`] impl and CSS class
/// names.
macro_rules! widget_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $( $(#[$vmeta:meta])* $variant:ident = $class:expr ),+ $(,)? }
        default = $default:ident;
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $( $(#[$vmeta])* $variant ),+ }

        impl Default for $name {
            fn default() -> Self { $name::$default }
        }

        impl $name {
            /// The GTK style class this variant contributes, or `""`.
            #[must_use]
            pub fn css_class(self) -> &'static str {
                match self { $( $name::$variant => $class ),+ }
            }
            /// Every variant, in declaration order.
            #[must_use]
            pub fn all() -> &'static [$name] {
                &[ $( $name::$variant ),+ ]
            }
        }

        impl WidgetEnum for $name {
            fn to_u16(self) -> u16 {
                let mut index = 0u16;
                $( if matches!(self, $name::$variant) { return index; } index += 1; )+
                let _ = index;
                0
            }
            fn from_u16(raw: u16) -> Option<Self> {
                let mut index = 0u16;
                $( if raw == index { return Some($name::$variant); } index += 1; )+
                let _ = index;
                None
            }
        }
    };
}

widget_enum! {
    /// `GtkOrientable:orientation`.
    pub enum Orientation { Horizontal = "horizontal", Vertical = "vertical" }
    default = Horizontal;
}

widget_enum! {
    /// `GtkPositionType` — where a value, a mark or a popover sits.
    pub enum Position { Left = "left", Right = "right", Top = "top", Bottom = "bottom" }
    default = Bottom;
}

widget_enum! {
    /// `GtkPackType` — which half of a decoration layout a control belongs to.
    pub enum Side { Start = "start", End = "end" }
    default = Start;
}

widget_enum! {
    /// `GtkIconSize`. `Inherit` adds no class, matching GTK.
    pub enum IconSize { Inherit = "", Normal = "normal-icons", Large = "large-icons" }
    default = Inherit;
}

widget_enum! {
    /// `GtkMessageType`. `Other` adds no class.
    pub enum MessageType {
        Info = "info", Warning = "warning", Question = "question",
        Error = "error", Other = "",
    }
    default = Info;
}

widget_enum! {
    /// `GtkLevelBarMode`.
    pub enum LevelBarMode { Continuous = "continuous", Discrete = "discrete" }
    default = Continuous;
}

widget_enum! {
    /// `GtkContentFit`.
    pub enum ContentFit { Fill = "", Contain = "", Cover = "", ScaleDown = "" }
    default = Contain;
}

widget_enum! {
    /// `GtkStringFilterMatchMode`, used by `DropDown`'s search.
    pub enum MatchMode { Exact = "", Substring = "", Prefix = "" }
    default = Substring;
}

widget_enum! {
    /// `GtkArrowType`, the class a `menubutton`'s `arrow` node carries.
    pub enum ArrowDirection {
        None = "none", Up = "up", Down = "down", Left = "left", Right = "right",
    }
    default = Down;
}

widget_enum! {
    /// `GtkFontLevel`.
    pub enum FontLevel { Family = "", Face = "", Font = "", Features = "" }
    default = Font;
}

widget_enum! {
    /// One token of `gtk-decoration-layout` that produces a node.
    /// `menu` is recognised by the setting but produces no child in 4.22.4.
    pub enum WindowButton {
        Icon = "icon", Minimize = "minimize", Maximize = "maximize", Close = "close",
    }
    default = Close;
}

/// `GtkAdjustment`: the shared numeric model behind `Scale`, `Scrollbar`,
/// `SpinButton` and every scrollable.
///
/// Every field is `f64` and every field can arrive from a `Prop::Float` a
/// model computed, so [`Adjustment::sanitized`] is applied on every ingress:
/// NaN and infinities become the initial values, and `lower > upper` swaps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjustment {
    /// Current value, always within `lower..=upper - page_size`.
    pub value: f64,
    /// Inclusive minimum.
    pub lower: f64,
    /// Inclusive maximum.
    pub upper: f64,
    /// One arrow-key or stepper step.
    pub step_increment: f64,
    /// One Page Up/Down or trough click.
    pub page_increment: f64,
    /// The visible span, non-zero only for scrollbars.
    pub page_size: f64,
}

impl Default for Adjustment {
    fn default() -> Self {
        Adjustment {
            value: 0.0,
            lower: 0.0,
            upper: 1.0,
            step_increment: 0.1,
            page_increment: 0.2,
            page_size: 0.0,
        }
    }
}

impl Adjustment {
    /// A sanitized adjustment over `lower..=upper`.
    #[must_use]
    pub fn new(value: f64, lower: f64, upper: f64) -> Self {
        Adjustment {
            value,
            lower,
            upper,
            ..Adjustment::default()
        }
        .sanitized()
    }

    /// Replace every non-finite field with its initial value, order the bounds,
    /// clamp `page_size` into the range and `value` into the usable span.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let fin = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        let mut lower = fin(self.lower, 0.0);
        let mut upper = fin(self.upper, 1.0);
        if lower > upper {
            std::mem::swap(&mut lower, &mut upper);
        }
        let page_size = fin(self.page_size, 0.0).clamp(0.0, upper - lower);
        let step_increment = fin(self.step_increment, 0.1).abs();
        let page_increment = fin(self.page_increment, 0.2).abs();
        let value = fin(self.value, lower).clamp(lower, upper - page_size);
        Adjustment {
            value,
            lower,
            upper,
            step_increment,
            page_increment,
            page_size,
        }
    }

    /// `value` clamped into the usable span.
    #[must_use]
    pub fn clamp(&self, value: f64) -> f64 {
        let value = if value.is_finite() { value } else { self.lower };
        value.clamp(self.lower, self.upper - self.page_size)
    }

    /// Where `value` sits in `0.0..=1.0`. A zero-width range reports 0.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        let span = self.upper - self.page_size - self.lower;
        if span <= 0.0 {
            0.0
        } else {
            ((self.value - self.lower) / span).clamp(0.0, 1.0)
        }
    }

    /// The value at `fraction` of the usable span.
    #[must_use]
    pub fn value_at_fraction(&self, fraction: f64) -> f64 {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.clamp(self.lower + fraction * (self.upper - self.page_size - self.lower))
    }

    /// Set the value, clamped. `true` if it actually moved.
    pub fn set_value(&mut self, value: f64) -> bool {
        let next = self.clamp(value);
        let moved = (next - self.value).abs() > f64::EPSILON;
        self.value = next;
        moved
    }
}

/// One `GtkScale` mark.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// Where on the adjustment the mark sits.
    pub value: f64,
    /// Which side of the trough it renders on.
    pub position: Position,
    /// Optional text under the indicator.
    pub label: Option<Rc<str>>,
}

/// The held-stepper repeat clock behind `SpinButton` (and, in P6, the scrollbar
/// steppers).
///
/// GTK's `climb_rate` shortens the interval as the button stays down; the
/// interval never falls below [`RepeatTimer::MIN_INTERVAL`], so a held button
/// cannot spin the event loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepeatTimer {
    next: Duration,
    interval: Duration,
    climb: f64,
}

impl RepeatTimer {
    /// The shortest interval a repeat can accelerate to.
    pub const MIN_INTERVAL: Duration = Duration::from_millis(20);

    /// Arm at `now`, first fire after `delay`.
    #[must_use]
    pub fn armed(now: Duration, delay: Duration, interval: Duration, climb: f64) -> Self {
        RepeatTimer {
            next: now + delay,
            interval: interval.max(Self::MIN_INTERVAL),
            climb: if climb.is_finite() {
                climb.clamp(1.0, 4.0)
            } else {
                1.0
            },
        }
    }

    /// When the next repeat is due.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        self.next
    }

    /// How many repeats are due at `now`, rearming for the next one.
    ///
    /// Returns 0 before the deadline. Bounded at 16 per call so a clock that
    /// jumped forward cannot emit thousands of steps in one tick.
    pub fn fire(&mut self, now: Duration) -> u32 {
        let mut fired = 0u32;
        while now >= self.next && fired < 16 {
            fired += 1;
            let scaled = self.interval.as_secs_f64() / self.climb;
            self.interval = Duration::from_secs_f64(scaled).max(Self::MIN_INTERVAL);
            self.next += self.interval;
        }
        fired
    }
}

/// What `ColorDialogButton` launches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorDialogSpec {
    /// Show an alpha slider.
    pub with_alpha: bool,
    /// Take a grab while open.
    pub modal: bool,
}

impl Default for ColorDialogSpec {
    fn default() -> Self {
        ColorDialogSpec {
            with_alpha: false,
            modal: true,
        }
    }
}

/// Where a `Picture`'s pixels come from.
#[derive(Debug, Clone, PartialEq)]
pub enum PictureSource {
    /// Nothing to draw.
    None,
    /// A path decoded through `skia_rs_safe::codec`.
    File(Rc<Path>),
    /// An in-memory encoded image (PNG), for tests and embedded assets.
    Bytes(Rc<[u8]>),
}

/// The `GtkWidget`-universal prop bookkeeping every widget controller shares.
///
/// [`crate::view::controller::GenericC`] applies contract §4.3's fifteen
/// universal props itself, by rewriting the node's whole class list. A widget
/// controller cannot do that: it owns classes of its own (`separator`'s
/// `.horizontal`, `menubutton`'s `.arrow`) that a wholesale rewrite would
/// erase. This type therefore applies the same props *incrementally* — it
/// remembers only the classes the `Classes` prop itself contributed, and adds
/// or removes exactly those — so a controller can call it from `set_prop`
/// without losing its own work.
#[derive(Debug, Default)]
pub struct Universal {
    /// The classes the last `Classes` prop put on the node.
    extra: Vec<Rc<str>>,
}

impl Universal {
    /// Seed a node with the kind's GTK focusability default.
    ///
    /// `build_controller` has already written the kind's base classes; this
    /// adds P3's [`FOCUSABLE_CLASS`] when GTK would make the widget a Tab
    /// stop, which is what the focus ring reads.
    #[must_use]
    pub fn new(node: &Node, kind: Kind) -> Self {
        if kind.is_focusable_by_default() {
            node.add_class(FOCUSABLE_CLASS);
        }
        Universal { extra: Vec::new() }
    }

    /// Write one universal prop onto `node`. `true` when `name` was one of
    /// them and the caller need do nothing more.
    pub fn apply(&mut self, node: &Node, kind: Kind, name: PropName, value: &Prop) -> bool {
        match name {
            PropName::Classes => {
                for stale in self.extra.drain(..) {
                    node.remove_class(&stale);
                }
                if let Prop::Classes(extra) = value {
                    for class in extra.iter() {
                        node.add_class(class);
                        self.extra.push(Rc::clone(class));
                    }
                }
                true
            }
            PropName::Focusable => {
                // Absent restores the kind's GTK default.
                let focusable = match value {
                    Prop::Bool(on) => *on,
                    _ => kind.is_focusable_by_default(),
                };
                if focusable {
                    node.add_class(FOCUSABLE_CLASS);
                } else {
                    node.remove_class(FOCUSABLE_CLASS);
                }
                true
            }
            PropName::Id => {
                node.set_id(match value {
                    Prop::Str(id) => Some(id),
                    _ => None,
                });
                true
            }
            PropName::Sensitive => {
                // Absent means GTK's default, which is sensitive.
                let sensitive = matches!(value, Prop::Bool(true) | Prop::None);
                node.set_state(PseudoStates::DISABLED, !sensitive);
                true
            }
            PropName::Checked => {
                node.set_state(PseudoStates::CHECKED, matches!(value, Prop::Bool(true)));
                true
            }
            PropName::Indeterminate => {
                node.set_state(
                    PseudoStates::INDETERMINATE,
                    matches!(value, Prop::Bool(true)),
                );
                true
            }
            PropName::Selected => {
                node.set_state(PseudoStates::SELECTED, matches!(value, Prop::Bool(true)));
                true
            }
            _ => false,
        }
    }
}

/// The `:hover`/`:active` bookkeeping every pointer-reactive widget shares.
///
/// Contract §5's preamble says every controller carries `pressed` and `hovered`
/// unless it has no pointer behaviour at all; this is that pair, plus the state
/// flag updates, in one place instead of thirty-two.
#[derive(Debug, Default)]
pub struct PointerState {
    /// A button is down on this node.
    pub pressed: bool,
    /// The pointer is inside this node.
    pub hovered: bool,
}

impl PointerState {
    /// Fold one event in, updating `node`'s pseudo-states.
    ///
    /// Returns `true` when this event completed a click *inside* `alloc` — the
    /// press-then-release-inside gesture GTK calls "clicked". A release outside
    /// the allocation clears `:active` and returns `false`, which is what makes
    /// drag-off-and-release cancel a button press.
    pub fn observe(&mut self, node: &Node, ev: &Event, alloc: Option<Rect>) -> bool {
        let inside = |local: (f32, f32)| match alloc {
            Some(rect) => {
                local.0 >= 0.0 && local.1 >= 0.0 && local.0 <= rect.width && local.1 <= rect.height
            }
            None => true,
        };
        match ev {
            Event::PointerEnter { .. } => {
                self.hovered = true;
                node.set_state(PseudoStates::HOVER, true);
            }
            Event::PointerLeave => {
                self.hovered = false;
                node.set_state(PseudoStates::HOVER, false);
            }
            Event::PointerMotion { local } => {
                self.hovered = inside(*local);
                node.set_state(PseudoStates::HOVER, self.hovered);
            }
            Event::PointerDown { local, .. } if inside(*local) => {
                self.pressed = true;
                node.set_state(PseudoStates::ACTIVE, true);
            }
            Event::PointerUp { local, .. } => {
                let was = self.pressed;
                self.pressed = false;
                node.set_state(PseudoStates::ACTIVE, false);
                return was && inside(*local);
            }
            _ => {}
        }
        false
    }
}

/// Build the controller for `kind`, and apply `props` to it.
///
/// [`Controller::build`] is `Self: Sized`, so the reconciler cannot go from a
/// [`Kind`] to a `Box<dyn Controller<Msg>>` on its own; this is that table, and
/// it lives beside the widgets so adding one is a single-file change. P6 adds
/// its own kinds' arms.
///
/// The catch-all is [`crate::view::controller::GenericC`], never an inert stub
/// (contract §11 E1): a kind no part has written a controller for still
/// renders, still styles and still clicks.
pub fn build_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    // Identity first, so a `Classes` prop lands on top of the base classes.
    node.set_classes(kind.base_classes());
    let mut boxed: Box<dyn Controller<Msg>> = match kind {
        Kind::Separator => Box::new(<separator::SeparatorC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Label => Box::new(<label::LabelC as Controller<Msg>>::build(node, props, cx)),
        Kind::Spinner => Box::new(<spinner::SpinnerC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Statusbar => Box::new(<statusbar::StatusbarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::LevelBar => Box::new(<level_bar::LevelBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::ProgressBar => Box::new(<progress_bar::ProgressBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        _ => crate::view::controller::generic_controller(kind, node, props, cx),
    };
    for (name, value) in props.iter() {
        boxed.set_prop(node, name, value, cx);
    }
    boxed
}

/// Build `kind` in isolation and render its retained subtree in GTK notation.
///
/// Hermetic: its own sheet, font database, icon theme and clock, no compositor
/// and no window. This is what the conformance gate and P8's gallery gate call.
#[must_use]
pub fn node_tree_of(kind: Kind, props: &Props) -> String {
    use crate::anim::ManualClock;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::icons::IconTheme;
    use crate::text::FontDatabase;

    let node = Node::with_classes(kind.css_name(), kind.base_classes());
    let sheet = CompiledSheet::compile("");
    let mut fonts = FontDatabase::new();
    let mut icons = IconTheme::with_name_and_roots("hicolor", Vec::new());
    let clock: Rc<dyn crate::anim::Clock> = Rc::new(ManualClock::new());
    let env = ResolveEnv::default();
    let mut cx = BuildCx {
        sheet: &sheet,
        fonts: &mut fonts,
        icons: &mut icons,
        clock: &clock,
        env: &env,
    };
    let _controller = build_controller::<()>(kind, &node, props, &mut cx);
    render_node_tree(&node)
}

/// Render a retained subtree in GTK's own `├──`/`╰──` notation.
///
/// `name.class1.class2` per node, classes in the order the node reports them.
#[must_use]
pub fn render_node_tree(root: &Node) -> String {
    fn spec(node: &Node) -> String {
        let mut out = node.name().to_string();
        for class in node.classes() {
            out.push('.');
            out.push_str(class.as_str());
        }
        out
    }
    fn walk(node: &Node, prefix: &str, out: &mut String) {
        let children = node.children();
        for (index, child) in children.iter().enumerate() {
            let last = index + 1 == children.len();
            out.push_str(prefix);
            out.push_str(if last { "╰── " } else { "├── " });
            out.push_str(&spec(child));
            out.push('\n');
            let mut next = prefix.to_owned();
            next.push_str(if last { "    " } else { "│   " });
            walk(child, &next, out);
        }
    }
    let mut out = spec(root);
    out.push('\n');
    walk(root, "", &mut out);
    out
}

/// One parsed line of either tree.
struct TreeLine {
    depth: usize,
    name: String,
    /// Classes the node must carry.
    required: Vec<String>,
    /// The whole node is configuration-dependent (`[name]`).
    node_optional: bool,
    /// `<child>`: an arbitrary application subtree.
    wildcard: bool,
}

/// Parse a GTK node-tree block into lines with their depths.
///
/// Repetition markers (`┊`, `⋮`) and blank lines are dropped: repetition does
/// not change which *paths* are legal, which is what the matcher checks.
fn parse_tree(text: &str) -> Vec<TreeLine> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() || line.trim().starts_with("```") {
            continue;
        }
        let trimmed = line.trim_start_matches(['│', '┊', '⋮', ' ']);
        if trimmed.is_empty() {
            continue;
        }
        // Depth is one level per four columns of prefix.
        let prefix_cols = line.chars().count() - trimmed.chars().count();
        let (depth, body) = match trimmed
            .strip_prefix("├── ")
            .or_else(|| trimmed.strip_prefix("╰── "))
        {
            Some(body) => (prefix_cols / 4 + 1, body),
            None => (0, trimmed),
        };
        let body = body.trim();
        if body == "<child>" {
            out.push(TreeLine {
                depth,
                name: String::new(),
                required: Vec::new(),
                node_optional: false,
                wildcard: true,
            });
            continue;
        }
        let (body, node_optional) = match body.strip_prefix('[') {
            Some(rest) => (rest.trim_end_matches(']'), true),
            None => (body, false),
        };
        let mut required = Vec::new();
        // The name runs up to the first '.' or '['.
        let head_end = body.find(['.', '[']).unwrap_or(body.len());
        let name = body[..head_end].to_owned();
        let mut rest = &body[head_end..];
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                // An `[.class]` is optional: it is neither required of the
                // rendered node nor forbidden on it.
                let end = after.find(']').unwrap_or(after.len());
                rest = &after[(end + 1).min(after.len())..];
            } else if let Some(after) = rest.strip_prefix('.') {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                required.push(after[..end].to_owned());
                rest = &after[end..];
            } else {
                break;
            }
        }
        out.push(TreeLine {
            depth,
            name,
            required,
            node_optional,
            wildcard: false,
        });
    }
    out
}

/// Turn parsed lines into `(path, line index)` pairs, where a path is the
/// slash-joined node names from the root.
fn paths(lines: &[TreeLine]) -> Vec<(String, usize)> {
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        stack.truncate(line.depth);
        let name = if line.wildcard {
            "<child>".to_owned()
        } else {
            line.name.clone()
        };
        stack.push(name);
        out.push((stack.join("/"), index));
    }
    out
}

/// Match a rendered tree against a vendored GTK fixture.
///
/// Both directions are enforced. Every rendered node must sit at a path the
/// fixture names (or under a `<child>` wildcard), and must carry every class
/// the fixture marks as always-present. Every fixture node that is not
/// `[optional]` and whose parent is present must appear in the rendered tree.
/// Extra classes on a rendered node are allowed: GTK's blocks do not list
/// application-supplied classes such as `.suggested-action`.
///
/// # Errors
///
/// A human-readable description of the first mismatch.
pub fn fixture_matches(fixture: &str, rendered: &str) -> Result<(), String> {
    let fixture_lines = parse_tree(fixture);
    let rendered_lines = parse_tree(rendered);
    let fixture_paths = paths(&fixture_lines);
    let rendered_paths = paths(&rendered_lines);

    let wildcard_prefixes: Vec<String> = fixture_paths
        .iter()
        .filter(|(_, index)| fixture_lines[*index].wildcard)
        .map(|(path, _)| path.trim_end_matches("/<child>").to_owned())
        .collect();

    for (path, index) in &rendered_paths {
        if wildcard_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix) && path.len() > prefix.len())
        {
            continue;
        }
        let candidates: Vec<usize> = fixture_paths
            .iter()
            .filter(|(fixture_path, _)| fixture_path == path)
            .map(|(_, i)| *i)
            .collect();
        if candidates.is_empty() {
            return Err(format!("rendered node at '{path}' is not in the fixture"));
        }
        // Two or more fixture lines can share a path — e.g. `block.filled`
        // and `block.empty` at `levelbar/trough/block` — to describe
        // mutually exclusive variants a repeated child can take. The
        // rendered node only needs to satisfy one of them.
        let actual = &rendered_lines[*index];
        let mut missing: Option<&str> = None;
        let matched = candidates.iter().any(|fixture_index| {
            let expected = &fixture_lines[*fixture_index];
            match expected
                .required
                .iter()
                .find(|c| !actual.required.contains(*c))
            {
                Some(class) => {
                    missing.get_or_insert(class.as_str());
                    false
                }
                None => true,
            }
        });
        if !matched {
            let class = missing.unwrap_or("");
            return Err(format!(
                "required class '{class}' missing from rendered node at '{path}'"
            ));
        }
    }

    let rendered_set: std::collections::HashSet<&str> = rendered_paths
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    for (path, index) in &fixture_paths {
        let line = &fixture_lines[*index];
        if line.node_optional || line.wildcard {
            continue;
        }
        let parent = match path.rfind('/') {
            Some(cut) => &path[..cut],
            None => "",
        };
        if !parent.is_empty() && !rendered_set.contains(parent) {
            continue;
        }
        if !rendered_set.contains(path.as_str()) {
            return Err(format!(
                "fixture requires a node at '{path}', which was not rendered"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Adjustment, Orientation, RepeatTimer, WidgetEnum};
    use crate::view::Prop;
    use std::time::Duration;

    #[test]
    fn a_widget_enum_round_trips_through_a_prop() {
        // mutation: make `to_u16` return a constant and Vertical decodes as
        // Horizontal.
        for variant in Orientation::all() {
            let prop = variant.to_prop();
            assert_eq!(
                Orientation::from_prop(Some(&prop), Orientation::Horizontal),
                *variant
            );
        }
        assert_eq!(
            Orientation::from_prop(Some(&Prop::Enum(9_999)), Orientation::Vertical),
            Orientation::Vertical,
            "an out-of-range discriminant falls back, never panics"
        );
        assert_eq!(
            Orientation::from_prop(Some(&Prop::Bool(true)), Orientation::Vertical),
            Orientation::Vertical,
            "a wrongly typed prop falls back"
        );
    }

    #[test]
    fn an_adjustment_never_lets_a_hostile_number_through() {
        // mutation: drop the `fin` guard in `sanitized` and every assertion
        // below reports NaN.
        let hostile = Adjustment {
            value: f64::NAN,
            lower: 10.0,
            upper: -10.0,
            step_increment: f64::INFINITY,
            page_increment: f64::NEG_INFINITY,
            page_size: f64::NAN,
        }
        .sanitized();
        assert!(hostile.value.is_finite() && hostile.lower.is_finite());
        assert!(hostile.lower <= hostile.upper, "bounds are ordered");
        assert!(hostile.step_increment.is_finite() && hostile.step_increment >= 0.0);
        assert!(hostile.fraction().is_finite());
        assert!(
            (Adjustment::new(5.0, 0.0, 1.0).value - 1.0).abs() < f64::EPSILON,
            "value is clamped"
        );
        assert!(
            Adjustment::new(0.5, 0.0, 0.0).fraction().abs() < f64::EPSILON,
            "no div by zero"
        );
    }

    #[test]
    fn a_repeat_timer_accelerates_but_never_below_its_floor() {
        // mutation: remove the `.max(MIN_INTERVAL)` and the interval collapses
        // to zero, so `fire` returns the 16-step cap on every call.
        let mut timer = RepeatTimer::armed(
            Duration::ZERO,
            Duration::from_millis(400),
            Duration::from_millis(100),
            2.0,
        );
        assert_eq!(timer.fire(Duration::from_millis(399)), 0, "not yet due");
        assert_eq!(timer.fire(Duration::from_millis(400)), 1);
        let first = timer.deadline();
        assert!(first > Duration::from_millis(400));
        for step in 1..40u64 {
            timer.fire(Duration::from_millis(400 + step * 100));
        }
        let before = timer.deadline();
        timer.fire(before);
        assert!(
            timer.deadline() - before >= RepeatTimer::MIN_INTERVAL,
            "the interval floors at 20ms"
        );
    }

    #[test]
    fn a_universal_prop_never_erases_a_controller_s_own_class() {
        // mutation: rewrite `Universal::apply`'s Classes arm as a
        // `node.set_classes(..)` and the separator loses `.horizontal`.
        use super::Universal;
        use crate::css::node::Node;
        use crate::view::{Kind, PropName};
        use std::rc::Rc;

        let node = Node::new("separator");
        node.add_class("horizontal");
        let mut universal = Universal::new(&node, Kind::Separator);
        let classes: Rc<[Rc<str>]> = Rc::from(vec![Rc::from("dim-label")]);
        assert!(universal.apply(
            &node,
            Kind::Separator,
            PropName::Classes,
            &Prop::Classes(classes)
        ));
        let names: Vec<String> = node
            .classes()
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect();
        assert!(names.iter().any(|c| c == "horizontal"));
        assert!(names.iter().any(|c| c == "dim-label"));

        // A later `Classes` prop drops only what the previous one added.
        assert!(universal.apply(&node, Kind::Separator, PropName::Classes, &Prop::None));
        let names: Vec<String> = node
            .classes()
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["horizontal".to_owned()]);
    }
}
