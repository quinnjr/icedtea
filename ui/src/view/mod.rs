//! The reactive framework: `View` description trees, keyed reconciliation
//! into M2's retained [`Node`](crate::css::node::Node)s, per-kind
//! controllers, and the `App` loop that folds their messages.
//!
//! ```text
//! view(&Model) -> View<Msg>          pure, rebuilt every frame
//!        │
//!        ▼  reconcile (keyed LCS)
//! Vec<Instance<Msg>>                 retained: Node + Controller + children
//!        │
//!        ├─ render::restyle_tree     cascade → ComputedStyle + AnimationState
//!        ├─ render::layout_tree      taffy, through Controller::measure
//!        └─ render::paint_tree       skia,  through Controller::paint
//!        │
//!        ▼  InputEvent → hit chain → capture/target/bubble
//! Vec<Msg> → update(&mut Model, Msg) -> Cmd<Msg> → queued, never nested
//! ```
//!
//! Identity is the point of the reconciler: a child matched by key and kind
//! keeps its `Node`, and with it its running transitions, its focus, its
//! shaping caches and any popup attached to it.
//!
//! Two loops share every stage: [`App::run`](app::App::run) over a live
//! [`Window`](crate::window::Window), and
//! [`App::run_offscreen`](app::App::run_offscreen) over a raster surface on a
//! [`ManualClock`](crate::anim::ManualClock), which is how the tests drive it.

use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::value::image::IconRef;
use crate::layout::{Align, Rect};

pub mod app;
pub mod builders;
pub mod cmd;
pub mod controller;
pub mod inbox;
pub mod reconcile;
pub mod render;

pub use app::{App, AppError, Frames, PopupEvent, ScriptStep};
pub use builders::widget;
pub use cmd::Cmd;
pub use controller::{Controller, Event, EventCx, Phase};
pub use inbox::{Inbox, InboxSender, SendError};
pub use reconcile::{BuildCx, Instance, Op, containers_of, reconcile};
pub use render::{Animations, NodeAddr, StyleMap, node_addr};

/// A typed property name, never a string — contract §0's "nothing names a
/// property string" rule, extended from CSS properties to widget props.
///
/// The order is contract §4.3's: the fifteen `GtkWidget`-universal names
/// first, then the shared widget names. `Ord` follows declaration order, and
/// [`Props`] keeps its entries sorted by it, so a `Props` comparison is a
/// slice comparison and `diff` is a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PropName {
    // GtkWidget-universal (15).
    Classes,
    Id,
    Visible,
    Sensitive,
    Focusable,
    Tooltip,
    Halign,
    Valign,
    Hexpand,
    Vexpand,
    Margin,
    WidthRequest,
    HeightRequest,
    Cursor,
    Opacity,
    // Shared widget props (82).
    Label,
    Text,
    Placeholder,
    Icon,
    IconSize,
    Active,
    Checked,
    Indeterminate,
    Value,
    Lower,
    Upper,
    StepIncrement,
    PageIncrement,
    Digits,
    Wrap,
    Ellipsize,
    Xalign,
    Yalign,
    Markup,
    Selectable,
    Uri,
    Group,
    Orientation,
    Spacing,
    Homogeneous,
    RowSpacing,
    ColumnSpacing,
    Position,
    Ratio,
    Expanded,
    Title,
    Subtitle,
    Fraction,
    Pulse,
    Inverted,
    ShowText,
    MaxLength,
    Visibility,
    Editable,
    EnableUndo,
    SelectionMode,
    Model,
    Selected,
    EnableSearch,
    ShowArrow,
    Modal,
    Autohide,
    Transition,
    TransitionDuration,
    Reveal,
    Decoration,
    Side,
    Anchor,
    Gravity,
    Constraint,
    Offset,
    Reactive,
    Columns,
    Rows,
    Column,
    Row,
    ColumnSpan,
    RowSpan,
    Fit,
    Paintable,
    DrawFn,
    ItemFactory,
    Page,
    Sortable,
    Resizable,
    MinContentWidth,
    MinContentHeight,
    OverlayScrolling,
    Kinetic,
    ShowSeparators,
    MarksTop,
    MarksBottom,
    FillLevel,
    Message,
    Detail,
    Buttons,
    MessageType,
    // P6 · containers (deviation P6-D3: additive names for §5.4–§5.7's
    // builders, appended after the last P4/P5 variant so no existing
    // discriminant, and therefore no `Props` ordering, changes).
    BaselinePosition,
    BaselineChild,
    BaselineRow,
    RowHomogeneous,
    ColumnHomogeneous,
    ShrinkCenterLast,
    HscrollbarPolicy,
    VscrollbarPolicy,
    HasFrame,
    MaxContentWidth,
    MaxContentHeight,
    PropagateNaturalWidth,
    PropagateNaturalHeight,
    PositionSet,
    WideHandle,
    ResizeStart,
    ResizeEnd,
    ShrinkStart,
    ShrinkEnd,
    LabelXalign,
    LabelWidget,
    UseUnderline,
    ResizeToplevel,
    SearchMode,
    ShowCloseButton,
    KeyCapture,
    Revealed,
    ShowTitleButtons,
    TitleWidget,
    TabPos,
    Scrollable,
    ShowTabs,
    ShowBorder,
    Reorderable,
    Detachable,
    MeasureOverlay,
    ClipOverlay,
    VisibleChild,
    Hhomogeneous,
    Vhomogeneous,
    InterpolateSize,
    PageName,
    PageTitle,
    NeedsAttention,
    Pages,
    // P6 · lists
    ActivateOnSingleClick,
    Activatable,
    MinChildrenPerLine,
    MaxChildrenPerLine,
    SingleClickActivate,
    EnableRubberband,
    MinColumns,
    MaxColumns,
    ShowRowSeparators,
    ShowColumnSeparators,
    SortColumn,
    SortOrder,
    Expand,
    /// A `ColumnViewColumn`'s comparator. Reconciliation: neither the
    /// contract's §5.5 table nor this part's own `PropName` list names a slot
    /// for `column_view_column`'s `.sorter()` -- every other name a sortable
    /// column could plausibly reuse (`SortColumn`/`SortOrder`) already means
    /// "the view's *current* sort", not "this column's comparator", so this
    /// adds the one genuinely missing name rather than overloading either.
    Sorter,
    // P6 · menus
    Flags,
    VisibleSubmenu,
    Accel,
    Submenu,
    Section,
    DisplayHint,
    Menus,
    // P6 · windows
    DefaultWidget,
    Deletable,
    Decorated,
    DefaultWidth,
    DefaultHeight,
    IconName,
    SectionName,
    ViewName,
    ProgramName,
    Version,
    Comments,
    Copyright,
    License,
    LicenseType,
    Website,
    WebsiteLabel,
    Authors,
    Artists,
    Documenters,
    TranslatorCredits,
    LogoIconName,
    WrapLicense,
    DefaultButton,
    CancelButton,
}

