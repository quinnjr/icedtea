//! Scalar vocabulary shared by the widget catalogue.
//!
//! Every enum declared here is `#[repr(u16)]` with a `from_u16` that maps an
//! unknown discriminant to the property's *initial* value rather than
//! panicking: these round-trip through [`crate::view::Prop::Enum`], whose
//! payload arrives from an application model.
//!
//! `Orientation`, `Position`, `Side`, `IconSize` and `MessageType` are P5's,
//! declared in [`crate::widgets`] through its `widget_enum!` macro (they carry
//! a CSS class per variant, which these do not); they are re-exported here so
//! one `use` reaches the whole vocabulary. [`ListItem`] is P4's
//! (`view::ListItem`, contract §11 E5) and is likewise only re-exported.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::rc::Rc;

use crate::css::value::image::IconRef;

#[doc(inline)]
pub use super::{IconSize, MessageType, Orientation, Position, Side};
#[doc(inline)]
pub use crate::view::ListItem;

/// When a scrollbar is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Policy {
    /// Always visible.
    Always = 0,
    /// Visible only when the content overflows.
    #[default]
    Automatic = 1,
    /// Never visible; scrolling still works.
    Never = 2,
    /// The application draws it.
    External = 3,
}

/// Where a box aligns baselines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum BaselinePosition {
    /// Baseline at the top of the extra space.
    Top = 0,
    /// Centred.
    #[default]
    Center = 1,
    /// Baseline at the bottom.
    Bottom = 2,
}

/// How many rows a list lets the user select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum SelectionMode {
    /// Nothing is selectable.
    None = 0,
    /// Zero or one row.
    #[default]
    Single = 1,
    /// Exactly one row, always.
    Browse = 2,
    /// Any number of rows.
    Multiple = 3,
}

/// A stack's page-change animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum StackTransition {
    /// Instant.
    #[default]
    None = 0,
    /// Cross-fade.
    Crossfade = 1,
    /// New page slides in from the right.
    SlideLeft = 2,
    /// New page slides in from the left.
    SlideRight = 3,
    /// New page slides in from below.
    SlideUp = 4,
    /// New page slides in from above.
    SlideDown = 5,
    /// Direction chosen from the page order, horizontally.
    SlideLeftRight = 6,
    /// Direction chosen from the page order, vertically.
    SlideUpDown = 7,
    /// New page covers the old, upwards.
    OverUp = 8,
    /// New page covers the old, downwards.
    OverDown = 9,
    /// New page covers the old, leftwards.
    OverLeft = 10,
    /// New page covers the old, rightwards.
    OverRight = 11,
    /// Old page uncovers the new, upwards.
    UnderUp = 12,
    /// Old page uncovers the new, downwards.
    UnderDown = 13,
    /// Old page uncovers the new, leftwards.
    UnderLeft = 14,
    /// Old page uncovers the new, rightwards.
    UnderRight = 15,
    /// Rotate left.
    RotateLeft = 16,
    /// Rotate right.
    RotateRight = 17,
}

/// What a `StackSwitcher`/`StackSidebar` needs to know about a page.
#[derive(Debug, Clone, PartialEq)]
pub struct StackPageInfo {
    /// The page's `name`, the value `visible_child` selects by.
    pub name: Rc<str>,
    /// Human-readable title, shown on the switcher button.
    pub title: Rc<str>,
    /// Optional icon.
    pub icon: Option<IconRef>,
    /// Adds `.needs-attention` to the switcher button.
    pub needs_attention: bool,
}

/// A column's sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum SortOrder {
    /// Smallest first.
    #[default]
    Ascending = 0,
    /// Largest first.
    Descending = 1,
}

/// A total order over list items, for a sortable column.
#[derive(Clone)]
#[allow(
    clippy::type_complexity,
    reason = "the contract's own Sorter signature; a type alias would only hide it"
)]
pub struct Sorter(Rc<dyn Fn(&ListItem, &ListItem) -> Ordering>);

impl Sorter {
    /// Wrap a comparison.
    #[must_use]
    pub fn new(f: impl Fn(&ListItem, &ListItem) -> Ordering + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// Compare by text, the default a column with no sorter gets.
    #[must_use]
    pub fn by_label() -> Self {
        Self::new(|a, b| a.text.cmp(&b.text))
    }

    /// Apply.
    #[must_use]
    pub fn compare(&self, a: &ListItem, b: &ListItem) -> Ordering {
        (self.0)(a, b)
    }

    /// Same closure?
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Sorter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sorter(..)")
    }
}

