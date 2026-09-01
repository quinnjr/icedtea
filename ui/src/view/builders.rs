//! Free-function widget builders and the event setters every one of them
//! chains.
//!
//! # The builder naming rule (normative — contract §4.4)
//!
//! P4 ships this frame; P5 and P6 fill it, and must follow the rule exactly.
//!
//! * A widget `Foo` gets **one** free constructor `fn foo(..) -> View<Msg>`
//!   in this module, named in `snake_case` from the GTK class name with the
//!   `Gtk` prefix dropped: `GtkCheckButton` → `check_button`,
//!   `GtkColorDialogButton` → `color_dialog_button`.
//! * Its required content is a **positional** argument: `button("Ok")`,
//!   `label("Hi")`, `scale(0.0, 100.0)`.
//! * Everything else is a chained `self`-consuming setter named after the
//!   **GTK property**, in `snake_case`, with no `set_` prefix:
//!   `.wrap(true)`, `.show_text(true)`, `.max_length(32)`.
//! * Handlers are `on_<eventkind snake_case>`, taking a `Msg` for
//!   [`Handler::Unit`] and a closure otherwise. All eighteen live in this
//!   file, on `View<Msg>` itself, so every builder inherits them.
//! * Every builder returns [`View<Msg>`], so §4.1's `.class()`, `.margin()`
//!   and `.key()` chain after it.

use crate::view::{EventKind, Handler, Kind, Prop, PropName, View};
use crate::widgets::WidgetEnum as _;
use crate::widgets::types::Orientation;
use crate::window::keyboard::KeyEvent;
use std::rc::Rc;

/// A bare [`View`] of `kind`, with no props and no handlers.
///
/// The generic constructor every named builder in P5/P6 starts from, and the
/// only one P4 itself needs.
#[must_use]
pub fn widget<Msg: Clone + 'static>(kind: Kind) -> View<Msg> {
    View::new(kind)
}

/// `GtkBox`. `box` is a keyword, hence the trailing underscore -- the one
/// place the snake_case rule cannot be followed literally.
#[must_use]
pub fn box_<Msg: Clone + 'static>(
    orientation: Orientation,
    children: impl IntoIterator<Item = View<Msg>>,
) -> View<Msg> {
    View::new(Kind::Box)
        .prop(PropName::Orientation, orientation)
        .children(children)
}

/// Tag `view` as filling a named slot of its parent (`"start"`, `"end"`,
/// `"center"`, `"overlay"`, `"titlebar"`, `"title"`, `"label"`).
///
/// One mechanism for every container that has more than one place to put a
/// child: `HeaderBar`, `ActionBar`, `CenterBox`, `Overlay`, `Paned`,
/// `Frame` and `Window` all read it.
#[must_use]
pub fn slot<Msg: Clone + 'static>(view: View<Msg>, name: &str) -> View<Msg> {
    view.prop(PropName::Section, Prop::Str(Rc::from(name)))
}

/// `GtkCenterBox`: exactly three children, in `start`, `centre`, `end` order.
#[must_use]
pub fn center_box<Msg: Clone + 'static>(
    start: View<Msg>,
    center: View<Msg>,
    end: View<Msg>,
) -> View<Msg> {
    View::new(Kind::CenterBox).children([
        slot(start, "start"),
        slot(center, "center"),
        slot(end, "end"),
    ])
}