impl PropName {
    /// Every name, in declaration order. Pinned at 97 by this module's tests
    /// so the table and the contract cannot drift.
    pub const ALL: &'static [PropName] = &[
        PropName::Classes,
        PropName::Id,
        PropName::Visible,
        PropName::Sensitive,
        PropName::Focusable,
        PropName::Tooltip,
        PropName::Halign,
        PropName::Valign,
        PropName::Hexpand,
        PropName::Vexpand,
        PropName::Margin,
        PropName::WidthRequest,
        PropName::HeightRequest,
        PropName::Cursor,
        PropName::Opacity,
        PropName::Label,
        PropName::Text,
        PropName::Placeholder,
        PropName::Icon,
        PropName::IconSize,
        PropName::Active,
        PropName::Checked,
        PropName::Indeterminate,
        PropName::Value,
        PropName::Lower,
        PropName::Upper,
        PropName::StepIncrement,
        PropName::PageIncrement,
        PropName::Digits,
        PropName::Wrap,
        PropName::Ellipsize,
        PropName::Xalign,
        PropName::Yalign,
        PropName::Markup,
        PropName::Selectable,
        PropName::Uri,
        PropName::Group,
        PropName::Orientation,
        PropName::Spacing,
        PropName::Homogeneous,
        PropName::RowSpacing,
        PropName::ColumnSpacing,
        PropName::Position,
        PropName::Ratio,
        PropName::Expanded,
        PropName::Title,
        PropName::Subtitle,
        PropName::Fraction,
        PropName::Pulse,
        PropName::Inverted,
        PropName::ShowText,
        PropName::MaxLength,
        PropName::Visibility,
        PropName::Editable,
        PropName::EnableUndo,
        PropName::SelectionMode,
        PropName::Model,
        PropName::Selected,
        PropName::EnableSearch,
        PropName::ShowArrow,
        PropName::Modal,
        PropName::Autohide,
        PropName::Transition,
        PropName::TransitionDuration,
        PropName::Reveal,
        PropName::Decoration,
        PropName::Side,
        PropName::Anchor,
        PropName::Gravity,
        PropName::Constraint,
        PropName::Offset,
        PropName::Reactive,
        PropName::Columns,
        PropName::Rows,
        PropName::Column,
        PropName::Row,
        PropName::ColumnSpan,
        PropName::RowSpan,
        PropName::Fit,
        PropName::Paintable,
        PropName::DrawFn,
        PropName::ItemFactory,
        PropName::Page,
        PropName::Sortable,
        PropName::Resizable,
        PropName::MinContentWidth,
        PropName::MinContentHeight,
        PropName::OverlayScrolling,
        PropName::Kinetic,
        PropName::ShowSeparators,
        PropName::MarksTop,
        PropName::MarksBottom,
        PropName::FillLevel,
        PropName::Message,
        PropName::Detail,
        PropName::Buttons,
        PropName::MessageType,
    ];

    /// Every `PropName` Part 6 introduced, in declaration order.
    ///
    /// Exists so a rebase that drops one fails a test instead of failing a
    /// widget at run time. `ALL` stays P4's ninety-seven: it is the contract's
    /// own table, and the M2 property-reference gate reads it.
    #[must_use]
    pub fn all_p6() -> &'static [PropName] {
        use PropName::*;
        &[
            BaselinePosition,
            BaselineChild,
            BaselineRow,
            RowHomogeneous,
            ColumnHomogeneous,
            ShrinkCenterLast,
            HscrollbarPolicy,
            VscrollbarPolicy,
            HasFrame,
            MaxContentWidth,
            MaxContentHeight,
            PropagateNaturalWidth,
            PropagateNaturalHeight,
            PositionSet,
            WideHandle,
            ResizeStart,
            ResizeEnd,
            ShrinkStart,
            ShrinkEnd,
            LabelXalign,
            LabelWidget,
            UseUnderline,
            ResizeToplevel,
            SearchMode,
            ShowCloseButton,
            KeyCapture,
            Revealed,
            ShowTitleButtons,
            TitleWidget,
            TabPos,
            Scrollable,
            ShowTabs,
            ShowBorder,
            Reorderable,
            Detachable,
            MeasureOverlay,
            ClipOverlay,
            VisibleChild,
            Hhomogeneous,
            Vhomogeneous,
            InterpolateSize,
            PageName,
            PageTitle,
            NeedsAttention,
            Pages,
            ActivateOnSingleClick,
            Activatable,
            MinChildrenPerLine,
            MaxChildrenPerLine,
            SingleClickActivate,
            EnableRubberband,
            MinColumns,
            MaxColumns,
            ShowRowSeparators,
            ShowColumnSeparators,
            SortColumn,
            SortOrder,
            Expand,
            Sorter,
            Flags,
            VisibleSubmenu,
            Accel,
            Submenu,
            Section,
            DisplayHint,
            Menus,
            DefaultWidget,
            Deletable,
            Decorated,
            DefaultWidth,
            DefaultHeight,
            IconName,
            SectionName,
            ViewName,
            ProgramName,
            Version,
            Comments,
            Copyright,
            License,
            LicenseType,
            Website,
            WebsiteLabel,
            Authors,
            Artists,
            Documenters,
            TranslatorCredits,
            LogoIconName,
            WrapLicense,
            DefaultButton,
            CancelButton,
        ]
    }
}

/// One row of a list model.
///
/// Contract deviation D7: §4.3's `Prop::Items(Rc<[ListItem]>)` names this
/// type but never defines it. `id` is the stable identity a keyed
/// [`View`] uses, so a re-sorted model moves rows rather than rebuilding them.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Stable identity across model edits.
    pub id: u64,
    /// The primary text a default item factory renders.
    pub text: Rc<str>,
    /// Optional second line.
    pub subtitle: Option<Rc<str>>,
    /// Optional leading icon.
    pub icon: Option<IconRef>,
}

/// A property value.
///
/// `Draw` compares by [`Rc::ptr_eq`] (contract §4.3): two closures are the
/// same prop only when they are the same allocation, so a `view` that rebuilds
/// its draw closure every frame repaints every frame — which is what a caller
/// that captures the model wants.
#[derive(Clone)]
pub enum Prop {
    /// An interned string.
    Str(Rc<str>),
    /// A flag.
    Bool(bool),
    /// A whole number, including enum discriminants that need a range.
    Int(i64),
    /// A real number.
    Float(f64),
    /// A GTK image function (`-gtk-icontheme`, `-gtk-recolor`, `-gtk-scaled`).
    Icon(IconRef),
    /// CSS classes to add to the node, beyond `Kind::base_classes`.
    Classes(Rc<[Rc<str>]>),
    /// `top, right, bottom, left`, in px.
    Edges([i32; 4]),
    /// `halign`/`valign`.
    Align(Align),
    /// A widget-local `#[repr(u16)]` enum.
    Enum(u16),
    /// A list model.
    Items(Rc<[ListItem]>),
    /// `GtkDrawingArea`'s paint callback: the canvas, the rect to fill, and
    /// the paint context — the last so a callback can shape text, resolve an
    /// icon or read the theme's colours (M5-D6).
    #[allow(
        clippy::type_complexity,
        reason = "the contract's own Prop::Draw signature; a type alias would only hide it"
    )]
    Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut crate::paint::PaintCx<'_>)>),
    /// A row factory for the list family. `Msg`-free by construction so it
    /// can live in the non-generic [`Props`], and compared by pointer exactly
    /// as `Draw` is.
    Factory(crate::widgets::types::ItemFactory),
    /// A sortable column's comparator. `Msg`-free and pointer-compared, the
    /// same trade `Factory` makes for the same reason.
    Sorter(crate::widgets::types::Sorter),
    /// The property is absent — what [`Props::diff`] reports for a removal
    /// and what [`crate::view::controller::Controller::set_prop`] receives
    /// when a prop disappears.
    None,
}

impl std::fmt::Debug for Prop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Prop::Str(s) => f.debug_tuple("Str").field(s).finish(),
            Prop::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Prop::Int(i) => f.debug_tuple("Int").field(i).finish(),
            Prop::Float(x) => f.debug_tuple("Float").field(x).finish(),
            Prop::Icon(icon) => f.debug_tuple("Icon").field(icon).finish(),
            Prop::Classes(c) => f.debug_tuple("Classes").field(c).finish(),
            Prop::Edges(e) => f.debug_tuple("Edges").field(e).finish(),
            Prop::Align(a) => f.debug_tuple("Align").field(a).finish(),
            Prop::Enum(v) => f.debug_tuple("Enum").field(v).finish(),
            Prop::Items(items) => f.debug_tuple("Items").field(items).finish(),
            // A closure has no useful representation; its identity is its
            // address, which is what `PartialEq` compares.
            Prop::Draw(rc) => write!(f, "Draw({:p})", Rc::as_ptr(rc)),
            Prop::Factory(factory) => f.debug_tuple("Factory").field(factory).finish(),
            Prop::Sorter(sorter) => f.debug_tuple("Sorter").field(sorter).finish(),
            Prop::None => f.write_str("None"),
        }
    }
}

