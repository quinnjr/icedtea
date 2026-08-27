//! Gaussian-ish blur, written here because nothing in `skia-rs-safe` 0.4.0
//! will run one for us.
//!
//! The rasterizer consults only `Paint::shader`, and `composite_layer` says
//! in so many words that a layer paint's image filter "is not yet applied
//! here" -- so `BlurMaskFilter` and `BlurImageFilter` both no-op. Shadows
//! and blurred effects therefore render into an offscreen premultiplied
//! buffer, get blurred by the three box passes below (the standard
//! three-box Gaussian approximation, with the radius split three ways so
//! the passes' variances sum to one Gaussian of the target sigma instead of
//! tripling it), and are blitted back with `Canvas::draw_image`, which
//! *does* apply the current path clip per pixel.

use skia_rs_safe::canvas::{Canvas, Surface};
use skia_rs_safe::codec::Image;
use skia_rs_safe::core::Color;

/// CSS's blur *radius* is twice the Gaussian standard deviation.
#[must_use]
pub fn sigma_for_blur_radius(blur_px: f32) -> f32 {
    if blur_px.is_finite() && blur_px > 0.0 {
        blur_px / 2.0
    } else {
        0.0
    }
}

/// The box-pass radius that approximates a Gaussian of `sigma`.
///
/// Skia's rule: `radius = floor(sigma * 3 * sqrt(2*pi) / 4 + 0.5)`, i.e.
/// about `1.88 * sigma`, run three times.
#[must_use]
pub fn box_blur_radius(sigma: f32) -> i32 {
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let scale = 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0;
    let r = sigma.mul_add(scale, 0.5).floor();
    if r.is_finite() {
        r.clamp(0.0, 512.0) as i32
    } else {
        0
    }
}

/// The three box-pass radii that variance-match a Gaussian of `sigma`.
///
/// [`box_blur_radius`]'s "one radius, run three times" rule is Skia's own
/// shorthand for *large* blurs, where losing a pixel of precision to
/// rounding is invisible; splitting `sigma` unevenly across the three
/// passes (Kutskir's `boxesForGauss`: an ideal box width
/// `w = sqrt(12*sigma^2/3 + 1)`, floored to the nearest odd integer below
/// and above, weighted by how far the ideal width sits between them) is
/// the standard construction that keeps the three-pass approximation close
/// to a true Gaussian at *every* sigma, small ones included. That matters
/// here specifically because a separable box kernel's reach is a square
/// (Chebyshev) neighbourhood, not a disk: outside a rounded corner, the
/// uneven, tighter split still over-reaches a true Gaussian's tail less
/// than three equal, rounded-up passes did, which is what a small
/// `box-shadow` blur next to a `border-radius` corner needs to stay clear
/// at a sample point a true Gaussian would have left fully transparent.
#[must_use]
fn box_radii_for_gauss(sigma: f32) -> [i32; 3] {
    if !sigma.is_finite() || sigma <= 0.0 {
        return [0, 0, 0];
    }
    let n = 3.0f32;
    let ideal_width = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal_width.floor();
    if (lower as i64).rem_euclid(2) == 0 {
        lower -= 1.0;
    }
    let lower = lower.max(1.0);
    let upper = lower + 2.0;
    let ideal_lower_count = (12.0 * sigma * sigma - n * lower * lower - 4.0 * n * lower - 3.0 * n)
        / (-4.0 * lower - 4.0);
    let lower_count = if ideal_lower_count.is_finite() {
        ideal_lower_count.round().clamp(0.0, n) as i32
    } else {
        0
    };
    let to_radius = |width: f32| (((width - 1.0) / 2.0).max(0.0).clamp(0.0, 512.0)) as i32;
    let (r_lower, r_upper) = (to_radius(lower), to_radius(upper));
    std::array::from_fn(|i| {
        if (i as i32) < lower_count {
            r_lower
        } else {
            r_upper
        }
    })
}

