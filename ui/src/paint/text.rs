//! Text paint: `text-shadow` behind, `text-decoration-*` and the glyph run.
//!
//! `letter-spacing` and `text-transform` are already baked into the blob by
//! `text::FontDatabase::shape`; this module only positions and colours it.

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{ColorValue, Keyword, LengthCtx, Rgba, Shadow, TextDecorationLines, Value};
use crate::layout::Rect;
use crate::paint::fill_paint;
use crate::paint::shadow::blit_blurred;
use crate::text::ShapedText;

/// The rects a `text-decoration-line` set paints, top-first.
///
/// Positions come from the run's own metrics: overline sits at the top of
/// the ascent, line-through at half the ascent above the baseline, and
/// underline one tenth of the size below it. Thickness is a sixteenth of
/// the size, floored at 1px, which is what GTK renders at UI sizes.
#[must_use]
pub fn decoration_rects(
    text: &ShapedText,
    origin: (f32, f32),
    lines: TextDecorationLines,
) -> Vec<Rect> {
    let (ox, oy) = (
        if origin.0.is_finite() { origin.0 } else { 0.0 },
        if origin.1.is_finite() { origin.1 } else { 0.0 },
    );
    let width = if text.metrics.width.is_finite() {
        text.metrics.width.max(0.0)
    } else {
        0.0
    };
    let ascent = text.metrics.ascent.max(0.0);
    let baseline = oy + ascent;
    let thickness = (text.size_px / 16.0).max(1.0);

    let mut out = Vec::new();
    if lines.contains(TextDecorationLines::OVERLINE) {
        out.push(Rect::new(ox, oy, width, thickness));
    }
    if lines.contains(TextDecorationLines::LINE_THROUGH) {
        out.push(Rect::new(ox, baseline - ascent / 2.0, width, thickness));
    }
    if lines.contains(TextDecorationLines::UNDERLINE) {
        out.push(Rect::new(
            ox,
            baseline + text.size_px / 10.0,
            width,
            thickness,
        ));
    }
    // `blink` renders nothing; CSS allows a UA to ignore it entirely.
    out
}

/// Paint one shaped run at `origin` (the content box's top-left).
pub fn paint_text(
    canvas: &mut Canvas<'_>,
    text: &ShapedText,
    origin: (f32, f32),
    style: &ComputedStyle,
    ctx: &LengthCtx,
) {
    let (ox, oy) = (
        if origin.0.is_finite() { origin.0 } else { 0.0 },
        if origin.1.is_finite() { origin.1 } else { 0.0 },
    );
    let color = style.color();
    let baseline = oy + text.metrics.ascent.max(0.0);

    // `TextDecorationLines` has no `FromValue` impl (only `Keyword`, `Rgba`,
    // `Rc<[Shadow]>` and the other scalar wrappers registered in
    // `css/computed.rs` do), and P4 may not add one there, so the flags are
    // read directly off the raw computed `Value`.
    let lines: TextDecorationLines = match style.raw(Prop::TextDecorationLine) {
        Value::TextDecorationLines(l) => *l,
        _ => TextDecorationLines::empty(),
    };
    let decoration_color = match style.raw(Prop::TextDecorationColor) {
        Value::Color(ColorValue::Absolute(rgba)) => *rgba,
        _ => color,
    };
    let decoration_style: Keyword = style.get(Prop::TextDecorationStyle);
    let rects = decoration_rects(text, (ox, oy), lines);

    // 1. text-shadow, behind everything. `Rc<[Shadow]>` already has a
    // `FromValue` impl (it backs `ComputedStyle::box_shadows()`), so
    // `text-shadow` reuses it via `style.get` rather than re-deriving the
    // `Value::List`/`Value::Shadow` match by hand.
    let shadows: std::rc::Rc<[Shadow]> = style.get(Prop::TextShadow);
    for shadow in shadows.iter().rev() {
        paint_text_shadow(canvas, text, (ox, baseline), shadow, color, ctx);
    }

    // 2. decorations under the glyphs, so descenders stay legible.
    for rect in &rects {
        paint_decoration(canvas, *rect, decoration_color, decoration_style);
    }

    // 3. the glyph run.
    if let Some(blob) = text.blob.as_ref() {
        canvas.draw_text_blob(blob, ox, baseline, &fill_paint(color));
    }
}