impl PartialEq for Prop {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Prop::Str(a), Prop::Str(b)) => a == b,
            (Prop::Bool(a), Prop::Bool(b)) => a == b,
            (Prop::Int(a), Prop::Int(b)) => a == b,
            // Bit equality, so a value that did not change does not restyle:
            // `NaN != NaN` would make an unset `fill-level` dirty every frame.
            (Prop::Float(a), Prop::Float(b)) => a.to_bits() == b.to_bits(),
            (Prop::Icon(a), Prop::Icon(b)) => a == b,
            (Prop::Classes(a), Prop::Classes(b)) => a == b,
            (Prop::Edges(a), Prop::Edges(b)) => a == b,
            (Prop::Align(a), Prop::Align(b)) => a == b,
            (Prop::Enum(a), Prop::Enum(b)) => a == b,
            (Prop::Items(a), Prop::Items(b)) => a == b,
            (Prop::Draw(a), Prop::Draw(b)) => Rc::ptr_eq(a, b),
            (Prop::Factory(a), Prop::Factory(b)) => a.ptr_eq(b),
            (Prop::Sorter(a), Prop::Sorter(b)) => a.ptr_eq(b),
            (Prop::None, Prop::None) => true,
            _ => false,
        }
    }
}

impl From<&str> for Prop {
    fn from(value: &str) -> Self {
        Prop::Str(Rc::from(value))
    }
}
impl From<String> for Prop {
    fn from(value: String) -> Self {
        Prop::Str(Rc::from(value.as_str()))
    }
}
impl From<bool> for Prop {
    fn from(value: bool) -> Self {
        Prop::Bool(value)
    }
}
impl From<i64> for Prop {
    fn from(value: i64) -> Self {
        Prop::Int(value)
    }
}
impl From<i32> for Prop {
    fn from(value: i32) -> Self {
        Prop::Int(i64::from(value))
    }
}
impl From<usize> for Prop {
    fn from(value: usize) -> Self {
        Prop::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}
impl From<f64> for Prop {
    fn from(value: f64) -> Self {
        Prop::Float(value)
    }
}
impl From<Align> for Prop {
    fn from(value: Align) -> Self {
        Prop::Align(value)
    }
}
impl From<IconRef> for Prop {
    fn from(value: IconRef) -> Self {
        Prop::Icon(value)
    }
}

/// A small sorted map from [`PropName`] to [`Prop`].
///
/// Most widgets carry under a dozen props, so a sorted `Vec` beats a hash
/// map on every operation that matters here: construction, whole-value
/// comparison, and the merge [`Props::diff`] performs.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Props(Vec<(PropName, Prop)>);

impl Props {
    /// Set `name`, replacing any previous value and keeping the map sorted.
    ///
    /// [`Prop::None`] is stored like any other value; a *removal* is
    /// expressed by the name being absent from the next frame's `Props`,
    /// which is what [`Props::diff`] reports.
    pub fn set(&mut self, name: PropName, value: Prop) {
        match self.0.binary_search_by_key(&name, |(n, _)| *n) {
            Ok(index) => self.0[index].1 = value,
            Err(index) => self.0.insert(index, (name, value)),
        }
    }

    /// The value under `name`, if any.
    #[must_use]
    pub fn get(&self, name: PropName) -> Option<&Prop> {
        self.0
            .binary_search_by_key(&name, |(n, _)| *n)
            .ok()
            .map(|index| &self.0[index].1)
    }