/// What a recycled row displays. Deliberately `Msg`-free: rebinding a pooled
/// row must not allocate a view subtree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowContent {
    /// The row's text.
    pub label: Rc<str>,
    /// An optional leading icon.
    pub icon: Option<IconRef>,
    /// Extra classes for the row node.
    pub classes: Rc<[Rc<str>]>,
}

impl RowContent {
    /// A plain text row.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        Self {
            label: Rc::from(label),
            icon: None,
            classes: Rc::from(&[][..]),
        }
    }
}

/// Maps a model index and item to what its row shows.
#[derive(Clone)]
#[allow(
    clippy::type_complexity,
    reason = "the contract's own ItemFactory signature; a type alias would only hide it"
)]
pub struct ItemFactory(Rc<dyn Fn(usize, &ListItem) -> RowContent>);

impl ItemFactory {
    /// Wrap a binder.
    #[must_use]
    pub fn new(f: impl Fn(usize, &ListItem) -> RowContent + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// The default factory: the item's own text and icon, and no extra class.
    #[must_use]
    pub fn label_only() -> Self {
        Self::new(|_, item| RowContent {
            label: Rc::clone(&item.text),
            icon: item.icon.clone(),
            classes: Rc::from(&[][..]),
        })
    }

    /// Bind one row.
    #[must_use]
    pub fn bind(&self, index: usize, item: &ListItem) -> RowContent {
        (self.0)(index, item)
    }

    /// Same closure?
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ItemFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ItemFactory(..)")
    }
}

bitflags::bitflags! {
    /// `GtkPopoverMenuFlags`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct MenuFlags: u8 {
        /// Submenus open as nested popovers instead of sliding in place.
        const NESTED = 1;
    }
}

/// A menu section's `display-hint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum DisplayHint {
    /// A plain vertical section.
    #[default]
    Normal = 0,
    /// `.inline-buttons`: the section's items render as a button row.
    InlineButtons = 1,
    /// `.circular`: round icon buttons.
    Circular = 2,
    /// `.horizontal-buttons`.
    HorizontalButtons = 3,
}

/// The licences `AboutDialog` can name without being handed the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum LicenseType {
    /// No licence stated.
    #[default]
    Unknown = 0,
    /// The application supplied its own text.
    Custom = 1,
    /// GNU GPL 2.0 or later.
    Gpl20 = 2,
    /// GNU GPL 3.0 or later.
    Gpl30 = 3,
    /// GNU LGPL 2.1 or later.
    Lgpl21 = 4,
    /// GNU LGPL 3.0 or later.
    Lgpl30 = 5,
    /// GNU AGPL 3.0 or later.
    Agpl30 = 6,
    /// BSD 2-clause.
    Bsd = 7,
    /// MIT/X11.
    MitX11 = 8,
    /// Apache 2.0.
    Apache20 = 9,
    /// Mozilla Public License 2.0.
    Mpl20 = 10,
}

/// Who draws a window's decorations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u16)]
pub enum Decoration {
    /// Client-side, with a shadow: `.csd`.
    Csd = 0,
    /// Client-side, no shadow: `.solid-csd`.
    SolidCsd = 1,
    /// Server-side, which is what icedtea's compositor does: `.ssd`.
    #[default]
    Ssd = 2,
}

/// One `from_u16` per `#[repr(u16)]` enum above: an unknown discriminant is
/// the property's initial value, never a panic.
macro_rules! from_u16 {
    ($ty:ident { $($variant:ident),+ $(,)? }) => {
        impl $ty {
            /// Decode a `Prop::Enum` payload; unknown values fall back to the
            /// initial value and are logged once per process.
            #[must_use]
            pub fn from_u16(raw: u16) -> Self {
                $(if raw == Self::$variant as u16 { return Self::$variant; })+
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    tracing::warn!(
                        raw,
                        ty = stringify!($ty),
                        "unknown enum discriminant; using the initial value"
                    );
                });
                Self::default()
            }

            /// Encode for `Prop::Enum`.
            #[must_use]
            pub fn to_u16(self) -> u16 {
                self as u16
            }
        }
    };
}

