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

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use selectors::Element as _;
use selectors::OpaqueElement;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{ChildLayout, Container, Rect};
use crate::view::controller::{Controller, Event};
use crate::view::{BuildCx, Kind, Prop, PropName, Props};
use crate::window::focus::FOCUSABLE_CLASS;

#[doc(inline)]
pub use crate::view::ListItem;

/// `ListItem`'s widget-facing constructor.
///
/// The type is declared in `view::mod` (contract deviation D7) and this part
/// may not edit `view/**`, so the inherent `impl` lives beside the re-export
/// every widget and test already uses. Same crate, so it is the same
/// `ListItem::new` a caller would get either way.
impl ListItem {
    /// A row with `text` and no subtitle or icon.
    #[must_use]
    pub fn new(id: u64, text: &str) -> Self {
        ListItem {
            id,
            text: std::rc::Rc::from(text),
            subtitle: None,
            icon: None,
        }
    }
}

pub mod box_;
pub mod button;
pub mod calendar;
pub mod center_box;
pub mod check_button;
pub mod color_dialog;
pub mod drawing_area;
pub mod drop_down;
pub mod edit;
pub mod editable_label;
pub mod entry;
pub mod expander;
pub mod font_dialog;
pub mod frame;
pub mod grid;
pub mod image;
pub mod info_bar;
pub mod label;
pub mod level_bar;
pub mod link_button;
pub mod menu_button;
pub mod node_tree;
pub mod paned;
pub mod password_entry;
pub mod picture;
pub mod popover;
pub mod progress_bar;
pub mod scale;
pub mod scrollbar;
pub mod search_entry;
pub mod separator;
pub mod spin_button;
pub mod spinner;
pub mod statusbar;
pub mod switch;
pub mod text_view;
pub mod toggle_button;
pub mod types;
pub mod window_controls;

#[doc(inline)]
pub use box_::BoxC;
#[doc(inline)]
pub use types::{
    BaselinePosition, Decoration, DisplayHint, ItemFactory, LicenseType, MenuFlags, Policy,
    RowContent, Selection, SelectionMode, SortOrder, Sorter, StackPageInfo, StackTransition,
};

// `node_tree::node_tree_of` is deliberately not re-exported here: P5 already
// defined a `node_tree_of` at this same path (below), and `ui/tests/node_trees.rs`
// imports it together with `fixture_matches`, its path-based sibling. Both
// P5 functions stay untouched; `node_tree`'s `matches_fixture` is the richer,
// backtracking matcher P6's new fixtures use instead.
#[doc(inline)]
pub use node_tree::{Mismatch, matches_fixture};

/// Re-express a pointer event given in the root node's space in `rect`'s space.
///
/// Controllers own their subnodes and hit-test against them; `EventCx` hands
/// coordinates local to the controller's own root node, so every subnode
/// gesture starts by subtracting that subnode's offset within the root.
pub(crate) fn shift_event(ev: &Event, rect: Rect) -> Event {
    let map = |local: (f32, f32)| (local.0 - rect.x, local.1 - rect.y);
    match ev {
        Event::PointerEnter { local } => Event::PointerEnter { local: map(*local) },
        Event::PointerMotion { local } => Event::PointerMotion { local: map(*local) },
        Event::PointerDown {
            button,
            local,
            serial,
        } => Event::PointerDown {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        Event::PointerUp {
            button,
            local,
            serial,
        } => Event::PointerUp {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        other => other.clone(),
    }
}

/// A subnode's rectangle in its controller root's own space.
pub(crate) fn local_rect(
    tree: &crate::layout::LayoutTree,
    root: &Node,
    sub: &Node,
) -> Option<Rect> {
    let root_box = tree.allocation(root)?.border_box;
    let sub_box = tree.allocation(sub)?.border_box;
    Some(Rect::new(
        sub_box.x - root_box.x,
        sub_box.y - root_box.y,
        sub_box.width,
        sub_box.height,
    ))
}

/// A controller root's *content* box, in that same root's own event space.
///
/// [`local_rect`] only answers for subnodes the reconciler gave a taffy node,
/// which the extra nodes a controller appends itself (`text`, `image.peek`,
/// `trough`, …) never are — `tree.allocation` is `None` for every one of them.
/// A controller that needs to hit-test its own content therefore has to derive
/// the region from its root's allocation, offset by the padding+border inset
/// its content box already carries, because `Event`'s `local` is relative to
/// the *border* box's origin (`window/pointer.rs::aim`).
pub(crate) fn content_rect_local(tree: &crate::layout::LayoutTree, root: &Node) -> Option<Rect> {
    let alloc = tree.allocation(root)?;
    let (content, border) = (alloc.content_box, alloc.border_box);
    Some(Rect::new(
        content.x - border.x,
        content.y - border.y,
        content.width,
        content.height,
    ))
}

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

/// Read a `Prop` as a `u16` enum discriminant; any other shape is `default`.
pub(crate) fn prop_u16(value: &Prop, default: u16) -> u16 {
    match value {
        Prop::Enum(v) => *v,
        Prop::Int(v) => u16::try_from(*v).unwrap_or(default),
        _ => default,
    }
}

/// Read a `Prop` as an integer; a non-finite float is `default`.
pub(crate) fn prop_i64(value: &Prop, default: i64) -> i64 {
    match value {
        Prop::Int(v) => *v,
        #[allow(
            clippy::cast_possible_truncation,
            reason = "widget props never reach 2^53"
        )]
        Prop::Float(v) if v.is_finite() => *v as i64,
        Prop::Bool(v) => i64::from(*v),
        _ => default,
    }
}