    /// `name` as a string; `None` when absent or of another variant.
    #[must_use]
    pub fn str(&self, name: PropName) -> Option<&str> {
        match self.get(name) {
            Some(Prop::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// `name` as a flag, falling back to `default`.
    #[must_use]
    pub fn bool(&self, name: PropName, default: bool) -> bool {
        match self.get(name) {
            Some(Prop::Bool(b)) => *b,
            _ => default,
        }
    }

    /// `name` as a whole number, falling back to `default`.
    #[must_use]
    pub fn int(&self, name: PropName, default: i64) -> i64 {
        match self.get(name) {
            Some(Prop::Int(i)) => *i,
            Some(Prop::Enum(v)) => i64::from(*v),
            _ => default,
        }
    }

    /// `name` as a real number, falling back to `default`. An `Int` widens,
    /// because a builder may hand either for a numeric property.
    #[must_use]
    pub fn float(&self, name: PropName, default: f64) -> f64 {
        match self.get(name) {
            Some(Prop::Float(x)) => *x,
            #[allow(clippy::cast_precision_loss, reason = "widget props never reach 2^53")]
            Some(Prop::Int(i)) => *i as f64,
            _ => default,
        }
    }

    /// Names whose value differs from `prev`, plus names present in one side
    /// only. Sorted, deduplicated — the reconciler's `SetProp` op set.
    #[must_use]
    pub fn diff(&self, prev: &Props) -> Vec<PropName> {
        let mut out = Vec::new();
        let (mut a, mut b) = (0, 0);
        while a < self.0.len() || b < prev.0.len() {
            match (self.0.get(a), prev.0.get(b)) {
                (Some((na, va)), Some((nb, vb))) if na == nb => {
                    if va != vb {
                        out.push(*na);
                    }
                    a += 1;
                    b += 1;
                }
                (Some((na, _)), Some((nb, _))) if na < nb => {
                    out.push(*na);
                    a += 1;
                }
                (Some(_), Some((nb, _))) => {
                    out.push(*nb);
                    b += 1;
                }
                (Some((na, _)), None) => {
                    out.push(*na);
                    a += 1;
                }
                (None, Some((nb, _))) => {
                    out.push(*nb);
                    b += 1;
                }
                (None, None) => break,
            }
        }
        out
    }

    /// Every `(name, value)`, in `PropName` order.
    pub fn iter(&self) -> impl Iterator<Item = (PropName, &Prop)> {
        self.0.iter().map(|(name, value)| (*name, value))
    }

    /// How many properties are set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One variant per in-scope widget, plus the sub-kinds GTK renders as their
/// own CSS node. Declaration order is contract §5's catalogue order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    // P5 · display
    Label,
    Spinner,
    Statusbar,
    LevelBar,
    ProgressBar,
    InfoBar,
    Scrollbar,
    Image,
    Picture,
    Separator,
    TextView,
    Scale,
    DrawingArea,
    WindowControls,
    Calendar,
    Popover,
    // P5 · buttons
    Button,
    ToggleButton,
    LinkButton,
    CheckButton,
    MenuButton,
    Switch,
    DropDown,
    ColorDialogButton,
    ColorDialog,
    FontDialogButton,
    FontDialog,
    // P5 · entries
    Entry,
    SearchEntry,
    PasswordEntry,
    SpinButton,
    EditableLabel,
    // P6 · containers
    Box,
    Grid,
    CenterBox,
    ScrolledWindow,
    Paned,
    Frame,
    Expander,
    SearchBar,
    ActionBar,
    HeaderBar,
    Notebook,
    NotebookTab,
    Overlay,
    Stack,
    StackPage,
    StackSwitcher,
    StackSidebar,
    // P6 · lists
    ListBox,
    ListBoxRow,
    FlowBox,
    FlowBoxChild,
    ListView,
    GridView,
    ColumnView,
    ColumnViewColumn,
    // P6 · menus
    PopoverMenu,
    PopoverMenuBar,
    PopoverMenuItem,
    // P6 · windows
    Window,
    ShortcutsWindow,
    AboutDialog,
    AlertDialog,
}

impl Kind {
    /// The CSS node name this kind's **root** node carries.
    ///
    /// Several kinds share one (`Button`/`ToggleButton`/`LinkButton` →
    /// `button`; `Box`/`CenterBox` → `box`; the four window kinds →
    /// `window`), so node-tree fixtures key on `Kind`, never on this string.
    ///
    /// Contract deviation D15: §5 names no node for `StackPage` and
    /// `ColumnViewColumn` (GTK renders neither as a node of its own), so P4
    /// pins `stackpage` and `button`; P6 may amend via §10.
    #[must_use]
    pub fn css_name(self) -> &'static str {
        match self {
            Kind::Label => "label",
            Kind::Spinner => "spinner",
            Kind::Statusbar => "statusbar",
            Kind::LevelBar => "levelbar",
            Kind::ProgressBar => "progressbar",
            Kind::InfoBar => "infobar",
            Kind::Scrollbar => "scrollbar",
            Kind::Image => "image",
            Kind::Picture => "picture",
            Kind::Separator => "separator",
            Kind::TextView => "textview",
            Kind::Scale => "scale",
            // GTK sets no CSS name on GtkDrawingArea; the node is
            // GtkWidget's default.
            Kind::DrawingArea => "widget",
            Kind::WindowControls => "windowcontrols",
            Kind::Calendar => "calendar",
            Kind::Popover | Kind::PopoverMenu => "popover",
            Kind::Button | Kind::ToggleButton | Kind::LinkButton => "button",
            Kind::CheckButton => "checkbutton",
            Kind::MenuButton => "menubutton",
            Kind::Switch => "switch",
            Kind::DropDown => "dropdown",
            Kind::ColorDialogButton => "colorbutton",
            Kind::FontDialogButton => "fontbutton",
            Kind::ColorDialog
            | Kind::FontDialog
            | Kind::Window
            | Kind::ShortcutsWindow
            | Kind::AboutDialog
            | Kind::AlertDialog => "window",
            Kind::Entry | Kind::SearchEntry | Kind::PasswordEntry => "entry",
            Kind::SpinButton => "spinbutton",
            Kind::EditableLabel => "editablelabel",
            Kind::Box | Kind::CenterBox => "box",
            Kind::Grid => "grid",
            Kind::ScrolledWindow => "scrolledwindow",
            Kind::Paned => "paned",
            Kind::Frame => "frame",
            Kind::Expander => "expander-widget",
            Kind::SearchBar => "searchbar",
            Kind::ActionBar => "actionbar",
            Kind::HeaderBar => "headerbar",
            Kind::Notebook => "notebook",
            Kind::NotebookTab => "tab",
            Kind::Overlay => "overlay",
            Kind::Stack => "stack",
            Kind::StackPage => "stackpage",
            Kind::StackSwitcher => "stackswitcher",
            Kind::StackSidebar => "stacksidebar",
            Kind::ListBox => "list",
            Kind::ListBoxRow => "row",
            Kind::FlowBox => "flowbox",
            Kind::FlowBoxChild => "flowboxchild",
            Kind::ListView => "listview",
            Kind::GridView => "gridview",
            Kind::ColumnView => "columnview",
            Kind::ColumnViewColumn | Kind::PopoverMenuItem => "button",
            Kind::PopoverMenuBar => "menubar",
        }
    }

    /// The `snake_case` spelling of this kind's variant name — P6's fixture
    /// filenames and P8's `--widget` flag both key on this, never on
    /// [`Kind::css_name`], which several kinds share.
    #[must_use]
    pub fn snake_name(self) -> &'static str {
        match self {
            Kind::Label => "label",
            Kind::Spinner => "spinner",
            Kind::Statusbar => "statusbar",
            Kind::LevelBar => "level_bar",
            Kind::ProgressBar => "progress_bar",
            Kind::InfoBar => "info_bar",
            Kind::Scrollbar => "scrollbar",
            Kind::Image => "image",
            Kind::Picture => "picture",
            Kind::Separator => "separator",
            Kind::TextView => "text_view",
            Kind::Scale => "scale",
            Kind::DrawingArea => "drawing_area",
            Kind::WindowControls => "window_controls",
            Kind::Calendar => "calendar",
            Kind::Popover => "popover",
            Kind::Button => "button",
            Kind::ToggleButton => "toggle_button",
            Kind::LinkButton => "link_button",
            Kind::CheckButton => "check_button",
            Kind::MenuButton => "menu_button",
            Kind::Switch => "switch",
            Kind::DropDown => "drop_down",
            Kind::ColorDialogButton => "color_dialog_button",
            Kind::ColorDialog => "color_dialog",
            Kind::FontDialogButton => "font_dialog_button",
            Kind::FontDialog => "font_dialog",
            Kind::Entry => "entry",
            Kind::SearchEntry => "search_entry",
            Kind::PasswordEntry => "password_entry",
            Kind::SpinButton => "spin_button",
            Kind::EditableLabel => "editable_label",
            Kind::Box => "box",
            Kind::Grid => "grid",
            Kind::CenterBox => "center_box",
            Kind::ScrolledWindow => "scrolled_window",
            Kind::Paned => "paned",
            Kind::Frame => "frame",
            Kind::Expander => "expander",
            Kind::SearchBar => "search_bar",
            Kind::ActionBar => "action_bar",
            Kind::HeaderBar => "header_bar",
            Kind::Notebook => "notebook",
            Kind::NotebookTab => "notebook_tab",
            Kind::Overlay => "overlay",
            Kind::Stack => "stack",
            Kind::StackPage => "stack_page",
            Kind::StackSwitcher => "stack_switcher",
            Kind::StackSidebar => "stack_sidebar",
            Kind::ListBox => "list_box",
            Kind::ListBoxRow => "list_box_row",
            Kind::FlowBox => "flow_box",
            Kind::FlowBoxChild => "flow_box_child",
            Kind::ListView => "list_view",
            Kind::GridView => "grid_view",
            Kind::ColumnView => "column_view",
            Kind::ColumnViewColumn => "column_view_column",
            Kind::PopoverMenu => "popover_menu",
            Kind::PopoverMenuBar => "popover_menu_bar",
            Kind::PopoverMenuItem => "popover_menu_item",
            Kind::Window => "window",
            Kind::ShortcutsWindow => "shortcuts_window",
            Kind::AboutDialog => "about_dialog",
            Kind::AlertDialog => "alert_dialog",
        }
    }

    /// Style classes the kind always adds, on top of `css_name`.
    #[must_use]
    pub fn base_classes(self) -> &'static [&'static str] {
        match self {
            Kind::ToggleButton => &["toggle"],
            Kind::LinkButton => &["link"],
            Kind::SearchEntry => &["search"],
            Kind::PasswordEntry => &["password"],
            Kind::ColorDialog | Kind::FontDialog => &["dialog"],
            Kind::AlertDialog => &["dialog", "message"],
            Kind::Window => &["background"],
            Kind::ShortcutsWindow => &["shortcuts"],
            Kind::AboutDialog => &["aboutdialog"],
            Kind::Popover => &["background"],
            Kind::PopoverMenu => &["background", "menu"],
            Kind::PopoverMenuItem => &["model"],
            Kind::StackSwitcher => &["stack-switcher"],
            Kind::StackSidebar => &["sidebar"],
            Kind::Calendar => &["view"],
            _ => &[],
        }
    }

    /// GTK's `focusable` default for this widget class.
    ///
    /// A `View` may override it with [`View::focusable`]; this is only the
    /// starting value the controller writes when the node is built.
    #[must_use]
    pub fn is_focusable_by_default(self) -> bool {
        matches!(
            self,
            Kind::Button
                | Kind::ToggleButton
                | Kind::LinkButton
                | Kind::CheckButton
                | Kind::MenuButton
                | Kind::Switch
                | Kind::DropDown
                | Kind::ColorDialogButton
                | Kind::FontDialogButton
                | Kind::Entry
                | Kind::SearchEntry
                | Kind::PasswordEntry
                | Kind::SpinButton
                | Kind::EditableLabel
                | Kind::TextView
                | Kind::Scale
                | Kind::Calendar
                | Kind::Expander
                | Kind::Paned
                | Kind::Notebook
                | Kind::NotebookTab
                | Kind::ListBoxRow
                | Kind::FlowBoxChild
                | Kind::ListView
                | Kind::GridView
                | Kind::ColumnView
                | Kind::PopoverMenuItem
        )
    }