/// One decoration line, honouring `text-decoration-style`.
fn paint_decoration(canvas: &mut Canvas<'_>, rect: Rect, color: Rgba, style: Keyword) {
    if rect.is_empty() || color.a <= 0.0 {
        return;
    }
    let mut paint = fill_paint(color);
    paint.set_anti_alias(false);
    match style {
        Keyword::Double => {
            let gap = rect.height * 2.0;
            canvas.draw_rect(&rect.to_skia(), &paint);
            canvas.draw_rect(
                &Rect::new(rect.x, rect.y + gap, rect.width, rect.height).to_skia(),
                &paint,
            );
        }
        Keyword::Dotted | Keyword::Dashed => {
            let on = if matches!(style, Keyword::Dotted) {
                rect.height
            } else {
                rect.height * 3.0
            };
            let step = on * 2.0;
            let mut x = rect.x;
            while x < rect.x + rect.width && step > 0.0 {
                let w = on.min(rect.x + rect.width - x);
                canvas.draw_rect(&Rect::new(x, rect.y, w, rect.height).to_skia(), &paint);
                x += step;
            }
        }
        // `wavy` is drawn solid in M2 (spec §Section 4).
        _ => canvas.draw_rect(&rect.to_skia(), &paint),
    }
}

/// One `text-shadow`: the run drawn offset, blurred if asked.
fn paint_text_shadow(
    canvas: &mut Canvas<'_>,
    text: &ShapedText,
    baseline_origin: (f32, f32),
    shadow: &Shadow,
    current: Rgba,
    ctx: &LengthCtx,
) {
    let Some(blob) = text.blob.as_ref() else {
        return;
    };
    let color = match shadow.color.as_ref() {
        Some(ColorValue::Absolute(rgba)) => *rgba,
        Some(_) | None => current,
    };
    if color.a <= 0.0 {
        return;
    }
    let dx = shadow
        .offset_x
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let dy = shadow
        .offset_y
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let blur = shadow
        .blur
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
    let (bx, by) = (baseline_origin.0 + dx, baseline_origin.1 + dy);

    if blur <= 0.0 {
        canvas.draw_text_blob(blob, bx, by, &fill_paint(color));
        return;
    }
    // The run's ink rect, in the caller's coordinates. `blit_blurred` owns
    // the offscreen surface budget -- the padding, the sigma cap that keeps
    // the blur from being cut off at a hard edge, and the dimension ceiling
    // that is the only defence against `text-shadow: 0 0 99999px` asking
    // for a gigapixel buffer. Duplicating it here let the two copies drift.
    let ink = Rect::new(
        bx,
        by - text.metrics.ascent,
        text.metrics.width,
        text.metrics.ascent + text.metrics.descent,
    );
    blit_blurred(canvas, ink, blur, |offscreen, offset, _surface| {
        offscreen.draw_text_blob(blob, bx - offset.0, by - offset.1, &fill_paint(color));
    });
}

