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

use crate::css::computed::{Background, ComputedStyle};
use crate::layout::Allocation;
use crate::text::FontStack;

/// Paint `label` in a button of `allocation` at `origin`, styled by `style`.
pub fn paint_button(
    surface: &mut Surface,
    origin: (f32, f32),
    style: &ComputedStyle,
    allocation: &Allocation,
    label: &str,
    fonts: &FontStack,
) {
    let (ox, oy) = origin;
    let radius = style.border_radius;
    let border = style.border_width;

    // --- background: the padding box, i.e. inside the border ------------
    let bg = Rect::from_xywh(
        ox + border,
        oy + border,
        (allocation.width - border * 2.0).max(0.0),
        (allocation.height - border * 2.0).max(0.0),
    );
    let bg_radius = (radius - border).max(0.0);
    let bg_height = bg.bottom - bg.top;

    match style.background {
        Background::Transparent => {}
        Background::Solid(color) => {
            let mut paint = Paint::new();
            paint.set_color32(color);
            paint.set_style(Style::Fill);
            paint.set_anti_alias(true);
            surface
                .canvas()
                .draw_round_rect(&bg, bg_radius, bg_radius, &paint);
        }
        Background::LinearGradientToTop { .. } => {
            let mut canvas = surface.canvas();
            let save = canvas.save();
            // Clip to the rounded padding box, then fill 1px bands.
            let mut clip = PathBuilder::new();
            clip.add_round_rect(&bg, bg_radius, bg_radius);
            canvas.clip_path(&clip.build(), ClipOp::Intersect, true);
            let rows = bg_height.ceil() as i32;
            for row in 0..rows {
                let y = bg.top + row as f32;
                let color = style.background.color_at(bg_height, y - bg.top + 0.5);
                let mut paint = Paint::new();
                paint.set_color32(color);
                paint.set_style(Style::Fill);
                paint.set_anti_alias(false);
                canvas.draw_rect(
                    &Rect::new(bg.left, y, bg.right, (y + 1.0).min(bg.bottom)),
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
    if !label.is_empty()
        && let Some(blob) = fonts.blob(label, style.font_size)
    {
        let metrics = fonts.measure(label, style.font_size);
        let mut paint = Paint::new();
        paint.set_color32(style.color);
        paint.set_style(Style::Fill);
        paint.set_anti_alias(true);
        surface.canvas().draw_text_blob(
            &blob,
            ox + allocation.label_x,
            oy + allocation.label_y + metrics.ascent,
            &paint,
        );
    }
}