    /// Every kind, in declaration order.
    #[must_use]
    pub fn all() -> &'static [Kind] {
        &[
            Kind::Label,
            Kind::Spinner,
            Kind::Statusbar,
            Kind::LevelBar,
            Kind::ProgressBar,
            Kind::InfoBar,
            Kind::Scrollbar,
            Kind::Image,
            Kind::Picture,
            Kind::Separator,
            Kind::TextView,
            Kind::Scale,
            Kind::DrawingArea,
            Kind::WindowControls,
            Kind::Calendar,
            Kind::Popover,
            Kind::Button,
            Kind::ToggleButton,
            Kind::LinkButton,
            Kind::CheckButton,
            Kind::MenuButton,
            Kind::Switch,
            Kind::DropDown,
            Kind::ColorDialogButton,
            Kind::ColorDialog,
            Kind::FontDialogButton,
            Kind::FontDialog,
            Kind::Entry,
            Kind::SearchEntry,
            Kind::PasswordEntry,
            Kind::SpinButton,
            Kind::EditableLabel,
            Kind::Box,
            Kind::Grid,
            Kind::CenterBox,
            Kind::ScrolledWindow,
            Kind::Paned,
            Kind::Frame,
            Kind::Expander,
            Kind::SearchBar,
            Kind::ActionBar,
            Kind::HeaderBar,
            Kind::Notebook,
            Kind::NotebookTab,
            Kind::Overlay,
            Kind::Stack,
            Kind::StackPage,
            Kind::StackSwitcher,
            Kind::StackSidebar,
            Kind::ListBox,
            Kind::ListBoxRow,
            Kind::FlowBox,
            Kind::FlowBoxChild,
            Kind::ListView,
            Kind::GridView,
            Kind::ColumnView,
            Kind::ColumnViewColumn,
            Kind::PopoverMenu,
            Kind::PopoverMenuBar,
            Kind::PopoverMenuItem,
            Kind::Window,
            Kind::ShortcutsWindow,
            Kind::AboutDialog,
            Kind::AlertDialog,
        ]
    }
}

/// A child's identity across frames.
///
/// Identity is what keeps a row's animation, focus, shaping cache and
/// attached popup alive when the list around it is edited. The three
/// constructors are deliberately distinct variants: `Key::from(3usize)` and
/// `Key::from(3u64)` are different keys, so a positional index can never
/// alias a model id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A positional index the caller assigned explicitly.
    Index(usize),
    /// A model identity.
    Id(u64),
    /// A name.
    Name(Rc<str>),
}

impl From<usize> for Key {
    fn from(value: usize) -> Self {
        Key::Index(value)
    }
}
impl From<u64> for Key {
    fn from(value: u64) -> Self {
        Key::Id(value)
    }
}
impl From<&str> for Key {
    fn from(value: &str) -> Self {
        Key::Name(Rc::from(value))
    }
}
impl From<String> for Key {
    fn from(value: String) -> Self {
        Key::Name(Rc::from(value.as_str()))
    }
}

/// The events a widget can produce a message from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum EventKind {
    Click,
    Activate,
    Toggle,
    Change,
    Selected,
    ValueChanged,
    PageChanged,
    Expanded,
    Scrolled,
    Close,
    Response,
    FocusIn,
    FocusOut,
    KeyPressed,
    ActivateLink,
    Reordered,
    Search,
    DateSelected,
    /// A raw pointer press on this node (M5-D5). Generic: any kind may carry it.
    PointerDown,
    /// A raw pointer motion, delivered to the node that took the press for as
    /// long as the implicit grab holds.
    PointerMotion,
    /// A raw pointer release, delivered even when it lands outside the node.
    PointerUp,
}

impl EventKind {
    /// Every event kind, in declaration order.
    pub const ALL: &'static [EventKind] = &[
        EventKind::Click,
        EventKind::Activate,
        EventKind::Toggle,
        EventKind::Change,
        EventKind::Selected,
        EventKind::ValueChanged,
        EventKind::PageChanged,
        EventKind::Expanded,
        EventKind::Scrolled,
        EventKind::Close,
        EventKind::Response,
        EventKind::FocusIn,
        EventKind::FocusOut,
        EventKind::KeyPressed,
        EventKind::ActivateLink,
        EventKind::Reordered,
        EventKind::Search,
        EventKind::DateSelected,
        EventKind::PointerDown,
        EventKind::PointerMotion,
        EventKind::PointerUp,
    ];
}

/// How one event turns into a message.
///
/// Contract deviation D13: the six variants are §4.4's, verbatim. §5's
/// Calendar (`(i32, u32, u32)`) and Notebook (`(usize, usize)`) handler
/// shapes have no variant to live in, so `on_date_selected` uses `Text` with
/// an ISO-8601 `YYYY-MM-DD` payload and `on_reordered` uses `Index` with the
/// destination index.
#[allow(
    clippy::type_complexity,
    reason = "Handler::Key's closure type names KeyEvent and Option<Msg>; a \
              type alias would need its own Msg parameter and add a layer \
              of indirection for a single variant"
)]
pub enum Handler<Msg> {
    /// A constant message, cloned on every fire.
    Unit(Msg),
    /// From the widget's text.
    Text(Rc<dyn Fn(&str) -> Msg>),
    /// From a boolean state.
    Bool(Rc<dyn Fn(bool) -> Msg>),
    /// From an index into a model or a page list.
    Index(Rc<dyn Fn(usize) -> Msg>),
    /// From a numeric value.
    Float(Rc<dyn Fn(f64) -> Msg>),
    /// From a key event; `None` means "not mine, keep bubbling".
    Key(Rc<dyn Fn(&crate::window::keyboard::KeyEvent) -> Option<Msg>>),
    /// Two floats, for `on_scrolled((x, y))` (deviation P6-D4).
    Pair(Rc<dyn Fn(f64, f64) -> Msg>),
    /// Two indices, for `on_reordered((from, to))` (deviation P6-D4).
    Indices(Rc<dyn Fn(usize, usize) -> Msg>),
    /// Local `(x, y)` plus the Linux input-event button code — the shape
    /// middle-click-to-close and a canvas drag need (M5-D5).
    PairButton(Rc<dyn Fn(f64, f64, u32) -> Msg>),
}

impl<Msg> Clone for Handler<Msg>
where
    Msg: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Handler::Unit(msg) => Handler::Unit(msg.clone()),
            Handler::Text(f) => Handler::Text(Rc::clone(f)),
            Handler::Bool(f) => Handler::Bool(Rc::clone(f)),
            Handler::Index(f) => Handler::Index(Rc::clone(f)),
            Handler::Float(f) => Handler::Float(Rc::clone(f)),
            Handler::Key(f) => Handler::Key(Rc::clone(f)),
            Handler::Pair(f) => Handler::Pair(Rc::clone(f)),
            Handler::Indices(f) => Handler::Indices(Rc::clone(f)),
            Handler::PairButton(f) => Handler::PairButton(Rc::clone(f)),
        }
    }
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for Handler<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Handler::Unit(msg) => f.debug_tuple("Unit").field(msg).finish(),
            Handler::Text(_) => f.write_str("Text(..)"),
            Handler::Bool(_) => f.write_str("Bool(..)"),
            Handler::Index(_) => f.write_str("Index(..)"),
            Handler::Float(_) => f.write_str("Float(..)"),
            Handler::Key(_) => f.write_str("Key(..)"),
            Handler::Pair(_) => f.write_str("Pair(..)"),
            Handler::Indices(_) => f.write_str("Indices(..)"),
            Handler::PairButton(_) => f.write_str("PairButton(..)"),
        }
    }
}

/// A widget's event-to-message bindings, sorted by [`EventKind`].
///
/// The reconciler replaces the whole set every frame (contract §4.7):
/// handlers close over the current model, so diffing them would be both
/// impossible and pointless.
pub struct Handlers<Msg>(Vec<(EventKind, Handler<Msg>)>);

impl<Msg> Default for Handlers<Msg> {
    // Not `#[derive]`: that would demand `Msg: Default`.
    fn default() -> Self {
        Handlers(Vec::new())
    }
}

impl<Msg: Clone> Clone for Handlers<Msg> {
    fn clone(&self) -> Self {
        Handlers(self.0.clone())
    }
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for Handlers<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl<Msg: Clone + 'static> Handlers<Msg> {
    /// Bind `kind`, replacing any previous binding.
    pub fn set(&mut self, kind: EventKind, handler: Handler<Msg>) {
        match self.0.binary_search_by_key(&kind, |(k, _)| *k) {
            Ok(index) => self.0[index].1 = handler,
            Err(index) => self.0.insert(index, (kind, handler)),
        }
    }

    /// Whether `kind` is bound at all.
    #[must_use]
    pub fn has(&self, kind: EventKind) -> bool {
        self.get(kind).is_some()
    }