/// Read a `Prop` as a float; non-finite becomes `default`.
#[allow(
    dead_code,
    reason = "every P6 controller reads it; unused until one does"
)]
pub(crate) fn prop_f64(value: &Prop, default: f64) -> f64 {
    match value {
        Prop::Float(v) if v.is_finite() => *v,
        #[allow(clippy::cast_precision_loss, reason = "widget props never reach 2^53")]
        Prop::Int(v) => *v as f64,
        _ => default,
    }
}

/// Read a `Prop` as a bool. A string is *not* coerced: `Prop::Str("yes")`
/// is a caller mistake, not `true`.
pub(crate) fn prop_bool(value: &Prop, default: bool) -> bool {
    match value {
        Prop::Bool(v) => *v,
        Prop::Int(v) => *v != 0,
        _ => default,
    }
}

/// Read a `Prop` as text; anything else is `""`.
#[allow(
    dead_code,
    reason = "every P6 controller reads it; unused until one does"
)]
pub(crate) fn prop_str(value: &Prop) -> &str {
    match value {
        Prop::Str(s) => s,
        _ => "",
    }
}

/// One controller's not-yet-flushed layout decisions for one node.
///
/// A widget controller has no `&mut LayoutTree` of its own — the tree lives
/// in the `App`'s `Runtime` (or, in a headless test, is never built at all)
/// — so a container/gap/homogeneous/child-placement decision is recorded
/// *on the node* here and folded into the real tree by [`flush_layout`].
///
/// Unlike a work queue, entries are **not** removed once applied: `App`
/// reruns `write_styles` (which rebuilds a node's taffy style from its CSS
/// alone) on every single frame, so a decision made once in `Controller::build`
/// must still be there to reapply on the tenth frame, long after the
/// `Props` value that produced it stopped changing. `homogeneous` is
/// re-applied to *whatever children the node currently has* for the same
/// reason: a child added after the prop was last set must still pick it up.
#[derive(Default, Clone)]
struct Pending {
    container: Option<Container>,
    gap: Option<(f32, Orientation)>,
    homogeneous: Option<(bool, Orientation)>,
    child_layout: Option<ChildLayout>,
    /// This node is a [`Container::Grid`]; re-derive every child's cell from
    /// its own recorded props on every flush, the same way `homogeneous`
    /// re-applies to whatever children the node currently has. A `Grid`
    /// controller builds before the reconciler attaches its children (M2's
    /// `build_instance` order), so it cannot loop over `node.children()`
    /// itself -- there are none yet -- and defers the loop to here instead.
    grid_from_children: bool,
    node: Option<Node>,
}