#[cfg(test)]
mod tests {
    use super::{decoration_rects, paint_text};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{
        FontFamily, FontStyle, GenericFamily, Keyword, LengthCtx, TextDecorationLines,
    };
    use crate::text::{FontDatabase, FontQuery, ShapeKey};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    /// Shape "Click me" at 14px with the system probe face.
    fn shaped(db: &mut FontDatabase) -> std::rc::Rc<crate::text::ShapedText> {
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("no system font found; install dejavu/liberation/noto sans");
        db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        })
    }

    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let label = Node::new("label");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &label, &env, &mut cx);
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let mut surface = Surface::new_raster_n32_premul(120, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_text(&mut canvas, &text, (10.0, 10.0), &style, &ctx());
        }
        surface
    }

    // Reconciliation (plan gave this comparing the stored premultiplied red
    // byte directly): `PixelBuffer::get_pixel` returns premultiplied
    // storage, where a translucent, anti-aliased white-glyph edge pixel has
    // `red == alpha` (e.g. `(37, 37)`) -- not 255. Skia's own glyph
    // rasteriser (`Canvas::draw_text_blob` fills each glyph outline as a
    // regular AA path) produces plenty of such partial-coverage boundary
    // pixels even at the interior of a thin 14px stroke, so comparing the
    // raw premultiplied byte against `0xC0`/`0x40` flagged most of a white
    // run's own anti-aliased edge as "dark": empirically 203 of 294 painted
    // pixels for `color: #ffffff`, and 56 for the shadow test's shadow-free
    // control -- both meant to be exactly zero. Un-premultiplying first
    // recovers the *intended* colour regardless of coverage (white stays
    // 255 at every alpha; `#2e3436`'s ~0x2e survives division essentially
    // unchanged), which is what "dark pixel" is supposed to mean here.
    fn dark_pixels(surface: &Surface, threshold: u8) -> u32 {
        let mut count = 0;
        for y in 0..40 {
            for x in 0..120 {
                let p = surface.pixel_buffer().get_pixel(x, y).expect("pixel");
                if p.alpha() == 0 {
                    continue;
                }
                let unpremul_red = (u16::from(p.red()) * 255 / u16::from(p.alpha())).min(255) as u8;
                if unpremul_red < threshold {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn the_label_is_actually_drawn_in_the_computed_colour() {
        // M1's `the_label_is_actually_drawn`, preserved. Mutation check:
        // painting with the initial colour instead of `style.color()` makes
        // the run black rather than #2e3436 -- caught by the second count.
        let surface = painted("label { color: #2e3436 }");
        assert!(
            dark_pixels(&surface, 0xC0) > 20,
            "the glyph run reached the canvas"
        );
        let white = painted("label { color: #ffffff }");
        assert_eq!(
            dark_pixels(&white, 0xC0),
            0,
            "a white label has no dark pixels"
        );
    }

    #[test]
    fn an_underline_puts_ink_below_the_baseline_across_the_run() {
        // Mutation check: computing the rects against the run's origin
        // instead of its baseline draws the line through the glyphs.
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let rects = decoration_rects(&text, (10.0, 10.0), TextDecorationLines::UNDERLINE);
        assert_eq!(rects.len(), 1);
        assert!(
            rects[0].y > 10.0 + text.metrics.ascent,
            "below the baseline"
        );
        assert!(rects[0].width >= text.metrics.width - 0.01);
    }

    #[test]
    fn overline_and_line_through_are_placed_above_and_across_the_glyphs() {
        // Mutation check: emitting the same y for all three lines collapses
        // the ordering assertion.
        let mut db = FontDatabase::probe_only();
        let text = shaped(&mut db);
        let all = decoration_rects(
            &text,
            (10.0, 10.0),
            TextDecorationLines::UNDERLINE
                | TextDecorationLines::OVERLINE
                | TextDecorationLines::LINE_THROUGH,
        );
        assert_eq!(all.len(), 3);
        let mut ys: Vec<f32> = all.iter().map(|r| r.y).collect();
        ys.sort_by(f32::total_cmp);
        assert!(
            ys[0] < ys[1] && ys[1] < ys[2],
            "overline, line-through, underline"
        );
    }

    #[test]
    fn a_text_shadow_lands_offset_from_the_glyphs() {
        // Mutation check: painting the shadow after the text hides it; the
        // offset column then matches the no-shadow render exactly.
        let with = painted("label { color: #ffffff; text-shadow: 4px 0 #000000 }");
        let without = painted("label { color: #ffffff }");
        assert!(dark_pixels(&with, 0x40) > 0);
        assert_eq!(dark_pixels(&without, 0x40), 0);
    }

    #[test]
    fn text_painting_never_panics_on_an_empty_run() {
        let mut db = FontDatabase::probe_only();
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let empty = db.shape(&ShapeKey {
            text: "",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        let sheet = CompiledSheet::compile("label { text-decoration-line: underline }");
        let node = Node::new("label");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut surface = Surface::new_raster_n32_premul(20, 20).expect("raster surface");
        let mut canvas = surface.canvas();
        paint_text(
            &mut canvas,
            &empty,
            (f32::NAN, f32::INFINITY),
            &style,
            &ctx(),
        );
    }
}