/// `GtkGrid`.
///
/// `GridC::build` has no `View` to derive track counts from -- only the
/// retained `Node` and `Props` -- so this builder is where the children's
/// own placements raise `Columns`/`Rows` before the props ever reach the
/// controller (`GridC::extent_of`'s doc comment).
#[must_use]
pub fn grid<Msg: Clone + 'static>(children: impl IntoIterator<Item = View<Msg>>) -> View<Msg> {
    let view = View::new(Kind::Grid).children(children);
    let (columns, rows) = crate::widgets::grid::GridC::extent_of(&view);
    view.prop(PropName::Columns, i64::from(columns))
        .prop(PropName::Rows, i64::from(rows))
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkGrid:row-spacing`.
    #[must_use]
    pub fn row_spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::RowSpacing, v)
    }

    /// `GtkGrid:column-spacing`.
    #[must_use]
    pub fn column_spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ColumnSpacing, v)
    }

    /// `GtkGrid:row-homogeneous`.
    #[must_use]
    pub fn row_homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::RowHomogeneous, v)
    }

    /// `GtkGrid:column-homogeneous`.
    #[must_use]
    pub fn column_homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ColumnHomogeneous, v)
    }

    /// `GtkGrid:baseline-row`.
    #[must_use]
    pub fn baseline_row(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::BaselineRow, v)
    }

    /// This child's grid cell, zero-based (`gtk_grid_attach`'s first two
    /// arguments).
    #[must_use]
    pub fn at(self, column: u16, row: u16) -> Self {
        self.prop(PropName::Column, i64::from(column))
            .prop(PropName::Row, i64::from(row))
    }

    /// How many cells this child covers (`gtk_grid_attach`'s last two).
    #[must_use]
    pub fn span(self, columns: u16, rows: u16) -> Self {
        self.prop(PropName::ColumnSpan, i64::from(columns.max(1)))
            .prop(PropName::RowSpan, i64::from(rows.max(1)))
    }
}

/// Contract deviation (Task 4): every P6 setter takes `impl Into<Prop>`
/// rather than a concrete type. `View<Msg>` has one inherent-method
/// namespace across the whole widget catalogue, and GTK gives `position`
/// an `int` on `GtkPaned` and a `GtkPositionType` on `GtkPopover` -- two
/// different Rust types under one setter name, which only an `Into<Prop>`
/// argument can host. These are the scalar conversions; one more
/// `From<T> for Prop` follows per widget-local `#[repr(u16)]` enum.
impl From<u32> for Prop {
    fn from(v: u32) -> Self {
        Prop::Int(i64::from(v))
    }
}
impl From<f32> for Prop {
    fn from(v: f32) -> Self {
        Prop::Float(f64::from(v))
    }
}
impl From<&[&str]> for Prop {
    fn from(v: &[&str]) -> Self {
        Prop::Classes(v.iter().map(|s| Rc::from(*s)).collect())
    }
}
impl From<Orientation> for Prop {
    fn from(v: Orientation) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::Position> for Prop {
    fn from(v: crate::widgets::Position) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::Side> for Prop {
    fn from(v: crate::widgets::Side) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::Policy> for Prop {
    fn from(v: crate::widgets::types::Policy) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::BaselinePosition> for Prop {
    fn from(v: crate::widgets::types::BaselinePosition) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::SelectionMode> for Prop {
    fn from(v: crate::widgets::types::SelectionMode) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::StackTransition> for Prop {
    fn from(v: crate::widgets::types::StackTransition) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::SortOrder> for Prop {
    fn from(v: crate::widgets::types::SortOrder) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::DisplayHint> for Prop {
    fn from(v: crate::widgets::types::DisplayHint) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::LicenseType> for Prop {
    fn from(v: crate::widgets::types::LicenseType) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::Decoration> for Prop {
    fn from(v: crate::widgets::types::Decoration) -> Self {
        Prop::Enum(v.to_u16())
    }
}
impl From<crate::widgets::types::MenuFlags> for Prop {
    fn from(v: crate::widgets::types::MenuFlags) -> Self {
        Prop::Int(i64::from(v.bits()))
    }
}
impl From<crate::widgets::types::ItemFactory> for Prop {
    fn from(v: crate::widgets::types::ItemFactory) -> Self {
        Prop::Factory(v)
    }
}
impl From<Rc<[crate::view::ListItem]>> for Prop {
    fn from(v: Rc<[crate::view::ListItem]>) -> Self {
        Prop::Items(v)
    }
}

/// `GtkFrame` wrapping `child`.
#[must_use]
pub fn frame<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Frame).child(child)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkFrame:label` (and every other widget's own `label` text prop).
    #[must_use]
    pub fn label(self, text: &str) -> Self {
        self.prop(PropName::Label, Prop::Str(Rc::from(text)))
    }

    /// `GtkFrame:label-xalign`.
    #[must_use]
    pub fn label_xalign(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::LabelXalign, v)
    }

    /// `GtkFrame:label-widget` -- a whole view instead of a text label.
    #[must_use]
    pub fn label_widget(self, view: View<Msg>) -> Self {
        let mut me = self;
        me.children.insert(0, slot(view, "label"));
        me.prop(PropName::LabelWidget, Prop::Bool(true))
    }
}

/// `GtkOverlay` wrapping `child` as its main child.
#[must_use]
pub fn overlay<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Overlay).child(child)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// Add `v` as an overlay child, painted over the main child.
    #[must_use]
    pub fn overlay(mut self, v: View<Msg>) -> Self {
        self.children.push(slot(v, "overlay"));
        self
    }

    /// `GtkOverlay:measure` (per-child, via `gtk_overlay_set_measure_overlay`).
    #[must_use]
    pub fn measure_overlay(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::MeasureOverlay, v)
    }

    /// `GtkOverlay:clip-overlay` (per-child, via `gtk_overlay_set_clip_overlay`).
    #[must_use]
    pub fn clip_overlay(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ClipOverlay, v)
    }
}

/// `GtkExpander` titled `label`, wrapping `child`.
#[must_use]
pub fn expander<Msg: Clone + 'static>(label: &str, child: View<Msg>) -> View<Msg> {
    View::new(Kind::Expander).label(label).child(child)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkExpander:expanded`.
    #[must_use]
    pub fn expanded(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Expanded, v)
    }
}

/// `GtkPaned`: `start` and `end` flank the draggable separator, in that
/// order.
#[must_use]
pub fn paned<Msg: Clone + 'static>(
    orientation: Orientation,
    start: View<Msg>,
    end: View<Msg>,
) -> View<Msg> {
    View::new(Kind::Paned)
        .prop(PropName::Orientation, orientation)
        .children([start, end])
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkPaned:position`.
    #[must_use]
    pub fn position(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Position, v)
    }

    /// `GtkPaned:position-set`.
    #[must_use]
    pub fn position_set(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::PositionSet, v)
    }

    /// `GtkPaned:wide-handle`.
    #[must_use]
    pub fn wide_handle(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::WideHandle, v)
    }

    /// `GtkPaned:resize-start-child`.
    #[must_use]
    pub fn resize_start(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ResizeStart, v)
    }

    /// `GtkPaned:resize-end-child`.
    #[must_use]
    pub fn resize_end(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ResizeEnd, v)
    }

    /// `GtkPaned:shrink-start-child`.
    #[must_use]
    pub fn shrink_start(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShrinkStart, v)
    }

    /// `GtkPaned:shrink-end-child`.
    #[must_use]
    pub fn shrink_end(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShrinkEnd, v)
    }
}

/// `GtkSearchBar` wrapping `child`.
#[must_use]
pub fn search_bar<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::SearchBar).child(child)
}

/// `GtkActionBar`, empty until [`View::pack_start`]/[`View::pack_end`]/
/// [`View::center`] fill it.
#[must_use]
pub fn action_bar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::ActionBar)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// Pack a child at the leading end (`gtk_action_bar_pack_start`,
    /// `gtk_header_bar_pack_start`).
    #[must_use]
    pub fn pack_start(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "start"));
        self
    }

    /// Pack a child at the trailing end.
    #[must_use]
    pub fn pack_end(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "end"));
        self
    }

    /// The centre widget.
    #[must_use]
    pub fn center(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "center"));
        self
    }
}

/// `GtkHeaderBar`, empty until [`View::pack_start`]/[`View::pack_end`]/
/// [`HeaderBarExt::title_widget`] fill it.
#[must_use]
pub fn header_bar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::HeaderBar)
}

/// `GtkHeaderBar`'s own property setters, chained after [`header_bar`].
///
/// Scoped to its own trait, rather than plain inherent `View<Msg>` methods,
/// for the same reason [`crate::widgets::action_bar::ActionBarExt`]'s own
/// doc comment gives: `.title` already exists over the same
/// [`PropName::Title`] for `ColorDialogButton` and `FontDialog`, and an
/// inherent method would silently win over both at every call site.
pub trait HeaderBarExt<Msg>: Sized {
    /// `GtkHeaderBar:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkHeaderBar:subtitle` (GTK 3; kept for the CSS-node shape only).
    fn subtitle(self, text: &str) -> Self;
    /// `GtkHeaderBar:title-widget`.
    fn title_widget(self, view: View<Msg>) -> Self;
    /// `GtkHeaderBar:show-title-buttons`.
    fn show_title_buttons(self, on: bool) -> Self;
    /// `GtkHeaderBar:decoration-layout`.
    fn decoration_layout(self, layout: &str) -> Self;
}

impl<Msg: Clone + 'static> HeaderBarExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn subtitle(self, text: &str) -> Self {
        self.prop(PropName::Subtitle, Prop::Str(Rc::from(text)))
    }
    fn title_widget(mut self, view: View<Msg>) -> Self {
        self.children.push(slot(view, "title"));
        self
    }
    fn show_title_buttons(self, on: bool) -> Self {
        self.prop(PropName::ShowTitleButtons, Prop::Bool(on))
    }
    fn decoration_layout(self, layout: &str) -> Self {
        self.prop(PropName::Decoration, Prop::Str(Rc::from(layout)))
    }
}

/// `GtkScrolledWindow` wrapping `child`.
#[must_use]
pub fn scrolled_window<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::ScrolledWindow).child(child)
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkScrolledWindow:hscrollbar-policy`.
    #[must_use]
    pub fn hscrollbar_policy(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::HscrollbarPolicy, v)
    }

    /// `GtkScrolledWindow:vscrollbar-policy`.
    #[must_use]
    pub fn vscrollbar_policy(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::VscrollbarPolicy, v)
    }

    /// `GtkScrolledWindow:has-frame`.
    #[must_use]
    pub fn has_frame(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::HasFrame, v)
    }

    /// `GtkScrolledWindow:min-content-width`.
    #[must_use]
    pub fn min_content_width(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::MinContentWidth, v)
    }

    /// `GtkScrolledWindow:min-content-height`.
    #[must_use]
    pub fn min_content_height(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::MinContentHeight, v)
    }

    /// `GtkScrolledWindow:max-content-width`.
    #[must_use]
    pub fn max_content_width(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::MaxContentWidth, v)
    }

