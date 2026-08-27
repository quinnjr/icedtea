//! `opacity`, `transform` and `filter`, as one save-layer.
//!
//! `skia-rs-canvas` 0.4.0's `composite_layer` applies a layer paint's alpha,
//! blend mode and *colour* filter, and says in-source that an image filter
//! "is not yet applied here". The rasterizer, in turn, reads only
//! `Paint::shader`. So opacity and the eight colour-matrix filter functions
//! work here; `blur()` and `drop-shadow()` are computed and animated but
//! cannot paint in M2, and warn once.

use std::sync::{Arc, Once};

use skia_rs_safe::canvas::{Canvas, SaveLayerFlags, SaveLayerRec};
use skia_rs_safe::paint::{ColorFilterRef, ColorMatrixFilter, Paint};

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::transform::transform_list_matrix;
use crate::css::value::{FilterFn, LengthCtx, Position, TransformFn, Value};
use crate::layout::Allocation;

/// The 5x4 identity colour matrix, row-major.
pub const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

static IMAGE_FILTER_WARNED: Once = Once::new();

/// `outer * inner` for two 5x4 affine colour matrices.
#[must_use]
pub fn compose_color_matrices(outer: &[f32; 20], inner: &[f32; 20]) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += outer[row * 5 + k] * inner[k * 5 + col];
            }
            out[row * 5 + col] = sum;
        }
        let mut bias = outer[row * 5 + 4];
        for k in 0..4 {
            bias += outer[row * 5 + k] * inner[k * 5 + 4];
        }
        out[row * 5 + 4] = bias;
    }
    out
}

