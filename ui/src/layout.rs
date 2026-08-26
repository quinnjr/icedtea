//! GTK's measure→allocate box model, expressed as a `taffy` tree.
//!
//! The button is a flex container carrying the computed padding, border and
//! min-size; the label is a leaf child sized to its shaped extents. Taffy
//! then produces the border-box allocation and the label's position inside
//! it, including the min-size clamp and the centring -- none of which this
//! crate reimplements.

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

/// Lay out a button whose label measures `label`.
///
/// # Panics
///
/// Panics only if `taffy` itself fails, which for a two-node tree built
/// entirely from finite lengths means a bug in this function, not bad input.
#[must_use]
pub fn layout_button(style: &ComputedStyle, label: &TextMetrics) -> Allocation {
    let [pad_top, pad_right, pad_bottom, pad_left] = style.padding;
    let border = style.border_width;

    let mut tree: TaffyTree<()> = TaffyTree::new();
    let label_node = tree
        .new_leaf(Style {
            size: Size {
                width: length(label.width),
                height: length(label.line_height),
            },
            ..Style::default()
        })
        .expect("taffy leaf");
    let button_node = tree
        .new_with_children(
            Style {
                display: Display::Flex,
                align_items: Some(AlignItems::CENTER),
                justify_content: Some(JustifyContent::CENTER),
                min_size: Size {
                    width: length(style.min_width),
                    height: length(style.min_height),
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
            &[label_node],
        )
        .expect("taffy container");

    tree.compute_layout(
        button_node,
        Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        },
    )
    .expect("taffy layout");

    let button = *tree.layout(button_node).expect("button layout");
    let label_layout = *tree.layout(label_node).expect("label layout");

    Allocation {
        width: button.size.width,
        height: button.size.height,
        label_x: label_layout.location.x,
        label_y: label_layout.location.y,
    }
}

#[cfg(test)]
mod tests {
    use super::layout_button;
    use crate::css::computed::{Background, ComputedStyle};
    use crate::text::TextMetrics;
    use skia_rs_safe::core::Color;

    fn adwaita_like() -> ComputedStyle {
        ComputedStyle {
            background: Background::Solid(Color(0xFFDA_D6D2)),
            color: Color(0xFF2E_3436),
            border_width: 1.0,
            border_color: Color(0xFFCD_C7C2),
            border_radius: 5.0,
            padding: [4.0, 9.0, 4.0, 9.0],
            min_width: 16.0,
            min_height: 24.0,
            font_size: 14.0,
            ..ComputedStyle::default()
        }
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
        // 60 + 9 + 9 + 1 + 1
        assert_eq!(allocation.width, 80.0);
        // 18 + 4 + 4 + 1 + 1
        assert_eq!(allocation.height, 28.0);
        assert_eq!(allocation.label_x, 10.0, "border 1 + padding-left 9");
        assert_eq!(allocation.label_y, 5.0, "border 1 + padding-top 4");
    }

    #[test]
    fn min_size_floors_a_tiny_label() {
        // "" is 0 wide and 6 tall: min-width 16 / min-height 24 must win.
        let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
        assert_eq!(
            allocation.width, 20.0,
            "max(0 + 18 + 2, min-width 16) == 20"
        );
        assert_eq!(
            allocation.height, 24.0,
            "max(6 + 8 + 2, min-height 24) == 24"
        );
    }

    #[test]
    fn a_borderless_paddingless_button_is_exactly_the_label() {
        let style = ComputedStyle {
            padding: [0.0; 4],
            border_width: 0.0,
            min_width: 0.0,
            min_height: 0.0,
            ..adwaita_like()
        };
        let allocation = layout_button(&style, &label(42.0, 17.0));
        assert_eq!(allocation.width, 42.0);
        assert_eq!(allocation.height, 17.0);
        assert_eq!(allocation.label_x, 0.0);
        assert_eq!(allocation.label_y, 0.0);
    }

    #[test]
    fn the_label_is_centred_when_min_size_grows_the_button() {
        let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
        // Content box is 24 - 2 - 8 = 14 tall; a 6-tall label centres at 4
        // inside it, i.e. 1 (border) + 4 (padding) + 4 == 9 from the top.
        assert_eq!(allocation.label_y, 9.0);
    }
}