/// Blur a premultiplied RGBA buffer in place.
///
/// Three box passes per axis, sized by [`box_radii_for_gauss`] rather than
/// one radius repeated three times: three passes of the *same full* radius
/// would sum their reach to `3 * radius` (each pass's reach adds, since
/// they run in sequence), tripling the intended spread. Edges are clamped
/// (the buffer is assumed to already carry the padding the caller wants).
pub fn blur_premul_rgba(pixels: &mut [u8], width: i32, height: i32, stride: usize, sigma: f32) {
    let radii = box_radii_for_gauss(sigma);
    if radii == [0, 0, 0] || width <= 0 || height <= 0 {
        return;
    }
    let (w, h) = (width as usize, height as usize);
    if stride < w * 4 || pixels.len() < stride * h {
        return;
    }
    let mut scratch = vec![0u8; stride * h];

    for pass_radius in radii {
        let r = pass_radius.max(0) as usize;
        box_pass_horizontal(pixels, &mut scratch, w, h, stride, r);
        box_pass_vertical(&scratch, pixels, w, h, stride, r);
    }
}

/// One horizontal box pass, `src` -> `dst`.
fn box_pass_horizontal(src: &[u8], dst: &mut [u8], w: usize, h: usize, stride: usize, r: usize) {
    for y in 0..h {
        let row = y * stride;
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            let mut sum = [0u32; 4];
            for sx in lo..=hi {
                let o = row + sx * 4;
                sum[0] += u32::from(src[o]);
                sum[1] += u32::from(src[o + 1]);
                sum[2] += u32::from(src[o + 2]);
                sum[3] += u32::from(src[o + 3]);
            }
            // Edge pixels see a shorter window: the divisor is deliberately
            // the number of samples actually taken, not the constant `2r+1`,
            // so a uniform region stays uniform (the flat-core rule).
            let count = (hi - lo + 1) as u32;
            let o = row + x * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
}

/// One vertical box pass, `src` -> `dst`.
fn box_pass_vertical(src: &[u8], dst: &mut [u8], w: usize, h: usize, stride: usize, r: usize) {
    for x in 0..w {
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            let mut sum = [0u32; 4];
            for sy in lo..=hi {
                let o = sy * stride + x * 4;
                sum[0] += u32::from(src[o]);
                sum[1] += u32::from(src[o + 1]);
                sum[2] += u32::from(src[o + 2]);
                sum[3] += u32::from(src[o + 3]);
            }
            let count = (hi - lo + 1) as u32;
            let o = y * stride + x * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
}

/// Render `draw` into a transparent `width` x `height` surface, blur it and
/// snapshot it.
///
/// `None` for a degenerate size or if the surface cannot be allocated.
pub fn blurred_image(
    width: i32,
    height: i32,
    sigma: f32,
    draw: impl FnOnce(&mut Canvas<'_>),
) -> Option<Image> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut surface = Surface::new_raster_n32_premul(width, height)?;
    {
        let mut canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        draw(&mut canvas);
    }
    let stride = surface.row_bytes();
    blur_premul_rgba(surface.pixels_mut(), width, height, stride, sigma);
    surface.make_image_snapshot()
}

#[cfg(test)]
mod tests {
    use super::{blur_premul_rgba, blurred_image, box_blur_radius, sigma_for_blur_radius};
    use skia_rs_safe::core::{Color, Rect};
    use skia_rs_safe::paint::{Paint, Style};

    /// A 21x21 opaque-white buffer with a hole, so the blur has something to
    /// spread. Premultiplied RGBA, 4 bytes per pixel.
    fn buffer(w: i32, h: i32) -> (Vec<u8>, usize) {
        let stride = (w as usize) * 4;
        (vec![0u8; stride * h as usize], stride)
    }

    fn set(px: &mut [u8], stride: usize, x: i32, y: i32, v: [u8; 4]) {
        let o = y as usize * stride + x as usize * 4;
        px[o..o + 4].copy_from_slice(&v);
    }

    fn get(px: &[u8], stride: usize, x: i32, y: i32) -> [u8; 4] {
        let o = y as usize * stride + x as usize * 4;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    }

    #[test]
    fn css_blur_radius_is_twice_the_gaussian_sigma() {
        // CSS Backgrounds L3 §box-shadow: the blur radius is twice the
        // standard deviation. Mutation check: using blur as sigma directly
        // doubles every shadow's spread.
        assert_eq!(sigma_for_blur_radius(0.0), 0.0);
        assert_eq!(sigma_for_blur_radius(8.0), 4.0);
        assert_eq!(sigma_for_blur_radius(-3.0), 0.0, "negative blur is zero");
        assert_eq!(sigma_for_blur_radius(f32::NAN), 0.0);
    }

    #[test]
    fn a_zero_sigma_blur_leaves_every_byte_alone() {
        // Mutation check: running the box passes with radius 0 anyway still
        // rounds bytes; this catches an off-by-one in the early return.
        let (mut px, stride) = buffer(5, 5);
        set(&mut px, stride, 2, 2, [200, 100, 50, 255]);
        let before = px.clone();
        blur_premul_rgba(&mut px, 5, 5, stride, 0.0);
        assert_eq!(px, before);
    }

    #[test]
    fn a_blurred_flat_core_keeps_its_exact_value() {
        // The exactness rule: a pixel far enough inside a uniform region is
        // unchanged by the blur, so shadow tests may assert it exactly.
        // Mutation check: a normalisation error (dividing by the wrong
        // window size) moves 255 off 255 here.
        let (mut px, stride) = buffer(41, 41);
        for y in 0..41 {
            for x in 0..41 {
                set(&mut px, stride, x, y, [255, 0, 0, 255]);
            }
        }
        blur_premul_rgba(&mut px, 41, 41, stride, 3.0);
        assert_eq!(get(&px, stride, 20, 20), [255, 0, 0, 255]);
    }

    #[test]
    fn a_blur_spreads_alpha_outside_the_original_shape() {
        // Mutation check: blurring only horizontally leaves (20, 12) at 0.
        let (mut px, stride) = buffer(41, 41);
        for y in 18..23 {
            for x in 18..23 {
                set(&mut px, stride, x, y, [255, 255, 255, 255]);
            }
        }
        blur_premul_rgba(&mut px, 41, 41, stride, 4.0);
        assert!(get(&px, stride, 20, 12)[3] > 0, "alpha spread upwards");
        assert!(get(&px, stride, 12, 20)[3] > 0, "alpha spread leftwards");
        assert!(get(&px, stride, 20, 20)[3] < 255, "the core was diluted");
    }

    #[test]
    fn box_blur_radius_follows_skias_sigma_to_radius_rule() {
        // SkBlurMask::ConvertSigmaToRadius-equivalent rounding. Mutation
        // check: truncating instead of rounding gives 4 for sigma 2.0.
        assert_eq!(box_blur_radius(0.0), 0);
        assert_eq!(box_blur_radius(1.0), 2);
        assert_eq!(box_blur_radius(2.0), 4);
    }

    #[test]
    fn blurred_image_renders_and_returns_a_bitmap() {
        // Mutation check: forgetting to clear the offscreen surface leaves
        // uninitialised alpha and the outside-the-shape assertion fails.
        let image = blurred_image(40, 40, 3.0, |canvas| {
            let mut paint = Paint::new();
            paint.set_color32(Color(0xFF00_0000));
            paint.set_style(Style::Fill);
            paint.set_anti_alias(false);
            canvas.draw_rect(&Rect::from_xywh(15.0, 15.0, 10.0, 10.0), &paint);
        })
        .expect("blurred image");
        assert_eq!(image.width(), 40);
        assert_eq!(image.height(), 40);
        let outside = image.read_pixel(1, 1).expect("pixel");
        assert!(outside.a < 0.01, "far outside the blurred square is clear");
        let inside = image.read_pixel(20, 20).expect("pixel");
        assert!(inside.a > 0.5, "the blurred core is still mostly opaque");
    }

    #[test]
    fn blur_never_panics_on_hostile_inputs() {
        for &sigma in &[f32::NAN, f32::INFINITY, -1.0, 0.0, 1.0e9] {
            let (mut px, stride) = buffer(3, 3);
            blur_premul_rgba(&mut px, 3, 3, stride, sigma);
            let _ = box_blur_radius(sigma);
            let _ = sigma_for_blur_radius(sigma);
        }
        // Degenerate geometry must not index out of bounds.
        let (mut empty, stride) = buffer(0, 0);
        blur_premul_rgba(&mut empty, 0, 0, stride, 2.0);
        assert!(blurred_image(0, 0, 2.0, |_| {}).is_none());
        assert!(blurred_image(-4, 8, 2.0, |_| {}).is_none());
    }
}