thread_local! {
    static PENDING: RefCell<HashMap<OpaqueElement, Pending>> = RefCell::new(HashMap::new());
    static CONTAINERS: RefCell<HashMap<OpaqueElement, Container>> = RefCell::new(HashMap::new());
    static NODE_PROPS: RefCell<HashMap<OpaqueElement, Props>> = RefCell::new(HashMap::new());
    static NODE_CHILD: RefCell<HashMap<OpaqueElement, ChildLayout>> = RefCell::new(HashMap::new());
}

/// The prop names [`record_props`] keeps; every other prop is dropped on the
/// way in.
///
/// A full `Props` clone would retain whatever a caller last set through it
/// forever -- `Prop::Draw`'s `Rc<dyn Fn>` included -- because this table has
/// no removal path (`reconcile`'s `Remove` op drops the `Instance`, not this
/// side entry). Grid placement is the only reader today and every value it
/// needs is a `Prop::Int`, cheap to keep and inert to clone, so `record_props`
/// keeps only these four rather than whatever the caller happened to pass.
const RECORDED_PROP_NAMES: [PropName; 4] = [
    PropName::Column,
    PropName::Row,
    PropName::ColumnSpan,
    PropName::RowSpan,
];

/// Record the subset of `props` a container controller can re-derive a
/// child's placement from. Called from the reconciler's `Insert`/`SetProp`
/// arms with that instance's just-applied `Props`.
pub(crate) fn record_props(node: &Node, props: &Props) {
    let mut kept = Props::default();
    for name in RECORDED_PROP_NAMES {
        if let Some(value) = props.get(name) {
            kept.set(name, value.clone());
        }
    }
    NODE_PROPS.with(|m| m.borrow_mut().insert(node.opaque(), kept));
}

/// The props last applied to `node`, or an empty set.
///
/// A container controller (e.g. [`grid::GridC`]) has no `&mut Instance` of
/// its own child -- only the child's [`Node`] -- so it re-derives a child's
/// placement from here instead of owning the child's instance.
#[must_use]
pub(crate) fn props_of(node: &Node) -> Props {
    NODE_PROPS
        .with(|m| m.borrow().get(&node.opaque()).cloned())
        .unwrap_or_default()
}

/// The child layout last recorded for `node`.
#[must_use]
pub(crate) fn child_layout_of(node: &Node) -> ChildLayout {
    NODE_CHILD
        .with(|m| m.borrow().get(&node.opaque()).copied())
        .unwrap_or_default()
}

/// Record `node`'s container, for later [`flush_layout`] and for the
/// headless `container_of` test hook every container controller exposes.
pub(crate) fn set_container(node: &Node, container: Container) {
    CONTAINERS.with(|c| c.borrow_mut().insert(node.opaque(), container));
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.container = Some(container);
        entry.node = Some(node.clone());
    });
}

/// Mark `node` (a [`Container::Grid`]) so [`flush_layout`] re-derives every
/// child's [`crate::layout::GridPlacement`] from that child's own recorded
/// `Column`/`Row`/`ColumnSpan`/`RowSpan` props each frame.
pub(crate) fn mark_grid_children(node: &Node) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.grid_from_children = true;
        entry.node = Some(node.clone());
    });
}

/// This node's last recorded container, for a headless (`App`-less) test.
///
/// A real `App` derives a node's container independently, from its `Kind`
/// and `Props` (`view::render::container_for`); this is a second, cheaper
/// source of truth that lets a widget's own unit tests ask "what container
/// did my controller build?" without standing up a `LayoutTree` at all.
#[must_use]
pub(crate) fn container_of(node: &Node) -> Container {
    CONTAINERS.with(|c| c.borrow().get(&node.opaque()).copied().unwrap_or_default())
}

/// Record `node`'s per-child layout (alignment/expansion), keyed by the
/// *child* node itself rather than its parent -- a `Container::Center`'s
/// three children each need a different [`ChildLayout`], which the
/// one-entry-per-node `container`/`gap`/`homogeneous` fields above cannot
/// express.
pub(crate) fn set_child_layout(node: &Node, layout: ChildLayout) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.child_layout = Some(layout);
        entry.node = Some(node.clone());
    });
    NODE_CHILD.with(|m| m.borrow_mut().insert(node.opaque(), layout));
}

