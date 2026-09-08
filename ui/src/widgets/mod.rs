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
#[doc(inline)]
pub use window_controls::WindowControlsC;

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

pub mod about_dialog;
pub mod action_bar;
pub mod alert_dialog;
pub mod box_;
pub mod button;
pub mod calendar;
pub mod center_box;
pub mod check_button;
pub mod color_dialog;
pub mod column_view;
pub mod drawing_area;
pub mod drop_down;
pub mod edit;
pub mod editable_label;
pub mod entry;
pub mod expander;
pub mod flow_box;
pub mod font_dialog;
pub mod frame;
pub mod grid;
pub mod grid_view;
pub mod header_bar;
pub mod headless;
pub mod image;
pub mod info_bar;
pub mod label;
pub mod level_bar;
pub mod link_button;
pub mod list_box;
pub mod list_view;
pub mod menu_button;
pub mod node_tree;
pub mod notebook;
pub mod overlay;
pub mod paned;
pub mod password_entry;
pub mod picture;
pub mod popover;
pub mod popover_menu;
pub mod popover_menu_bar;
pub mod progress_bar;
pub mod scale;
pub mod scrollbar;
pub mod scrolled_window;
pub mod search_bar;
pub mod search_entry;
pub mod separator;
pub mod shortcuts_window;
pub mod spin_button;
pub mod spinner;
pub mod stack;
pub mod stack_sidebar;
pub mod stack_switcher;
pub mod state;
pub mod statusbar;
pub mod switch;
pub mod text_view;
pub mod toggle_button;
pub mod types;
pub mod window;
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
pub use node_tree::{Mismatch, fixture_matches, matches_fixture, node_tree_of, render_node_tree};

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
        // `upper - page_size` can land *below* `lower` even though `page_size`
        // was just clamped to `upper - lower`: that subtraction rounds, and
        // `upper - fl(upper - lower) < lower` for e.g. `lower = -100.0`,
        // `upper = 28.3`. `f64::clamp` asserts `min <= max`, so the floor is
        // what keeps a legal adjustment from panicking.
        let value = fin(self.value, lower).clamp(lower, (upper - page_size).max(lower));
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
        // Floored at `lower` for the same float-cancellation reason
        // [`Adjustment::sanitized`] gives.
        value.clamp(self.lower, (self.upper - self.page_size).max(self.lower))
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

    /// Whether a repeat is due at `now`, rearming for the next one.
    ///
    /// `false` before the deadline. **At most one repeat per call, and the
    /// next deadline is measured from `now`, not from the deadline that was
    /// missed.** A held button repeats on a wall clock the caller does not
    /// control: `tick` runs from the app loop, and one gallery repaint takes
    /// seconds (`support::GALLERY_MAP_TIMEOUT` measures why), so several
    /// intervals routinely elapse between two ticks. Counting them and
    /// stepping once per missed interval made a held `SpinButton` jump
    /// straight from its first step to its clamped bound -- the observed
    /// `3 -> 4 -> 10` -- instead of stepping visibly, one increment at a
    /// time, for as long as the button is held; and each counted interval
    /// also applied `climb` again, so a slow frame accelerated the repeat as
    /// if the user had held the button through every one of them. Dropping
    /// the arrears is what a repeat *means*: GTK's own stepper is a
    /// `g_timeout` callback, which fires once per invocation however late the
    /// main loop is, and never replays the ones it missed.
    pub fn fire(&mut self, now: Duration) -> bool {
        if now < self.next {
            return false;
        }
        let scaled = self.interval.as_secs_f64() / self.climb;
        self.interval = Duration::from_secs_f64(scaled).max(Self::MIN_INTERVAL);
        self.next = now + self.interval;
        true
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

/// A GTK revealer: a child subtree whose visibility animates between hidden
/// and shown.
///
/// `GtkSearchBar` and `GtkActionBar` both own one, so the timing, the
/// progress bookkeeping and the `next_deadline` rule live in one place
/// instead of being copied. Unlike a real `GtkRevealer` this does not scale
/// or clip anything by itself: nothing in the paint walker reads a per-node
/// transform yet (`ExpanderC`'s own `progress`/`content_scale` is the same
/// shape, tracked but not yet wired to a visual effect), so `progress` is a
/// value a controller's own `paint`/hit-testing can consult, not something
/// this type applies to `node` on its own.
pub struct Revealer {
    /// The `revealer` node.
    pub node: Node,
    /// Target state.
    pub revealed: bool,
    /// `0.0` hidden .. `1.0` shown.
    pub progress: f32,
    /// When the running animation started; `None` when at rest.
    started: Option<Duration>,
    duration: Duration,
}

impl Revealer {
    /// Build a `revealer` node under `parent`.
    #[must_use]
    pub fn build(parent: &Node, revealed: bool) -> Self {
        let node = Node::new("revealer");
        parent.append_child(&node);
        Self {
            node,
            revealed,
            progress: if revealed { 1.0 } else { 0.0 },
            started: None,
            duration: Duration::from_millis(250),
        }
    }

    /// Ask for a new target; a no-op when it is already the target.
    pub fn set_revealed(&mut self, revealed: bool, now: Duration) {
        if self.revealed == revealed {
            return;
        }
        self.revealed = revealed;
        self.started = Some(now);
    }

    /// Advance the animation.
    pub fn tick(&mut self, now: Duration) {
        let Some(started) = self.started else {
            return;
        };
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (now.saturating_sub(started).as_secs_f32() / self.duration.as_secs_f32())
                .clamp(0.0, 1.0)
        };
        self.progress = if self.revealed { t } else { 1.0 - t };
        if t >= 1.0 {
            self.started = None;
        }
    }

    /// The next frame this wants, or `None` at rest. `Duration::ZERO` means
    /// "now", never "spin": `tick` clears `started` at the end.
    #[must_use]
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|started| {
            let end = started + self.duration;
            if now >= end {
                Duration::ZERO
            } else {
                end - now
            }
        })
    }
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
    /// `GtkWidget:width-request`/`height-request`, in px, kept together
    /// because the side table [`set_size_request`] writes carries the pair
    /// and each prop arrives on its own.
    size_request: (f32, f32),
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
        Universal {
            extra: Vec::new(),
            size_request: (0.0, 0.0),
        }
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
            PropName::WidthRequest | PropName::HeightRequest => {
                // GTK's size request is universal, and until P8-D71's
                // close-out nothing but `Label`/`Entry`/`DrawingArea` read
                // either name: `gallery`'s `list_view` sample asked for
                // 200x120 and laid out 0x0, which is why no interaction
                // could be driven against it at all. Both axes are re-read
                // together, because the side table carries the pair.
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a size request beyond 2^24 px is not a size"
                )]
                if let Prop::Int(px) = value {
                    let px = *px as f32;
                    if name == PropName::WidthRequest {
                        self.size_request.0 = px;
                    } else {
                        self.size_request.1 = px;
                    }
                } else if name == PropName::WidthRequest {
                    self.size_request.0 = 0.0;
                } else {
                    self.size_request.1 = 0.0;
                }
                set_size_request(node, self.size_request.0, self.size_request.1);
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