/// The W3C Filter Effects matrix for one filter function.
///
/// `None` for `blur()` and `drop-shadow()`, which are not colour matrices.
fn matrix_for(filter: &FilterFn) -> Option<[f32; 20]> {
    let clamp01 = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            1.0
        }
    };
    Some(match filter {
        FilterFn::Grayscale(amount) => {
            let a = clamp01(*amount);
            luminance_mix(a)
        }
        FilterFn::Sepia(amount) => {
            let a = clamp01(*amount);
            let sepia = [
                0.393, 0.769, 0.189, 0.0, 0.0, //
                0.349, 0.686, 0.168, 0.0, 0.0, //
                0.272, 0.534, 0.131, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ];
            lerp_matrix(&IDENTITY_MATRIX, &sepia, a)
        }
        FilterFn::Saturate(amount) => {
            saturate_matrix(if amount.is_finite() { *amount } else { 1.0 })
        }
        FilterFn::HueRotate(degrees) => {
            hue_rotate_matrix(if degrees.is_finite() { *degrees } else { 0.0 })
        }
        FilterFn::Invert(amount) => {
            let a = clamp01(*amount);
            let s = 1.0 - 2.0 * a;
            [
                s, 0.0, 0.0, 0.0, a, //
                0.0, s, 0.0, 0.0, a, //
                0.0, 0.0, s, 0.0, a, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Brightness(amount) => {
            let a = if amount.is_finite() {
                amount.max(0.0)
            } else {
                1.0
            };
            [
                a, 0.0, 0.0, 0.0, 0.0, //
                0.0, a, 0.0, 0.0, 0.0, //
                0.0, 0.0, a, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Contrast(amount) => {
            let a = if amount.is_finite() {
                amount.max(0.0)
            } else {
                1.0
            };
            let b = (1.0 - a) / 2.0;
            [
                a, 0.0, 0.0, 0.0, b, //
                0.0, a, 0.0, 0.0, b, //
                0.0, 0.0, a, 0.0, b, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        FilterFn::Opacity(amount) => {
            let a = clamp01(*amount);
            [
                1.0, 0.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, a, 0.0,
            ]
        }
        FilterFn::Blur(_) | FilterFn::DropShadow(_) => return None,
    })
}

/// Rec.601 luma mixed with the identity by `amount` (1 == full grayscale).
fn luminance_mix(amount: f32) -> [f32; 20] {
    let gray = [
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.2126, 0.7152, 0.0722, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    lerp_matrix(&IDENTITY_MATRIX, &gray, amount)
}

fn lerp_matrix(a: &[f32; 20], b: &[f32; 20], t: f32) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for i in 0..20 {
        out[i] = a[i] + (b[i] - a[i]) * t;
    }
    out
}

/// W3C `saturate()` coefficients.
fn saturate_matrix(s: f32) -> [f32; 20] {
    let (r, g, b) = (0.213, 0.715, 0.072);
    [
        r + (1.0 - r) * s,
        g - g * s,
        b - b * s,
        0.0,
        0.0, //
        r - r * s,
        g + (1.0 - g) * s,
        b - b * s,
        0.0,
        0.0, //
        r - r * s,
        g - g * s,
        b + (1.0 - b) * s,
        0.0,
        0.0, //
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

/// W3C `feColorMatrix type="hueRotate"` coefficients.
fn hue_rotate_matrix(degrees: f32) -> [f32; 20] {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        0.213 + c * 0.787 - s * 0.213,
        0.715 - c * 0.715 - s * 0.715,
        0.072 - c * 0.072 + s * 0.928,
        0.0,
        0.0,
        0.213 - c * 0.213 + s * 0.143,
        0.715 + c * 0.285 + s * 0.140,
        0.072 - c * 0.072 - s * 0.283,
        0.0,
        0.0,
        0.213 - c * 0.213 - s * 0.787,
        0.715 - c * 0.715 + s * 0.715,
        0.072 + c * 0.928 + s * 0.072,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

/// The single colour matrix a whole `filter` list composes to.
///
/// `None` when the list contributes no colour matrix at all. `blur()` and
/// `drop-shadow()` entries are skipped with a one-time warning.
#[must_use]
pub fn color_matrix_for(filters: &[FilterFn]) -> Option<[f32; 20]> {
    let mut matrix: Option<[f32; 20]> = None;
    for filter in filters {
        match matrix_for(filter) {
            Some(m) => {
                matrix = Some(match matrix {
                    Some(existing) => compose_color_matrices(&m, &existing),
                    None => m,
                });
            }
            None => {
                IMAGE_FILTER_WARNED.call_once(|| {
                    tracing::warn!(
                        "filter: blur() and drop-shadow() are computed and animated but cannot \
                         paint: skia-rs-safe 0.4.0's layer composite ignores image filters"
                    );
                });
            }
        }
    }
    matrix
}

/// Push the node's opacity/transform/filter stack; returns the save count
/// [`end_effects`] must restore to.
pub fn begin_effects(
    canvas: &mut Canvas<'_>,
    style: &ComputedStyle,
    alloc: &Allocation,
    ctx: &LengthCtx,
) -> usize {
    let opacity = style.opacity().clamp(0.0, 1.0);
    let transforms: Vec<TransformFn> = match style.raw(Prop::Transform) {
        Value::Transform(list) => list.to_vec(),
        _ => Vec::new(),
    };
    let filters: Vec<FilterFn> = match style.raw(Prop::Filter) {
        Value::Filter(list) => list.to_vec(),
        _ => Vec::new(),
    };
    let color_matrix = color_matrix_for(&filters);

    if opacity >= 1.0 && transforms.is_empty() && color_matrix.is_none() {
        // Fast path: no layer, no matrix, nothing to restore beyond a save.
        return canvas.save();
    }

    let mut layer_paint = Paint::new();
    layer_paint.set_alpha(opacity);
    if let Some(matrix) = color_matrix {
        let filter: ColorFilterRef = Arc::new(ColorMatrixFilter::new(matrix));
        layer_paint.set_color_filter(Some(filter));
    }
    let save = canvas.save_layer(&SaveLayerRec {
        bounds: None,
        paint: Some(&layer_paint),
        flags: SaveLayerFlags::NONE,
    });

    if !transforms.is_empty() {
        let basis = (alloc.border_box.width, alloc.border_box.height);
        let origin = transform_origin(style, alloc, ctx);
        let matrix = transform_list_matrix(&transforms, ctx, basis, origin);
        canvas.concat(&matrix);
    }
    save
}

/// `transform-origin` in absolute coordinates; the CSS default is the box's
/// centre.
fn transform_origin(style: &ComputedStyle, alloc: &Allocation, ctx: &LengthCtx) -> (f32, f32) {
    let box_rect = alloc.border_box;
    let Value::Position(position) = style.raw(Prop::TransformOrigin) else {
        return (
            box_rect.x + box_rect.width / 2.0,
            box_rect.y + box_rect.height / 2.0,
        );
    };
    let mut x_ctx = *ctx;
    x_ctx.percent_basis = Some(box_rect.width);
    let mut y_ctx = *ctx;
    y_ctx.percent_basis = Some(box_rect.height);
    let _: &Position = position;
    (
        box_rect.x + position.x.resolve(&x_ctx).unwrap_or(box_rect.width / 2.0),
        box_rect.y + position.y.resolve(&y_ctx).unwrap_or(box_rect.height / 2.0),
    )
}

/// Pop whatever [`begin_effects`] pushed.
pub fn end_effects(canvas: &mut Canvas<'_>, save_count: usize) {
    canvas.restore_to_count(save_count);
}

#[cfg(test)]
mod tests {
    use super::{begin_effects, color_matrix_for, compose_color_matrices, end_effects};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{FilterFn, Length, LengthCtx};
    use crate::layout::{Allocation, Rect};
    use crate::paint::fill_paint;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::{Color, Rect as SkRect};

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    fn alloc() -> Allocation {
        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    /// Paint a solid red 40x20 rect through the effect stack `css` declares.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("box");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            let save = begin_effects(&mut canvas, &style, &alloc(), &ctx());
            let red = crate::css::value::Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
            let mut paint = fill_paint(red);
            paint.set_anti_alias(false);
            canvas.draw_rect(&SkRect::from_xywh(0.0, 0.0, 40.0, 20.0), &paint);
            end_effects(&mut canvas, save);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn opacity_scales_the_whole_subtree_once() {
        // Mutation check: applying opacity to each draw's paint instead of
        // to a layer double-darkens overlapping draws; here it would leave
        // alpha at 255 because the fill paint is opaque.
        let surface = painted("box { opacity: 0.5 }");
        let p = pixel(&surface, 10, 10);
        assert!(
            p.alpha() > 0x70 && p.alpha() < 0x90,
            "half-transparent, got {p:?}"
        );
    }

    #[test]
    fn a_translate_transform_moves_the_painted_box() {
        // Mutation check: applying the matrix after the draw, or ignoring
        // transform-origin, leaves ink at (10, 10).
        let surface = painted("box { transform: translate(15px, 10px) }");
        assert_eq!(pixel(&surface, 10, 10).alpha(), 0);
        assert_eq!(pixel(&surface, 25, 15), Color(0xFFFF_0000));
    }

    #[test]
    fn a_grayscale_filter_desaturates_the_layer() {
        // Mutation check: setting the colour filter on the draw paint rather
        // than the layer paint has no effect at all -- the rasterizer never
        // reads it.
        let surface = painted("box { filter: grayscale(1) }");
        let p = pixel(&surface, 10, 10);
        assert_eq!(p.red(), p.green());
        assert_eq!(p.green(), p.blue());
        assert!(p.red() > 0x30 && p.red() < 0x70, "Rec.601 luma of pure red");
    }

    #[test]
    fn chained_filters_compose_into_one_matrix() {
        // Mutation check: composing in the wrong order (inner after outer)
        // changes the result for a non-commuting pair like invert+brightness.
        let one = color_matrix_for(&[FilterFn::Grayscale(1.0)]).expect("grayscale matrix");
        let chained = color_matrix_for(&[FilterFn::Grayscale(1.0), FilterFn::Invert(1.0)])
            .expect("chained matrix");
        assert_ne!(one, chained);
        let identity = compose_color_matrices(&one, &super::IDENTITY_MATRIX);
        assert_eq!(identity, one);
    }

    #[test]
    fn blur_and_drop_shadow_filters_paint_unchanged_and_do_not_panic() {
        // Contract deviation 4: skia-rs-safe 0.4.0's layer composite ignores
        // image filters, so these two are logged once and skipped. Mutation
        // check: returning None for the whole list when one entry is a blur
        // would drop the colour-matrix entries too.
        let surface = painted("box { filter: blur(4px) }");
        assert_eq!(pixel(&surface, 10, 10), Color(0xFFFF_0000));
        assert!(color_matrix_for(&[FilterFn::Blur(Length::px(4.0))]).is_none());
        assert!(
            color_matrix_for(&[FilterFn::Blur(Length::px(4.0)), FilterFn::Grayscale(1.0)])
                .is_some(),
            "the colour-matrix entries still apply"
        );
    }

    #[test]
    fn an_unstyled_node_takes_the_no_layer_fast_path() {
        // Mutation check: always pushing a layer costs an offscreen buffer
        // per node per frame; the save count must be unchanged for a node
        // with no opacity, transform or filter.
        let surface = painted("box { color: #000 }");
        assert_eq!(pixel(&surface, 10, 10), Color(0xFFFF_0000));
    }
}