/// Record a gap floor for `node`'s main axis, keyed by `orientation`.
pub(crate) fn set_gap(node: &Node, spacing: f32, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.gap = Some((
            if spacing.is_finite() {
                spacing.max(0.0)
            } else {
                0.0
            },
            orientation,
        ));
        entry.node = Some(node.clone());
    });
}

/// Record that every child of `node` should expand on `orientation`'s axis
/// while `on` holds, applied to whatever children `node` has when
/// [`flush_layout`] next runs.
pub(crate) fn set_homogeneous(node: &Node, on: bool, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry(node.opaque()).or_default();
        entry.homogeneous = Some((on, orientation));
        entry.node = Some(node.clone());
    });
}

/// Apply every recorded decision to `tree`.
///
/// Called by the real render loop (`view::render::layout_tree`) immediately
/// after `write_styles` and before `LayoutTree::compute`, and by any test
/// that builds its own `LayoutTree`. Entries whose node never made it into
/// `tree` are silent no-ops: `LayoutTree::set_container`/`set_gap_floor`/
/// `set_child_layout` already ignore an unsynced node, which covers a
/// controller that recorded a decision before its node was attached, or a
/// node `reconcile` has since removed.
pub fn flush_layout(tree: &mut crate::layout::LayoutTree) {
    PENDING.with(|p| {
        for pending in p.borrow().values() {
            let Some(node) = &pending.node else { continue };
            if let Some(container) = pending.container {
                tree.set_container(node, container);
            }
            if let Some(child_layout) = pending.child_layout {
                tree.set_child_layout(node, child_layout);
            }
            if let Some((gap, orientation)) = pending.gap {
                // GTK's own gap property and CSS `border-spacing` both
                // apply; the larger wins, which is what GtkBox does when a
                // theme sets both.
                tree.set_gap_floor(node, gap, orientation == Orientation::Horizontal);
            }
            if let Some((on, orientation)) = pending.homogeneous {
                let horizontal = orientation == Orientation::Horizontal;
                for child in node.children() {
                    let mut cl = tree.child_layout(&child).unwrap_or_default();
                    if horizontal {
                        cl.hexpand = on;
                    } else {
                        cl.vexpand = on;
                    }
                    tree.set_child_layout(&child, cl);
                }
            }
            if pending.grid_from_children {
                for child in node.children() {
                    let Some(place) = grid::GridC::placement_of(&props_of(&child)) else {
                        continue;
                    };
                    let mut cl = tree.child_layout(&child).unwrap_or_default();
                    cl.grid = Some(place);
                    tree.set_child_layout(&child, cl);
                }
            }
        }
    });
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
        Kind::Box => Box::new(<box_::BoxC as Controller<Msg>>::build(node, props, cx)),
        Kind::CenterBox => Box::new(<center_box::CenterBoxC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Grid => Box::new(<grid::GridC as Controller<Msg>>::build(node, props, cx)),
        Kind::Frame => Box::new(<frame::FrameC as Controller<Msg>>::build(node, props, cx)),
        Kind::Expander => Box::new(<expander::ExpanderC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Paned => Box::new(<paned::PanedC as Controller<Msg>>::build(node, props, cx)),
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
        Kind::InfoBar => Box::new(<info_bar::InfoBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Scrollbar => Box::new(<scrollbar::ScrollbarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Image => Box::new(<image::ImageC as Controller<Msg>>::build(node, props, cx)),
        Kind::Picture => Box::new(<picture::PictureC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::TextView => Box::new(<text_view::TextViewC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Scale => Box::new(<scale::ScaleC as Controller<Msg>>::build(node, props, cx)),
        Kind::DrawingArea => Box::new(<drawing_area::DrawingAreaC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::WindowControls => {
            Box::new(<window_controls::WindowControlsC as Controller<Msg>>::build(node, props, cx))
        }
        Kind::Calendar => Box::new(<calendar::CalendarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Popover => Box::new(<popover::PopoverC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Button => Box::new(<button::ButtonC as Controller<Msg>>::build(node, props, cx)),
        Kind::ToggleButton => Box::new(<toggle_button::ToggleButtonC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::LinkButton => Box::new(<link_button::LinkButtonC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::CheckButton => Box::new(<check_button::CheckButtonC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Switch => Box::new(<switch::SwitchC as Controller<Msg>>::build(node, props, cx)),
        Kind::MenuButton => Box::new(<menu_button::MenuButtonC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::DropDown => Box::new(<drop_down::DropDownC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::ColorDialogButton => {
            Box::new(<color_dialog::ColorDialogButtonC as Controller<Msg>>::build(node, props, cx))
        }
        Kind::ColorDialog => Box::new(<color_dialog::ColorDialogC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::FontDialogButton => Box::new(
            <font_dialog::FontDialogButtonC as Controller<Msg>>::build(node, props, cx),
        ),
        Kind::FontDialog => Box::new(<font_dialog::FontDialogC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Entry => Box::new(<entry::EntryC as Controller<Msg>>::build(node, props, cx)),
        Kind::SearchEntry => Box::new(<search_entry::SearchEntryC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::PasswordEntry => Box::new(
            <password_entry::PasswordEntryC as Controller<Msg>>::build(node, props, cx),
        ),
        Kind::SpinButton => Box::new(<spin_button::SpinButtonC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::EditableLabel => Box::new(
            <editable_label::EditableLabelC as Controller<Msg>>::build(node, props, cx),
        ),
        _ => crate::view::controller::generic_controller(kind, node, props, cx),
    };
    for (name, value) in props.iter() {
        boxed.set_prop(node, name, value, cx);
    }
    boxed
}

/// Where a kind's application children are attached.
///
/// Most kinds take children on their own node. A few nest them in a subnode
/// GTK's own tree names — `popover > contents` is P5's only case; P6 adds
/// the scrolled window's viewport and `expander-widget`'s own dedicated
/// `content` node (`box`'s second child, always present so the child keeps
/// its identity across a collapse/expand cycle -- `ExpanderC`'s own doc
/// comment). P4's reconciler calls this before `Node::append_child`.
///
/// It also has to answer for every kind whose controller appends its *own*
/// subnodes straight onto its root, because the reconciler's "no application
/// children" cleanup (`reconcile`'s step 4, over whatever this returns) walks
/// that same node and detaches anything under it beyond position zero —
/// harmless for a kind whose visible chrome lives on the root itself (a
/// plain `Button`'s padding/border still paints with its `label` child gone)
/// or that paints its own content in `Controller::paint` regardless of its
/// child nodes (`Switch`), fatal for one whose chrome lives entirely on a
/// child the cleanup would otherwise strip on every single reconcile.
/// `MenuButton` routes to its embedded popover's `contents` — the same node
/// its "children become the popover content" is documented to use, and
/// otherwise always empty — and `DropDown`, which takes no application
/// children at all, to a dedicated, never-attached `sink` node so the
/// cleanup has nothing real to touch.
#[must_use]
pub fn child_slot(kind: Kind, controller: &dyn std::any::Any) -> Option<Node> {
    match kind {
        Kind::Popover => controller
            .downcast_ref::<popover::PopoverC>()
            .map(|c| c.contents.clone()),
        Kind::Expander => controller
            .downcast_ref::<expander::ExpanderC>()
            .map(|c| c.content.clone()),
        Kind::MenuButton => controller
            .downcast_ref::<menu_button::MenuButtonC>()
            .map(|c| c.popover.contents.clone()),
        Kind::DropDown => controller
            .downcast_ref::<drop_down::DropDownC>()
            .map(|c| c.sink.clone()),
        Kind::ColorDialogButton => controller
            .downcast_ref::<color_dialog::ColorDialogButtonC>()
            .map(|c| c.sink.clone()),
        Kind::ColorDialog => controller
            .downcast_ref::<color_dialog::ColorDialogC>()
            .map(|c| c.sink.clone()),
        Kind::FontDialogButton => controller
            .downcast_ref::<font_dialog::FontDialogButtonC>()
            .map(|c| c.sink.clone()),
        Kind::FontDialog => controller
            .downcast_ref::<font_dialog::FontDialogC>()
            .map(|c| c.sink.clone()),
        _ => None,
    }
}

/// Everything `Controller::build` needs, with no Wayland connection and no
/// application: the bundled Adwaita light sheet, a probe-only font database,
/// a rootless icon theme and a `ManualClock` at zero.
pub struct Headless {
    sheet: crate::css::cascade::CompiledSheet,
    fonts: crate::text::FontDatabase,
    icons: crate::icons::IconTheme,
    clock: Rc<dyn crate::anim::Clock>,
    env: crate::css::computed::ResolveEnv,
    /// Backing storage for [`Headless::event_cx`]'s `tree`/`styles`/`focus`/
    /// `clipboard` fields -- empty and unsynced, since no widget task's
    /// `on_event` test hits an allocation-dependent path.
    tree: crate::layout::LayoutTree,
    styles: crate::view::StyleMap,
    focus: crate::window::focus::FocusRing,
    clipboard: crate::window::selection::Clipboard,
}

impl Headless {
    /// A context over `BUNDLED_ADWAITA_LIGHT`.
    #[must_use]
    pub fn new() -> Self {
        let sheet = crate::css::cascade::CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        Self {
            sheet,
            fonts: crate::text::FontDatabase::new(),
            // There is no `IconTheme::empty()`; `node_tree_of` below already
            // stands up a rootless "hicolor" theme for the same hermetic
            // purpose, so this reuses that rather than adding a second way to
            // spell "no real icons".
            icons: crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new()),
            clock: Rc::new(crate::anim::ManualClock::new()),
            env: crate::css::computed::ResolveEnv::default(),
            tree: crate::layout::LayoutTree::new(),
            styles: crate::view::StyleMap::new(),
            focus: <crate::window::focus::FocusRing as Default>::default(),
            clipboard: crate::window::selection::Clipboard::offscreen(),
        }
    }

    /// Borrow it as a `BuildCx`.
    pub fn cx(&mut self) -> BuildCx<'_> {
        BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }

    /// An `EventCx` over `node`, with an empty focus ring and clipboard.
    ///
    /// `handlers` and `cmds` are the two fields an `EventCx` carries that
    /// depend on `Msg`, which `Headless` itself is not generic over; each
    /// call leaks a fresh, empty pair for them (`Box::leak`, never freed).
    /// `Headless` is test/dev-only scaffolding built once per test and
    /// dropped at its end, so the leak is a handful of bytes per call, not a
    /// growing one -- the same trade the rest of this struct already makes
    /// by never tearing down its font/icon caches either.
    pub fn event_cx<'a, Msg: 'static>(
        &'a mut self,
        node: &'a Node,
    ) -> crate::view::EventCx<'a, Msg> {
        let handlers: &crate::view::Handlers<Msg> =
            Box::leak(Box::new(crate::view::Handlers::default()));
        let cmds: &mut Vec<crate::view::Cmd<Msg>> = Box::leak(Box::new(Vec::new()));
        crate::view::EventCx {
            node,
            handlers,
            tree: &self.tree,
            styles: &self.styles,
            focus: &mut self.focus,
            clipboard: &mut self.clipboard,
            icons: &mut self.icons,
            fonts: &mut self.fonts,
            clock: &self.clock,
            env: &self.env,
            cmds,
            phase: crate::view::Phase::Target,
            handled: false,
        }
    }

    /// [`Headless::event_cx`], with `populate` given a chance to register
    /// handlers first -- a controller's `on_event` only emits a `Msg` when
    /// [`crate::view::Handlers`] actually holds a handler for the
    /// [`crate::view::EventKind`] it fires, which the empty set `event_cx`
    /// leaks never does.
    pub fn event_cx_with_handlers<'a, Msg: Clone + 'static>(
        &'a mut self,
        node: &'a Node,
        populate: impl FnOnce(&mut crate::view::Handlers<Msg>),
    ) -> crate::view::EventCx<'a, Msg> {
        let mut handlers = crate::view::Handlers::default();
        populate(&mut handlers);
        let handlers: &crate::view::Handlers<Msg> = Box::leak(Box::new(handlers));
        let cmds: &mut Vec<crate::view::Cmd<Msg>> = Box::leak(Box::new(Vec::new()));
        crate::view::EventCx {
            node,
            handlers,
            tree: &self.tree,
            styles: &self.styles,
            focus: &mut self.focus,
            clipboard: &mut self.clipboard,
            icons: &mut self.icons,
            fonts: &mut self.fonts,
            clock: &self.clock,
            env: &self.env,
            cmds,
            phase: crate::view::Phase::Target,
            handled: false,
        }
    }

    /// A synthetic, already-pressed key event for keysym `name` (an
    /// xkbcommon keysym name, e.g. `"Right"`, `"Home"`), with no modifiers.
    ///
    /// Reconciliation: the task text describes this as going through
    /// `Keymap::vendored_us().key_by_name(name)`, but neither exists --
    /// `window::keyboard::Keymap` (P3's file, off limits to P6 except its
    /// own registration lines) translates a *keycode* it already holds
    /// (`Keymap::translate`) and has no reverse keysym-to-keycode lookup to
    /// build one on. `KeyEvent`'s fields are all public, so this builds one
    /// directly from `xkb::keysym_from_name` instead of inventing a P3 API
    /// this task cannot add.
    ///
    /// Takes no `&self`: it needs none, and `event_cx`'s returned `EventCx`
    /// already holds `Headless` mutably borrowed for as long as a test keeps
    /// using it, which a `&self`/`&mut self` receiver here would collide
    /// with on every call interleaved with `on_event` the way the widget
    /// tests that use both actually call them.
    #[must_use]
    pub fn key(name: &str) -> crate::window::keyboard::KeyEvent {
        let keysym = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_NO_FLAGS);
        crate::window::keyboard::KeyEvent {
            keycode: 0,
            keysym,
            utf8: None,
            mods: crate::window::keyboard::Mods::empty(),
            consumed: crate::window::keyboard::Mods::empty(),
            pressed: true,
            repeat: false,
            serial: 0,
            time_ms: 0,
        }
    }
}

impl Default for Headless {
    fn default() -> Self {
        Self::new()
    }
}

/// A widget built outside an `App`, for tests and for `node_tree::matches_fixture`.
pub struct BuiltWidget<Msg> {
    /// The widget's root retained node.
    pub node: Node,
    /// Its controller, already `build`-ed.
    pub controller: Box<dyn Controller<Msg>>,
}

/// Build one widget headlessly.
#[must_use]
pub fn build_widget<Msg: Clone + 'static>(kind: Kind, props: &Props) -> BuiltWidget<Msg> {
    let mut hx = Headless::new();
    let node = Node::with_classes(kind.css_name(), kind.base_classes());
    let controller = {
        let mut cx = hx.cx();
        build_controller::<Msg>(kind, &node, props, &mut cx)
    };
    BuiltWidget { node, controller }
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

/// Offscreen `App` driving, for every widget's rest-state and interaction
/// pixel tests. No Wayland connection, `ManualClock` time.
#[cfg(test)]
pub(crate) mod offscreen {
    use std::rc::Rc;

    use crate::anim::ManualClock;
    use crate::view::{App, Cmd, Frames, ScriptStep, View};

    /// Run `script` against a one-widget app and return its captured frames.
    pub(crate) fn frames<M: 'static, Msg: Clone + 'static>(
        model: M,
        update: fn(&mut M, Msg) -> Cmd<Msg>,
        view: fn(&M) -> View<Msg>,
        size: (u32, u32),
        script: Vec<ScriptStep<Msg>>,
    ) -> Frames {
        let clock = Rc::new(ManualClock::new());
        App::new(model, update, view)
            .run_offscreen(size, clock, script)
            .expect("offscreen run")
    }

    /// One pixel, or a panic naming the coordinate.
    pub(crate) fn px(frames: &Frames, frame: usize, x: u32, y: u32) -> (u8, u8, u8, u8) {
        frames
            .pixel(frame, x, y)
            .unwrap_or_else(|| panic!("no pixel at ({x}, {y}) in frame {frame}"))
    }

    /// The x of the first column between two painted children -- derived
    /// from the frame, never hard-coded.
    pub(crate) fn gap_column(frames: &Frames) -> u32 {
        let background = px(frames, 0, 0, 0);
        let mut seen_ink = false;
        for x in 0..frames.width() {
            let p = px(frames, 0, x, frames.height() / 2);
            if p != background {
                seen_ink = true;
            } else if seen_ink {
                return x;
            }
        }
        panic!("no gap column: the children never separate");
    }
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