from_u16!(Policy {
    Always,
    Automatic,
    Never,
    External
});
from_u16!(BaselinePosition {
    Top,
    Center,
    Bottom
});
from_u16!(SelectionMode {
    None,
    Single,
    Browse,
    Multiple
});
from_u16!(SortOrder {
    Ascending,
    Descending
});
from_u16!(DisplayHint {
    Normal,
    InlineButtons,
    Circular,
    HorizontalButtons
});
from_u16!(Decoration { Csd, SolidCsd, Ssd });
from_u16!(StackTransition {
    None,
    Crossfade,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    SlideLeftRight,
    SlideUpDown,
    OverUp,
    OverDown,
    OverLeft,
    OverRight,
    UnderUp,
    UnderDown,
    UnderLeft,
    UnderRight,
    RotateLeft,
    RotateRight,
});
from_u16!(LicenseType {
    Unknown,
    Custom,
    Gpl20,
    Gpl30,
    Lgpl21,
    Lgpl30,
    Agpl30,
    Bsd,
    MitX11,
    Apache20,
    Mpl20,
});

/// A set of selected model indices, with GTK's four selection modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    mode: SelectionMode,
    set: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl Selection {
    /// An empty selection in `mode`.
    #[must_use]
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            set: BTreeSet::new(),
            anchor: None,
        }
    }

    /// Switch modes, collapsing the set when the new mode allows fewer rows.
    pub fn set_mode(&mut self, mode: SelectionMode) {
        self.mode = mode;
        match mode {
            SelectionMode::None => self.set.clear(),
            SelectionMode::Single | SelectionMode::Browse => {
                let keep = self.set.iter().next().copied();
                self.set.clear();
                if let Some(k) = keep {
                    self.set.insert(k);
                }
            }
            SelectionMode::Multiple => {}
        }
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Is `index` selected?
    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        self.set.contains(&index)
    }

    /// Selected indices, ascending.
    #[must_use]
    pub fn selected(&self) -> Vec<usize> {
        self.set.iter().copied().collect()
    }

    /// The lowest selected index.
    #[must_use]
    pub fn first(&self) -> Option<usize> {
        self.set.iter().next().copied()
    }

    /// How many rows are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Nothing selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Select `index`, replacing the set under `Single`/`Browse`. Returns
    /// whether anything changed.
    pub fn select(&mut self, index: usize) -> bool {
        if self.mode == SelectionMode::None {
            return false;
        }
        self.anchor = Some(index);
        match self.mode {
            SelectionMode::Multiple => self.set.insert(index),
            _ => {
                if self.set.len() == 1 && self.set.contains(&index) {
                    return false;
                }
                self.set.clear();
                self.set.insert(index);
                true
            }
        }
    }

    /// Flip `index`. Under `Single` this deselects; under `Browse` it cannot.
    pub fn toggle(&mut self, index: usize) -> bool {
        match self.mode {
            SelectionMode::None => false,
            SelectionMode::Browse => self.select(index),
            SelectionMode::Single => {
                self.anchor = Some(index);
                if self.set.remove(&index) {
                    true
                } else {
                    self.set.clear();
                    self.set.insert(index)
                }
            }
            SelectionMode::Multiple => {
                self.anchor = Some(index);
                if self.set.remove(&index) {
                    true
                } else {
                    self.set.insert(index)
                }
            }
        }
    }

    /// Extend the selection from the anchor to `index` (Shift-click).
    pub fn extend_to(&mut self, index: usize) -> bool {
        if self.mode != SelectionMode::Multiple {
            return self.select(index);
        }
        let Some(anchor) = self.anchor else {
            return self.select(index);
        };
        let (lo, hi) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        let before = self.set.len();
        // `hi` is a model index, so the range is bounded by the model, but
        // `usize::MAX` from a hostile prop must not be walked one by one.
        let hi = hi.min(lo.saturating_add(1_000_000));
        for i in lo..=hi {
            self.set.insert(i);
        }
        self.set.len() != before
    }

    /// Ctrl-A. `count` is the model length.
    pub fn select_all(&mut self, count: usize) -> bool {
        if self.mode != SelectionMode::Multiple {
            return false;
        }
        let before = self.set.len();
        for i in 0..count.min(1_000_000) {
            self.set.insert(i);
        }
        self.set.len() != before
    }

    /// Deselect everything. `Browse` refuses while it holds a row.
    pub fn clear(&mut self) -> bool {
        if self.mode == SelectionMode::Browse && !self.set.is_empty() {
            return false;
        }
        let changed = !self.set.is_empty();
        self.set.clear();
        changed
    }

    /// Drop every index at or above `count`, after a model swap.
    pub fn retain_below(&mut self, count: usize) {
        self.set.retain(|i| *i < count);
        if self.anchor.is_some_and(|a| a >= count) {
            self.anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoration, LicenseType, Policy, Selection, SelectionMode};

    #[test]
    fn browse_mode_always_keeps_exactly_one_row_selected() {
        // Mutation check: letting `clear` empty a Browse selection (the
        // Single/Browse copy-paste bug) makes the last assertion 0.
        let mut sel = Selection::new(SelectionMode::Browse);
        assert!(sel.select(3));
        assert_eq!(sel.selected(), vec![3]);
        assert!(!sel.clear(), "Browse refuses to empty");
        assert_eq!(sel.len(), 1);
        assert!(sel.select(5));
        assert_eq!(sel.selected(), vec![5], "Browse replaces, never adds");
    }

    #[test]
    fn multiple_mode_toggles_extends_and_selects_all() {
        // Mutation check: making `extend_to` walk from 0 instead of the
        // anchor selects 0..=6 and this fails on the first assertion.
        let mut sel = Selection::new(SelectionMode::Multiple);
        sel.select(2);
        sel.extend_to(6);
        assert_eq!(sel.selected(), vec![2, 3, 4, 5, 6]);
        assert!(sel.toggle(4));
        assert_eq!(sel.selected(), vec![2, 3, 5, 6]);
        assert!(sel.select_all(8));
        assert_eq!(sel.len(), 8);
        assert!(sel.clear());
        assert!(sel.is_empty());
    }

    #[test]
    fn selection_mode_none_never_selects_anything() {
        // Mutation check: forgetting the SelectionMode::None guard in
        // `select` makes `len()` 1.
        let mut sel = Selection::new(SelectionMode::None);
        assert!(!sel.select(1));
        assert!(!sel.toggle(1));
        assert!(!sel.select_all(4));
        assert_eq!(sel.len(), 0);
    }

    #[test]
    fn retain_below_drops_rows_a_shrunken_model_no_longer_has() {
        // Mutation check: skipping retain_below after a model swap leaves a
        // selected index past the end and the list paints a phantom row.
        let mut sel = Selection::new(SelectionMode::Multiple);
        sel.select(1);
        sel.select(9);
        sel.retain_below(5);
        assert_eq!(sel.selected(), vec![1]);
    }

    #[test]
    fn a_selection_never_panics_on_hostile_indices() {
        // Indices come from an application model and from hit-testing.
        for mode in [
            SelectionMode::None,
            SelectionMode::Single,
            SelectionMode::Browse,
            SelectionMode::Multiple,
        ] {
            let mut sel = Selection::new(mode);
            sel.select(usize::MAX);
            sel.extend_to(0);
            sel.toggle(usize::MAX);
            sel.select_all(usize::MAX);
            sel.retain_below(0);
            let _ = sel.selected();
            let _ = sel.first();
        }
    }

    #[test]
    fn the_scalar_enums_round_trip_their_wire_encoding() {
        // Mutation check: `Prop::Enum` stores a u16 discriminant; a
        // mismatched from_u16 silently reads a neighbouring variant, which
        // is how a Policy::Never scrollbar becomes Policy::Always.
        for p in [
            Policy::Always,
            Policy::Automatic,
            Policy::Never,
            Policy::External,
        ] {
            assert_eq!(Policy::from_u16(p as u16), p);
        }
        for d in [Decoration::Csd, Decoration::SolidCsd, Decoration::Ssd] {
            assert_eq!(Decoration::from_u16(d as u16), d);
        }
        assert_eq!(
            Policy::from_u16(9999),
            Policy::Automatic,
            "unknown => initial"
        );
        assert_eq!(LicenseType::from_u16(9999), LicenseType::Unknown);
    }
}
