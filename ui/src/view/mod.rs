//! The reactive framework: `View` description trees, keyed reconciliation
//! into M2's retained [`Node`](crate::css::node::Node)s, per-kind
//! controllers, and the `App` loop that folds their messages.

use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::value::image::IconRef;
use crate::layout::{Align, Rect};

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
    /// A `DrawingArea` paint callback.
    #[allow(
        clippy::type_complexity,
        reason = "the contract's own Prop::Draw signature; a type alias would only hide it"
    )]
    Draw(Rc<dyn Fn(&mut Canvas<'_>, Rect)>),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let f: Rc<dyn Fn(&mut skia_rs_safe::canvas::Canvas<'_>, crate::layout::Rect)> =
            Rc::new(|_, _| {});
        let g: Rc<dyn Fn(&mut skia_rs_safe::canvas::Canvas<'_>, crate::layout::Rect)> =
            Rc::new(|_, _| {});
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
}