    /// `GtkScrolledWindow:max-content-height`.
    #[must_use]
    pub fn max_content_height(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::MaxContentHeight, v)
    }

    /// `GtkScrolledWindow:propagate-natural-width`.
    #[must_use]
    pub fn propagate_natural_width(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::PropagateNaturalWidth, v)
    }

    /// `GtkScrolledWindow:propagate-natural-height`.
    #[must_use]
    pub fn propagate_natural_height(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::PropagateNaturalHeight, v)
    }

    /// `GtkScrolledWindow:kinetic-scrolling`.
    #[must_use]
    pub fn kinetic_scrolling(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Kinetic, v)
    }

    /// `GtkScrolledWindow:overlay-scrolling`.
    #[must_use]
    pub fn overlay_scrolling(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::OverlayScrolling, v)
    }
}

/// `GtkNotebook` over `pages`, each a [`notebook_tab`] view.
///
/// Reconciliation: `notebook.rs`'s own module doc explains how each page's
/// label and content reach `header`/`stack` despite `Controller::build`
/// never seeing a view's children -- `NotebookC::child_index`/
/// `reserved_total`/`place`, the same "attach flat, then sort by hand"
/// pattern `HeaderBarC` uses for its own packed children.
#[must_use]
pub fn notebook<Msg: Clone + 'static>(pages: impl IntoIterator<Item = View<Msg>>) -> View<Msg> {
    View::new(Kind::Notebook).children(pages)
}

