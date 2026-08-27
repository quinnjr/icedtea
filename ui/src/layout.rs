//! GTK's measure→allocate box model over a `css::node::Node` tree,
//! expressed as a `taffy` tree.
//!
//! # `min-width`/`min-height` are content-box minimums
//!
//! GTK's `min-width`/`min-height` floor the widget's *content*, exactly as
//! `box-sizing: content-box` says they should; CSS's own `min-width` on a
//! `border-box` element does not. taffy resolves `min_size` in the box the
//! node's `box_sizing` names and adds padding+border when that box is the
//! content box (`taffy-0.14.0/src/compute/flexbox.rs:244`), so this module
//! sets `BoxSizing::ContentBox` and passes GTK's minimums straight through.
//!
//! Passing them through as border-box minimums makes every widget that is
//! floored by its minimum too small by exactly its padding plus border:
//! Adwaita's "Click me" came out 27 px high against GTK 4.22's 34, and an
//! empty button 20x27 against 36x34.

use crate::css::value::Keyword;

/// An axis-aligned box in absolute, tree-origin coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width; never negative for a rect this module produces.
    pub width: f32,
    /// Height; never negative for a rect this module produces.
    pub height: f32,
}

impl Rect {
    /// A rect from its origin and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The empty rect at the origin.
    #[must_use]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }

    /// Right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Whether either dimension is zero or smaller (or not a number).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    /// Shrink by `sides` (top, right, bottom, left), clamped at zero size.
    #[must_use]
    pub fn inset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    /// Grow by `sides` (top, right, bottom, left).
    #[must_use]
    pub fn outset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x - left,
            y: self.y - top,
            width: (self.width + left + right).max(0.0),
            height: (self.height + top + bottom).max(0.0),
        }
    }

    /// The same box as a `skia-rs` edge-form rect.
    #[must_use]
    pub fn to_skia(&self) -> skia_rs_safe::core::Rect {
        skia_rs_safe::core::Rect::from_xywh(self.x, self.y, self.width, self.height)
    }
}

/// One node's laid-out geometry, in absolute tree-origin coordinates.
///
/// Replaces M1's `{width, height, label_x, label_y}`: a generic node has no
/// "label", and per-side border and padding are first-class in M2.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Allocation {
    /// The border box: what `background-clip: border-box` fills.
    pub border_box: Rect,
    /// The content box: where children and text start.
    pub content_box: Rect,
    /// Used border widths, top/right/bottom/left.
    pub border: [f32; 4],
    /// Used padding, top/right/bottom/left.
    pub padding: [f32; 4],
}

impl Allocation {
    /// The padding box: the border box minus the used border widths.
    #[must_use]
    pub fn padding_box(&self) -> Rect {
        self.border_box.inset(self.border)
    }

    /// The box a `background-origin`/`background-clip` keyword names.
    ///
    /// Any keyword without a box in M2 (`text`, and anything a future
    /// registry row adds) resolves to the border box, which is CSS's
    /// initial `background-clip` — never an empty box.
    #[must_use]
    pub fn box_for(&self, k: Keyword) -> Rect {
        match k {
            Keyword::PaddingBox => self.padding_box(),
            Keyword::ContentBox => self.content_box,
            _ => self.border_box,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Allocation, Rect};
    use crate::css::value::Keyword;

    /// A 100x50 border box with a 2px border and 4/6/4/6 padding.
    fn alloc() -> Allocation {
        Allocation {
            border_box: Rect::new(10.0, 20.0, 100.0, 50.0),
            content_box: Rect::new(18.0, 26.0, 84.0, 38.0),
            border: [2.0, 2.0, 2.0, 2.0],
            padding: [4.0, 6.0, 4.0, 6.0],
        }
    }

    #[test]
    fn inset_subtracts_trbl_and_never_goes_negative() {
        // Mutation check: swapping any two of the TRBL indices, or dropping
        // the `.max(0.0)` clamp, changes one of these three numbers.
        let r = Rect::new(0.0, 0.0, 20.0, 10.0).inset([1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r, Rect::new(4.0, 1.0, 14.0, 6.0));
        let collapsed = Rect::new(0.0, 0.0, 4.0, 4.0).inset([9.0, 9.0, 9.0, 9.0]);
        assert_eq!(collapsed.width, 0.0);
        assert_eq!(collapsed.height, 0.0);
    }

    #[test]
    fn box_for_walks_border_padding_content_inwards() {
        // Mutation check: returning `border_box` for `PaddingBox` (the easy
        // mistake, since GTK's background-origin default is padding-box)
        // moves the second assertion by the 2px border.
        let a = alloc();
        assert_eq!(
            a.box_for(Keyword::BorderBox),
            Rect::new(10.0, 20.0, 100.0, 50.0)
        );
        assert_eq!(
            a.box_for(Keyword::PaddingBox),
            Rect::new(12.0, 22.0, 96.0, 46.0)
        );
        assert_eq!(
            a.box_for(Keyword::ContentBox),
            Rect::new(18.0, 26.0, 84.0, 38.0)
        );
        assert_eq!(a.padding_box(), a.box_for(Keyword::PaddingBox));
    }

    #[test]
    fn box_for_falls_back_to_the_border_box_for_any_other_keyword() {
        // `background-clip: text` is parsed by the registry but has no box
        // in M2; it must not panic and must not silently clip to nothing.
        assert_eq!(alloc().box_for(Keyword::TextBox), alloc().border_box);
    }

    #[test]
    fn rect_geometry_never_panics_on_hostile_numbers() {
        for &v in &[
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0e30,
            1.0e30,
            0.0,
            -0.0,
        ] {
            let r = Rect::new(v, v, v, v);
            let _ = r.right();
            let _ = r.bottom();
            let _ = r.is_empty();
            let _ = r.inset([v, v, v, v]);
            let _ = r.outset([v, v, v, v]);
            let _ = r.to_skia();
        }
    }
}
