//! Skia paint for one themed button.
//!
//! Backgrounds are painted row-band by row-band under a rounded-rect clip
//! rather than through a gradient shader: `Background::color_at` is then the
//! single definition of what color lands on any given row, so a pixel
//! assertion is derivable straight from the theme's declaration.

use skia_rs_safe::canvas::{ClipOp, Surface};
use skia_rs_safe::core::Rect;
use skia_rs_safe::paint::{Paint, Style};
use skia_rs_safe::path::PathBuilder;

use crate::css::computed::{Background, BackgroundClip, ComputedStyle};
use crate::layout::Allocation;
use crate::text::ShapedText;

/// Paint `label` in a button of `allocation` at `origin`, styled by `style`.
///
/// The label arrives already shaped: this used to reshape it *and* remeasure
/// it on every paint, reparsing the face twice per frame for a string that
/// had not changed.
pub fn paint_button(
    surface: &mut Surface,
    origin: (f32, f32),
    style: &ComputedStyle,
    allocation: &Allocation,
    label: Option<&ShapedText>,
) {
    let (ox, oy) = origin;
    let radius = style.border_radius;
    let border = style.border_width;

    // `background-origin` stays at its CSS default (padding-box): that is
    // the box a gradient is *sized* against. `background-clip` only decides
    // how much of the result survives, so widening the clip to the border
    // box fills under the border without restretching the gradient.
    let origin_box = Rect::from_xywh(
        ox + border,
        oy + border,
        (allocation.width - border * 2.0).max(0.0),
        (allocation.height - border * 2.0).max(0.0),
    );
    let origin_height = origin_box.bottom - origin_box.top;

    let (clip, clip_radius) = match style.background_clip {
        BackgroundClip::BorderBox => (
            Rect::from_xywh(ox, oy, allocation.width, allocation.height),
            radius,
        ),
        BackgroundClip::PaddingBox => (origin_box, (radius - border).max(0.0)),
        BackgroundClip::ContentBox => {
            let [top, right, bottom, left] = style.padding;
            (
                Rect::from_xywh(
                    origin_box.left + left,
                    origin_box.top + top,
                    (origin_box.right - origin_box.left - left - right).max(0.0),
                    (origin_box.bottom - origin_box.top - top - bottom).max(0.0),
                ),
                (radius - border - top.max(left)).max(0.0),
            )
        }
    };

    match style.background {
        Background::Transparent => {}
        Background::Solid(color) => {
            let mut paint = Paint::new();
            paint.set_color32(color);
            paint.set_style(Style::Fill);
            paint.set_anti_alias(true);
            surface
                .canvas()
                .draw_round_rect(&clip, clip_radius, clip_radius, &paint);
        }
        Background::LinearGradientToTop { .. } => {
            let mut canvas = surface.canvas();
            let save = canvas.save();
            // Clip to the rounded background-clip box, then fill 1px bands.
            let mut path = PathBuilder::new();
            path.add_round_rect(&clip, clip_radius, clip_radius);
            canvas.clip_path(&path.build(), ClipOp::Intersect, true);
            let rows = (clip.bottom - clip.top).ceil() as i32;
            for row in 0..rows {
                let y = clip.top + row as f32;
                // Sampled against the origin box, not the clip box: rows
                // outside it clamp to the end stops, as CSS requires.
                let color = style
                    .background
                    .color_at(origin_height, y - origin_box.top + 0.5);
                let mut paint = Paint::new();
                paint.set_color32(color);
                paint.set_style(Style::Fill);
                paint.set_anti_alias(false);
                canvas.draw_rect(
                    &Rect::new(clip.left, y, clip.right, (y + 1.0).min(clip.bottom)),
                    &paint,
                );
            }
            canvas.restore_to_count(save);
        }
    }

    // --- border: stroked on the centre line of the border ---------------
    if border > 0.0 && style.border_color.alpha() > 0 {
        let half = border / 2.0;
        let outline = Rect::from_xywh(
            ox + half,
            oy + half,
            (allocation.width - border).max(0.0),
            (allocation.height - border).max(0.0),
        );
        let outline_radius = (radius - half).max(0.0);
        let mut paint = Paint::new();
        paint.set_color32(style.border_color);
        paint.set_style(Style::Stroke);
        paint.set_stroke_width(border);
        paint.set_anti_alias(true);
        surface
            .canvas()
            .draw_round_rect(&outline, outline_radius, outline_radius, &paint);
    }

    // --- label ----------------------------------------------------------
    if let Some(label) = label
        && let Some(blob) = label.blob.as_ref()
    {
        let mut paint = Paint::new();
        paint.set_color32(style.color);
        paint.set_style(Style::Fill);
        paint.set_anti_alias(true);
        surface.canvas().draw_text_blob(
            blob,
            ox + allocation.label_x,
            oy + allocation.label_y + label.metrics.ascent,
            &paint,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::paint_button;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ComputedStyle;
    use crate::css::select::{CssNode, PseudoStates};
    use crate::layout::Allocation;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    /// Paint one 40x20 square button with `css` applied and read a pixel back.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        let node = CssNode::new("button", &[], PseudoStates::default(), Some(window));
        let style = ComputedStyle::resolve(&sheet, &node);
        let allocation = Allocation {
            width: 40.0,
            height: 20.0,
            label_x: 0.0,
            label_y: 0.0,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        paint_button(&mut surface, (0.0, 0.0), &style, &allocation, None);
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
    }

    const SQUARE: &str = "button { background-color: #112233; border-radius: 0; \
                          border: 4px solid transparent; padding: 4px }";

    #[test]
    fn the_background_fills_the_border_box_by_default() {
        // E9: the background was clipped to the padding box unconditionally,
        // so a translucent border showed the surface through instead of the
        // element's own background. CSS and GTK default to `border-box`.
        let surface = painted(SQUARE);
        assert_eq!(pixel(&surface, 0, 0), Color(0xFF11_2233));
        assert_eq!(pixel(&surface, 20, 10), Color(0xFF11_2233));
    }

    #[test]
    fn padding_box_clips_the_background_inside_the_border() {
        let surface = painted(&format!(
            "{SQUARE}\nbutton {{ background-clip: padding-box }}"
        ));
        assert_eq!(pixel(&surface, 0, 0).alpha(), 0);
        assert_eq!(pixel(&surface, 3, 3).alpha(), 0);
        assert_eq!(pixel(&surface, 5, 5), Color(0xFF11_2233));
    }

    #[test]
    fn content_box_clips_the_background_inside_the_padding_too() {
        // Adwaita:1600 uses `background-clip: content-box`. Border 4px plus
        // padding 4px means the content box starts at x=8.
        let surface = painted(&format!(
            "{SQUARE}\nbutton {{ background-clip: CONTENT-BOX }}"
        ));
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0);
        assert_eq!(pixel(&surface, 9, 9), Color(0xFF11_2233));
    }

    #[test]
    fn a_gradient_is_clipped_the_same_way_but_still_sampled_from_its_origin_box() {
        // `background-origin` stays at its CSS default (padding-box), so
        // widening the clip must not restretch the gradient: the row inside
        // the border reads the same colour either way.
        let gradient = "button { background-image: linear-gradient(to top, #ff0000, #00ff00); \
                        border-radius: 0; border: 4px solid transparent; padding: 0 }";
        let clipped = painted(&format!(
            "{gradient}\nbutton {{ background-clip: padding-box }}"
        ));
        let full = painted(gradient);
        assert_eq!(pixel(&clipped, 0, 0).alpha(), 0);
        assert_ne!(pixel(&full, 0, 0).alpha(), 0);
        assert_eq!(pixel(&full, 20, 10), pixel(&clipped, 20, 10));
    }
}
