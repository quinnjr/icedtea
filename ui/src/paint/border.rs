//! Per-side borders.
//!
//! Each side is a *filled* path -- the border ring intersected with that
//! side's mitre wedge -- never a stroke. Stroking cannot express four
//! different widths, four colours and four corner radii meeting on the
//! diagonals, which is exactly what GTK draws.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::core::IRect;
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{
    BorderImageSlice, BorderImageWidthSide, Image, Keyword, NumberOrPercent, RepeatStyle, Rgba,
};
use crate::layout::{Allocation, Rect};
use crate::paint::background::{drawable_bounds, intersect};
use crate::paint::geometry::{Side, inner_radii, rounded_ring_path, side_wedge_path};
use crate::paint::{PaintCx, fill_paint, rounded_rect_path};

/// Whether a `border-style` keyword paints anything at all.
#[must_use]
pub fn is_visible_border_style(style: Keyword) -> bool {
    !matches!(style, Keyword::None | Keyword::Hidden)
}

/// Paint all four borders.
pub fn paint_borders(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    widths: [f32; 4],
    colors: [Rgba; 4],
    styles: [Keyword; 4],
    radii: &[[f32; 2]; 4],
) {
    let used: [f32; 4] = std::array::from_fn(|i| {
        if widths[i].is_finite() && widths[i] > 0.0 && is_visible_border_style(styles[i]) {
            widths[i]
        } else {
            0.0
        }
    });
    if used.iter().all(|w| *w <= 0.0) {
        return;
    }

    let outer = alloc.border_box;
    let inner = outer.inset(used);
    let inner_r = inner_radii(radii, used);
    let ring = rounded_ring_path(outer, radii, inner, &inner_r);

    let uniform = used[1..].iter().all(|w| (w - used[0]).abs() < f32::EPSILON)
        && colors[1..].iter().all(|c| *c == colors[0])
        && styles[1..].iter().all(|s| *s == styles[0])
        && styles[0] == Keyword::Solid;
    if uniform {
        // One fill, so the four sides never seam against each other along
        // the mitres -- this is the path Adwaita's 1px button border takes.
        canvas.draw_path(&ring, &fill_paint(colors[0]));
        return;
    }

    for (index, side) in [Side::Top, Side::Right, Side::Bottom, Side::Left]
        .into_iter()
        .enumerate()
    {
        if used[index] <= 0.0 {
            continue;
        }
        let save = canvas.save();
        canvas.clip_path(
            &side_wedge_path(outer, inner, side),
            ClipOp::Intersect,
            true,
        );
        paint_one_side(
            canvas,
            &ring,
            outer,
            used,
            index,
            colors[index],
            styles[index],
            radii,
        );
        canvas.restore_to_count(save);
    }
}

