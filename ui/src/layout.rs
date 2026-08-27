//! GTK's measure→allocate box model, expressed as a `taffy` tree.
//!
//! The button is a flex container carrying the computed padding, border and
//! min-size; the label is a leaf child sized to its shaped extents. Taffy
//! then produces the border-box allocation and the label's position inside
//! it, including the min-size clamp and the centring -- none of which this
//! crate reimplements.
//!
//! # `min-width`/`min-height` are content-box minimums
//!
//! GTK's `min-width`/`min-height` floor the widget's *content*, exactly as
//! `box-sizing: content-box` says they should; CSS's own `min-width` on a
//! `border-box` element does not. Taffy takes `min_size` in the box the
//! node's `box_sizing` names, so the minimums are converted here:
//!
//! ```text
//! content    = max(intrinsic, min)
//! border box = content + padding + border
//! ```
//!
//! Passing them through as border-box minimums (which is what this module
//! used to do) makes every button that is floored by its minimum too small
//! by exactly its padding plus border: Adwaita's "Click me" came out 27 px
//! high against GTK 4.22's 34, and an empty button 20x27 against 36x34.

use taffy::NodeId;
use taffy::prelude::{
    AlignItems, AvailableSpace, Display, JustifyContent, Size, Style, TaffyTree, length,
};

use crate::css::computed::ComputedStyle;
use crate::text::TextMetrics;

/// A laid-out button: its border-box size and its label's position within it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Allocation {
    /// Border-box width in px.
    pub width: f32,
    /// Border-box height in px.
    pub height: f32,
    /// Label content box's left edge, relative to the button's left edge.
    pub label_x: f32,
    /// Label content box's top edge, relative to the button's top edge.
    pub label_y: f32,
}

/// A reusable two-node `taffy` tree for one button.
///
/// The tree is built once and its styles overwritten per layout: a restyle
/// happens on every `:hover`/`:active` transition, and rebuilding a tree
/// (two allocations, two node inserts, a fresh arena) for each of those was
/// pure churn -- taffy is built to be kept and updated.
pub struct ButtonLayout {
    tree: TaffyTree<()>,
    button: NodeId,
    label: NodeId,
}

impl Default for ButtonLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl ButtonLayout {
    /// Build the tree. Cheap enough to do once per widget.
    ///
    /// # Panics
    ///
    /// Only if `taffy` fails to insert a node, which for a two-node tree
    /// means a bug in `taffy`, not bad input.
    #[must_use]
    pub fn new() -> Self {
        let mut tree: TaffyTree<()> = TaffyTree::new();
        let label = tree.new_leaf(Style::default()).expect("taffy leaf");
        let button = tree
            .new_with_children(Style::default(), &[label])
            .expect("taffy container");
        Self {
            tree,
            button,
            label,
        }
    }

    /// Lay out a button whose label measures `label`.
    ///
    /// # Panics
    ///
    /// Panics only if `taffy` itself fails, which for a two-node tree built
    /// entirely from finite lengths means a bug in this function, not bad
    /// input.
    pub fn compute(&mut self, style: &ComputedStyle, label: &TextMetrics) -> Allocation {
        let [pad_top, pad_right, pad_bottom, pad_left] = style.padding(0.0);
        // M1 paints one uniform border; the top side is the one taffy is given.
        // P4 replaces this whole module with a per-side `LayoutTree`.
        let border = style.border_widths()[0];
        let (min_width, min_height) = style.min_size((0.0, 0.0));

        self.tree
            .set_style(
                self.label,
                Style {
                    size: Size {
                        width: length(label.width),
                        height: length(label.line_height),
                    },
                    ..Style::default()
                },
            )
            .expect("taffy label style");
        self.tree
            .set_style(
                self.button,
                Style {
                    display: Display::Flex,
                    align_items: Some(AlignItems::CENTER),
                    justify_content: Some(JustifyContent::CENTER),
                    // Content-box minimums, converted to the border-box
                    // minimums taffy wants: see this module's header.
                    min_size: Size {
                        width: length(min_width + pad_left + pad_right + border * 2.0),
                        height: length(min_height + pad_top + pad_bottom + border * 2.0),
                    },
                    padding: taffy::geometry::Rect {
                        left: length(pad_left),
                        right: length(pad_right),
                        top: length(pad_top),
                        bottom: length(pad_bottom),
                    },
                    border: taffy::geometry::Rect {
                        left: length(border),
                        right: length(border),
                        top: length(border),
                        bottom: length(border),
                    },
                    ..Style::default()
                },
            )
            .expect("taffy button style");

        self.tree
            .compute_layout(
                self.button,
                Size {
                    width: AvailableSpace::MaxContent,
                    height: AvailableSpace::MaxContent,
                },
            )
            .expect("taffy layout");

        let button = *self.tree.layout(self.button).expect("button layout");
        let label_layout = *self.tree.layout(self.label).expect("label layout");

        Allocation {
            width: button.size.width,
            height: button.size.height,
            label_x: label_layout.location.x,
            label_y: label_layout.location.y,
        }
    }
}