    /// How many events are bound.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, kind: EventKind) -> Option<&Handler<Msg>> {
        self.0
            .binary_search_by_key(&kind, |(k, _)| *k)
            .ok()
            .map(|index| &self.0[index].1)
    }

    /// Fire a parameterless event. `None` when nothing is bound, or when
    /// what is bound wants a value this call cannot supply.
    #[must_use]
    pub fn fire_unit(&self, kind: EventKind) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a text event.
    #[must_use]
    pub fn fire_text(&self, kind: EventKind, value: &str) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Text(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a boolean event.
    #[must_use]
    pub fn fire_bool(&self, kind: EventKind, value: bool) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Bool(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire an index event.
    #[must_use]
    pub fn fire_index(&self, kind: EventKind, value: usize) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Index(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a numeric event.
    #[must_use]
    pub fn fire_float(&self, kind: EventKind, value: f64) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Float(f) => Some(f(value)),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }

    /// Fire a two-float handler; `None` when nothing of that arity is bound.
    ///
    /// Unlike [`Handlers::fire_text`] and friends there is no `Unit`
    /// fallthrough: a `Unit` binding on `Scrolled` is a builder that meant a
    /// different arity, and firing it would emit the wrong message.
    #[must_use]
    pub fn fire_pair(&self, kind: EventKind, a: f64, b: f64) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Pair(f) => Some(f(a, b)),
            _ => None,
        }
    }

    /// Fire a `PairButton` **or** a `Pair` handler registered for `kind`.
    ///
    /// A `Pair` receives `(x, y)` and the button is dropped, so
    /// `on_pointer_down` and `on_pointer_down_with_button` can sit on
    /// different nodes without the caller choosing a fire method. As with
    /// [`Handlers::fire_pair`] there is no `Unit` fallthrough: a `Unit`
    /// binding on a pointer kind is a builder that meant a different arity.
    ///
    /// `button` is `0` for a motion, which carries none (P0-D2); no real
    /// `BTN_*` code is zero.
    #[must_use]
    pub fn fire_pair_button(&self, kind: EventKind, x: f64, y: f64, button: u32) -> Option<Msg> {
        match self.get(kind)? {
            Handler::PairButton(f) => Some(f(x, y, button)),
            Handler::Pair(f) => Some(f(x, y)),
            _ => None,
        }
    }

    /// Fire a two-index handler; `None` when nothing of that arity is bound.
    #[must_use]
    pub fn fire_indices(&self, kind: EventKind, a: usize, b: usize) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Indices(f) => Some(f(a, b)),
            _ => None,
        }
    }

    /// Fire a key event. `None` both when nothing is bound and when the
    /// bound closure declined the key, so the caller keeps bubbling.
    #[must_use]
    pub fn fire_key(&self, kind: EventKind, ev: &crate::window::keyboard::KeyEvent) -> Option<Msg> {
        match self.get(kind)? {
            Handler::Key(f) => f(ev),
            Handler::Unit(msg) => Some(msg.clone()),
            _ => None,
        }
    }
}

/// An immutable description of one widget and its subtree.
///
/// A `view(&Model) -> View<Msg>` is pure: it allocates a fresh tree every
/// frame and the reconciler (`view::reconcile`) diffs it into the retained
/// [`Node`](crate::css::node::Node)s, so identity — animation, focus,
/// shaping caches, attached popups — lives in the `Instance` tree, never here.
pub struct View<Msg> {
    /// Which widget.
    pub kind: Kind,
    /// The identity that survives an edit of the surrounding list.
    pub key: Option<Key>,
    /// Typed properties.
    pub props: Props,
    /// Child views, in order.
    pub children: Vec<View<Msg>>,
    /// Event-to-message bindings.
    pub handlers: Handlers<Msg>,
}

impl<Msg: std::fmt::Debug> std::fmt::Debug for View<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("props", &self.props)
            .field("handlers", &self.handlers)
            .field("children", &self.children)
            .finish()
    }
}