/// One `notebook`'s page: `label` for the strip, `child` for its content.
#[must_use]
pub fn notebook_tab<Msg: Clone + 'static>(label: &str, child: View<Msg>) -> View<Msg> {
    View::new(Kind::NotebookTab).label(label).child(child)
}

/// `GtkNotebook`'s own property setters, chained after [`notebook`], and
/// `GtkNotebook:reorderable`/`GtkNotebook:detachable`, chained after
/// [`notebook_tab`] since they are per-page GTK properties.
///
/// Scoped to its own trait for the same reason [`HeaderBarExt`]'s doc
/// comment gives: `.page` would otherwise collide with any future widget
/// that wants the same name over a different `PropName`.
pub trait NotebookExt<Msg>: Sized {
    /// `GtkNotebook:page`.
    fn page(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:tab-pos`.
    fn tab_pos(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:scrollable`.
    fn scrollable(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:show-tabs`.
    fn show_tabs(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:show-border`.
    fn show_border(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:reorderable` (a per-page property in real GTK; this
    /// controller applies it strip-wide -- see contract deviation notes).
    fn reorderable(self, v: impl Into<Prop>) -> Self;
    /// `GtkNotebook:detachable` (per-page).
    fn detachable(self, v: impl Into<Prop>) -> Self;
}

impl<Msg: Clone + 'static> NotebookExt<Msg> for View<Msg> {
    fn page(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Page, v)
    }
    fn tab_pos(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::TabPos, v)
    }
    fn scrollable(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Scrollable, v)
    }
    fn show_tabs(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShowTabs, v)
    }
    fn show_border(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShowBorder, v)
    }
    fn reorderable(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Reorderable, v)
    }
    fn detachable(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Detachable, v)
    }
}

/// `GtkStack` over `pages`, each a [`stack_page`] view.
#[must_use]
pub fn stack<Msg: Clone + 'static>(pages: impl IntoIterator<Item = View<Msg>>) -> View<Msg> {
    View::new(Kind::Stack).children(pages)
}

/// One `stack`'s page: `name` is the value `visible_child` selects by,
/// `title` is what a `StackSwitcher`/`StackSidebar` shows.
#[must_use]
pub fn stack_page<Msg: Clone + 'static>(name: &str, title: &str, child: View<Msg>) -> View<Msg> {
    View::new(Kind::StackPage)
        .prop(PropName::PageName, Prop::Str(Rc::from(name)))
        .prop(PropName::PageTitle, Prop::Str(Rc::from(title)))
        .child(child)
}

/// [`stack_page`]'s own two properties, chained after it. Scoped to its own
/// trait for the same reason [`HeaderBarExt`]'s doc comment gives: `.icon`
/// would otherwise collide with any future widget that wants the same name
/// over a different `PropName`.
pub trait StackPageExt<Msg>: Sized {
    /// `GtkStackPage:icon-name`.
    fn icon(self, v: impl Into<Prop>) -> Self;
    /// `GtkStackPage:needs-attention`.
    fn needs_attention(self, v: impl Into<Prop>) -> Self;
}

impl<Msg: Clone + 'static> StackPageExt<Msg> for View<Msg> {
    fn icon(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Icon, v)
    }
    fn needs_attention(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::NeedsAttention, v)
    }
}

impl<Msg: Clone + 'static> View<Msg> {
    /// `GtkBox:spacing`, `GtkGrid` row/column spacing's shorthand.
    #[must_use]
    pub fn spacing(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Spacing, v)
    }

    /// `GtkStack:visible-child-name`.
    #[must_use]
    pub fn visible_child(self, name: &str) -> Self {
        self.prop(PropName::VisibleChild, Prop::Str(Rc::from(name)))
    }

    /// `GtkStack:transition-type`.
    #[must_use]
    pub fn transition_type(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Transition, v)
    }

    /// `GtkStack:transition-duration`, ms.
    #[must_use]
    pub fn transition_duration(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::TransitionDuration, v)
    }

    /// `GtkStack:hhomogeneous`.
    #[must_use]
    pub fn hhomogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Hhomogeneous, v)
    }

    /// `GtkStack:vhomogeneous`.
    #[must_use]
    pub fn vhomogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Vhomogeneous, v)
    }

    /// `GtkStack:interpolate-size`.
    #[must_use]
    pub fn interpolate_size(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::InterpolateSize, v)
    }

    /// `GtkBox:homogeneous`.
    #[must_use]
    pub fn homogeneous(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Homogeneous, v)
    }

    /// `GtkBox:baseline-position`.
    #[must_use]
    pub fn baseline_position(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::BaselinePosition, v)
    }

    /// `GtkCenterBox:shrink-center-last`.
    #[must_use]
    pub fn shrink_center_last(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ShrinkCenterLast, v)
    }
}

impl<Msg: Clone + 'static> View<Msg> {
    /// The widget was clicked.
    #[must_use]
    pub fn on_click(self, msg: Msg) -> Self {
        self.on(EventKind::Click, Handler::Unit(msg))
    }

    /// The widget was activated — Space/Enter, or a click that completed
    /// inside it.
    #[must_use]
    pub fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }

    /// A popover, dialog or window was closed.
    #[must_use]
    pub fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }

    /// The widget took the focus.
    #[must_use]
    pub fn on_focus_in(self, msg: Msg) -> Self {
        self.on(EventKind::FocusIn, Handler::Unit(msg))
    }

    /// The widget lost the focus.
    #[must_use]
    pub fn on_focus_out(self, msg: Msg) -> Self {
        self.on(EventKind::FocusOut, Handler::Unit(msg))
    }

    /// A toggle/check/switch changed state.
    #[must_use]
    pub fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }

    /// An expander opened or closed.
    #[must_use]
    pub fn on_expanded(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Expanded, Handler::Bool(Rc::new(f)))
    }

    /// Editable text changed.
    #[must_use]
    pub fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }

    /// A search entry's debounced text changed.
    #[must_use]
    pub fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Search, Handler::Text(Rc::new(f)))
    }

    /// A link button's URI was activated.
    #[must_use]
    pub fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }

    /// A calendar date was picked.
    ///
    /// Contract deviation D13: the payload is an ISO-8601 `YYYY-MM-DD`
    /// string, because §4.4's `Handler` has no `(i32, u32, u32)` variant.
    #[must_use]
    pub fn on_date_selected(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::DateSelected, Handler::Text(Rc::new(f)))
    }

    /// A row, item or dropdown entry was selected.
    #[must_use]
    pub fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }

    /// A notebook or stack switched page.
    #[must_use]
    pub fn on_page_changed(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::PageChanged, Handler::Index(Rc::new(f)))
    }

    /// A dialog button was chosen; the index is into the dialog's button list.
    #[must_use]
    pub fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }

    /// A reorderable child was dropped, `(from, to)`.
    ///
    /// Contract §10 E10: P4's Task 5 shipped this as `impl Fn(usize) -> Msg`
    /// over `Handler::Index` (deviation D13's other half); P6 deviation 4
    /// adds `Handler::Indices` for exactly this two-argument shape and E10
    /// re-types this setter onto it, in the same commit that adds
    /// `Handler::Indices`/`Handlers::fire_indices` -- the one place P6
    /// touches P4's own setter rather than adding beside it. The lone P4
    /// call site is `every_event_kind_has_exactly_one_on_setter`, updated in
    /// this same commit.
    #[must_use]
    pub fn on_reordered(self, f: impl Fn(usize, usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Reordered, Handler::Indices(Rc::new(f)))
    }

    /// A scale, scrollbar or spin button's value changed.
    #[must_use]
    pub fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }

    /// A scrolled window's offset changed; the payload is the vertical
    /// offset in px.
    #[must_use]
    pub fn on_scrolled(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::Scrolled, Handler::Float(Rc::new(f)))
    }

    /// A key reached this widget. Return `None` to let it keep bubbling to
    /// the window's focus navigation (contract §3.5).
    #[must_use]
    pub fn on_key(self, f: impl Fn(&KeyEvent) -> Option<Msg> + 'static) -> Self {
        self.on(EventKind::KeyPressed, Handler::Key(Rc::new(f)))
    }
}

// The per-widget builders themselves live beside their controllers under
// `crate::widgets` (contract §11 E5); this module re-exports them so every
// builder is reachable as `view::builders::<name>`.
pub use crate::widgets::action_bar::ActionBarExt;
pub use crate::widgets::button::{ButtonExt, button, button_from};
pub use crate::widgets::calendar::{CalendarExt, calendar};
pub use crate::widgets::check_button::{CheckButtonExt, check_button};
pub use crate::widgets::color_dialog::{
    ColorDialogButtonExt, ColorDialogExt, color_dialog, color_dialog_button,
};
pub use crate::widgets::drawing_area::{DrawingAreaExt, drawing_area};
pub use crate::widgets::drop_down::{DropDownExt, drop_down, drop_down_from};
pub use crate::widgets::editable_label::{EditableLabelExt, editable_label};
pub use crate::widgets::entry::{EntryExt, entry};
pub use crate::widgets::font_dialog::{
    FontDialogButtonExt, FontDialogExt, font_dialog, font_dialog_button,
};
pub use crate::widgets::image::{ImageExt, image, image_named};
pub use crate::widgets::info_bar::{InfoBarExt, info_bar};
pub use crate::widgets::label::{LabelExt, label};
pub use crate::widgets::level_bar::{LevelBarExt, level_bar};
pub use crate::widgets::link_button::{LinkButtonExt, link_button};
pub use crate::widgets::menu_button::{MenuButtonExt, menu_button};
pub use crate::widgets::password_entry::{PasswordEntryExt, password_entry};
pub use crate::widgets::picture::{PictureExt, picture, picture_from_bytes};
pub use crate::widgets::popover::{PopoverExt, popover};
pub use crate::widgets::progress_bar::{ProgressBarExt, progress_bar};
pub use crate::widgets::scale::{ScaleExt, scale};
pub use crate::widgets::scrollbar::{ScrollbarExt, scrollbar};
pub use crate::widgets::search_bar::SearchBarExt;
pub use crate::widgets::search_entry::{SearchEntryExt, search_entry};
pub use crate::widgets::separator::separator;
pub use crate::widgets::spin_button::{SpinButtonExt, spin_button};
pub use crate::widgets::spinner::{SpinnerExt, spinner};
pub use crate::widgets::statusbar::{StatusbarExt, statusbar};
pub use crate::widgets::switch::{SwitchExt, switch};
pub use crate::widgets::text_view::{TextViewExt, text_view};
pub use crate::widgets::toggle_button::{ToggleButtonExt, toggle_button};
pub use crate::widgets::window_controls::{WindowControlsExt, window_controls};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{EventKind, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Clicked,
        Text(String),
        Index(usize),
        Value(u64),
        Flag(bool),
    }

    #[test]
    fn widget_builds_a_bare_view_of_the_kind() {
        let v: View<Msg> = widget(Kind::Spinner);
        assert_eq!(v.kind, Kind::Spinner);
        assert!(v.props.is_empty());
    }

    #[test]
    fn every_event_kind_has_exactly_one_on_setter() {
        // The eighteen setters, each binding its own EventKind and nothing
        // else. A new EventKind without a setter fails this test.
        let bound: Vec<EventKind> = vec![
            widget::<Msg>(Kind::Button).on_click(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_activate(Msg::Clicked),
            widget::<Msg>(Kind::Popover).on_close(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_in(Msg::Clicked),
            widget::<Msg>(Kind::Button).on_focus_out(Msg::Clicked),
            widget::<Msg>(Kind::ToggleButton).on_toggle(Msg::Flag),
            widget::<Msg>(Kind::Expander).on_expanded(Msg::Flag),
            widget::<Msg>(Kind::Entry).on_change(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::SearchEntry).on_search(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::LinkButton).on_activate_link(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::Calendar).on_date_selected(|s| Msg::Text(s.to_owned())),
            widget::<Msg>(Kind::DropDown).on_selected(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_page_changed(Msg::Index),
            widget::<Msg>(Kind::AlertDialog).on_response(Msg::Index),
            widget::<Msg>(Kind::Notebook).on_reordered(|_from, to| Msg::Index(to)),
            widget::<Msg>(Kind::Scale).on_value_changed(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::ScrolledWindow).on_scrolled(|v| Msg::Value(v as u64)),
            widget::<Msg>(Kind::Entry).on_key(|_| Some(Msg::Clicked)),
        ]
        .into_iter()
        .map(|v| {
            assert_eq!(v.handlers.len(), 1, "a setter bound more than one event");
            *EventKind::ALL
                .iter()
                .find(|k| v.handlers.has(**k))
                .expect("the setter bound no event")
        })
        .collect();

        let mut sorted = bound.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            18,
            "two setters bound the same EventKind: {bound:?}"
        );
        assert_eq!(sorted, EventKind::ALL.to_vec());
    }

    #[test]
    fn the_typed_setters_carry_their_payload_through() {
        let v: View<Msg> = widget(Kind::Entry).on_change(|s| Msg::Text(s.to_owned()));
        assert_eq!(
            v.handlers.fire_text(EventKind::Change, "abc"),
            Some(Msg::Text("abc".into()))
        );

        let v: View<Msg> = widget(Kind::DropDown).on_selected(Msg::Index);
        assert_eq!(
            v.handlers.fire_index(EventKind::Selected, 4),
            Some(Msg::Index(4))
        );

        let v: View<Msg> = widget(Kind::Scale).on_value_changed(|x| Msg::Value(x as u64));
        assert_eq!(
            v.handlers.fire_float(EventKind::ValueChanged, 12.0),
            Some(Msg::Value(12))
        );
    }

    #[test]
    fn a_key_handler_that_declines_produces_nothing() {
        let v: View<Msg> = widget(Kind::Entry).on_key(|_| None);
        let ev = crate::window::keyboard::Keymap::from_string(include_str!(
            "../../tests/fixtures/keymaps/us.xkb"
        ))
        .expect("the vendored us keymap compiles")
        .translate(38, true, 1, 0); // evdev 38 == `a`
        assert_eq!(v.handlers.fire_key(EventKind::KeyPressed, &ev), None);
    }

    #[test]
    fn builders_chain_with_the_universal_setters() {
        let v: View<Msg> = widget::<Msg>(Kind::Button)
            .class("suggested-action")
            .margin(0, 6, 0, 6)
            .on_click(Msg::Clicked)
            .key("ok");
        assert_eq!(v.kind, Kind::Button);
        assert_eq!(
            v.props.get(PropName::Margin),
            Some(&crate::view::Prop::Edges([0, 6, 0, 6]))
        );
        assert_eq!(v.handlers.fire_unit(EventKind::Click), Some(Msg::Clicked));
        assert_eq!(v.key, Some(crate::view::Key::Name(std::rc::Rc::from("ok"))));
    }
}
