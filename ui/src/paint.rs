//! Skia paint for one themed button.
//!
//! Backgrounds are painted row-band by row-band under a rounded-rect clip
//! rather than through a gradient shader: `Fill::color_at` is then the
//! single definition of what color lands on any given row, so a pixel
//! assertion is derivable straight from the theme's declaration.

use skia_rs_safe::canvas::{ClipOp, Surface};
use skia_rs_safe::core::{Color, Rect};
use skia_rs_safe::paint::{Paint, Style};
use skia_rs_safe::path::PathBuilder;

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::color::{ColorValue, Rgba};
use crate::css::value::image::{ColorStop, GradientKind, LinearDirection, SideOrCorner};
use crate::css::value::{Gradient, Image, Keyword, Length};
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
    let radius = style.border_radii(allocation.width, allocation.height)[0][0];
    let border = style.border_widths()[0];
    let fill = Fill::of(style);

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

    let (clip, clip_radius) = match style.get::<Keyword>(Prop::BackgroundClip) {
        Keyword::PaddingBox => (origin_box, (radius - border).max(0.0)),
        Keyword::ContentBox => {
            let [top, right, bottom, left] = style.padding(allocation.width);
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
        // `border-box` and anything else: the whole element.
        _ => (
            Rect::from_xywh(ox, oy, allocation.width, allocation.height),
            radius,
        ),
    };

    match fill {
        Fill::None => {}
        Fill::Solid(color) => {
            let mut paint = Paint::new();
            paint.set_color32(color);
            paint.set_style(Style::Fill);
            paint.set_anti_alias(true);
            surface
                .canvas()
                .draw_round_rect(&clip, clip_radius, clip_radius, &paint);
        }
        Fill::ToTop { .. } => {
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
                let color = fill.color_at(origin_height, y - origin_box.top + 0.5);
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
    let border_color = style.border_colors()[0].to_color32();
    if border > 0.0 && border_color.alpha() > 0 {
        let half = border / 2.0;
        let outline = Rect::from_xywh(
            ox + half,
            oy + half,
            (allocation.width - border).max(0.0),
            (allocation.height - border).max(0.0),
        );
        let outline_radius = (radius - half).max(0.0);
        let mut paint = Paint::new();
        paint.set_color32(border_color);
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
        paint.set_color32(style.color().to_color32());
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

/// The one background shape this module paints.
///
/// A **P3 shim**: it recognises exactly what M1's `Background` did -- a flat
/// fill and a two-stop `linear-gradient(to top, ...)` -- out of the computed
/// background layers, so the offscreen gate's pinned bytes are reproduced
/// without depending on the general gradient sampler. P4 deletes it along with
/// this whole module.
enum Fill {
    None,
    Solid(Color),
    ToTop {
        from: (Color, f32),
        to: (Color, Option<f32>),
    },
}

/// Round-to-nearest linear interpolation between two bytes.
///
/// Deliberately *not* `Color4f::lerp`: that round-trips each channel through
/// `x / 255.0` and back, so it is not bit-identical to rounding the byte-space
/// interpolation once. The offscreen gate asserts exact pixel equality against
/// values derived straight from the theme's declarations.
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = f32::from(a);
    let b = f32::from(b);
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

impl Fill {
    /// The topmost background layer, or the background colour behind it.
    fn of(style: &ComputedStyle) -> Self {
        let layers = style.background_layers();
        if let Some(layer) = layers.first() {
            match &layer.image {
                Image::Solid(ColorValue::Absolute(rgba)) => return Self::Solid(rgba.to_color32()),
                Image::Gradient(gradient) => {
                    if let Some(fill) = Self::from_gradient(gradient) {
                        return fill;
                    }
                    tracing::debug!("gradient shape is beyond M1's painter; painting nothing");
                }
                other => {
                    tracing::debug!(image = ?other, "image kind is beyond M1's painter");
                }
            }
        }
        let color = style.get::<Rgba>(Prop::BackgroundColor);
        if color.a <= 0.0 {
            Self::None
        } else {
            Self::Solid(color.to_color32())
        }
    }

    fn from_gradient(gradient: &Gradient) -> Option<Self> {
        if gradient.repeating {
            return None;
        }
        let GradientKind::Linear {
            direction: LinearDirection::Side(SideOrCorner::Top),
        } = gradient.kind
        else {
            return None;
        };
        let [from, to] = gradient.stops.as_ref() else {
            return None;
        };
        let color = |stop: &ColorStop| match &stop.color {
            ColorValue::Absolute(rgba) => Some(rgba.to_color32()),
            _ => None,
        };
        let position = |stop: &ColorStop| match stop.position.as_ref() {
            Some(Length::Abs { value, .. }) => Some(Some(*value)),
            None => Some(None),
            Some(_) => None,
        };
        Some(Self::ToTop {
            from: (color(from)?, position(from)?.unwrap_or(0.0)),
            to: (color(to)?, position(to)?),
        })
    }

    /// The colour this fill paints at `y_from_top` in a box `height` tall.
    ///
    /// `to top` gradients are sampled along a line whose origin is the
    /// *bottom* edge, so this converts. Positions before the first stop and
    /// after the last clamp to that stop's colour, per CSS.
    fn color_at(&self, height: f32, y_from_top: f32) -> Color {
        match self {
            Self::None => Color::TRANSPARENT,
            Self::Solid(color) => *color,
            Self::ToTop { from, to } => {
                let line = height.max(1.0);
                let p0 = from.1;
                let p1 = to.1.unwrap_or(line);
                let y = (line - y_from_top).clamp(0.0, line);
                if y <= p0 || (p1 - p0).abs() < f32::EPSILON {
                    return from.0;
                }
                if y >= p1 {
                    return to.0;
                }
                let t = (y - p0) / (p1 - p0);
                Color::from_argb(
                    lerp_u8(from.0.alpha(), to.0.alpha(), t),
                    lerp_u8(from.0.red(), to.0.red(), t),
                    lerp_u8(from.0.green(), to.0.green(), t),
                    lerp_u8(from.0.blue(), to.0.blue(), t),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::paint_button;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ComputedStyle;
    use crate::css::node::Node;
    use crate::layout::Allocation;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    /// Paint one 40x20 square button with `css` applied and read a pixel back.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let window = Node::with_classes("window", &["background"]);
        let node = Node::new("button");
        window.append_child(&node);
        let style = ComputedStyle::resolve_chain(
            &sheet,
            &node,
            &crate::css::computed::ResolveEnv::default(),
            &mut crate::css::select::MatchCx::new(),
        );
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

    #[test]
    fn gradient_sampling_follows_to_top_semantics() {
        // to top => the gradient line starts at the bottom edge.
        let hover = super::Fill::ToTop {
            from: (Color(0xFFD6_D1CD), 0.0),
            to: (Color(0xFFE8_E6E3), Some(1.0)),
        };
        let h = 30.0;
        // The bottom edge itself is the first stop.
        assert_eq!(hover.color_at(h, h), Color(0xFFD6_D1CD));
        // The bottom pixel row's centre sits halfway through the 1px stop
        // band, so it is the midpoint of the two stops, not either of them.
        assert_eq!(hover.color_at(h, h - 0.5), Color(0xFFDF_DCD8));
        // Anything above 1px from the bottom is past the second stop.
        assert_eq!(hover.color_at(h, h / 2.0), Color(0xFFE8_E6E3));
        assert_eq!(hover.color_at(h, 0.5), Color(0xFFE8_E6E3));

        let normal = super::Fill::ToTop {
            from: (Color(0xFFF6_F5F4), 2.0),
            to: (Color(0xFFFB_FAFA), None),
        };
        // Below the 2px first stop the fill is flat.
        assert_eq!(normal.color_at(h, h - 0.5), Color(0xFFF6_F5F4));
        assert_eq!(normal.color_at(h, h - 1.5), Color(0xFFF6_F5F4));
        // The top edge is the second stop.
        assert_eq!(normal.color_at(h, 0.0), Color(0xFFFB_FAFA));
        // The middle is strictly between the two.
        let mid = normal.color_at(h, h / 2.0);
        assert!(mid != Color(0xFFF6_F5F4) && mid != Color(0xFFFB_FAFA));
        assert!((0xF6..=0xFB).contains(&mid.red()));
        assert!((0xF5..=0xFA).contains(&mid.green()));
        assert!((0xF4..=0xFA).contains(&mid.blue()));
    }
}