impl<Msg: Clone + 'static> View<Msg> {
    /// An empty view of `kind`.
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        View {
            kind,
            key: None,
            props: Props::default(),
            children: Vec::new(),
            handlers: Handlers::default(),
        }
    }

    /// Give this view an identity for keyed reconciliation.
    #[must_use]
    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Append one child.
    #[must_use]
    pub fn child(mut self, child: View<Msg>) -> Self {
        self.children.push(child);
        self
    }

    /// Append many children, in iteration order.
    #[must_use]
    pub fn children(mut self, children: impl IntoIterator<Item = View<Msg>>) -> Self {
        self.children.extend(children);
        self
    }

    /// Set one property.
    #[must_use]
    pub fn prop(mut self, name: PropName, value: impl Into<Prop>) -> Self {
        self.props.set(name, value.into());
        self
    }

    /// Bind one event.
    #[must_use]
    pub fn on(mut self, event: EventKind, handler: Handler<Msg>) -> Self {
        self.handlers.set(event, handler);
        self
    }

    /// Append one CSS class, after anything `classes` already added.
    #[must_use]
    pub fn class(mut self, class: &str) -> Self {
        let mut list: Vec<Rc<str>> = match self.props.get(PropName::Classes) {
            Some(Prop::Classes(existing)) => existing.to_vec(),
            _ => Vec::new(),
        };
        list.push(Rc::from(class));
        self.props
            .set(PropName::Classes, Prop::Classes(Rc::from(list)));
        self
    }

    /// Append several CSS classes, in order.
    #[must_use]
    pub fn classes(mut self, classes: &[&str]) -> Self {
        let mut list: Vec<Rc<str>> = match self.props.get(PropName::Classes) {
            Some(Prop::Classes(existing)) => existing.to_vec(),
            _ => Vec::new(),
        };
        list.extend(classes.iter().map(|c| Rc::from(*c)));
        self.props
            .set(PropName::Classes, Prop::Classes(Rc::from(list)));
        self
    }

    /// Set the CSS id.
    #[must_use]
    pub fn id(self, id: &str) -> Self {
        self.prop(PropName::Id, id)
    }

    /// GTK's `visible`. An invisible widget keeps its `Node` and its
    /// controller but is skipped by layout, paint and hit-testing.
    #[must_use]
    pub fn visible(self, on: bool) -> Self {
        self.prop(PropName::Visible, on)
    }

    /// GTK's `sensitive`. Insensitive sets `PseudoStates::DISABLED` and
    /// takes the node out of the focus ring and the hit-test.
    #[must_use]
    pub fn sensitive(self, on: bool) -> Self {
        self.prop(PropName::Sensitive, on)
    }

    /// Override [`Kind::is_focusable_by_default`].
    #[must_use]
    pub fn focusable(self, on: bool) -> Self {
        self.prop(PropName::Focusable, on)
    }

    /// Tooltip text.
    #[must_use]
    pub fn tooltip(self, text: &str) -> Self {
        self.prop(PropName::Tooltip, text)
    }

    /// Horizontal alignment within the parent's allocation.
    #[must_use]
    pub fn halign(self, a: Align) -> Self {
        self.prop(PropName::Halign, a)
    }

    /// Vertical alignment within the parent's allocation.
    #[must_use]
    pub fn valign(self, a: Align) -> Self {
        self.prop(PropName::Valign, a)
    }

    /// Take any extra horizontal space the parent has.
    #[must_use]
    pub fn hexpand(self, on: bool) -> Self {
        self.prop(PropName::Hexpand, on)
    }

    /// Take any extra vertical space the parent has.
    #[must_use]
    pub fn vexpand(self, on: bool) -> Self {
        self.prop(PropName::Vexpand, on)
    }

    /// Margins, in px, in CSS order.
    #[must_use]
    pub fn margin(self, top: i32, right: i32, bottom: i32, left: i32) -> Self {
        self.prop(PropName::Margin, Prop::Edges([top, right, bottom, left]))
    }

    /// GTK's `width-request` — a minimum, not a fixed size.
    #[must_use]
    pub fn width_request(self, px: i32) -> Self {
        self.prop(PropName::WidthRequest, px)
    }

    /// GTK's `height-request`.
    #[must_use]
    pub fn height_request(self, px: i32) -> Self {
        self.prop(PropName::HeightRequest, px)
    }

    /// A CSS `cursor` keyword; mapped to a `wp_cursor_shape_v1` name by
    /// `window::pointer::cursor_shape_for`.
    #[must_use]
    pub fn cursor(self, name: &str) -> Self {
        self.prop(PropName::Cursor, name)
    }

    /// Widget opacity, `0.0..=1.0`, applied as an inline style override.
    #[must_use]
    pub fn opacity(self, value: f64) -> Self {
        self.prop(PropName::Opacity, value.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum TestMsg {
        Ok,
        Named(String),
        Toggled(bool),
        Picked(usize),
        Moved(u64),
    }

    #[test]
    fn keys_convert_from_the_three_identity_shapes() {
        assert_eq!(Key::from(3usize), Key::Index(3));
        assert_eq!(Key::from(9_u64), Key::Id(9));
        assert_eq!(Key::from("row-a"), Key::Name(Rc::from("row-a")));
        // Distinct constructors never collide, even at the same number.
        assert_ne!(Key::from(3usize), Key::from(3_u64));
    }

    #[test]
    fn a_unit_handler_clones_its_message_on_every_fire() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(EventKind::Click, Handler::Unit(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
        assert_eq!(handlers.fire_unit(EventKind::Activate), None);
    }

    #[test]
    fn typed_fires_only_match_their_own_handler_variant() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(
            EventKind::Change,
            Handler::Text(Rc::new(|s: &str| TestMsg::Named(s.to_owned()))),
        );
        handlers.set(EventKind::Toggle, Handler::Bool(Rc::new(TestMsg::Toggled)));
        handlers.set(
            EventKind::Selected,
            Handler::Index(Rc::new(TestMsg::Picked)),
        );
        handlers.set(
            EventKind::ValueChanged,
            Handler::Float(Rc::new(|v: f64| TestMsg::Moved(v as u64))),
        );

        assert_eq!(
            handlers.fire_text(EventKind::Change, "hi"),
            Some(TestMsg::Named("hi".into()))
        );
        assert_eq!(
            handlers.fire_bool(EventKind::Toggle, true),
            Some(TestMsg::Toggled(true))
        );
        assert_eq!(
            handlers.fire_index(EventKind::Selected, 2),
            Some(TestMsg::Picked(2))
        );
        assert_eq!(
            handlers.fire_float(EventKind::ValueChanged, 5.0),
            Some(TestMsg::Moved(5))
        );

        // A mismatched fire is a controller bug and yields nothing rather
        // than firing the wrong message.
        assert_eq!(handlers.fire_unit(EventKind::Change), None);
        assert_eq!(handlers.fire_bool(EventKind::Change, true), None);
    }

    #[test]
    fn setting_the_same_event_twice_replaces_rather_than_stacks() {
        let mut handlers: Handlers<TestMsg> = Handlers::default();
        handlers.set(EventKind::Click, Handler::Unit(TestMsg::Ok));
        handlers.set(
            EventKind::Click,
            Handler::Unit(TestMsg::Named("second".into())),
        );
        assert_eq!(handlers.len(), 1);
        assert_eq!(
            handlers.fire_unit(EventKind::Click),
            Some(TestMsg::Named("second".into()))
        );
        assert!(handlers.has(EventKind::Click));
        assert!(!handlers.has(EventKind::Close));
    }

    #[test]
    fn the_event_kind_table_is_the_contract_s_eighteen_plus_m5_d5s_three() {
        assert_eq!(EventKind::ALL.len(), 21);
        assert_eq!(EventKind::ALL[0], EventKind::Click);
        assert_eq!(EventKind::ALL[17], EventKind::DateSelected);
        assert_eq!(EventKind::ALL[20], EventKind::PointerUp);
        let mut sorted = EventKind::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 21);
    }

    #[test]
    fn props_are_kept_sorted_and_set_overwrites_in_place() {
        let mut props = Props::default();
        props.set(PropName::Label, Prop::Str("Ok".into()));
        props.set(
            PropName::Classes,
            Prop::Classes(Rc::from([Rc::from("flat")])),
        );
        props.set(PropName::Label, Prop::Str("Cancel".into()));

        let names: Vec<PropName> = props.iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec![PropName::Classes, PropName::Label]);
        assert_eq!(props.str(PropName::Label), Some("Cancel"));
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn typed_accessors_fall_back_when_the_variant_is_wrong() {
        let mut props = Props::default();
        props.set(PropName::Active, Prop::Str("yes".into()));
        props.set(PropName::Value, Prop::Int(7));

        // A `Str` under `Active` is a caller bug, not a panic.
        assert!(!props.bool(PropName::Active, false));
        // `Int` widens to `float`, because a builder may hand either.
        assert!((props.float(PropName::Value, 0.0) - 7.0).abs() < f64::EPSILON);
        assert_eq!(props.int(PropName::Value, 0), 7);
        assert_eq!(props.str(PropName::Value), None);
        assert_eq!(props.int(PropName::Digits, -1), -1);
    }

    #[test]
    fn diff_reports_changed_added_and_removed_names_sorted() {
        let mut prev = Props::default();
        prev.set(PropName::Label, Prop::Str("Ok".into()));
        prev.set(PropName::Sensitive, Prop::Bool(true));
        prev.set(PropName::Tooltip, Prop::Str("gone".into()));

        let mut next = Props::default();
        next.set(PropName::Label, Prop::Str("Ok".into())); // unchanged
        next.set(PropName::Sensitive, Prop::Bool(false)); // changed
        next.set(PropName::Visible, Prop::Bool(true)); // added

        assert_eq!(
            next.diff(&prev),
            vec![PropName::Visible, PropName::Sensitive, PropName::Tooltip]
                .into_iter()
                .collect::<Vec<_>>()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
            "diff must report added, changed and removed names, sorted"
        );
    }

    #[test]
    #[allow(
        clippy::type_complexity,
        reason = "matches Prop::Draw's own signature; a type alias would only hide it"
    )]
    fn draw_props_compare_by_pointer_not_by_call() {
        let f: Rc<
            dyn Fn(
                &mut skia_rs_safe::canvas::Canvas<'_>,
                crate::layout::Rect,
                &mut crate::paint::PaintCx<'_>,
            ),
        > = Rc::new(|_, _, _| {});
        let g: Rc<
            dyn Fn(
                &mut skia_rs_safe::canvas::Canvas<'_>,
                crate::layout::Rect,
                &mut crate::paint::PaintCx<'_>,
            ),
        > = Rc::new(|_, _, _| {});
        assert_eq!(Prop::Draw(Rc::clone(&f)), Prop::Draw(Rc::clone(&f)));
        assert_ne!(Prop::Draw(f), Prop::Draw(g));
    }

    #[test]
    fn the_prop_name_table_is_the_contract_s_ninety_seven() {
        assert_eq!(PropName::ALL.len(), 97);
        assert_eq!(PropName::ALL[0], PropName::Classes);
        assert_eq!(PropName::ALL[96], PropName::MessageType);
        let mut sorted = PropName::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            97,
            "PropName has a duplicate or an ordering gap"
        );
    }

    #[test]
    fn align_defaults_to_fill() {
        assert_eq!(crate::layout::Align::default(), crate::layout::Align::Fill);
    }

    #[test]
    fn every_kind_is_in_all_exactly_once() {
        // Contract deviation D14: §8.4 R1 estimates "~59"; §4.2's own
        // enumeration is 64, and the enumeration is the normative one.
        assert_eq!(Kind::all().len(), 64);
        let mut seen: Vec<Kind> = Kind::all().to_vec();
        seen.sort_unstable_by_key(|k| *k as usize);
        seen.dedup();
        assert_eq!(seen.len(), 64, "Kind::all has a duplicate");
        assert_eq!(Kind::all()[0], Kind::Label);
        assert_eq!(Kind::all()[63], Kind::AlertDialog);
    }

    #[test]
    fn kinds_that_share_a_css_name_are_told_apart_by_their_base_classes() {
        assert_eq!(Kind::Button.css_name(), "button");
        assert_eq!(Kind::ToggleButton.css_name(), "button");
        assert_eq!(Kind::LinkButton.css_name(), "button");
        assert_eq!(Kind::Button.base_classes(), &[] as &[&str]);
        assert_eq!(Kind::ToggleButton.base_classes(), &["toggle"]);
        assert_eq!(Kind::LinkButton.base_classes(), &["link"]);

        assert_eq!(Kind::Box.css_name(), "box");
        assert_eq!(Kind::CenterBox.css_name(), "box");

        assert_eq!(Kind::Entry.css_name(), "entry");
        assert_eq!(Kind::SearchEntry.base_classes(), &["search"]);
        assert_eq!(Kind::PasswordEntry.base_classes(), &["password"]);

        assert_eq!(Kind::Window.css_name(), "window");
        assert_eq!(Kind::Window.base_classes(), &["background"]);
        assert_eq!(Kind::AlertDialog.base_classes(), &["dialog", "message"]);
        assert_eq!(Kind::PopoverMenu.base_classes(), &["background", "menu"]);
    }

    #[test]
    fn no_css_name_is_empty_and_none_carries_a_dot_or_a_space() {
        for kind in Kind::all() {
            let name = kind.css_name();
            assert!(!name.is_empty(), "{kind:?} has no CSS node name");
            assert!(
                !name.contains('.') && !name.contains(' '),
                "{kind:?}'s node name {name:?} smuggles a class in"
            );
            for class in kind.base_classes() {
                assert!(
                    !class.is_empty() && !class.starts_with('.'),
                    "{kind:?} base class {class:?} must be bare"
                );
            }
        }
    }

    #[test]
    fn a_new_view_carries_only_its_kind() {
        let v: View<TestMsg> = View::new(Kind::Button);
        assert_eq!(v.kind, Kind::Button);
        assert_eq!(v.key, None);
        assert!(v.props.is_empty());
        assert!(v.children.is_empty());
        assert!(v.handlers.is_empty());
    }

    #[test]
    fn universal_setters_write_the_props_they_are_named_for() {
        let v: View<TestMsg> = View::new(Kind::Label)
            .id("title")
            .classes(&["heading", "dim-label"])
            .class("extra")
            .visible(false)
            .sensitive(false)
            .focusable(true)
            .tooltip("a tip")
            .halign(crate::layout::Align::Start)
            .valign(crate::layout::Align::Center)
            .hexpand(true)
            .vexpand(false)
            .margin(1, 2, 3, 4)
            .width_request(120)
            .height_request(24)
            .cursor("pointer")
            .opacity(0.5);

        assert_eq!(v.props.str(PropName::Id), Some("title"));
        let Some(Prop::Classes(classes)) = v.props.get(PropName::Classes) else {
            panic!("classes were not stored as Prop::Classes");
        };
        assert_eq!(
            classes.iter().map(|c| &**c).collect::<Vec<_>>(),
            vec!["heading", "dim-label", "extra"],
            "`class` appends to `classes`, in call order"
        );
        assert!(!v.props.bool(PropName::Visible, true));
        assert!(!v.props.bool(PropName::Sensitive, true));
        assert!(v.props.bool(PropName::Focusable, false));
        assert_eq!(v.props.str(PropName::Tooltip), Some("a tip"));
        assert_eq!(
            v.props.get(PropName::Halign),
            Some(&Prop::Align(crate::layout::Align::Start))
        );
        assert_eq!(
            v.props.get(PropName::Valign),
            Some(&Prop::Align(crate::layout::Align::Center))
        );
        assert!(v.props.bool(PropName::Hexpand, false));
        assert!(!v.props.bool(PropName::Vexpand, true));
        assert_eq!(
            v.props.get(PropName::Margin),
            Some(&Prop::Edges([1, 2, 3, 4]))
        );
        assert_eq!(v.props.int(PropName::WidthRequest, 0), 120);
        assert_eq!(v.props.int(PropName::HeightRequest, 0), 24);
        assert_eq!(v.props.str(PropName::Cursor), Some("pointer"));
        assert!((v.props.float(PropName::Opacity, 1.0) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn children_and_keys_chain_in_order() {
        let v: View<TestMsg> = View::new(Kind::Box)
            .child(View::new(Kind::Label).key("a"))
            .children([
                View::new(Kind::Button).key("b"),
                View::new(Kind::Button).key(7_u64),
            ]);
        assert_eq!(v.children.len(), 3);
        assert_eq!(v.children[0].key, Some(Key::Name(Rc::from("a"))));
        assert_eq!(v.children[1].key, Some(Key::Name(Rc::from("b"))));
        assert_eq!(v.children[2].key, Some(Key::Id(7)));
        assert_eq!(v.children[2].kind, Kind::Button);
    }

    #[test]
    fn on_binds_a_handler_the_view_can_fire() {
        let v: View<TestMsg> =
            View::new(Kind::Button).on(EventKind::Click, Handler::Unit(TestMsg::Ok));
        assert_eq!(v.handlers.fire_unit(EventKind::Click), Some(TestMsg::Ok));
    }

    #[test]
    fn setting_a_universal_prop_twice_keeps_the_last_value() {
        let v: View<TestMsg> = View::new(Kind::Label).tooltip("first").tooltip("second");
        assert_eq!(v.props.str(PropName::Tooltip), Some("second"));
        assert_eq!(v.props.len(), 1);
    }

    #[test]
    fn the_two_new_handler_arities_fire_and_the_others_do_not() {
        // Mutation check: making fire_pair fall through to fire_unit returns
        // Some for the Indices handler too, and a scroll event would emit a
        // reorder message.
        let mut h: Handlers<(u32, u32)> = Handlers::default();
        h.set(
            EventKind::Scrolled,
            Handler::Pair(Rc::new(|a, b| (a as u32, b as u32))),
        );
        h.set(
            EventKind::Reordered,
            Handler::Indices(Rc::new(|a, b| (a as u32, b as u32))),
        );
        assert_eq!(h.fire_pair(EventKind::Scrolled, 3.0, 4.0), Some((3, 4)));
        assert_eq!(h.fire_indices(EventKind::Reordered, 1, 2), Some((1, 2)));
        assert_eq!(h.fire_pair(EventKind::Reordered, 1.0, 2.0), None);
        assert_eq!(h.fire_unit(EventKind::Scrolled), None);
    }

    #[test]
    fn a_factory_prop_compares_by_pointer_like_draw() {
        // Mutation check: comparing factories as always-equal makes
        // Props::diff miss a model swap and the list keeps the old rows.
        use crate::widgets::types::{ItemFactory, ListItem, RowContent};
        let f = ItemFactory::new(|_, item: &ListItem| RowContent::from_label(&item.text));
        let g = ItemFactory::new(|_, item: &ListItem| RowContent::from_label(&item.text));
        assert_eq!(Prop::Factory(f.clone()), Prop::Factory(f));
        assert_ne!(
            Prop::Factory(ItemFactory::new(|_, i: &ListItem| RowContent::from_label(
                &i.text
            ))),
            Prop::Factory(g)
        );
    }

    #[test]
    fn every_p6_prop_name_is_distinct_and_ordered() {
        // Mutation check: a duplicated variant name would not compile, but a
        // duplicated *use* (two builders writing PropName::Position for
        // different meanings) shows up as a Props::set collision; this pins
        // the count so an accidental deletion during a rebase is caught.
        let names = PropName::all_p6();
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate P6 PropName");
        // Task 20 adds `PropName::Sorter` -- see its own doc comment for why
        // no existing P6 name could carry a `ColumnViewColumn` comparator.
        assert_eq!(names.len(), 90);
    }

    #[test]
    fn the_focusable_default_follows_gtk_not_the_node_name() {
        assert!(Kind::Button.is_focusable_by_default());
        assert!(Kind::Entry.is_focusable_by_default());
        assert!(Kind::ListBoxRow.is_focusable_by_default());
        // Contract §3.5: WindowControls' three buttons are never candidates.
        assert!(!Kind::WindowControls.is_focusable_by_default());
        assert!(!Kind::Label.is_focusable_by_default());
        assert!(!Kind::Box.is_focusable_by_default());
        assert!(!Kind::ListBox.is_focusable_by_default());
        assert_eq!(
            Kind::all()
                .iter()
                .filter(|k| k.is_focusable_by_default())
                .count(),
            27
        );
    }
}