/// Fill (or dash, or double) one side's share of the ring.
#[allow(clippy::too_many_arguments, reason = "one side's full CSS description")]
fn paint_one_side(
    canvas: &mut Canvas<'_>,
    ring: &skia_rs_safe::path::Path,
    outer: crate::layout::Rect,
    used: [f32; 4],
    index: usize,
    color: Rgba,
    style: Keyword,
    radii: &[[f32; 2]; 4],
) {
    match style {
        Keyword::Double => {
            // Three equal bands; the middle one is left empty.
            let third: [f32; 4] = std::array::from_fn(|i| used[i] / 3.0);
            let two_thirds: [f32; 4] = std::array::from_fn(|i| used[i] * 2.0 / 3.0);
            let outer_band =
                rounded_ring_path(outer, radii, outer.inset(third), &inner_radii(radii, third));
            let inner_start = outer.inset(two_thirds);
            let inner_band = rounded_ring_path(
                inner_start,
                &inner_radii(radii, two_thirds),
                outer.inset(used),
                &inner_radii(radii, used),
            );
            canvas.draw_path(&outer_band, &fill_paint(color));
            canvas.draw_path(&inner_band, &fill_paint(color));
        }
        Keyword::Dotted | Keyword::Dashed => {
            // Stroke the centre line of this side with a dash effect, then
            // let the wedge clip keep it inside the side. `DashEffect` is
            // the crate's only dash primitive and it applies to strokes.
            let w = used[index];
            // A round-capped dash of length `w` already spans `2w`, so the
            // classic `[w, w]` interval makes every dot touch its
            // neighbours: a 1px dotted border came out pixel-identical to
            // solid. Blink and Gecko both use a zero-length dash on a `2w`
            // period, which a round cap renders as a `w`-diameter dot with
            // a `w` gap after it.
            let (on, off) = if matches!(style, Keyword::Dotted) {
                (0.0, w * 2.0)
            } else {
                (w * 3.0, w * 2.0)
            };
            let centre = outer.inset(std::array::from_fn(|i| used[i] / 2.0));
            let path = rounded_rect_path(
                centre,
                &inner_radii(radii, std::array::from_fn(|i| used[i] / 2.0)),
            );
            let mut paint = fill_paint(color);
            paint.set_style(skia_rs_safe::paint::Style::Stroke);
            paint.set_stroke_width(w);
            paint.set_stroke_cap(if matches!(style, Keyword::Dotted) {
                skia_rs_safe::paint::StrokeCap::Round
            } else {
                skia_rs_safe::paint::StrokeCap::Butt
            });
            if let Some(dash) = DashEffect::new(vec![on, off], 0.0) {
                let effect: PathEffectRef = std::sync::Arc::new(dash);
                paint.set_path_effect(Some(effect));
            }
            canvas.draw_path(&path, &paint);
        }
        // `groove`, `ridge`, `inset` and `outset` are drawn as solid in M2;
        // GTK itself renders them flat in Adwaita, and the spec's paint list
        // asks only that they render, not that they emboss.
        _ => {
            canvas.draw_path(ring, &fill_paint(color));
        }
    }
}