// The node-keyed side tables and the headless builder moved out of this file
// verbatim (`state`, `headless`); every path a caller — crate-internal or an
// `ui/tests` integration crate — already used still resolves here.
#[doc(inline)]
pub use headless::{BuiltWidget, Headless, build_widget};
#[doc(inline)]
pub use state::flush_layout;
pub(crate) use state::{
    apply_universal, child_layout_of, container_of, forget_subtree, mark_grid_children,
    mark_overlay_children, measure_row, paint_row, props_of, record_props, row_index_of,
    row_text_height, set_child_layout, set_container, set_displayed, set_gap, set_homogeneous,
    set_icon, set_row_classes, set_row_index, set_size_request, set_text, set_transition_progress,
    size_request_of, text_of,
};

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
            // Only the primary button drives `:active` and the click gesture,
            // the way real GTK activates a widget on `GDK_BUTTON_PRIMARY`
            // (P5-D11). A middle/right press must not arm the pressed state, and
            // a non-left release must not clear it or report a click -- else a
            // middle-click on a widget that also carries its own middle-button
            // handler fires a spurious activation alongside it.
            Event::PointerDown { local, button, .. }
                if *button == crate::window::layer::BTN_LEFT && inside(*local) =>
            {
                self.pressed = true;
                node.set_state(PseudoStates::ACTIVE, true);
            }
            Event::PointerUp { local, button, .. } if *button == crate::window::layer::BTN_LEFT => {
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
        Kind::ScrolledWindow => {
            Box::new(<scrolled_window::ScrolledWindowC as Controller<Msg>>::build(node, props, cx))
        }
        Kind::SearchBar => Box::new(<search_bar::SearchBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::ActionBar => Box::new(<action_bar::ActionBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
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
        Kind::HeaderBar => Box::new(<header_bar::HeaderBarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Calendar => Box::new(<calendar::CalendarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Notebook => Box::new(<notebook::NotebookC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Overlay => Box::new(<overlay::OverlayC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Stack => Box::new(<stack::StackC as Controller<Msg>>::build(node, props, cx)),
        Kind::Popover => Box::new(<popover::PopoverC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::PopoverMenu => Box::new(<popover_menu::PopoverMenuC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::PopoverMenuBar => {
            Box::new(<popover_menu_bar::PopoverMenuBarC as Controller<Msg>>::build(node, props, cx))
        }
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
        Kind::ListBox => Box::new(<list_box::ListBoxC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::ListView => Box::new(<list_view::ListViewC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::FlowBox => Box::new(<flow_box::FlowBoxC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::GridView => Box::new(<grid_view::GridViewC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::ColumnView => Box::new(<column_view::ColumnViewC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::StackSwitcher => Box::new(
            <stack_switcher::StackSwitcherC as Controller<Msg>>::build(node, props, cx),
        ),
        Kind::StackSidebar => Box::new(<stack_sidebar::StackSidebarC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::Window => Box::new(<window::WindowC as Controller<Msg>>::build(node, props, cx)),
        Kind::ShortcutsWindow => Box::new(<shortcuts_window::ShortcutsWindowC as Controller<
            Msg,
        >>::build(node, props, cx)),
        Kind::AboutDialog => Box::new(<about_dialog::AboutDialogC as Controller<Msg>>::build(
            node, props, cx,
        )),
        Kind::AlertDialog => Box::new(<alert_dialog::AlertDialogC as Controller<Msg>>::build(
            node, props, cx,
        )),
        _ => crate::view::controller::generic_controller(kind, node, props, cx),
    };
    for (name, value) in props.iter() {
        apply_universal(node, kind, name, value);
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
        Kind::SearchBar => controller
            .downcast_ref::<search_bar::SearchBarC>()
            .map(|c| c.contents.clone()),
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
        Kind::ColumnView => controller
            .downcast_ref::<column_view::ColumnViewC>()
            .map(|c| c.header.clone()),
        _ => None,
    }
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
    fn an_adjustment_survives_a_page_size_that_cancels_its_range() {
        // mutation: drop either `.max(lower)` floor and this panics inside
        // `f64::clamp` ("assertion failed: min <= max"): `page_size` clamps to
        // `fl(28.3 - -100.0)` = 128.3, and `28.3 - 128.3` is `-100.00000000000001`,
        // a hair *below* `lower`.
        let clamped = Adjustment {
            value: 0.0,
            lower: -100.0,
            upper: 28.3,
            step_increment: 0.1,
            page_increment: 0.2,
            page_size: 200.0,
        }
        .sanitized();
        assert!(
            (clamped.value - clamped.lower).abs() < f64::EPSILON,
            "a page size that swallows the whole range pins the value at `lower`, \
             not below it: {} vs {}",
            clamped.value,
            clamped.lower
        );
        assert!(
            (clamped.clamp(1_000.0) - clamped.lower).abs() < f64::EPSILON,
            "and `clamp` agrees rather than panicking"
        );
    }

    #[test]
    fn a_repeat_timer_accelerates_but_never_below_its_floor() {
        // mutation: remove the `.max(MIN_INTERVAL)` and the interval collapses
        // to zero, so the deadline stops advancing and the floor assertion
        // below fails.
        let mut timer = RepeatTimer::armed(
            Duration::ZERO,
            Duration::from_millis(400),
            Duration::from_millis(100),
            2.0,
        );
        assert!(!timer.fire(Duration::from_millis(399)), "not yet due");
        assert!(timer.fire(Duration::from_millis(400)));
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
    fn a_late_repeat_fires_once_and_re_anchors_on_the_clock_it_was_given() {
        // mutation: restore `fire`'s catch-up loop (`while now >= self.next`,
        // `self.next += self.interval`) and the deadline assertion below fails
        // (`5.1s` against `5.103s`): the re-armed deadline is measured from
        // the last missed deadline, so it stays on the old grid instead of one
        // interval past the clock the caller actually gave. The `bool` return
        // makes the *other* half of the same bug -- several repeats settled in
        // one call -- unrepresentable rather than merely untested.
        //
        // This is the `SpinButton` repeat overshoot: the app loop cannot tick
        // on time while a repaint is in flight, and a repeat that settles its
        // arrears in one call steps a held stepper straight to its bound.
        let mut timer = RepeatTimer::armed(
            Duration::ZERO,
            Duration::from_millis(400),
            Duration::from_millis(100),
            1.0,
        );
        // Deliberately not a whole number of intervals past the arming
        // delay: a catch-up loop lands the deadline on `400 + n * 100`, which
        // for a round `late` would coincide with the re-anchored answer.
        let late = Duration::from_millis(5_003);
        assert!(timer.fire(late), "a missed deadline still fires");
        assert!(
            !timer.fire(late),
            "and only once: the arrears of the 46 intervals it slept through \
             are dropped, not replayed"
        );
        assert_eq!(
            timer.deadline(),
            late + Duration::from_millis(100),
            "the next deadline is one interval after the clock it was given, \
             not after the deadline that was missed"
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