/// Lay out one button in a throwaway tree.
///
/// Convenient for one-off callers and tests; a widget that restyles keeps a
/// [`ButtonLayout`] instead.
#[must_use]
pub fn layout_button(style: &ComputedStyle, label: &TextMetrics) -> Allocation {
    ButtonLayout::new().compute(style, label)
}

#[cfg(test)]
mod tests {
    use super::layout_button;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::text::TextMetrics;

    /// The Adwaita button box, expressed as the CSS it actually comes from.
    fn style_from(css: &str) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        ComputedStyle::resolve_chain(&sheet, &button, &ResolveEnv::default(), &mut MatchCx::new())
    }

    fn adwaita_like() -> ComputedStyle {
        style_from(
            "button { background-color: #dad6d2; color: #2e3436; \
             border: 1px solid #cdc7c2; border-radius: 5px; padding: 4px 9px; \
             min-width: 16px; min-height: 24px; font-size: 14px }",
        )
    }

    fn bare() -> ComputedStyle {
        style_from(
            "button { background-color: #dad6d2; color: #2e3436; border: 0 solid #cdc7c2; \
             border-radius: 5px; padding: 0; min-width: 0; min-height: 0; font-size: 14px }",
        )
    }

    fn label(width: f32, height: f32) -> TextMetrics {
        TextMetrics {
            width,
            ascent: height * 0.8,
            descent: height * 0.2,
            line_height: height,
        }
    }

    #[test]
    fn allocation_is_text_plus_padding_plus_border() {
        let allocation = layout_button(&adwaita_like(), &label(60.0, 18.0));
        // content max(60, min-width 16) = 60, + 9 + 9 + 1 + 1
        assert_eq!(allocation.width, 80.0);
        // content max(18, min-height 24) = 24, + 4 + 4 + 1 + 1
        assert_eq!(allocation.height, 34.0);
        assert_eq!(allocation.label_x, 10.0, "border 1 + padding-left 9");
        assert_eq!(
            allocation.label_y, 8.0,
            "border 1 + padding-top 4 + (24 - 18) / 2 centring"
        );
    }

    #[test]
    fn min_size_is_a_content_box_minimum_not_a_border_box_one() {
        // A2, the reviewer's scenario. GTK's `min-width`/`min-height` floor
        // the *content* box, so the padding and border are added on top:
        // an empty Adwaita button is 36x34, not 20x27.
        let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
        assert_eq!(
            allocation.width, 36.0,
            "max(0, min-width 16) + 9 + 9 + 1 + 1 == 36"
        );
        assert_eq!(
            allocation.height, 34.0,
            "max(6, min-height 24) + 4 + 4 + 1 + 1 == 34"
        );
    }

    #[test]
    fn an_intrinsic_size_above_the_minimum_still_wins() {
        // The clamp is `max`, not "always the minimum": a label taller than
        // min-height must still grow the button.
        let allocation = layout_button(&adwaita_like(), &label(200.0, 40.0));
        assert_eq!(allocation.width, 220.0);
        assert_eq!(allocation.height, 50.0);
    }

    #[test]
    fn a_borderless_paddingless_button_is_exactly_the_label() {
        let style = bare();
        let allocation = layout_button(&style, &label(42.0, 17.0));
        assert_eq!(allocation.width, 42.0);
        assert_eq!(allocation.height, 17.0);
        assert_eq!(allocation.label_x, 0.0);
        assert_eq!(allocation.label_y, 0.0);
    }

    #[test]
    fn one_tree_reused_gives_the_same_answer_as_a_fresh_one() {
        // A8: the tree is now kept across restyles, so a second layout must
        // not inherit anything from the first.
        let mut layout = super::ButtonLayout::new();
        let tall = layout.compute(&adwaita_like(), &label(200.0, 40.0));
        let small = layout.compute(&adwaita_like(), &label(0.0, 6.0));
        assert_eq!(small, layout_button(&adwaita_like(), &label(0.0, 6.0)));
        assert_eq!(tall, layout_button(&adwaita_like(), &label(200.0, 40.0)));
        assert_eq!(
            layout.compute(&adwaita_like(), &label(200.0, 40.0)),
            tall,
            "the tree did not go back to the larger layout"
        );
    }

    #[test]
    fn the_label_is_centred_when_min_size_grows_the_button() {
        let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
        // The content box is min-height 24 tall; a 6-tall label centres at 9
        // inside it, i.e. 1 (border) + 4 (padding) + 9 == 14 from the top.
        assert_eq!(allocation.label_y, 14.0);
    }
}