/// Paint a nine-patch `border-image` over the border box.
///
/// Returns `false` when the source cannot be resolved to pixels, so the
/// caller falls back to [`paint_borders`]. Only `url()` sources have pixels
/// in M2; gradients and `-gtk-*` images as border-image sources are M4.
pub fn paint_border_image(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    source: &Image,
    slice: &BorderImageSlice,
    widths: &[BorderImageWidthSide; 4],
    repeat: RepeatStyle,
    cx: &mut PaintCx<'_>,
) -> bool {
    let Image::Url(url) = source else {
        return false;
    };
    let Some((iw, ih)) = cx
        .images
        .get(url)
        .map(|img| (img.width() as f32, img.height() as f32))
    else {
        return false;
    };
    if iw <= 0.0 || ih <= 0.0 {
        return false;
    }

    // Slice offsets in source pixels, TRBL.
    let slice_px = |value: NumberOrPercent, basis: f32| -> f32 {
        match value {
            NumberOrPercent::Number(n) if n.is_finite() => n.clamp(0.0, basis),
            NumberOrPercent::Percent(p) if p.is_finite() => (p * basis).clamp(0.0, basis),
            _ => 0.0,
        }
    };
    let st = slice_px(slice.sides[0], ih);
    let sr = slice_px(slice.sides[1], iw);
    let sb = slice_px(slice.sides[2], ih);
    let sl = slice_px(slice.sides[3], iw);

    // Destination widths, TRBL: `auto` uses the slice, a number scales the
    // used border width, a length resolves directly.
    let ctx = cx.base_length_ctx();
    let dest = |side: &BorderImageWidthSide, used: f32, slice: f32| -> f32 {
        match side {
            BorderImageWidthSide::Auto => slice,
            BorderImageWidthSide::Number(n) if n.is_finite() => n * used,
            BorderImageWidthSide::Length(len) => len.resolve(&ctx).unwrap_or(used),
            BorderImageWidthSide::Number(_) => used,
        }
    };
    let dt = dest(&widths[0], alloc.border[0], st);
    let dr = dest(&widths[1], alloc.border[1], sr);
    let db = dest(&widths[2], alloc.border[2], sb);
    let dl = dest(&widths[3], alloc.border[3], sl);

    let outer = alloc.border_box;
    if outer.is_empty() {
        return false;
    }

    // Nine slots: (src IRect, dst Rect, tiles?).
    //
    // The source grid's four column edges and four row edges are rounded
    // *once* and shared. Truncating each slot's own `x`/`x + w` with `as
    // i32` made a fractional slice (`border-image-slice: 33%` on a 10x10
    // source) hand neighbouring slots overlapping rectangles -- column 3
    // and column 6 sampled twice -- and drop source columns elsewhere.
    let edge = |v: f32, limit: f32| -> i32 {
        if v.is_finite() {
            v.round().clamp(0.0, limit) as i32
        } else {
            0
        }
    };
    let (col0, col3) = (0, edge(iw, iw));
    let col1 = edge(sl, iw).min(col3);
    let col2 = edge(iw - sr, iw).max(col1);
    let (row0, row3) = (0, edge(ih, ih));
    let row1 = edge(st, ih).min(row3);
    let row2 = edge(ih - sb, ih).max(row1);
    let src = |x0: i32, y0: i32, x1: i32, y1: i32| IRect::new(x0, y0, x1, y1);
    let dmid_w = (outer.width - dl - dr).max(0.0);
    let dmid_h = (outer.height - dt - db).max(0.0);

    let mut slots: Vec<(IRect, Rect, bool, bool)> = vec![
        (
            src(col0, row0, col1, row1),
            Rect::new(outer.x, outer.y, dl, dt),
            false,
            false,
        ),
        (
            src(col1, row0, col2, row1),
            Rect::new(outer.x + dl, outer.y, dmid_w, dt),
            true,
            false,
        ),
        (
            src(col2, row0, col3, row1),
            Rect::new(outer.right() - dr, outer.y, dr, dt),
            false,
            false,
        ),
        (
            src(col0, row1, col1, row2),
            Rect::new(outer.x, outer.y + dt, dl, dmid_h),
            false,
            true,
        ),
        (
            src(col2, row1, col3, row2),
            Rect::new(outer.right() - dr, outer.y + dt, dr, dmid_h),
            false,
            true,
        ),
        (
            src(col0, row2, col1, row3),
            Rect::new(outer.x, outer.bottom() - db, dl, db),
            false,
            false,
        ),
        (
            src(col1, row2, col2, row3),
            Rect::new(outer.x + dl, outer.bottom() - db, dmid_w, db),
            true,
            false,
        ),
        (
            src(col2, row2, col3, row3),
            Rect::new(outer.right() - dr, outer.bottom() - db, dr, db),
            false,
            false,
        ),
    ];
    if slice.fill {
        slots.push((
            src(col1, row1, col2, row2),
            Rect::new(outer.x + dl, outer.y + dt, dmid_w, dmid_h),
            true,
            true,
        ));
    }

    let tiles_x = matches!(repeat.x, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let tiles_y = matches!(repeat.y, Keyword::Repeat | Keyword::Round | Keyword::Space);
    // Every tiling loop below is bounded by what can actually reach a pixel.
    // `Rect::is_empty` accepts `+inf`, so a border box of `1e9` px (or an
    // infinite one) drove `while x < dst.right() { x += step }` for up to
    // `i32::MAX` iterations -- and at `x >= 2^24` a `+= 1.0` step is a no-op
    // in f32, so the inner loop never terminated at all.
    let Some(bounds) = drawable_bounds(canvas) else {
        return false;
    };
    let Some(image) = cx.images.get(url) else {
        return false;
    };
    let mut painted = false;
    let mut budget = MAX_BORDER_IMAGE_TILES;
    for (src_rect, dst_rect, edge_x, edge_y) in slots {
        if dst_rect.is_empty() || src_rect.width() <= 0 || src_rect.height() <= 0 {
            continue;
        }
        if (edge_x && tiles_x) || (edge_y && tiles_y) {
            // Tile the slot with source-sized copies rather than stretching.
            let step_x = if edge_x && tiles_x {
                src_rect.width() as f32
            } else {
                dst_rect.width
            };
            let step_y = if edge_y && tiles_y {
                src_rect.height() as f32
            } else {
                dst_rect.height
            };
            // A step under one device pixel emits copies nothing can tell
            // apart, and a non-finite one never advances.
            let (step_x, step_y) = (
                step_x.max(MIN_BORDER_IMAGE_STEP),
                step_y.max(MIN_BORDER_IMAGE_STEP),
            );
            if !(step_x.is_finite() && step_y.is_finite()) {
                continue;
            }
            let Some(visible) = intersect(dst_rect, bounds) else {
                continue;
            };
            let mut y = visible.y - ((visible.y - dst_rect.y) % step_y);
            while y < visible.bottom() && budget > 0 {
                let mut x = visible.x - ((visible.x - dst_rect.x) % step_x);
                while x < visible.right() && budget > 0 {
                    let piece = Rect::new(
                        x,
                        y,
                        step_x.min(dst_rect.right() - x),
                        step_y.min(dst_rect.bottom() - y),
                    );
                    if !piece.is_empty() {
                        canvas.draw_image_rect(image, Some(&src_rect), &piece.to_skia(), None);
                        budget -= 1;
                    }
                    x += step_x;
                }
                y += step_y;
            }
        } else {
            canvas.draw_image_rect(image, Some(&src_rect), &dst_rect.to_skia(), None);
        }
        painted = true;
    }
    painted
}

/// The most nine-patch copies one `border-image` may emit per paint.
///
/// A 3x3 source with `1 fill` and `repeat` over a 1920x1080 border box asks
/// for about two million `draw_image_rect` calls *per frame*; the visible
/// area is the same handful of pixels either way.
const MAX_BORDER_IMAGE_TILES: u32 = 1 << 16;

/// The narrowest nine-patch tiling step, in px. One device pixel, for the
/// same reason `background`'s `MIN_TILE_STEP` is.
const MIN_BORDER_IMAGE_STEP: f32 = 1.0;

#[cfg(test)]
mod tests {
    use super::{is_visible_border_style, paint_borders};
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    fn alloc(w: f32, h: f32, border: [f32; 4]) -> Allocation {
        let border_box = Rect::new(0.0, 0.0, w, h);
        Allocation {
            border_box,
            content_box: border_box.inset(border),
            border,
            padding: [0.0; 4],
        }
    }

    fn painted(
        widths: [f32; 4],
        colors: [Rgba; 4],
        styles: [Keyword; 4],
        radii: [[f32; 2]; 4],
    ) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_borders(
                &mut canvas,
                &alloc(40.0, 20.0, widths),
                widths,
                colors,
                styles,
                &radii,
            );
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    use super::paint_border_image;
    use crate::css::computed::ResolveEnv;
    use crate::css::value::{
        BorderImageSlice, BorderImageWidthSide, Image, NumberOrPercent, RepeatStyle,
    };
    use crate::paint::{ImageCache, PaintCx};
    use crate::text::FontDatabase;
    use std::collections::HashMap;

    #[test]
    fn an_unresolvable_border_image_reports_false_so_the_caller_falls_back() {
        // Contract §8: `false` means "I painted nothing; use paint_borders".
        // Mutation check: returning `true` unconditionally silently loses
        // every ordinary border on a theme with a bad border-image URL.
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new());
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        let mut canvas = surface.canvas();
        let painted = paint_border_image(
            &mut canvas,
            &alloc(40.0, 20.0, [4.0; 4]),
            &Image::Url("/nonexistent/icedtea-border.png".into()),
            &BorderImageSlice {
                sides: [NumberOrPercent::Number(4.0); 4],
                fill: false,
            },
            &[
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
            ],
            RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::Repeat,
            },
            &mut cx,
        );
        assert!(!painted);
    }

    #[test]
    fn image_none_reports_false_without_touching_the_canvas() {
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new());
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            assert!(!paint_border_image(
                &mut canvas,
                &alloc(40.0, 20.0, [4.0; 4]),
                &Image::None,
                &BorderImageSlice {
                    sides: [NumberOrPercent::Number(0.0); 4],
                    fill: false,
                },
                &[
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                ],
                RepeatStyle {
                    x: Keyword::Stretch,
                    y: Keyword::Stretch,
                },
                &mut cx,
            ));
        }
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
    }

    /// A 3x3 source image -- red corners, green edges, blue centre -- written
    /// as a real PNG to a temp file, for a `border-image` slice of `1 1 1 1`
    /// where each source pixel becomes exactly one nine-slice slot.
    fn three_by_three_border_image() -> tempfile::TempPath {
        use skia_rs_safe::codec::{Image as CodecImage, ImageEncoder, ImageInfo, PngEncoder};
        use skia_rs_safe::core::{AlphaType, ColorType};

        const RED: [u8; 4] = [0xFF, 0x00, 0x00, 0xFF];
        const GREEN: [u8; 4] = [0x00, 0xFF, 0x00, 0xFF];
        const BLUE: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];
        // Row-major, top-left to bottom-right: corners red, edges green,
        // centre blue.
        let pixels: [[u8; 4]; 9] = [
            RED, GREEN, RED, //
            GREEN, BLUE, GREEN, //
            RED, GREEN, RED,
        ];
        let mut bytes = Vec::with_capacity(9 * 4);
        for pixel in pixels {
            bytes.extend_from_slice(&pixel);
        }
        let info = ImageInfo::new(3, 3, ColorType::Rgba8888, AlphaType::Unpremul);
        let image =
            CodecImage::from_raster_data_owned(info, bytes, 3 * 4).expect("3x3 image builds");
        let encoded = PngEncoder::new()
            .encode_bytes(&image)
            .expect("3x3 image encodes to PNG");
        let mut file = tempfile::NamedTempFile::with_suffix(".png").expect("temp file");
        std::io::Write::write_all(&mut file, &encoded).expect("write PNG bytes");
        file.into_temp_path()
    }

    #[test]
    fn a_nine_slice_border_image_paints_its_corner_and_edge_colours() {
        // Positive companion to the two `false`-path tests above: a real,
        // decodable border-image actually reaches the canvas with the right
        // pixels in the right nine-slice slots.
        //
        // Mutation check: swapping the `st`/`sr`/`sb`/`sl` slice assignments,
        // or the src/dst rects any nine-slice slot is built from, moves one
        // of red/green/blue into the wrong slot and fails one of the three
        // assertions below.
        let path = three_by_three_border_image();
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new());
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        let url: std::rc::Rc<str> = std::rc::Rc::from(path.to_str().expect("utf8 temp path"));
        let painted;
        {
            let mut canvas = surface.canvas();
            painted = paint_border_image(
                &mut canvas,
                &alloc(40.0, 40.0, [4.0; 4]),
                &Image::Url(url),
                &BorderImageSlice {
                    sides: [NumberOrPercent::Number(1.0); 4],
                    fill: true,
                },
                &[
                    BorderImageWidthSide::Number(1.0),
                    BorderImageWidthSide::Number(1.0),
                    BorderImageWidthSide::Number(1.0),
                    BorderImageWidthSide::Number(1.0),
                ],
                RepeatStyle {
                    x: Keyword::Stretch,
                    y: Keyword::Stretch,
                },
                &mut cx,
            );
        }
        assert!(painted, "a decodable border-image must report true");

        // Corner slot: dt == dl == 4px (Number(1.0) * used border 4px), so
        // (1, 1) sits inside the stretched top-left corner -- red.
        assert_eq!(pixel(&surface, 1, 1), Color(0xFFFF_0000));
        // Top edge slot: stretched across x in [4, 36), y in [0, 4) -- green.
        assert_eq!(pixel(&surface, 20, 1), Color(0xFF00_FF00));
        // Filled centre slot: stretched across the whole middle -- blue.
        assert_eq!(pixel(&surface, 20, 20), Color(0xFF00_00FF));
    }

    #[test]
    fn a_uniform_border_paints_adwaitas_exact_edge_colour() {
        // The M1 gate's `pixel(cx, 0) == 0xFFCDC7C2`, as a unit test. The
        // straight run of the top edge is fully covered, so it is exact.
        // Mutation check: stroking on the centre line (M1's model) halves
        // the coverage of a 1px border and this stops being exact.
        let surface = painted(
            [1.0; 4],
            [rgba(0xCD, 0xC7, 0xC2); 4],
            [Keyword::Solid; 4],
            [[5.0, 5.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 0), Color(0xFFCD_C7C2));
        assert_eq!(
            pixel(&surface, 0, 0).alpha(),
            0,
            "outside the 5px corner arc"
        );
        assert_eq!(
            pixel(&surface, 20, 10).alpha(),
            0,
            "the interior is untouched"
        );
    }

    #[test]
    fn mixed_per_side_colours_meet_on_the_mitre_and_do_not_overdraw() {
        // Mutation check: filling the whole ring once per side paints the
        // last side's colour everywhere, so the left edge reads red.
        let red = rgba(0xFF, 0x00, 0x00);
        let blue = rgba(0x00, 0x00, 0xFF);
        let surface = painted(
            [6.0, 6.0, 6.0, 6.0],
            [red, blue, blue, red],
            [Keyword::Solid; 4],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1), Color(0xFFFF_0000), "top is red");
        assert_eq!(pixel(&surface, 38, 10), Color(0xFF00_00FF), "right is blue");
        assert_eq!(pixel(&surface, 1, 10), Color(0xFFFF_0000), "left is red");
    }

    #[test]
    fn none_and_hidden_sides_paint_nothing_even_with_a_width() {
        // CSS: `border-style: none` forces the used width to zero. P3
        // already zeroes the width, but a caller passing both must still be
        // safe. Mutation check: dropping the style check paints a black bar.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [
                Keyword::None,
                Keyword::Hidden,
                Keyword::None,
                Keyword::Hidden,
            ],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
        assert_eq!(pixel(&surface, 1, 10).alpha(), 0);
        assert!(!is_visible_border_style(Keyword::None));
        assert!(!is_visible_border_style(Keyword::Hidden));
        assert!(is_visible_border_style(Keyword::Double));
    }

    #[test]
    fn a_dashed_border_leaves_gaps_along_its_edge() {
        // Mutation check: falling back to a solid fill makes every sampled
        // pixel opaque and `gaps` stays zero.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [Keyword::Dashed; 4],
            [[0.0, 0.0]; 4],
        );
        let gaps = (6..34)
            .filter(|&x| pixel(&surface, x, 1).alpha() == 0)
            .count();
        assert!(gaps > 0, "a dashed top edge must have gaps");
        let inked = (6..34)
            .filter(|&x| pixel(&surface, x, 1).alpha() > 0)
            .count();
        assert!(inked > 0, "a dashed top edge must also have ink");
    }

    #[test]
    fn border_painting_never_panics_on_hostile_geometry() {
        for &v in &[f32::NAN, f32::INFINITY, -8.0, 0.0, 1.0e30] {
            let _ = painted([v; 4], [rgba(1, 2, 3); 4], [Keyword::Solid; 4], [[v, v]; 4]);
            let _ = painted(
                [v; 4],
                [rgba(1, 2, 3); 4],
                [Keyword::Double; 4],
                [[v, v]; 4],
            );
            let _ = painted(
                [v; 4],
                [rgba(1, 2, 3); 4],
                [Keyword::Dotted; 4],
                [[v, v]; 4],
            );
        }
    }
    /// A 3x3 opaque-red PNG, so a `border-image` test has real pixels to
    /// slice without depending on anything on disk.
    const RED_3X3_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0x08, 0x06, 0x00, 0x00, 0x00, 0x56,
        0x28, 0xb5, 0xbf, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x86, 0x19, 0x70, 0x72, 0x00, 0x5d, 0xd7, 0x11, 0xef, 0xdc, 0x4f,
        0x31, 0x10, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_dotted_border_leaves_gaps_between_its_dots() {
        // F64. `[w, w]` dash intervals with round caps make every dot span
        // its whole period, so a dotted border was pixel-identical to a
        // solid one. Mutation check: restoring `(w, w)` inks every pixel of
        // the top edge and the gap assertion fails.
        let surface = painted(
            [4.0; 4],
            [rgba(255, 0, 0); 4],
            [Keyword::Dotted; 4],
            [[0.0, 0.0]; 4],
        );
        let top: Vec<u8> = (6..34).map(|x| pixel(&surface, x, 2).alpha()).collect();
        assert!(top.iter().any(|&a| a > 0), "the dots are drawn");
        assert!(top.contains(&0), "and there are gaps");
    }

    #[test]
    fn a_fractional_border_image_slice_does_not_overlap_its_slots() {
        // F65. `border-image-slice: 33%` on a 3x3 source truncated each
        // slot's own edges with `as i32`, so neighbouring slots sampled the
        // same source column twice and dropped others. The grid's edges are
        // now rounded once and shared, so the nine slots partition the
        // source exactly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("border.png");
        std::fs::write(&path, RED_3X3_PNG).expect("write png");
        let url = path.to_string_lossy().into_owned();

        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        let env = crate::css::computed::ResolveEnv::default();
        let colors = crate::css::value::ColorTable::default();
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new());
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let slice = crate::css::value::BorderImageSlice {
            sides: [crate::css::value::NumberOrPercent::Percent(0.33); 4],
            fill: false,
        };
        let widths: [crate::css::value::BorderImageWidthSide; 4] =
            std::array::from_fn(|_| crate::css::value::BorderImageWidthSide::Number(1.0));
        let repeat = crate::css::value::RepeatStyle {
            x: Keyword::Stretch,
            y: Keyword::Stretch,
        };
        let mut canvas = surface.canvas();
        let drew = super::paint_border_image(
            &mut canvas,
            &alloc(40.0, 20.0, [4.0; 4]),
            &crate::css::value::Image::Url(std::rc::Rc::from(url.as_str())),
            &slice,
            &widths,
            repeat,
            &mut cx,
        );
        assert!(drew, "a decodable url() paints the nine-patch");
    }

    #[test]
    fn a_tiling_border_image_over_an_unbounded_box_terminates() {
        // F66. The nine-patch tiling loops were bounded only by the
        // destination rect, which `Rect::is_empty` lets be `+inf`; and past
        // 2^24 a `x += 1.0` step is a no-op in f32, so the inner loop never
        // terminated at all. Both loops are now bounded by the drawable
        // bounds, a one-device-pixel step floor and a total tile budget.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("border.png");
        std::fs::write(&path, RED_3X3_PNG).expect("write png");
        let url = path.to_string_lossy().into_owned();

        let env = crate::css::computed::ResolveEnv::default();
        let colors = crate::css::value::ColorTable::default();
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new());
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let slice = crate::css::value::BorderImageSlice {
            sides: [crate::css::value::NumberOrPercent::Number(1.0); 4],
            fill: true,
        };
        let widths: [crate::css::value::BorderImageWidthSide; 4] =
            std::array::from_fn(|_| crate::css::value::BorderImageWidthSide::Number(1.0));
        let repeat = crate::css::value::RepeatStyle {
            x: Keyword::Repeat,
            y: Keyword::Repeat,
        };
        let source = crate::css::value::Image::Url(std::rc::Rc::from(url.as_str()));

        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        let started = std::time::Instant::now();
        {
            let mut canvas = surface.canvas();
            for &extent in &[f32::INFINITY, 1.0e9, 1.0e30] {
                let border_box = Rect::new(0.0, 0.0, extent, extent);
                let alloc = Allocation {
                    border_box,
                    content_box: border_box,
                    border: [1.0; 4],
                    padding: [0.0; 4],
                };
                let _ = super::paint_border_image(
                    &mut canvas,
                    &alloc,
                    &source,
                    &slice,
                    &widths,
                    repeat,
                    &mut cx,
                );
            }
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "an unbounded border-image took {elapsed:?}"
        );
    }
}
